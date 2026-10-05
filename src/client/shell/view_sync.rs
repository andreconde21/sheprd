//! andreconde fork (sheprd): the shared sidebar view, for apps that mirror sheprd (contract v1,
//! `docs/sheprd-view-sync.md` in conductore-mobile).
//!
//! Opt-in with `share_view = true` in sidebar.toml. On this machine (the hub) sheprd then:
//! - writes `~/.local/state/sheprd/view.json` (layout, per-agent presence and marks, order,
//!   focus) when it changes and at least every 30 s; `sheprd-msg relay` copies it to the other
//!   machines with their own `self` key;
//! - drains `~/.local/state/sheprd/view-updates.jsonl` (marks written by such apps, collected
//!   here by the relay from the other machines) and applies them to sidebar.toml.

use super::projects::{self, ProjectLayout};
use super::ClientEndpointStatus;
use super::{ClientEndpointId, ClientShellEndpoint};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

const MAX_AGENTS: usize = 2000;
const MAX_LINE: usize = 1024;
const STALE_LOCK: Duration = Duration::from_secs(10);

fn state_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/state/sheprd"))
}

/// The hub's sheprd-msg name (`name` in sheprd-msg.toml), else the hostname.
fn hub_name() -> String {
    let configured = std::fs::read_to_string(crate::config::config_dir().join("sheprd-msg.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| table.get("name")?.as_str().map(str::to_owned));
    configured
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "hub".to_owned())
}

fn presence_name(presence: projects::Presence) -> &'static str {
    match presence {
        projects::Presence::Blocked => "blocked",
        projects::Presence::Unread => "unread",
        projects::Presence::Done => "done",
        projects::Presence::Working => "working",
        projects::Presence::Idle => "idle",
    }
}

