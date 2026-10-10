//! andreconde fork (sheprd): client-local project groups for the federated sidebar.
//! User-facing docs: .github/README.md.
//!
//! Projects group workspaces from any machine (Local, gpu-box, ...) under one header,
//! independent of where the panes live. The layout is purely client-side and
//! lives in `<config_dir>/sidebar.toml`, so the stock server never sees it. It is
//! hand-editable; UI actions (right-click menus, keys) rewrite it.
//!
//! ```toml
//! show_hidden = false
//! hidden = ["gpu-box/scratch"]
//!
//! [[group]]
//! name = "Storefront"
//! pinned = true
//! members = ["gpu-box/Storefront", "local/Storefront"]   # machine/workspace label
//! match = ["storefront"]                                 # auto-assign by label substring
//! ```
//!
//! Kept in its own module behind a process-wide lock so the upstream render and
//! navigation signatures stay untouched (cheap rebases).

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{OnceLock, RwLock},
    time::{Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

use super::ClientShellEndpoint;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(super) struct ProjectGroup {
    pub(super) name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) collapsed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) members: Vec<String>,
    #[serde(default, rename = "match", skip_serializing_if = "Vec::is_empty")]
    pub(super) rules: Vec<String>,
    /// One-line note shown under the project header ("waiting on client reply").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    /// Notifications for this project's agents: "all" (default), "blocked"
    /// (only when one waits on you) or "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) notify: Option<String>,
    /// Two-letter tag for the collapsed rail (default: derived from the name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) short: Option<String>,
}

/// Rail tag: explicit `short`, else capitals ("StoreFront" -> "SF"), else
/// first letters of the first two words ("Acme ops" -> "AO"), else the
/// first two letters ("Infrastructure" -> "In").
pub(super) fn project_tag(name: &str, short: Option<&str>) -> String {
    if let Some(short) = short.filter(|short| !short.trim().is_empty()) {
        return short.trim().chars().take(2).collect();
    }
    let words = name.split_whitespace().collect::<Vec<_>>();
    if words.len() >= 2 {
        return words
            .iter()
            .take(2)
            .filter_map(|word| word.chars().next())
            .flat_map(char::to_uppercase)
            .collect();
    }
    let capitals = name
        .chars()
        .filter(|c| c.is_uppercase())
        .take(2)
        .collect::<String>();
    if capitals.chars().count() == 2 {
        return capitals;
    }
    let mut chars = name.chars();
    let first = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    let second = chars
        .next()
        .map(|c| c.to_lowercase().collect::<String>())
        .unwrap_or_default();
    format!("{first}{second}")
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(super) struct ProjectLayout {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) show_hidden: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) hidden: Vec<String>,
    /// Agents marked unread by hand (`machine/pane_id`); cleared when focused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) unread: Vec<String>,
    /// Compact view: one line per workspace instead of one row per agent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) compact: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) other_collapsed: bool,
    /// Show only agents that are working or need attention.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) active_only: bool,
    /// Agents marked inactive by hand, as `machine/pane@state_change_seq`: the
    /// mark lapses as soon as the agent changes state again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) dismissed: Vec<String>,
    /// Agents taken out of the active view by hand, as `machine/pane@state_change_seq`: they
    /// show under "all agents" only, until the agent changes state again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) idled: Vec<String>,
    /// Agents pinned to the active view by hand (`machine/pane`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) kept: Vec<String>,
    /// How long an idle agent still counts as active (default 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) recent_hours: Option<u64>,
    /// Live sidebar filter while the filter prompt is open (never saved).
    #[serde(skip)]
    pub(super) filter: Option<String>,
    /// Workspaces dragged to "Other": never auto-matched into a project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) ungrouped: Vec<String>,
    #[serde(default, rename = "group", skip_serializing_if = "Vec::is_empty")]
    pub(super) groups: Vec<ProjectGroup>,
    /// Share this view with apps that mirror sheprd (~/.local/state/sheprd/view.json).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) share_view: bool,
    /// Predictive local echo for panes on other machines is on unless this is set (SHE-100003).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) local_echo_off: bool,
}

struct Store {
    layout: ProjectLayout,
    mtime: Option<SystemTime>,
    checked: Instant,
    last_focused_agent: Option<String>,
}

fn path() -> PathBuf {
    crate::config::config_dir().join("sidebar.toml")
}

fn read_file() -> (ProjectLayout, Option<SystemTime>) {
    // Unit tests must never see (or depend on) the developer's real layout.
    if cfg!(test) {
        return (ProjectLayout::default(), None);
    }
    let path = path();
    let mtime = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let layout = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default();
    (layout, mtime)
}

fn store() -> &'static RwLock<Store> {
    static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let (layout, mtime) = read_file();
        RwLock::new(Store {
            layout,
            mtime,
            checked: Instant::now(),
            last_focused_agent: None,
        })
    })
}

/// Current layout; picks up hand edits to sidebar.toml at most once a second.
pub(super) fn layout() -> ProjectLayout {
    let store = store();
    {
        let guard = store.read().unwrap_or_else(|e| e.into_inner());
        if guard.checked.elapsed().as_secs() < 1 {
            return guard.layout.clone();
        }
    }
    let mut guard = store.write().unwrap_or_else(|e| e.into_inner());
    guard.checked = Instant::now();
    let mtime = if cfg!(test) {
        None
    } else {
        std::fs::metadata(path())
            .and_then(|meta| meta.modified())
            .ok()
    };
    if mtime != guard.mtime {
        let (layout, mtime) = read_file();
        guard.layout = layout;
        guard.mtime = mtime;
    }
    guard.layout.clone()
}

/// Apply a change and persist it.
pub(super) fn update(change: impl FnOnce(&mut ProjectLayout)) {
    let _ = layout();
    let store = store();
    let mut guard = store.write().unwrap_or_else(|e| e.into_inner());
    let before = guard.layout.clone();
    change(&mut guard.layout);
    if guard.layout == before {
        return;
    }
    if cfg!(test) {
        return;
    }
    let path = path();
    if let Ok(content) = toml::to_string_pretty(&guard.layout) {
        let header = "# sheprd sidebar projects. Hand-editable.\n";
        let tmp = path.with_extension("toml.tmp");
        if std::fs::write(&tmp, format!("{header}{content}")).is_ok()
            && std::fs::rename(&tmp, &path).is_ok()
        {
            guard.mtime = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok();
        }
    }
}

pub(super) fn machine_key(endpoint: &ClientShellEndpoint) -> String {
    endpoint.label.to_lowercase()
}

/// Stable identity of a workspace: `machine/id:label`. The id survives renames
/// and tells apart workspaces that share a name; the label is only there so the
/// file stays readable. Older name-only entries (`machine/label`) still match.
pub(super) fn workspace_key(
    endpoint: &ClientShellEndpoint,
    workspace: &crate::protocol::ClientShellWorkspace,
) -> String {
    format!(
        "{}/{}:{}",
        machine_key(endpoint),
        workspace.workspace_id,
        workspace.label
    )
}

