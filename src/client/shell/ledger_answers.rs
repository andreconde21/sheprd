//! andreconde fork (sheprd): answering an agent's card questions (SHE-100009). Shared by the
//! work panel and the Cards view: the option buttons a question draws, what a click on one does,
//! and the requests that stage, dismiss and send answers. Staged, sent and dismissed answers are
//! server state (every client shows them); expand, filters and scroll stay in this client.

use super::render::{display_width, put_text};
use super::*;
use crate::work_ledger::{LedgerAnswer, LedgerQuestion};

/// The endpoint methods an answer button needs; without them the buttons are not drawn.
const ANSWER_METHODS: [&str; 3] = ["ledger.dismiss", "ledger.send", "ledger.stage"];

/// Longest option label on a button; the full text is in the question's card.
const MAX_BUTTON_LABEL: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LedgerAction {
    /// Expand or collapse a card row (this client only).
    Toggle {
        endpoint_id: ClientEndpointId,
        pane_id: String,
        item_id: String,
    },
    /// Stage an answer, or unstage it with `None`.
    Stage {
        endpoint_id: ClientEndpointId,
        pane_id: String,
        item_id: String,
        question_id: u32,
        answer: Option<LedgerAnswer>,
    },
    /// Open the text prompt for a free answer, starting from the staged one.
    Write {
        endpoint_id: ClientEndpointId,
        pane_id: String,
        item_id: String,
        question_id: u32,
        current: String,
    },
    /// A question, a card, or (without an item) every done card of the pane, or of every pane
    /// on that machine without a pane.
    Dismiss {
        endpoint_id: ClientEndpointId,
        pane_id: Option<String>,
        item_id: Option<String>,
        question_id: Option<u32>,
    },
    /// Type the staged answers into the pane, or into every pane of that machine.
    Send {
        endpoint_id: ClientEndpointId,
        pane_id: Option<String>,
    },
    /// Go to the agent's pane.
    Focus {
        endpoint_id: ClientEndpointId,
        pane_id: String,
    },
}

/// Whether this machine's server takes answers (it advertised the methods when it connected).
pub(super) fn answers_supported(endpoint: &ClientShellEndpoint) -> bool {
    endpoint.methods.as_ref().is_some_and(|methods| {
        ANSWER_METHODS
            .iter()
            .all(|method| methods.contains(*method))
    })
}

/// A window on rows that may be scrolled: virtual row `first` is drawn at screen row `top`, and
/// nothing is drawn at or below `bottom`. The work panel uses it unscrolled.
pub(super) struct Canvas<'a> {
    pub(super) buffer: &'a mut Buffer,
    pub(super) top: u16,
    pub(super) first: u16,
    pub(super) bottom: u16,
}

impl Canvas<'_> {
    pub(super) fn row(&self, vy: u16) -> Option<u16> {
        vy.checked_sub(self.first)
            .map(|offset| self.top.saturating_add(offset))
            .filter(|y| *y < self.bottom)
    }

    /// Draws `text` on virtual row `vy`; the screen rect it took when visible.
    pub(super) fn put(
        &mut self,
        x: u16,
        vy: u16,
        width: u16,
        text: &str,
        style: Style,
    ) -> Option<Rect> {
        let y = self.row(vy)?;
        let width = display_width(text).min(width);
        put_text(self.buffer, x, y, width, text, style);
        Some(Rect::new(x, y, width, 1))
    }
}

/// What a question is drawn with: whose it is, and whether the user can answer it here.
pub(super) struct QuestionTarget<'a> {
    pub(super) endpoint_id: &'a ClientEndpointId,
    pub(super) pane_id: &'a str,
    pub(super) item_id: &'a str,
    pub(super) answerable: bool,
}

fn button_label(question: &LedgerQuestion, index: usize, option: &str) -> String {
    let mut label = option.chars().take(MAX_BUTTON_LABEL).collect::<String>();
    if option.chars().count() > MAX_BUTTON_LABEL {
        label.push('…');
    }
    if question.recommended == Some(index) {
        label.push('★');
    }
    format!("[{label}]")
}

