//! andreconde fork (sheprd): the card ledger as the work panel draws it (SHE-100008). Built once
//! per push from the server, so drawing a frame only looks rows up: no sorting, counting or
//! formatting of other panes' ledgers in the render path.

use std::collections::{HashMap, HashSet};

use crate::work_ledger::LedgerItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LedgerRowState {
    /// Has questions for the user.
    Waiting,
    Active,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LedgerRow {
    pub(super) item: LedgerItem,
    pub(super) state: LedgerRowState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PaneLedger {
    /// Waiting on the user, then active, then done (the server's order).
    pub(super) rows: Vec<LedgerRow>,
    /// Item ids, so the TASKS section can leave out tasks a card row already shows.
    pub(super) ids: HashSet<String>,
    pub(super) open_questions: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkLedgerView {
    generation: Option<u64>,
    boot_id: String,
    revision: u64,
    panes: HashMap<String, PaneLedger>,
}

impl WorkLedgerView {
    pub(crate) fn from_wire(
        generation: Option<u64>,
        ledger: crate::protocol::endpoint::EndpointWorkLedger,
    ) -> Self {
        let panes = ledger
            .panes
            .into_iter()
            .map(|(pane_id, items)| {
                let ids = items.iter().map(|item| item.id.clone()).collect();
                let open_questions = items.iter().map(|item| item.questions.len()).sum();
                let rows = items
                    .into_iter()
                    .map(|item| LedgerRow {
                        state: if crate::work_ledger::waiting_on_user(&item) {
                            LedgerRowState::Waiting
                        } else if item.done {
                            LedgerRowState::Done
                        } else {
                            LedgerRowState::Active
                        },
                        item,
                    })
                    .collect();
                (
                    pane_id,
                    PaneLedger {
                        rows,
                        ids,
                        open_questions,
                    },
                )
            })
            .collect();
        Self {
            generation,
            boot_id: ledger.boot_id,
            revision: ledger.revision,
            panes,
        }
    }

    /// Whether a push replaces this view: a new connection or server boot, or a newer push.
    pub(crate) fn superseded_by(
        &self,
        generation: Option<u64>,
        boot_id: &str,
        revision: u64,
    ) -> bool {
        self.generation != generation || self.boot_id != boot_id || revision > self.revision
    }

    pub(super) fn pane(&self, pane_id: &str) -> Option<&PaneLedger> {
        self.panes.get(pane_id).filter(|pane| !pane.rows.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(revision: u64) -> crate::protocol::endpoint::EndpointWorkLedger {
        let mut ledger = crate::work_ledger::WorkLedger::default();
        ledger.complete("SHE-1", None, None, 1).unwrap();
        ledger
            .decide("SHE-2", "Use Checkout".into(), None, 2)
            .unwrap();
        ledger
            .ask(
                "SHE-3",
                crate::work_ledger::LedgerQuestionInput {
                    text: "Monthly?".into(),
                    ..Default::default()
                },
                None,
                3,
            )
            .unwrap();
        crate::protocol::endpoint::EndpointWorkLedger {
            boot_id: "boot".into(),
            revision,
            panes: [(
                "w1:p1".to_owned(),
                ledger.sorted_items().into_iter().cloned().collect(),
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn rows_are_classified_once_per_push() {
        let view = WorkLedgerView::from_wire(Some(1), wire(1));
        let pane = view.pane("w1:p1").unwrap();
        let states = pane
            .rows
            .iter()
            .map(|row| (row.item.id.as_str(), row.state))
            .collect::<Vec<_>>();
        assert_eq!(
            states,
            vec![
                ("SHE-3", LedgerRowState::Waiting),
                ("SHE-2", LedgerRowState::Active),
                ("SHE-1", LedgerRowState::Done),
            ]
        );
        assert_eq!(pane.open_questions, 1);
        assert!(pane.ids.contains("SHE-2"));
        assert!(view.pane("w1:p2").is_none());
    }

    #[test]
    fn only_newer_pushes_or_new_connections_replace_the_view() {
        let view = WorkLedgerView::from_wire(Some(1), wire(5));
        assert!(!view.superseded_by(Some(1), "boot", 5));
        assert!(!view.superseded_by(Some(1), "boot", 4));
        assert!(view.superseded_by(Some(1), "boot", 6));
        assert!(view.superseded_by(Some(2), "boot", 1));
        assert!(view.superseded_by(Some(1), "other-boot", 1));
    }
}
