//! andreconde fork (sheprd): the card ledger (SHE-100008). Per terminal, what an agent reports
//! about each card it works on: status, a short summary, open questions with suggested answers,
//! and the decisions taken. Card ids are opaque text (SHE-12, CAL-40, #123); nothing here knows a
//! tracker. Explicit writes come from `sheprd card …`; the Claude Code hook seeds inferred items
//! for agents that never call it, and an explicit write always wins over an inferred one.
//!
//! Pure data: no I/O, no clocks (callers pass `now_ms`). The same types are persisted in the
//! session snapshot, returned by the `ledger.*` API and pushed to clients, so every field added
//! later must be optional (`#[serde(default)]`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_ITEM_ID_CHARS: usize = 32;
pub(crate) const MAX_ITEMS: usize = 48;
pub(crate) const MAX_OPEN_QUESTIONS: usize = 8;
pub(crate) const MAX_OPTIONS: usize = 6;
pub(crate) const MAX_OPTION_CHARS: usize = 80;
pub(crate) const MAX_DECISIONS: usize = 16;
pub(crate) const MAX_DECISION_CHARS: usize = 200;
pub(crate) const MAX_SUMMARY_CHARS: usize = 600;
pub(crate) const MAX_TITLE_CHARS: usize = 120;
pub(crate) const MAX_STATUS_CHARS: usize = 40;
pub(crate) const MAX_TEXT_CHARS: usize = 400;
pub(crate) const MAX_LINK_CHARS: usize = 512;
/// Serialized size one pane's ledger may take. A write that would pass it first drops the oldest
/// decisions, then old done and inferred cards, and is refused when that is not enough.
pub(crate) const MAX_PANE_BYTES: usize = 128 * 1024;
/// Serialized size of all panes' ledgers in one client push, well under the 2 MiB frame limit.
pub(crate) const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkLedger {
    #[serde(default)]
    items: BTreeMap<String, LedgerItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Seeded by the hook from what it saw, not written by the agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inferred: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The agent session that wrote it last, when the pane reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    #[serde(default)]
    pub updated_at_ms: u64,
    /// Question ids are never reused within an item, so an answer (SHE-100009) can name one.
    #[serde(default)]
    pub next_question_id: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<LedgerQuestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<LedgerDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerQuestion {
    pub id: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Index into `options` of the suggested answer (0-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inferred: bool,
    #[serde(default)]
    pub asked_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerDecision {
    pub text: String,
    #[serde(default)]
    pub decided_at_ms: u64,
}

/// A question as a writer sends it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerQuestionInput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
}

/// Fields to set on an item. `None` leaves a field alone; an empty string clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LedgerUpdate {
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) inferred: bool,
    /// Inferred writes only: replaces the item's inferred questions.
    pub(crate) inferred_questions: Option<Vec<LedgerQuestionInput>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerError {
    InvalidId(String),
    Invalid(String),
    Limit(String),
}

impl LedgerError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            LedgerError::InvalidId(_) => "invalid_ledger_item_id",
            LedgerError::Invalid(_) => "invalid_ledger_request",
            LedgerError::Limit(_) => "ledger_limit",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            LedgerError::InvalidId(message)
            | LedgerError::Invalid(message)
            | LedgerError::Limit(message) => message,
        }
    }
}

/// Whether the panel should put this item first: it has questions for the user.
pub(crate) fn waiting_on_user(item: &LedgerItem) -> bool {
    !item.questions.is_empty()
}

pub(crate) fn validate_item_id(id: &str) -> Result<(), LedgerError> {
    let valid = !id.is_empty()
        && id.chars().count() <= MAX_ITEM_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(LedgerError::InvalidId(format!(
            "card id must be 1-{MAX_ITEM_ID_CHARS} characters of A-Z a-z 0-9 # . _ -"
        )))
    }
}

fn bounded(field: &str, value: String, max: usize) -> Result<String, LedgerError> {
    let value = value.trim().to_owned();
    if value.chars().count() > max {
        return Err(LedgerError::Invalid(format!(
            "{field} may be at most {max} characters"
        )));
    }
    // Control characters (newlines and escape sequences included) would reach terminals raw.
    if value.chars().any(char::is_control) {
        return Err(LedgerError::Invalid(format!(
            "{field} must be one line without control characters"
        )));
    }
    Ok(value)
}

