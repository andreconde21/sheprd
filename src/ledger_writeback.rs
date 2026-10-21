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

/// What a write-back wrote, so a done card is written again only when it changed since: its
/// title, link, summary and decisions (not who wrote it or where).
pub(crate) fn fingerprint(card: &DoneCard) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    for part in [
        &card.title,
        card.link.as_deref().unwrap_or_default(),
        &card.summary,
    ]
    .into_iter()
    .chain(card.decisions.iter().map(String::as_str))
    {
        // Length-prefixed, so moving text between fields changes the result.
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Which parts of a write-back to run. A part not run counts as done before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Parts {
    pub(crate) markdown: bool,
    pub(crate) hook: bool,
}

impl Parts {
    pub(crate) const ALL: Self = Self {
        markdown: true,
        hook: true,
    };

    /// What the next `done` has to run after `last`: everything when the card changed since
    /// (or never ran), nothing after a full success, else only the parts that did not succeed,
    /// so a hook that posted once is not run again because the file was missing.
    pub(crate) fn after(last: Option<&LedgerWriteback>, fingerprint: &str) -> Option<Self> {
        let Some(last) = last.filter(|last| last.fingerprint == fingerprint) else {
            return Some(Self::ALL);
        };
        if last.ok {
            return None;
        }
        Some(Self {
            markdown: last.markdown != Some(true),
            hook: last.hook != Some(true),
        })
    }
}

/// Runs the configured write-back for one done card, or the `parts` of it still to do.
/// `date` (YYYY-MM-DD) dates its Done line.
pub(crate) async fn run(
    config: LedgerWritebackConfig,
    card: DoneCard,
    date: String,
    now_ms: u64,
    parts: Parts,
) -> LedgerWriteback {
    let mut written = Vec::new();
    let mut failures = Vec::new();
    let mut markdown = None;
    let mut hook = None;
    if config.markdown_path.is_some() && !parts.markdown {
        written.push("file written before".to_owned());
        markdown = Some(true);
    } else if config.markdown_path.is_some() {
        let (config, card) = (config.clone(), card.clone());
        let result = match tokio::task::spawn_blocking(move || {
            write_markdown(&config, &card, &date)
        })
        .await
        {
            Ok(result) => result,
            Err(err) => Err(Failure::new("write_failed", err.to_string())),
        };
        markdown = Some(result.is_ok());
        match result {
            Ok(message) => written.push(message),
            Err(failure) => failures.push(failure),
        }
    }
    if let Some(command) = config.on_card_done.as_deref() {
        if parts.hook {
            let timeout = config
                .timeout_seconds
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_TIMEOUT);
            let result = run_hook(command, &card, timeout).await;
            hook = Some(result.is_ok());
            match result {
                Ok(()) => written.push("hook ran".into()),
                Err(failure) => failures.push(failure),
            }
        } else {
            written.push("hook ran before".to_owned());
            hook = Some(true);
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
            fingerprint: fingerprint(&card),
            markdown,
            hook,
        },
        None => LedgerWriteback {
            ok: true,
            code: "written".into(),
            message: written.join("; "),
            at_ms: now_ms,
            fingerprint: fingerprint(&card),
            markdown,
            hook,
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
    // Said, so "wrote" never hides that the tracker still shows the old status.
    let note = if updated.status_set {
        ""
    } else {
        " (no frontmatter: status not set)"
    };
    if updated.content == content {
        return Ok(format!("{name} already up to date{note}"));
    }
    write_atomically(&path, &updated.content).map_err(|err| {
        Failure::new(
            "write_failed",
            format!("cannot write {}: {err}", path.display()),
        )
    })?;
    Ok(format!("wrote {name}{note}"))
}

/// Replaces `path` with `content` through a temporary file in the same folder, keeping the
/// file's permissions and, where the platform has them and the server may, its owner (a
/// server running as root must not leave the user's card owned by root). A symlink is
/// followed: its target is replaced, not the link.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&path)?;
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
        if let Err(err) = crate::platform::copy_file_owner(&metadata, &file) {
            tracing::debug!(%err, path = %path.display(), "card file keeps the server's owner");
        }
        // After the owner: changing it may clear set-id bits.
        file.set_permissions(metadata.permissions())?;
        file.sync_all()?;
        drop(file);
        crate::platform::replace_file(&temp, &path)
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

/// An ATX heading's level and title (`### Decisions ##` is level 3, "Decisions").
fn heading(line: &str) -> Option<(usize, &str)> {
    let text = without_ending(line);
    let trimmed = text.trim_start_matches(' ');
    if text.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    let rest = &trimmed[level..];
    if !(1..=6).contains(&level) || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let title = rest.trim();
    // A closing run of '#' after a space is not part of the title.
    let title = match title.trim_end_matches('#') {
        stripped
            if stripped.len() != title.len()
                && (stripped.is_empty() || stripped.ends_with([' ', '\t'])) =>
        {
            stripped.trim_end()
        }
        _ => title,
    };
    Some((level, title))
}

/// A Decisions heading's title: "Decisions", optional spaces, then the end, "(" or ":", in any
/// case (`## Decisions (orchestrator + André, 07/10)`). Not "Decisions needed": those list open
/// questions, not decisions taken.
fn is_decisions_title(title: &str) -> bool {
    let Some(word) = title
        .get(..9)
        .filter(|word| word.eq_ignore_ascii_case("decisions"))
    else {
        return false;
    };
    title[word.len()..]
        .trim_start_matches([' ', '\t'])
        .chars()
        .next()
        .is_none_or(|next| matches!(next, '(' | ':'))
}

/// The lines outside fenced code and HTML comments: where a heading can be.
fn markdown_lines(lines: &[String], from: usize) -> Vec<bool> {
    let mut outside = vec![false; lines.len()];
    let mut fence: Option<(char, usize)> = None;
    let mut comment = false;
    for (index, line) in lines.iter().enumerate().skip(from) {
        let text = without_ending(line).trim_start();
        if comment {
            comment = !text.contains("-->");
            continue;
        }
        let marker = text.chars().next().filter(|&c| c == '`' || c == '~');
        if let Some(marker) = marker {
            let run = text.chars().take_while(|&c| c == marker).count();
            if run >= 3 {
                fence = match fence {
                    None => Some((marker, run)),
                    Some((open, length)) if open == marker && run >= length => None,
                    open => open,
                };
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }
        // A comment opens only at the start of a line: `<!--` inside inline code is text.
        if let Some(rest) = text.strip_prefix("<!--") {
            comment = !rest.contains("-->");
        }
        outside[index] = true;
    }
    outside
}

/// A YAML scalar for the status in the style the file used: double- or single-quoted, or plain
/// when that reads back the same (else double-quoted).
fn yaml_scalar(value: &str, style: Option<char>) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '/'))
        && !value.starts_with([' ', '-'])
        && !value.ends_with(' ');
    match style {
        Some('\'') => format!("'{}'", value.replace('\'', "''")),
        None if plain => value.to_owned(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\"")),
    }
}

