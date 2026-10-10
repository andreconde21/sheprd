//! andreconde fork (sheprd): the work panel on the right (SHE-100004). For the focused agent it
//! shows the tasks it handed to subagents, grouped by reference id, with each stage's state
//! (dev ✓  adv ●  chr ·), from the `sheprd_w_*` tokens of sheprd-claude-hook. Click an id for the
//! card (the `[[refs]]` command), a stage for its report, ↗ for the pages a browser stage opened.
//! Full, mini (one mark per task) or hidden: prefix+shift+b cycles.

use super::projects;
use super::render::{display_width, put_text};
use super::*;

/// Panel widths: full and mini.
pub(super) const FULL_WIDTH: u16 = 36;
pub(super) const MINI_WIDTH: u16 = 3;

/// Saved panel state (chrome preferences): 0 hidden, 1 mini, 2 full.
pub(super) const HIDDEN: u8 = 0;
pub(super) const MINI: u8 = 1;
pub(super) const FULL: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct WorkTask {
    pub(super) key: String,
    pub(super) label: String,
    /// (stage type, state: 'r' running, 'd' done, 's' stopped, 'f' failed).
    pub(super) stages: Vec<(String, char)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum WorkHit {
    Card(String),
    Stage {
        machine: Option<String>,
        file: String,
        key: String,
        label: String,
        stage: String,
    },
    Url {
        machine: Option<String>,
        file: String,
        key: String,
        label: String,
        stage: String,
    },
    Expand,
    /// A link the tool that started the workspace reported (`card_link`), opened in its app.
    Link(String),
    /// A `[[status]]` result's details, shown in the reader.
    Details {
        title: String,
        details: String,
    },
}

/// Tasks from one agent's `sheprd_w_1..` tokens ("id|label|stage:s,stage:s"), newest first.
pub(super) fn tasks(agent: &crate::protocol::ClientShellAgent) -> Vec<WorkTask> {
    let mut numbered = agent
        .tokens
        .iter()
        .filter_map(|(name, value)| {
            let index = name
                .trim_start_matches('$')
                .strip_prefix("sheprd_w_")?
                .parse::<usize>()
                .ok()?;
            Some((index, value.as_str()))
        })
        .collect::<Vec<_>>();
    numbered.sort_by_key(|(index, _)| *index);
    numbered
        .into_iter()
        .filter_map(|(_, value)| parse_task(value))
        .collect()
}

fn parse_task(value: &str) -> Option<WorkTask> {
    let mut parts = value.splitn(3, '|');
    let key = parts.next()?.trim().to_owned();
    let label = parts.next()?.trim().to_owned();
    let stages = parts
        .next()?
        .split(',')
        .filter_map(|stage| {
            let (name, state) = stage.rsplit_once(':')?;
            Some((name.trim().to_owned(), state.trim().chars().next()?))
        })
        .collect::<Vec<_>>();
    (!stages.is_empty()).then_some(WorkTask { key, label, stages })
}

fn short_stage(name: &str) -> String {
    match name {
        "adversarial" => "adv".to_owned(),
        "chrome" => "chr".to_owned(),
        other if other.chars().count() <= 5 => other.to_owned(),
        other => other.chars().take(3).collect(),
    }
}

fn mark(state: char) -> &'static str {
    match state {
        'd' => "✓",
        'r' => "●",
        'f' => "✗",
        _ => "◌",
    }
}

fn mark_color(state: char, palette: &Palette) -> ratatui::style::Color {
    match state {
        'd' => palette.green,
        'r' => palette.accent,
        'f' => palette.red,
        _ => palette.overlay0,
    }
}

fn is_browser_stage(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("chrome") || name.contains("browser")
}

/// A workspace token (`card`, `card_link`), as reported by the tool that started it.
fn workspace_token<'a>(
    workspace: &'a crate::protocol::ClientShellWorkspace,
    name: &str,
) -> Option<&'a str> {
    workspace
        .tokens
        .iter()
        .find(|(token, _)| token.trim_start_matches('$') == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

/// The card the focused agent's workspace was started for (e.g. by Cockpit Board) and its link.
fn workspace_card(
    endpoints: &[ClientShellEndpoint],
    active: &ClientEndpointId,
    agent: &crate::protocol::ClientShellAgent,
) -> Option<(String, Option<String>)> {
    let snapshot = endpoints
        .iter()
        .find(|e| e.endpoint_id == *active)?
        .snapshot
        .as_deref()?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id == agent.workspace_id)?;
    let card = workspace_token(workspace, "card")?.to_owned();
    let link = workspace_token(workspace, "card_link")
        .filter(|link| {
            ["obsidian://", "https://", "http://"]
                .iter()
                .any(|scheme| link.starts_with(scheme))
        })
        .map(str::to_owned);
    Some((card, link))
}

