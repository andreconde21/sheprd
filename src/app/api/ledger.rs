//! andreconde fork (sheprd): `ledger.*` handlers, the card ledger of a pane's terminal
//! (SHE-100008). Writes bump `work_ledger_revision` (client fanout resends on it), mark the
//! session dirty (the ledger is persisted) and emit `ledger.updated`.
//! SHE-100009: the user's answers. `ledger.stage` keeps a choice on the server so every client
//! shows it, `ledger.send` types a pane's staged answers into it as one line, and the agent's
//! next explicit write, `ledger.answers` or `ledger.list` with `ack` clears what it was sent.
//! SHE-100010: `ledger.complete` starts the card's write-back (`[ledger.writeback]`) on a worker;
//! its outcome comes back as `AppEvent::LedgerWritebackFinished` and is kept on the card.

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, LedgerAnswersParams, LedgerAskParams,
    LedgerCompleteParams, LedgerDecideParams, LedgerDismissParams, LedgerItemTarget,
    LedgerListParams, LedgerPaneInfo, LedgerSendFailure, LedgerSendParams, LedgerSentPane,
    LedgerStageParams, LedgerUpdateParams, ResponseResult,
};
use crate::app::App;
use crate::layout::PaneId;
use crate::work_ledger::{LedgerError, LedgerQuestionInput, LedgerUpdate, LedgerWrite, WorkLedger};

use super::responses::{encode_error, encode_success};

/// Text then Enter, like `agent.prompt`: the Enter comes late enough to submit, not paste.
const LEDGER_SEND_SUBMIT_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// The pane a ledger request names, resolved to what a write needs.
struct LedgerTarget {
    ws_idx: usize,
    pane: PaneId,
    public_pane_id: String,
    terminal_id: crate::terminal::TerminalId,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn ledger_error(id: String, error: LedgerError) -> String {
    encode_error(id, error.code(), error.message())
}

impl App {
    fn ledger_target(&self, pane_id: &str) -> Option<LedgerTarget> {
        let (ws_idx, pane) = self.parse_pane_id(pane_id)?;
        let terminal_id = self
            .state
            .workspaces
            .get(ws_idx)?
            .pane_state(pane)?
            .attached_terminal_id
            .clone();
        if !self.state.terminals.contains_key(&terminal_id) {
            return None;
        }
        Some(LedgerTarget {
            ws_idx,
            pane,
            public_pane_id: self.public_pane_id(ws_idx, pane)?,
            terminal_id,
        })
    }

    /// Runs one write against the target's ledger, with the agent session the pane reported.
    fn ledger_write<T>(
        &mut self,
        target: &LedgerTarget,
        write: impl FnOnce(&mut WorkLedger, Option<&str>, u64) -> Result<T, LedgerError>,
    ) -> Option<Result<T, LedgerError>> {
        let terminal = self.state.terminals.get_mut(&target.terminal_id)?;
        let session = terminal
            .hook_authority
            .as_ref()
            .and_then(|authority| authority.session_ref.as_ref())
            .or_else(|| {
                terminal
                    .persisted_agent_session
                    .as_ref()
                    .map(|session| &session.session_ref)
            })
            .map(|session| session.value.clone());
        Some(write(
            &mut terminal.work_ledger,
            session.as_deref(),
            now_ms(),
        ))
    }

    fn ledger_changed(&mut self, target: &LedgerTarget, item_id: &str) {
        self.state.work_ledger_revision = self.state.work_ledger_revision.wrapping_add(1);
        self.state.mark_session_dirty();
        // Like token updates, ledger events bypass plugin hooks.
        self.event_hub.push(EventEnvelope {
            event: EventKind::LedgerUpdated,
            data: EventData::LedgerUpdated {
                pane_id: target.public_pane_id.clone(),
                workspace_id: self.public_workspace_id(target.ws_idx),
                item_id: item_id.to_owned(),
            },
        });
    }

    fn ledger_item_response(
        &mut self,
        id: String,
        target: &LedgerTarget,
        item_id: &str,
        result: Option<Result<LedgerWrite<bool>, LedgerError>>,
    ) -> String {
        let (changed, dropped) = match result {
            Some(Ok(write)) => (write.value, write.dropped),
            Some(Err(error)) => return ledger_error(id, error),
            None => return pane_not_found(id, &target.public_pane_id),
        };
        if changed {
            self.ledger_changed(target, item_id);
        }
        let item = self
            .state
            .terminals
            .get(&target.terminal_id)
            .and_then(|terminal| terminal.work_ledger.get(item_id))
            .cloned();
        match item {
            Some(item) => encode_success(
                id,
                ResponseResult::LedgerItem {
                    pane_id: target.public_pane_id.clone(),
                    item,
                    changed,
                    dropped,
                },
            ),
            None => encode_error(id, "ledger_item_not_found", "card is not in the ledger"),
        }
    }

    pub(super) fn handle_ledger_update(
        &mut self,
        id: String,
        params: LedgerUpdateParams,
    ) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let update = LedgerUpdate {
            title: params.title,
            status: params.status,
            summary: params.summary,
            link: params.link,
            inferred: params.inferred,
            inferred_questions: params.inferred_questions,
        };
        let item_id = params.item_id;
        let result = self.ledger_write(&target, |ledger, session, now| {
            ledger.update(&item_id, update, session, now)
        });
        self.ledger_item_response(id, &target, &item_id, result)
    }