/// A `status:` line's value split out: what comes before it, its quote style, its meaning, and
/// what follows it (spaces and a `# comment`).
struct StatusValue<'a> {
    lead: &'a str,
    style: Option<char>,
    value: String,
    tail: &'a str,
}

fn status_value(rest: &str) -> StatusValue<'_> {
    let token = rest.trim_start();
    let lead = &rest[..rest.len() - token.len()];
    let quote = token.chars().next().filter(|&c| c == '"' || c == '\'');
    if let Some(quote) = quote {
        let mut escaped = false;
        let mut chars = token.char_indices().skip(1).peekable();
        while let Some((index, c)) = chars.next() {
            if quote == '"' && escaped {
                escaped = false;
            } else if quote == '"' && c == '\\' {
                escaped = true;
            } else if c == quote {
                if quote == '\'' && chars.peek().is_some_and(|&(_, next)| next == '\'') {
                    chars.next();
                    continue;
                }
                let end = index + c.len_utf8();
                let quoted = &token[..end];
                let value = if quote == '"' {
                    serde_json::from_str(quoted).unwrap_or_else(|_| quoted[1..index].to_owned())
                } else {
                    quoted[1..index].replace("''", "'")
                };
                return StatusValue {
                    lead,
                    style: Some(quote),
                    value,
                    tail: &token[end..],
                };
            }
        }
    }
    // Plain: up to a ` #` comment, trailing spaces left with the tail.
    let end = token
        .char_indices()
        .find(|&(index, c)| c == '#' && index > 0 && token[..index].ends_with([' ', '\t']))
        .map_or(token.len(), |(index, _)| index);
    let value = token[..end].trim_end();
    StatusValue {
        lead,
        style: None,
        value: value.to_owned(),
        tail: &token[value.len()..],
    }
}

