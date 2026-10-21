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
    /// SHE-100010: what writing the done card back to the tracker did. Cleared when the card
    /// reopens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writeback: Option<LedgerWriteback>,
}

/// The outcome of a done card's write-back (SHE-100010). `code` is open text (`written`,
/// `missing_file`, `unknown_project`, `write_failed`, `hook_failed`, …) rather than an enum, so
/// a reader that does not know a new code still shows `message`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerWriteback {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub code: String,
    /// Short, one line: the file written, or why it failed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub at_ms: u64,
    /// What was written (`ledger_writeback::fingerprint`): a done card whose title, link,
    /// summary or decisions changed since is written again on the next `done`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
    /// Whether the Markdown file and the command hook went through, each when configured: the
    /// next `done` re-runs only a part that failed (unless the card changed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<bool>,
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
    /// The user's answer (SHE-100009): staged until `sent_at_ms` is set, then waiting for the
    /// agent to acknowledge it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<LedgerAnswer>,
    /// When the answer was typed into the agent's pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at_ms: Option<u64>,
    /// The user chose not to answer. Hidden; kept so the hook does not ask it again.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dismissed: bool,
}

/// An answer to a question: one of its options (0-based) or free text. A struct rather than an
/// enum so later kinds stay optional fields that older readers skip.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A sent or dismissed answer as the agent reads it (`sheprd card answers`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerAnswerInfo {
    pub item_id: String,
    pub question_id: u32,
    pub question: String,
    /// The chosen option or the free text; absent for a dismissed question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dismissed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at_ms: Option<u64>,
}

/// One pane's staged answers, composed into the line typed into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerSend {
    pub(crate) text: String,
    pub(crate) questions: Vec<(String, u32)>,
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

/// What a write returned, and the other cards it pushed out of a full ledger to make room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerWrite<T> {
    pub(crate) value: T,
    pub(crate) dropped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerError {
    InvalidId(String),
    Invalid(String),
    Limit(String),
    NotFound(String),
}

impl LedgerError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            LedgerError::InvalidId(_) => "invalid_ledger_item_id",
            LedgerError::Invalid(_) => "invalid_ledger_request",
            LedgerError::Limit(_) => "ledger_limit",
            LedgerError::NotFound(_) => "ledger_item_not_found",
        }
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            LedgerError::InvalidId(message)
            | LedgerError::Invalid(message)
            | LedgerError::Limit(message)
            | LedgerError::NotFound(message) => message,
        }
    }
}

impl LedgerQuestion {
    /// Still asks the user something: not answered and sent, not dismissed.
    pub(crate) fn is_open(&self) -> bool {
        !self.dismissed && self.sent_at_ms.is_none()
    }

    /// Answered but not sent yet.
    pub(crate) fn is_staged(&self) -> bool {
        self.is_open() && self.answer.is_some()
    }

    /// The answer as the agent reads it: the option's text, or the free text.
    pub(crate) fn answer_text(&self) -> Option<String> {
        let answer = self.answer.as_ref()?;
        match (answer.option, answer.text.as_ref()) {
            (Some(index), _) => self.options.get(index).cloned(),
            (None, Some(text)) => Some(text.clone()),
            (None, None) => None,
        }
    }
}

/// A sent line's question hints: at most this many characters, `…` included.
pub(crate) const SEND_HINT_CHARS: usize = 40;
const SEND_SHORT_HINT_CHARS: usize = 16;
/// Past this many characters a sent line shortens its question hints.
pub(crate) const SEND_LINE_CHARS: usize = 400;