fn split_id(rest: &str) -> Option<(&str, &str)> {
    let (id, label) = rest.split_once(':')?;
    let looks_like_id =
        id.len() > 1 && id.starts_with('w') && id[1..].chars().all(|c| c.is_ascii_alphanumeric());
    looks_like_id.then_some((id, label))
}

/// Does a stored entry (new or legacy form) refer to the workspace `key`?
pub(super) fn same_workspace(entry: &str, key: &str) -> bool {
    let (Some((entry_machine, entry_rest)), Some((machine, rest))) =
        (entry.split_once('/'), key.split_once('/'))
    else {
        return entry == key;
    };
    if entry_machine != machine {
        return false;
    }
    match (split_id(entry_rest), split_id(rest)) {
        (Some((entry_id, _)), Some((id, _))) => entry_id == id,
        (None, Some((_, label))) => entry_rest == label,
        _ => entry_rest == rest,
    }
}

pub(super) fn agent_key(endpoint: &ClientShellEndpoint, pane_id: &str) -> String {
    format!("{}/{}", machine_key(endpoint), pane_id)
}

impl ProjectLayout {
    /// Groups in display order (pinned first, otherwise file order) as indices.
    pub(super) fn display_order(&self) -> Vec<usize> {
        let mut order = (0..self.groups.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| !self.groups[*index].pinned);
        order
    }

    pub(super) fn is_hidden(&self, key: &str) -> bool {
        self.hidden.iter().any(|hidden| same_workspace(hidden, key))
    }

    /// Hide or unhide one workspace.
    pub(super) fn toggle_hidden(&mut self, key: &str) {
        if self.is_hidden(key) {
            self.hidden.retain(|hidden| !same_workspace(hidden, key));
        } else {
            self.hidden.push(key.to_owned());
        }
    }

    pub(super) fn is_unread(&self, key: &str) -> bool {
        self.unread.iter().any(|unread| unread == key)
    }

    pub(super) fn explicit_group(&self, key: &str) -> Option<usize> {
        self.groups.iter().position(|group| {
            group
                .members
                .iter()
                .any(|member| same_workspace(member, key))
        })
    }

    /// Group for a workspace: explicit membership wins, then the first rule that
    /// matches the workspace name or any folder its panes run in.
    pub(super) fn group_of(&self, key: &str, label: &str, paths: &[String]) -> Option<usize> {
        if self
            .ungrouped
            .iter()
            .any(|ungrouped| same_workspace(ungrouped, key))
        {
            return None;
        }
        self.explicit_group(key).or_else(|| {
            let label = label.to_lowercase();
            let paths = paths
                .iter()
                .map(|path| path.to_lowercase())
                .collect::<Vec<_>>();
            self.groups.iter().position(|group| {
                group.rules.iter().any(|rule| {
                    let rule = rule.to_lowercase();
                    !rule.is_empty()
                        && (label.contains(&rule) || paths.iter().any(|path| path.contains(&rule)))
                })
            })
        })
    }

    /// Rank inside a group: explicit members first in member order, then matches.
    fn member_rank(&self, group: usize, key: &str) -> usize {
        self.groups[group]
            .members
            .iter()
            .position(|member| same_workspace(member, key))
            .unwrap_or(usize::MAX)
    }

    /// Move a workspace into `group_name`; an empty name moves it to Other.
    pub(super) fn assign(&mut self, key: &str, group_name: &str) {
        for group in &mut self.groups {
            group.members.retain(|member| !same_workspace(member, key));
        }
        self.ungrouped
            .retain(|ungrouped| !same_workspace(ungrouped, key));
        let name = group_name.trim();
        if name.is_empty() {
            self.ungrouped.push(key.to_owned());
            return;
        }
        match self
            .groups
            .iter_mut()
            .find(|group| group.name.eq_ignore_ascii_case(name))
        {
            Some(group) => group.members.push(key.to_owned()),
            None => self.groups.push(ProjectGroup {
                name: name.to_owned(),
                members: vec![key.to_owned()],
                ..ProjectGroup::default()
            }),
        }
    }

    /// Move a workspace one step within its group (materialising rule matches
    /// into explicit members so the order sticks).
    pub(super) fn move_member(&mut self, members_in_view: &[String], key: &str, delta: isize) {
        let Some(position) = members_in_view.iter().position(|member| member == key) else {
            return;
        };
        let target = position as isize + delta;
        if target < 0 || target as usize >= members_in_view.len() {
            return;
        }
        let mut members = members_in_view.to_vec();
        members.swap(position, target as usize);
        if let Some(group) = self.explicit_group(key).or_else(|| {
            self.groups.iter().position(|group| {
                members_in_view
                    .iter()
                    .any(|member| group.members.contains(member))
            })
        }) {
            self.groups[group].members = members;
        }
    }

    /// Move a group one step in display order (pinned groups stay above others).
    pub(super) fn move_group(&mut self, name: &str, delta: isize) {
        let order = self.display_order();
        let Some(position) = order
            .iter()
            .position(|index| self.groups[*index].name == name)
        else {
            return;
        };
        let target = position as isize + delta;
        if target < 0 || target as usize >= order.len() {
            return;
        }
        let (a, b) = (order[position], order[target as usize]);
        if self.groups[a].pinned != self.groups[b].pinned {
            return;
        }
        self.groups.swap(a, b);
    }

    pub(super) fn group_mut(&mut self, name: &str) -> Option<&mut ProjectGroup> {
        self.groups.iter_mut().find(|group| group.name == name)
    }
}

/// Folders a workspace lives in: its new-pane cwd plus every pane's cwd.
pub(super) fn workspace_paths(
    snapshot: &crate::protocol::ClientShellSnapshot,
    workspace: &crate::protocol::ClientShellWorkspace,
) -> Vec<String> {
    let mut paths = vec![workspace.new_workspace_cwd.clone()];
    for pane in snapshot
        .panes
        .iter()
        .filter(|pane| pane.workspace_id == workspace.workspace_id)
    {
        paths.extend(pane.foreground_cwd.iter().cloned());
        paths.extend(pane.cwd.iter().cloned());
    }
    paths.retain(|path| !path.is_empty());
    paths
}

/// One workspace placed in the federated sidebar.
#[derive(Clone, Debug)]
pub(super) struct PlacedWorkspace {
    pub(super) endpoint: usize,
    pub(super) index: usize,
    pub(super) key: String,
    pub(super) hidden: bool,
}

pub(super) struct ProjectSection {
    pub(super) group: usize,
    pub(super) members: Vec<PlacedWorkspace>,
}

