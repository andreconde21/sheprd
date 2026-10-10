//! andreconde fork: menus, clicks, prompts and keys for sidebar projects.
//! The layout model lives in `projects.rs`; this file only turns UI gestures
//! into layout changes (and, for agents on the active machine, stock methods).

use std::borrow::Cow;

use super::projects;
use super::*;

type Action = ClientContextMenuAction;

/// How many lines the agent peek preview shows.
const PEEK_LINES: usize = 12;

fn item(label: impl Into<Cow<'static, str>>, action: Action) -> ClientContextMenuItem {
    ClientContextMenuItem {
        label: label.into(),
        action,
    }
}

fn move_items(items: &mut Vec<ClientContextMenuItem>, groups: &[String], grouped: bool) {
    for (index, group) in groups.iter().enumerate() {
        items.push(item(format!("→ {group}"), Action::ProjectAssignTo(index)));
    }
    if grouped {
        items.push(item("→ Other", Action::ProjectRemove));
    }
    items.push(item("→ New project…", Action::ProjectAssignNew));
}

/// A project's notification mode: "all" (default), "blocked" or "none".
fn notify_mode(name: &str) -> String {
    projects::layout()
        .groups
        .iter()
        .find(|group| group.name == name)
        .and_then(|group| group.notify.clone())
        .filter(|mode| mode == "blocked" || mode == "none")
        .unwrap_or_else(|| "all".to_owned())
}

/// Read-only usage lines for a project's menu (from the sheprd usage hook).
fn usage_items(name: &str) -> Vec<ClientContextMenuItem> {
    let layout = projects::layout();
    let group = (name != projects::OTHER).then_some(name);
    let line = |label: &str, days: i64| {
        let usage = projects::project_usage(&layout, group, days);
        (usage[4] > 0).then(|| {
            item(
                format!(
                    "{label}: {} · {} tokens",
                    projects::format_minutes(usage[4]),
                    projects::format_tokens(usage[0] + usage[1] + usage[3])
                ),
                Action::Info,
            )
        })
    };
    [line("Today", 1), line("Last 7 days", 7)]
        .into_iter()
        .flatten()
        .collect()
}

pub(super) fn project_menu_items(target: &ClientContextMenuTarget) -> Vec<ClientContextMenuItem> {
    match target {
        ClientContextMenuTarget::ProjectWorkspace {
            grouped,
            hidden,
            groups,
            base,
            ..
        } => {
            let mut items = base
                .as_deref()
                .map(super::context_menu::items_for)
                .unwrap_or_default();
            move_items(&mut items, groups, *grouped);
            if *grouped {
                items.push(item("Move up", Action::ProjectMoveUp));
                items.push(item("Move down", Action::ProjectMoveDown));
            }
            items.push(item(
                if *hidden { "Unhide" } else { "Hide" },
                Action::ProjectToggleHidden,
            ));
            items
        }
        ClientContextMenuTarget::Project {
            name,
            pinned,
            collapsed,
            ..
        } => {
            let collapse = item(
                if *collapsed { "Expand" } else { "Collapse" },
                Action::ProjectToggleCollapse,
            );
            let usage = usage_items(name);
            if name == projects::OTHER {
                let mut items = vec![
                    item("New agent…", Action::ProjectNewAgent),
                    item("New workspace…", Action::ProjectNewWorkspace),
                    collapse,
                ];
                items.extend(usage);
                return items;
            }
            let mut items = vec![
                item("New agent here…", Action::ProjectNewAgent),
                item("New workspace here…", Action::ProjectNewWorkspace),
                collapse,
                item(
                    if *pinned { "Unpin" } else { "Pin to top" },
                    Action::ProjectTogglePin,
                ),
                item("Move up", Action::ProjectMoveUp),
                item("Move down", Action::ProjectMoveDown),
                item("Note…", Action::ProjectNote),
                item("Close idle workspaces…", Action::ProjectTidy),
                item(
                    format!("Notify: {} (click to change)", notify_mode(name)),
                    Action::ProjectNotify,
                ),
                item("Rename…", Action::ProjectRename),
                item("Auto-match rules…", Action::ProjectRules),
                item("Delete project", Action::ProjectDelete),
            ];
            items.extend(usage);
            items
        }
        ClientContextMenuTarget::TidyConfirm { targets, labels } => {
            let mut items = if targets.is_empty() {
                vec![item("Nothing idle for 7+ days here", Action::Info)]
            } else {
                vec![item(
                    format!("Close these {} (idle 7+ days)", targets.len()),
                    Action::TidyConfirm,
                )]
            };
            items.extend(
                labels
                    .iter()
                    .map(|label| item(format!("  {label}"), Action::Info)),
            );
            items
        }
        ClientContextMenuTarget::Info { lines } => lines
            .iter()
            .map(|line| item(line.clone(), Action::Info))
            .collect(),
        ClientContextMenuTarget::NewWorkspacePicker { machines, .. } => machines
            .iter()
            .enumerate()
            .map(|(index, (_, label))| item(format!("on {label}"), Action::NewOnMachine(index)))
            .collect(),
        ClientContextMenuTarget::TaskPicker { tasks } => tasks
            .iter()
            .enumerate()
            .map(|(index, task)| item(format!("→ {task}"), Action::TaskOpen(index)))
            .collect(),
        ClientContextMenuTarget::Agent {
            unread_key,
            seq,
            status,
            timeline,
            tasks,
            workspace_key,
            hidden,
            active,
            groups,
            grouped,
            ..
        } => {
            let presence = projects::layout().presence(unread_key, *seq, *status);
            let mut items = vec![
                item("Go to", Action::AgentFocus),
                item("Peek last lines", Action::AgentPeek),
            ];
            if !timeline.is_empty() {
                items.push(item("Timeline", Action::AgentTimeline));
            }
            if !tasks.is_empty() {
                items.push(item(
                    format!("Mentioned ({})", tasks.len()),
                    Action::AgentTasks,
                ));
            }
            // Status and active-view membership are separate controls; hiding is per workspace.
            let layout = projects::layout();
            if presence.needs_attention() {
                items.push(item("Mark read", Action::AgentMarkInactive));
            } else {
                items.push(item("Mark unread", Action::AgentMarkUnread));
            }
            let in_active = presence.is_active()
                || (!layout.is_idled(unread_key, *seq)
                    && projects::idle_secs(unread_key)
                        .is_some_and(|secs| secs < layout.recent_secs()));
            items.push(if layout.is_kept(unread_key) {
                item("Stop keeping active", Action::AgentToggleKeep)
            } else if in_active {
                item("Remove from active", Action::AgentRemoveFromActive)
            } else {
                item("Keep active", Action::AgentKeepActive)
            });
            if *active {
                items.push(item("Rename pane…", Action::AgentRename));
            }
            if workspace_key.is_some() {
                move_items(&mut items, groups, *grouped);
                items.push(item(
                    if *hidden {
                        "Unhide workspace"
                    } else {
                        "Hide workspace"
                    },
                    Action::ProjectToggleHidden,
                ));
            }
            items
        }
        _ => Vec::new(),
    }
}

