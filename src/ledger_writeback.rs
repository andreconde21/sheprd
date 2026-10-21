//! andreconde fork (sheprd): writes a done card back to the team's tracker (SHE-100010), so the
//! card file stays the durable record and the ledger a temporary inbox. Two generic ways, both
//! off unless `[ledger.writeback]` configures them; nothing here knows a particular tracker:
//!
//! - a Markdown task file per card (`markdown_path`, e.g. `{root}/{project}/{id}.md`): its
//!   frontmatter `status:` becomes `done_status`, and the decisions plus a dated `Done: summary`
//!   line are appended under `## Decisions`. Every other byte stays as it was. Never committed.
//! - a command (`on_card_done`) that gets the card as JSON on stdin.
//!
//! Runs on a worker task, never on the server loop; the hook is killed after its timeout. The
//! outcome goes back to the card (`LedgerItem::writeback`) through an `AppEvent`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::config::LedgerWritebackConfig;
use crate::work_ledger::{LedgerItem, LedgerWriteback};

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// The hook's stderr kept for the outcome's reason.
const HOOK_STDERR_LIMIT: usize = 4096;
/// An outcome's message is one short line in the panel.
const MAX_MESSAGE_CHARS: usize = 160;

/// The done card as the adapter and the hook see it; also the hook's stdin JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DoneCard {
    pub(crate) id: String,
    pub(crate) title: String,
    /// The configured `done_status`, else "done".
    pub(crate) status: String,
    pub(crate) link: Option<String>,
    pub(crate) summary: String,
    pub(crate) decisions: Vec<String>,
    pub(crate) pane_id: String,
    pub(crate) agent_session: Option<String>,
}

/// Why a write-back failed: the outcome's code and its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub(crate) fn enabled(config: &LedgerWritebackConfig) -> bool {
    config.markdown_path.is_some() || config.on_card_done.is_some()
}

pub(crate) fn done_card(
    config: &LedgerWritebackConfig,
    item: &LedgerItem,
    pane_id: &str,
) -> DoneCard {
    DoneCard {
        id: item.id.clone(),
        title: item.title.clone(),
        status: config
            .done_status
            .clone()
            .filter(|status| !status.trim().is_empty())
            .unwrap_or_else(|| "done".into()),
        link: item.link.clone(),
        summary: item.summary.clone(),
        decisions: item
            .decisions
            .iter()
            .map(|decision| decision.text.clone())
            .collect(),
        pane_id: pane_id.to_owned(),
        agent_session: item.agent_session.clone(),
    }
}

/// Runs the configured write-back for one done card. `date` (YYYY-MM-DD) dates its Done line.
pub(crate) async fn run(
    config: LedgerWritebackConfig,
    card: DoneCard,
    date: String,
    now_ms: u64,
) -> LedgerWriteback {
    let mut written = Vec::new();
    let mut failures = Vec::new();
    if config.markdown_path.is_some() {
        let (config, card) = (config.clone(), card.clone());
        match tokio::task::spawn_blocking(move || write_markdown(&config, &card, &date)).await {
            Ok(Ok(message)) => written.push(message),
            Ok(Err(failure)) => failures.push(failure),
            Err(err) => failures.push(Failure::new("write_failed", err.to_string())),
        }
    }
    if let Some(command) = config.on_card_done.as_deref() {
        let timeout = config
            .timeout_seconds
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_TIMEOUT);
        match run_hook(command, &card, timeout).await {
            Ok(()) => written.push("hook ran".into()),
            Err(failure) => failures.push(failure),
        }
    }
    let outcome = match failures.first() {
        Some(first) => LedgerWriteback {
            ok: false,
            code: first.code.into(),
            // What failed first, then what still went through.
            message: failures
                .iter()
                .map(|failure| failure.message.as_str())
                .chain(written.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("; "),
            at_ms: now_ms,
        },
        None => LedgerWriteback {
            ok: true,
            code: "written".into(),
            message: written.join("; "),
            at_ms: now_ms,
        },
    };
    if !outcome.ok {
        tracing::warn!(card = %card.id, code = %outcome.code, message = %outcome.message, "card write-back failed");
    }
    LedgerWriteback {
        message: one_line(&outcome.message),
        ..outcome
    }
}