/// Resolve every workspace of every endpoint into project sections; returns the
/// sections in display order plus the set of (endpoint, index) they claimed.
pub(super) fn sections(
    layout: &ProjectLayout,
    endpoints: &[ClientShellEndpoint],
) -> (Vec<ProjectSection>, HashSet<(usize, usize)>) {
    let mut sections = layout
        .display_order()
        .into_iter()
        .map(|group| ProjectSection {
            group,
            members: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut claimed = HashSet::new();
    for (endpoint_index, endpoint) in endpoints.iter().enumerate() {
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        for (index, workspace) in snapshot.workspaces.iter().enumerate() {
            let key = workspace_key(endpoint, workspace);
            let paths = workspace_paths(snapshot, workspace);
            let Some(group) = layout.group_of(&key, &workspace.label, &paths) else {
                continue;
            };
            claimed.insert((endpoint_index, index));
            let hidden = layout.is_hidden(&key);
            if let Some(section) = sections.iter_mut().find(|section| section.group == group) {
                section.members.push(PlacedWorkspace {
                    endpoint: endpoint_index,
                    index,
                    key,
                    hidden,
                });
            }
        }
    }
    for section in &mut sections {
        section
            .members
            .sort_by_key(|member| layout.member_rank(section.group, &member.key));
    }
    (sections, claimed)
}

/// Visible member keys of the group that owns `key`, in display order.
pub(super) fn group_members_in_view(
    layout: &ProjectLayout,
    endpoints: &[ClientShellEndpoint],
    key: &str,
) -> Vec<String> {
    let (sections, _) = sections(layout, endpoints);
    sections
        .into_iter()
        .find(|section| section.members.iter().any(|member| member.key == key))
        .map(|section| {
            section
                .members
                .into_iter()
                .map(|member| member.key)
                .collect()
        })
        .unwrap_or_default()
}

/// Sort key that puts agents in project order; ungrouped agents keep their
/// original relative order after every project. `None` = hidden, drop it.
pub(super) fn agent_rank(
    layout: &ProjectLayout,
    endpoint: &ClientShellEndpoint,
    workspace: &crate::protocol::ClientShellWorkspace,
    paths: &[String],
) -> Option<(usize, usize)> {
    let workspace_label = workspace.label.as_str();
    let key = workspace_key(endpoint, workspace);
    if layout.is_hidden(&key) && !layout.show_hidden {
        return None;
    }
    let order = layout.display_order();
    Some(match layout.group_of(&key, workspace_label, paths) {
        Some(group) => (
            order.iter().position(|index| *index == group).unwrap_or(0),
            layout.member_rank(group, &key),
        ),
        None => (usize::MAX, 0),
    })
}

/// What the sidebar shows for an agent once manual marks are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    Blocked,
    Unread,
    Done,
    Working,
    Idle,
}

impl Presence {
    pub(super) fn needs_attention(self) -> bool {
        matches!(self, Self::Blocked | Self::Unread | Self::Done)
    }

    pub(super) fn is_active(self) -> bool {
        self != Self::Idle
    }
}

impl ProjectLayout {
    fn dismissed_key(key: &str, seq: u64) -> String {
        format!("{key}@{seq}")
    }

    pub(super) fn presence(
        &self,
        key: &str,
        seq: u64,
        status: crate::api::schema::AgentStatus,
    ) -> Presence {
        use crate::api::schema::AgentStatus;
        if self.is_unread(key) {
            return Presence::Unread;
        }
        let dismissed = self
            .dismissed
            .iter()
            .any(|entry| *entry == Self::dismissed_key(key, seq));
        match status {
            AgentStatus::Working => Presence::Working,
            AgentStatus::Blocked if !dismissed => Presence::Blocked,
            AgentStatus::Done if !dismissed => Presence::Done,
            _ => Presence::Idle,
        }
    }

    /// Manual status: unread (needs attention) or inactive (dismissed until the
    /// agent's next state change).
    pub(super) fn mark(&mut self, key: &str, seq: u64, unread: bool) {
        self.unread.retain(|entry| entry != key);
        let dismissed = Self::dismissed_key(key, seq);
        let prefix = format!("{key}@");
        self.dismissed.retain(|entry| !entry.starts_with(&prefix));
        if unread {
            self.unread.push(key.to_owned());
        } else {
            self.dismissed.push(dismissed);
            let excess = self.dismissed.len().saturating_sub(200);
            self.dismissed.drain(..excess);
        }
    }
}

impl ProjectLayout {
    /// Taken out of the active view since the agent's last state change.
    pub(super) fn is_idled(&self, key: &str, seq: u64) -> bool {
        let entry = Self::dismissed_key(key, seq);
        self.idled.iter().any(|idled| *idled == entry)
    }

    /// Takes an agent out of the active view until its next state change (and marks it read).
    pub(super) fn remove_from_active(&mut self, key: &str, seq: u64) {
        self.mark(key, seq, false);
        self.kept.retain(|kept| kept != key);
        let prefix = format!("{key}@");
        self.idled.retain(|entry| !entry.starts_with(&prefix));
        self.idled.push(Self::dismissed_key(key, seq));
        let excess = self.idled.len().saturating_sub(200);
        self.idled.drain(..excess);
    }

    /// Puts an agent back in the active view: keeps it there, and drops a "removed" mark.
    pub(super) fn keep_active(&mut self, key: &str) {
        let prefix = format!("{key}@");
        self.idled.retain(|entry| !entry.starts_with(&prefix));
        if !self.is_kept(key) {
            self.kept.push(key.to_owned());
        }
    }

    /// Marked read (or inactive) since the agent's last state change.
    pub(super) fn is_dismissed(&self, key: &str, seq: u64) -> bool {
        let entry = Self::dismissed_key(key, seq);
        self.dismissed.iter().any(|dismissed| *dismissed == entry)
    }

    pub(super) fn is_kept(&self, key: &str) -> bool {
        self.kept.iter().any(|kept| kept == key)
    }

    pub(super) fn toggle_kept(&mut self, key: &str) {
        if self.is_kept(key) {
            self.kept.retain(|kept| kept != key);
        } else {
            self.kept.push(key.to_owned());
        }
    }

    pub(super) fn recent_secs(&self) -> u64 {
        self.recent_hours.unwrap_or(24) * 3600
    }
}

/// When each agent last changed state (unix seconds), as observed by this
/// client. herdr sends no timestamps, so sheprd records them itself and keeps
/// them in `<state_dir>/sheprd-activity.json` so restarts don't reset them.
#[derive(Default, Deserialize, Serialize)]
struct Activity {
    #[serde(default)]
    agents: std::collections::HashMap<String, (u64, u64)>,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    saved: Option<Instant>,
    #[serde(skip)]
    primed: HashSet<String>,
}

fn activity_path() -> PathBuf {
    crate::config::state_dir().join("sheprd-activity.json")
}

fn activity() -> &'static std::sync::Mutex<Activity> {
    static ACTIVITY: OnceLock<std::sync::Mutex<Activity>> = OnceLock::new();
    ACTIVITY.get_or_init(|| {
        let loaded = if cfg!(test) {
            None
        } else {
            std::fs::read_to_string(activity_path())
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
        };
        std::sync::Mutex::new(loaded.unwrap_or_default())
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Record state changes from a fresh endpoint snapshot. Agents seen for the
/// first time get no timestamp (unknown age = treated as old), except ones that
/// are working, which are clearly current.
pub(super) fn observe_activity(endpoint: &ClientShellEndpoint) {
    let Some(snapshot) = endpoint.snapshot.as_deref() else {
        return;
    };
    let now = unix_now();
    let mut store = activity().lock().unwrap_or_else(|e| e.into_inner());
    for agent in &snapshot.agents {
        let key = agent_key(endpoint, &agent.pane_id);
        match store.agents.get(&key) {
            Some((seq, _)) if *seq == agent.state_change_seq => {}
            Some(_) => {
                store.agents.insert(key, (agent.state_change_seq, now));
                store.dirty = true;
            }
            None => {
                let at = if agent.agent_status == crate::api::schema::AgentStatus::Working {
                    now
                } else {
                    0
                };
                store.agents.insert(key, (agent.state_change_seq, at));
                store.dirty = true;
            }
        }
    }
    // Workspaces: remember when each first appeared. On the first snapshot of a
    // machine in this process, unknown ones are old (unknown age); after that,
    // a new id is a workspace that was just created.
    let machine = machine_key(endpoint);
    let primed = store.primed.contains(&machine);
    for workspace in &snapshot.workspaces {
        let key = format!("{machine}/ws:{}", workspace.workspace_id);
        if !store.agents.contains_key(&key) {
            store.agents.insert(key, (0, if primed { now } else { 0 }));
            store.dirty = true;
        }
    }
    store.primed.insert(machine);
    drop(store);
    observe_usage(endpoint, snapshot);
    let mut store = activity().lock().unwrap_or_else(|e| e.into_inner());
    let due = store
        .saved
        .is_none_or(|saved| saved.elapsed().as_secs() >= 10);
    if store.dirty && due && !cfg!(test) {
        let month = 30 * 24 * 3600;
        store
            .agents
            .retain(|_, (_, at)| *at == 0 || now.saturating_sub(*at) < month);
        if let Ok(content) = serde_json::to_vec(&*store) {
            let path = activity_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, content).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                store.dirty = false;
                store.saved = Some(Instant::now());
            }
        }
    }
}

/// Seconds since the agent last changed state, if known.
pub(super) fn idle_secs(key: &str) -> Option<u64> {
    let store = activity().lock().unwrap_or_else(|e| e.into_inner());
    store
        .agents
        .get(key)
        .and_then(|(_, at)| (*at > 0).then(|| unix_now().saturating_sub(*at)))
}

/// Token/time usage per agent session, reported by the sheprd usage hook as
/// pane metadata (`sheprd_u_YYYYMMDD`, `sheprd_session`) and kept here so it
/// outlives the agent: `<state_dir>/sheprd-usage.json`.
#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct UsageRecord {
    /// Workspace key at last sighting, plus what project rules match on.
    pub(super) key: String,
    pub(super) label: String,
    #[serde(default)]
    pub(super) paths: Vec<String>,
    /// day -> [input, output, cache_read, cache_write, active_minutes]
    pub(super) days: std::collections::BTreeMap<String, [u64; 5]>,
}

