//! andreconde fork (sheprd): the full-screen Cards view (SHE-100009). Every card every agent
//! reported, on every connected machine, grouped by machine and agent: filter by what waits on
//! you, answer and send per agent or all at once, dismiss done cards, click an agent to go to
//! it. prefix+shift+c or a click on the work panel's CARDS heading opens it; esc closes it.
//! Filters, scope and scroll are this client's; answers are the servers'.

use super::ledger_answers::{
    answers_supported, render_card, Canvas, CardHit, LedgerAction, QuestionTarget,
};
use super::render::{display_width, put_text};
use super::work_ledger_view::{LedgerRowState, PaneLedger};
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CardsFilter {
    #[default]
    Waiting,
    Active,
    Done,
    All,
}

impl CardsFilter {
    const ALL: [CardsFilter; 4] = [Self::Waiting, Self::Active, Self::Done, Self::All];

    fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting on you",
            Self::Active => "In progress",
            Self::Done => "Done",
            Self::All => "All",
        }
    }

    fn shows(self, state: LedgerRowState) -> bool {
        match self {
            Self::Waiting => state == LedgerRowState::Waiting,
            Self::Active => state == LedgerRowState::Active,
            Self::Done => state == LedgerRowState::Done,
            Self::All => true,
        }
    }

    pub(super) fn next(self, forward: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|filter| *filter == self)
            .unwrap_or(0);
        let len = Self::ALL.len();
        Self::ALL[if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        }]
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientCardsOverlay {
    pub(super) filter: CardsFilter,
    /// Every agent on every machine, or only the focused one.
    pub(super) all_agents: bool,
    pub(super) scroll: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CardsHit {
    Close,
    Filter(CardsFilter),
    AllAgents(bool),
    Card(String),
    Ledger(LedgerAction),
    /// One `ledger.send` / `ledger.dismiss` per machine, over the panes in view.
    SendAll,
    DismissAllDone,
}

pub(super) struct CardsRender {
    pub(super) area: Rect,
    pub(super) hits: Vec<(Rect, CardsHit)>,
    pub(super) max_scroll: u16,
}

/// One agent's cards as the view shows them.
pub(super) struct CardsPane<'a> {
    pub(super) endpoint: &'a ClientShellEndpoint,
    pub(super) pane_id: &'a str,
    pub(super) ledger: &'a PaneLedger,
    pub(super) answerable: bool,
}