fn one_line(text: &str) -> String {
    let line = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_MESSAGE_CHARS {
        return line;
    }
    let mut short = line.chars().take(MAX_MESSAGE_CHARS - 1).collect::<String>();
    short.push('…');
    short
}

/// The card's file: `markdown_path` with `{root}`, `{project}`, `{prefix}` and `{id}` filled in.
fn markdown_path(config: &LedgerWritebackConfig, id: &str) -> Result<PathBuf, Failure> {
    let Some(template) = config.markdown_path.as_deref() else {
        return Err(Failure::new("not_configured", "no markdown_path"));
    };
    // The id is one path component at most ([A-Za-z0-9#._-]); never "." or "..".
    if id.chars().all(|c| c == '.') {
        return Err(Failure::new(
            "invalid_id",
            format!("card id {id} names no file"),
        ));
    }
    let prefix = id.split('-').next().unwrap_or(id);
    let mut path = template.replace("{id}", id).replace("{prefix}", prefix);
    if path.contains("{project}") {
        let Some(project) = config.projects.get(prefix) else {
            return Err(Failure::new(
                "unknown_project",
                format!("no project for prefix {prefix} in ledger.writeback.projects"),
            ));
        };
        path = path.replace("{project}", project);
    }
    if path.contains("{root}") {
        let Some(root) = config.root.as_deref() else {
            return Err(Failure::new(
                "not_configured",
                "markdown_path uses {root} but ledger.writeback.root is not set",
            ));
        };
        path = path.replace("{root}", root);
    }
    Ok(crate::worktree::expand_tilde_path(&path))
}

/// Serializes read-modify-write of card files across write-backs.
static MARKDOWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_markdown(
    config: &LedgerWritebackConfig,
    card: &DoneCard,
    date: &str,
) -> Result<String, Failure> {
    let path = markdown_path(config, &card.id)?;
    let _lock = MARKDOWN_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(Failure::new(
                "missing_file",
                format!("file missing: {}", path.display()),
            ));
        }
        Err(err) => {
            return Err(Failure::new(
                "write_failed",
                format!("cannot read {}: {err}", path.display()),
            ));
        }
    };
    let mut entries = card.decisions.clone();
    if !card.summary.trim().is_empty() {
        entries.push(format!("Done {date}: {}", card.summary.trim()));
    }
    let updated = update_markdown(&content, config.done_status.as_deref(), &entries);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    if updated == content {
        return Ok(format!("{name} already up to date"));
    }
    write_atomically(&path, &updated).map_err(|err| {
        Failure::new(
            "write_failed",
            format!("cannot write {}: {err}", path.display()),
        )
    })?;
    Ok(format!("wrote {name}"))
}

/// Replaces `path` with `content` through a temporary file in the same folder, keeping the
/// file's permissions. A symlink is followed: its target is replaced, not the link.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path)?;
    let permissions = std::fs::metadata(&path)?.permissions();
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("file has no folder"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("path has no file name"))?
        .to_string_lossy();
    let temp = dir.join(format!(".{name}.sheprd-{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(content.as_bytes())?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// A bullet line already in the section for `entry`: the same text after the list marker. A
/// `Done <date>: summary` line matches another date's with the same summary.
fn has_entry(lines: &[String], entry: &str) -> bool {
    let done_summary = entry
        .strip_prefix("Done ")
        .and_then(|rest| rest.split_once(": "))
        .map(|(_, summary)| summary);
    lines.iter().any(|line| {
        let text = line.trim().trim_start_matches(['-', '*', '+']).trim_start();
        text == entry
            || done_summary.is_some_and(|summary| {
                text.strip_prefix("Done ")
                    .and_then(|rest| rest.split_once(": "))
                    .is_some_and(|(_, existing)| existing == summary)
            })
    })
}

fn is_bullet(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ")
}

fn without_ending(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// A YAML scalar for the status: plain when that reads back the same, else double-quoted.
fn yaml_scalar(value: &str, quoted: bool) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '/'))
        && !value.starts_with([' ', '-'])
        && !value.ends_with(' ');
    if plain && !quoted {
        value.to_owned()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
    }
}