#[derive(Default, Deserialize, Serialize)]
struct UsageStore {
    #[serde(default)]
    sessions: std::collections::HashMap<String, UsageRecord>,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    saved: Option<Instant>,
}

fn usage_path() -> PathBuf {
    crate::config::state_dir().join("sheprd-usage.json")
}

fn usage_store() -> &'static std::sync::Mutex<UsageStore> {
    static USAGE: OnceLock<std::sync::Mutex<UsageStore>> = OnceLock::new();
    USAGE.get_or_init(|| {
        let loaded = if cfg!(test) {
            None
        } else {
            std::fs::read_to_string(usage_path())
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
        };
        std::sync::Mutex::new(loaded.unwrap_or_default())
    })
}

pub(super) fn agent_token<'a>(
    agent: &'a crate::protocol::ClientShellAgent,
    name: &str,
) -> Option<&'a str> {
    agent
        .tokens
        .iter()
        .find(|(token, _)| token == name || token.strip_prefix('$') == Some(name))
        .map(|(_, value)| value.as_str())
}

/// Numbered tokens `<prefix>1..`, in order, from the agent-insights hook.
fn numbered_tokens(agent: &crate::protocol::ClientShellAgent, prefix: &str) -> Vec<String> {
    let mut items = agent
        .tokens
        .iter()
        .filter_map(|(name, value)| {
            let index = name
                .trim_start_matches('$')
                .strip_prefix(prefix)?
                .parse::<usize>()
                .ok()?;
            Some((index, value.clone()))
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|(index, _)| *index);
    items.into_iter().map(|(_, value)| value).collect()
}

/// To-do progress (done, total) and items, from the agent-insights hook.
pub(super) fn agent_todo(
    agent: &crate::protocol::ClientShellAgent,
) -> Option<((u32, u32), Vec<String>)> {
    let (done, total) = agent_token(agent, "sheprd_todo")?.split_once('/')?;
    let progress = (done.trim().parse().ok()?, total.trim().parse().ok()?);
    (progress.1 > 0).then(|| (progress, numbered_tokens(agent, "sheprd_todo_")))
}

/// Timeline lines ("09:12 fix the bar widget (+3 edits)"), oldest first.
pub(super) fn agent_timeline(agent: &crate::protocol::ClientShellAgent) -> Vec<String> {
    numbered_tokens(agent, "sheprd_tl_")
}

/// References (task ids, issues…) mentioned in the chat, newest first. `sheprd_tasks` is the
/// token's name before 0.9.3-13, still reported by older hooks.
pub(super) fn agent_tasks(agent: &crate::protocol::ClientShellAgent) -> Vec<String> {
    agent_token(agent, "sheprd_refs")
        .or_else(|| agent_token(agent, "sheprd_tasks"))
        .map(|tasks| {
            tasks
                .split(',')
                .map(str::trim)
                .filter(|task| !task.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Agents whose to-do list is expanded under their row (session only).
fn expanded_todos() -> &'static std::sync::Mutex<HashSet<String>> {
    static EXPANDED: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    EXPANDED.get_or_init(Default::default)
}

pub(super) fn todo_expanded(key: &str) -> bool {
    expanded_todos()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(key)
}

pub(super) fn toggle_todo(key: &str) {
    let mut expanded = expanded_todos().lock().unwrap_or_else(|e| e.into_inner());
    if !expanded.remove(key) {
        expanded.insert(key.to_owned());
    }
}

/// Runs the reference source's command for `id` off the UI thread; the body arrives via
/// `take_task_body` (shown as a peek).
pub(super) fn open_task(id: String) {
    match task_command(&id) {
        Some(command) => {
            std::thread::spawn(move || {
                let text = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
                push_task_body(id, text);
            });
        }
        None => match task_open(&id) {
            Some(command) => spawn_detached(&command),
            None => push_task_body(id, None),
        },
    }
}

/// A reference body (usually markdown with YAML front matter) prepared for the reader:
/// (title line, subtitle, body). Front matter becomes the title/subtitle; headings, bullets,
/// checkboxes and emphasis are mapped to what herdr's notes renderer draws.
pub(super) fn reader_document(id: &str, text: &str) -> (String, String, String) {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !key.starts_with(' ') && !value.is_empty() {
                        fields.push((key.trim().to_owned(), value.to_owned()));
                    }
                }
            }
            body = rest[end + 4..].trim_start_matches(|c| c == '-' || c == '\n');
        }
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let title = match field("title") {
        Some(title) if !title.is_empty() => format!("{id}  {title}"),
        _ => id.to_owned(),
    };
    let subtitle = ["status", "priority", "project", "assignee", "type"]
        .iter()
        .filter_map(|name| field(name).map(|value| format!("{name} {value}")))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut out = String::new();
    let mut in_code = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            out.push_str(trimmed);
        } else if in_code {
            out.push_str(line);
        } else if let Some(heading) = trimmed
            .strip_prefix("# ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("#### "))
        {
            out.push_str("### ");
            out.push_str(heading);
        } else {
            let indent = (line.len() - trimmed.len()) / 2;
            let bullet = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .or_else(|| trimmed.strip_prefix("+ "));
            let line = match bullet {
                Some(item) => {
                    let item = item
                        .strip_prefix("[ ] ")
                        .map(|rest| format!("☐ {rest}"))
                        .or_else(|| {
                            item.strip_prefix("[x] ")
                                .or_else(|| item.strip_prefix("[X] "))
                                .map(|rest| format!("☑ {rest}"))
                        })
                        .unwrap_or_else(|| item.to_owned());
                    format!("- {}{item}", "  ".repeat(indent))
                }
                None if trimmed.starts_with('>') => {
                    format!("│ {}", trimmed.trim_start_matches('>').trim_start())
                }
                None => line.to_owned(),
            };
            out.push_str(&line.replace("**", "").replace("__", ""));
        }
        out.push('\n');
    }
    (title, subtitle, out)
}

