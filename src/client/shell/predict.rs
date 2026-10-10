//! andreconde fork (sheprd): predictive local echo for panes on another machine (SHE-100003 A),
//! after mosh. A printable key typed in the focused remote pane is drawn underlined at the cursor
//! right away; the server's echo confirms it, and a mismatch or a timeout erases the predictions.
//!
//! Safety, as in mosh: predictions are only shown once an echo in the current "epoch" was
//! confirmed (an epoch starts at Enter or any other non-printable key), so password prompts and
//! apps that don't echo never show ghost characters. Full-screen apps (Claude Code, vim) are
//! covered by the same rule: a key that doesn't echo at the cursor (vim normal mode) is never
//! confirmed, so nothing is shown. Never across a line wrap, and only when echo takes longer than
//! 30 ms.

use std::time::{Duration, Instant};

use crate::protocol::{
    ClientKeyCode, ClientKeyKind, ClientPaneInputEvent, FrameData, PaneSurfaceFrame,
};

const SHOW_ABOVE: Duration = Duration::from_millis(30);
const MIN_GIVE_UP: Duration = Duration::from_secs(1);

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Local echo on or off (sidebar.toml `local_echo_off`, the menu's "local echo" entry).
pub(super) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
    if !enabled {
        predictor().reset();
    }
}

pub(super) fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The client's one predictor (one focused pane at a time).
pub(super) fn predictor() -> std::sync::MutexGuard<'static, EchoPredictor> {
    static PREDICTOR: std::sync::OnceLock<std::sync::Mutex<EchoPredictor>> =
        std::sync::OnceLock::new();
    PREDICTOR
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug)]
struct Prediction {
    x: u16,
    y: u16,
    ch: char,
    at: Instant,
}

#[derive(Debug, Default)]
pub(super) struct EchoPredictor {
    pane_id: Option<String>,
    pending: Vec<Prediction>,
    /// Predicted cursor (surface coordinates) while predictions are pending.
    cursor: Option<(u16, u16)>,
    /// An echo was confirmed in this epoch.
    trusted: bool,
    /// Smoothed time from keypress to confirmed echo.
    rtt: Option<Duration>,
}

/// The single printable ASCII character a key event types, if that's all it does.
fn typed_char(event: &ClientPaneInputEvent) -> Option<char> {
    let text = match event {
        ClientPaneInputEvent::TextCommit(text) => text.as_str(),
        ClientPaneInputEvent::Key {
            kind: ClientKeyKind::Press | ClientKeyKind::Repeat,
            modifiers,
            generated_text: Some(text),
            ..
        } if *modifiers & !1 == 0 => text.as_str(),
        _ => return None,
    };
    let mut chars = text.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && (ch.is_ascii_graphic() || ch == ' ')).then_some(ch)
}

fn is_backspace(event: &ClientPaneInputEvent) -> bool {
    matches!(
        event,
        ClientPaneInputEvent::Key {
            code: ClientKeyCode::Backspace,
            kind: ClientKeyKind::Press | ClientKeyKind::Repeat,
            modifiers: 0,
            ..
        }
    )
}

/// Whether the server has echoed a predicted key: the cell shows it AND the server's cursor moved
/// past it. A matching cell alone is not enough: a space "matches" the blank cell before the
/// echo arrives, and dropping its prediction early put the next key in its place.
fn echoed(frame: &FrameData, prediction: &Prediction) -> bool {
    let shows = cell_symbol(frame, prediction.x, prediction.y).is_some_and(|symbol| {
        symbol == prediction.ch.encode_utf8(&mut [0; 4])
            || (prediction.ch == ' ' && symbol.is_empty())
    });
    let past = match frame.cursor.as_ref() {
        Some(cursor) => {
            cursor.y > prediction.y || (cursor.y == prediction.y && cursor.x > prediction.x)
        }
        // No cursor to go by: trust the cell, except for a space (indistinguishable from blank).
        None => prediction.ch != ' ',
    };
    shows && past
}

fn cell_symbol(frame: &FrameData, x: u16, y: u16) -> Option<&str> {
    if x >= frame.width || y >= frame.height {
        return None;
    }
    frame
        .cells
        .get(usize::from(y) * usize::from(frame.width) + usize::from(x))
        .map(|cell| cell.symbol.as_str())
}