/// A free-text answer in a sent line: quoted, with `"` and `\` escaped, so a ` · ` or ` → `
/// inside it cannot be read as the next answer.
fn quote_answer(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        if matches!(ch, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// A chosen option in a sent line: as is, or quoted like free text when it holds a separator
/// (or starts with a quote) and would read as more than one answer.
fn send_option(option: &str) -> String {
    if option.contains(" · ") || option.contains(" → ") || option.starts_with('"') {
        quote_answer(option)
    } else {
        option.to_owned()
    }
}

/// A question as a sent line's hint: one line, without the separators the line itself uses.
fn send_hint(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('→', "->")
        .replace('·', "-")
        .replace(['(', ')'], "")
}

fn shorten(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(limit - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// Whether the panel should put this item first: it has questions for the user.
pub(crate) fn waiting_on_user(item: &LedgerItem) -> bool {
    item.questions.iter().any(LedgerQuestion::is_open)
}

/// The questions the panel shows: dismissed ones are gone for the user.
pub(crate) fn visible_questions(item: &LedgerItem) -> impl Iterator<Item = &LedgerQuestion> {
    item.questions.iter().filter(|question| !question.dismissed)
}

pub(crate) fn validate_item_id(id: &str) -> Result<(), LedgerError> {
    // A leading '-' would read as a flag on the command line.
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && id.chars().count() <= MAX_ITEM_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(LedgerError::InvalidId(format!(
            "card id must be 1-{MAX_ITEM_ID_CHARS} characters of A-Z a-z 0-9 # . _ -, not starting with -"
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
            writeback: None,
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

    /// The agent has seen its answers: sent questions go, and so do the explicit ones the user
    /// dismissed. Dismissed inferred questions stay hidden until the hook stops asking them.
    fn acknowledge(&mut self) -> bool {
        let before = self.questions.len();
        self.questions.retain(|question| {
            question.sent_at_ms.is_none() && (question.inferred || !question.dismissed)
        });
        self.questions.len() != before
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

    /// Runs one write and keeps the ledger within `MAX_PANE_BYTES`: the written card's own
    /// oldest decisions go first (never its newest), then other cards as `entry` would drop
    /// them. Other cards' decisions are never touched. A write that still does not fit is
    /// undone; an inferred one is dropped quietly (`Ok(None)`), an explicit one is refused.
    fn within_budget<T>(
        &mut self,
        keep: &str,
        inferred: bool,
        write: impl FnOnce(&mut Self) -> Result<T, LedgerError>,
    ) -> Result<Option<LedgerWrite<T>>, LedgerError> {
        let before = self.clone();
        let value = match write(self) {
            Ok(value) => value,
            Err(error) => {
                *self = before;
                return Err(error);
            }
        };
        while self.serialized_len() > MAX_PANE_BYTES {
            if !inferred && self.drop_oldest_decision(keep) {
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
                        "a pane's cards may take at most {} KiB; shorten this card or remove some",
                        MAX_PANE_BYTES / 1024
                    )));
                }
            }
        }
        // Cards that made room, by the item limit or the byte budget.
        let dropped = before
            .items
            .keys()
            .filter(|id| !self.items.contains_key(*id))
            .cloned()
            .collect();
        Ok(Some(LedgerWrite { value, dropped }))
    }

    fn drop_oldest_decision(&mut self, id: &str) -> bool {
        match self.items.get_mut(id) {
            Some(item) if item.decisions.len() > 1 => {
                item.decisions.remove(0);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn update(
        &mut self,
        id: &str,
        update: LedgerUpdate,
        session: Option<&str>,
        now_ms: u64,
    ) -> Result<LedgerWrite<bool>, LedgerError> {
        let inferred = update.inferred;
        Ok(self
            .within_budget(id, inferred, |ledger| {
                ledger.apply_update(id, update, session, now_ms)
            })?
            .unwrap_or(LedgerWrite {
                value: false,
                dropped: Vec::new(),
            }))
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
            changed |= item.acknowledge();
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
            if !done {
                // A reopened card's last write-back is history (SHE-100010).
                changed |= item.writeback.take().is_some();
            }
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
                    Some(question) => {
                        // A staged option means nothing against other options.
                        let answer =
                            if question.options == input.options || question.sent_at_ms.is_some() {
                                question.answer.clone()
                            } else {
                                None
                            };
                        next.push(LedgerQuestion {
                            options: input.options,
                            recommended: input.recommended,
                            answer,
                            ..question.clone()
                        })
                    }
                    None => next.push(LedgerQuestion {
                        id: item.next_question_id(),
                        text: input.text,
                        options: input.options,
                        recommended: input.recommended,
                        inferred: true,
                        asked_at_ms: now_ms,
                        answer: None,
                        sent_at_ms: None,
                        dismissed: false,
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
    ) -> Result<LedgerWrite<(u32, bool)>, LedgerError> {
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
        item.acknowledge();
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
            answer: None,
            sent_at_ms: None,
            dismissed: false,
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
    ) -> Result<LedgerWrite<bool>, LedgerError> {
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
        let mut changed = created | item.make_explicit() | item.acknowledge();
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
    ) -> Result<LedgerWrite<bool>, LedgerError> {
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

    /// Records a write-back outcome on a card that is still done (SHE-100010); a card that
    /// reopened or went meanwhile is left alone.
    pub(crate) fn set_writeback(&mut self, id: &str, outcome: LedgerWriteback) -> bool {
        match self.items.get_mut(id) {
            Some(item) if item.done && item.writeback.as_ref() != Some(&outcome) => {
                item.writeback = Some(outcome);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> Result<bool, LedgerError> {
        validate_item_id(id)?;
        Ok(self.items.remove(id).is_some())
    }

    fn question_mut(
        &mut self,
        id: &str,
        question_id: u32,
    ) -> Result<&mut LedgerQuestion, LedgerError> {
        validate_item_id(id)?;
        self.items
            .get_mut(id)
            .ok_or_else(|| LedgerError::NotFound("card is not in the ledger".into()))?
            .questions
            .iter_mut()
            .find(|question| question.id == question_id && !question.dismissed)
            .ok_or_else(|| LedgerError::NotFound("question is not open on this card".into()))
    }

    /// Stages the user's answer to an open question (`None` unstages it). A sent answer is
    /// final; the agent has it already.
    pub(crate) fn stage(
        &mut self,
        id: &str,
        question_id: u32,
        answer: Option<LedgerAnswer>,
    ) -> Result<bool, LedgerError> {
        let before = self.clone();
        let question = self.question_mut(id, question_id)?;
        if question.sent_at_ms.is_some() {
            return Err(LedgerError::Invalid("this answer was already sent".into()));
        }
        let answer = answer
            .map(|answer| match (answer.option, answer.text) {
                (Some(index), None) if index < question.options.len() => Ok(LedgerAnswer {
                    option: Some(index),
                    text: None,
                }),
                (Some(_), None) => Err(LedgerError::Invalid(
                    "the answer must be one of the question's options".into(),
                )),
                (None, Some(text)) => {
                    let text = bounded("answer", text, MAX_TEXT_CHARS)?;
                    if text.is_empty() {
                        return Err(LedgerError::Invalid("answer must not be empty".into()));
                    }
                    Ok(LedgerAnswer {
                        option: None,
                        text: Some(text),
                    })
                }
                _ => Err(LedgerError::Invalid(
                    "an answer is either an option or text".into(),
                )),
            })
            .transpose()?;
        if question.answer == answer {
            return Ok(false);
        }
        question.answer = answer;
        if self.serialized_len() > MAX_PANE_BYTES {
            *self = before;
            return Err(LedgerError::Limit(format!(
                "a pane's cards may take at most {} KiB; shorten the answer",
                MAX_PANE_BYTES / 1024
            )));
        }
        Ok(true)
    }

    /// Dismisses one question, or the whole card when `question_id` is `None`.
    pub(crate) fn dismiss(
        &mut self,
        id: &str,
        question_id: Option<u32>,
    ) -> Result<bool, LedgerError> {
        let Some(question_id) = question_id else {
            return self.remove(id);
        };
        let question = self.question_mut(id, question_id)?;
        question.dismissed = true;
        if question.sent_at_ms.is_none() {
            question.answer = None;
        }
        Ok(true)
    }

    /// Dismisses every done card; returns their ids.
    pub(crate) fn dismiss_done(&mut self) -> Vec<String> {
        let done = self
            .items
            .values()
            .filter(|item| item.done)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        for id in &done {
            self.items.remove(id);
        }
        done
    }

    /// The one line that answers every staged question, card by card:
    /// `SHE-12 Q1 (Which provider?) → Stripe · Q2 (Webhook path?) → "free \"text\""`.
    /// Each answer carries a short form of its question, so the agent can tell what it answers
    /// (it may never have asked an inferred one). Free text is quoted with `"` and `\` escaped.
    /// Past [`SEND_LINE_CHARS`] the explicit questions' hints shrink and then go, then the
    /// inferred ones shrink; the answers themselves are never cut.
    pub(crate) fn compose_send(&self) -> Option<LedgerSend> {
        struct Part {
            label: String,
            hint: String,
            inferred: bool,
            answer: String,
        }
        let mut parts = Vec::new();
        let mut questions = Vec::new();
        for item in self.items.values() {
            let mut first = true;
            for question in item
                .questions
                .iter()
                .filter(|question| question.is_staged())
            {
                let Some(answer) = question.answer.as_ref() else {
                    continue;
                };
                let answer = match (answer.option, answer.text.as_ref()) {
                    (Some(index), _) => match question.options.get(index) {
                        Some(option) => send_option(option),
                        None => continue,
                    },
                    (None, Some(text)) => quote_answer(text),
                    (None, None) => continue,
                };
                let label = if first {
                    format!("{} Q{}", item.id, question.id)
                } else {
                    format!("Q{}", question.id)
                };
                first = false;
                parts.push(Part {
                    label,
                    hint: send_hint(&question.text),
                    inferred: question.inferred || item.inferred,
                    answer,
                });
                questions.push((item.id.clone(), question.id));
            }
        }
        if parts.is_empty() {
            return None;
        }
        let line = |explicit: usize, inferred: usize| {
            parts
                .iter()
                .map(|part| {
                    let limit = if part.inferred { inferred } else { explicit };
                    let hint = shorten(&part.hint, limit);
                    if hint.is_empty() {
                        format!("{} → {}", part.label, part.answer)
                    } else {
                        format!("{} ({hint}) → {}", part.label, part.answer)
                    }
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let steps = [
            (SEND_HINT_CHARS, SEND_HINT_CHARS),
            (SEND_SHORT_HINT_CHARS, SEND_HINT_CHARS),
            (0, SEND_HINT_CHARS),
            (0, SEND_SHORT_HINT_CHARS),
        ];
        let mut text = String::new();
        for (explicit, inferred) in steps {
            text = line(explicit, inferred);
            if text.chars().count() <= SEND_LINE_CHARS {
                break;
            }
        }
        Some(LedgerSend { text, questions })
    }

    /// Marks composed answers as typed into the pane.
    pub(crate) fn mark_sent(&mut self, questions: &[(String, u32)], now_ms: u64) {
        for (id, question_id) in questions {
            if let Some(question) = self
                .items
                .get_mut(id)
                .and_then(|item| item.questions.iter_mut().find(|q| q.id == *question_id))
            {
                question.sent_at_ms = Some(now_ms);
            }
        }
    }

    /// Answers the agent has to read: sent and dismissed ones, card by card.
    pub(crate) fn answers(&self) -> Vec<LedgerAnswerInfo> {
        self.items
            .values()
            .flat_map(|item| {
                item.questions
                    .iter()
                    .filter(|question| question.sent_at_ms.is_some() || question.dismissed)
                    .map(|question| LedgerAnswerInfo {
                        item_id: item.id.clone(),
                        question_id: question.id,
                        question: question.text.clone(),
                        answer: question.sent_at_ms.and_then(|_| question.answer_text()),
                        dismissed: question.dismissed,
                        sent_at_ms: question.sent_at_ms,
                    })
            })
            .collect()
    }

    /// The agent read its answers: see `LedgerItem::acknowledge`. Returns the cards that changed.
    pub(crate) fn acknowledge(&mut self) -> Vec<String> {
        self.items
            .values_mut()
            .filter_map(|item| item.acknowledge().then(|| item.id.clone()))
            .collect()
    }
}

/// Explicit writes always get their item (`entry` returns `None` only to inferred ones).
fn explicit<T>(value: Option<T>) -> Result<T, LedgerError> {
    value.ok_or_else(|| LedgerError::Invalid("ledger write was dropped".into()))
}

/// Keeps one client push within `MAX_MESSAGE_BYTES`: done cards lose their details first, then
/// go; then every card loses its summary and decisions; then whole panes go, the largest first.
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
    let mut total = size(panes);
    let mut by_size = panes
        .iter()
        .map(|(pane_id, items)| {
            // The pane's items, its quoted key, a colon and a comma.
            let bytes = serde_json::to_vec(items).map_or(usize::MAX, |bytes| bytes.len());
            (bytes.saturating_add(pane_id.len() + 4), pane_id.clone())
        })
        .collect::<Vec<_>>();
    by_size.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    for (bytes, pane_id) in by_size {
        if total <= MAX_MESSAGE_BYTES {
            break;
        }
        panes.remove(&pane_id);
        total = total.saturating_sub(bytes);
    }
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
            "-x",
            "--pane",
            &"x".repeat(MAX_ITEM_ID_CHARS + 1),
        ] {
            assert!(validate_item_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn update_creates_changes_and_reports_no_op() {
        let mut ledger = WorkLedger::default();
        assert_eq!(
            ledger
                .update("SHE-1", explicit("Stripe"), Some("s1"), 10)
                .map(|write| write.value),
            Ok(true)
        );
        assert_eq!(
            ledger
                .update("SHE-1", explicit("Stripe"), Some("s1"), 20)
                .map(|write| write.value),
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
        assert_eq!(
            ledger
                .update("SHE-1", cleared, None, 30)
                .map(|write| write.value),
            Ok(true)
        );
        let clear = LedgerUpdate {
            status: Some(String::new()),
            link: Some(String::new()),
            ..LedgerUpdate::default()
        };
        assert_eq!(
            ledger
                .update("SHE-1", clear, None, 40)
                .map(|write| write.value),
            Ok(true)
        );
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
            .unwrap()
            .value;
        let (second, _) = ledger
            .ask("SHE-1", ask("Which plan?", &[], None), None, 2)
            .unwrap()
            .value;
        assert_eq!((first, second), (1, 2));
        assert_eq!(
            ledger
                .ask(
                    "SHE-1",
                    ask("Use Stripe?", &["yes", "no"], Some(0)),
                    None,
                    3
                )
                .map(|write| write.value),
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
            .unwrap()
            .value;
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
            ledger
                .decide("SHE-1", "Use Stripe".into(), None, 1)
                .map(|write| write.value),
            Ok(true)
        );
        assert_eq!(
            ledger
                .decide("SHE-1", "Use Stripe".into(), None, 2)
                .map(|write| write.value),
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
            ledger
                .update("SHE-1", inferred("running", Some(&["ok?"])), None, 1)
                .map(|write| write.value),
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
            ledger
                .update("SHE-1", inferred("done", Some(&["x?"])), None, 3)
                .map(|write| write.value),
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
            ledger
                .update("session", inferred("running", Some(&["b?", "c?"])), None, 3)
                .map(|write| write.value),
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
        assert_eq!(
            ledger
                .update("NEW-1", explicit("t"), None, 103)
                .unwrap()
                .dropped,
            vec!["C-5".to_owned()]
        );
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
            ledger
                .complete("SHE-1", Some("shipped".into()), None, 1)
                .map(|write| write.value),
            Ok(true)
        );
        assert_eq!(
            ledger
                .complete("SHE-1", None, None, 2)
                .map(|write| write.value),
            Ok(false)
        );
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
                "reaction": "future field"}], "future": true}}
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
            ledger
                .update("GUESS-1", inferred("dev", None), None, 101)
                .map(|write| write.value),
            Ok(false)
        );
        assert!(ledger.get("GUESS-1").is_none() && ledger.get("C-0").is_some());

        ledger.remove("C-1").unwrap();
        ledger
            .update("GUESS-1", inferred("dev", None), None, 102)
            .unwrap();
        assert_eq!(
            ledger
                .update("GUESS-2", inferred("dev", None), None, 103)
                .map(|write| write.value),
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
        assert_eq!(
            ledger
                .complete("SHE-1", None, None, 3)
                .map(|write| write.value),
            Ok(true)
        );
        let item = ledger.get("SHE-1").unwrap();
        assert!(item.done && item.questions.is_empty());
        assert_eq!(ledger.sorted_items()[1].id, "SHE-1", "done sorts last");
    }

    /// Fills one card to every per-field limit with 4-byte characters; returns the cards it
    /// pushed out.
    fn fill_card(
        ledger: &mut WorkLedger,
        id: &str,
        now_ms: u64,
    ) -> Result<Vec<String>, LedgerError> {
        let mut dropped = Vec::new();
        let wide = |count: usize| "\u{1d11e}".repeat(count);
        let update = LedgerUpdate {
            title: Some(wide(MAX_TITLE_CHARS)),
            status: Some(wide(MAX_STATUS_CHARS)),
            summary: Some(wide(MAX_SUMMARY_CHARS)),
            link: Some(wide(MAX_LINK_CHARS)),
            ..LedgerUpdate::default()
        };
        dropped.extend(ledger.update(id, update, None, now_ms)?.dropped);
        for index in 0..MAX_OPEN_QUESTIONS {
            let options = (0..MAX_OPTIONS)
                .map(|option| format!("{option}{}", wide(MAX_OPTION_CHARS - 1)))
                .collect::<Vec<_>>();
            let options = options.iter().map(String::as_str).collect::<Vec<_>>();
            let text = format!("{index}{}", wide(MAX_TEXT_CHARS - 1));
            dropped.extend(
                ledger
                    .ask(id, ask(&text, &options, Some(0)), None, now_ms)?
                    .dropped,
            );
        }
        for index in 0..MAX_DECISIONS {
            let text = format!("{index:02}{}", wide(MAX_DECISION_CHARS - 2));
            dropped.extend(ledger.decide(id, text, None, now_ms)?.dropped);
        }
        Ok(dropped)
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
        assert_eq!(
            ledger.get("C-0").unwrap().decisions.len(),
            MAX_DECISIONS,
            "a write never trims another card"
        );
    }

    #[test]
    fn a_write_only_trims_its_own_card_and_names_what_it_dropped() {
        let mut ledger = WorkLedger::default();
        fill_card(&mut ledger, "SHE-1", 1).unwrap();
        fill_card(&mut ledger, "SHE-2", 2).unwrap();
        let before = ledger.get("SHE-1").unwrap().decisions.clone();
        // SHE-3 fits only by giving up some of its own decisions.
        fill_card(&mut ledger, "SHE-3", 3).unwrap();
        assert!(ledger.get("SHE-3").unwrap().decisions.len() < MAX_DECISIONS);
        assert_eq!(ledger.get("SHE-1").unwrap().decisions, before);
        assert_eq!(ledger.get("SHE-2").unwrap().decisions.len(), MAX_DECISIONS);

        // No room left and nothing that may go: refused, nothing else touched.
        let big = || LedgerUpdate {
            summary: Some("\u{1d11e}".repeat(MAX_SUMMARY_CHARS)),
            ..LedgerUpdate::default()
        };
        assert!(matches!(
            ledger.update("SHE-4", big(), None, 4),
            Err(LedgerError::Limit(_))
        ));
        assert!(ledger.get("SHE-4").is_none());
        assert_eq!(ledger.get("SHE-1").unwrap().decisions, before);

        // A done card may make room, and the write names it.
        ledger.complete("SHE-2", None, None, 5).unwrap();
        assert_eq!(
            fill_card(&mut ledger, "SHE-4", 6),
            Ok(vec!["SHE-2".to_owned()])
        );
        assert_eq!(ledger.get("SHE-1").unwrap().decisions, before);
        assert!(ledger.serialized_len() <= MAX_PANE_BYTES);
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
        assert!(!panes.is_empty());
        assert!(
            panes.values().flatten().all(|item| !item.done),
            "done cards went first"
        );

        let mut small = BTreeMap::from([("w1:p1".to_owned(), items[..2].to_vec())]);
        let before = small.clone();
        fit_message(&mut small);
        assert_eq!(small, before, "a push that fits is left alone");
    }

    #[test]
    fn the_largest_panes_go_first_when_a_push_is_too_big() {
        let mut ledger = WorkLedger::default();
        for index in 0..MAX_ITEMS {
            let _ = fill_card(&mut ledger, &format!("C-{index}"), index as u64);
        }
        let full = ledger
            .sorted_items()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let small = full[..1].to_vec();
        // Key order would drop "w1:p9" first; size order drops the full panes.
        let mut panes = (0..20)
            .map(|pane| (format!("w1:p{pane}"), full.clone()))
            .collect::<BTreeMap<_, _>>();
        panes.insert("w1:p9".into(), small.clone());
        panes.insert("w1:p10".into(), small);
        fit_message(&mut panes);
        assert!(serde_json::to_vec(&panes).unwrap().len() <= MAX_MESSAGE_BYTES);
        assert!(panes.len() < 20, "some panes had to go");
        assert!(panes.contains_key("w1:p9") && panes.contains_key("w1:p10"));
    }

    fn option(index: usize) -> Option<LedgerAnswer> {
        Some(LedgerAnswer {
            option: Some(index),
            text: None,
        })
    }

    fn free(text: &str) -> Option<LedgerAnswer> {
        Some(LedgerAnswer {
            option: None,
            text: Some(text.into()),
        })
    }

    /// SHE-12 asks Q1 (Stripe or Multibanco) and Q2 (free); CAL-40 asks Q1.
    fn asked() -> WorkLedger {
        let mut ledger = WorkLedger::default();
        ledger
            .ask(
                "SHE-12",
                ask("Provider?", &["Stripe", "Multibanco"], Some(0)),
                None,
                1,
            )
            .unwrap();
        ledger
            .ask("SHE-12", ask("Webhook path?", &[], None), None, 2)
            .unwrap();
        ledger
            .ask("CAL-40", ask("Ship now?", &["now", "defer"], None), None, 3)
            .unwrap();
        ledger
    }

    #[test]
    fn staging_validates_and_unstages() {
        let mut ledger = asked();
        assert_eq!(ledger.stage("SHE-12", 1, option(1)), Ok(true));
        assert_eq!(
            ledger.stage("SHE-12", 1, option(1)),
            Ok(false),
            "same answer"
        );
        assert!(ledger.get("SHE-12").unwrap().questions[0].is_staged());
        assert!(
            waiting_on_user(ledger.get("SHE-12").unwrap()),
            "staged is still open"
        );
        for bad in [
            option(2),
            free(""),
            free("two\nlines"),
            Some(LedgerAnswer::default()),
        ] {
            assert!(
                matches!(
                    ledger.stage("SHE-12", 1, bad.clone()),
                    Err(LedgerError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        let both = Some(LedgerAnswer {
            option: Some(0),
            text: Some("x".into()),
        });
        assert!(ledger.stage("SHE-12", 1, both).is_err());
        assert!(matches!(
            ledger.stage("SHE-12", 9, option(0)),
            Err(LedgerError::NotFound(_))
        ));
        assert!(matches!(
            ledger.stage("NOPE-1", 1, option(0)),
            Err(LedgerError::NotFound(_))
        ));
        assert_eq!(
            ledger.get("SHE-12").unwrap().questions[0].answer,
            option(1),
            "a refused stage leaves the earlier one"
        );
        assert_eq!(ledger.stage("SHE-12", 1, None), Ok(true));
        assert!(!ledger.get("SHE-12").unwrap().questions[0].is_staged());
    }

    #[test]
    fn send_composes_one_line_marks_sent_and_ack_clears() {
        let mut ledger = asked();
        assert_eq!(ledger.compose_send(), None, "nothing staged");
        ledger.stage("SHE-12", 1, option(0)).unwrap();
        ledger
            .stage("SHE-12", 2, free("use /hooks/stripe"))
            .unwrap();
        ledger.stage("CAL-40", 1, option(1)).unwrap();
        let send = ledger.compose_send().unwrap();
        assert_eq!(
            send.text,
            "CAL-40 Q1 (Ship now?) → defer · SHE-12 Q1 (Provider?) → Stripe \
             · Q2 (Webhook path?) → \"use /hooks/stripe\""
        );
        assert_eq!(send.questions.len(), 3);
        ledger.mark_sent(&send.questions, 50);
        assert_eq!(
            ledger.compose_send(),
            None,
            "sent answers are not sent again"
        );
        assert!(!waiting_on_user(ledger.get("SHE-12").unwrap()));
        assert!(matches!(
            ledger.stage("SHE-12", 1, option(1)),
            Err(LedgerError::Invalid(_))
        ));
        let answers = ledger.answers();
        assert_eq!(answers.len(), 3);
        assert_eq!(answers[1].answer.as_deref(), Some("Stripe"));
        assert_eq!(answers[2].answer.as_deref(), Some("use /hooks/stripe"));
        assert_eq!(answers[2].sent_at_ms, Some(50));

        // The agent's next explicit write on a card acknowledges that card only.
        ledger
            .decide("SHE-12", "Stripe with a webhook".into(), None, 60)
            .unwrap();
        assert!(ledger.get("SHE-12").unwrap().questions.is_empty());
        assert_eq!(ledger.get("CAL-40").unwrap().questions.len(), 1);
        assert_eq!(ledger.acknowledge(), vec!["CAL-40".to_owned()]);
        assert!(ledger.answers().is_empty());
        assert!(ledger.acknowledge().is_empty());
    }

    #[test]
    fn options_with_a_separator_are_quoted_in_the_sent_line() {
        let mut ledger = WorkLedger::default();
        ledger
            .ask(
                "SHE-1",
                ask("Plan?", &["a · b", "x → y", "plain"], None),
                None,
                1,
            )
            .unwrap();
        ledger
            .ask("SHE-1", ask("Other?", &["plain"], None), None, 2)
            .unwrap();
        ledger.stage("SHE-1", 1, option(0)).unwrap();
        ledger.stage("SHE-1", 2, option(0)).unwrap();
        assert_eq!(
            ledger.compose_send().unwrap().text,
            "SHE-1 Q1 (Plan?) → \"a · b\" · Q2 (Other?) → plain"
        );
        ledger.stage("SHE-1", 1, option(1)).unwrap();
        assert!(ledger
            .compose_send()
            .unwrap()
            .text
            .starts_with("SHE-1 Q1 (Plan?) → \"x → y\" · "));
    }

    #[test]
    fn sent_lines_escape_free_text_and_shorten_hints_when_long() {
        let mut ledger = asked();
        ledger
            .stage("SHE-12", 2, free(r#"say "hi" · C:\tmp → x"#))
            .unwrap();
        assert_eq!(
            ledger.compose_send().unwrap().text,
            r#"SHE-12 Q2 (Webhook path?) → "say \"hi\" · C:\\tmp → x""#
        );

        let mut ledger = WorkLedger::default();
        let long = "Should the nightly export also include the archived (old) projects?";
        ledger.ask("SHE-1", ask(long, &[], None), None, 1).unwrap();
        ledger
            .update("SHE-2", inferred("running", Some(&[long])), None, 2)
            .unwrap();
        ledger.stage("SHE-1", 1, free("yes")).unwrap();
        ledger.stage("SHE-2", 1, free("no")).unwrap();
        let hint = "Should the nightly export also include…";
        assert!(hint.chars().count() <= SEND_HINT_CHARS);
        assert_eq!(
            ledger.compose_send().unwrap().text,
            format!("SHE-1 Q1 ({hint}) → \"yes\" · SHE-2 Q1 ({hint}) → \"no\"")
        );

        // A long line drops the explicit question's hint before the inferred one shrinks.
        let answer = "x".repeat(SEND_LINE_CHARS - 80);
        ledger.stage("SHE-1", 1, free(&answer)).unwrap();
        let text = ledger.compose_send().unwrap().text;
        assert!(text.starts_with("SHE-1 Q1 → \""), "{text}");
        assert!(text.contains(&format!("SHE-2 Q1 ({hint})")), "{text}");
        let answer = "x".repeat(MAX_TEXT_CHARS);
        ledger.stage("SHE-1", 1, free(&answer)).unwrap();
        let text = ledger.compose_send().unwrap().text;
        assert!(
            text.contains("SHE-2 Q1 (Should the nigh…) → \"no\""),
            "{text}"
        );
        assert!(text.contains(&answer), "answers are never cut");
    }

    #[test]
    fn inferred_writes_do_not_acknowledge_and_keep_dismissals() {
        let mut ledger = WorkLedger::default();
        ledger
            .update("SHE-1", inferred("running", Some(&["Deploy?"])), None, 1)
            .unwrap();
        ledger.stage("SHE-1", 1, free("yes")).unwrap();
        let send = ledger.compose_send().unwrap();
        ledger.mark_sent(&send.questions, 2);
        ledger
            .update(
                "SHE-1",
                inferred("running", Some(&["Deploy?", "Tag?"])),
                None,
                3,
            )
            .unwrap();
        let questions = &ledger.get("SHE-1").unwrap().questions;
        assert_eq!(
            questions[0].sent_at_ms,
            Some(2),
            "the hook does not acknowledge"
        );
        assert_eq!(ledger.dismiss("SHE-1", Some(2)), Ok(true));
        assert_eq!(ledger.acknowledge(), vec!["SHE-1".to_owned()]);
        // The hook asks the same again: the dismissed question stays dismissed.
        ledger
            .update("SHE-1", inferred("running", Some(&["Tag?"])), None, 4)
            .unwrap();
        let item = ledger.get("SHE-1").unwrap();
        assert_eq!(item.questions.len(), 1);
        assert!(item.questions[0].dismissed);
        assert!(!waiting_on_user(item));
        assert_eq!(visible_questions(item).count(), 0);
    }

    #[test]
    fn a_staged_option_is_dropped_when_the_hook_changes_the_options() {
        let mut ledger = WorkLedger::default();
        let with = |options: &[&str]| LedgerUpdate {
            inferred: true,
            inferred_questions: Some(vec![ask("Which?", options, None)]),
            ..LedgerUpdate::default()
        };
        ledger.update("SHE-1", with(&["a", "b"]), None, 1).unwrap();
        ledger.stage("SHE-1", 1, option(1)).unwrap();
        ledger.update("SHE-1", with(&["a", "b"]), None, 2).unwrap();
        assert_eq!(ledger.get("SHE-1").unwrap().questions[0].answer, option(1));
        ledger.update("SHE-1", with(&["c", "d"]), None, 3).unwrap();
        assert_eq!(ledger.get("SHE-1").unwrap().questions[0].answer, None);
    }

    #[test]
    fn dismissing_questions_cards_and_done_cards() {
        let mut ledger = asked();
        ledger.stage("SHE-12", 1, option(0)).unwrap();
        assert_eq!(ledger.dismiss("SHE-12", Some(1)), Ok(true));
        assert!(matches!(
            ledger.dismiss("SHE-12", Some(1)),
            Err(LedgerError::NotFound(_))
        ));
        let item = ledger.get("SHE-12").unwrap();
        assert_eq!(
            item.questions[0].answer, None,
            "a dismissed answer is not sent"
        );
        assert_eq!(visible_questions(item).count(), 1);
        assert_eq!(ledger.compose_send(), None);
        let answers = ledger.answers();
        assert_eq!((answers.len(), answers[0].dismissed), (1, true));
        // An explicit dismissed question goes once the agent has read it.
        ledger.acknowledge();
        assert_eq!(ledger.get("SHE-12").unwrap().questions.len(), 1);

        ledger.complete("SHE-7", None, None, 5).unwrap();
        ledger.complete("SHE-8", None, None, 6).unwrap();
        let mut done = ledger.dismiss_done();
        done.sort();
        assert_eq!(done, vec!["SHE-7".to_owned(), "SHE-8".to_owned()]);
        assert_eq!(ledger.dismiss("CAL-40", None), Ok(true));
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn old_snapshots_without_answers_still_load() {
        let json = r#"{"items":{"SHE-1":{"id":"SHE-1","questions":[{"id":1,"text":"q?"}]}}}"#;
        let ledger: WorkLedger = serde_json::from_str(json).unwrap();
        let question = &ledger.get("SHE-1").unwrap().questions[0];
        assert_eq!(
            (
                question.answer.as_ref(),
                question.sent_at_ms,
                question.dismissed
            ),
            (None, None, false)
        );
        // And an unanswered question serializes as before.
        assert!(!serde_json::to_string(&ledger).unwrap().contains("answer"));
    }

    #[test]
    fn writeback_outcomes_stay_on_done_cards_until_they_reopen() {
        let mut ledger = WorkLedger::default();
        let outcome = LedgerWriteback {
            ok: false,
            code: "missing_file".into(),
            message: "file missing: /tasks/SHE-1.md".into(),
            at_ms: 5,
            fingerprint: String::new(),
            markdown: Some(false),
            hook: None,
        };
        ledger.update("SHE-1", explicit("Stripe"), None, 1).unwrap();
        assert!(
            !ledger.set_writeback("SHE-1", outcome.clone()),
            "only done cards"
        );
        assert!(
            !ledger.set_writeback("SHE-9", outcome.clone()),
            "only cards that exist"
        );
        ledger.complete("SHE-1", None, None, 2).unwrap();
        assert!(ledger.set_writeback("SHE-1", outcome.clone()));
        assert!(
            !ledger.set_writeback("SHE-1", outcome.clone()),
            "same outcome, no change"
        );
        assert_eq!(ledger.get("SHE-1").unwrap().writeback, Some(outcome));

        // A decision on the done card keeps it; a new status reopens it and clears it.
        ledger.decide("SHE-1", "Later".into(), None, 3).unwrap();
        assert!(ledger.get("SHE-1").unwrap().writeback.is_some());
        let reopened = ledger
            .update(
                "SHE-1",
                LedgerUpdate {
                    status: Some("dev".into()),
                    ..LedgerUpdate::default()
                },
                None,
                4,
            )
            .unwrap();
        assert!(reopened.value);
        assert!(ledger.get("SHE-1").unwrap().writeback.is_none());
    }

    #[test]
    fn writeback_field_is_optional_on_the_wire() {
        // Older snapshots and servers have no `writeback`; older readers skip it.
        let item: LedgerItem =
            serde_json::from_value(serde_json::json!({"id": "SHE-1", "done": true})).unwrap();
        assert!(item.writeback.is_none());
        assert!(!serde_json::to_string(&item).unwrap().contains("writeback"));
        let item: LedgerItem = serde_json::from_value(serde_json::json!({
            "id": "SHE-1", "done": true,
            "writeback": {"ok": true, "code": "future_code", "extra": 1}
        }))
        .unwrap();
        let outcome = item.writeback.unwrap();
        assert!(outcome.ok && outcome.code == "future_code" && outcome.message.is_empty());
    }
}