/// The task-id-shaped word at `col` in a row of cell symbols ("…see OP-018, then…" → "OP-018"),
/// trimmed of surrounding punctuation. Whether it is a task is up to `task_command`.
pub(super) fn word_at<'a>(
    symbols: impl IntoIterator<Item = &'a str>,
    col: usize,
) -> Option<String> {
    let chars: Vec<char> = symbols
        .into_iter()
        .map(|symbol| symbol.chars().next().unwrap_or(' '))
        .collect();
    let part = |c: char| c.is_ascii_alphanumeric() || "#_-./".contains(c);
    if !chars.get(col).copied().is_some_and(part) {
        return None;
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| part(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| part(chars[i])).last()? + 1;
    let word: String = chars[start..end].iter().collect();
    let word = word.trim_matches(|c: char| ".-/".contains(c));
    (!word.is_empty()).then(|| word.to_owned())
}

/// A Markdown file path under column `col` (`docs/plan.md`, `~/x.md`, `/abs/r.markdown`,
/// `file:///abs/x.md`), without surrounding quotes, brackets, punctuation, a `:line` suffix or a
/// `#anchor`.
pub(super) fn markdown_path_at<'a>(
    symbols: impl IntoIterator<Item = &'a str>,
    col: usize,
) -> Option<String> {
    let chars: Vec<char> = symbols
        .into_iter()
        .map(|symbol| symbol.chars().next().unwrap_or(' '))
        .collect();
    let part = |c: char| c.is_alphanumeric() || "_-./~:#@+%,()[]'\"`<>".contains(c);
    if !chars.get(col).copied().is_some_and(part) {
        return None;
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| part(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| part(chars[i])).last()? + 1;
    let token: String = chars[start..end].iter().collect();
    let trim = |c: char| "\"'`()[]<>,;:.".contains(c);
    let mut path = token.trim_matches(trim).to_owned();
    if let Some(rest) = path.strip_prefix("file://") {
        path = rest.to_owned();
    }
    if let Some((before, _)) = path.split_once('#') {
        path = before.to_owned();
    }
    // `docs/plan.md:12` or `docs/plan.md:12:4`
    while let Some((before, after)) = path.rsplit_once(':') {
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            path = before.to_owned();
        } else {
            break;
        }
    }
    let path = path.trim_matches(trim).to_owned();
    let lower = path.to_lowercase();
    ([".md", ".markdown", ".mdx"]
        .iter()
        .any(|ext| lower.ends_with(ext))
        && path.len() > 3)
        .then_some(path)
}

/// The command that shows a reference's body (a task, an issue…), from the user's reference
/// sources in `<config_dir>/sheprd-refs.toml`:
/// `[[refs]] name = "issues" pattern = "#\\d+" command = "gh issue view {id}"`.
/// The first source whose pattern matches the whole id wins.
pub(super) fn task_command(id: &str) -> Option<String> {
    ref_command(id, |source| source.command.as_deref())
}

/// The `open` command of the reference source matching `id` (opens the card in another app, e.g.
/// `xdg-open 'obsidian://…{id}'`), used when the source has no reader `command`.
pub(super) fn task_open(id: &str) -> Option<String> {
    ref_command(id, |source| source.open.as_deref())
}

#[derive(Deserialize)]
struct RefSource {
    pattern: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    open: Option<String>,
}

fn ref_command(id: &str, pick: impl Fn(&RefSource) -> Option<&str>) -> Option<String> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        refs: Vec<RefSource>,
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "#_-./".contains(c))
    {
        return None;
    }
    let path = crate::config::config_dir().join("sheprd-refs.toml");
    let config: Config = toml::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    config.refs.iter().find_map(|source| {
        let pattern = regex::Regex::new(&format!("^(?:{})$", source.pattern)).ok()?;
        if !pattern.is_match(id) {
            return None;
        }
        pick(source).map(|command| command.replace("{id}", id))
    })
}