/// The panes in view, machine by machine (in the sidebar's order) and agent by agent (in the
/// snapshot's order), for the scope; machines that are not online come back as `Err(label)`.
pub(super) fn panes_in_view<'a>(
    endpoints: &'a [ClientShellEndpoint],
    active: &ClientEndpointId,
    all_agents: bool,
) -> Vec<Result<Vec<CardsPane<'a>>, &'a ClientShellEndpoint>> {
    let mut groups = Vec::new();
    for endpoint in endpoints {
        let online = endpoint.status == ClientEndpointStatus::Online && endpoint.snapshot.is_some();
        let focused = endpoint
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.agents.iter().find(|agent| agent.focused))
            .map(|agent| agent.pane_id.as_str());
        if !all_agents && endpoint.endpoint_id != *active {
            continue;
        }
        if !online {
            if all_agents && !endpoint.endpoint_id.is_local() {
                groups.push(Err(endpoint));
            }
            continue;
        }
        let Some(view) = endpoint.work_ledger.as_ref() else {
            continue;
        };
        let answerable = answers_supported(endpoint);
        let order = endpoint
            .snapshot
            .as_deref()
            .map(|snapshot| {
                snapshot
                    .agents
                    .iter()
                    .map(|agent| agent.pane_id.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut panes = view
            .panes()
            .filter(|(pane_id, _)| all_agents || focused == Some(pane_id.as_str()))
            .map(|(pane_id, ledger)| CardsPane {
                endpoint,
                pane_id: pane_id.as_str(),
                ledger,
                answerable,
            })
            .collect::<Vec<_>>();
        panes.sort_by_key(|pane| {
            (
                order
                    .iter()
                    .position(|id| *id == pane.pane_id)
                    .unwrap_or(usize::MAX),
                pane.pane_id.to_owned(),
            )
        });
        if !panes.is_empty() {
            groups.push(Ok(panes));
        }
    }
    groups
}

pub(super) fn agent_label(endpoint: &ClientShellEndpoint, pane_id: &str) -> String {
    endpoint
        .snapshot
        .as_deref()
        .and_then(|snapshot| {
            snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane_id)
        })
        .and_then(|agent| {
            agent
                .name
                .as_deref()
                .or(agent.display_agent.as_deref())
                .or(agent.agent.as_deref())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "agent".to_owned())
}

pub(super) fn render(
    buffer: &mut Buffer,
    overlay: &ClientCardsOverlay,
    endpoints: &[ClientShellEndpoint],
    active: &ClientEndpointId,
    expanded: &HashSet<(ClientEndpointId, String, String)>,
    refusals: &super::ledger_answers::LedgerRefusals,
    palette: &Palette,
) -> Option<CardsRender> {
    let screen = buffer.area;
    for y in screen.y..screen.bottom() {
        for x in screen.x..screen.right() {
            let cell = &mut buffer[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
    let area = Rect::new(
        screen.x + 2.min(screen.width / 8),
        screen.y + 1.min(screen.height / 8),
        screen.width.saturating_sub(2 * 2.min(screen.width / 8)),
        screen.height.saturating_sub(2 * 1.min(screen.height / 8)),
    );
    if area.width < 30 || area.height < 8 {
        return None;
    }
    let base = Style::default()
        .fg(palette.text)
        .bg(palette.panel_bg)
        .remove_modifier(Modifier::DIM);
    // The same frame as the other overlays.
    for y in area.y..area.bottom() {
        put_text(
            buffer,
            area.x,
            y,
            area.width,
            &" ".repeat(usize::from(area.width)),
            base,
        );
    }
    let border = base.fg(palette.accent);
    for x in area.x..area.right() {
        let (top, bottom) = if x == area.x {
            ("┌", "└")
        } else if x + 1 == area.right() {
            ("┐", "┘")
        } else {
            ("─", "─")
        };
        put_text(buffer, x, area.y, 1, top, border);
        put_text(buffer, x, area.bottom() - 1, 1, bottom, border);
    }
    for y in area.y + 1..area.bottom() - 1 {
        put_text(buffer, area.x, y, 1, "│", border);
        put_text(buffer, area.right() - 1, y, 1, "│", border);
    }
    let inner = Rect::new(area.x + 2, area.y + 1, area.width - 4, area.height - 2);
    let mut hits = Vec::new();
    let strong = base.add_modifier(Modifier::BOLD);
    put_text(buffer, inner.x, inner.y, inner.width, "cards", strong);
    let close = " esc close ";
    let cw = display_width(close);
    let close_rect = Rect::new(inner.right().saturating_sub(cw), inner.y, cw, 1);
    put_text(
        buffer,
        close_rect.x,
        close_rect.y,
        cw,
        close,
        strong
            .fg(super::panel_contrast_fg(palette))
            .bg(palette.accent),
    );
    hits.push((close_rect, CardsHit::Close));

    let groups = panes_in_view(endpoints, active, overlay.all_agents);
    let in_view = groups.iter().flatten().flatten();
    let staged = in_view
        .clone()
        .filter(|pane| pane.answerable)
        .map(|pane| pane.ledger.staged)
        .sum::<usize>();
    let done = in_view
        .filter(|pane| pane.answerable)
        .map(|pane| pane.ledger.done)
        .sum::<usize>();

    // Filter chips, scope chips, and the actions over everything in view.
    let chip = |selected: bool| {
        if selected {
            strong
                .fg(super::panel_contrast_fg(palette))
                .bg(palette.accent)
        } else {
            base.fg(palette.overlay1)
        }
    };
    let cy = inner.y + 1;
    let mut cx = inner.x;
    let mut chips = CardsFilter::ALL
        .iter()
        .map(|filter| {
            (
                format!(" {} ", filter.label()),
                *filter == overlay.filter,
                CardsHit::Filter(*filter),
            )
        })
        .collect::<Vec<_>>();
    chips.push(("│".to_owned(), false, CardsHit::Close));
    chips.push((
        " This agent ".to_owned(),
        !overlay.all_agents,
        CardsHit::AllAgents(false),
    ));
    chips.push((
        " All agents ".to_owned(),
        overlay.all_agents,
        CardsHit::AllAgents(true),
    ));
    for (label, selected, hit) in chips {
        let w = display_width(&label);
        if cx + w > inner.right() {
            break;
        }
        if label == "│" {
            put_text(buffer, cx, cy, w, &label, base.fg(palette.overlay0));
        } else {
            put_text(buffer, cx, cy, w, &label, chip(selected));
            hits.push((Rect::new(cx, cy, w, 1), hit));
        }
        cx += w + 1;
    }
    let mut rx = inner.right();
    for (label, show, hit) in [
        (
            "[Dismiss all done]".to_owned(),
            done > 0,
            CardsHit::DismissAllDone,
        ),
        (
            format!("[Send all {staged}]"),
            staged > 0,
            CardsHit::SendAll,
        ),
    ] {
        let w = display_width(&label);
        if !show || rx < cx + w + 1 {
            continue;
        }
        rx -= w;
        put_text(buffer, rx, cy, w, &label, base.fg(palette.accent));
        hits.push((Rect::new(rx, cy, w, 1), hit));
        rx = rx.saturating_sub(1);
    }

    let body = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(3),
    );
    let mut rows = Vec::new();
    let vy = {
        let mut canvas = Canvas {
            buffer: &mut *buffer,
            top: body.y,
            first: overlay.scroll,
            bottom: body.bottom(),
        };
        let mut vy = 0u16;
        let right = body.right();
        for group in &groups {
            let panes = match group {
                Ok(panes) => panes,
                Err(endpoint) => {
                    canvas.put(
                        body.x,
                        vy,
                        body.width,
                        &endpoint.label,
                        strong.fg(palette.overlay1),
                    );
                    vy = vy.saturating_add(1);
                    canvas.put(
                        body.x + 2,
                        vy,
                        body.width.saturating_sub(2),
                        "not reachable: its cards come back when it reconnects",
                        base.fg(palette.overlay0),
                    );
                    vy = vy.saturating_add(2);
                    continue;
                }
            };
            let Some(first) = panes.first() else {
                continue;
            };
            let shown = panes
                .iter()
                .filter(|pane| {
                    pane.ledger
                        .rows
                        .iter()
                        .any(|row| overlay.filter.shows(row.state))
                })
                .collect::<Vec<_>>();
            if shown.is_empty() {
                continue;
            }
            let machine = if first.answerable {
                first.endpoint.label.clone()
            } else {
                format!(
                    "{} · read only: update sheprd there to answer from here",
                    first.endpoint.label
                )
            };
            canvas.put(
                body.x,
                vy,
                body.width,
                &machine,
                strong.fg(palette.overlay1),
            );
            vy = vy.saturating_add(1);
            for pane in shown {
                let endpoint_id = &pane.endpoint.endpoint_id;
                let mut header = format!(
                    "{} · {}",
                    agent_label(pane.endpoint, pane.pane_id),
                    pane.pane_id
                );
                if pane.ledger.open_questions > 0 {
                    header.push_str(&format!("  ?{}", pane.ledger.open_questions));
                }
                if let Some(rect) = canvas.put(
                    body.x + 1,
                    vy,
                    body.width.saturating_sub(1),
                    &header,
                    strong.fg(palette.accent),
                ) {
                    rows.push((
                        rect,
                        CardsHit::Ledger(LedgerAction::Focus {
                            endpoint_id: endpoint_id.clone(),
                            pane_id: pane.pane_id.to_owned(),
                        }),
                    ));
                }
                if pane.answerable && pane.ledger.staged > 0 {
                    let send = format!("[Send {}]", pane.ledger.staged);
                    let sw = display_width(&send);
                    if let Some(rect) = canvas.put(
                        right.saturating_sub(sw),
                        vy,
                        sw,
                        &send,
                        strong
                            .fg(super::panel_contrast_fg(palette))
                            .bg(palette.accent),
                    ) {
                        // Before the header's own hit, which spans the row.
                        rows.insert(
                            rows.len().saturating_sub(1),
                            (
                                rect,
                                CardsHit::Ledger(LedgerAction::Send {
                                    endpoint_id: endpoint_id.clone(),
                                    pane_id: Some(pane.pane_id.to_owned()),
                                }),
                            ),
                        );
                    }
                }
                vy = vy.saturating_add(1);
                if let Some(reason) = refusals
                    .get(&(endpoint_id.clone(), pane.pane_id.to_owned()))
                    .filter(|_| pane.answerable && pane.ledger.staged > 0)
                {
                    canvas.put(
                        body.x + 3,
                        vy,
                        body.width.saturating_sub(3),
                        &format!("! not sent: {reason}"),
                        base.fg(palette.red),
                    );
                    vy = vy.saturating_add(1);
                }
                let target = QuestionTarget {
                    endpoint_id,
                    pane_id: pane.pane_id,
                    item_id: "",
                    answerable: pane.answerable,
                };
                for row in pane
                    .ledger
                    .rows
                    .iter()
                    .filter(|row| overlay.filter.shows(row.state))
                {
                    let open = expanded.contains(&(
                        endpoint_id.clone(),
                        pane.pane_id.to_owned(),
                        row.item.id.clone(),
                    ));
                    let target = QuestionTarget {
                        item_id: &row.item.id,
                        ..target
                    };
                    let mut card_hits = Vec::new();
                    vy = render_card(
                        &mut canvas,
                        body.x + 2,
                        right,
                        vy,
                        row,
                        open,
                        &target,
                        palette,
                        base,
                        &mut card_hits,
                    );
                    rows.extend(card_hits.into_iter().map(|(rect, hit)| {
                        (
                            rect,
                            match hit {
                                CardHit::Open(id) => CardsHit::Card(id),
                                CardHit::Action(action) => CardsHit::Ledger(action),
                            },
                        )
                    }));
                }
                vy = vy.saturating_add(1);
            }
        }
        if vy == 0 {
            canvas.put(
                body.x,
                0,
                body.width,
                "No cards here. Try another filter, or All agents.",
                base.fg(palette.overlay0),
            );
            vy = 1;
        }
        vy
    };
    hits.extend(rows);
    Some(CardsRender {
        area,
        hits,
        max_scroll: vy.saturating_sub(body.height),
    })
}

impl ClientShellState {
    /// prefix+shift+c, or a click on CARDS: what waits on you when anything does, else all.
    pub(super) fn open_cards_view(&mut self) {
        let waiting = self.endpoints.iter().any(|endpoint| {
            endpoint
                .work_ledger
                .as_ref()
                .is_some_and(|view| view.panes().any(|(_, pane)| pane.open_questions > 0))
        });
        self.overlay = Some(ClientShellOverlay::Cards(ClientCardsOverlay {
            filter: if waiting {
                CardsFilter::Waiting
            } else {
                CardsFilter::All
            },
            all_agents: true,
            scroll: 0,
        }));
    }

    fn cards_overlay_mut(&mut self) -> Option<&mut ClientCardsOverlay> {
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::Cards(cards)) => Some(cards),
            _ => None,
        }
    }

    pub(super) fn scroll_cards_view(&mut self, delta: i32) {
        let max = self.hits.cards_max_scroll;
        if let Some(cards) = self.cards_overlay_mut() {
            cards.scroll = (i32::from(cards.scroll) + delta).clamp(0, i32::from(max)) as u16;
        }
    }

    /// One request per machine for the panes in view: send their staged answers, or dismiss
    /// their done cards. A machine with only some of its panes in view gets one per pane.
    fn cards_view_batch(&mut self, send: bool, outcome: &mut ClientShellInput) {
        let Some(all_agents) = self.cards_overlay_mut().map(|cards| cards.all_agents) else {
            return;
        };
        let mut actions = Vec::new();
        for group in panes_in_view(&self.endpoints, &self.active_endpoint_id, all_agents)
            .into_iter()
            .flatten()
        {
            let panes = group
                .into_iter()
                .filter(|pane| pane.answerable)
                .filter(|pane| {
                    if send {
                        pane.ledger.staged > 0
                    } else {
                        pane.ledger.done > 0
                    }
                })
                .collect::<Vec<_>>();
            let Some(first) = panes.first() else {
                continue;
            };
            let endpoint_id = first.endpoint.endpoint_id.clone();
            let targets = if all_agents {
                vec![None]
            } else {
                panes
                    .iter()
                    .map(|pane| Some(pane.pane_id.to_owned()))
                    .collect()
            };
            for pane_id in targets {
                actions.push(if send {
                    LedgerAction::Send {
                        endpoint_id: endpoint_id.clone(),
                        pane_id,
                    }
                } else {
                    LedgerAction::Dismiss {
                        endpoint_id: endpoint_id.clone(),
                        pane_id,
                        item_id: None,
                        question_id: None,
                    }
                });
            }
        }
        for action in actions {
            self.run_ledger_action(action, outcome);
        }
    }

    pub(super) fn handle_cards_click(&mut self, hit: CardsHit, outcome: &mut ClientShellInput) {
        match hit {
            CardsHit::Close => self.overlay = None,
            CardsHit::Filter(filter) => {
                if let Some(cards) = self.cards_overlay_mut() {
                    cards.filter = filter;
                    cards.scroll = 0;
                }
            }
            CardsHit::AllAgents(all) => {
                if let Some(cards) = self.cards_overlay_mut() {
                    cards.all_agents = all;
                    cards.scroll = 0;
                }
            }
            CardsHit::Card(id) => super::projects::open_task(id),
            CardsHit::Ledger(action) => self.run_ledger_action(action, outcome),
            CardsHit::SendAll => self.cards_view_batch(true, outcome),
            CardsHit::DismissAllDone => self.cards_view_batch(false, outcome),
        }
        outcome.repaint = true;
    }
}