fn question(input: LedgerQuestionInput) -> Result<LedgerQuestionInput, LedgerError> {
    let text = bounded("question", input.text, MAX_TEXT_CHARS)?;
    if text.is_empty() {
        return Err(LedgerError::Invalid("question must not be empty".into()));
    }
    if input.options.len() > MAX_OPTIONS {
        return Err(LedgerError::Invalid(format!(
            "a question may have at most {MAX_OPTIONS} options"
        )));
    }
    let options = input
        .options
        .into_iter()
        .map(|option| bounded("option", option, MAX_OPTION_CHARS))
        .collect::<Result<Vec<_>, _>>()?;
    if options.iter().any(String::is_empty) {
        return Err(LedgerError::Invalid("options must not be empty".into()));
    }
    if input
        .recommended
        .is_some_and(|index| index >= options.len())
    {
        return Err(LedgerError::Invalid(
            "the recommended option must be one of the options".into(),
        ));
    }
    Ok(LedgerQuestionInput {
        text,
        options,
        recommended: input.recommended,
    })
}

impl LedgerItem {
    fn new(id: &str, inferred: bool) -> Self {
        Self {
            id: id.to_owned(),
            title: String::new(),
            status: None,
            done: false,
            summary: String::new(),
            inferred,
            link: None,
            agent_session: None,
            updated_at_ms: 0,
            next_question_id: 1,
            questions: Vec::new(),
            decisions: Vec::new(),
        }
    }

    /// An explicit write takes over an inferred item: the hook's guesses go.
    fn make_explicit(&mut self) -> bool {
        if !self.inferred {
            return false;
        }
        self.inferred = false;
        self.questions.retain(|question| !question.inferred);
        true
    }

    fn next_question_id(&mut self) -> u32 {
        let id = self.next_question_id.max(1);
        self.next_question_id = id.saturating_add(1);
        id
    }

    fn touch(&mut self, session: Option<&str>, now_ms: u64) {
        self.updated_at_ms = now_ms;
        if let Some(session) = session {
            self.agent_session = Some(session.to_owned());
        }
    }
}

impl WorkLedger {
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn get(&self, id: &str) -> Option<&LedgerItem> {
        self.items.get(id)
    }

    /// Items in panel order: waiting on the user, then active, then done; newest first in each.
    pub(crate) fn sorted_items(&self) -> Vec<&LedgerItem> {
        let mut items = self.items.values().collect::<Vec<_>>();
        items.sort_by(|left, right| {
            sort_group(left)
                .cmp(&sort_group(right))
                .then(right.updated_at_ms.cmp(&left.updated_at_ms))
                .then(left.id.cmp(&right.id))
        });
        items
    }

    /// The item to write, created when missing. A full ledger drops its oldest done item, then
    /// its oldest inferred one; with neither the write is refused. An inferred write only ever
    /// makes room by dropping another inferred item, and is dropped itself (`None`) otherwise.
    fn entry(
        &mut self,
        id: &str,
        inferred: bool,
    ) -> Result<Option<(&mut LedgerItem, bool)>, LedgerError> {
        validate_item_id(id)?;
        let mut created = false;
        if !self.items.contains_key(id) {
            if self.items.len() >= MAX_ITEMS {
                let Some(evict) = self.eviction_candidate(id, inferred) else {
                    if inferred {
                        return Ok(None);
                    }
                    return Err(LedgerError::Limit(format!(
                        "a pane may track at most {MAX_ITEMS} open cards; mark some done or remove them"
                    )));
                };
                self.items.remove(&evict);
            }
            self.items
                .insert(id.to_owned(), LedgerItem::new(id, inferred));
            created = true;
        }
        let item = self
            .items
            .get_mut(id)
            .ok_or_else(|| LedgerError::Invalid("ledger item vanished".into()))?;
        Ok(Some((item, created)))
    }

    /// The item to drop to make room for `keep`: the oldest done one, then the oldest inferred
    /// one. For an inferred writer only inferred items count.
    fn eviction_candidate(&self, keep: &str, inferred_writer: bool) -> Option<String> {
        let oldest = |filter: &dyn Fn(&LedgerItem) -> bool| {
            self.items
                .values()
                .filter(|item| item.id != keep && filter(item))
                .min_by_key(|item| item.updated_at_ms)
                .map(|item| item.id.clone())
        };
        if inferred_writer {
            return oldest(&|item| item.inferred);
        }
        oldest(&|item| item.done).or_else(|| oldest(&|item| item.inferred))
    }

    fn serialized_len(&self) -> usize {
        serde_json::to_vec(&self.items).map_or(usize::MAX, |bytes| bytes.len())
    }