/// Runs a command detached (opening something in another app); output is ignored.
pub(super) fn spawn_detached(command: &str) {
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Current context size of an agent, from the usage hook.
pub(super) fn agent_context_tokens(agent: &crate::protocol::ClientShellAgent) -> Option<u64> {
    agent_token(agent, "sheprd_ctx")?.parse().ok()
}

fn observe_usage(endpoint: &ClientShellEndpoint, snapshot: &crate::protocol::ClientShellSnapshot) {
    let mut store = usage_store().lock().unwrap_or_else(|e| e.into_inner());
    for agent in &snapshot.agents {
        let Some(session) = agent_token(agent, "sheprd_session") else {
            continue;
        };
        // One token per day: sheprd_u_YYYYMMDD = "in,out,cache_read,cache_write,minutes".
        let days = agent
            .tokens
            .iter()
            .filter_map(|(name, value)| {
                let date = name.trim_start_matches('$').strip_prefix("sheprd_u_")?;
                if date.len() != 8 {
                    return None;
                }
                let mut totals = [0u64; 5];
                let mut parts = value.split(',');
                for slot in &mut totals {
                    *slot = parts.next()?.trim().parse().ok()?;
                }
                Some((
                    format!("{}-{}-{}", &date[0..4], &date[4..6], &date[6..8]),
                    totals,
                ))
            })
            .collect::<Vec<_>>();
        if days.is_empty() {
            continue;
        }
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == agent.workspace_id)
        else {
            continue;
        };
        let record = store
            .sessions
            .entry(format!("{}/{session}", machine_key(endpoint)))
            .or_default();
        let key = workspace_key(endpoint, workspace);
        let mut changed = record.key != key;
        record.key = key;
        record.label = workspace.label.clone();
        record.paths = workspace_paths(snapshot, workspace);
        for (day, totals) in days {
            if record.days.get(&day) != Some(&totals) {
                record.days.insert(day, totals);
                changed = true;
            }
        }
        store.dirty |= changed;
    }
    let due = store
        .saved
        .is_none_or(|saved| saved.elapsed().as_secs() >= 30);
    if store.dirty && due && !cfg!(test) {
        if let Ok(content) = serde_json::to_vec(&*store) {
            let path = usage_path();
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, content).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                store.dirty = false;
                store.saved = Some(Instant::now());
            }
        }
    }
}

/// Usage across every project over the last `days` local days.
pub(super) fn total_usage(days: i64) -> [u64; 5] {
    let since = chrono_like_today_minus(days - 1).unwrap_or_default();
    let store = usage_store().lock().unwrap_or_else(|e| e.into_inner());
    let mut total = [0u64; 5];
    for record in store.sessions.values() {
        for totals in record.days.range(since.clone()..).map(|(_, totals)| totals) {
            for (sum, value) in total.iter_mut().zip(totals) {
                *sum += value;
            }
        }
    }
    total
}

/// True at most every 3s: gate for building the widget status at all.
pub(super) fn status_due() -> bool {
    static LAST: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|at| at.elapsed().as_secs() < 3) {
        return false;
    }
    *last = Some(Instant::now());
    true
}

/// Status for desktop widgets (the Omarchy bar plugin): written to
/// `<state_dir>/sheprd-status.json` only when it changes, at most every 3s.
pub(super) fn write_status(status: &serde_json::Value) {
    static LAST: OnceLock<std::sync::Mutex<(Option<Instant>, String)>> = OnceLock::new();
    if cfg!(test) {
        return;
    }
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let body = status.to_string();
    // Rewrite unchanged content every 30s so readers can tell sheprd is alive.
    let keepalive = last.0.is_none_or(|at| at.elapsed().as_secs() >= 30);
    if body == last.1 && !keepalive {
        return;
    }
    let mut stamped = status.clone();
    stamped["updated"] = serde_json::json!(unix_now());
    let path = crate::config::state_dir().join("sheprd-status.json");
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, stamped.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        *last = (Some(Instant::now()), body);
    }
}

/// Usage of one project (`None` = Other) over the last `days` local days:
/// [input, output, cache_read, cache_write, active_minutes].
pub(super) fn project_usage(layout: &ProjectLayout, group: Option<&str>, days: i64) -> [u64; 5] {
    let since = (chrono_like_today_minus(days - 1)).unwrap_or_default();
    let store = usage_store().lock().unwrap_or_else(|e| e.into_inner());
    let mut total = [0u64; 5];
    for record in store.sessions.values() {
        let owner = layout
            .group_of(&record.key, &record.label, &record.paths)
            .map(|index| layout.groups[index].name.as_str());
        if owner != group {
            continue;
        }
        for totals in record.days.range(since.clone()..).map(|(_, totals)| totals) {
            for (sum, value) in total.iter_mut().zip(totals) {
                *sum += value;
            }
        }
    }
    total
}

/// Local date `n` days ago as YYYY-MM-DD (no chrono dependency: ask `date`
/// once per call is too slow, so compute from the system clock + local offset).
fn chrono_like_today_minus(n: i64) -> Option<String> {
    let offset = local_utc_offset_secs();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64
        + offset;
    let days = now.div_euclid(86_400) - n;
    Some(civil_from_days(days))
}

fn local_utc_offset_secs() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|output| {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let sign = if text.starts_with('-') { -1 } else { 1 };
                let digits = text.trim_start_matches(['+', '-']);
                let hours: i64 = digits.get(0..2)?.parse().ok()?;
                let minutes: i64 = digits.get(2..4)?.parse().ok()?;
                Some(sign * (hours * 3600 + minutes * 60))
            })
            .unwrap_or(0)
    })
}

/// Days since 1970-01-01 -> "YYYY-MM-DD" (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// "1h05m" / "37m".
pub(super) fn format_minutes(minutes: u64) -> String {
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// "1.2M" / "340k" / "900".
pub(super) fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}k", tokens / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// Seconds since a workspace first appeared, if this client saw it appear.
pub(super) fn workspace_age_secs(
    endpoint: &ClientShellEndpoint,
    workspace_id: &str,
) -> Option<u64> {
    idle_secs(&format!("{}/ws:{}", machine_key(endpoint), workspace_id))
}

pub(super) fn format_age(secs: u64) -> String {
    match secs {
        0..=59 => "now".to_owned(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// A workspace sheprd just asked a machine to create, waiting to appear so the
/// agent command can be typed into it.
#[derive(Clone, Debug)]
pub(super) struct PendingLaunch {
    pub(super) endpoint_id: super::ClientEndpointId,
    pub(super) label: String,
    pub(super) known: HashSet<String>,
    pub(super) command: Option<String>,
    pub(super) since: Instant,
}

fn launch_store() -> &'static std::sync::Mutex<Option<PendingLaunch>> {
    static LAUNCH: OnceLock<std::sync::Mutex<Option<PendingLaunch>>> = OnceLock::new();
    LAUNCH.get_or_init(Default::default)
}

pub(super) fn set_launch(launch: Option<PendingLaunch>) {
    *launch_store().lock().unwrap_or_else(|e| e.into_inner()) = launch;
}

pub(super) fn launch() -> Option<PendingLaunch> {
    launch_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Peek (prefix+space): reveal idle age, context, numbers and latency for a
/// few seconds instead of showing them all the time.
const PEEK_SECS: u64 = 10;

fn peek_store() -> &'static std::sync::Mutex<Option<Instant>> {
    static PEEK: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
    PEEK.get_or_init(Default::default)
}

pub(super) fn toggle_peek() {
    let mut peek = peek_store().lock().unwrap_or_else(|e| e.into_inner());
    *peek = if peek.is_some() {
        None
    } else {
        Some(Instant::now())
    };
}

pub(super) fn peeking() -> bool {
    peek_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|since| since.elapsed().as_secs() < PEEK_SECS)
}

/// Ends an expired peek; true when the sidebar needs a repaint.
pub(super) fn expire_peek() -> bool {
    let mut peek = peek_store().lock().unwrap_or_else(|e| e.into_inner());
    if peek.is_some_and(|since| since.elapsed().as_secs() >= PEEK_SECS) {
        *peek = None;
        return true;
    }
    false
}

/// Agents whose desktop notification was clicked, waiting to be focused.
fn focus_queue() -> &'static std::sync::Mutex<Vec<(super::ClientEndpointId, String)>> {
    static QUEUE: OnceLock<std::sync::Mutex<Vec<(super::ClientEndpointId, String)>>> =
        OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

pub(crate) fn request_focus(endpoint_id: super::ClientEndpointId, pane_id: String) {
    focus_queue()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((endpoint_id, pane_id));
}

pub(super) fn take_focus_requests() -> Vec<(super::ClientEndpointId, String)> {
    std::mem::take(&mut *focus_queue().lock().unwrap_or_else(|e| e.into_inner()))
}

/// Sidebar filter prompt (prefix+/): the typed query and the highlighted row.
#[derive(Default)]
struct FilterState {
    query: Option<String>,
    selected: usize,
}

fn filter_store() -> &'static std::sync::Mutex<FilterState> {
    static FILTER: OnceLock<std::sync::Mutex<FilterState>> = OnceLock::new();
    FILTER.get_or_init(Default::default)
}

