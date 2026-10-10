//! andreconde fork (sheprd): the status board (SHE-100000). One document with every agent on every
//! machine, those that need you first: what it is on now (to-do), where the work stands (the
//! optional model summary), its tasks' pipelines (work panel tokens) and the questions it is
//! waiting on. Shown in the reader; opened from the sidebar header ("board") or the menu.

use super::projects::{self, Presence, ProjectLayout};
use super::*;

struct Entry {
    rank: u8,
    project: String,
    text: String,
}

fn rank(presence: Presence) -> u8 {
    match presence {
        Presence::Blocked => 0,
        Presence::Unread => 1,
        Presence::Done => 2,
        Presence::Working => 3,
        Presence::Idle => 4,
    }
}

fn mark(presence: Presence) -> &'static str {
    match presence {
        Presence::Blocked => "● blocked",
        Presence::Unread => "● needs you",
        Presence::Done => "✓ done",
        Presence::Working => "▸ working",
        Presence::Idle => "○ idle",
    }
}

fn title(agent: &crate::protocol::ClientShellAgent) -> String {
    [
        agent.terminal_title_stripped.as_deref(),
        agent.title.as_deref(),
        agent.display_agent.as_deref(),
        agent.name.as_deref(),
        agent.agent.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|title| !title.is_empty())
    .unwrap_or("agent")
    .to_owned()
}

fn stages_line(task: &super::work_panel::WorkTask) -> String {
    task.stages
        .iter()
        .map(|(stage, state)| {
            let mark = match state {
                'd' => "✓",
                'r' => "●",
                'f' => "✗",
                _ => "◌",
            };
            format!("{stage} {mark}")
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn agent_text(
    agent: &crate::protocol::ClientShellAgent,
    machine: &str,
    workspace: &str,
    workspace_key: &str,
    presence: Presence,
) -> String {
    let mut text = format!(
        "#### {} · {} · {machine}/{workspace}\n",
        mark(presence),
        title(agent)
    );
    for status in super::status::statuses(workspace_key) {
        let glyph = match status.state.as_str() {
            "ok" => "✓",
            "fail" => "✗",
            "review" => "◐",
            _ => "●",
        };
        text.push_str(&format!("- {glyph} {}: {}\n", status.name, status.text));
    }
    if let Some(card) = projects::agent_tasks(agent).first() {
        text.push_str(&format!("- card: {card}\n"));
    }
    if let Some(summary) = projects::agent_token(agent, "sheprd_sum") {
        text.push_str(&format!("- **{summary}**\n"));
    }
    for i in 1..=3 {
        if let Some(done) = projects::agent_token(agent, &format!("sheprd_sum_{i}")) {
            text.push_str(&format!("  - {done}\n"));
        }
    }
    if let Some(((done, total), items)) = projects::agent_todo(agent) {
        let now = items
            .iter()
            .find(|item| item.starts_with('▸'))
            .map(|item| format!(": {}", item.trim_start_matches('▸').trim()))
            .unwrap_or_default();
        text.push_str(&format!("- to-do {done}/{total}{now}\n"));
    }
    for task in super::work_panel::tasks(agent).iter().take(5) {
        let name = if task.key.is_empty() {
            task.label.clone()
        } else {
            format!("{} {}", task.key, task.label)
        };
        text.push_str(&format!("- {name} — {}\n", stages_line(task)));
    }
    let questions = (1..=3)
        .filter_map(|i| projects::agent_token(agent, &format!("sheprd_q_{i}")))
        .collect::<Vec<_>>();
    if !questions.is_empty() {
        text.push_str("- waiting on you:\n");
        for question in questions {
            text.push_str(&format!("  - {question}\n"));
        }
    }
    text
}

/// The board as markdown for the reader.
pub(super) fn board_document(endpoints: &[ClientShellEndpoint], layout: &ProjectLayout) -> String {
    let (sections, _) = projects::sections(layout, endpoints);
    let project_of = |key: &str| {
        sections
            .iter()
            .find(|section| section.members.iter().any(|member| member.key == key))
            .map(|section| layout.groups[section.group].name.clone())
            .unwrap_or_else(|| "Other".to_owned())
    };
    let mut entries = Vec::new();
    for endpoint in endpoints
        .iter()
        .filter(|endpoint| endpoint.status == ClientEndpointStatus::Online)
    {
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        for agent in &snapshot.agents {
            let Some(workspace) = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == agent.workspace_id)
            else {
                continue;
            };
            let key = projects::workspace_key(endpoint, workspace);
            if layout.is_hidden(&key) {
                continue;
            }
            let presence = layout.presence(
                &projects::agent_key(endpoint, &agent.pane_id),
                agent.state_change_seq,
                agent.agent_status,
            );
            // Red CI counts as needing you (SHE-100004).
            let failing = super::status::failing(&key);
            entries.push(Entry {
                rank: if failing {
                    rank(presence).min(1)
                } else {
                    rank(presence)
                },
                project: project_of(&key),
                text: agent_text(agent, &endpoint.label, &workspace.label, &key, presence),
            });
        }
    }
    if entries.is_empty() {
        return "No agents running.\n".to_owned();
    }
    let needs_you = entries.iter().filter(|entry| entry.rank <= 1).count();
    // Projects ordered by their most urgent agent; agents by urgency inside a project.
    let mut projects_order: Vec<(u8, String)> = Vec::new();
    for entry in &entries {
        match projects_order
            .iter_mut()
            .find(|(_, name)| *name == entry.project)
        {
            Some((best, _)) => *best = (*best).min(entry.rank),
            None => projects_order.push((entry.rank, entry.project.clone())),
        }
    }
    projects_order.sort_by_key(|(best, _)| *best);
    let mut doc = format!("{} agents · {needs_you} need you\n\n", entries.len());
    for (_, project) in projects_order {
        doc.push_str(&format!("## {project}\n\n"));
        let mut members = entries
            .iter()
            .filter(|entry| entry.project == project)
            .collect::<Vec<_>>();
        members.sort_by_key(|entry| entry.rank);
        for entry in members {
            doc.push_str(&entry.text);
            doc.push('\n');
        }
    }
    doc
}