/// The card file with its frontmatter `status:` set to `done_status` (added when missing; a
/// file without frontmatter keeps none) and each entry not yet there appended as a bullet under
/// `## Decisions` (created at the end when missing). Every other line keeps its bytes, and new
/// lines use the file's line ending (CRLF when it has any).
pub(crate) fn update_markdown(
    content: &str,
    done_status: Option<&str>,
    entries: &[String],
) -> String {
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = content
        .split_inclusive('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();

    // Frontmatter: `---` on the first line up to the next `---` (or `...`).
    let mut body_start = 0;
    if lines
        .first()
        .is_some_and(|line| without_ending(line) == "---")
    {
        if let Some(end) =
            (1..lines.len()).find(|&index| matches!(without_ending(&lines[index]), "---" | "..."))
        {
            body_start = end + 1;
            if let Some(status) = done_status
                .map(str::trim)
                .filter(|status| !status.is_empty())
            {
                match (1..end).find(|&index| lines[index].starts_with("status:")) {
                    Some(index) => {
                        let line = &lines[index];
                        let ending = &line[without_ending(line).len()..];
                        let old = without_ending(line)["status:".len()..].trim();
                        let quoted = old.starts_with(['"', '\'']);
                        let value = yaml_scalar(status, quoted);
                        let unquoted = old.trim_matches(['"', '\'']);
                        if unquoted != status {
                            lines[index] = format!("status: {value}{ending}");
                        }
                    }
                    None => {
                        lines.insert(
                            end,
                            format!("status: {}{newline}", yaml_scalar(status, false)),
                        );
                        body_start += 1;
                    }
                }
            }
        }
    }

    // The `## Decisions` section: from its heading to the next heading of level 1 or 2.
    let mut fenced = false;
    let mut heading = None;
    let mut section_end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(body_start) {
        let text = without_ending(line);
        if text.trim_start().starts_with("```") || text.trim_start().starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let top_heading = text.starts_with("# ") || text.starts_with("## ");
        match heading {
            None if text.trim_end().eq_ignore_ascii_case("## decisions") => heading = Some(index),
            Some(_) if top_heading => {
                section_end = index;
                break;
            }
            _ => {}
        }
    }

    let section = heading.map_or(&[][..], |heading| &lines[heading + 1..section_end]);
    let mut new_entries = Vec::new();
    for entry in entries {
        let entry = entry.trim();
        if !entry.is_empty() && !has_entry(section, entry) && !new_entries.contains(&entry) {
            new_entries.push(entry);
        }
    }
    if new_entries.is_empty() {
        return lines.concat();
    }
    let bullets = new_entries
        .iter()
        .map(|entry| format!("- {entry}{newline}"))
        .collect::<Vec<_>>();

    let Some(heading) = heading else {
        // No section: a new one at the end, after a blank line.
        let mut out = lines.concat();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(newline);
        }
        if !out.is_empty() && !out.ends_with(&format!("{newline}{newline}")) && out != newline {
            out.push_str(newline);
        }
        out.push_str(&format!("## Decisions{newline}{newline}"));
        out.extend(bullets);
        return out;
    };

    let last_content = (heading + 1..section_end)
        .rev()
        .find(|&index| !lines[index].trim().is_empty());
    let mut insert = Vec::new();
    let at = match last_content {
        Some(index) => {
            if !lines[index].ends_with('\n') {
                lines[index].push_str(newline);
            }
            if !is_bullet(&lines[index]) {
                insert.push(newline.to_owned());
            }
            index + 1
        }
        None => {
            if !lines[heading].ends_with('\n') {
                lines[heading].push_str(newline);
            }
            insert.push(newline.to_owned());
            heading + 1
        }
    };
    insert.extend(bullets);
    // Keep a blank line before the next heading.
    if at < lines.len() && !lines[at].trim().is_empty() {
        insert.push(newline.to_owned());
    }
    lines.splice(at..at, insert);
    lines.concat()
}