/// Called every frame with the prompt's text (None when the prompt is closed);
/// a changed query puts the highlight back on the first match.
pub(super) fn set_filter(query: Option<String>) {
    let mut state = filter_store().lock().unwrap_or_else(|e| e.into_inner());
    if state.query != query {
        state.selected = 0;
        state.query = query;
    }
}

pub(super) fn filter_query() -> Option<String> {
    filter_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .query
        .clone()
}

pub(super) fn filter_move(delta: isize) {
    let mut state = filter_store().lock().unwrap_or_else(|e| e.into_inner());
    state.selected = state.selected.saturating_add_signed(delta);
}

pub(super) fn filter_selected() -> usize {
    filter_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .selected
}

/// Finished peek reads, waiting for the tick to show them (None = failed).
fn peek_results() -> &'static std::sync::Mutex<Vec<Option<String>>> {
    static RESULTS: OnceLock<std::sync::Mutex<Vec<Option<String>>>> = OnceLock::new();
    RESULTS.get_or_init(Default::default)
}

/// Finished reference-body reads (sheprd-refs.toml sources), shown in the reader.
fn task_results() -> &'static std::sync::Mutex<Vec<(String, Option<String>)>> {
    static RESULTS: OnceLock<std::sync::Mutex<Vec<(String, Option<String>)>>> = OnceLock::new();
    RESULTS.get_or_init(Default::default)
}

pub(super) fn push_task_body(id: String, text: Option<String>) {
    task_results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, text));
}

pub(super) fn take_task_body() -> Option<(String, Option<String>)> {
    task_results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop()
}

pub(super) fn push_peek(text: Option<String>) {
    peek_results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(text);
}

pub(super) fn take_peek() -> Option<Option<String>> {
    peek_results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop()
}

fn peek_anchor_store() -> &'static std::sync::Mutex<(u16, u16)> {
    static ANCHOR: OnceLock<std::sync::Mutex<(u16, u16)>> = OnceLock::new();
    ANCHOR.get_or_init(|| std::sync::Mutex::new((2, 2)))
}

/// Where the last sheprd menu opened; the peek preview pops up there.
pub(super) fn set_peek_anchor(anchor: (u16, u16)) {
    *peek_anchor_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = anchor;
}

