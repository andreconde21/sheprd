//! andreconde fork (sheprd): `ledger.*` handlers, the card ledger of a pane's terminal
//! (SHE-100008). Writes bump `work_ledger_revision` (client fanout resends on it), mark the
//! session dirty (the ledger is persisted) and emit `ledger.updated`.

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, LedgerAskParams, LedgerCompleteParams, LedgerDecideParams,
    LedgerItemTarget, LedgerListParams, LedgerPaneInfo, LedgerUpdateParams, ResponseResult,
};
use crate::app::App;
use crate::layout::PaneId;
use crate::work_ledger::{LedgerError, LedgerQuestionInput, LedgerUpdate, LedgerWrite, WorkLedger};

use super::responses::{encode_error, encode_success};

/// The pane a ledger request names, resolved to what a write needs.
struct LedgerTarget {
    ws_idx: usize,
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
        self.ledger_item_response(id, &target, &item_id, result)
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
        let panes = match params.pane_id.as_deref() {
            Some(pane_id) => {
                let Some(target) = self.ledger_target(pane_id) else {
                    return pane_not_found(id, pane_id);
                };
                vec![self.ledger_pane_info(
                    target.ws_idx,
                    &target.public_pane_id,
                    &target.terminal_id,
                )]
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
        EventData, LedgerAskParams, LedgerCompleteParams, LedgerDecideParams, LedgerItemTarget,
        LedgerListParams, LedgerUpdateParams, Method, Request, ResponseResult, SuccessResponse,
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
    }
}
