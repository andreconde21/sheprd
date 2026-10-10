//! andreconde fork (sheprd): move a Claude Code agent to another machine, keeping its
//! conversation (SHE-100001). The work is `scripts/sheprd-move` (bundled), run from here:
//! check first; with uncommitted changes ask "commit and push" or "carry as a patch"; then move
//! and focus the resumed agent on the target.

use std::sync::{Mutex, OnceLock};

use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MoveJob {
    /// Script machine names: a saved machine's label, or "local".
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) to_endpoint: ClientEndpointId,
    pub(crate) pane: String,
    pub(crate) session: String,
    pub(crate) cwd: String,
    pub(crate) label: String,
}

#[derive(Debug)]
enum MoveEvent {
    Checked {
        job: MoveJob,
        report: serde_json::Value,
    },
    Moved {
        job: MoveJob,
        result: serde_json::Value,
    },
    Failed(String),
}

fn events() -> &'static Mutex<Vec<MoveEvent>> {
    static EVENTS: OnceLock<Mutex<Vec<MoveEvent>>> = OnceLock::new();
    EVENTS.get_or_init(Default::default)
}

fn push(event: MoveEvent) {
    events()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(event);
}

fn script_machine(endpoints: &[ClientShellEndpoint], id: &ClientEndpointId) -> Option<String> {
    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == *id)?;
    Some(if id.is_local() {
        "local".to_owned()
    } else {
        endpoint.label.clone()
    })
}

/// Runs `sheprd-move <action>` for a job; the parsed JSON, or an error message.
fn run_script(
    action: &str,
    job: &MoveJob,
    dirty: Option<&str>,
) -> Result<serde_json::Value, String> {
    let script = crate::sheprd_msg::bundled_script("sheprd-move")
        .ok_or_else(|| "sheprd-move is missing next to sheprd".to_owned())?;
    let mut command = std::process::Command::new("python3");
    command.arg(script).arg(action).args([
        "--from",
        &job.from,
        "--to",
        &job.to,
        "--pane",
        &job.pane,
        "--cwd",
        &job.cwd,
        "--session",
        &job.session,
        "--label",
        &job.label,
    ]);
    if let Some(dirty) = dirty {
        command.args(["--dirty", dirty]);
    }
    if let Ok(exe) = std::env::current_exe() {
        command.env("HERDR_BIN_PATH", exe);
    }
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("could not run sheprd-move: {err}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|_| "sheprd-move printed no answer".to_owned())?;
    match value.get("error").and_then(|e| e.as_str()) {
        Some(error) => Err(error.to_owned()),
        None => Ok(value),
    }
}

fn start_check(job: MoveJob) {
    std::thread::spawn(move || match run_script("check", &job, None) {
        Ok(report) => push(MoveEvent::Checked { job, report }),
        Err(error) => push(MoveEvent::Failed(error)),
    });
}

pub(super) fn start_move(job: MoveJob, dirty: Option<&'static str>) {
    std::thread::spawn(move || match run_script("run", &job, dirty) {
        Ok(result) => push(MoveEvent::Moved { job, result }),
        Err(error) => push(MoveEvent::Failed(error)),
    });
}

impl ClientShellState {
    /// Machines a focused-on agent can move to: every other online machine, as (endpoint, name).
    pub(super) fn move_targets(&self, from: &ClientEndpointId) -> Vec<(ClientEndpointId, String)> {
        self.endpoints
            .iter()
            .filter(|endpoint| {
                endpoint.endpoint_id != *from && endpoint.status == ClientEndpointStatus::Online
            })
            .filter_map(|endpoint| {
                Some((
                    endpoint.endpoint_id.clone(),
                    script_machine(&self.endpoints, &endpoint.endpoint_id)?,
                ))
            })
            .collect()
    }

    /// The Claude Code session and folder of an agent, when it can be moved.
    pub(super) fn movable_agent(
        &self,
        endpoint_id: &ClientEndpointId,
        pane_id: &str,
    ) -> Option<(String, String, String)> {
        let snapshot = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == *endpoint_id)?
            .snapshot
            .as_deref()?;
        let agent = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)?;
        let session = projects::agent_token(agent, "sheprd_session")?.to_owned();
        let pane = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id)?;
        let cwd = pane.foreground_cwd.clone().or_else(|| pane.cwd.clone())?;
        let label = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == agent.workspace_id)
            .map(|workspace| workspace.label.clone())
            .unwrap_or_default();
        Some((session, cwd, label))
    }

    /// Picking a machine in "Move to…": check first, off the UI thread.
    pub(super) fn begin_move(
        &mut self,
        from: &ClientEndpointId,
        pane_id: &str,
        to: ClientEndpointId,
    ) {
        let (Some((session, cwd, label)), Some(from_name), Some(to_name)) = (
            self.movable_agent(from, pane_id),
            script_machine(&self.endpoints, from),
            script_machine(&self.endpoints, &to),
        ) else {
            return;
        };
        self.receive_endpoint_unavailable(format!("Checking {to_name} before moving…"));
        start_check(MoveJob {
            from: from_name,
            to: to_name,
            to_endpoint: to,
            pane: pane_id.to_owned(),
            session,
            cwd,
            label,
        });
    }

    /// Client tick: act on finished checks and moves.
    pub(super) fn tick_moves(&mut self, outcome: &mut ClientShellInput) {
        let finished = std::mem::take(&mut *events().lock().unwrap_or_else(|e| e.into_inner()));
        for event in finished {
            let (x, y) = projects::peek_anchor();
            match event {
                MoveEvent::Failed(error) => {
                    self.open_menu(
                        ClientContextMenuTarget::Info {
                            lines: vec!["Could not move the agent:".to_owned(), error],
                        },
                        x,
                        y,
                    );
                }
                MoveEvent::Checked { job, report } => {
                    let problems = report
                        .get("problems")
                        .and_then(|p| p.as_array())
                        .map(|p| {
                            p.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let dirty = report
                        .get("dirty")
                        .and_then(|d| d.as_array())
                        .map(|d| d.len())
                        .unwrap_or(0);
                    if !problems.is_empty() {
                        let mut lines = vec![format!("Can't move to {}:", job.to)];
                        lines.extend(problems);
                        self.open_menu(ClientContextMenuTarget::Info { lines }, x, y);
                    } else if dirty > 0 {
                        self.open_menu(ClientContextMenuTarget::MoveConfirm { job, dirty }, x, y);
                    } else {
                        self.receive_endpoint_unavailable(format!("Moving to {}…", job.to));
                        start_move(job, None);
                    }
                }
                MoveEvent::Moved { job, result } => {
                    let pane = result
                        .get("target_pane")
                        .and_then(|p| p.as_str())
                        .map(str::to_owned);
                    self.receive_endpoint_unavailable(format!(
                        "Moved to {}: the conversation continues there",
                        job.to
                    ));
                    if let Some(pane) = pane {
                        self.focus_or_activate(
                            job.to_endpoint.clone(),
                            ClientEndpointFocusTarget::Pane(pane),
                            outcome,
                        );
                    }
                }
            }
            outcome.repaint = true;
        }
    }
}
