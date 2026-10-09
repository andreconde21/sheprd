//! andreconde fork (sheprd): the shared sidebar view, for apps that mirror sheprd (contract v1,
//! `docs/sheprd-view-sync.md` in conductore-mobile).
//!
//! Opt-in with `share_view = true` in sidebar.toml. On this machine (the hub) sheprd then:
//! - writes `~/.local/state/sheprd/view.json` (layout, per-agent presence and marks, order,
//!   focus) when it changes and at least every 30 s; `sheprd-msg relay` copies it to the other
//!   machines with their own `self` key;
//! - drains `~/.local/state/sheprd/view-updates.jsonl` (marks written by such apps, collected
//!   here by the relay from the other machines) and applies them to sidebar.toml: v1 marks, and
//!   v2 layout edits (move, hide, project create/rename/pin/rules/tag/delete/order), refusing
//!   an edit that conflicts with a newer local change and listing it in view.json `rejected`.

use super::projects::{self, ProjectLayout};
use super::ClientEndpointStatus;
use super::{ClientEndpointId, ClientShellEndpoint};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

const MAX_AGENTS: usize = 2000;
/// Contract v2 raised the line cap for all lines (v1 writers keep theirs at 1024).
const MAX_LINE: usize = 4096;
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
    // /etc/hostname is Linux-only; `hostname` covers macOS.
    configured
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
                    // Extra v1 key (readers ignore unknown keys): taken out of the active view.
                    "removed": layout.is_idled(&key, seq),
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
        // Contract v2: this sheprd applies layout edits, and lists the ones it refused.
        "updates": 2,
        "rejected": rejected()
            .iter()
            .map(|(id, why)| serde_json::json!({ "id": id, "why": why }))
            .collect::<Vec<_>>(),
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

/// One layout edit (contract v2).
#[derive(Debug, PartialEq)]
pub(super) enum Edit {
    Assign {
        workspace: String,
        project: String,
    },
    Hide {
        workspace: String,
    },
    Show {
        workspace: String,
    },
    ProjectCreate {
        project: String,
        rules: Vec<String>,
    },
    ProjectRename {
        project: String,
        to: String,
    },
    ProjectPin {
        project: String,
        pinned: bool,
    },
    ProjectRules {
        project: String,
        rules: Vec<String>,
        was: Option<Vec<String>>,
    },
    ProjectShort {
        project: String,
        short: String,
    },
    ProjectDelete {
        project: String,
        members: Option<Vec<String>>,
    },
    ProjectMove {
        project: String,
        before: String,
    },
    MemberMove {
        workspace: String,
        before: String,
    },
    RemoveActive {
        agent: String,
        state_seq: u64,
    },
    KeepActive {
        agent: String,
    },
}

/// One line of view-updates.jsonl: a v1 mark or a v2 layout edit.
#[derive(Debug, PartialEq)]
pub(super) enum Line {
    Mark(Op),
    Edit { id: String, edit: Edit },
}

impl Line {
    fn id(&self) -> &str {
        match self {
            Line::Mark(op) => &op.id,
            Line::Edit { id, .. } => id,
        }
    }
}