/// Draws one question from virtual row `vy` between `x` and `right`: its text, then either its
/// answer buttons (`[Stripe★] [Multibanco] [✎] [✕]`, wrapped), "sent → …" until the agent reads
/// it, or (on a server without answers) its numbered options. Returns the next free row.
#[allow(clippy::too_many_arguments)] // drawing context: the panel and the view share it
pub(super) fn render_question(
    canvas: &mut Canvas<'_>,
    x: u16,
    right: u16,
    mut vy: u16,
    question: &LedgerQuestion,
    target: &QuestionTarget<'_>,
    palette: &Palette,
    base: Style,
    hits: &mut Vec<(Rect, LedgerAction)>,
) -> u16 {
    let width = right.saturating_sub(x);
    for (n, line) in super::work_panel::wrap(&question.text, usize::from(width.saturating_sub(2)))
        .into_iter()
        .take(2)
        .enumerate()
    {
        let bullet = if n == 0 { "? " } else { "  " };
        canvas.put(
            x,
            vy,
            width,
            &format!("{bullet}{line}"),
            base.fg(palette.yellow),
        );
        vy = vy.saturating_add(1);
    }
    if !target.answerable {
        for (index, option) in question.options.iter().enumerate() {
            let star = if question.recommended == Some(index) {
                " ★"
            } else {
                ""
            };
            canvas.put(
                x,
                vy,
                width,
                &format!("  {}) {option}{star}", index + 1),
                base,
            );
            vy = vy.saturating_add(1);
        }
        return vy;
    }
    if question.sent_at_ms.is_some() {
        let answer = question.answer_text().unwrap_or_default();
        canvas.put(
            x + 2,
            vy,
            width.saturating_sub(2),
            &format!("sent → {answer} · waiting for the agent"),
            base.fg(palette.overlay0),
        );
        return vy.saturating_add(1);
    }
    let staged = question.answer.as_ref();
    let mut buttons = question
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let chosen = staged.is_some_and(|answer| answer.option == Some(index));
            let answer = (!chosen).then_some(LedgerAnswer {
                option: Some(index),
                text: None,
            });
            (
                button_label(question, index, option),
                chosen,
                LedgerAction::Stage {
                    endpoint_id: target.endpoint_id.clone(),
                    pane_id: target.pane_id.to_owned(),
                    item_id: target.item_id.to_owned(),
                    question_id: question.id,
                    answer,
                },
            )
        })
        .collect::<Vec<_>>();
    let text = staged.and_then(|answer| answer.text.clone());
    let write_label = match &text {
        Some(text) => {
            let mut short = text.chars().take(MAX_BUTTON_LABEL).collect::<String>();
            if text.chars().count() > MAX_BUTTON_LABEL {
                short.push('…');
            }
            format!("[✎ \"{short}\"]")
        }
        None => "[✎]".to_owned(),
    };
    buttons.push((
        write_label,
        text.is_some(),
        LedgerAction::Write {
            endpoint_id: target.endpoint_id.clone(),
            pane_id: target.pane_id.to_owned(),
            item_id: target.item_id.to_owned(),
            question_id: question.id,
            current: text.unwrap_or_default(),
        },
    ));
    buttons.push((
        "[✕]".to_owned(),
        false,
        LedgerAction::Dismiss {
            endpoint_id: target.endpoint_id.clone(),
            pane_id: Some(target.pane_id.to_owned()),
            item_id: Some(target.item_id.to_owned()),
            question_id: Some(question.id),
        },
    ));
    let start = x + 2;
    let mut bx = start;
    for (label, chosen, action) in buttons {
        let w = display_width(&label).min(right.saturating_sub(start));
        if bx > start && bx + w > right {
            vy = vy.saturating_add(1);
            bx = start;
        }
        let style = if chosen {
            base.fg(super::panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            base.fg(palette.accent)
        };
        if let Some(rect) = canvas.put(bx, vy, w, &label, style) {
            hits.push((rect, action));
        }
        bx += w + 1;
    }
    vy.saturating_add(1)
}

/// A click on a card: its id opens the card (the `[[refs]]` command); the rest acts on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CardHit {
    Open(String),
    Action(LedgerAction),
}