    pub(super) fn handle_ledger_ask(&mut self, id: String, params: LedgerAskParams) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let input = LedgerQuestionInput {
            text: params.text,
            options: params.options,
            recommended: params.recommended,
        };
        let item_id = params.item_id;
        let result = self.ledger_write(&target, |ledger, session, now| {
            ledger.ask(&item_id, input, session, now)
        });
        match result {
            Some(Ok(LedgerWrite {
                value: (question_id, changed),
                dropped,
            })) => {
                if changed {
                    self.ledger_changed(&target, &item_id);
                }
                encode_success(
                    id,
                    ResponseResult::LedgerQuestionAsked {
                        pane_id: target.public_pane_id,
                        item_id,
                        question_id,
                        dropped,
                    },
                )
            }
            Some(Err(error)) => ledger_error(id, error),
            None => pane_not_found(id, &params.pane_id),
        }
    }

    pub(super) fn handle_ledger_decide(
        &mut self,
        id: String,
        params: LedgerDecideParams,
    ) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let (item_id, text) = (params.item_id, params.text);
        let result = self.ledger_write(&target, |ledger, session, now| {
            ledger.decide(&item_id, text, session, now)
        });
        self.ledger_item_response(id, &target, &item_id, result)
    }

    pub(super) fn handle_ledger_complete(
        &mut self,
        id: String,
        params: LedgerCompleteParams,
    ) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let (item_id, summary) = (params.item_id, params.summary);
        let result = self.ledger_write(&target, |ledger, session, now| {
            ledger.complete(&item_id, summary, session, now)
        });
        let succeeded = matches!(&result, Some(Ok(_)));
        let response = self.ledger_item_response(id, &target, &item_id, result);
        if succeeded {
            self.start_ledger_writeback(&target, &item_id);
        }
        response
    }

    /// Writes a done card back to the tracker, off the server loop. It is written again only
    /// when its last write-back failed or never ran, or the card changed since (a decision or a
    /// summary added after `done`), so `done` twice does not run the hook twice; nor does a
    /// `done` while the card's write-back is still running.
    fn start_ledger_writeback(&mut self, target: &LedgerTarget, item_id: &str) {
        if !crate::ledger_writeback::enabled(&self.ledger_writeback) {
            return;
        }
        let Some(item) = self
            .state
            .terminals
            .get(&target.terminal_id)
            .and_then(|terminal| terminal.work_ledger.get(item_id))
        else {
            return;
        };
        if !item.done {
            return;
        }
        let card = crate::ledger_writeback::done_card(
            &self.ledger_writeback,
            item,
            &target.public_pane_id,
        );
        let written = item.writeback.as_ref().is_some_and(|last| {
            last.ok && last.fingerprint == crate::ledger_writeback::fingerprint(&card)
        });
        let key = (target.terminal_id.clone(), item_id.to_owned());
        if written || self.ledger_writebacks_in_flight.contains(&key) {
            return;
        }
        let config = self.ledger_writeback.clone();
        let date = crate::platform::local_datetime()
            .map(|now| now.date().to_string())
            .unwrap_or_default();
        let (terminal_id, item_id) = (target.terminal_id.clone(), item_id.to_owned());
        let event_tx = self.event_tx.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(card = %item_id, "card write-back needs the server runtime; skipped");
            return;
        };
        self.ledger_writebacks_in_flight.insert(key);
        runtime.spawn(async move {
            let outcome = crate::ledger_writeback::run(config, card, date, now_ms()).await;
            let _ = event_tx
                .send(crate::events::AppEvent::LedgerWritebackFinished {
                    terminal_id,
                    item_id,
                    outcome,
                })
                .await;
        });
    }

    /// A write-back finished: its outcome goes on the card, if the card is still there and done.
    pub(crate) fn handle_ledger_writeback_finished(
        &mut self,
        terminal_id: &crate::terminal::TerminalId,
        item_id: &str,
        outcome: crate::work_ledger::LedgerWriteback,
    ) {
        self.ledger_writebacks_in_flight
            .remove(&(terminal_id.clone(), item_id.to_owned()));
        let changed = self
            .state
            .terminals
            .get_mut(terminal_id)
            .is_some_and(|terminal| terminal.work_ledger.set_writeback(item_id, outcome));
        if !changed {
            return;
        }
        let target = self
            .ledger_owners()
            .into_iter()
            .find(|(_, _, owner)| owner == terminal_id)
            .and_then(|(_, pane_id, _)| self.ledger_target(&pane_id));
        match target {
            Some(target) => self.ledger_changed(&target, item_id),
            None => {
                self.state.work_ledger_revision = self.state.work_ledger_revision.wrapping_add(1);
                self.state.mark_session_dirty();
            }
        }
    }

    pub(super) fn handle_ledger_remove(&mut self, id: String, params: LedgerItemTarget) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let item_id = params.item_id;
        let result = self.ledger_write(&target, |ledger, _, _| ledger.remove(&item_id));
        match result {
            Some(Ok(removed)) => {
                if removed {
                    self.ledger_changed(&target, &item_id);
                }
                encode_success(
                    id,
                    ResponseResult::LedgerItemRemoved {
                        pane_id: target.public_pane_id,
                        item_id,
                        removed,
                    },
                )
            }
            Some(Err(error)) => ledger_error(id, error),
            None => pane_not_found(id, &params.pane_id),
        }
    }

    pub(super) fn handle_ledger_list(&mut self, id: String, params: LedgerListParams) -> String {
        if params.ack && params.pane_id.is_none() {
            return ledger_error(
                id,
                LedgerError::Invalid("ack needs the agent's own pane_id".into()),
            );
        }
        let panes = match params.pane_id.as_deref() {
            Some(pane_id) => {
                let Some(target) = self.ledger_target(pane_id) else {
                    return pane_not_found(id, pane_id);
                };
                let panes = vec![self.ledger_pane_info(
                    target.ws_idx,
                    &target.public_pane_id,
                    &target.terminal_id,
                )];
                // Listed first, so the agent sees what it acknowledges.
                if params.ack {
                    self.ledger_acknowledge(&target);
                }
                panes
            }
            None => self
                .ledger_owners()
                .into_iter()
                .map(|(ws_idx, pane_id, terminal_id)| {
                    self.ledger_pane_info(ws_idx, &pane_id, &terminal_id)
                })
                .collect(),
        };
        encode_success(id, ResponseResult::LedgerList { panes })
    }

    /// The agent read its answers: what was sent to it, and what the user dismissed, goes.
    fn ledger_acknowledge(&mut self, target: &LedgerTarget) {
        let changed = self
            .ledger_write(target, |ledger, _, _| Ok(ledger.acknowledge()))
            .and_then(Result::ok)
            .unwrap_or_default();
        for item_id in changed {
            self.ledger_changed(target, &item_id);
        }
    }

    /// The panes a pane-or-all request covers: the named one, or every pane with a ledger.
    fn ledger_targets(&self, pane_id: Option<&str>) -> Result<Vec<LedgerTarget>, String> {
        match pane_id {
            Some(pane_id) => self
                .ledger_target(pane_id)
                .map(|target| vec![target])
                .ok_or_else(|| pane_id.to_owned()),
            None => Ok(self
                .ledger_owners()
                .into_iter()
                .filter_map(|(_, pane_id, _)| self.ledger_target(&pane_id))
                .collect()),
        }
    }

    pub(super) fn handle_ledger_stage(&mut self, id: String, params: LedgerStageParams) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let (item_id, question_id, answer) = (params.item_id, params.question_id, params.answer);
        let result = self
            .ledger_write(&target, |ledger, _, _| {
                ledger.stage(&item_id, question_id, answer)
            })
            .map(|result| {
                result.map(|value| LedgerWrite {
                    value,
                    dropped: Vec::new(),
                })
            });
        self.ledger_item_response(id, &target, &item_id, result)
    }

    pub(super) fn handle_ledger_dismiss(
        &mut self,
        id: String,
        params: LedgerDismissParams,
    ) -> String {
        if params.question_id.is_some() && params.item_id.is_none() {
            return ledger_error(id, LedgerError::Invalid("question_id needs item_id".into()));
        }
        if let Some(item_id) = params.item_id {
            let Some(pane_id) = params.pane_id else {
                return ledger_error(id, LedgerError::Invalid("item_id needs pane_id".into()));
            };
            let Some(target) = self.ledger_target(&pane_id) else {
                return pane_not_found(id, &pane_id);
            };
            let question_id = params.question_id;
            return match self.ledger_write(&target, |ledger, _, _| {
                ledger.dismiss(&item_id, question_id)
            }) {
                Some(Ok(changed)) => {
                    if changed {
                        self.ledger_changed(&target, &item_id);
                    }
                    encode_success(
                        id,
                        ResponseResult::LedgerDismissed {
                            dismissed: usize::from(changed),
                        },
                    )
                }
                Some(Err(error)) => ledger_error(id, error),
                None => pane_not_found(id, &pane_id),
            };
        }
        // Every done card of the pane, or of every pane.
        let targets = match self.ledger_targets(params.pane_id.as_deref()) {
            Ok(targets) => targets,
            Err(pane_id) => return pane_not_found(id, &pane_id),
        };
        let mut dismissed = 0;
        for target in targets {
            let removed = self
                .ledger_write(&target, |ledger, _, _| Ok(ledger.dismiss_done()))
                .and_then(Result::ok)
                .unwrap_or_default();
            dismissed += removed.len();
            for item_id in removed {
                self.ledger_changed(&target, &item_id);
            }
        }
        encode_success(id, ResponseResult::LedgerDismissed { dismissed })
    }

    pub(super) fn handle_ledger_send(&mut self, id: String, params: LedgerSendParams) -> String {
        let targets = match self.ledger_targets(params.pane_id.as_deref()) {
            Ok(targets) => targets,
            Err(pane_id) => return pane_not_found(id, &pane_id),
        };
        let mut sent = Vec::new();
        let mut failed = Vec::new();
        for target in targets {
            match self.ledger_send_to(&target) {
                Ok(Some(pane)) => sent.push(pane),
                Ok(None) => {}
                Err((code, message)) => failed.push(LedgerSendFailure {
                    pane_id: target.public_pane_id,
                    code: code.into(),
                    message,
                }),
            }
        }
        if params.pane_id.is_some() {
            if let Some(failure) = failed.pop() {
                return encode_error(id, &failure.code, failure.message);
            }
            if sent.is_empty() {
                return encode_error(
                    id,
                    "no_staged_answers",
                    "no answers are staged on this pane",
                );
            }
        }
        encode_success(id, ResponseResult::LedgerSent { sent, failed })
    }

    /// Types one pane's staged answers into it, then Enter: an agent that is working gets it as
    /// queued input. Refused when the pane does not run its agent in front (a shell would run
    /// the line) or the agent waits on a dialog (the line would answer that instead).
    fn ledger_send_to(
        &mut self,
        target: &LedgerTarget,
    ) -> Result<Option<LedgerSentPane>, (&'static str, String)> {
        let Some(terminal) = self.state.terminals.get(&target.terminal_id) else {
            return Ok(None);
        };
        let Some(send) = terminal.work_ledger.compose_send() else {
            return Ok(None);
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return Err((
                "agent_blocked",
                "the agent waits on a dialog; answer it first, then send".into(),
            ));
        }
        let agent_running = terminal.effective_known_agent().is_some_and(|agent| {
            self.lookup_runtime_sender(target.ws_idx, target.pane)
                .is_some_and(|runtime| super::super::agents::runtime_hosts_agent(runtime, agent))
        });
        if !agent_running {
            return Err((
                "agent_not_ready",
                "no agent runs in this pane right now".into(),
            ));
        }
        let Some(runtime) = self.lookup_runtime_sender(target.ws_idx, target.pane) else {
            return Err(("pane_not_found", "the pane has no terminal".into()));
        };
        let (text, enter) =
            crate::app::api_helpers::encode_api_submission_parts(runtime, &send.text);
        // Nothing waits for the Enter: the answers are in the pane's input queue already.
        if let Err(err) = runtime.queue_user_input_submission(
            bytes::Bytes::from(text),
            bytes::Bytes::from(enter),
            LEDGER_SEND_SUBMIT_DELAY,
            None,
        ) {
            return Err(("ledger_send_failed", err.to_string()));
        }
        let now = now_ms();
        if let Some(terminal) = self.state.terminals.get_mut(&target.terminal_id) {
            terminal.work_ledger.mark_sent(&send.questions, now);
        }
        let mut items = send
            .questions
            .iter()
            .map(|(item_id, _)| item_id.clone())
            .collect::<Vec<_>>();
        items.dedup();
        for item_id in items {
            self.ledger_changed(target, &item_id);
        }
        Ok(Some(LedgerSentPane {
            pane_id: target.public_pane_id.clone(),
            text: send.text,
            questions: send.questions.len(),
        }))
    }

    pub(super) fn handle_ledger_answers(
        &mut self,
        id: String,
        params: LedgerAnswersParams,
    ) -> String {
        let Some(target) = self.ledger_target(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let answers = self
            .state
            .terminals
            .get(&target.terminal_id)
            .map(|terminal| terminal.work_ledger.answers())
            .unwrap_or_default();
        if !params.peek {
            self.ledger_acknowledge(&target);
        }
        encode_success(
            id,
            ResponseResult::LedgerAnswers {
                pane_id: target.public_pane_id,
                answers,
            },
        )
    }

    fn ledger_pane_info(
        &self,
        ws_idx: usize,
        pane_id: &str,
        terminal_id: &crate::terminal::TerminalId,
    ) -> LedgerPaneInfo {
        LedgerPaneInfo {
            pane_id: pane_id.to_owned(),
            workspace_id: self.public_workspace_id(ws_idx),
            items: self
                .state
                .terminals
                .get(terminal_id)
                .map(|terminal| {
                    terminal
                        .work_ledger
                        .sorted_items()
                        .into_iter()
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Every pane whose terminal has a non-empty ledger, in workspace and pane order.
    pub(crate) fn ledger_owners(&self) -> Vec<(usize, String, crate::terminal::TerminalId)> {
        let mut owners = Vec::new();
        for (ws_idx, workspace) in self.state.workspaces.iter().enumerate() {
            let mut panes = workspace
                .tabs
                .iter()
                .flat_map(|tab| tab.panes.iter())
                .filter(|(_, pane)| {
                    self.state
                        .terminals
                        .get(&pane.attached_terminal_id)
                        .is_some_and(|terminal| !terminal.work_ledger.is_empty())
                })
                .filter_map(|(pane_id, pane): (&PaneId, _)| {
                    Some((
                        workspace.public_pane_number(*pane_id)?,
                        pane.attached_terminal_id.clone(),
                    ))
                })
                .collect::<Vec<_>>();
            panes.sort_by_key(|(number, _)| *number);
            owners.extend(panes.into_iter().map(|(number, terminal_id)| {
                (
                    ws_idx,
                    crate::workspace::public_pane_id_for_number(&workspace.id, number),
                    terminal_id,
                )
            }));
        }
        owners
    }
}

fn pane_not_found(id: String, pane_id: &str) -> String {
    encode_error(id, "pane_not_found", format!("pane {pane_id} not found"))
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{
        EventData, LedgerAnswer, LedgerAnswersParams, LedgerAskParams, LedgerCompleteParams,
        LedgerDecideParams, LedgerDismissParams, LedgerItemTarget, LedgerListParams,
        LedgerSendParams, LedgerStageParams, LedgerUpdateParams, Method, Request, ResponseResult,
        SuccessResponse,
    };
    use crate::app::App;
    use crate::config::Config;
    use crate::workspace::Workspace;

    fn app_with_test_workspace() -> (App, String) {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("ledger")];
        app.state.ensure_test_terminals();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        (app, public_pane_id)
    }

    fn request(app: &mut App, method: Method) -> serde_json::Value {
        serde_json::from_str(&app.handle_api_request(Request {
            id: "req".into(),
            method,
        }))
        .unwrap()
    }

    fn update(pane_id: &str, item_id: &str) -> LedgerUpdateParams {
        LedgerUpdateParams {
            pane_id: pane_id.into(),
            item_id: item_id.into(),
            title: Some("Stripe checkout".into()),
            status: Some("dev".into()),
            summary: None,
            link: None,
            inferred: false,
            inferred_questions: None,
        }
    }

    #[test]
    fn writes_bump_revision_mark_dirty_and_emit_events() {
        let (mut app, pane_id) = app_with_test_workspace();
        app.state.session_dirty = false;
        let start = app.event_hub.current_sequence();
        let response = request(&mut app, Method::LedgerUpdate(update(&pane_id, "SHE-12")));
        assert_eq!(response["result"]["type"], "ledger_item");
        assert_eq!(response["result"]["changed"], true);
        assert_eq!(response["result"]["item"]["title"], "Stripe checkout");
        assert_eq!(app.state.work_ledger_revision, 1);
        assert!(app.state.session_dirty);
        let events = app.event_hub.events_after(start);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].1.data,
            EventData::LedgerUpdated {
                pane_id: pane_id.clone(),
                workspace_id: app.public_workspace_id(0),
                item_id: "SHE-12".into(),
            }
        );

        // The same write again changes nothing: no revision, no event.
        app.state.session_dirty = false;
        let response = request(&mut app, Method::LedgerUpdate(update(&pane_id, "SHE-12")));
        assert_eq!(response["result"]["changed"], false);
        assert_eq!(app.state.work_ledger_revision, 1);
        assert!(!app.state.session_dirty);
        assert_eq!(app.event_hub.events_after(start).len(), 1);
    }

    #[test]
    fn ask_decide_complete_list_and_remove() {
        let (mut app, pane_id) = app_with_test_workspace();
        let response = request(
            &mut app,
            Method::LedgerAsk(LedgerAskParams {
                pane_id: pane_id.clone(),
                item_id: "SHE-12".into(),
                text: "Monthly or yearly?".into(),
                options: vec!["monthly".into(), "yearly".into()],
                recommended: Some(1),
            }),
        );
        assert_eq!(response["result"]["type"], "ledger_question_asked");
        assert_eq!(response["result"]["question_id"], 1);
        request(
            &mut app,
            Method::LedgerDecide(LedgerDecideParams {
                pane_id: pane_id.clone(),
                item_id: "SHE-12".into(),
                text: "Use Checkout, not Elements".into(),
            }),
        );
        request(
            &mut app,
            Method::LedgerComplete(LedgerCompleteParams {
                pane_id: pane_id.clone(),
                item_id: "SHE-7".into(),
                summary: Some("merged".into()),
            }),
        );

        let response = request(&mut app, Method::LedgerList(LedgerListParams::default()));
        let success: SuccessResponse = serde_json::from_value(response).unwrap();
        let ResponseResult::LedgerList { panes } = success.result else {
            panic!("expected ledger list");
        };
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, pane_id);
        let ids = panes[0]
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["SHE-12", "SHE-7"]);
        assert_eq!(panes[0].items[0].decisions.len(), 1);
        assert!(panes[0].items[1].done);

        let response = request(
            &mut app,
            Method::LedgerRemove(LedgerItemTarget {
                pane_id: pane_id.clone(),
                item_id: "SHE-7".into(),
            }),
        );
        assert_eq!(response["result"]["removed"], true);
        assert_eq!(app.state.work_ledger_revision, 4);
    }

    #[test]
    fn bad_pane_bad_id_and_limits_are_errors() {
        let (mut app, pane_id) = app_with_test_workspace();
        let response = request(&mut app, Method::LedgerUpdate(update("w9:p9", "SHE-1")));
        assert_eq!(response["error"]["code"], "pane_not_found");
        let response = request(&mut app, Method::LedgerUpdate(update(&pane_id, "bad id")));
        assert_eq!(response["error"]["code"], "invalid_ledger_item_id");
        let mut long = update(&pane_id, "SHE-1");
        long.summary = Some("x".repeat(crate::work_ledger::MAX_SUMMARY_CHARS + 1));
        let response = request(&mut app, Method::LedgerUpdate(long));
        assert_eq!(response["error"]["code"], "invalid_ledger_request");
        let response = request(
            &mut app,
            Method::LedgerList(LedgerListParams {
                pane_id: Some("w9:p9".into()),
                ack: false,
            }),
        );
        assert_eq!(response["error"]["code"], "pane_not_found");
        assert_eq!(app.state.work_ledger_revision, 0);
    }

    #[test]
    fn inferred_write_to_explicit_item_is_an_ok_no_op() {
        let (mut app, pane_id) = app_with_test_workspace();
        request(&mut app, Method::LedgerUpdate(update(&pane_id, "SHE-1")));
        let mut inferred = update(&pane_id, "SHE-1");
        inferred.title = Some("hook guess".into());
        inferred.inferred = true;
        let response = request(&mut app, Method::LedgerUpdate(inferred));
        assert_eq!(response["result"]["changed"], false);
        assert_eq!(response["result"]["item"]["title"], "Stripe checkout");
        assert_eq!(app.state.work_ledger_revision, 1);
    }

    #[test]
    fn writes_record_the_reported_agent_session() {
        let (mut app, pane_id) = app_with_test_workspace();
        let (_, pane) = app.parse_pane_id(&pane_id).unwrap();
        let terminal_id = app.state.workspaces[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
                source: "claude".into(),
                agent: "claude".into(),
                session_ref: crate::agent_resume::AgentSessionRef::id("abc-123").unwrap(),
            });
        let response = request(&mut app, Method::LedgerUpdate(update(&pane_id, "SHE-1")));
        assert_eq!(response["result"]["item"]["agent_session"], "abc-123");
    }

    #[test]
    fn capability_defaults_to_off_for_older_servers() {
        let older: crate::api::schema::ServerCapabilities =
            serde_json::from_value(serde_json::json!({"live_handoff": true})).unwrap();
        assert!(!older.work_ledger);
        assert!(!older.work_ledger_answers);
    }

    fn ask_two(app: &mut App, pane_id: &str) {
        for (text, options) in [
            (
                "Provider?",
                vec!["Stripe".to_owned(), "Multibanco".to_owned()],
            ),
            ("Webhook path?", Vec::new()),
        ] {
            request(
                app,
                Method::LedgerAsk(LedgerAskParams {
                    pane_id: pane_id.into(),
                    item_id: "SHE-12".into(),
                    text: text.into(),
                    options,
                    recommended: None,
                }),
            );
        }
    }

    fn stage(pane_id: &str, question_id: u32, answer: Option<LedgerAnswer>) -> Method {
        Method::LedgerStage(LedgerStageParams {
            pane_id: pane_id.into(),
            item_id: "SHE-12".into(),
            question_id,
            answer,
        })
    }

    fn option(index: usize) -> Option<LedgerAnswer> {
        Some(LedgerAnswer {
            option: Some(index),
            text: None,
        })
    }

    #[test]
    fn stage_dismiss_and_answers_round_trip() {
        let (mut app, pane_id) = app_with_test_workspace();
        ask_two(&mut app, &pane_id);
        let revision = app.state.work_ledger_revision;
        let response = request(&mut app, stage(&pane_id, 1, option(0)));
        assert_eq!(response["result"]["changed"], true);
        assert_eq!(
            response["result"]["item"]["questions"][0]["answer"]["option"],
            0
        );
        assert_eq!(app.state.work_ledger_revision, revision + 1);
        let response = request(&mut app, stage(&pane_id, 1, option(0)));
        assert_eq!(response["result"]["changed"], false);
        assert_eq!(app.state.work_ledger_revision, revision + 1);
        let response = request(&mut app, stage(&pane_id, 1, option(5)));
        assert_eq!(response["error"]["code"], "invalid_ledger_request");
        let response = request(&mut app, stage(&pane_id, 7, option(0)));
        assert_eq!(response["error"]["code"], "ledger_item_not_found");
        let response = request(&mut app, stage("w9:p9", 1, option(0)));
        assert_eq!(response["error"]["code"], "pane_not_found");

        let response = request(
            &mut app,
            Method::LedgerDismiss(LedgerDismissParams {
                pane_id: Some(pane_id.clone()),
                item_id: Some("SHE-12".into()),
                question_id: Some(2),
            }),
        );
        assert_eq!(response["result"]["dismissed"], 1);
        let response = request(
            &mut app,
            Method::LedgerAnswers(LedgerAnswersParams {
                pane_id: pane_id.clone(),
                peek: true,
            }),
        );
        assert_eq!(response["result"]["answers"][0]["dismissed"], true);
        let response = request(
            &mut app,
            Method::LedgerAnswers(LedgerAnswersParams {
                pane_id: pane_id.clone(),
                peek: false,
            }),
        );
        assert_eq!(response["result"]["answers"].as_array().unwrap().len(), 1);
        let response = request(
            &mut app,
            Method::LedgerAnswers(LedgerAnswersParams {
                pane_id: pane_id.clone(),
                peek: false,
            }),
        );
        assert!(response["result"]["answers"].as_array().unwrap().is_empty());

        // Bad shapes are refused.
        for params in [
            LedgerDismissParams {
                pane_id: Some(pane_id.clone()),
                item_id: None,
                question_id: Some(1),
            },
            LedgerDismissParams {
                pane_id: None,
                item_id: Some("SHE-12".into()),
                question_id: None,
            },
        ] {
            let response = request(&mut app, Method::LedgerDismiss(params));
            assert_eq!(response["error"]["code"], "invalid_ledger_request");
        }
        let response = request(
            &mut app,
            Method::LedgerList(LedgerListParams {
                pane_id: None,
                ack: true,
            }),
        );
        assert_eq!(response["error"]["code"], "invalid_ledger_request");
    }

    #[test]
    fn dismiss_all_done_covers_every_pane() {
        let (mut app, pane_id) = app_with_test_workspace();
        for item_id in ["SHE-1", "SHE-2"] {
            request(
                &mut app,
                Method::LedgerComplete(LedgerCompleteParams {
                    pane_id: pane_id.clone(),
                    item_id: item_id.into(),
                    summary: None,
                }),
            );
        }
        request(&mut app, Method::LedgerUpdate(update(&pane_id, "SHE-3")));
        let start = app.event_hub.current_sequence();
        let response = request(
            &mut app,
            Method::LedgerDismiss(LedgerDismissParams::default()),
        );
        assert_eq!(response["result"]["dismissed"], 2);
        assert_eq!(app.event_hub.events_after(start).len(), 2);
        let response = request(&mut app, Method::LedgerList(LedgerListParams::default()));
        assert_eq!(response["result"]["panes"][0]["items"][0]["id"], "SHE-3");
    }

    #[test]
    fn send_refuses_a_pane_without_its_agent_and_keeps_answers_staged() {
        let (mut app, pane_id) = app_with_test_workspace();
        ask_two(&mut app, &pane_id);
        let response = request(
            &mut app,
            Method::LedgerSend(LedgerSendParams {
                pane_id: Some(pane_id.clone()),
            }),
        );
        assert_eq!(response["error"]["code"], "no_staged_answers");
        request(&mut app, stage(&pane_id, 1, option(1)));
        // No agent runs in the test pane: typing would reach a shell.
        let response = request(
            &mut app,
            Method::LedgerSend(LedgerSendParams {
                pane_id: Some(pane_id.clone()),
            }),
        );
        assert_eq!(response["error"]["code"], "agent_not_ready");
        let response = request(&mut app, Method::LedgerSend(LedgerSendParams::default()));
        assert_eq!(response["result"]["failed"][0]["code"], "agent_not_ready");
        assert!(response["result"]["sent"].as_array().unwrap().is_empty());
        let response = request(&mut app, Method::LedgerList(LedgerListParams::default()));
        let question = &response["result"]["panes"][0]["items"][0]["questions"][0];
        assert_eq!(question["answer"]["option"], 1);
        assert!(question["sent_at_ms"].is_null());
    }

    #[test]
    fn only_acknowledging_reads_count_as_ui_changes() {
        let changes = |method| {
            crate::api::request_changes_ui(&crate::api::schema::Request {
                id: "t".into(),
                method,
            })
        };
        assert!(!changes(Method::LedgerList(LedgerListParams::default())));
        assert!(changes(Method::LedgerList(LedgerListParams {
            pane_id: None,
            ack: true,
        })));
        let answers = |peek| {
            Method::LedgerAnswers(LedgerAnswersParams {
                pane_id: "w1:p1".into(),
                peek,
            })
        };
        assert!(!changes(answers(true)));
        assert!(changes(answers(false)));
        assert!(changes(Method::LedgerSend(LedgerSendParams::default())));
    }

    /// A test pane with a runtime whose input the test reads, and the agent state it detects.
    fn app_with_pane_input(
        detected: Option<(crate::detect::Agent, crate::detect::AgentState)>,
    ) -> (App, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
        let (mut app, pane_id) = app_with_test_workspace();
        let (_, pane) = app.parse_pane_id(&pane_id).unwrap();
        let terminal_id = app.state.workspaces[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        if let Some((agent, state)) = detected {
            app.state
                .terminals
                .get_mut(&terminal_id)
                .unwrap()
                .set_detected_state(Some(agent), state);
        }
        let (runtime, rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane, runtime);
        (app, pane_id, rx)
    }

    fn assert_send_refused(
        app: &mut App,
        pane_id: &str,
        rx: &mut tokio::sync::mpsc::Receiver<bytes::Bytes>,
        code: &str,
    ) {
        ask_two(app, pane_id);
        request(app, stage(pane_id, 1, option(0)));
        let response = request(
            app,
            Method::LedgerSend(LedgerSendParams {
                pane_id: Some(pane_id.to_owned()),
            }),
        );
        assert_eq!(response["error"]["code"], code, "{response}");
        let response = request(app, Method::LedgerSend(LedgerSendParams::default()));
        assert_eq!(response["result"]["failed"][0]["code"], code, "{response}");
        assert!(rx.try_recv().is_err(), "nothing was typed into the pane");
        let response = request(app, Method::LedgerList(LedgerListParams::default()));
        let question = &response["result"]["panes"][0]["items"][0]["questions"][0];
        assert_eq!(question["answer"]["option"], 0, "the answer stays staged");
        assert!(question["sent_at_ms"].is_null());
    }

    #[tokio::test]
    async fn send_refuses_an_agent_waiting_on_a_dialog() {
        let (mut app, pane_id, mut rx) = app_with_pane_input(Some((
            crate::detect::Agent::OpenCode,
            crate::detect::AgentState::Blocked,
        )));
        assert_send_refused(&mut app, &pane_id, &mut rx, "agent_blocked");
    }

    #[tokio::test]
    async fn send_refuses_a_live_pane_that_runs_no_agent() {
        // The pane has a terminal that would take the line: only the agent gate stops it.
        let (mut app, pane_id, mut rx) = app_with_pane_input(None);
        assert_send_refused(&mut app, &pane_id, &mut rx, "agent_not_ready");
    }

    #[tokio::test]
    async fn send_types_one_line_into_a_working_agent_and_list_acknowledges() {
        let (mut app, pane_id) = app_with_test_workspace();
        let (_, pane) = app.parse_pane_id(&pane_id).unwrap();
        let terminal_id = app.state.workspaces[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(
                Some(crate::detect::Agent::OpenCode),
                crate::detect::AgentState::Working,
            );
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane, runtime);
        ask_two(&mut app, &pane_id);
        request(&mut app, stage(&pane_id, 1, option(0)));
        request(
            &mut app,
            stage(
                &pane_id,
                2,
                Some(LedgerAnswer {
                    option: None,
                    text: Some("/hooks/stripe".into()),
                }),
            ),
        );
        let response = request(&mut app, Method::LedgerSend(LedgerSendParams::default()));
        let line = "SHE-12 Q1 (Provider?) → Stripe · Q2 (Webhook path?) → \"/hooks/stripe\"";
        assert_eq!(response["result"]["sent"][0]["text"], line);
        assert_eq!(response["result"]["sent"][0]["questions"], 2);
        let typed = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("typed line")
            .expect("pane input channel");
        assert_eq!(typed, bytes::Bytes::from(line.as_bytes().to_vec()));
        let enter = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("enter")
            .expect("pane input channel");
        assert_eq!(enter, bytes::Bytes::from_static(b"\r"));
        // Sent answers are not sent twice.
        let response = request(&mut app, Method::LedgerSend(LedgerSendParams::default()));
        assert!(response["result"]["sent"].as_array().unwrap().is_empty());

        // The agent lists its own cards: it sees the answers, then they are gone.
        let response = request(
            &mut app,
            Method::LedgerList(LedgerListParams {
                pane_id: Some(pane_id.clone()),
                ack: true,
            }),
        );
        let questions = &response["result"]["panes"][0]["items"][0]["questions"];
        assert_eq!(questions.as_array().unwrap().len(), 2);
        assert!(!questions[0]["sent_at_ms"].is_null());
        let response = request(&mut app, Method::LedgerList(LedgerListParams::default()));
        assert!(response["result"]["panes"][0]["items"][0]["questions"].is_null());
    }

    #[tokio::test]
    async fn done_writes_the_card_back_and_keeps_the_outcome() {
        let (mut app, pane_id) = app_with_test_workspace();
        let dir = std::env::temp_dir().join(format!(
            "sheprd-ledger-writeback-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SHE-1.md"), "---\nstatus: dev\n---\n\nBody\n").unwrap();
        app.ledger_writeback = crate::config::LedgerWritebackConfig {
            markdown_path: Some(format!("{}/{{id}}.md", dir.display())),
            done_status: Some("verify".into()),
            ..Default::default()
        };
        request(
            &mut app,
            Method::LedgerDecide(LedgerDecideParams {
                pane_id: pane_id.clone(),
                item_id: "SHE-1".into(),
                text: "Use Checkout".into(),
            }),
        );
        let complete = |app: &mut App, item_id: &str| {
            request(
                app,
                Method::LedgerComplete(LedgerCompleteParams {
                    pane_id: pane_id.clone(),
                    item_id: item_id.into(),
                    summary: None,
                }),
            )
        };
        // Answered at once; the write-back runs on its own and reports through an event.
        assert_eq!(complete(&mut app, "SHE-1")["result"]["type"], "ledger_item");
        complete(&mut app, "SHE-2");
        for _ in 0..2 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(10), app.event_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            app.handle_internal_event(event);
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("SHE-1.md")).unwrap(),
            "---\nstatus: verify\n---\n\nBody\n\n## Decisions\n\n- Use Checkout\n"
        );
        let revision = app.state.work_ledger_revision;
        let response = request(&mut app, Method::LedgerList(LedgerListParams::default()));
        let items = &response["result"]["panes"][0]["items"];
        let outcome = |id: &str| {
            items
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == id)
                .unwrap()["writeback"]
                .clone()
        };
        assert_eq!(outcome("SHE-1")["ok"], true);
        assert_eq!(outcome("SHE-1")["message"], "wrote SHE-1.md");
        assert_eq!(outcome("SHE-2")["ok"], false);
        assert_eq!(outcome("SHE-2")["code"], "missing_file");

        // Done again: the written card is left alone, the failed one is tried again.
        complete(&mut app, "SHE-1");
        complete(&mut app, "SHE-2");
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), app.event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            &event,
            crate::events::AppEvent::LedgerWritebackFinished { item_id, .. } if item_id == "SHE-2"
        ));
        assert!(app.event_rx.try_recv().is_err());
        assert_eq!(app.state.work_ledger_revision, revision);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn done_while_writing_back_starts_no_second_run_and_changes_are_written_again() {
        let (mut app, pane_id) = app_with_test_workspace();
        let dir = std::env::temp_dir().join(format!(
            "sheprd-ledger-rerun-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SHE-1.md");
        std::fs::write(&path, "Body\n").unwrap();
        app.ledger_writeback = crate::config::LedgerWritebackConfig {
            markdown_path: Some(format!("{}/{{id}}.md", dir.display())),
            ..Default::default()
        };
        let decide = |app: &mut App, text: &str| {
            request(
                app,
                Method::LedgerDecide(LedgerDecideParams {
                    pane_id: pane_id.clone(),
                    item_id: "SHE-1".into(),
                    text: text.into(),
                }),
            );
        };
        let complete = |app: &mut App| {
            request(
                app,
                Method::LedgerComplete(LedgerCompleteParams {
                    pane_id: pane_id.clone(),
                    item_id: "SHE-1".into(),
                    summary: None,
                }),
            );
        };
        async fn finish(app: &mut App) {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(10), app.event_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            app.handle_internal_event(event);
        }

        decide(&mut app, "First");
        complete(&mut app);
        // A second done while the first write-back runs starts none.
        complete(&mut app);
        finish(&mut app).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(app.event_rx.try_recv().is_err(), "one write-back only");
        assert!(app.ledger_writebacks_in_flight.is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Body\n\n## Decisions\n\n- First\n"
        );

        // A decision after done: the next done writes it, though `complete` changes nothing.
        decide(&mut app, "Second");
        complete(&mut app);
        finish(&mut app).await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Body\n\n## Decisions\n\n- First\n- Second\n"
        );
        // Nothing new: no run.
        complete(&mut app);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(app.event_rx.try_recv().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