fn no_controls(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

/// `machine/<id>:<label>` or `machine/<label>`, as in `layout`.
fn valid_workspace(key: &str) -> bool {
    let Some((machine, rest)) = key.split_once('/') else {
        return false;
    };
    (1..=64).contains(&machine.chars().count())
        && (1..=256).contains(&rest.chars().count())
        && no_controls(key)
}

fn valid_project(name: &str) -> bool {
    (1..=128).contains(&name.chars().count())
        && no_controls(name)
        && name.trim() == name
        && !name.is_empty()
}

fn string_field(value: &serde_json::Value, field: &str) -> Option<String> {
    value.get(field)?.as_str().map(str::to_owned)
}

fn rules_field(value: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    let rules = value
        .get(field)?
        .as_array()?
        .iter()
        .map(|rule| rule.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let valid = rules.len() <= 32
        && rules.iter().all(|rule| {
            (1..=128).contains(&rule.chars().count()) && no_controls(rule) && !rule.contains(',')
        });
    valid.then_some(rules)
}

fn workspaces_field(value: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    let keys = value
        .get(field)?
        .as_array()?
        .iter()
        .map(|key| {
            key.as_str()
                .filter(|key| valid_workspace(key))
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(keys)
}

fn parse_edit(value: &serde_json::Value, op: &str) -> Option<Edit> {
    let workspace = || string_field(value, "workspace").filter(|key| valid_workspace(key));
    let project = || string_field(value, "project").filter(|name| valid_project(name));
    let agent = || string_field(value, "agent").filter(|agent| valid_agent(agent));
    // An optional field that is present must be valid.
    let optional = |field: &str, parse: &dyn Fn() -> Option<Vec<String>>| match value.get(field) {
        None | Some(serde_json::Value::Null) => Some(None),
        Some(_) => parse().map(Some),
    };
    Some(match op {
        "assign" => Edit::Assign {
            workspace: workspace()?,
            project: string_field(value, "project")
                .filter(|name| name.is_empty() || valid_project(name))?,
        },
        "hide" => Edit::Hide {
            workspace: workspace()?,
        },
        "show" => Edit::Show {
            workspace: workspace()?,
        },
        "project-create" => Edit::ProjectCreate {
            project: project()?,
            rules: optional("match", &|| rules_field(value, "match"))?.unwrap_or_default(),
        },
        "project-rename" => Edit::ProjectRename {
            project: project()?,
            to: string_field(value, "to").filter(|name| valid_project(name))?,
        },
        "project-pin" => Edit::ProjectPin {
            project: project()?,
            pinned: value.get("pinned")?.as_bool()?,
        },
        "project-rules" => Edit::ProjectRules {
            project: project()?,
            rules: rules_field(value, "match")?,
            was: optional("was", &|| rules_field(value, "was"))?,
        },
        "project-short" => Edit::ProjectShort {
            project: project()?,
            short: string_field(value, "short")
                .filter(|short| short.chars().count() <= 8 && no_controls(short))?,
        },
        "project-delete" => Edit::ProjectDelete {
            project: project()?,
            members: optional("members", &|| workspaces_field(value, "members"))?,
        },
        "project-move" => Edit::ProjectMove {
            project: project()?,
            before: string_field(value, "before")
                .filter(|name| name.is_empty() || valid_project(name))?,
        },
        "member-move" => Edit::MemberMove {
            workspace: workspace()?,
            before: string_field(value, "before")
                .filter(|key| key.is_empty() || valid_workspace(key))?,
        },
        "remove-active" => Edit::RemoveActive {
            agent: agent()?,
            state_seq: value.get("state_seq")?.as_u64()?,
        },
        "keep-active" => Edit::KeepActive { agent: agent()? },
        _ => return None,
    })
}

/// Parses one line of view-updates.jsonl; None (and a log line) for anything invalid.
pub(super) fn parse_line(line: &str) -> Option<Line> {
    if line.len() > MAX_LINE {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("v")?.as_u64()? {
        1 => parse_op(&value).map(Line::Mark),
        2 => {
            let id = value.get("id")?.as_str()?;
            if !valid_id(id) {
                return None;
            }
            let edit = parse_edit(&value, value.get("op")?.as_str()?)?;
            Some(Line::Edit {
                id: id.to_owned(),
                edit,
            })
        }
        _ => None,
    }
}

/// A v1 mark line.
fn parse_op(value: &serde_json::Value) -> Option<Op> {
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

const GONE: &str = "project renamed or deleted meanwhile";

fn group_index(layout: &ProjectLayout, name: &str) -> Result<usize, &'static str> {
    layout
        .groups
        .iter()
        .position(|group| group.name == name)
        .ok_or(GONE)
}

fn same_rules(a: &[String], b: &[String]) -> bool {
    let set = |rules: &[String]| {
        rules
            .iter()
            .map(|rule| rule.to_lowercase())
            .collect::<std::collections::BTreeSet<_>>()
    };
    set(a) == set(b)
}

fn same_members(a: &[String], b: &[String]) -> bool {
    let covered = |from: &[String], to: &[String]| {
        from.iter().all(|key| {
            to.iter().any(|other| {
                projects::same_workspace(other, key) || projects::same_workspace(key, other)
            })
        })
    };
    covered(a, b) && covered(b, a)
}

/// The project a workspace shows in (group index) and that project's members in display order.
pub(super) type ProjectView<'a> = dyn Fn(&ProjectLayout, &str) -> Option<(usize, Vec<String>)> + 'a;

/// Applies one layout edit, or refuses it with the reason shown in the app.
pub(super) fn apply_edit(
    layout: &mut ProjectLayout,
    edit: &Edit,
    project_view: &ProjectView<'_>,
) -> Result<(), &'static str> {
    match edit {
        Edit::Assign { workspace, project } => layout.assign(workspace, project),
        Edit::Hide { workspace } => {
            if !layout.is_hidden(workspace) {
                layout.hidden.push(workspace.clone());
            }
        }
        Edit::Show { workspace } => layout
            .hidden
            .retain(|hidden| !projects::same_workspace(hidden, workspace)),
        Edit::ProjectCreate { project, rules } => {
            if layout
                .groups
                .iter()
                .any(|group| group.name.eq_ignore_ascii_case(project))
            {
                return Err("exists");
            }
            layout.groups.push(projects::ProjectGroup {
                name: project.clone(),
                rules: rules.clone(),
                ..Default::default()
            });
        }
        Edit::ProjectRename { project, to } => {
            let index = group_index(layout, project)?;
            if layout
                .groups
                .iter()
                .enumerate()
                .any(|(other, group)| other != index && group.name.eq_ignore_ascii_case(to))
            {
                return Err("name taken");
            }
            layout.groups[index].name = to.clone();
        }
        Edit::ProjectPin { project, pinned } => {
            let index = group_index(layout, project)?;
            layout.groups[index].pinned = *pinned;
        }
        Edit::ProjectRules {
            project,
            rules,
            was,
        } => {
            let index = group_index(layout, project)?;
            if was
                .as_ref()
                .is_some_and(|was| !same_rules(was, &layout.groups[index].rules))
            {
                return Err("rules changed meanwhile");
            }
            layout.groups[index].rules = rules.clone();
        }
        Edit::ProjectShort { project, short } => {
            let index = group_index(layout, project)?;
            layout.groups[index].short = (!short.is_empty()).then(|| short.clone());
        }
        Edit::ProjectDelete { project, members } => {
            let index = group_index(layout, project)?;
            if members
                .as_ref()
                .is_some_and(|members| !same_members(members, &layout.groups[index].members))
            {
                return Err("project changed meanwhile");
            }
            // As sheprd's own Delete: members fall back to a matching project or Other.
            layout.groups.remove(index);
        }
        Edit::ProjectMove { project, before } => {
            let index = group_index(layout, project)?;
            let pinned = layout.groups[index].pinned;
            if !before.is_empty() {
                let target = group_index(layout, before)?;
                if layout.groups[target].pinned != pinned {
                    return Err("pinned projects stay above");
                }
            }
            let group = layout.groups.remove(index);
            let at = if before.is_empty() {
                layout
                    .groups
                    .iter()
                    .rposition(|other| other.pinned == pinned)
                    .map_or(if pinned { 0 } else { layout.groups.len() }, |last| {
                        last + 1
                    })
            } else {
                group_index(layout, before)?
            };
            layout.groups.insert(at, group);
        }
        Edit::MemberMove { workspace, before } => {
            let Some((group, mut members)) = project_view(layout, workspace) else {
                return Err("not in a project");
            };
            let matches = |key: &String, target: &str| {
                projects::same_workspace(target, key) || projects::same_workspace(key, target)
            };
            let Some(from) = members.iter().position(|key| matches(key, workspace)) else {
                return Err("not in a project");
            };
            let moved = members.remove(from);
            let at = if before.is_empty() {
                members.len()
            } else {
                members
                    .iter()
                    .position(|key| matches(key, before))
                    .ok_or("moved to another project meanwhile")?
            };
            members.insert(at, moved);
            layout.groups[group].members = members;
        }
        Edit::RemoveActive { agent, state_seq } => layout.remove_from_active(agent, *state_seq),
        Edit::KeepActive { agent } => layout.keep_active(agent),
    }
    Ok(())
}

/// Refused v2 lines, newest last, reported in view.json (`rejected`) for the app.
fn rejected() -> std::sync::MutexGuard<'static, VecDeque<(String, String)>> {
    static REJECTED: OnceLock<std::sync::Mutex<VecDeque<(String, String)>>> = OnceLock::new();
    REJECTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn note_rejected(id: &str, why: &str) {
    let mut list = rejected();
    list.push_back((id.to_owned(), why.to_owned()));
    while list.len() > 50 {
        list.pop_front();
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

/// The project section a workspace shows in, with its members in display order.
fn project_view(
    layout: &ProjectLayout,
    endpoints: &[ClientShellEndpoint],
    workspace: &str,
) -> Option<(usize, Vec<String>)> {
    let (sections, _) = projects::sections(layout, endpoints);
    sections.into_iter().find_map(|section| {
        section
            .members
            .iter()
            .any(|member| {
                projects::same_workspace(workspace, &member.key)
                    || projects::same_workspace(&member.key, workspace)
            })
            .then(|| {
                let members = section
                    .members
                    .into_iter()
                    .map(|member| member.key)
                    .collect();
                (section.group, members)
            })
    })
}

/// Applies queued write-back lines in file order, skipping ids already applied (the last 500).
fn apply_updates(endpoints: &[ClientShellEndpoint]) {
    static SEEN: OnceLock<std::sync::Mutex<VecDeque<String>>> = OnceLock::new();
    let lines = drain_lines();
    if lines.is_empty() {
        return;
    }
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut parsed = Vec::new();
    for line in lines.iter().filter(|line| !line.trim().is_empty()) {
        match parse_line(line) {
            Some(line) if !seen.iter().any(|id| id == line.id()) => {
                seen.push_back(line.id().to_owned());
                if seen.len() > 500 {
                    seen.pop_front();
                }
                parsed.push(line);
            }
            Some(_) => {}
            None => tracing::warn!("sheprd view sync: skipped an invalid update line"),
        }
    }
    if parsed.is_empty() {
        return;
    }
    let mut refused = Vec::new();
    projects::update(|layout| {
        let view =
            |layout: &ProjectLayout, workspace: &str| project_view(layout, endpoints, workspace);
        for line in &parsed {
            match line {
                Line::Mark(op) => apply(layout, op),
                Line::Edit { id, edit } => {
                    if let Err(why) = apply_edit(layout, edit, &view) {
                        refused.push((id.clone(), why));
                    }
                }
            }
        }
    });
    for (id, why) in refused {
        tracing::info!(%id, why, "sheprd view sync: refused a layout edit");
        note_rejected(&id, why);
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
    apply_updates(endpoints);
    write_view(&view_json(endpoints, active_endpoint_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_and_rejects_bad_ones() {
        let ok = r#"{"v":1,"id":"c-1759612401456-08bd","at":1,"from":"conductore","op":"dismiss","agent":"dev/w2:p1","state_seq":41}"#;
        assert_eq!(
            parse_line(ok),
            Some(Line::Mark(Op {
                id: "c-1759612401456-08bd".into(),
                op: "dismiss".into(),
                agent: "dev/w2:p1".into(),
                state_seq: 41
            }))
        );
        let missing_seq =
            r#"{"v":1,"id":"c-1759612401456-08bd","op":"dismiss","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_line(missing_seq), None);
        let bad_agent = r#"{"v":1,"id":"c-1759612401456-08bd","op":"keep","agent":"dev/w2 p1"}"#;
        assert_eq!(parse_line(bad_agent), None);
        let bad_op = r#"{"v":1,"id":"c-1759612401456-08bd","op":"toggle","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_line(bad_op), None);
        let v1_op_as_v2 = r#"{"v":2,"id":"c-1759612401456-08bd","op":"keep","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_line(v1_op_as_v2), None);
        let v3 = r#"{"v":3,"id":"c-1759612401456-08bd","op":"keep","agent":"dev/w2:p1"}"#;
        assert_eq!(parse_line(v3), None);
    }

    #[test]
    fn parses_contract_v2_examples() {
        let assign = r#"{"v":2,"id":"c-1760040000123-3f9a","at":1760040000,"from":"conductore","op":"assign","workspace":"dev/w2:sf","project":"Storefront"}"#;
        assert_eq!(
            parse_line(assign),
            Some(Line::Edit {
                id: "c-1760040000123-3f9a".into(),
                edit: Edit::Assign {
                    workspace: "dev/w2:sf".into(),
                    project: "Storefront".into()
                }
            })
        );
        let rules = r#"{"v":2,"id":"c-1760040002789-77aa","at":1760040002,"from":"conductore","op":"project-rules","project":"Shop","match":["shop","storefront"],"was":["storefront"]}"#;
        assert!(matches!(
            parse_line(rules),
            Some(Line::Edit {
                edit: Edit::ProjectRules { was: Some(_), .. },
                ..
            })
        ));
        let remove = r#"{"v":2,"id":"c-1760040003012-0c1d","at":1760040003,"from":"conductore","op":"remove-active","agent":"dev/w2:p1","state_seq":41}"#;
        assert!(matches!(
            parse_line(remove),
            Some(Line::Edit {
                edit: Edit::RemoveActive { state_seq: 41, .. },
                ..
            })
        ));
        let to_other = r#"{"v":2,"id":"c-1760040000123-3f9b","op":"assign","workspace":"dev/w2:sf","project":""}"#;
        assert!(parse_line(to_other).is_some());
        // Field rules: padded names, commas in rules, long tags, control characters.
        for bad in [
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"project-create","project":" Shop"}"#,
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"project-rules","project":"Shop","match":["a,b"]}"#,
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"project-short","project":"Shop","short":"123456789"}"#,
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"hide","workspace":"dev"}"#,
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"project-pin","project":"Shop"}"#,
            r#"{"v":2,"id":"c-1760040000123-3f9a","op":"project-delete","project":"Shop","members":"dev/w1"}"#,
        ] {
            assert_eq!(parse_line(bad), None, "{bad}");
        }
        let long = format!(
            r#"{{"v":2,"id":"c-1760040000123-3f9a","op":"project-create","project":"Shop","match":[{}]}}"#,
            vec!["\"abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz\""; 30].join(",")
        );
        assert!(long.len() > 1024 && long.len() <= MAX_LINE);
        assert!(parse_line(&long).is_some());
    }

    fn group(name: &str) -> projects::ProjectGroup {
        projects::ProjectGroup {
            name: name.into(),
            ..Default::default()
        }
    }

    fn no_view(_: &ProjectLayout, _: &str) -> Option<(usize, Vec<String>)> {
        None
    }

    #[test]
    fn project_edits_apply_and_refuse_on_conflict() {
        let mut layout = ProjectLayout::default();
        let run = |layout: &mut ProjectLayout, edit: Edit| apply_edit(layout, &edit, &no_view);
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectCreate {
                    project: "Shop".into(),
                    rules: vec!["shop".into()]
                }
            ),
            Ok(())
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectCreate {
                    project: "shop".into(),
                    rules: vec![]
                }
            ),
            Err("exists")
        );
        layout.groups.push(group("Billing"));
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectRename {
                    project: "Shop".into(),
                    to: "billing".into()
                }
            ),
            Err("name taken")
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectRename {
                    project: "Shop".into(),
                    to: "SHOP".into()
                }
            ),
            Ok(())
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectPin {
                    project: "Shop".into(),
                    pinned: true
                }
            ),
            Err(GONE)
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectRules {
                    project: "SHOP".into(),
                    rules: vec!["store".into()],
                    was: Some(vec!["other".into()])
                }
            ),
            Err("rules changed meanwhile")
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectRules {
                    project: "SHOP".into(),
                    rules: vec!["store".into()],
                    was: Some(vec!["SHOP".into()])
                }
            ),
            Ok(())
        );
        assert_eq!(layout.groups[0].rules, vec!["store".to_owned()]);
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectShort {
                    project: "SHOP".into(),
                    short: "Sh".into()
                }
            ),
            Ok(())
        );
        assert_eq!(layout.groups[0].short.as_deref(), Some("Sh"));
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectShort {
                    project: "SHOP".into(),
                    short: String::new()
                }
            ),
            Ok(())
        );
        assert_eq!(layout.groups[0].short, None);
        layout.assign("dev/w1:api", "Billing");
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectDelete {
                    project: "Billing".into(),
                    members: Some(vec![])
                }
            ),
            Err("project changed meanwhile")
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::ProjectDelete {
                    project: "Billing".into(),
                    members: Some(vec!["dev/w1:api".into()])
                }
            ),
            Ok(())
        );
        assert_eq!(layout.groups.len(), 1);
    }

    #[test]
    fn project_move_keeps_pinned_projects_above() {
        let mut layout = ProjectLayout::default();
        for name in ["A", "B", "C"] {
            layout.groups.push(group(name));
        }
        layout.groups[0].pinned = true;
        let names = |layout: &ProjectLayout| {
            layout
                .groups
                .iter()
                .map(|g| g.name.clone())
                .collect::<Vec<_>>()
        };
        let mv = |layout: &mut ProjectLayout, project: &str, before: &str| {
            apply_edit(
                layout,
                &Edit::ProjectMove {
                    project: project.into(),
                    before: before.into(),
                },
                &no_view,
            )
        };
        assert_eq!(mv(&mut layout, "C", "B"), Ok(()));
        assert_eq!(names(&layout), ["A", "C", "B"]);
        assert_eq!(mv(&mut layout, "C", "A"), Err("pinned projects stay above"));
        assert_eq!(mv(&mut layout, "C", ""), Ok(()));
        assert_eq!(names(&layout), ["A", "B", "C"]);
        assert_eq!(mv(&mut layout, "A", ""), Ok(()));
        assert_eq!(names(&layout), ["A", "B", "C"]);
        assert_eq!(mv(&mut layout, "X", ""), Err(GONE));
    }

    #[test]
    fn member_move_materialises_the_order() {
        let mut layout = ProjectLayout::default();
        layout.groups.push(group("Shop"));
        let view = |_: &ProjectLayout, workspace: &str| {
            (workspace != "dev/w9:x").then(|| {
                (
                    0,
                    vec![
                        "dev/w1:a".to_owned(),
                        "dev/w2:b".to_owned(),
                        "dev/w3:c".to_owned(),
                    ],
                )
            })
        };
        let mv = |layout: &mut ProjectLayout, workspace: &str, before: &str| {
            apply_edit(
                layout,
                &Edit::MemberMove {
                    workspace: workspace.into(),
                    before: before.into(),
                },
                &view,
            )
        };
        assert_eq!(mv(&mut layout, "dev/w3:c", "dev/w1:a"), Ok(()));
        assert_eq!(
            layout.groups[0].members,
            ["dev/w3:c", "dev/w1:a", "dev/w2:b"]
        );
        assert_eq!(mv(&mut layout, "dev/w1:a", ""), Ok(()));
        assert_eq!(
            layout.groups[0].members,
            ["dev/w2:b", "dev/w3:c", "dev/w1:a"]
        );
        assert_eq!(
            mv(&mut layout, "dev/w1:a", "dev/w8:z"),
            Err("moved to another project meanwhile")
        );
        assert_eq!(mv(&mut layout, "dev/w9:x", ""), Err("not in a project"));
    }

    #[test]
    fn workspace_and_agent_edits() {
        let mut layout = ProjectLayout::default();
        let run = |layout: &mut ProjectLayout, edit: Edit| apply_edit(layout, &edit, &no_view);
        assert_eq!(
            run(
                &mut layout,
                Edit::Hide {
                    workspace: "dev/w1:a".into()
                }
            ),
            Ok(())
        );
        assert_eq!(
            run(
                &mut layout,
                Edit::Hide {
                    workspace: "dev/w1:a".into()
                }
            ),
            Ok(())
        );
        assert_eq!(layout.hidden.len(), 1);
        assert_eq!(
            run(
                &mut layout,
                Edit::Show {
                    workspace: "dev/w1:renamed".into()
                }
            ),
            Ok(())
        );
        assert!(layout.hidden.is_empty());
        assert_eq!(
            run(
                &mut layout,
                Edit::Assign {
                    workspace: "dev/w1:a".into(),
                    project: "New".into()
                }
            ),
            Ok(())
        );
        assert_eq!(layout.groups[0].members, ["dev/w1:a"]);
        assert_eq!(
            run(
                &mut layout,
                Edit::Assign {
                    workspace: "dev/w1:a".into(),
                    project: String::new()
                }
            ),
            Ok(())
        );
        assert_eq!(layout.ungrouped, ["dev/w1:a"]);
        assert_eq!(
            run(
                &mut layout,
                Edit::RemoveActive {
                    agent: "dev/w1:p1".into(),
                    state_seq: 7
                }
            ),
            Ok(())
        );
        assert!(layout.is_idled("dev/w1:p1", 7));
        assert_eq!(
            run(
                &mut layout,
                Edit::KeepActive {
                    agent: "dev/w1:p1".into()
                }
            ),
            Ok(())
        );
        assert!(!layout.is_idled("dev/w1:p1", 7));
        assert!(layout.is_kept("dev/w1:p1"));
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