    /// Runs one write and keeps the ledger within `MAX_PANE_BYTES`: the oldest decisions go
    /// first, then old cards as `entry` would drop them. A write that still does not fit is
    /// undone; an inferred one is dropped quietly (`Ok(None)`), an explicit one is refused.
    fn within_budget<T>(
        &mut self,
        keep: &str,
        inferred: bool,
        write: impl FnOnce(&mut Self) -> Result<T, LedgerError>,
    ) -> Result<Option<T>, LedgerError> {
        let before = self.clone();
        let result = write(self)?;
        while self.serialized_len() > MAX_PANE_BYTES {
            if !inferred && self.drop_oldest_decision() {
                continue;
            }
            match self.eviction_candidate(keep, inferred) {
                Some(evict) => {
                    self.items.remove(&evict);
                }
                None => {
                    *self = before;
                    if inferred {
                        return Ok(None);
                    }
                    return Err(LedgerError::Limit(format!(
                        "a pane's cards may take at most {} KiB; shorten them or remove some",
                        MAX_PANE_BYTES / 1024
                    )));
                }
            }
        }
        Ok(Some(result))
    }

    fn drop_oldest_decision(&mut self) -> bool {
        let Some(item) = self
            .items
            .values_mut()
            .filter(|item| !item.decisions.is_empty())
            .min_by_key(|item| item.decisions[0].decided_at_ms)
        else {
            return false;
        };
        item.decisions.remove(0);
        true
    }

    pub(crate) fn update(
        &mut self,
        id: &str,
        update: LedgerUpdate,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        let inferred = update.inferred;
        Ok(self
            .within_budget(id, inferred, |ledger| {
                ledger.apply_update(id, update, session, now_ms)
            })?
            .unwrap_or(false))
    }