/// The view as contract v1 describes it, without `updated` (stamped when written).
pub(super) fn view_json(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
) -> serde_json::Value {
    let layout = projects::layout();
    let mut shown = serde_json::to_value(&layout).unwrap_or_default();
    if let Some(map) = shown.as_object_mut() {
        for private in ["unread", "dismissed", "kept", "share_view"] {
            map.remove(private);
        }
    }
    let mut agents = BTreeMap::new();
    let mut focus = None;
    for endpoint in endpoints
        .iter()
        .filter(|endpoint| endpoint.status == ClientEndpointStatus::Online)
    {
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        for agent in &snapshot.agents {
            let key = projects::agent_key(endpoint, &agent.pane_id);
            if agent.focused && endpoint.endpoint_id == *active_endpoint_id {
                focus = Some(key.clone());
            }
            let seq = agent.state_change_seq;
            agents.insert(
                key.clone(),
                serde_json::json!({
                    "presence": presence_name(layout.presence(&key, seq, agent.agent_status)),
                    "state_seq": seq,
                    "unread": layout.is_unread(&key),
                    "dismissed": layout.is_dismissed(&key, seq),
                    "kept": layout.is_kept(&key),
                }),
            );
        }
    }
    // Agents sheprd doesn't see right now but that still carry a mark.
    let marked = layout
        .unread
        .iter()
        .chain(&layout.kept)
        .cloned()
        .chain(
            layout
                .dismissed
                .iter()
                .filter_map(|entry| entry.rsplit_once('@').map(|(key, _)| key.to_owned())),
        )
        .collect::<Vec<_>>();
    for key in marked {
        agents.entry(key.clone()).or_insert_with(|| {
            serde_json::json!({
                "presence": if layout.is_unread(&key) { "unread" } else { "idle" },
                "state_seq": 0,
                "unread": layout.is_unread(&key),
                "dismissed": false,
                "kept": layout.is_kept(&key),
            })
        });
    }
    let agents: serde_json::Map<String, serde_json::Value> =
        agents.into_iter().take(MAX_AGENTS).collect();
    let order = super::sheprd_sidebar::ordered_workspace_keys(endpoints, active_endpoint_id);
    serde_json::json!({
        "version": 1,
        "source": concat!("sheprd (herdr ", env!("CARGO_PKG_VERSION"), ")"),
        "hub": hub_name(),
        "self": "local",
        "layout": shown,
        "agents": agents,
        "order": order,
        "focus": focus,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Writes view.json when it changed, and at least every 30 s so readers can tell sheprd runs.
fn write_view(view: &serde_json::Value) {
    static LAST: OnceLock<std::sync::Mutex<(Option<Instant>, String)>> = OnceLock::new();
    let Some(dir) = state_dir() else { return };
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let body = view.to_string();
    let keepalive = last.0.is_none_or(|at| at.elapsed().as_secs() >= 30);
    if body == last.1 && !keepalive {
        return;
    }
    let mut stamped = view.clone();
    stamped["updated"] = serde_json::json!(unix_now());
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join("view.json");
    let tmp = dir.join("view.json.tmp");
    if std::fs::write(&tmp, stamped.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        *last = (Some(Instant::now()), body);
    }
}

/// One validated write-back line.
#[derive(Debug, PartialEq)]
pub(super) struct Op {
    id: String,
    op: String,
    agent: String,
    state_seq: u64,
}

fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_agent(agent: &str) -> bool {
    let Some((machine, pane)) = agent.split_once('/') else {
        return false;
    };
    (1..=64).contains(&machine.chars().count())
        && !machine.chars().any(|c| c == '/' || c.is_control())
        && (1..=64).contains(&pane.len())
        && pane
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ":._-".contains(c))
}

/// Parses one line of view-updates.jsonl; None (and a log line) for anything invalid.
pub(super) fn parse_op(line: &str) -> Option<Op> {
    if line.len() > MAX_LINE {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("v")?.as_u64()? != 1 {
        return None;
    }
    let id = value.get("id")?.as_str()?;
    let op = value.get("op")?.as_str()?;
    let agent = value.get("agent")?.as_str()?;
    if !valid_id(id)
        || !valid_agent(agent)
        || !["unread", "read", "dismiss", "keep", "unkeep"].contains(&op)
    {
        return None;
    }
    let state_seq = value.get("state_seq").and_then(serde_json::Value::as_u64);
    if op == "dismiss" && state_seq.is_none() {
        return None;
    }
    Some(Op {
        id: id.to_owned(),
        op: op.to_owned(),
        agent: agent.to_owned(),
        state_seq: state_seq.unwrap_or_default(),
    })
}

/// Applies one op. The ops set a state, never toggle it, so a replay is harmless.
pub(super) fn apply(layout: &mut ProjectLayout, op: &Op) {
    let agent = op.agent.as_str();
    match op.op.as_str() {
        "unread" => layout.mark(agent, op.state_seq, true),
        "read" => layout.unread.retain(|key| key != agent),
        "dismiss" => layout.mark(agent, op.state_seq, false),
        "keep" if !layout.is_kept(agent) => layout.kept.push(agent.to_owned()),
        "unkeep" => layout.kept.retain(|key| key != agent),
        _ => {}
    }
}

/// Takes the O_EXCL lock file (contract v1), clearing one older than 10 s. One try per tick:
/// the UI thread never waits on it.
fn take_lock(dir: &std::path::Path) -> Option<PathBuf> {
    let lock = dir.join("view-updates.lock");
    let create = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
    };
    let mut file = match create() {
        Ok(file) => file,
        Err(_) => {
            let stale = std::fs::metadata(&lock)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age > STALE_LOCK);
            if !stale || std::fs::remove_file(&lock).is_err() {
                return None;
            }
            create().ok()?
        }
    };
    let _ = write!(file, "{}", std::process::id());
    Some(lock)
}

/// Reads and empties view-updates.jsonl under the lock; returns its lines.
fn drain_lines() -> Vec<String> {
    let Some(dir) = state_dir() else {
        return Vec::new();
    };
    let path = dir.join("view-updates.jsonl");
    if std::fs::metadata(&path).map_or(true, |meta| meta.len() == 0) {
        return Vec::new();
    }
    let Some(lock) = take_lock(&dir) else {
        return Vec::new();
    };
    let mut text = String::new();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        if file.read_to_string(&mut text).is_ok() {
            let _ = file.set_len(0);
        }
    }
    let _ = std::fs::remove_file(lock);
    text.lines().map(str::to_owned).collect()
}

