//! andreconde fork (sheprd): the card ledger API (SHE-100008). Agents report per-card status,
//! summaries, open questions and decisions for their pane; `sheprd card …` is the CLI for it.

use serde::{Deserialize, Serialize};

pub use crate::work_ledger::{LedgerAnswer, LedgerAnswerInfo, LedgerItem, LedgerQuestionInput};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerUpdateParams {
    pub pane_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Free text (e.g. "dev", "review", "blocked"). An empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Seeded from what a hook saw rather than written by the agent; never overrides an
    /// explicit item.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inferred: bool,
    /// With `inferred`: replaces the item's inferred questions (an empty list clears them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inferred_questions: Option<Vec<LedgerQuestionInput>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerAskParams {
    pub pane_id: String,
    pub item_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Index into `options` of the suggested answer (0-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerDecideParams {
    pub pane_id: String,
    pub item_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerCompleteParams {
    pub pane_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerItemTarget {
    pub pane_id: String,
    pub item_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerListParams {
    /// One pane; every pane with a ledger when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// The pane's agent is reading its own cards: its sent answers count as read (SHE-100009).
    /// Needs `pane_id`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ack: bool,
}

/// Stages the user's answer to a question (SHE-100009); `answer: null` unstages it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerStageParams {
    pub pane_id: String,
    pub item_id: String,
    pub question_id: u32,
    #[serde(default)]
    pub answer: Option<LedgerAnswer>,
}

/// Dismisses a question (`item_id` and `question_id`), a card (`item_id`), or every done card
/// of the pane, or of every pane when `pane_id` is omitted too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerDismissParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<u32>,
}

/// Types the pane's staged answers into it as one line, or every pane's (one line each) when
/// `pane_id` is omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerSendParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// The sent and dismissed answers of a pane's cards; they count as read unless `peek`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerAnswersParams {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub peek: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerSentPane {
    pub pane_id: String,
    /// The line typed into the pane.
    pub text: String,
    pub questions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerSendFailure {
    pub pane_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LedgerPaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    /// Waiting on the user first, then active, then done.
    pub items: Vec<LedgerItem>,
}