    fn apply_update(
        &mut self,
        id: &str,
        update: LedgerUpdate,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        validate_item_id(id)?;
        let title = update
            .title
            .map(|title| bounded("title", title, MAX_TITLE_CHARS))
            .transpose()?;
        let status = update
            .status
            .map(|status| bounded("status", status, MAX_STATUS_CHARS))
            .transpose()?;
        let summary = update
            .summary
            .map(|summary| bounded("summary", summary, MAX_SUMMARY_CHARS))
            .transpose()?;
        let link = update
            .link
            .map(|link| bounded("link", link, MAX_LINK_CHARS))
            .transpose()?;
        if update.inferred_questions.is_some() && !update.inferred {
            return Err(LedgerError::Invalid(
                "inferred questions need an inferred write; ask explicit questions with ledger.ask"
                    .into(),
            ));
        }
        let inferred_questions = update
            .inferred_questions
            .map(|questions| {
                questions
                    .into_iter()
                    .take(MAX_OPEN_QUESTIONS)
                    .map(question)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        if update.inferred && self.items.get(id).is_some_and(|item| !item.inferred) {
            // Explicit wins: the hook's guess never overwrites what the agent said.
            return Ok(false);
        }
        let Some((item, created)) = self.entry(id, update.inferred)? else {
            // A full ledger has no room for another guess.
            return Ok(false);
        };
        let mut changed = created;
        if !update.inferred {
            changed |= item.make_explicit();
        }
        if let Some(title) = title {
            changed |= item.title != title;
            item.title = title;
        }
        if let Some(status) = status {
            let done = update.inferred && status == "done";
            let status = (!status.is_empty()).then_some(status);
            changed |= item.status != status || item.done != done;
            item.status = status;
            // An explicit status reopens a done card; the hook's "done" closes its own.
            item.done = done;
        }
        if let Some(summary) = summary {
            changed |= item.summary != summary;
            item.summary = summary;
        }
        if let Some(link) = link {
            let link = (!link.is_empty()).then_some(link);
            changed |= item.link != link;
            item.link = link;
        }
        if let Some(questions) = inferred_questions {
            let previous = item
                .questions
                .iter()
                .filter(|question| question.inferred)
                .cloned()
                .collect::<Vec<_>>();
            let mut next = Vec::with_capacity(questions.len());
            for input in questions {
                match previous.iter().find(|question| question.text == input.text) {
                    Some(question) => next.push(LedgerQuestion {
                        options: input.options,
                        recommended: input.recommended,
                        ..question.clone()
                    }),
                    None => next.push(LedgerQuestion {
                        id: item.next_question_id(),
                        text: input.text,
                        options: input.options,
                        recommended: input.recommended,
                        inferred: true,
                        asked_at_ms: now_ms,
                    }),
                }
            }
            if next != previous {
                item.questions.retain(|question| !question.inferred);
                item.questions.extend(next);
                changed = true;
            }
        }
        if changed {
            item.touch(session, now_ms);
        }
        Ok(changed)
    }

    /// Adds an open question; returns its id and whether the ledger changed (asking the same
    /// open question again returns the existing one).
    pub(crate) fn ask(
        &mut self,
        id: &str,
        input: LedgerQuestionInput,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<(u32, bool), LedgerError> {
        explicit(self.within_budget(id, false, |ledger| {
            ledger.apply_ask(id, input, session, now_ms)
        })?)
    }

    fn apply_ask(
        &mut self,
        id: &str,
        input: LedgerQuestionInput,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<(u32, bool), LedgerError> {
        validate_item_id(id)?;
        let input = question(input)?;
        if let Some(existing) = self.items.get(id).and_then(|item| {
            item.questions
                .iter()
                .find(|question| !question.inferred && question.text == input.text)
                .filter(|question| {
                    question.options == input.options && question.recommended == input.recommended
                })
        }) {
            return Ok((existing.id, false));
        }
        // Re-asking with other options replaces the earlier question of the same text.
        let open = self.items.get(id).map_or(0, |item| {
            item.questions
                .iter()
                .filter(|question| !question.inferred && question.text != input.text)
                .count()
        });
        if open >= MAX_OPEN_QUESTIONS {
            return Err(LedgerError::Limit(format!(
                "a card may have at most {MAX_OPEN_QUESTIONS} open questions"
            )));
        }
        let (item, _) = explicit(self.entry(id, false)?)?;
        item.make_explicit();
        item.questions
            .retain(|question| question.text != input.text);
        let question_id = item.next_question_id();
        item.questions.push(LedgerQuestion {
            id: question_id,
            text: input.text,
            options: input.options,
            recommended: input.recommended,
            inferred: false,
            asked_at_ms: now_ms,
        });
        item.touch(session, now_ms);
        Ok((question_id, true))
    }

    pub(crate) fn decide(
        &mut self,
        id: &str,
        text: String,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        explicit(self.within_budget(id, false, |ledger| {
            ledger.apply_decide(id, text, session, now_ms)
        })?)
    }

    fn apply_decide(
        &mut self,
        id: &str,
        text: String,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        validate_item_id(id)?;
        let text = bounded("decision", text, MAX_DECISION_CHARS)?;
        if text.is_empty() {
            return Err(LedgerError::Invalid("decision must not be empty".into()));
        }
        let (item, created) = explicit(self.entry(id, false)?)?;
        let mut changed = created | item.make_explicit();
        if !item.decisions.iter().any(|decision| decision.text == text) {
            item.decisions.push(LedgerDecision {
                text,
                decided_at_ms: now_ms,
            });
            if item.decisions.len() > MAX_DECISIONS {
                let excess = item.decisions.len() - MAX_DECISIONS;
                item.decisions.drain(..excess);
            }
            changed = true;
        }
        if changed {
            item.touch(session, now_ms);
        }
        Ok(changed)
    }

    pub(crate) fn complete(
        &mut self,
        id: &str,
        summary: Option<String>,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        explicit(self.within_budget(id, false, |ledger| {
            ledger.apply_complete(id, summary, session, now_ms)
        })?)
    }

    fn apply_complete(
        &mut self,
        id: &str,
        summary: Option<String>,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<bool, LedgerError> {
        validate_item_id(id)?;
        let summary = summary
            .map(|summary| bounded("summary", summary, MAX_SUMMARY_CHARS))
            .transpose()?;
        let (item, created) = explicit(self.entry(id, false)?)?;
        let mut changed = created | item.make_explicit() | !item.done;
        item.done = true;
        // A done card asks nothing any more, so it no longer sorts as waiting on the user.
        changed |= !item.questions.is_empty();
        item.questions.clear();
        if let Some(summary) = summary {
            changed |= item.summary != summary;
            item.summary = summary;
        }
        if changed {
            item.touch(session, now_ms);
        }
        Ok(changed)
    }

    pub(crate) fn remove(&mut self, id: &str) -> Result<bool, LedgerError> {
        validate_item_id(id)?;
        Ok(self.items.remove(id).is_some())
    }
}

/// Explicit writes always get their item (`entry` returns `None` only to inferred ones).
fn explicit<T>(value: Option<T>) -> Result<T, LedgerError> {
    value.ok_or_else(|| LedgerError::Invalid("ledger write was dropped".into()))
}

/// Keeps one client push within `MAX_MESSAGE_BYTES`: done cards lose their details first, then
/// go; then every card loses its summary and decisions; then whole panes go, the last first.
pub(crate) fn fit_message(panes: &mut BTreeMap<String, Vec<LedgerItem>>) {
    let size = |panes: &BTreeMap<String, Vec<LedgerItem>>| {
        serde_json::to_vec(panes).map_or(usize::MAX, |bytes| bytes.len())
    };
    if size(panes) <= MAX_MESSAGE_BYTES {
        return;
    }
    let steps: [fn(&mut Vec<LedgerItem>); 3] = [
        |items| {
            for item in items.iter_mut().filter(|item| item.done) {
                item.summary.clear();
                item.link = None;
                item.questions.clear();
                item.decisions.clear();
            }
        },
        |items| items.retain(|item| !item.done),
        |items| {
            for item in items.iter_mut() {
                item.summary.clear();
                item.decisions.clear();
            }
        },
    ];
    for step in steps {
        panes.values_mut().for_each(step);
        if size(panes) <= MAX_MESSAGE_BYTES {
            return;
        }
    }
    while size(panes) > MAX_MESSAGE_BYTES && panes.pop_last().is_some() {}
}

fn sort_group(item: &LedgerItem) -> u8 {
    if waiting_on_user(item) {
        0
    } else if !item.done {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explicit(title: &str) -> LedgerUpdate {
        LedgerUpdate {
            title: Some(title.into()),
            ..LedgerUpdate::default()
        }
    }

    fn inferred(status: &str, questions: Option<&[&str]>) -> LedgerUpdate {
        LedgerUpdate {
            title: Some("guessed".into()),
            status: Some(status.into()),
            inferred: true,
            inferred_questions: questions.map(|questions| {
                questions
                    .iter()
                    .map(|text| LedgerQuestionInput {
                        text: (*text).into(),
                        ..LedgerQuestionInput::default()
                    })
                    .collect()
            }),
            ..LedgerUpdate::default()
        }
    }

    fn ask(text: &str, options: &[&str], recommended: Option<usize>) -> LedgerQuestionInput {
        LedgerQuestionInput {
            text: text.into(),
            options: options.iter().map(|option| (*option).into()).collect(),
            recommended,
        }
    }

    #[test]
    fn item_ids_are_opaque_but_bounded() {
        for id in ["SHE-12", "CAL-40", "#123", "cb_12.v2"] {
            assert_eq!(validate_item_id(id), Ok(()), "{id}");
        }
        for id in [
            "",
            "has space",
            "a/b",
            "ü",
            &"x".repeat(MAX_ITEM_ID_CHARS + 1),
        ] {
            assert!(validate_item_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn update_creates_changes_and_reports_no_op() {
        let mut ledger = WorkLedger::default();
        assert_eq!(
            ledger.update("SHE-1", explicit("Stripe"), Some("s1"), 10),
            Ok(true)
        );
        assert_eq!(
            ledger.update("SHE-1", explicit("Stripe"), Some("s1"), 20),
            Ok(false)
        );
        let item = ledger.get("SHE-1").unwrap();
        assert_eq!(
            (
                item.title.as_str(),
                item.updated_at_ms,
                item.agent_session.as_deref()
            ),
            ("Stripe", 10, Some("s1"))
        );
        let cleared = LedgerUpdate {
            status: Some("review".into()),
            link: Some("https://example.test/SHE-1".into()),
            ..LedgerUpdate::default()
        };
        assert_eq!(ledger.update("SHE-1", cleared, None, 30), Ok(true));
        let clear = LedgerUpdate {
            status: Some(String::new()),
            link: Some(String::new()),
            ..LedgerUpdate::default()
        };
        assert_eq!(ledger.update("SHE-1", clear, None, 40), Ok(true));
        let item = ledger.get("SHE-1").unwrap();
        assert_eq!((item.status.as_deref(), item.link.as_deref()), (None, None));
    }

    #[test]
    fn field_limits_are_enforced() {
        let mut ledger = WorkLedger::default();
        let long = LedgerUpdate {
            summary: Some("x".repeat(MAX_SUMMARY_CHARS + 1)),
            ..LedgerUpdate::default()
        };
        assert!(matches!(
            ledger.update("SHE-1", long, None, 1),
            Err(LedgerError::Invalid(_))
        ));
        assert!(ledger.is_empty());
        let options = ["a", "b", "c", "d", "e", "f", "g"];
        assert!(ledger
            .ask("SHE-1", ask("q?", &options, None), None, 1)
            .is_err());
        assert!(ledger
            .ask("SHE-1", ask("q?", &["a"], Some(1)), None, 1)
            .is_err());
        assert!(ledger
            .ask("SHE-1", ask("q?", &[&"o".repeat(81)], None), None, 1)
            .is_err());
        assert!(ledger.ask("SHE-1", ask("  ", &[], None), None, 1).is_err());
        assert!(ledger.is_empty());
    }

    #[test]
    fn question_ids_stay_stable_and_are_never_reused() {
        let mut ledger = WorkLedger::default();
        let (first, _) = ledger
            .ask(
                "SHE-1",
                ask("Use Stripe?", &["yes", "no"], Some(0)),
                None,
                1,
            )
            .unwrap();
        let (second, _) = ledger
            .ask("SHE-1", ask("Which plan?", &[], None), None, 2)
            .unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(
            ledger.ask(
                "SHE-1",
                ask("Use Stripe?", &["yes", "no"], Some(0)),
                None,
                3
            ),
            Ok((1, false))
        );
        // Re-asking with other options replaces it under a fresh id.
        let (third, changed) = ledger
            .ask(
                "SHE-1",
                ask("Use Stripe?", &["yes", "no"], Some(1)),
                None,
                4,
            )
            .unwrap();
        assert_eq!((third, changed), (3, true));
        let ids = ledger
            .get("SHE-1")
            .unwrap()
            .questions
            .iter()
            .map(|question| question.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![2, 3]);
    }

    #[test]
    fn open_questions_are_capped() {
        let mut ledger = WorkLedger::default();
        for index in 0..MAX_OPEN_QUESTIONS {
            ledger
                .ask("SHE-1", ask(&format!("q{index}?"), &[], None), None, 1)
                .unwrap();
        }
        assert!(matches!(
            ledger.ask("SHE-1", ask("one more?", &[], None), None, 2),
            Err(LedgerError::Limit(_))
        ));
    }

    #[test]
    fn decisions_dedupe_and_drop_the_oldest() {
        let mut ledger = WorkLedger::default();
        assert_eq!(
            ledger.decide("SHE-1", "Use Stripe".into(), None, 1),
            Ok(true)
        );
        assert_eq!(
            ledger.decide("SHE-1", "Use Stripe".into(), None, 2),
            Ok(false)
        );
        for index in 0..MAX_DECISIONS {
            ledger
                .decide("SHE-1", format!("d{index}"), None, 3)
                .unwrap();
        }
        let decisions = &ledger.get("SHE-1").unwrap().decisions;
        assert_eq!(decisions.len(), MAX_DECISIONS);
        assert_eq!(decisions[0].text, "d0");
    }

    #[test]
    fn explicit_wins_over_inferred() {
        let mut ledger = WorkLedger::default();
        assert_eq!(
            ledger.update("SHE-1", inferred("running", Some(&["ok?"])), None, 1),
            Ok(true)
        );
        assert!(ledger.get("SHE-1").unwrap().inferred);
        // An explicit write flips it and drops the hook's questions.
        ledger
            .ask("SHE-1", ask("Real question?", &[], None), None, 2)
            .unwrap();
        let item = ledger.get("SHE-1").unwrap();
        assert!(!item.inferred);
        assert_eq!(item.questions.len(), 1);
        assert_eq!(item.questions[0].text, "Real question?");
        // Inferred writes to an explicit item are accepted no-ops.
        assert_eq!(
            ledger.update("SHE-1", inferred("done", Some(&["x?"])), None, 3),
            Ok(false)
        );
        assert_eq!(ledger.get("SHE-1").unwrap().updated_at_ms, 2);
    }

    #[test]
    fn inferred_questions_replace_the_inferred_set_keeping_ids() {
        let mut ledger = WorkLedger::default();
        ledger
            .update("session", inferred("running", Some(&["a?", "b?"])), None, 1)
            .unwrap();
        ledger
            .update("session", inferred("running", Some(&["b?", "c?"])), None, 2)
            .unwrap();
        let item = ledger.get("session").unwrap();
        let questions = item
            .questions
            .iter()
            .map(|question| (question.id, question.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(questions, vec![(2, "b?"), (3, "c?")]);
        assert_eq!(
            ledger.update("session", inferred("running", Some(&["b?", "c?"])), None, 3),
            Ok(false)
        );
        ledger
            .update("session", inferred("done", Some(&[])), None, 4)
            .unwrap();
        let item = ledger.get("session").unwrap();
        assert!(item.questions.is_empty() && item.done);
        assert!(ledger
            .update(
                "SHE-1",
                LedgerUpdate {
                    inferred_questions: Some(Vec::new()),
                    ..LedgerUpdate::default()
                },
                None,
                5
            )
            .is_err());
    }

    #[test]
    fn full_ledger_evicts_done_then_inferred_then_refuses() {
        let mut ledger = WorkLedger::default();
        for index in 0..MAX_ITEMS {
            ledger
                .update(&format!("C-{index}"), explicit("t"), None, index as u64)
                .unwrap();
        }
        assert!(matches!(
            ledger.update("NEW-1", explicit("t"), None, 100),
            Err(LedgerError::Limit(_))
        ));
        ledger.complete("C-5", None, None, 101).unwrap();
        ledger.complete("C-3", None, None, 102).unwrap();
        ledger.update("NEW-1", explicit("t"), None, 103).unwrap();
        assert!(ledger.get("C-5").is_none() && ledger.get("C-3").is_some());
        assert_eq!(ledger.len(), MAX_ITEMS);
    }

    #[test]
    fn items_sort_waiting_then_active_then_done() {
        let mut ledger = WorkLedger::default();
        ledger
            .update("A-1", explicit("old active"), None, 1)
            .unwrap();
        ledger
            .update("A-2", explicit("new active"), None, 5)
            .unwrap();
        ledger.complete("D-1", None, None, 9).unwrap();
        ledger.ask("Q-1", ask("?", &[], None), None, 2).unwrap();
        let order = ledger
            .sorted_items()
            .into_iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["Q-1", "A-2", "A-1", "D-1"]);
    }

    #[test]
    fn complete_and_reopen() {
        let mut ledger = WorkLedger::default();
        assert_eq!(
            ledger.complete("SHE-1", Some("shipped".into()), None, 1),
            Ok(true)
        );
        assert_eq!(ledger.complete("SHE-1", None, None, 2), Ok(false));
        let reopen = LedgerUpdate {
            status: Some("fixing review".into()),
            ..LedgerUpdate::default()
        };
        ledger.update("SHE-1", reopen, None, 3).unwrap();
        assert!(!ledger.get("SHE-1").unwrap().done);
        assert_eq!(ledger.remove("SHE-1"), Ok(true));
        assert_eq!(ledger.remove("SHE-1"), Ok(false));
    }

    #[test]
    fn serde_tolerates_missing_and_future_fields() {
        let ledger: WorkLedger = serde_json::from_value(serde_json::json!({
            "items": {"SHE-1": {"id": "SHE-1", "questions": [{"id": 4, "text": "x?",
                "answer": "future field"}], "future": true}}
        }))
        .unwrap();
        let item = ledger.get("SHE-1").unwrap();
        assert_eq!(item.questions[0].id, 4);
        assert!(!item.done && !item.inferred && item.title.is_empty());
        let empty: WorkLedger = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.is_empty());
        let round_trip: WorkLedger =
            serde_json::from_str(&serde_json::to_string(&ledger).unwrap()).unwrap();
        assert_eq!(round_trip, ledger);
    }

    #[test]
    fn text_fields_reject_control_characters() {
        let mut ledger = WorkLedger::default();
        for bad in ["red \u{1b}[31m", "two\nlines", "bell\u{7}", "csi \u{9b}2J"] {
            let update = LedgerUpdate {
                title: Some(bad.into()),
                ..LedgerUpdate::default()
            };
            assert!(matches!(
                ledger.update("SHE-1", update, None, 1),
                Err(LedgerError::Invalid(_))
            ));
            assert!(ledger.decide("SHE-1", bad.into(), None, 1).is_err());
            assert!(ledger.ask("SHE-1", ask(bad, &[], None), None, 1).is_err());
            assert!(ledger
                .ask("SHE-1", ask("q?", &[bad], None), None, 1)
                .is_err());
        }
        assert!(ledger.is_empty());
    }

    #[test]
    fn inferred_writes_never_push_out_explicit_cards() {
        let mut ledger = WorkLedger::default();
        for index in 0..MAX_ITEMS {
            ledger
                .update(&format!("C-{index}"), explicit("t"), None, index as u64)
                .unwrap();
        }
        ledger.complete("C-0", None, None, 100).unwrap();
        assert_eq!(
            ledger.update("GUESS-1", inferred("dev", None), None, 101),
            Ok(false)
        );
        assert!(ledger.get("GUESS-1").is_none() && ledger.get("C-0").is_some());

        ledger.remove("C-1").unwrap();
        ledger
            .update("GUESS-1", inferred("dev", None), None, 102)
            .unwrap();
        assert_eq!(
            ledger.update("GUESS-2", inferred("dev", None), None, 103),
            Ok(true)
        );
        assert!(ledger.get("GUESS-1").is_none() && ledger.get("C-0").is_some());
    }

    #[test]
    fn done_closes_open_questions() {
        let mut ledger = WorkLedger::default();
        ledger
            .ask("SHE-1", ask("Monthly?", &[], None), None, 1)
            .unwrap();
        ledger.update("SHE-2", explicit("t"), None, 2).unwrap();
        assert_eq!(ledger.complete("SHE-1", None, None, 3), Ok(true));
        let item = ledger.get("SHE-1").unwrap();
        assert!(item.done && item.questions.is_empty());
        assert_eq!(ledger.sorted_items()[1].id, "SHE-1", "done sorts last");
    }

    /// Fills one card to every per-field limit with 4-byte characters.
    fn fill_card(ledger: &mut WorkLedger, id: &str, now_ms: u64) -> Result<(), LedgerError> {
        let wide = |count: usize| "\u{1d11e}".repeat(count);
        let update = LedgerUpdate {
            title: Some(wide(MAX_TITLE_CHARS)),
            status: Some(wide(MAX_STATUS_CHARS)),
            summary: Some(wide(MAX_SUMMARY_CHARS)),
            link: Some(wide(MAX_LINK_CHARS)),
            ..LedgerUpdate::default()
        };
        ledger.update(id, update, None, now_ms)?;
        for index in 0..MAX_OPEN_QUESTIONS {
            let options = (0..MAX_OPTIONS)
                .map(|option| format!("{option}{}", wide(MAX_OPTION_CHARS - 1)))
                .collect::<Vec<_>>();
            let options = options.iter().map(String::as_str).collect::<Vec<_>>();
            let text = format!("{index}{}", wide(MAX_TEXT_CHARS - 1));
            ledger.ask(id, ask(&text, &options, Some(0)), None, now_ms)?;
        }
        for index in 0..MAX_DECISIONS {
            let text = format!("{index:02}{}", wide(MAX_DECISION_CHARS - 2));
            ledger.decide(id, text, None, now_ms)?;
        }
        Ok(())
    }

    #[test]
    fn a_pane_ledger_stays_within_its_byte_budget() {
        let mut ledger = WorkLedger::default();
        let mut refused = 0;
        for index in 0..MAX_ITEMS {
            if matches!(
                fill_card(&mut ledger, &format!("C-{index}"), index as u64),
                Err(LedgerError::Limit(_))
            ) {
                refused += 1;
            }
            assert!(ledger.serialized_len() <= MAX_PANE_BYTES);
        }
        assert!(refused > 0, "full cards cannot all fit");
        // Decisions went before any card did.
        assert!(ledger.get("C-0").is_some());
        assert!(ledger.get("C-0").unwrap().decisions.is_empty());
    }

    #[test]
    fn one_push_stays_under_the_frame_limit_however_full_the_panes_are() {
        let mut ledger = WorkLedger::default();
        for index in 0..MAX_ITEMS {
            let _ = fill_card(&mut ledger, &format!("C-{index}"), index as u64);
        }
        ledger.complete("C-0", None, None, 1_000).unwrap();
        let items = ledger
            .sorted_items()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut panes = (0..24)
            .map(|pane| (format!("w1:p{pane:02}"), items.clone()))
            .collect::<BTreeMap<_, _>>();
        assert!(serde_json::to_vec(&panes).unwrap().len() > crate::protocol::MAX_FRAME_SIZE);
        fit_message(&mut panes);
        assert!(serde_json::to_vec(&panes).unwrap().len() <= MAX_MESSAGE_BYTES);
        assert!(panes.contains_key("w1:p00"), "the first panes stay");
        assert!(
            panes["w1:p00"].iter().all(|item| !item.done),
            "done cards went first"
        );

        let mut small = BTreeMap::from([("w1:p1".to_owned(), items[..2].to_vec())]);
        let before = small.clone();
        fit_message(&mut small);
        assert_eq!(small, before, "a push that fits is left alone");
    }
}