pub(super) fn peek_anchor() -> (u16, u16) {
    *peek_anchor_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

static HINTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn set_hinting(on: bool) {
    HINTING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn hinting() -> bool {
    HINTING.load(std::sync::atomic::Ordering::Relaxed)
}

/// A press on a sheprd sidebar row, kept until the button comes up so the
/// same gesture can be a click (focus) or a drag (move to a project).
#[derive(Clone, Debug)]
pub(super) struct RowPress {
    pub(super) endpoint_id: super::ClientEndpointId,
    pub(super) workspace_id: String,
    pub(super) pane_id: Option<String>,
    pub(super) start: (u16, u16),
    pub(super) dragging: Option<(u16, u16)>,
}

fn press_store() -> &'static std::sync::Mutex<Option<RowPress>> {
    static PRESS: OnceLock<std::sync::Mutex<Option<RowPress>>> = OnceLock::new();
    PRESS.get_or_init(Default::default)
}

pub(super) fn set_press(press: Option<RowPress>) {
    *press_store().lock().unwrap_or_else(|e| e.into_inner()) = press;
}

pub(super) fn press() -> Option<RowPress> {
    press_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub(super) fn drag_to(point: (u16, u16)) -> bool {
    let mut guard = press_store().lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        Some(press) if press.start != point || press.dragging.is_some() => {
            press.dragging = Some(point);
            true
        }
        _ => false,
    }
}

/// Header key used for the catch-all "Other" group.
pub(super) const OTHER: &str = "\u{0}other";

/// Clear the manual unread flag of an agent once it gains focus (on the focus
/// transition only, so marking the focused agent itself still sticks).
pub(super) fn note_focused_agent(key: Option<String>) {
    let store = store();
    let previous = {
        let mut guard = store.write().unwrap_or_else(|e| e.into_inner());
        if guard.last_focused_agent == key {
            return;
        }
        std::mem::replace(&mut guard.last_focused_agent, key.clone())
    };
    let _ = previous;
    if let Some(key) = key {
        if layout().is_unread(&key) {
            update(|layout| layout.unread.retain(|unread| unread != &key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, members: &[&str], rules: &[&str]) -> ProjectGroup {
        ProjectGroup {
            name: name.into(),
            members: members.iter().map(|m| m.to_string()).collect(),
            rules: rules.iter().map(|r| r.to_string()).collect(),
            ..ProjectGroup::default()
        }
    }

    #[test]
    fn explicit_membership_beats_rules() {
        let layout = ProjectLayout {
            groups: vec![
                group("A", &[], &["store"]),
                group("B", &["gpu-box/Storefront"], &[]),
            ],
            ..ProjectLayout::default()
        };
        assert_eq!(
            layout.group_of("gpu-box/Storefront", "Storefront", &[]),
            Some(1)
        );
        assert_eq!(
            layout.group_of("local/Storefront", "Storefront", &[]),
            Some(0)
        );
        assert_eq!(layout.group_of("local/Notes", "Notes", &[]), None);
        assert_eq!(
            layout.group_of("gpu-box/sf", "sf", &["/home/me/code/Storefront".into()]),
            Some(0)
        );
    }

    #[test]
    fn pinned_groups_display_first_and_stay_above() {
        let mut layout = ProjectLayout {
            groups: vec![group("A", &[], &[]), group("B", &[], &[])],
            ..ProjectLayout::default()
        };
        layout.groups[1].pinned = true;
        assert_eq!(layout.display_order(), vec![1, 0]);
        layout.move_group("A", -1);
        assert_eq!(layout.display_order(), vec![1, 0]);
    }

    #[test]
    fn assign_moves_between_groups_and_creates() {
        let mut layout = ProjectLayout {
            groups: vec![group("A", &["local/x"], &[])],
            ..ProjectLayout::default()
        };
        layout.assign("local/x", "b");
        assert!(layout.groups[0].members.is_empty());
        assert_eq!(layout.groups[1].name, "b");
        layout.assign("local/x", "A");
        assert_eq!(layout.groups[0].members, vec!["local/x".to_string()]);
        assert!(layout.groups[1].members.is_empty());
    }

    #[test]
    fn rail_tags() {
        assert_eq!(project_tag("StoreFront", None), "SF");
        assert_eq!(project_tag("Acme ops", None), "AO");
        assert_eq!(project_tag("Infrastructure", None), "In");
        assert_eq!(project_tag("LF", None), "LF");
        assert_eq!(project_tag("VTM", None), "VT");
        assert_eq!(project_tag("Anything", Some("op")), "op");
    }

    #[test]
    fn civil_dates_and_formatting() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(20_727), "2026-10-01");
        assert_eq!(format_minutes(65), "1h05m");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(340_000), "340k");
    }

    #[test]
    fn manual_marks_override_until_state_changes() {
        use crate::api::schema::AgentStatus;
        let mut layout = ProjectLayout::default();
        assert_eq!(
            layout.presence("dev/p1", 4, AgentStatus::Done),
            Presence::Done
        );
        layout.mark("dev/p1", 4, false);
        assert_eq!(
            layout.presence("dev/p1", 4, AgentStatus::Done),
            Presence::Idle
        );
        assert_eq!(
            layout.presence("dev/p1", 5, AgentStatus::Done),
            Presence::Done
        );
        layout.mark("dev/p1", 5, true);
        assert_eq!(
            layout.presence("dev/p1", 5, AgentStatus::Idle),
            Presence::Unread
        );
        assert!(layout.dismissed.is_empty());
    }

    #[test]
    fn dragging_to_other_beats_rules_and_back() {
        let mut layout = ProjectLayout {
            groups: vec![group("A", &[], &["store"])],
            ..ProjectLayout::default()
        };
        assert_eq!(
            layout.group_of("local/Storefront", "Storefront", &[]),
            Some(0)
        );
        layout.assign("local/Storefront", "");
        assert_eq!(layout.group_of("local/Storefront", "Storefront", &[]), None);
        layout.assign("local/Storefront", "A");
        assert!(layout.ungrouped.is_empty());
        assert_eq!(
            layout.group_of("local/Storefront", "Storefront", &[]),
            Some(0)
        );
    }

    #[test]
    fn move_member_materialises_order() {
        let mut layout = ProjectLayout {
            groups: vec![group("A", &["local/x"], &["y"])],
            ..ProjectLayout::default()
        };
        let view = vec!["local/x".to_string(), "dev/y".to_string()];
        layout.move_member(&view, "dev/y", -1);
        assert_eq!(
            layout.groups[0].members,
            vec!["dev/y".to_string(), "local/x".to_string()]
        );
    }

    #[test]
    fn layout_round_trips_toml() {
        let layout = ProjectLayout {
            hidden: vec!["dev/old".into()],
            groups: vec![group("A", &["local/x"], &["cal"])],
            ..ProjectLayout::default()
        };
        let text = toml::to_string_pretty(&layout).unwrap();
        assert!(text.contains("[[group]]"));
        assert_eq!(toml::from_str::<ProjectLayout>(&text).unwrap(), layout);
    }
}

#[cfg(test)]
mod word_at_tests {
    use super::{markdown_path_at, word_at};

    fn row(text: &str) -> Vec<String> {
        text.chars().map(String::from).collect()
    }

    #[test]
    fn markdown_paths_under_the_cursor() {
        let at =
            |text: &str, col: usize| markdown_path_at(row(text).iter().map(String::as_str), col);
        assert_eq!(
            at("see docs/plan.md for details", 6),
            Some("docs/plan.md".into())
        );
        assert_eq!(
            at("wrote `~/notes/report.md`.", 12),
            Some("~/notes/report.md".into())
        );
        assert_eq!(
            at("(/abs/x.markdown:12:4)", 5),
            Some("/abs/x.markdown".into())
        );
        assert_eq!(at("file:///tmp/a.md#intro", 9), Some("/tmp/a.md".into()));
        assert_eq!(at("open README.MD now", 7), Some("README.MD".into()));
        assert_eq!(at("see src/main.rs", 6), None);
        assert_eq!(at("BILL-12 is done", 2), None);
    }

    #[test]
    fn finds_the_task_id_under_the_column_without_punctuation() {
        let cells = row("see OP-018, then (SHP-1090).");
        let symbols = || cells.iter().map(String::as_str);
        assert_eq!(word_at(symbols(), 6).as_deref(), Some("OP-018"));
        assert_eq!(word_at(symbols(), 4).as_deref(), Some("OP-018"));
        assert_eq!(word_at(symbols(), 20).as_deref(), Some("SHP-1090"));
        assert_eq!(word_at(symbols(), 3), None);
    }
}

#[cfg(test)]
mod reader_tests {
    use super::reader_document;

    #[test]
    fn front_matter_becomes_title_and_subtitle_and_markdown_is_mapped() {
        let text = "---\nid: OP-018\ntitle: \"Rotate the token\"\nstatus: todo\nproject: ops\nbadges:\n  - x\n---\n\n## Description\n\n**Bold** text\n- [ ] open item\n  - nested\n> quoted\n";
        let (title, subtitle, body) = reader_document("OP-018", text);
        assert_eq!(title, "OP-018  Rotate the token");
        assert_eq!(subtitle, "status todo · project ops");
        assert!(body.contains("### Description"));
        assert!(body.contains("Bold text"));
        assert!(body.contains("- ☐ open item"));
        assert!(body.contains("-   nested"));
        assert!(body.contains("│ quoted"));
        assert!(!body.contains("badges"));
    }
}

#[cfg(test)]
mod active_membership_tests {
    use super::ProjectLayout;

    #[test]
    fn remove_from_active_lapses_at_the_next_state_change_and_keep_undoes_it() {
        let mut layout = ProjectLayout::default();
        layout.kept.push("dev/w1:p1".into());
        layout.remove_from_active("dev/w1:p1", 7);
        assert!(layout.is_idled("dev/w1:p1", 7));
        assert!(
            layout.is_dismissed("dev/w1:p1", 7),
            "removing also marks it read"
        );
        assert!(!layout.is_kept("dev/w1:p1"), "removing drops a keep");
        assert!(
            !layout.is_idled("dev/w1:p1", 8),
            "a new state brings it back"
        );
        layout.keep_active("dev/w1:p1");
        assert!(!layout.is_idled("dev/w1:p1", 7));
        assert!(layout.is_kept("dev/w1:p1"));
    }
}