/// Draws a card row from virtual row `vy` (`▸ SHE-12 title  ● ?2`, a ✕ on done cards that can
/// be dismissed) and, when `open`, its status, summary, decisions and questions. Returns the
/// next free row. The panel and the Cards view add their own lines after it.
#[allow(clippy::too_many_arguments)] // drawing context: the panel and the view share it
pub(super) fn render_card(
    canvas: &mut Canvas<'_>,
    x: u16,
    right: u16,
    mut vy: u16,
    row: &super::work_ledger_view::LedgerRow,
    open: bool,
    target: &QuestionTarget<'_>,
    palette: &Palette,
    base: Style,
    hits: &mut Vec<(Rect, CardHit)>,
) -> u16 {
    use super::work_ledger_view::LedgerRowState;
    let item = &row.item;
    // Inferred rows (the hook's guess) are dimmed and marked "~".
    let text_style = if item.inferred {
        base.fg(palette.overlay1)
    } else {
        base
    };
    let Some(y) = canvas.row(vy) else {
        return vy.saturating_add(1);
    };
    canvas.put(
        x,
        vy,
        1,
        if open { "▾" } else { "▸" },
        base.fg(palette.overlay1),
    );
    let mut cx = x + 2;
    let id = if item.inferred {
        format!("~{}", item.id)
    } else {
        item.id.clone()
    };
    let id_style = text_style
        .fg(if item.inferred {
            palette.overlay1
        } else {
            palette.accent
        })
        .add_modifier(Modifier::BOLD);
    if let Some(rect) = canvas.put(cx, vy, right.saturating_sub(cx), &id, id_style) {
        hits.push((rect, CardHit::Open(item.id.clone())));
        cx = rect.right() + 1;
    }
    let (glyph, color) = match row.state {
        LedgerRowState::Done => ("✓", palette.green),
        _ if item.inferred => ("●", palette.overlay1),
        _ => ("●", palette.accent),
    };
    let questions = if row.open_questions == 0 {
        String::new()
    } else {
        format!(" ?{}", row.open_questions)
    };
    let dismissable = target.answerable && row.state == LedgerRowState::Done;
    let marks_width = 1 + display_width(&questions) + if dismissable { 2 } else { 0 };
    let label_width = right.saturating_sub(cx).saturating_sub(marks_width + 1);
    canvas.put(cx, vy, label_width, &item.title, text_style);
    let mx = right.saturating_sub(marks_width);
    if mx > cx {
        canvas.put(mx, vy, 1, glyph, base.fg(color));
        canvas.put(
            mx + 1,
            vy,
            display_width(&questions),
            &questions,
            base.fg(palette.yellow),
        );
        if dismissable {
            if let Some(rect) = canvas.put(right - 1, vy, 1, "✕", base.fg(palette.overlay1)) {
                hits.push((
                    rect,
                    CardHit::Action(LedgerAction::Dismiss {
                        endpoint_id: target.endpoint_id.clone(),
                        pane_id: Some(target.pane_id.to_owned()),
                        item_id: Some(item.id.clone()),
                        question_id: None,
                    }),
                ));
            }
        }
    }
    // Pushed after the id's and ✕'s hits: a click anywhere else on the row toggles it.
    hits.push((
        Rect::new(x, y, right.saturating_sub(x), 1),
        CardHit::Action(LedgerAction::Toggle {
            endpoint_id: target.endpoint_id.clone(),
            pane_id: target.pane_id.to_owned(),
            item_id: item.id.clone(),
        }),
    ));
    vy = vy.saturating_add(1);
    if !open {
        return vy;
    }
    let dx = x + 2;
    let inner = right.saturating_sub(dx);
    if let Some(status) = &item.status {
        canvas.put(
            dx,
            vy,
            inner,
            &format!("· {status}"),
            base.fg(palette.overlay1),
        );
        vy = vy.saturating_add(1);
    }
    for line in super::work_panel::wrap(&item.summary, usize::from(inner))
        .into_iter()
        .take(3)
    {
        canvas.put(dx, vy, inner, &line, text_style);
        vy = vy.saturating_add(1);
    }
    for decision in &item.decisions {
        for (n, line) in
            super::work_panel::wrap(&decision.text, usize::from(inner.saturating_sub(2)))
                .into_iter()
                .take(2)
                .enumerate()
        {
            let bullet = if n == 0 { "✓ " } else { "  " };
            canvas.put(
                dx,
                vy,
                inner,
                &format!("{bullet}{line}"),
                base.fg(palette.green),
            );
            vy = vy.saturating_add(1);
        }
    }
    let mut actions = Vec::new();
    for question in crate::work_ledger::visible_questions(item) {
        vy = render_question(
            canvas,
            dx,
            right,
            vy,
            question,
            target,
            palette,
            base,
            &mut actions,
        );
    }
    hits.extend(
        actions
            .into_iter()
            .map(|(rect, action)| (rect, CardHit::Action(action))),
    );
    vy
}