/// Opens a link in this machine's default app.
pub(super) fn open_link(link: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(link)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// The focused agent on the active machine, with that machine's label (None for Local).
fn focused_agent<'a>(
    endpoints: &'a [ClientShellEndpoint],
    active: &ClientEndpointId,
) -> Option<(&'a crate::protocol::ClientShellAgent, Option<String>)> {
    let endpoint = endpoints.iter().find(|e| e.endpoint_id == *active)?;
    let agent = endpoint
        .snapshot
        .as_deref()?
        .agents
        .iter()
        .find(|a| a.focused)?;
    let machine = (!endpoint.endpoint_id.is_local()).then(|| endpoint.label.clone());
    Some((agent, machine))
}

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    endpoints: &[ClientShellEndpoint],
    active: &ClientEndpointId,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let base = Style::default().fg(palette.text).bg(palette.panel_bg);
    for y in area.y..area.bottom() {
        put_text(
            buffer,
            area.x,
            y,
            area.width,
            &" ".repeat(usize::from(area.width)),
            base,
        );
        put_text(
            buffer,
            area.x,
            y,
            1,
            "│",
            Style::default().fg(palette.overlay0).bg(palette.panel_bg),
        );
    }
    hits.work_panel.clear();
    let (agent, machine) = match focused_agent(endpoints, active) {
        Some((agent, machine)) => (Some(agent), machine),
        None => (None, None),
    };
    let tasks = agent.map(tasks).unwrap_or_default();
    let file = agent
        .and_then(|agent| projects::agent_token(agent, "sheprd_wf"))
        .map(str::to_owned);
    let (x, width) = (area.x + 1, area.width.saturating_sub(1));
    if area.width <= MINI_WIDTH {
        // Mini: one mark per task; click opens the full panel.
        for (row, task) in tasks.iter().enumerate().take(usize::from(area.height)) {
            let state = if task.stages.iter().any(|(_, s)| *s == 'r') {
                'r'
            } else if task.stages.iter().any(|(_, s)| *s == 'f') {
                'f'
            } else if task.stages.iter().all(|(_, s)| *s == 'd') {
                'd'
            } else {
                's'
            };
            let y = area.y + row as u16;
            put_text(
                buffer,
                x,
                y,
                width,
                &format!(" {}", mark(state)),
                base.fg(mark_color(state, palette)),
            );
        }
        hits.work_panel.push((area, WorkHit::Expand));
        return;
    }
    let title = match agent {
        Some(agent) => format!(
            " {}{}",
            agent
                .name
                .as_deref()
                .or(agent.display_agent.as_deref())
                .or(agent.agent.as_deref())
                .unwrap_or("agent"),
            machine
                .as_deref()
                .map(|machine| format!(" · {machine}"))
                .unwrap_or_default()
        ),
        None => " no agent focused".to_owned(),
    };
    put_text(
        buffer,
        x,
        area.y,
        width,
        &title,
        base.fg(palette.overlay1).add_modifier(Modifier::BOLD),
    );
    // The card this workspace was started for (Cockpit Board and other launchers).
    if let Some((card, link)) = agent.and_then(|agent| workspace_card(endpoints, active, agent)) {
        let text = format!(" card {card}{}", if link.is_some() { " ↗" } else { "" });
        let w = display_width(&text).min(width);
        put_text(buffer, x, area.y + 1, w, &text, base.fg(palette.accent));
        let hit = match link {
            Some(link) => WorkHit::Link(link),
            None => WorkHit::Card(card),
        };
        hits.work_panel.push((Rect::new(x, area.y + 1, w, 1), hit));
    }
    // `[[status]]` results for this workspace (PR, checks, deploy): red = needs you.
    let mut top = area.y + 2;
    let workspace = agent.and_then(|agent| {
        let endpoint = endpoints.iter().find(|e| e.endpoint_id == *active)?;
        let snapshot = endpoint.snapshot.as_deref()?;
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == agent.workspace_id)?;
        Some(projects::workspace_key(endpoint, workspace))
    });
    for status in workspace
        .map(|key| super::status::statuses(&key))
        .unwrap_or_default()
    {
        if top + 1 >= area.bottom() {
            break;
        }
        let (glyph, color) = match status.state.as_str() {
            "ok" => ("✓", palette.green),
            "fail" => ("✗", palette.red),
            "review" => ("◐", palette.yellow),
            _ => ("●", palette.accent),
        };
        let text = format!(" {glyph} {}", status.text);
        let w = display_width(&text).min(width);
        put_text(buffer, x, top, w, &text, base.fg(color));
        if !status.details.is_empty() {
            hits.work_panel.push((
                Rect::new(x, top, w, 1),
                WorkHit::Details {
                    title: format!("{} · {}", status.name, status.text),
                    details: status.details.clone(),
                },
            ));
        }
        if !status.url.is_empty() && x + w + 2 <= x + width {
            put_text(buffer, x + w + 1, top, 1, "↗", base.fg(palette.accent));
            hits.work_panel.push((
                Rect::new(x + w + 1, top, 1, 1),
                WorkHit::Link(status.url.clone()),
            ));
        }
        top += 1;
    }
    if top > area.y + 2 {
        top += 1;
    }
    let Some(agent) = agent else {
        return;
    };
    let heading = base.fg(palette.overlay1).add_modifier(Modifier::BOLD);
    let mut y = top;
    // What the agent has been doing (model summary, asked for by sheprd while this shows).
    let summary = projects::agent_token(agent, "sheprd_sum");
    let label = match projects::agent_token(agent, "sheprd_sum_at") {
        Some(at) => format!(" SUMMARY · {at}"),
        None => " SUMMARY".to_owned(),
    };
    put_text(buffer, x, y, width, &label, heading);
    y += 1;
    match summary {
        Some(summary) => {
            for line in wrap(summary, usize::from(width.saturating_sub(2)))
                .into_iter()
                .take(3)
            {
                if y >= area.bottom() {
                    return;
                }
                put_text(buffer, x + 1, y, width - 1, &line, base);
                y += 1;
            }
            for i in 1..=3 {
                if let Some(done) = projects::agent_token(agent, &format!("sheprd_sum_{i}")) {
                    let lines = wrap(done, usize::from(width.saturating_sub(4)));
                    for (n, line) in lines.into_iter().take(2).enumerate() {
                        if y >= area.bottom() {
                            return;
                        }
                        let bullet = if n == 0 { "· " } else { "  " };
                        put_text(
                            buffer,
                            x + 1,
                            y,
                            width - 1,
                            &format!("{bullet}{line}"),
                            base.fg(palette.overlay1),
                        );
                        y += 1;
                    }
                }
            }
        }
        None => {
            // A summary takes a few seconds; after 3 minutes without one, say why.
            let waiting = summary_asked_at(machine.as_deref(), &agent.pane_id)
                .is_none_or(|at| at.elapsed() < std::time::Duration::from_secs(180));
            let note = if waiting {
                "on its way…"
            } else {
                "none yet (set summary_command)"
            };
            put_text(buffer, x + 1, y, width - 1, note, base.fg(palette.overlay0));
            y += 1;
        }
    }
    // Questions in its last message that wait on you.
    let questions = (1..=3)
        .filter_map(|i| projects::agent_token(agent, &format!("sheprd_q_{i}")))
        .collect::<Vec<_>>();
    if !questions.is_empty() && y + 1 < area.bottom() {
        y += 1;
        put_text(
            buffer,
            x,
            y,
            width,
            " WAITING ON YOU",
            heading.fg(palette.yellow),
        );
        y += 1;
        for question in questions {
            for line in wrap(question, usize::from(width.saturating_sub(4)))
                .into_iter()
                .take(2)
            {
                if y >= area.bottom() {
                    return;
                }
                put_text(
                    buffer,
                    x + 1,
                    y,
                    width - 1,
                    &format!("? {line}"),
                    base.fg(palette.yellow),
                );
                y += 1;
            }
        }
    }
    // To-do: what it is on, then what is left.
    if let Some(((done, total), items)) = projects::agent_todo(agent) {
        if y + 1 < area.bottom() {
            y += 1;
            put_text(
                buffer,
                x,
                y,
                width,
                &format!(" TO-DO {done}/{total}"),
                heading,
            );
            y += 1;
            let mut shown = items
                .iter()
                .filter(|item| !item.starts_with('✓'))
                .take(5)
                .collect::<Vec<_>>();
            if shown.is_empty() {
                shown = items.iter().rev().take(1).collect();
            }
            for item in shown {
                if y >= area.bottom() {
                    return;
                }
                let color = if item.starts_with('▸') {
                    palette.accent
                } else {
                    palette.text
                };
                put_text(buffer, x + 1, y, width - 1, item, base.fg(color));
                y += 1;
            }
        }
    }
    if !tasks.is_empty() && y + 2 < area.bottom() {
        y += 1;
        put_text(buffer, x, y, width, " TASKS", heading);
        y += 1;
    }
    for task in &tasks {
        if y + 1 >= area.bottom() {
            break;
        }
        // Line 1: card id (click opens the card) and label.
        let mut cx = x + 1;
        if !task.key.is_empty() {
            let id_width = display_width(&task.key);
            put_text(
                buffer,
                cx,
                y,
                id_width,
                &task.key,
                base.fg(palette.accent).add_modifier(Modifier::BOLD),
            );
            hits.work_panel.push((
                Rect::new(cx, y, id_width, 1),
                WorkHit::Card(task.key.clone()),
            ));
            cx += id_width + 1;
        }
        let rest = (x + width).saturating_sub(cx);
        put_text(buffer, cx, y, rest, &task.label, base);
        // Line 2: stages (click opens that stage's report; ↗ opens the pages a browser stage used).
        let mut sx = x + 2;
        let line = y + 1;
        for (stage, state) in &task.stages {
            let text = format!("{} {}", short_stage(stage), mark(*state));
            let w = display_width(&text);
            if sx + w > x + width {
                break;
            }
            put_text(
                buffer,
                sx,
                line,
                w,
                &short_stage(stage),
                base.fg(palette.overlay1),
            );
            put_text(
                buffer,
                sx + w - 1,
                line,
                1,
                mark(*state),
                base.fg(mark_color(*state, palette)),
            );
            if let Some(file) = &file {
                let hit = WorkHit::Stage {
                    machine: machine.clone(),
                    file: file.clone(),
                    key: task.key.clone(),
                    label: task.label.clone(),
                    stage: stage.clone(),
                };
                hits.work_panel.push((Rect::new(sx, line, w, 1), hit));
            }
            sx += w + 2;
            if is_browser_stage(stage) && *state == 'd' && sx + 1 <= x + width {
                if let Some(file) = &file {
                    put_text(buffer, sx - 1, line, 1, "↗", base.fg(palette.accent));
                    let hit = WorkHit::Url {
                        machine: machine.clone(),
                        file: file.clone(),
                        key: task.key.clone(),
                        label: task.label.clone(),
                        stage: stage.clone(),
                    };
                    hits.work_panel.push((Rect::new(sx - 1, line, 1, 1), hit));
                    sx += 1;
                }
            }
        }
        y += 3;
    }
    // Timeline: the latest prompts it got, oldest first, as many as fit.
    let timeline = projects::agent_timeline(agent);
    if !timeline.is_empty() && y + 2 < area.bottom() {
        put_text(buffer, x, y, width, " TIMELINE", heading);
        y += 1;
        let room = usize::from(area.bottom().saturating_sub(y));
        let skip = timeline.len().saturating_sub(room);
        for line in timeline.iter().skip(skip) {
            put_text(buffer, x + 1, y, width - 1, line, base.fg(palette.overlay1));
            y += 1;
        }
    }
}