impl EchoPredictor {
    /// Whether predictions are on screen now.
    pub(super) fn showing(&self) -> bool {
        self.trusted && !self.pending.is_empty() && self.rtt.is_some_and(|rtt| rtt >= SHOW_ABOVE)
    }

    /// Drops every prediction and starts a new epoch; true when that changes the screen.
    pub(super) fn reset(&mut self) -> bool {
        let showing = self.showing();
        self.pending.clear();
        self.cursor = None;
        self.trusted = false;
        showing
    }

    /// Records keys sent to `pane_id`. `remote` is false for panes on this machine, where echo is
    /// immediate. Returns true when the screen must be redrawn.
    pub(super) fn observe(
        &mut self,
        pane_id: &str,
        events: &[ClientPaneInputEvent],
        surface: Option<&PaneSurfaceFrame>,
        remote: bool,
        now: Instant,
    ) -> bool {
        let pane = surface.and_then(|surface| {
            surface
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .map(|pane| (surface, pane))
        });
        let Some((surface, pane)) = pane.filter(|(_, pane)| remote && pane.focused) else {
            return self.reset();
        };
        let mut repaint = false;
        if self.pane_id.as_deref() != Some(pane_id) {
            repaint |= self.reset();
            self.pane_id = Some(pane_id.to_owned());
        }
        let before = self.showing();
        for event in events {
            if let Some(ch) = typed_char(event) {
                let cursor = self.cursor.or_else(|| {
                    surface
                        .frame
                        .cursor
                        .as_ref()
                        .filter(|cursor| cursor.visible)
                        .map(|cursor| (cursor.x, cursor.y))
                });
                let inner = pane.inner_rect;
                let fits = cursor.filter(|(x, y)| {
                    *x >= inner.x
                        && x.saturating_add(1) < inner.x.saturating_add(inner.width)
                        && *y >= inner.y
                        && *y < inner.y.saturating_add(inner.height)
                });
                let Some((x, y)) = fits else {
                    repaint |= self.reset();
                    break;
                };
                self.pending.push(Prediction { x, y, ch, at: now });
                self.cursor = Some((x + 1, y));
            } else if is_backspace(event) && !self.pending.is_empty() {
                self.pending.pop();
                self.cursor = self.cursor.map(|(x, y)| (x.saturating_sub(1), y));
                if self.pending.is_empty() {
                    self.cursor = None;
                }
            } else {
                repaint |= self.reset();
            }
        }
        repaint || before || self.showing()
    }