/// Applies queued write-back lines, skipping ids already applied (the last 500).
fn apply_updates() {
    static SEEN: OnceLock<std::sync::Mutex<VecDeque<String>>> = OnceLock::new();
    let lines = drain_lines();
    if lines.is_empty() {
        return;
    }
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut ops = Vec::new();
    for line in lines.iter().filter(|line| !line.trim().is_empty()) {
        match parse_op(line) {
            Some(op) if !seen.contains(&op.id) => {
                seen.push_back(op.id.clone());
                if seen.len() > 500 {
                    seen.pop_front();
                }
                ops.push(op);
            }
            Some(_) => {}
            None => tracing::warn!("sheprd view sync: skipped an invalid update line"),
        }
    }
    if !ops.is_empty() {
        projects::update(|layout| {
            for op in &ops {
                apply(layout, op);
            }
        });
    }
}

/// Called from the client tick: at most every 2 s, and only when the user opted in.
pub(super) fn tick(endpoints: &[ClientShellEndpoint], active_endpoint_id: &ClientEndpointId) {
    static LAST: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
    if cfg!(test) || !projects::layout().share_view {
        return;
    }
    {
        let mut last = LAST
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(2)) {
            return;
        }
        *last = Some(Instant::now());
    }
    apply_updates();
    write_view(&view_json(endpoints, active_endpoint_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_and_rejects_bad_ones() {
        let ok = r#"{"v":1,"id":"c-1759612401456-08bd","at":1,"from":"conductore","op":"dismiss","agent":"dev/w2:p1","state_seq":41}"#;
        assert_eq!(
            parse_op(ok),
            Some(Op {
                id: "c-1759612401456-08bd".into(),
                op: "dismiss".into(),
                agent: "dev/w2:p1".into(),
                state_seq: 41
            })
        );
        let missing_seq =
            r#"{"v":1,"id":"c-1759612401456-08bd","op":"dismiss","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_op(missing_seq), None);
        let bad_agent = r#"{"v":1,"id":"c-1759612401456-08bd","op":"keep","agent":"dev/w2 p1"}"#;
        assert_eq!(parse_op(bad_agent), None);
        let bad_op = r#"{"v":1,"id":"c-1759612401456-08bd","op":"toggle","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_op(bad_op), None);
        let v2 = r#"{"v":2,"id":"c-1759612401456-08bd","op":"keep","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_op(v2), None);
    }

    #[test]
    fn ops_set_state_and_replay_is_harmless() {
        let mut layout = ProjectLayout::default();
        let op = |op: &str, seq: u64| Op {
            id: "c-12345678".into(),
            op: op.into(),
            agent: "dev/w2:p1".into(),
            state_seq: seq,
        };
        apply(&mut layout, &op("unread", 0));
        apply(&mut layout, &op("unread", 0));
        assert_eq!(layout.unread, vec!["dev/w2:p1".to_owned()]);
        apply(&mut layout, &op("read", 0));
        assert!(layout.unread.is_empty());
        apply(&mut layout, &op("keep", 0));
        apply(&mut layout, &op("keep", 0));
        assert_eq!(layout.kept, vec!["dev/w2:p1".to_owned()]);
        apply(&mut layout, &op("unkeep", 0));
        assert!(layout.kept.is_empty());
        apply(&mut layout, &op("dismiss", 41));
        assert!(layout.is_dismissed("dev/w2:p1", 41));
    }
}