/// A merged workspace menu whose picked index falls inside the stock part:
/// hand back the stock menu so the upstream activation code runs unchanged.
pub(super) fn split_project_menu(
    menu: ClientContextMenuOverlay,
    index: usize,
) -> Result<ClientContextMenuOverlay, ClientContextMenuOverlay> {
    match menu.target {
        ClientContextMenuTarget::ProjectWorkspace {
            base: Some(base), ..
        } if index < super::context_menu::items_for(&base).len() => Ok(ClientContextMenuOverlay {
            target: *base,
            x: menu.x,
            y: menu.y,
            highlighted: index,
        }),
        _ => Err(menu),
    }
}

impl ClientShellState {
    fn endpoint_by_id(&self, endpoint_id: &ClientEndpointId) -> Option<&ClientShellEndpoint> {
        self.endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
    }

    fn workspace_label_and_paths(
        &self,
        endpoint_id: &ClientEndpointId,
        workspace_id: &str,
    ) -> Option<(String, String, Vec<String>)> {
        let endpoint = self.endpoint_by_id(endpoint_id)?;
        let snapshot = endpoint.snapshot.as_deref()?;
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)?;
        Some((
            projects::workspace_key(endpoint, workspace),
            workspace.label.clone(),
            projects::workspace_paths(snapshot, workspace),
        ))
    }

    /// Project a workspace currently belongs to (explicitly or by rule).
    fn workspace_group(
        &self,
        endpoint_id: &ClientEndpointId,
        workspace_id: &str,
    ) -> Option<String> {
        let (key, label, paths) = self.workspace_label_and_paths(endpoint_id, workspace_id)?;
        let layout = projects::layout();
        layout
            .group_of(&key, &label, &paths)
            .map(|index| layout.groups[index].name.clone())
    }

    fn group_names() -> Vec<String> {
        let layout = projects::layout();
        layout
            .display_order()
            .into_iter()
            .map(|index| layout.groups[index].name.clone())
            .collect()
    }

    fn workspace_target(
        &mut self,
        endpoint_id: &ClientEndpointId,
        workspace_id: &str,
        x: u16,
        y: u16,
    ) -> Option<ClientContextMenuTarget> {
        let (key, _, _) = self.workspace_label_and_paths(endpoint_id, workspace_id)?;
        let base = if endpoint_id == &self.active_endpoint_id {
            self.open_workspace_context_menu(workspace_id.to_owned(), x, y);
            match self.overlay.take() {
                Some(ClientShellOverlay::ContextMenu(menu)) => Some(Box::new(menu.target)),
                other => {
                    self.overlay = other;
                    None
                }
            }
        } else {
            None
        };
        let layout = projects::layout();
        Some(ClientContextMenuTarget::ProjectWorkspace {
            grouped: self.workspace_group(endpoint_id, workspace_id).is_some(),
            hidden: layout.is_hidden(&key),
            groups: Self::group_names(),
            key,
            base,
        })
    }

    fn agent_target(
        &self,
        endpoint_id: ClientEndpointId,
        pane_id: String,
    ) -> Option<ClientContextMenuTarget> {
        let endpoint = self.endpoint_by_id(&endpoint_id)?;
        let agent = endpoint
            .snapshot
            .as_deref()?
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)?;
        let (seq, status, workspace_id) = (
            agent.state_change_seq,
            agent.agent_status,
            agent.workspace_id.clone(),
        );
        let timeline = projects::agent_timeline(agent);
        let tasks = projects::agent_tasks(agent);
        let unread_key = projects::agent_key(endpoint, &pane_id);
        let workspace_key = self
            .workspace_label_and_paths(&endpoint_id, &workspace_id)
            .map(|(key, _, _)| key);
        Some(ClientContextMenuTarget::Agent {
            unread_key,
            seq,
            status,
            hidden: workspace_key
                .as_deref()
                .is_some_and(|key| projects::layout().is_hidden(key)),
            grouped: self.workspace_group(&endpoint_id, &workspace_id).is_some(),
            groups: Self::group_names(),
            workspace_key,
            active: endpoint_id == self.active_endpoint_id,
            endpoint_id,
            pane_id,
            timeline,
            tasks,
        })
    }

    fn open_menu(&mut self, target: ClientContextMenuTarget, x: u16, y: u16) {
        projects::set_peek_anchor((x, y));
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target,
            x,
            y,
            highlighted: 0,
        }));
    }

    fn header_at(&self, point: (u16, u16)) -> Option<String> {
        self.hits
            .projects
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, key)| key.clone())
    }

    fn row_at(&self, point: (u16, u16)) -> Option<super::sheprd_sidebar::RowHit> {
        self.hits
            .sheprd_rows
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .cloned()
    }

    /// Right-click in the sheprd sidebar: headers, agent rows, workspace rows.
    pub(super) fn open_project_context_menu_at(
        &mut self,
        point: (u16, u16),
        x: u16,
        y: u16,
    ) -> bool {
        if let Some(key) = self.header_at(point) {
            let layout = projects::layout();
            let target = if key == projects::OTHER {
                ClientContextMenuTarget::Project {
                    name: key,
                    pinned: false,
                    collapsed: layout.other_collapsed,
                }
            } else {
                let Some(group) = layout.groups.iter().find(|group| group.name == key) else {
                    return false;
                };
                ClientContextMenuTarget::Project {
                    pinned: group.pinned,
                    collapsed: group.collapsed,
                    name: key,
                }
            };
            self.open_menu(target, x, y);
            return true;
        }
        if self.sidebar_collapsed {
            return false;
        }
        let Some(row) = self.row_at(point) else {
            return false;
        };
        let target = match row.pane_id {
            Some(pane_id) => self.agent_target(row.endpoint_id, pane_id),
            None => self.workspace_target(&row.endpoint_id, &row.workspace_id, x, y),
        };
        match target {
            Some(target) => {
                self.open_menu(target, x, y);
                true
            }
            None => false,
        }
    }

    /// Left-button down on a sidebar row: remember it; click vs drag is decided
    /// when the button comes up.
    pub(super) fn begin_row_press(&mut self, point: (u16, u16)) -> bool {
        let Some(row) = self.row_at(point) else {
            return false;
        };
        projects::set_press(Some(projects::RowPress {
            endpoint_id: row.endpoint_id,
            workspace_id: row.workspace_id,
            pane_id: row.pane_id,
            start: point,
            dragging: None,
        }));
        true
    }

    /// Left-button up after a row press. A click focuses the agent/workspace;
    /// a drag onto a header moves the workspace to that project (or Other), and
    /// a drag onto another row moves it into that row's project, just above it.
    pub(super) fn finish_row_press(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(press) = projects::press() else {
            return false;
        };
        projects::set_press(None);
        self.chrome_drag = None;
        self.workspace_press = None;
        outcome.repaint = true;
        if press.dragging.is_none() {
            let target = match press.pane_id {
                Some(pane_id) => ClientEndpointFocusTarget::Pane(pane_id),
                None => ClientEndpointFocusTarget::Workspace(press.workspace_id),
            };
            self.focus_or_activate(press.endpoint_id, target, outcome);
            return true;
        }
        let Some((key, _, _)) =
            self.workspace_label_and_paths(&press.endpoint_id, &press.workspace_id)
        else {
            return true;
        };
        if let Some(header) = self.header_at(point) {
            let name = if header == projects::OTHER {
                String::new()
            } else {
                header
            };
            projects::update(|layout| {
                layout.assign(&key, &name);
                if let Some(group) = layout.group_mut(&name) {
                    group.collapsed = false;
                }
            });
            return true;
        }
        let Some(target) = self.row_at(point) else {
            return true;
        };
        if target.endpoint_id == press.endpoint_id && target.workspace_id == press.workspace_id {
            return true;
        }
        let target_key = self
            .workspace_label_and_paths(&target.endpoint_id, &target.workspace_id)
            .map(|(key, _, _)| key);
        let group = self.workspace_group(&target.endpoint_id, &target.workspace_id);
        let endpoints = &self.endpoints;
        projects::update(|layout| {
            layout.assign(&key, group.as_deref().unwrap_or(""));
            let (Some(group), Some(target_key)) = (group.as_deref(), target_key) else {
                return;
            };
            let mut view = projects::group_members_in_view(layout, endpoints, &key);
            view.retain(|member| member != &key);
            let position = view
                .iter()
                .position(|member| member == &target_key)
                .unwrap_or(view.len());
            view.insert(position, key.clone());
            if let Some(group) = layout.group_mut(group) {
                group.members = view;
            }
        });
        true
    }

    /// Left-click on a project header toggles it; the header toggles switch
    /// the view (detailed/compact) and the filter (all/active).
    pub(super) fn handle_project_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let sheprd_sidebar = self.hits.sheprd_view_toggle.width > 0;
        if sheprd_sidebar && super::contains(self.hits.new_workspace, point) {
            let project = self.focused_project();
            self.open_new_workspace_picker(project, false);
            outcome.repaint = true;
            return true;
        }
        if let Some(key) = self
            .hits
            .sheprd_rail
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, key)| key.clone())
        {
            if let Some((endpoint_id, target)) = super::sheprd_sidebar::project_target(
                &self.endpoints,
                &self.active_endpoint_id,
                &key,
            ) {
                self.focus_or_activate(endpoint_id, target, outcome);
            }
            outcome.repaint = true;
            return true;
        }
        if let Some(key) = self
            .hits
            .sheprd_todo_toggle
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, key)| key.clone())
        {
            projects::toggle_todo(&key);
            outcome.repaint = true;
            return true;
        }
        if super::contains(self.hits.sheprd_attention, point) {
            self.focus_next_attention_agent(outcome);
            return true;
        }
        if super::contains(self.hits.sheprd_board, point) {
            self.open_board();
            outcome.repaint = true;
            return true;
        }
        if super::contains(self.hits.sheprd_hidden_toggle, point) {
            self.toggle_show_hidden_workspaces();
        } else if super::contains(self.hits.sheprd_view_toggle, point) {
            projects::update(|layout| layout.compact = !layout.compact);
        } else if super::contains(self.hits.sheprd_filter_toggle, point) {
            projects::update(|layout| layout.active_only = !layout.active_only);
            self.workspace_scroll = 0;
        } else if let Some(key) = self.header_at(point) {
            projects::update(|layout| {
                if key == projects::OTHER {
                    layout.other_collapsed = !layout.other_collapsed;
                } else if let Some(group) = layout.group_mut(&key) {
                    group.collapsed = !group.collapsed;
                }
            });
        } else {
            return false;
        }
        outcome.repaint = true;
        true
    }

    /// prefix+u: focus the next agent (in sidebar order) that is blocked,
    /// finished-unseen or marked unread.
    pub(super) fn focus_next_attention_agent(&mut self, outcome: &mut ClientShellInput) {
        let layout = projects::layout();
        let rows = super::aggregate_navigation::aggregate_agent_rows(
            &self.endpoints,
            &self.active_endpoint_id,
            crate::config::AgentPanelSortConfig::Spaces,
        );
        let targets = rows
            .iter()
            .filter(|row| !row.endpoint.stale())
            .map(|row| {
                let endpoint = &self.endpoints[row.endpoint.endpoint_index];
                let presence = layout.presence(
                    &projects::agent_key(endpoint, &row.agent.pane_id),
                    row.agent.state_change_seq,
                    row.agent.agent_status,
                );
                // Red CI on the agent's workspace counts as needing you (SHE-100004).
                let failing = endpoint.snapshot.as_deref().is_some_and(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == row.agent.workspace_id)
                        .is_some_and(|workspace| {
                            super::status::failing(&projects::workspace_key(endpoint, workspace))
                        })
                });
                (
                    endpoint.endpoint_id.clone(),
                    row.agent.pane_id.clone(),
                    presence.needs_attention() || failing,
                    row.agent.focused && endpoint.endpoint_id == self.active_endpoint_id,
                )
            })
            .collect::<Vec<_>>();
        let current = targets.iter().position(|(_, _, _, focused)| *focused);
        let start = current.map_or(0, |index| index + 1);
        let next = (0..targets.len())
            .map(|offset| (start + offset) % targets.len().max(1))
            .find(|index| targets[*index].2 && Some(*index) != current);
        match next {
            Some(index) => {
                let (endpoint_id, pane_id, _, _) = targets[index].clone();
                self.focus_or_activate(
                    endpoint_id,
                    ClientEndpointFocusTarget::Pane(pane_id),
                    outcome,
                );
            }
            None => {
                self.receive_endpoint_unavailable("No agent needs you right now".into());
            }
        }
        outcome.repaint = true;
    }

    fn prompt(&mut self, title: &'static str, initial: &str, target: ClientRenameTarget) {
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title,
            input: TextEditor::new(initial, false),
            target,
        }));
    }

    pub(super) fn activate_project_action(
        &mut self,
        target: ClientContextMenuTarget,
        action: Action,
        outcome: &mut ClientShellInput,
    ) {
        match target {
            ClientContextMenuTarget::ProjectWorkspace { key, groups, .. } => match action {
                Action::ProjectAssignTo(index) => {
                    if let Some(name) = groups.get(index) {
                        projects::update(|layout| layout.assign(&key, name));
                    }
                }
                Action::ProjectAssignNew => self.prompt(
                    "move to project",
                    "",
                    ClientRenameTarget::ProjectAssign { key },
                ),
                Action::ProjectRemove => projects::update(|layout| layout.assign(&key, "")),
                Action::ProjectToggleHidden => {
                    projects::update(|layout| layout.toggle_hidden(&key))
                }
                Action::ProjectMoveUp | Action::ProjectMoveDown => {
                    let layout = projects::layout();
                    let view = projects::group_members_in_view(&layout, &self.endpoints, &key);
                    let delta = if action == Action::ProjectMoveUp {
                        -1
                    } else {
                        1
                    };
                    projects::update(|layout| layout.move_member(&view, &key, delta));
                }
                _ => {}
            },
            ClientContextMenuTarget::NewWorkspacePicker {
                machines,
                project,
                run_agent,
            } => {
                if let Action::NewOnMachine(index) = action {
                    if let Some((endpoint_id, _)) = machines.get(index) {
                        self.prompt_new_workspace(endpoint_id.clone(), project, run_agent);
                    }
                }
            }
            ClientContextMenuTarget::TaskPicker { tasks } => {
                if let Action::TaskOpen(index) = action {
                    if let Some(id) = tasks.get(index).cloned() {
                        projects::open_task(id);
                    }
                }
            }
            ClientContextMenuTarget::TidyConfirm { targets, .. } => {
                if action == Action::TidyConfirm {
                    for (endpoint_id, workspace_id) in targets {
                        let method = crate::api::schema::Method::WorkspaceClose(
                            crate::api::schema::WorkspaceCloseParams {
                                workspace_id,
                                close_group: false,
                            },
                        );
                        if let Some(action) = self.endpoint_request(&endpoint_id, method) {
                            outcome.actions.push(action);
                        }
                    }
                }
            }
            ClientContextMenuTarget::Project { name, .. } => match action {
                Action::ProjectNewAgent | Action::ProjectNewWorkspace => self
                    .open_new_workspace_picker(
                        (name != projects::OTHER).then_some(name),
                        action == Action::ProjectNewAgent,
                    ),
                Action::ProjectToggleCollapse => projects::update(|layout| {
                    if name == projects::OTHER {
                        layout.other_collapsed = !layout.other_collapsed;
                    } else if let Some(group) = layout.group_mut(&name) {
                        group.collapsed = !group.collapsed;
                    }
                }),
                Action::ProjectTogglePin => projects::update(|layout| {
                    if let Some(group) = layout.group_mut(&name) {
                        group.pinned = !group.pinned;
                    }
                }),
                Action::ProjectMoveUp => projects::update(|layout| layout.move_group(&name, -1)),
                Action::ProjectMoveDown => projects::update(|layout| layout.move_group(&name, 1)),
                Action::ProjectRename => {
                    let initial = name.clone();
                    self.prompt(
                        "rename project",
                        &initial,
                        ClientRenameTarget::ProjectRename { name },
                    )
                }
                Action::ProjectRules => {
                    let rules = projects::layout()
                        .groups
                        .iter()
                        .find(|group| group.name == name)
                        .map(|group| group.rules.join(", "))
                        .unwrap_or_default();
                    self.prompt(
                        "auto-match workspace names or folders (comma separated)",
                        &rules,
                        ClientRenameTarget::ProjectRules { name },
                    )
                }
                Action::ProjectNotify => {
                    let next = match notify_mode(&name).as_str() {
                        "all" => Some("blocked".to_owned()),
                        "blocked" => Some("none".to_owned()),
                        _ => None,
                    };
                    projects::update(|layout| {
                        if let Some(group) = layout.group_mut(&name) {
                            group.notify = next;
                        }
                    })
                }
                Action::ProjectTidy => {
                    let (targets, labels) = super::sheprd_sidebar::idle_workspaces(
                        &self.endpoints,
                        &self.active_endpoint_id,
                        &name,
                        7 * 24 * 3600,
                    );
                    let (x, y) = projects::peek_anchor();
                    self.open_menu(
                        ClientContextMenuTarget::TidyConfirm { targets, labels },
                        x,
                        y,
                    );
                }
                Action::ProjectNote => {
                    let note = projects::layout()
                        .groups
                        .iter()
                        .find(|group| group.name == name)
                        .and_then(|group| group.note.clone())
                        .unwrap_or_default();
                    self.prompt(
                        "project note (empty removes it)",
                        &note,
                        ClientRenameTarget::ProjectNote { name },
                    )
                }
                Action::ProjectDelete => {
                    projects::update(|layout| layout.groups.retain(|group| group.name != name))
                }
                _ => {}
            },
            ClientContextMenuTarget::Agent {
                endpoint_id,
                pane_id,
                unread_key,
                seq,
                workspace_key,
                groups,
                timeline,
                tasks,
                ..
            } => match action {
                Action::AgentFocus => {
                    self.focus_or_activate(
                        endpoint_id,
                        ClientEndpointFocusTarget::Pane(pane_id),
                        outcome,
                    );
                }
                Action::AgentMarkUnread => {
                    projects::update(|layout| layout.mark(&unread_key, seq, true))
                }
                Action::AgentTimeline => {
                    let (x, y) = projects::peek_anchor();
                    self.open_menu(ClientContextMenuTarget::Info { lines: timeline }, x, y);
                }
                Action::AgentTasks => {
                    let (x, y) = projects::peek_anchor();
                    self.open_menu(ClientContextMenuTarget::TaskPicker { tasks }, x, y);
                }
                Action::AgentPeek => {
                    let lines = (PEEK_LINES * 3).to_string();
                    if let Some(mut command) = self.herdr_cli(
                        &endpoint_id,
                        &[
                            "pane", "read", &pane_id, "--source", "recent", "--lines", &lines,
                        ],
                    ) {
                        std::thread::spawn(move || {
                            let text = command
                                .output()
                                .ok()
                                .filter(|output| output.status.success())
                                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
                            projects::push_peek(text);
                        });
                    }
                }
                Action::AgentToggleKeep => {
                    projects::update(|layout| layout.toggle_kept(&unread_key))
                }
                Action::AgentMarkInactive => {
                    projects::update(|layout| layout.mark(&unread_key, seq, false))
                }
                Action::AgentRemoveFromActive => {
                    projects::update(|layout| layout.remove_from_active(&unread_key, seq))
                }
                Action::AgentKeepActive => {
                    projects::update(|layout| layout.keep_active(&unread_key))
                }
                Action::AgentRename => {
                    let label = self
                        .snapshot
                        .as_deref()
                        .and_then(|snapshot| {
                            snapshot.panes.iter().find(|pane| pane.pane_id == pane_id)
                        })
                        .and_then(|pane| pane.label.clone())
                        .unwrap_or_default();
                    self.prompt("rename pane", &label, ClientRenameTarget::Pane { pane_id })
                }
                Action::ProjectAssignTo(index) => {
                    if let (Some(key), Some(name)) = (workspace_key, groups.get(index)) {
                        projects::update(|layout| layout.assign(&key, name));
                    }
                }
                Action::ProjectRemove => {
                    if let Some(key) = workspace_key {
                        projects::update(|layout| layout.assign(&key, ""));
                    }
                }
                Action::ProjectAssignNew => {
                    if let Some(key) = workspace_key {
                        self.prompt(
                            "move to project",
                            "",
                            ClientRenameTarget::ProjectAssign { key },
                        )
                    }
                }
                Action::ProjectToggleHidden => {
                    if let Some(key) = workspace_key {
                        projects::update(|layout| layout.toggle_hidden(&key))
                    }
                }
                _ => {}
            },
            _ => {}
        }
        outcome.repaint = true;
    }

    /// Answer a fork prompt. Returns the target back when it is a stock one.
    pub(super) fn save_project_prompt(
        &mut self,
        target: ClientRenameTarget,
        text: &str,
        outcome: &mut ClientShellInput,
    ) -> Option<ClientRenameTarget> {
        match target {
            ClientRenameTarget::ProjectAssign { key } => {
                projects::update(|layout| layout.assign(&key, text))
            }
            ClientRenameTarget::ProjectRename { name } => {
                let text = text.trim().to_owned();
                if !text.is_empty() {
                    projects::update(|layout| {
                        if let Some(group) = layout.group_mut(&name) {
                            group.name = text;
                        }
                    })
                }
            }
            ClientRenameTarget::ProjectRules { name } => {
                let rules = text
                    .split(',')
                    .map(str::trim)
                    .filter(|rule| !rule.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                projects::update(|layout| {
                    if let Some(group) = layout.group_mut(&name) {
                        group.rules = rules;
                    }
                })
            }
            ClientRenameTarget::ProjectNote { name } => {
                let note = text.trim().to_owned();
                projects::update(|layout| {
                    if let Some(group) = layout.group_mut(&name) {
                        group.note = (!note.is_empty()).then_some(note);
                    }
                })
            }
            ClientRenameTarget::SidebarFilter => {
                if let Some(target) = super::sheprd_sidebar::filter_selection(
                    &self.endpoints,
                    &self.active_endpoint_id,
                ) {
                    let focus = match target.pane_id {
                        Some(pane_id) => ClientEndpointFocusTarget::Pane(pane_id),
                        None => ClientEndpointFocusTarget::Workspace(target.workspace_id),
                    };
                    self.focus_or_activate(target.endpoint_id, focus, outcome);
                }
                projects::set_filter(None);
            }
            ClientRenameTarget::JumpAgent => {
                if let Some(number) = text
                    .trim()
                    .trim_start_matches('#')
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                {
                    // Same order as the numbers drawn in the sidebar.
                    let target = super::aggregate_navigation::online_agent_targets(
                        &self.endpoints,
                        &self.active_endpoint_id,
                        crate::config::AgentPanelSortConfig::Spaces,
                    )
                    .into_iter()
                    .nth(number - 1);
                    if let Some(target) = target {
                        self.focus_or_activate(
                            target.endpoint_id,
                            ClientEndpointFocusTarget::Pane(target.pane_id),
                            outcome,
                        );
                    }
                }
            }
            ClientRenameTarget::NewWorkspaceOn {
                endpoint_id,
                project,
                run_agent,
                cwd,
            } => self.create_workspace_on(endpoint_id, project, run_agent, cwd, text, outcome),
            other => return Some(other),
        }
        outcome.repaint = true;
        None
    }

    /// Keyboard entry points (bound in keybindings as project_menu / jump_agent
    /// / toggle_hidden_workspaces).
    pub(super) fn open_focused_project_menu(&mut self) {
        let Some(workspace_id) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_workspace_id.clone())
        else {
            return;
        };
        let endpoint_id = self.active_endpoint_id.clone();
        let (x, y) = self
            .hits
            .sheprd_rows
            .iter()
            .find(|hit| hit.endpoint_id == endpoint_id && hit.workspace_id == workspace_id)
            .map_or((2, 2), |hit| (hit.rect.x + 2, hit.rect.y));
        if let Some(target) = self.workspace_target(&endpoint_id, &workspace_id, x, y) {
            self.open_menu(target, x, y);
        }
    }

    /// True when no longer jump number could start with `n` (e.g. 4 of 25).
    pub(super) fn jump_number_is_final(&self, n: usize) -> bool {
        let total = super::aggregate_navigation::online_agent_targets(
            &self.endpoints,
            &self.active_endpoint_id,
            crate::config::AgentPanelSortConfig::Spaces,
        )
        .len();
        n > 0 && n.saturating_mul(10) > total
    }

    /// prefix+/: filter the sidebar as you type.
    pub(super) fn open_sidebar_filter(&mut self) {
        self.prompt(
            "filter · ↑↓ pick · enter go · alt+m mark · alt+k keep · alt+h hide",
            "",
            ClientRenameTarget::SidebarFilter,
        );
    }

    /// alt+m / alt+k / alt+h on the filter's highlighted row.
    pub(super) fn sidebar_filter_action(&mut self, key: char, _outcome: &mut ClientShellInput) {
        let Some(target) =
            super::sheprd_sidebar::filter_selection(&self.endpoints, &self.active_endpoint_id)
        else {
            return;
        };
        let Some(endpoint) = self.endpoint_by_id(&target.endpoint_id) else {
            return;
        };
        match (key, target.pane_id.as_deref()) {
            ('m', Some(pane_id)) | ('k', Some(pane_id)) => {
                let Some(agent) = endpoint.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .agents
                        .iter()
                        .find(|agent| agent.pane_id == pane_id)
                }) else {
                    return;
                };
                let agent_key = projects::agent_key(endpoint, pane_id);
                let (seq, status) = (agent.state_change_seq, agent.agent_status);
                if key == 'k' {
                    projects::update(|layout| layout.toggle_kept(&agent_key));
                } else {
                    projects::update(|layout| {
                        let unread = !layout.presence(&agent_key, seq, status).needs_attention();
                        layout.mark(&agent_key, seq, unread);
                    });
                }
            }
            ('h', _) => {
                if let Some((key, _, _)) =
                    self.workspace_label_and_paths(&target.endpoint_id, &target.workspace_id)
                {
                    projects::update(|layout| layout.toggle_hidden(&key));
                }
            }
            _ => {}
        }
    }

    pub(super) fn open_jump_agent_prompt(&mut self) {
        self.prompt("jump to agent #", "", ClientRenameTarget::JumpAgent);
    }

    pub(super) fn toggle_peek(&mut self) {
        projects::toggle_peek();
    }

    pub(super) fn toggle_show_hidden_workspaces(&mut self) {
        projects::update(|layout| layout.show_hidden = !layout.show_hidden);
    }

    /// Machines you can create a workspace on, active one first.
    fn online_machines(&self) -> Vec<(ClientEndpointId, String)> {
        let mut machines = self
            .endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.endpoint_id.is_local() || endpoint.status == ClientEndpointStatus::Online
            })
            .map(|endpoint| (endpoint.endpoint_id.clone(), endpoint.label.clone()))
            .collect::<Vec<_>>();
        machines.sort_by_key(|(endpoint_id, _)| endpoint_id != &self.active_endpoint_id);
        machines
    }

    /// Project of the focused workspace on the active machine.
    fn focused_project(&self) -> Option<String> {
        let workspace_id = self.snapshot.as_deref()?.focused_workspace_id.clone()?;
        self.workspace_group(&self.active_endpoint_id, &workspace_id)
    }

    /// prefix+alt+c, the footer "new" button and the project menu land here.
    pub(super) fn open_new_workspace_picker(&mut self, project: Option<String>, run_agent: bool) {
        let machines = self.online_machines();
        if machines.len() <= 1 {
            if let Some((endpoint_id, _)) = machines.into_iter().next() {
                self.prompt_new_workspace(endpoint_id, project, run_agent);
            }
            return;
        }
        let (x, y) = if self.hits.new_workspace.height > 0 {
            (
                self.hits.new_workspace.x + 1,
                self.hits
                    .new_workspace
                    .y
                    .saturating_sub(machines.len() as u16 + 2),
            )
        } else {
            (2, 2)
        };
        self.open_menu(
            ClientContextMenuTarget::NewWorkspacePicker {
                machines,
                project,
                run_agent,
            },
            x,
            y,
        );
    }

    pub(super) fn open_new_workspace_for_focus(&mut self) {
        let project = self.focused_project();
        self.open_new_workspace_picker(project, false);
    }

    fn prompt_new_workspace(
        &mut self,
        endpoint_id: ClientEndpointId,
        project: Option<String>,
        run_agent: bool,
    ) {
        // Start in the project's folder on that machine when it has one there.
        let cwd = project.as_deref().and_then(|project| {
            let layout = projects::layout();
            let (sections, _) = projects::sections(&layout, &self.endpoints);
            sections
                .into_iter()
                .filter(|section| layout.groups[section.group].name == project)
                .flat_map(|section| section.members)
                .find_map(|member| {
                    let endpoint = &self.endpoints[member.endpoint];
                    (endpoint.endpoint_id == endpoint_id).then_some(())?;
                    let workspace = endpoint.snapshot.as_deref()?.workspaces.get(member.index)?;
                    (!workspace.new_workspace_cwd.is_empty())
                        .then(|| workspace.new_workspace_cwd.clone())
                })
        });
        let initial = project.clone().unwrap_or_default();
        self.prompt(
            if run_agent {
                "new agent: workspace name"
            } else {
                "new workspace name"
            },
            &initial,
            ClientRenameTarget::NewWorkspaceOn {
                endpoint_id,
                project,
                run_agent,
                cwd,
            },
        );
    }

    /// herdr's own CLI (this binary), aimed at one machine. The client
    /// connection only carries a fixed set of methods, so reading a pane or
    /// typing into one goes through the CLI, which reaches any saved machine.
    fn herdr_cli(
        &self,
        endpoint_id: &ClientEndpointId,
        args: &[&str],
    ) -> Option<std::process::Command> {
        let exe = std::env::current_exe().ok()?;
        let mut command = std::process::Command::new(exe);
        if !endpoint_id.is_local() {
            command
                .arg("--machine")
                .arg(&self.endpoint_by_id(endpoint_id)?.label);
        }
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        Some(command)
    }

    /// A reference body opens in the markdown reader (scrollable, esc closes).
    /// The status board (SHE-100000) in the reader.
    pub(super) fn open_board(&mut self) {
        let doc = super::board::board_document(&self.endpoints, &projects::layout());
        self.show_task_body("status board", Some(doc));
    }

    fn show_task_body(&mut self, id: &str, text: Option<String>) {
        let (version, reader, body) = match text {
            Some(text) => {
                let (title, subtitle, body) = projects::reader_document(id, &text);
                (title, subtitle, body)
            }
            None => (
                id.to_owned(),
                "not found".to_owned(),
                "No reference source matches this id, or its command failed.".to_owned(),
            ),
        };
        self.overlay = Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState {
                version,
                body,
                scroll: 0,
                preview: false,
                reader: Some(reader),
            },
        ));
        self.chrome_drag = None;
    }

    /// A finished peek: show the agent's last lines where its menu was.
    fn show_peek(&mut self, text: Option<String>) {
        let lines = match text {
            Some(text) => {
                let lines = text
                    .lines()
                    .map(str::trim_end)
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| line.chars().take(72).collect::<String>())
                    .collect::<Vec<_>>();
                let start = lines.len().saturating_sub(PEEK_LINES);
                lines[start..].to_vec()
            }
            None => vec!["(could not read this agent)".to_owned()],
        };
        let (x, y) = projects::peek_anchor();
        self.open_menu(ClientContextMenuTarget::Info { lines }, x, y);
    }

    /// Send one API request to a specific machine (not just the active one).
    fn endpoint_request(
        &mut self,
        endpoint_id: &ClientEndpointId,
        method: crate::api::schema::Method,
    ) -> Option<ClientShellAction> {
        let boot_id = self
            .endpoint_by_id(endpoint_id)?
            .snapshot
            .as_deref()?
            .boot_id
            .clone();
        let id = format!("sheprd:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        Some(ClientShellAction::Endpoint {
            endpoint_id: endpoint_id.clone(),
            boot_id,
            request: Box::new(crate::api::schema::Request { id, method }),
        })
    }

    fn create_workspace_on(
        &mut self,
        endpoint_id: ClientEndpointId,
        project: Option<String>,
        run_agent: bool,
        cwd: Option<String>,
        text: &str,
        outcome: &mut ClientShellInput,
    ) {
        let label = Some(text.trim())
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
            .or_else(|| project.clone())
            .unwrap_or_else(|| "workspace".to_owned());
        let Some(endpoint) = self.endpoint_by_id(&endpoint_id) else {
            return;
        };
        let machine = projects::machine_key(endpoint);
        let known = endpoint
            .snapshot
            .as_deref()
            .map(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.workspace_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let method = crate::api::schema::Method::WorkspaceCreate(
            crate::api::schema::WorkspaceCreateParams {
                source_workspace_id: None,
                cwd,
                focus: true,
                label: Some(label.clone()),
                env: Default::default(),
            },
        );
        let Some(action) = self.endpoint_request(&endpoint_id, method) else {
            return;
        };
        outcome.actions.push(action);
        if let Some(project) = project.as_deref() {
            let key = format!("{machine}/{label}");
            projects::update(|layout| layout.assign(&key, project));
        }
        projects::set_launch(Some(projects::PendingLaunch {
            endpoint_id: endpoint_id.clone(),
            label,
            known,
            command: run_agent.then(|| "cc".to_owned()),
            since: std::time::Instant::now(),
        }));
        if endpoint_id != self.active_endpoint_id {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: None,
            });
        }
    }

    /// Periodic sheprd work, from the client loop's 100 ms timer.
    pub(crate) fn tick_sheprd(&mut self, outcome: &mut ClientShellInput) {
        static ECHO_LOADED: std::sync::Once = std::sync::Once::new();
        ECHO_LOADED.call_once(|| super::predict::set_enabled(!projects::layout().local_echo_off));
        outcome.repaint |= super::predict::predictor().expire(std::time::Instant::now());
        if self.endpoints.len() > 1 {
            crate::sheprd_msg::ensure_relay();
        }
        super::view_sync::tick(&self.endpoints, &self.active_endpoint_id);
        super::status::tick(&self.endpoints);
        if self.work_panel == super::work_panel::FULL {
            super::work_panel::request_summary(&self.endpoints, &self.active_endpoint_id);
        }
        outcome.actions.extend(self.tick_sheprd_launch());
        for (endpoint_id, pane_id) in projects::take_focus_requests() {
            self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Pane(pane_id),
                outcome,
            );
            outcome.repaint = true;
        }
        outcome.repaint |= projects::expire_peek();
        if let Some((id, text)) = projects::take_task_body() {
            self.show_task_body(&id, text);
            outcome.repaint = true;
        }
        if let Some(text) = projects::take_peek() {
            self.show_peek(text);
            outcome.repaint = true;
        }
        if projects::status_due() {
            projects::write_status(&super::sheprd_sidebar::status_json(
                &self.endpoints,
                &self.active_endpoint_id,
            ));
        }
    }

    /// Once the workspace created above shows up, type the agent command into
    /// its first pane (and stop waiting after a minute).
    fn tick_sheprd_launch(&mut self) -> Vec<ClientShellAction> {
        let Some(launch) = projects::launch() else {
            return Vec::new();
        };
        if launch.since.elapsed().as_secs() > 60 {
            projects::set_launch(None);
            return Vec::new();
        }
        let pane_id = self
            .endpoint_by_id(&launch.endpoint_id)
            .and_then(|endpoint| {
                let snapshot = endpoint.snapshot.as_deref()?;
                let workspace = snapshot.workspaces.iter().find(|workspace| {
                    workspace.label == launch.label
                        && !launch.known.contains(&workspace.workspace_id)
                })?;
                snapshot
                    .panes
                    .iter()
                    .find(|pane| pane.workspace_id == workspace.workspace_id)
                    .map(|pane| pane.pane_id.clone())
            });
        let Some(pane_id) = pane_id else {
            return Vec::new();
        };
        projects::set_launch(None);
        let Some(command) = launch.command else {
            return Vec::new();
        };
        if let Some(mut run) =
            self.herdr_cli(&launch.endpoint_id, &["pane", "run", &pane_id, &command])
        {
            run.stdout(std::process::Stdio::null());
            std::thread::spawn(move || {
                let _ = run.status();
            });
        }
        Vec::new()
    }

    /// Quiet projects: drop a notification (toast and sound) when the agent's
    /// project is set to "none", or to "blocked" and the agent merely finished.
    pub(super) fn sheprd_notification_muted(
        &self,
        endpoint_id: &ClientEndpointId,
        event: &crate::protocol::SemanticNotification,
    ) -> bool {
        let Some(workspace_id) = event.workspace_id.as_deref() else {
            return false;
        };
        let Some(project) = self.workspace_group(endpoint_id, workspace_id) else {
            return false;
        };
        match notify_mode(&project).as_str() {
            "none" => true,
            "blocked" => event.kind != crate::protocol::SemanticNotificationKind::NeedsAttention,
            _ => false,
        }
    }
}