/// Runs `on_card_done` with the card as JSON on stdin. Fails on a spawn error, a non-zero exit
/// (with the last line of its stderr) or the timeout, after which it and its children are killed.
async fn run_hook(command: &str, card: &DoneCard, timeout: Duration) -> Result<(), Failure> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let input =
        serde_json::to_vec(card).map_err(|err| Failure::new("hook_failed", err.to_string()))?;
    let mut process = crate::platform::detached_custom_command_process(command);
    process
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    crate::platform::configure_status_command(&mut process);
    let mut process = tokio::process::Command::from(process);
    process.kill_on_drop(true);
    let mut child = process
        .spawn()
        .map_err(|err| Failure::new("hook_failed", format!("hook did not start: {err}")))?;
    // Kills the hook's process group when this returns, timeout included.
    let _guard = crate::platform::StatusCommandGuard::new(&child)
        .map_err(|err| Failure::new("hook_failed", format!("hook did not start: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read its input may close it early; that is its business.
        let _ = tokio::time::timeout(timeout, stdin.write_all(&input)).await;
    }
    // Drained to the end (a chatty hook never blocks on a full pipe); the last bytes are kept.
    let stderr = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut kept = Vec::new();
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                kept.extend_from_slice(&buffer[..read]);
                if kept.len() > HOOK_STDERR_LIMIT {
                    kept.drain(..kept.len() - HOOK_STDERR_LIMIT);
                }
            }
            kept
        })
    });
    let operation = async {
        let status = child.wait().await;
        // Something the hook left running may hold stderr open; do not wait for it long.
        let stderr = match stderr {
            Some(task) => tokio::time::timeout(Duration::from_millis(200), task)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        (status, stderr)
    };
    match tokio::time::timeout(timeout, operation).await {
        Err(_) => Err(Failure::new(
            "hook_failed",
            format!("hook timed out after {}s", timeout.as_secs()),
        )),
        Ok((Err(err), _)) => Err(Failure::new("hook_failed", format!("hook failed: {err}"))),
        Ok((Ok(status), _)) if status.success() => Ok(()),
        Ok((Ok(status), stderr)) => {
            let stderr = String::from_utf8_lossy(&stderr);
            let reason = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_owned();
            let exit = status
                .code()
                .map_or_else(|| status.to_string(), |code| format!("exit {code}"));
            Err(Failure::new(
                "hook_failed",
                if reason.is_empty() {
                    format!("hook failed ({exit})")
                } else {
                    format!("hook failed ({exit}): {reason}")
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> LedgerWritebackConfig {
        LedgerWritebackConfig {
            markdown_path: Some("{root}/{project}/{id}.md".into()),
            root: None,
            projects: [("SHE".to_owned(), "sheprd".to_owned())].into(),
            done_status: Some("verify".into()),
            on_card_done: None,
            timeout_seconds: None,
        }
    }

    fn card(id: &str) -> DoneCard {
        DoneCard {
            id: id.into(),
            title: "Stripe checkout".into(),
            status: "verify".into(),
            link: Some("https://example.test/SHE-1".into()),
            summary: "Checkout live on dev".into(),
            decisions: vec!["Use Checkout".into(), "Webhooks at /stripe".into()],
            pane_id: "w1:p1".into(),
            agent_session: Some("abc".into()),
        }
    }

    fn entries(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    const CARD: &str = "---\nid: SHE-1\ntitle: \"Stripe: checkout\"\nstatus: in-progress\nbadges:\n  - technical\n---\n\n## Description\n\nText  with  spacing \n\n## Decisions\n\nAgreed with André.\n\n## Validation\n\n```yaml\n## Decisions\n```\n";

    #[test]
    fn markdown_changes_only_status_and_decisions() {
        let out = update_markdown(
            CARD,
            Some("verify"),
            &entries(&["Use Checkout", "Done 2026-10-21: live"]),
        );
        assert_eq!(
            out,
            "---\nid: SHE-1\ntitle: \"Stripe: checkout\"\nstatus: verify\nbadges:\n  - technical\n---\n\n## Description\n\nText  with  spacing \n\n## Decisions\n\nAgreed with André.\n\n- Use Checkout\n- Done 2026-10-21: live\n\n## Validation\n\n```yaml\n## Decisions\n```\n"
        );
        // Running it again changes nothing; a later decision lands after the earlier ones.
        assert_eq!(
            update_markdown(
                &out,
                Some("verify"),
                &entries(&["Use Checkout", "Done 2026-10-22: live"])
            ),
            out
        );
        let more = update_markdown(&out, Some("verify"), &entries(&["Second"]));
        assert!(more.contains("- Done 2026-10-21: live\n- Second\n\n## Validation"));
    }

    #[test]
    fn markdown_without_a_section_gets_one_at_the_end() {
        let out = update_markdown(
            "---\nstatus: todo\n---\n\nBody",
            Some("verify"),
            &entries(&["A"]),
        );
        assert_eq!(
            out,
            "---\nstatus: verify\n---\n\nBody\n\n## Decisions\n\n- A\n"
        );
        let out = update_markdown("Body\n\n", None, &entries(&["A"]));
        assert_eq!(out, "Body\n\n## Decisions\n\n- A\n");
        // An empty section at the end of the file.
        let out = update_markdown("Body\n\n## Decisions\n", None, &entries(&["A"]));
        assert_eq!(out, "Body\n\n## Decisions\n\n- A\n");
    }

    #[test]
    fn markdown_status_is_added_quoted_or_left_alone() {
        assert_eq!(
            update_markdown("---\nid: X\n---\nBody\n", Some("verify"), &[]),
            "---\nid: X\nstatus: verify\n---\nBody\n"
        );
        assert_eq!(
            update_markdown("---\nstatus: \"todo\"\n---\n", Some("verify"), &[]),
            "---\nstatus: \"verify\"\n---\n"
        );
        assert_eq!(
            update_markdown("---\nstatus: todo\n---\n", Some("needs: review"), &[]),
            "---\nstatus: \"needs: review\"\n---\n"
        );
        // No frontmatter, no done_status, or the status already set: nothing changes.
        assert_eq!(
            update_markdown("status: todo\n", Some("verify"), &[]),
            "status: todo\n"
        );
        assert_eq!(
            update_markdown("---\nstatus: todo\n---\n", None, &[]),
            "---\nstatus: todo\n---\n"
        );
        assert_eq!(
            update_markdown("---\nstatus: 'verify'\n---\n", Some("verify"), &[]),
            "---\nstatus: 'verify'\n---\n"
        );
        // An indented `status:` belongs to another key.
        assert_eq!(
            update_markdown("---\nmeta:\n  status: x\nstatus: a\n---\n", Some("b"), &[]),
            "---\nmeta:\n  status: x\nstatus: b\n---\n"
        );
    }

    #[test]
    fn markdown_keeps_crlf_line_endings() {
        let card = "---\r\nstatus: todo\r\n---\r\n\r\n## Decisions\r\n\r\n- Old\r\n";
        assert_eq!(
            update_markdown(card, Some("verify"), &entries(&["Old", "New"])),
            "---\r\nstatus: verify\r\n---\r\n\r\n## Decisions\r\n\r\n- Old\r\n- New\r\n"
        );
    }

    #[test]
    fn paths_resolve_through_the_prefix_map() {
        let mut config = config();
        config.root = Some("/data/tasks".into());
        assert_eq!(
            markdown_path(&config, "SHE-100010").unwrap(),
            PathBuf::from("/data/tasks/sheprd/SHE-100010.md")
        );
        assert_eq!(
            markdown_path(&config, "CAL-1").unwrap_err().code,
            "unknown_project"
        );
        assert_eq!(markdown_path(&config, "..").unwrap_err().code, "invalid_id");
        config.markdown_path = Some("/vault/{prefix}/{id}.md".into());
        assert_eq!(
            markdown_path(&config, "CAL-1").unwrap(),
            PathBuf::from("/vault/CAL/CAL-1.md")
        );
        config.root = None;
        config.markdown_path = Some("{root}/{id}.md".into());
        assert_eq!(
            markdown_path(&config, "CAL-1").unwrap_err().code,
            "not_configured"
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sheprd-writeback-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(dir.join("sheprd")).unwrap();
        dir
    }

    #[tokio::test]
    async fn run_writes_the_file_and_reports_missing_ones() {
        let dir = temp_dir("run");
        let mut config = config();
        config.root = Some(dir.display().to_string());
        let path = dir.join("sheprd/SHE-1.md");
        std::fs::write(&path, CARD).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }

        let outcome = run(config.clone(), card("SHE-1"), "2026-10-21".into(), 7).await;
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(
            (
                outcome.code.as_str(),
                outcome.message.as_str(),
                outcome.at_ms
            ),
            ("written", "wrote SHE-1.md", 7)
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("status: verify\n"));
        assert!(written.contains(
            "- Use Checkout\n- Webhooks at /stripe\n- Done 2026-10-21: Checkout live on dev\n"
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.join("sheprd")).unwrap().count(), 1);

        let again = run(config.clone(), card("SHE-1"), "2026-10-22".into(), 8).await;
        assert!(
            again.ok && again.message == "SHE-1.md already up to date",
            "{again:?}"
        );

        let missing = run(config.clone(), card("SHE-2"), "2026-10-21".into(), 9).await;
        assert!(!missing.ok);
        assert_eq!(missing.code, "missing_file");
        assert!(!dir.join("sheprd/SHE-2.md").exists());

        let unknown = run(config, card("CAL-2"), "2026-10-21".into(), 9).await;
        assert_eq!(
            (unknown.ok, unknown.code.as_str()),
            (false, "unknown_project")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_gets_json_and_failures_are_reported() {
        let dir = temp_dir("hook");
        let stdin_path = dir.join("stdin.json");
        let hook = |command: String| LedgerWritebackConfig {
            on_card_done: Some(command),
            timeout_seconds: Some(1),
            ..LedgerWritebackConfig::default()
        };

        let outcome = run(
            hook(format!("cat > '{}'", stdin_path.display())),
            card("SHE-1"),
            "2026-10-21".into(),
            1,
        )
        .await;
        assert!(outcome.ok, "{outcome:?}");
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&stdin_path).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "SHE-1",
                "title": "Stripe checkout",
                "status": "verify",
                "link": "https://example.test/SHE-1",
                "summary": "Checkout live on dev",
                "decisions": ["Use Checkout", "Webhooks at /stripe"],
                "pane_id": "w1:p1",
                "agent_session": "abc",
            })
        );

        // Reading stdin first: a test binary may run with SIGPIPE at its default meanwhile.
        let failed = run(
            hook("cat >/dev/null; echo nope >&2; exit 3".into()),
            card("SHE-1"),
            String::new(),
            1,
        )
        .await;
        assert_eq!(
            (failed.ok, failed.code.as_str(), failed.message.as_str()),
            (false, "hook_failed", "hook failed (exit 3): nope")
        );

        // A missing file does not stop the hook; the outcome names both.
        let both = run(
            LedgerWritebackConfig {
                markdown_path: Some(format!("{}/{{id}}.md", dir.display())),
                ..hook("cat >/dev/null".into())
            },
            card("SHE-404"),
            String::new(),
            1,
        )
        .await;
        assert_eq!((both.ok, both.code.as_str()), (false, "missing_file"));
        assert!(both.message.ends_with(".md; hook ran"), "{both:?}");

        let started = std::time::Instant::now();
        let slow = run(hook("sleep 30".into()), card("SHE-1"), String::new(), 1).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            (slow.ok, slow.message.as_str()),
            (false, "hook timed out after 1s")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writeback_needs_a_path_or_a_command() {
        assert!(!enabled(&LedgerWritebackConfig::default()));
        assert!(enabled(&config()));
    }
}