/// Asks the focused agent's machine for a fresh summary when the panel shows it: the first time,
/// then when it moved on (state changed) and 10 minutes passed. The hook skips sessions whose
/// transcript barely grew, so looking at an idle agent costs nothing.
type Asked = std::collections::HashMap<String, (u64, std::time::Instant)>;

fn asked() -> &'static std::sync::Mutex<Asked> {
    static ASKED: std::sync::OnceLock<std::sync::Mutex<Asked>> = std::sync::OnceLock::new();
    ASKED.get_or_init(Default::default)
}

/// When a summary was last asked for this agent (None: never).
fn summary_asked_at(machine: Option<&str>, pane: &str) -> Option<std::time::Instant> {
    asked()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&format!("{}/{pane}", machine.unwrap_or("local")))
        .map(|(_, at)| *at)
}

pub(super) fn request_summary(endpoints: &[ClientShellEndpoint], active: &ClientEndpointId) {
    if cfg!(test) {
        return;
    }
    let Some((agent, machine)) = focused_agent(endpoints, active) else {
        return;
    };
    // Claude Code agents are summarized from their transcript, any other agent from its pane.
    let session = projects::agent_token(agent, "sheprd_session")
        .filter(|session| {
            session
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .map(str::to_owned);
    let key = format!(
        "{}/{}",
        machine.as_deref().unwrap_or("local"),
        agent.pane_id
    );
    {
        let mut asked = asked().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((seq, at)) = asked.get(&key) {
            if *seq == agent.state_change_seq || at.elapsed() < std::time::Duration::from_secs(600)
            {
                return;
            }
        }
        asked.insert(key, (agent.state_change_seq, std::time::Instant::now()));
    }
    let pane = agent.pane_id.clone();
    std::thread::spawn(move || {
        let mut command = match machine {
            None => {
                let Some(hook) = crate::sheprd_msg::bundled_script("sheprd-claude-hook") else {
                    return;
                };
                let mut command = std::process::Command::new("python3");
                command.arg(hook).args(["--summarize-pane", &pane]);
                if let Some(session) = &session {
                    command.arg(session);
                }
                if let Ok(exe) = std::env::current_exe() {
                    command.env("HERDR_BIN_PATH", exe);
                }
                command
            }
            Some(label) => {
                let Some((target, herdr_session)) = machine_profile(&label) else {
                    return;
                };
                let socket = if herdr_session == "default" {
                    String::new()
                } else {
                    format!("HERDR_SOCKET_PATH=\"$HOME/.config/herdr/sessions/{herdr_session}/herdr.sock\" ")
                };
                let script = format!(
                    "PATH=\"$HOME/.local/bin:$PATH\"; {socket}sheprd-claude-hook --summarize-pane '{pane}' {}",
                    session.as_deref().unwrap_or("")
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
        let _ = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Word-wraps `text` to `width` columns.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty()
            && display_width(&line) as usize + 1 + display_width(word) as usize > width
        {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Reads the hook's details file, here or on the agent's machine (over SSH, with the target from
/// the saved machines).
fn read_details(machine: Option<&str>, file: &str) -> Option<serde_json::Value> {
    let text = match machine {
        None => {
            let path = match file.strip_prefix("~/") {
                Some(rest) => std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest),
                None => std::path::PathBuf::from(file),
            };
            std::fs::read_to_string(path).ok()?
        }
        Some(label) => {
            let target = ssh_target(label)?;
            let output = std::process::Command::new("ssh")
                .args([
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=5",
                    &target,
                    "cat",
                    file,
                ])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
    };
    serde_json::from_str(&text).ok()
}

/// The SSH target and herdr session of a saved machine, by its label.
pub(super) fn machine_profile(label: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(crate::client::endpoint::catalog_path()).ok()?;
    let catalog: serde_json::Value = serde_json::from_str(&text).ok()?;
    let profile = catalog
        .get("ssh")?
        .as_array()?
        .iter()
        .find(|profile| profile.get("label").and_then(|l| l.as_str()) == Some(label))?;
    let target = profile.get("target")?.as_str()?.to_owned();
    let session = profile
        .get("session")
        .and_then(|s| s.as_str())
        .unwrap_or("default")
        .to_owned();
    Some((target, session))
}

/// The SSH target of a saved machine, by its label.
pub(super) fn ssh_target(label: &str) -> Option<String> {
    let text = std::fs::read_to_string(crate::client::endpoint::catalog_path()).ok()?;
    let catalog: serde_json::Value = serde_json::from_str(&text).ok()?;
    catalog
        .get("ssh")?
        .as_array()?
        .iter()
        .find(|profile| profile.get("label").and_then(|l| l.as_str()) == Some(label))?
        .get("target")?
        .as_str()
        .map(str::to_owned)
}

fn find_stage<'a>(
    details: &'a serde_json::Value,
    key: &str,
    label: &str,
    stage: &str,
) -> Option<&'a serde_json::Value> {
    details
        .get("tasks")?
        .as_array()?
        .iter()
        .find(|task| {
            let task_key = task.get("key").and_then(|k| k.as_str()).unwrap_or("");
            if key.is_empty() {
                task_key.is_empty() && task.get("label").and_then(|l| l.as_str()) == Some(label)
            } else {
                task_key == key
            }
        })?
        .get("stages")?
        .as_array()?
        .iter()
        .find(|s| s.get("type").and_then(|t| t.as_str()) == Some(stage))
}

/// A stage's report as markdown for the reader.
pub(super) fn stage_document(
    details: &serde_json::Value,
    key: &str,
    label: &str,
    stage: &str,
) -> Option<String> {
    let found = find_stage(details, key, label, stage)?;
    let get = |field: &str| found.get(field).and_then(|v| v.as_str()).unwrap_or("");
    let state = match get("state") {
        "d" => "done",
        "r" => "running",
        "f" => "failed",
        _ => "stopped without a final answer",
    };
    let mut doc = format!("# {stage} · {state}\n\n{}\n\n", get("description"));
    let report = get("report");
    if report.trim().is_empty() {
        doc.push_str("No final report yet.\n");
    } else {
        doc.push_str(report);
        doc.push('\n');
    }
    for (field, title) in [("urls", "Pages opened"), ("files", "Files saved")] {
        if let Some(items) = found
            .get(field)
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty())
        {
            doc.push_str(&format!("\n## {title}\n\n"));
            for item in items.iter().filter_map(|v| v.as_str()) {
                doc.push_str(&format!("- {item}\n"));
            }
        }
    }
    Some(doc)
}

/// Opens a stage's report in the reader (fetched off the UI thread).
pub(super) fn open_stage(
    machine: Option<String>,
    file: String,
    key: String,
    label: String,
    stage: String,
) {
    std::thread::spawn(move || {
        let id = if key.is_empty() {
            format!("{stage} · {label}")
        } else {
            format!("{key} · {stage}")
        };
        let doc = read_details(machine.as_deref(), &file)
            .and_then(|details| stage_document(&details, &key, &label, &stage));
        projects::push_task_body(id, doc);
    });
}

/// Opens the last page a browser stage opened, in this machine's browser.
pub(super) fn open_url(
    machine: Option<String>,
    file: String,
    key: String,
    label: String,
    stage: String,
) {
    std::thread::spawn(move || {
        let url = read_details(machine.as_deref(), &file).and_then(|details| {
            find_stage(&details, &key, &label, &stage)?
                .get("urls")?
                .as_array()?
                .iter()
                .rev()
                .find_map(|u| u.as_str().map(str::to_owned))
        });
        let Some(url) = url.filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        else {
            return;
        };
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hook_tokens() {
        assert_eq!(
            parse_task("CAL-1021|Bring PRs onto trunk|dev:d,adversarial:r"),
            Some(WorkTask {
                key: "CAL-1021".into(),
                label: "Bring PRs onto trunk".into(),
                stages: vec![("dev".into(), 'd'), ("adversarial".into(), 'r')],
            })
        );
        let no_id = parse_task("|Draft release notes|agent:s").unwrap();
        assert_eq!((no_id.key.as_str(), no_id.stages[0].1), ("", 's'));
        assert_eq!(parse_task("garbage"), None);
        assert_eq!(parse_task("A|b|"), None);
    }

    #[test]
    fn stage_document_includes_report_and_pages() {
        let details = serde_json::json!({"tasks": [
            {"key": "CAL-9", "label": "x", "stages": [
                {"type": "chrome", "state": "d", "description": "DEV check", "report": "All good.",
                 "urls": ["https://dev.example/a"], "files": []}
            ]},
            {"key": "", "label": "Draft notes", "stages": [
                {"type": "agent", "state": "s", "description": "Draft notes", "report": ""}
            ]}
        ]});
        let doc = stage_document(&details, "CAL-9", "x", "chrome").unwrap();
        assert!(doc.contains("# chrome · done") && doc.contains("All good."));
        assert!(doc.contains("- https://dev.example/a"));
        let no_id = stage_document(&details, "", "Draft notes", "agent").unwrap();
        assert!(
            no_id.contains("stopped without a final answer")
                && no_id.contains("No final report yet.")
        );
        assert!(stage_document(&details, "CAL-9", "x", "dev").is_none());
    }
}