/// `update_markdown`'s result: the new content, and whether the status could be set (false:
/// a status was configured but the file has no frontmatter).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MarkdownUpdate {
    pub(crate) content: String,
    pub(crate) status_set: bool,
}

/// The card file with its frontmatter `status:` value set to `done_status` (only the value: its
/// quoting and a trailing comment stay; added when missing, but a file without frontmatter gets
/// none) and each entry not yet there appended as a bullet under the first `Decisions` heading
/// of any level outside code and comments (a `## Decisions` is created at the end when there is
/// none). Every other line keeps its bytes, a UTF-8 BOM stays, and new lines use the file's
/// line ending (CRLF when it has any).
pub(crate) fn update_markdown(
    content: &str,
    done_status: Option<&str>,
    entries: &[String],
) -> MarkdownUpdate {
    let (bom, content) = match content.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", content),
    };
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = content
        .split_inclusive('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let status = done_status
        .map(str::trim)
        .filter(|status| !status.is_empty());
    let mut status_set = status.is_none();

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
            if let Some(status) = status {
                status_set = true;
                match (1..end).find(|&index| lines[index].starts_with("status:")) {
                    Some(index) => {
                        let line = &lines[index];
                        let text = without_ending(line);
                        let ending = &line[text.len()..];
                        let old = status_value(&text["status:".len()..]);
                        if old.value != status {
                            let lead = if old.lead.is_empty() { " " } else { old.lead };
                            let value = yaml_scalar(status, old.style);
                            lines[index] = format!("status:{lead}{value}{}{ending}", old.tail);
                        }
                    }
                    None => {
                        lines.insert(
                            end,
                            format!("status: {}{newline}", yaml_scalar(status, None)),
                        );
                        body_start += 1;
                    }
                }
            }
        }
    }
    let finish = |lines: Vec<String>| MarkdownUpdate {
        content: format!("{bom}{}", lines.concat()),
        status_set,
    };

    // The Decisions section: the first level-2 `Decisions` heading, else the first at another
    // level, up to the next heading of its level or above (level 2 at the deepest, so a
    // `# Decisions` does not swallow the `##` sections after it).
    let outside = markdown_lines(&lines, body_start);
    let headings = lines
        .iter()
        .enumerate()
        .skip(body_start)
        .filter(|(index, _)| outside[*index])
        .filter_map(|(index, line)| heading(line).map(|(level, title)| (index, level, title)))
        .collect::<Vec<_>>();
    let decisions = |&&(_, _, title): &&(usize, usize, &str)| is_decisions_title(title);
    let section = headings
        .iter()
        .filter(decisions)
        .find(|(_, level, _)| *level == 2)
        .or_else(|| headings.iter().find(decisions))
        .map(|&(index, level, _)| (index, level));
    let following = |from: usize| headings.iter().filter(move |(index, _, _)| *index > from);
    let section_end = section.map_or(lines.len(), |(heading, level)| {
        following(heading)
            .find(|(_, other, _)| *other <= level.max(2))
            .map_or(lines.len(), |(index, _, _)| *index)
    });
    // New entries go after the section's own text, before its first sub-heading.
    let content_end = section.map_or(lines.len(), |(heading, _)| {
        following(heading)
            .next()
            .map_or(section_end, |(index, _, _)| (*index).min(section_end))
    });

    let existing = section.map_or(&[][..], |(heading, _)| &lines[heading + 1..section_end]);
    let mut new_entries = Vec::new();
    for entry in entries {
        let entry = entry.trim();
        if !entry.is_empty() && !has_entry(existing, entry) && !new_entries.contains(&entry) {
            new_entries.push(entry);
        }
    }
    if new_entries.is_empty() {
        return finish(lines);
    }
    let bullets = new_entries
        .iter()
        .map(|entry| format!("- {entry}{newline}"))
        .collect::<Vec<_>>();

    let Some((heading, _)) = section else {
        // No section: a new one at the end, after a blank line.
        if let Some(last) = lines.last_mut() {
            if !last.ends_with('\n') {
                last.push_str(newline);
            }
        }
        if lines.last().is_some_and(|last| !last.trim().is_empty()) {
            lines.push(newline.to_owned());
        }
        lines.push(format!("## Decisions{newline}"));
        lines.push(newline.to_owned());
        lines.extend(bullets);
        return finish(lines);
    };

    let last_content = (heading + 1..content_end)
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
    finish(lines)
}