    /// Checks pending predictions against the latest surface; true when the screen must be
    /// redrawn (predictions were or are shown).
    pub(super) fn confirm(&mut self, surface: Option<&PaneSurfaceFrame>, now: Instant) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let before = self.showing();
        let Some(frame) = surface.map(|surface| &surface.frame) else {
            return self.reset() || before;
        };
        while let Some(first) = self.pending.first() {
            if echoed(frame, first) {
                let sample = now.saturating_duration_since(first.at);
                self.rtt = Some(match self.rtt {
                    Some(rtt) => (rtt * 3 + sample) / 4,
                    None => sample,
                });
                self.trusted = true;
                self.pending.remove(0);
            } else {
                break;
            }
        }
        if self.pending.is_empty() {
            self.cursor = None;
        } else if self
            .pending
            .iter()
            .skip(1)
            .any(|later| echoed(frame, later))
        {
            // A later key was echoed but an earlier one was not: the app did something else.
            self.reset();
        }
        before || self.showing() || self.expire(now)
    }

    /// Drops predictions the server never echoed; true when that changes the screen.
    pub(super) fn expire(&mut self, now: Instant) -> bool {
        let give_up = self
            .rtt
            .map_or(MIN_GIVE_UP, |rtt| (rtt * 4).max(MIN_GIVE_UP));
        if self
            .pending
            .first()
            .is_some_and(|first| now.saturating_duration_since(first.at) > give_up)
        {
            return self.reset();
        }
        false
    }

    /// Draws the shown predictions over the composed frame; `origin` is the pane surface's
    /// position in the frame.
    pub(super) fn overlay(&self, frame: &mut FrameData, origin: (u16, u16)) {
        if !self.showing() {
            return;
        }
        let underline = ratatui::style::Modifier::UNDERLINED.bits();
        for prediction in &self.pending {
            let (x, y) = (origin.0 + prediction.x, origin.1 + prediction.y);
            if x >= frame.width || y >= frame.height {
                continue;
            }
            let index = usize::from(y) * usize::from(frame.width) + usize::from(x);
            if let Some(cell) = frame.cells.get_mut(index) {
                cell.symbol = prediction.ch.to_string();
                cell.modifier |= underline;
            }
        }
        if let (Some((x, y)), Some(cursor)) = (self.cursor, frame.cursor.as_mut()) {
            cursor.x = origin.0 + x;
            cursor.y = origin.1 + y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CellData, CursorState, PaneSurfacePane, SurfaceRect};

    fn cell(symbol: &str) -> CellData {
        CellData {
            symbol: symbol.into(),
            fg: 0,
            bg: 0,
            modifier: 0,
            skip: false,
            hyperlink: None,
        }
    }

    fn surface(text: &str, cursor_x: u16, alternate: bool) -> PaneSurfaceFrame {
        let width = 20;
        let mut cells = vec![cell(" "); usize::from(width) * 2];
        for (index, ch) in text.chars().enumerate() {
            cells[index] = cell(&ch.to_string());
        }
        let rect = SurfaceRect {
            x: 0,
            y: 0,
            width,
            height: 2,
        };
        PaneSurfaceFrame {
            boot_id: "boot".into(),
            projection_revision: 1,
            surface_revision: 1,
            frame: FrameData {
                width,
                height: 2,
                cells,
                cursor: Some(CursorState {
                    x: cursor_x,
                    y: 0,
                    visible: true,
                    shape: 0,
                }),
                hyperlinks: Vec::new(),
                graphics: Vec::new(),
            },
            panes: vec![PaneSurfacePane {
                pane_id: "p1".into(),
                content_revision: 1,
                rect,
                inner_rect: rect,
                scrollbar_rect: None,
                scroll: None,
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: alternate,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: Default::default(),
        }
    }

    fn key(text: &str) -> ClientPaneInputEvent {
        ClientPaneInputEvent::TextCommit(text.into())
    }

    fn enter() -> ClientPaneInputEvent {
        ClientPaneInputEvent::Key {
            code: ClientKeyCode::Enter,
            modifiers: 0,
            kind: ClientKeyKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: false,
            physical_key_id: None,
            windows_record: None,
        }
    }

    #[test]
    fn predictions_show_only_after_an_echo_in_the_epoch_and_on_slow_links() {
        let t0 = Instant::now();
        let mut echo = EchoPredictor::default();
        let prompt = surface("$ ", 2, false);
        echo.observe("p1", &[key("l")], Some(&prompt), true, t0);
        assert!(!echo.showing(), "nothing shown before an echo is confirmed");
        let echoed = surface("$ l", 3, false);
        echo.confirm(Some(&echoed), t0 + Duration::from_millis(80));
        assert!(echo.trusted);
        echo.observe(
            "p1",
            &[key("s")],
            Some(&echoed),
            true,
            t0 + Duration::from_millis(90),
        );
        assert!(echo.showing());
        let mut frame = echoed.frame.clone();
        echo.overlay(&mut frame, (0, 0));
        assert_eq!(frame.cells[3].symbol, "s");
        assert_eq!(frame.cursor.as_ref().map(|cursor| cursor.x), Some(4));
        // Enter starts a new epoch: the next line's keys wait for an echo again.
        echo.observe(
            "p1",
            &[enter()],
            Some(&echoed),
            true,
            t0 + Duration::from_millis(95),
        );
        echo.observe(
            "p1",
            &[key("x")],
            Some(&echoed),
            true,
            t0 + Duration::from_millis(96),
        );
        assert!(!echo.showing());
    }

    #[test]
    fn a_space_is_not_confirmed_by_the_blank_cell_before_its_echo() {
        let t0 = Instant::now();
        let mut echo = EchoPredictor::default();
        echo.observe("p1", &[key("a")], Some(&surface("$ ", 2, false)), true, t0);
        echo.confirm(
            Some(&surface("$ a", 3, false)),
            t0 + Duration::from_millis(60),
        );
        // Type " b" before the space is echoed: the cell under the space is blank already.
        let echoed_a = surface("$ a", 3, false);
        echo.observe(
            "p1",
            &[key(" "), key("b")],
            Some(&echoed_a),
            true,
            t0 + Duration::from_millis(70),
        );
        echo.confirm(Some(&echoed_a), t0 + Duration::from_millis(80));
        assert_eq!(
            echo.pending.len(),
            2,
            "the space stays predicted until the cursor moves"
        );
        let mut frame = echoed_a.frame.clone();
        echo.overlay(&mut frame, (0, 0));
        assert_eq!(
            frame.cells[4].symbol, "b",
            "b is drawn after the space, not on it"
        );
        // The server echoes the space: confirmed now, b still pending at its own cell.
        echo.confirm(
            Some(&surface("$ a ", 4, false)),
            t0 + Duration::from_millis(130),
        );
        assert_eq!(echo.pending.len(), 1);
        assert_eq!(echo.cursor, Some((5, 0)));
    }

    #[test]
    fn password_prompts_never_show_and_expire() {
        let t0 = Instant::now();
        let mut echo = EchoPredictor::default();
        let prompt = surface("Password: ", 10, false);
        for (index, ch) in ["h", "u", "n", "t", "e", "r", "2"].into_iter().enumerate() {
            echo.observe(
                "p1",
                &[key(ch)],
                Some(&prompt),
                true,
                t0 + Duration::from_millis(index as u64),
            );
            echo.confirm(Some(&prompt), t0 + Duration::from_millis(index as u64 + 1));
            assert!(!echo.showing());
        }
        echo.expire(t0 + Duration::from_secs(2));
        assert!(echo.pending.is_empty());
    }

    #[test]
    fn mismatch_local_pane_alternate_screen_and_fast_links_disable_it() {
        let t0 = Instant::now();
        let mut echo = EchoPredictor::default();
        let prompt = surface("$ ", 2, false);
        echo.observe("p1", &[key("a")], Some(&prompt), true, t0);
        echo.confirm(
            Some(&surface("$ a", 3, false)),
            t0 + Duration::from_millis(50),
        );
        echo.observe(
            "p1",
            &[key("b"), key("c")],
            Some(&surface("$ a", 3, false)),
            true,
            t0,
        );
        // The app echoed "c" where "b" was expected: everything is dropped.
        echo.confirm(
            Some(&surface("$ axc", 5, false)),
            t0 + Duration::from_millis(60),
        );
        assert!(echo.pending.is_empty() && !echo.trusted);

        echo.observe("p1", &[key("a")], Some(&prompt), false, t0);
        assert!(echo.pending.is_empty(), "local panes echo instantly");
        // Full-screen app where a key does not echo at the cursor (vim normal mode): recorded,
        // never confirmed, never shown.
        let mut vim = EchoPredictor::default();
        let normal_mode = surface("hello", 0, true);
        vim.observe("p1", &[key("j")], Some(&normal_mode), true, t0);
        vim.confirm(Some(&normal_mode), t0 + Duration::from_millis(70));
        vim.observe("p1", &[key("j")], Some(&normal_mode), true, t0);
        assert!(!vim.showing());

        let mut fast = EchoPredictor::default();
        fast.observe("p1", &[key("a")], Some(&prompt), true, t0);
        fast.confirm(
            Some(&surface("$ a", 3, false)),
            t0 + Duration::from_millis(5),
        );
        fast.observe("p1", &[key("b")], Some(&prompt), true, t0);
        assert!(!fast.showing(), "hidden when echo is faster than 30 ms");
    }

    #[test]
    fn backspace_takes_back_a_prediction_and_line_end_stops() {
        let t0 = Instant::now();
        let mut echo = EchoPredictor::default();
        let prompt = surface("$ ", 2, false);
        echo.observe("p1", &[key("a")], Some(&prompt), true, t0);
        echo.confirm(
            Some(&surface("$ a", 3, false)),
            t0 + Duration::from_millis(50),
        );
        let backspace = ClientPaneInputEvent::Key {
            code: ClientKeyCode::Backspace,
            modifiers: 0,
            kind: ClientKeyKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: false,
            physical_key_id: None,
            windows_record: None,
        };
        echo.observe(
            "p1",
            &[key("b"), key("c"), backspace],
            Some(&surface("$ a", 3, false)),
            true,
            t0,
        );
        assert_eq!(echo.pending.len(), 1);
        assert_eq!(echo.cursor, Some((4, 0)));

        let mut edge = EchoPredictor::default();
        edge.observe("p1", &[key("a")], Some(&surface("$ ", 19, false)), true, t0);
        assert!(
            edge.pending.is_empty(),
            "no prediction in the last column (wrap)"
        );
    }
}
