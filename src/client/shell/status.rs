//! andreconde fork (sheprd): workspace status from `[[status]]` commands (PR, checks, deploy),
//! SHE-100004 (with SHE-100002 folded in). Each command runs in the workspace folder on the
//! workspace's machine (over SSH for remote ones) and prints one JSON line:
//! `{"state":"ok|pending|fail|review|none","text":"PR #212 · checks ✗ 2","url":"…","details":"…"}`.
//!
//! ```toml
//! # ~/.config/herdr/sheprd-refs.toml
//! [[status]]
//! name = "github"
//! command = "sheprd-status-github"   # shipped with sheprd; installed by `sheprd setup`
//! match = ["my-repo"]                # optional: workspace label or folder contains one of these
//! ```
//!
//! A background thread refreshes every workspace that has an agent every 3 minutes; the UI only
//! reads the cache. A failing status counts as "needs you" (attention counter and prefix+u).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{projects, ClientEndpointStatus, ClientShellEndpoint};

const ROUND: Duration = Duration::from_secs(180);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Status {
    pub(super) name: String,
    pub(super) state: String,
    pub(super) text: String,
    pub(super) url: String,
    pub(super) details: String,
}

#[derive(Clone, Debug)]
struct Job {
    key: String,
    label: String,
    machine: Option<String>,
    cwd: String,
}

#[derive(Deserialize, Clone)]
struct Source {
    name: String,
    command: String,
    #[serde(default, rename = "match")]
    matches: Vec<String>,
}

fn sources() -> Vec<Source> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        status: Vec<Source>,
    }
    let path = crate::config::config_dir().join("sheprd-refs.toml");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<Config>(&text).ok())
        .map(|config| config.status)
        .unwrap_or_default()
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static JOBS: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

fn results() -> &'static Mutex<HashMap<String, Vec<Status>>> {
    static RESULTS: OnceLock<Mutex<HashMap<String, Vec<Status>>>> = OnceLock::new();
    RESULTS.get_or_init(Default::default)
}

/// The cached statuses of a workspace (by workspace key).
pub(super) fn statuses(key: &str) -> Vec<Status> {
    results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .cloned()
        .unwrap_or_default()
}

/// Red CI (or any failing status) on this workspace: it needs you.
pub(super) fn failing(key: &str) -> bool {
    results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .is_some_and(|list| list.iter().any(|status| status.state == "fail"))
}

/// Called from the client tick: refreshes the list of workspaces to check (every 30 s) and starts
/// the worker once, when any `[[status]]` source is configured.
pub(super) fn tick(endpoints: &[ClientShellEndpoint]) {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    if cfg!(test) {
        return;
    }
    {
        let mut last = LAST
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let mut list = Vec::new();
    for endpoint in endpoints
        .iter()
        .filter(|endpoint| endpoint.status == ClientEndpointStatus::Online)
    {
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        for workspace in &snapshot.workspaces {
            if !snapshot
                .agents
                .iter()
                .any(|agent| agent.workspace_id == workspace.workspace_id)
            {
                continue;
            }
            list.push(Job {
                key: projects::workspace_key(endpoint, workspace),
                label: workspace.label.clone(),
                machine: (!endpoint.endpoint_id.is_local()).then(|| endpoint.label.clone()),
                cwd: workspace.new_workspace_cwd.clone(),
            });
        }
    }
    *jobs().lock().unwrap_or_else(|e| e.into_inner()) = list;
    static STARTED: std::sync::Once = std::sync::Once::new();
    if !sources().is_empty() {
        STARTED.call_once(|| {
            std::thread::spawn(worker);
        });
    }
}

fn worker() {
    // Each workspace is checked when it first appears and then every ROUND.
    let mut checked: HashMap<String, Instant> = HashMap::new();
    loop {
        let sources = sources();
        let list = jobs().lock().unwrap_or_else(|e| e.into_inner()).clone();
        for job in &list {
            if checked.get(&job.key).is_some_and(|at| at.elapsed() < ROUND) {
                continue;
            }
            checked.insert(job.key.clone(), Instant::now());
            let found = sources
                .iter()
                .filter(|source| source_matches(source, job))
                .filter_map(|source| run(source, job))
                .collect::<Vec<_>>();
            tracing::debug!(workspace = %job.key, cwd = %job.cwd, results = found.len(), "sheprd status round");
            results()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(job.key.clone(), found);
        }
        checked.retain(|key, _| list.iter().any(|job| job.key == *key));
        std::thread::sleep(Duration::from_secs(10));
    }
}

fn source_matches(source: &Source, job: &Job) -> bool {
    source.matches.is_empty()
        || source.matches.iter().any(|needle| {
            let needle = needle.to_lowercase();
            job.label.to_lowercase().contains(&needle) || job.cwd.to_lowercase().contains(&needle)
        })
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn run(source: &Source, job: &Job) -> Option<Status> {
    let mut command = match &job.machine {
        None => {
            let mut command = std::process::Command::new("sh");
            command.arg("-c").arg(&source.command).current_dir(&job.cwd);
            command
        }
        Some(label) => {
            let target = super::work_panel::ssh_target(label)?;
            let script = format!(
                "PATH=\"$HOME/.local/bin:$PATH\"; cd {} && {}",
                shell_quote(&job.cwd),
                source.command
            );
            let mut command = std::process::Command::new("ssh");
            command.args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                &target,
                &script,
            ]);
            command
        }
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
    let mut output = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut output).ok()?;
    parse(&source.name, &output)
}

/// The first JSON line a status command printed; None when it printed none or said "none".
pub(super) fn parse(name: &str, output: &str) -> Option<Status> {
    let value: serde_json::Value = output
        .lines()
        .find_map(|line| serde_json::from_str(line.trim()).ok())?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(if key == "details" { 8000 } else { 200 })
            .collect::<String>()
    };
    let state = field("state");
    if !["ok", "pending", "fail", "review"].contains(&state.as_str()) {
        return None;
    }
    let url = field("url");
    Some(Status {
        name: name.to_owned(),
        state,
        text: field("text"),
        url: if url.starts_with("https://") || url.starts_with("http://") {
            url
        } else {
            String::new()
        },
        details: field("details"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        let status = parse(
            "github",
            "noise\n{\"state\":\"fail\",\"text\":\"PR #2 · checks ✗ 1\",\"url\":\"https://x/2\",\"details\":\"# PR\"}\n",
        )
        .unwrap();
        assert_eq!(
            (status.state.as_str(), status.text.as_str()),
            ("fail", "PR #2 · checks ✗ 1")
        );
        assert_eq!(status.url, "https://x/2");
        assert_eq!(
            parse("github", "{\"state\":\"none\",\"text\":\"no PR\"}"),
            None
        );
        assert_eq!(parse("github", "not json"), None);
        let unsafe_url = parse("x", "{\"state\":\"ok\",\"url\":\"file:///etc/passwd\"}").unwrap();
        assert!(unsafe_url.url.is_empty());
    }

    #[test]
    fn match_rules_use_label_or_folder() {
        let job = Job {
            key: "dev/w1:shop".into(),
            label: "Shop".into(),
            machine: None,
            cwd: "/root/Projects/storefront".into(),
        };
        let source = |matches: &[&str]| Source {
            name: "s".into(),
            command: "true".into(),
            matches: matches.iter().map(|m| m.to_string()).collect(),
        };
        assert!(source_matches(&source(&[]), &job));
        assert!(source_matches(&source(&["shop"]), &job));
        assert!(source_matches(&source(&["storefront"]), &job));
        assert!(!source_matches(&source(&["billing"]), &job));
    }
}