/// Runs `on_card_done` with the card as JSON on stdin. Fails on a spawn error, a non-zero exit
/// (with the last line of its stderr) or the timeout. Only a timeout kills it and its children;
/// background work a hook that exited normally left running (a detached upload, say) is left
/// alone.
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
    let mut child = process
        .spawn()
        .map_err(|err| Failure::new("hook_failed", format!("hook did not start: {err}")))?;
    // Kills the hook's process group when dropped armed: on the timeout, or if this task goes.
    let mut guard = crate::platform::StatusCommandGuard::new(&child)
        .map_err(|err| Failure::new("hook_failed", format!("hook did not start: {err}")))?;
    let deadline = tokio::time::Instant::now() + timeout;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read its input may close it early; that is its business.
        let _ = tokio::time::timeout_at(deadline, stdin.write_all(&input)).await;
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
    let finished = tokio::time::timeout_at(deadline, operation).await;
    if finished.is_ok() {
        guard.disarm();
    }
    match finished {
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

    fn md(content: &str, status: Option<&str>, entries: &[String]) -> String {
        update_markdown(content, status, entries).content
    }

    fn entries(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    const CARD: &str = "---\nid: SHE-1\ntitle: \"Stripe: checkout\"\nstatus: in-progress\nbadges:\n  - technical\n---\n\n## Description\n\nText  with  spacing \n\n## Decisions\n\nAgreed with André.\n\n## Validation\n\n```yaml\n## Decisions\n```\n";

    #[test]
    fn markdown_changes_only_status_and_decisions() {
        let out = md(
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
            md(
                &out,
                Some("verify"),
                &entries(&["Use Checkout", "Done 2026-10-22: live"])
            ),
            out
        );
        let more = md(&out, Some("verify"), &entries(&["Second"]));
        assert!(more.contains("- Done 2026-10-21: live\n- Second\n\n## Validation"));
    }

    #[test]
    fn markdown_without_a_section_gets_one_at_the_end() {
        let out = md(
            "---\nstatus: todo\n---\n\nBody",
            Some("verify"),
            &entries(&["A"]),
        );
        assert_eq!(
            out,
            "---\nstatus: verify\n---\n\nBody\n\n## Decisions\n\n- A\n"
        );
        let out = md("Body\n\n", None, &entries(&["A"]));
        assert_eq!(out, "Body\n\n## Decisions\n\n- A\n");
        // An empty section at the end of the file.
        let out = md("Body\n\n## Decisions\n", None, &entries(&["A"]));
        assert_eq!(out, "Body\n\n## Decisions\n\n- A\n");
    }

    #[test]
    fn markdown_status_is_added_quoted_or_left_alone() {
        assert_eq!(
            md("---\nid: X\n---\nBody\n", Some("verify"), &[]),
            "---\nid: X\nstatus: verify\n---\nBody\n"
        );
        assert_eq!(
            md("---\nstatus: \"todo\"\n---\n", Some("verify"), &[]),
            "---\nstatus: \"verify\"\n---\n"
        );
        assert_eq!(
            md("---\nstatus: todo\n---\n", Some("needs: review"), &[]),
            "---\nstatus: \"needs: review\"\n---\n"
        );
        // No frontmatter, no done_status, or the status already set: nothing changes.
        assert_eq!(md("status: todo\n", Some("verify"), &[]), "status: todo\n");
        assert_eq!(
            md("---\nstatus: todo\n---\n", None, &[]),
            "---\nstatus: todo\n---\n"
        );
        assert_eq!(
            md("---\nstatus: 'verify'\n---\n", Some("verify"), &[]),
            "---\nstatus: 'verify'\n---\n"
        );
        // An indented `status:` belongs to another key.
        assert_eq!(
            md("---\nmeta:\n  status: x\nstatus: a\n---\n", Some("b"), &[]),
            "---\nmeta:\n  status: x\nstatus: b\n---\n"
        );
    }

    #[test]
    fn markdown_keeps_crlf_line_endings() {
        let card = "---\r\nstatus: todo\r\n---\r\n\r\n## Decisions\r\n\r\n- Old\r\n";
        assert_eq!(
            md(card, Some("verify"), &entries(&["Old", "New"])),
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

        let outcome = run(
            config.clone(),
            card("SHE-1"),
            "2026-10-21".into(),
            7,
            Parts::ALL,
        )
        .await;
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

        let again = run(
            config.clone(),
            card("SHE-1"),
            "2026-10-22".into(),
            8,
            Parts::ALL,
        )
        .await;
        assert!(
            again.ok && again.message == "SHE-1.md already up to date",
            "{again:?}"
        );

        let missing = run(
            config.clone(),
            card("SHE-2"),
            "2026-10-21".into(),
            9,
            Parts::ALL,
        )
        .await;
        assert!(!missing.ok);
        assert_eq!(missing.code, "missing_file");
        assert!(!dir.join("sheprd/SHE-2.md").exists());

        let unknown = run(config, card("CAL-2"), "2026-10-21".into(), 9, Parts::ALL).await;
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
            Parts::ALL,
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
            Parts::ALL,
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
            Parts::ALL,
        )
        .await;
        assert_eq!((both.ok, both.code.as_str()), (false, "missing_file"));
        assert!(both.message.ends_with(".md; hook ran"), "{both:?}");

        let started = std::time::Instant::now();
        let slow = run(
            hook("sleep 30".into()),
            card("SHE-1"),
            String::new(),
            1,
            Parts::ALL,
        )
        .await;
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

    #[test]
    fn a_bom_stays_and_a_file_without_frontmatter_says_so() {
        let out = update_markdown(
            "\u{feff}---\nstatus: todo\n---\nBody\n",
            Some("verify"),
            &[],
        );
        assert_eq!(out.content, "\u{feff}---\nstatus: verify\n---\nBody\n");
        assert!(out.status_set);
        let out = update_markdown("\u{feff}Body\n", Some("verify"), &entries(&["A"]));
        assert_eq!(out.content, "\u{feff}Body\n\n## Decisions\n\n- A\n");
        assert!(!out.status_set);
        assert!(update_markdown("Body\n", None, &[]).status_set);

        let dir = temp_dir("bom");
        let mut config = config();
        config.root = Some(dir.display().to_string());
        std::fs::write(dir.join("sheprd/SHE-1.md"), "Body\n").unwrap();
        assert_eq!(
            write_markdown(&config, &card("SHE-1"), "2026-10-21").unwrap(),
            "wrote SHE-1.md (no frontmatter: status not set)"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_status_value_changes() {
        let status = |line: &str, value: &str| {
            md(&format!("---\n{line}\n---\n"), Some(value), &[])
                .lines()
                .nth(1)
                .unwrap_or_default()
                .to_owned()
        };
        assert_eq!(
            status("status: todo  # board column", "verify"),
            "status: verify  # board column"
        );
        assert_eq!(
            status("status: 'todo' # c", "verify"),
            "status: 'verify' # c"
        );
        assert_eq!(
            status("status: 'todo'", "it's done"),
            "status: 'it''s done'"
        );
        assert_eq!(
            status("status: \"a # b\" # c", "verify"),
            "status: \"verify\" # c"
        );
        assert_eq!(status("status:   todo", "verify"), "status:   verify");
        assert_eq!(status("status:", "verify"), "status: verify");
        assert_eq!(
            status("status: verify # same", "verify"),
            "status: verify # same"
        );
        assert_eq!(status("status: \"verify\"", "verify"), "status: \"verify\"");
    }

    #[test]
    fn decisions_heading_of_any_level_outside_code_and_comments() {
        let card = "# Card\n\n```md\n## Decisions\n```\n<!--\n## Decisions\n-->\n### decisions  \n\n- Old\n#### Detail\n\ntext\n### Next\n";
        assert_eq!(
            md(card, None, &entries(&["New"])),
            "# Card\n\n```md\n## Decisions\n```\n<!--\n## Decisions\n-->\n### decisions  \n\n- Old\n- New\n\n#### Detail\n\ntext\n### Next\n"
        );
        // Only the first Decisions heading gets the entries; a closing "##" is not the title.
        assert_eq!(
            md(
                "## Decisions ##\n- A\n## Decisions\n",
                None,
                &entries(&["B"])
            ),
            "## Decisions ##\n- A\n- B\n\n## Decisions\n"
        );
        // A heading inside a longer fence, closed only by a fence as long.
        assert_eq!(
            md("````\n```\n## Decisions\n````\n", None, &entries(&["B"])),
            "````\n```\n## Decisions\n````\n\n## Decisions\n\n- B\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_file_keeps_its_owner() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = temp_dir("owner");
        let path = dir.join("sheprd/card.md");
        std::fs::write(&path, "old").unwrap();
        // Only root can hand a file to another user; elsewhere the owner is ours anyway.
        let owner = if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::chown(&path, Some(65534), Some(65534)).unwrap();
            (65534, 65534)
        } else {
            let metadata = std::fs::metadata(&path).unwrap();
            (metadata.uid(), metadata.gid())
        };
        write_atomically(&path, "new").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), owner);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_timed_out_hook_has_its_children_killed() {
        let dir = temp_dir("children");
        let pid_file = dir.join("pid");
        let alive = |pid: &str| {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .is_ok_and(|status| status.success())
        };
        let hook = |command: String| LedgerWritebackConfig {
            on_card_done: Some(command),
            timeout_seconds: Some(1),
            ..LedgerWritebackConfig::default()
        };
        let background = format!(
            "cat >/dev/null; sleep 30 </dev/null >/dev/null 2>&1 & echo $! > '{}'",
            pid_file.display()
        );

        let outcome = run(
            hook(background.clone()),
            card("SHE-1"),
            String::new(),
            1,
            Parts::ALL,
        )
        .await;
        assert!(outcome.ok, "{outcome:?}");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        assert!(
            alive(&pid),
            "a hook that exited leaves its background work alone"
        );
        let _ = std::process::Command::new("kill").arg(pid.trim()).status();

        let outcome = run(
            hook(format!("{background}; sleep 30")),
            card("SHE-1"),
            String::new(),
            1,
            Parts::ALL,
        )
        .await;
        assert_eq!(outcome.message, "hook timed out after 1s");
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let started = std::time::Instant::now();
        while alive(&pid) && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!alive(&pid), "a timed-out hook's children are killed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_fingerprint_follows_what_is_written() {
        let base = fingerprint(&card("SHE-1"));
        assert_eq!(base.len(), 16);
        let mut other = card("SHE-1");
        other.pane_id = "w9:p9".into();
        other.agent_session = None;
        assert_eq!(fingerprint(&other), base, "who wrote it does not count");
        other.decisions.push("Another".into());
        assert_ne!(fingerprint(&other), base);
        let mut moved = card("SHE-1");
        moved.summary = String::new();
        moved.title = "Stripe checkoutCheckout live on dev".into();
        assert_ne!(fingerprint(&moved), base);
    }

    #[test]
    fn a_decisions_heading_with_a_note_at_level_two_wins() {
        // LF-100454's shape: the real `## Decisions (…)` early, a nested `### Decisions` later.
        let card = "---\nstatus: todo\n---\n\n## Description\n\nText\n\n## Decisions (orquestrador + André, 07/10)\n\n- Old\n\n## Plan\n\n### Decisions\n\n- Nested\n";
        assert_eq!(
            md(card, None, &entries(&["New"])),
            "---\nstatus: todo\n---\n\n## Description\n\nText\n\n## Decisions (orquestrador + André, 07/10)\n\n- Old\n- New\n\n## Plan\n\n### Decisions\n\n- Nested\n"
        );
        assert_eq!(
            md(
                "## Validation\n#### Decisions\nx\n## Decisions\n- a\n",
                None,
                &entries(&["b"])
            ),
            "## Validation\n#### Decisions\nx\n## Decisions\n- a\n- b\n"
        );
        // Only another level when there is no level-2 one; "Decisions:" counts, "Decisionsx" not.
        assert_eq!(
            md(
                "### Decisions: agreed\n- a\n## Decisionsx\n",
                None,
                &entries(&["b"])
            ),
            "### Decisions: agreed\n- a\n- b\n\n## Decisionsx\n"
        );
        assert!(is_decisions_title("decisions"));
        assert!(!is_decisions_title("Decisionsx"));
        assert!(!is_decisions_title("Décisions"));
    }

    #[test]
    fn inline_code_does_not_open_a_comment() {
        assert_eq!(
            md(
                "Use `<!--` here\n\n## Decisions\n- old\n",
                None,
                &entries(&["new"])
            ),
            "Use `<!--` here\n\n## Decisions\n- old\n- new\n"
        );
    }

    #[test]
    fn a_level_one_section_ends_at_level_two_and_entries_precede_sub_headings() {
        assert_eq!(
            md("# Decisions\n- a\n## Other\ntext\n", None, &entries(&["b"])),
            "# Decisions\n- a\n- b\n\n## Other\ntext\n"
        );
        assert_eq!(
            md(
                "## Decisions\n\n- a\n\n### Why\n\nreasons\n\n## Next\n",
                None,
                &entries(&["b"])
            ),
            "## Decisions\n\n- a\n- b\n\n### Why\n\nreasons\n\n## Next\n"
        );
        // An entry already under the sub-heading is still not repeated.
        assert_eq!(
            md("## Decisions\n\n### Why\n- b\n", None, &entries(&["b"])),
            "## Decisions\n\n### Why\n- b\n"
        );
    }

    #[test]
    fn only_the_part_that_failed_runs_again() {
        let fp = "abc";
        let last = |ok: bool, markdown: Option<bool>, hook: Option<bool>| LedgerWriteback {
            ok,
            code: String::new(),
            message: String::new(),
            at_ms: 0,
            fingerprint: fp.into(),
            markdown,
            hook,
        };
        assert_eq!(Parts::after(None, fp), Some(Parts::ALL));
        assert_eq!(
            Parts::after(Some(&last(true, Some(true), Some(true))), fp),
            None
        );
        assert_eq!(
            Parts::after(Some(&last(false, Some(false), Some(true))), fp),
            Some(Parts {
                markdown: true,
                hook: false
            })
        );
        assert_eq!(
            Parts::after(Some(&last(false, Some(true), Some(false))), fp),
            Some(Parts {
                markdown: false,
                hook: true
            })
        );
        // The card changed: both again.
        assert_eq!(
            Parts::after(Some(&last(false, Some(false), Some(true))), "other"),
            Some(Parts::ALL)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_retry_after_a_missing_file_does_not_run_the_hook_again() {
        let dir = temp_dir("retry");
        let runs = dir.join("runs");
        let config = LedgerWritebackConfig {
            markdown_path: Some(format!("{}/{{id}}.md", dir.display())),
            on_card_done: Some(format!("cat >/dev/null; echo run >> '{}'", runs.display())),
            ..LedgerWritebackConfig::default()
        };
        let first = run(config.clone(), card("SHE-1"), String::new(), 1, Parts::ALL).await;
        assert_eq!(
            (first.ok, first.markdown, first.hook),
            (false, Some(false), Some(true))
        );
        std::fs::write(dir.join("SHE-1.md"), "Body\n").unwrap();
        let parts = Parts::after(Some(&first), &fingerprint(&card("SHE-1"))).unwrap();
        let second = run(config, card("SHE-1"), String::new(), 2, parts).await;
        assert!(second.ok, "{second:?}");
        assert_eq!((second.markdown, second.hook), (Some(true), Some(true)));
        assert_eq!(second.message, "wrote SHE-1.md; hook ran before");
        assert_eq!(
            std::fs::read_to_string(&runs).unwrap(),
            "run\n",
            "the hook ran once"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn decisions_needed_is_not_the_decisions_section() {
        assert_eq!(
            md(
                "## Decisions needed\n- which?\n\n## Decisions\n- a\n",
                None,
                &entries(&["b"])
            ),
            "## Decisions needed\n- which?\n\n## Decisions\n- a\n- b\n"
        );
        assert!(is_decisions_title("Decisions  (07/10)"));
        assert!(is_decisions_title("Decisions:"));
        assert!(!is_decisions_title("Decisions to make first"));
    }
}