impl ClientShellState {
    /// A ledger request to one machine: through the tracked lane on the active one, so a refusal
    /// (a busy dialog, an older server) shows a notice; fire-and-forget on the others, whose
    /// next push shows the outcome either way.
    fn ledger_request(
        &mut self,
        endpoint_id: &ClientEndpointId,
        method: crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) {
        if !self.endpoint_is_online(endpoint_id) {
            let label = self.endpoint_label(endpoint_id).to_owned();
            outcome.repaint |= self.receive_endpoint_unavailable(format!("{label} is not ready"));
            return;
        }
        if *endpoint_id == self.active_endpoint_id {
            self.push_endpoint_method(method, outcome);
        } else if let Some(action) = self.endpoint_request(endpoint_id, method) {
            outcome.actions.push(action);
        }
    }

    pub(super) fn run_ledger_action(
        &mut self,
        action: LedgerAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{
            LedgerDismissParams, LedgerSendParams, LedgerStageParams, Method,
        };
        match action {
            LedgerAction::Toggle {
                endpoint_id,
                pane_id,
                item_id,
            } => {
                let key = (endpoint_id, pane_id, item_id);
                if !self.expanded_ledger_items.remove(&key) {
                    self.expanded_ledger_items.insert(key);
                }
            }
            LedgerAction::Stage {
                endpoint_id,
                pane_id,
                item_id,
                question_id,
                answer,
            } => self.ledger_request(
                &endpoint_id,
                Method::LedgerStage(LedgerStageParams {
                    pane_id,
                    item_id,
                    question_id,
                    answer,
                }),
                outcome,
            ),
            LedgerAction::Write {
                endpoint_id,
                pane_id,
                item_id,
                question_id,
                current,
            } => {
                // The prompt takes the Cards view's place and gives it back when it closes.
                let cards = match self.overlay.take() {
                    Some(ClientShellOverlay::Cards(cards)) => Some(cards),
                    other => {
                        self.overlay = other;
                        None
                    }
                };
                self.prompt(
                    "answer (empty clears it)",
                    &current,
                    ClientRenameTarget::LedgerAnswer {
                        endpoint_id,
                        pane_id,
                        item_id,
                        question_id,
                        cards,
                    },
                );
            }
            LedgerAction::Dismiss {
                endpoint_id,
                pane_id,
                item_id,
                question_id,
            } => self.ledger_request(
                &endpoint_id,
                Method::LedgerDismiss(LedgerDismissParams {
                    pane_id,
                    item_id,
                    question_id,
                }),
                outcome,
            ),
            LedgerAction::Send {
                endpoint_id,
                pane_id,
            } => self.ledger_request(
                &endpoint_id,
                Method::LedgerSend(LedgerSendParams { pane_id }),
                outcome,
            ),
            LedgerAction::Focus {
                endpoint_id,
                pane_id,
            } => {
                if matches!(self.overlay, Some(ClientShellOverlay::Cards(_))) {
                    self.overlay = None;
                }
                self.focus_or_activate(
                    endpoint_id,
                    ClientEndpointFocusTarget::Pane(pane_id),
                    outcome,
                );
            }
        }
        outcome.repaint = true;
    }

    /// The free-text prompt closed: stage what was typed (empty unstages), then bring the Cards
    /// view back when the prompt came from it.
    pub(super) fn save_ledger_answer(
        &mut self,
        endpoint_id: ClientEndpointId,
        pane_id: String,
        item_id: String,
        question_id: u32,
        text: &str,
        outcome: &mut ClientShellInput,
    ) {
        let text = text.trim();
        let answer = (!text.is_empty()).then(|| LedgerAnswer {
            option: None,
            text: Some(text.to_owned()),
        });
        self.run_ledger_action(
            LedgerAction::Stage {
                endpoint_id,
                pane_id,
                item_id,
                question_id,
                answer,
            },
            outcome,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(staged: Option<LedgerAnswer>, sent: bool) -> LedgerQuestion {
        LedgerQuestion {
            id: 3,
            text: "Provider?".into(),
            options: vec!["Stripe".into(), "Multibanco".into()],
            recommended: Some(0),
            inferred: false,
            asked_at_ms: 1,
            answer: staged,
            sent_at_ms: sent.then_some(9),
            dismissed: false,
        }
    }

    fn draw(
        question: &LedgerQuestion,
        answerable: bool,
        width: u16,
    ) -> (Buffer, Vec<(Rect, LedgerAction)>, u16) {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 8));
        let mut hits = Vec::new();
        let endpoint_id = ClientEndpointId::Local;
        let target = QuestionTarget {
            endpoint_id: &endpoint_id,
            pane_id: "w1:p1",
            item_id: "SHE-12",
            answerable,
        };
        let palette = Palette::catppuccin();
        let next = {
            let mut canvas = Canvas {
                buffer: &mut buffer,
                top: 0,
                first: 0,
                bottom: 8,
            };
            render_question(
                &mut canvas,
                0,
                width,
                0,
                question,
                &target,
                &palette,
                Style::default(),
                &mut hits,
            )
        };
        (buffer, hits, next)
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn buttons_stage_unstage_write_and_dismiss() {
        let (buffer, hits, next) = draw(&question(None, false), true, 40);
        assert_eq!(row(&buffer, 1), "  [Stripe★] [Multibanco] [✎] [✕]");
        assert_eq!(next, 2);
        let actions = hits.iter().map(|(_, action)| action).collect::<Vec<_>>();
        assert!(matches!(
            actions[0],
            LedgerAction::Stage {
                question_id: 3,
                answer: Some(LedgerAnswer {
                    option: Some(0),
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(actions[2], LedgerAction::Write { .. }));
        assert!(matches!(
            actions[3],
            LedgerAction::Dismiss {
                question_id: Some(3),
                ..
            }
        ));
        // A click on the staged option unstages it.
        let staged = question(
            Some(LedgerAnswer {
                option: Some(1),
                text: None,
            }),
            false,
        );
        let (_, hits, _) = draw(&staged, true, 40);
        assert!(matches!(
            hits[1].1,
            LedgerAction::Stage { answer: None, .. }
        ));
        assert!(matches!(
            hits[0].1,
            LedgerAction::Stage {
                answer: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn buttons_wrap_inside_the_panel_and_hits_match_their_cells() {
        let (buffer, hits, next) = draw(&question(None, false), true, 20);
        assert_eq!(row(&buffer, 1), "  [Stripe★]");
        assert_eq!(row(&buffer, 2), "  [Multibanco] [✎]");
        assert_eq!(row(&buffer, 3), "  [✕]");
        assert_eq!(next, 4);
        assert!(hits.iter().all(|(rect, _)| rect.right() <= 20));
        assert_eq!(hits[1].0, Rect::new(2, 2, 12, 1));
    }

    #[test]
    fn sent_answers_wait_and_older_servers_get_numbered_options() {
        let sent = question(
            Some(LedgerAnswer {
                option: Some(1),
                text: None,
            }),
            true,
        );
        let (buffer, hits, _) = draw(&sent, true, 60);
        assert!(row(&buffer, 1).starts_with("  sent → Multibanco"));
        assert!(hits.is_empty());
        let (buffer, hits, _) = draw(&question(None, false), false, 40);
        assert_eq!(row(&buffer, 1), "  1) Stripe ★");
        assert!(hits.is_empty());
    }

    #[test]
    fn a_scrolled_canvas_draws_only_its_window() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 4));
        let mut canvas = Canvas {
            buffer: &mut buffer,
            top: 1,
            first: 5,
            bottom: 3,
        };
        assert_eq!(canvas.put(0, 4, 5, "above", Style::default()), None);
        assert_eq!(
            canvas.put(0, 6, 5, "shown", Style::default()),
            Some(Rect::new(0, 2, 5, 1))
        );
        assert_eq!(canvas.put(0, 7, 5, "below", Style::default()), None);
    }
}
