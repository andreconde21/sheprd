//! andreconde fork (sheprd): Ctrl+click opens anything (SHE-100007).
//!
//! - any file path: Markdown and code open in the editor split (at the line for `src/a.rs:42`),
//!   images and PDFs in the desktop viewer, fetched over SSH first when the pane is remote;
//! - `http://localhost:<port>` in a remote pane opens through an SSH tunnel to that machine;
//! - references (`[[refs]]` sources) open as before, with `{ssh}` / `{cwd}` of the pane;
//! - Ctrl+Alt+click (or Ctrl+Shift+click where the terminal passes it) copies instead;
//! - hint mode (prefix+i) labels everything openable on screen; type a label to open it,
//!   a capital label to copy it;
//! - holding Ctrl over a reference previews its title.

use super::*;
use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Something openable in a pane's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Thing {
    Url(String),
    Ref(String),
    File { path: String, line: Option<u32> },
}

impl Thing {
    /// What Ctrl+Alt+click copies.
    fn text(&self) -> String {
        match self {
            Thing::Url(url) => url.clone(),
            Thing::Ref(id) => id.clone(),
            Thing::File {
                path,
                line: Some(line),
            } => format!("{path}:{line}"),
            Thing::File { path, line: None } => path.clone(),
        }
    }
}

/// A [`Thing`] and the columns it spans (`end` exclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) thing: Thing,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    Document,
    Viewer,
}

const LEAD: &str = "\"'`([<{*|";
const TRAIL: &str = "\"'`)]>},;:.!?*|";

/// File extensions that count as a path even without a `/` (`plan.md`, `main.rs:12`).
const KNOWN_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "json",
    "toml", "yaml", "yml", "cs", "csproj", "sln", "razor", "cshtml", "go", "java", "kt", "kts",
    "swift", "c", "h", "cpp", "hpp", "rb", "php", "sh", "bash", "zsh", "fish", "sql", "html",
    "css", "scss", "vue", "svelte", "xml", "lock", "log", "ini", "cfg", "conf", "env", "csv",
    "tsv", "dart", "lua", "nix", "tf", "gradle", "props", "png", "jpg", "jpeg", "gif", "webp",
    "svg", "bmp", "avif", "ico", "tif", "tiff", "pdf",
];

const VIEWER_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "ico", "tif", "tiff", "pdf",
];

/// Everything openable in one row of text.
pub(super) fn things_in_line(chars: &[char], refs: &[regex::Regex]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        let mut start = index;
        while index < chars.len() && !chars[index].is_whitespace() {
            index += 1;
        }
        let mut end = index;
        while start < end && LEAD.contains(chars[start]) {
            start += 1;
        }
        while end > start && TRAIL.contains(chars[end - 1]) {
            end -= 1;
        }
        if start == end {
            continue;
        }
        let token: String = chars[start..end].iter().collect();
        if let Some(thing) = classify(&token, refs) {
            found.push(Found { thing, start, end });
        }
    }
    found
}

fn classify(token: &str, refs: &[regex::Regex]) -> Option<Thing> {
    if token.starts_with("http://") || token.starts_with("https://") {
        return Some(Thing::Url(token.to_owned()));
    }
    let is_ref = token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "#_-./".contains(c))
        && refs.iter().any(|pattern| pattern.is_match(token));
    if is_ref {
        return Some(Thing::Ref(token.to_owned()));
    }
    file_target(token).map(|(path, line)| Thing::File { path, line })
}

/// A file path with an optional `:line` (and `:col`, dropped), or None when the token doesn't look
/// like a path: it needs an extension, and a known one when there is no `/` (so `e.g.`,
/// `v0.9.3` and `example.com` stay plain text).
pub(super) fn file_target(token: &str) -> Option<(String, Option<u32>)> {
    let mut path = token.strip_prefix("file://").unwrap_or(token);
    if path.contains("://") {
        return None;
    }
    if let Some((before, _)) = path.split_once('#') {
        path = before;
    }
    let mut line = None;
    while let Some((before, after)) = path.rsplit_once(':') {
        if after.is_empty() || !after.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        line = after.parse::<u32>().ok().filter(|line| *line > 0);
        path = before;
    }
    if path.is_empty()
        || !path
            .chars()
            .all(|c| c.is_alphanumeric() || "_-./~+@%,".contains(c))
    {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let dot = name.rfind('.')?;
    let extension = &name[dot + 1..];
    if extension.is_empty()
        || extension.len() > 10
        || !extension.chars().all(|c| c.is_ascii_alphanumeric())
        || !extension.chars().any(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    let known = KNOWN_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str());
    if dot == 0 && !path.contains('/') {
        return None;
    }
    if !path.contains('/') && !known {
        return None;
    }
    Some((path.to_owned(), line))
}

fn file_kind(path: &str) -> FileKind {
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if VIEWER_EXTENSIONS.contains(&extension.as_str()) {
        FileKind::Viewer
    } else {
        FileKind::Document
    }
}

/// `(scheme, port, path-and-query)` of a URL whose host is this machine (`localhost`,
/// `127.0.0.1`, `0.0.0.0`, `[::1]`, `*.localhost`).
pub(super) fn local_url_parts(url: &str) -> Option<(String, u16, String)> {
    let (scheme, after) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let (authority, rest) = match after.find(['/', '?', '#']) {
        Some(index) => (&after[..index], &after[index..]),
        None => (after, ""),
    };
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, tail) = bracketed.split_once(']')?;
            (host, tail.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let host = host.to_ascii_lowercase();
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
        && !host.ends_with(".localhost")
    {
        return None;
    }
    let port = match port {
        Some(port) => port.parse().ok()?,
        None if scheme == "https" => 443,
        None => 80,
    };
    Some((scheme.to_owned(), port, rest.to_owned()))
}

/// Labels for `count` hints, all the same length so none is a prefix of another.
pub(super) fn hint_labels(count: usize) -> Vec<String> {
    const KEYS: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";
    if count <= KEYS.len() {
        return KEYS[..count]
            .iter()
            .map(|key| char::from(*key).to_string())
            .collect();
    }
    KEYS.iter()
        .flat_map(|first| {
            KEYS.iter()
                .map(move |second| format!("{}{}", char::from(*first), char::from(*second)))
        })
        .take(count)
        .collect()
}

#[derive(Clone, Debug)]
struct Hint {
    label: String,
    pane_id: String,
    x: u16,
    y: u16,
    width: u16,
    thing: Thing,
}

fn hints() -> &'static Mutex<Vec<Hint>> {
    static HINTS: OnceLock<Mutex<Vec<Hint>>> = OnceLock::new();
    HINTS.get_or_init(Default::default)
}

/// Length of the current hint labels: the prompt acts as soon as that many keys are typed.
pub(super) fn hint_label_len() -> usize {
    hints()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .first()
        .map_or(1, |hint| hint.label.len())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewAt {
    id: String,
    x: u16,
    y: u16,
}

#[derive(Default)]
struct PreviewState {
    at: Option<PreviewAt>,
    /// id -> one-line preview; None while loading or when there is nothing to show.
    cache: HashMap<String, Option<String>>,
    dirty: bool,
}

fn preview_state() -> &'static Mutex<PreviewState> {
    static PREVIEW: OnceLock<Mutex<PreviewState>> = OnceLock::new();
    PREVIEW.get_or_init(Default::default)
}

/// True once a preview finished loading since the last call (the client tick repaints).
pub(super) fn take_preview_dirty() -> bool {
    std::mem::take(
        &mut preview_state()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dirty,
    )
}

type Tunnels = HashMap<(String, u16), (u16, std::process::Child)>;

fn tunnels() -> &'static Mutex<Tunnels> {
    static TUNNELS: OnceLock<Mutex<Tunnels>> = OnceLock::new();
    TUNNELS.get_or_init(Default::default)
}

/// The remote port's own number when it is free here (cookies and OAuth callbacks keep working),
/// else any free port.
fn free_local_port(port: u16) -> Option<u16> {
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
        return Some(port);
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    Some(listener.local_addr().ok()?.port())
}

/// Forwards a local port to `localhost:<port>` on the SSH target (one tunnel per machine and
/// port, reused while it lives; it ends after 12 h) and opens the URL through it.
fn tunnel_and_open(target: String, scheme: String, port: u16, rest: String) -> Option<()> {
    let local = {
        let mut tunnels = tunnels().lock().unwrap_or_else(|e| e.into_inner());
        let key = (target.clone(), port);
        let alive = tunnels
            .get_mut(&key)
            .and_then(|(local, child)| matches!(child.try_wait(), Ok(None)).then_some(*local));
        match alive {
            Some(local) => local,
            None => {
                let local = free_local_port(port)?;
                let child = std::process::Command::new("ssh")
                    .args([
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=5",
                        "-o",
                        "ExitOnForwardFailure=yes",
                        "-o",
                        "ServerAliveInterval=30",
                        "-L",
                        &format!("127.0.0.1:{local}:localhost:{port}"),
                        &target,
                        "sleep 43200",
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok()?;
                tunnels.insert(key, (local, child));
                local
            }
        }
    };
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], local));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(200))
            .is_ok()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let url = format!("{scheme}://localhost:{local}{rest}");
    if let Ok(Some(mut child)) = crate::platform::open_url(&url) {
        let _ = child.wait();
    }
    Some(())
}

fn cache_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("sheprd")
        .join("open")
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{home}{rest}")
        }
        _ => path.to_owned(),
    }
}

impl ClientShellState {
    /// The visible text of a pane, one `Vec<char>` per row.
    fn pane_text(&self, pane_id: &str) -> Option<Vec<Vec<char>>> {
        let surface = self.pane_surface.as_ref()?;
        let pane = surface.panes.iter().find(|pane| pane.pane_id == pane_id)?;
        let frame = &surface.frame;
        let x = usize::from(pane.inner_rect.x);
        let width = usize::from(pane.inner_rect.width);
        let rows = (0..pane.inner_rect.height)
            .map(|row| usize::from(pane.inner_rect.y + row))
            .take_while(|y| *y < usize::from(frame.height))
            .map(|y| {
                let start = y * usize::from(frame.width) + x;
                frame
                    .cells
                    .get(start..start + width)
                    .map(|cells| {
                        cells
                            .iter()
                            .map(|cell| cell.symbol.chars().next().unwrap_or(' '))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        Some(rows)
    }

    /// The openable thing under (row, col) of a pane.
    pub(super) fn thing_at(&self, pane_id: &str, row: u16, col: u16) -> Option<Thing> {
        let rows = self.pane_text(pane_id)?;
        let line = rows.get(usize::from(row))?;
        let refs = super::projects::ref_patterns();
        things_in_line(line, &refs)
            .into_iter()
            .find(|found| (found.start..found.end).contains(&usize::from(col)))
            .map(|found| found.thing)
    }

    /// The folder a pane's shell is in, on its machine.
    pub(super) fn pane_cwd(&self, pane_id: &str) -> String {
        self.endpoint_by_id(&self.active_endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
            .and_then(|snapshot| snapshot.panes.iter().find(|pane| pane.pane_id == pane_id))
            .and_then(|pane| pane.foreground_cwd.clone().or_else(|| pane.cwd.clone()))
            .unwrap_or_else(|| "~".to_owned())
    }

    /// The SSH target of the active machine, None when it is this one.
    fn active_ssh_target(&self) -> Option<String> {
        if self.active_endpoint_id.is_local() {
            return None;
        }
        super::work_panel::ssh_target(self.endpoint_label(&self.active_endpoint_id))
    }

    fn ref_context(&self, pane_id: &str) -> super::projects::RefContext {
        super::projects::RefContext {
            ssh: self
                .active_ssh_target()
                .map(|target| format!("ssh -o BatchMode=yes -o ConnectTimeout=5 {target}"))
                .unwrap_or_default(),
            cwd: self.pane_cwd(pane_id),
        }
    }

    pub(super) fn open_thing(
        &mut self,
        pane_id: &str,
        thing: Thing,
        outcome: &mut ClientShellInput,
    ) {
        match thing {
            Thing::Url(url) => {
                let actions = self.web_url_actions(url);
                outcome.actions.extend(actions);
            }
            Thing::Ref(id) => {
                super::projects::open_task_in(id, Some(self.ref_context(pane_id)));
            }
            Thing::File { path, line } => match file_kind(&path) {
                FileKind::Document => self.open_document(pane_id, path, line),
                FileKind::Viewer => self.open_in_viewer(pane_id, path),
            },
        }
        outcome.repaint = true;
    }

    pub(super) fn copy_thing(&mut self, thing: &Thing, outcome: &mut ClientShellInput) {
        outcome
            .actions
            .push(ClientShellAction::ClipboardWrite(thing.text().into_bytes()));
        outcome.repaint |= self.show_copy_feedback(std::time::Instant::now());
    }

    /// Opening a web link: through an SSH tunnel when it points at the remote machine itself.
    pub(super) fn web_url_actions(&mut self, url: String) -> Vec<ClientShellAction> {
        let Some((scheme, port, rest)) = local_url_parts(&url) else {
            return vec![ClientShellAction::OpenSafeWebUrl(url)];
        };
        let Some(target) = self.active_ssh_target() else {
            return vec![ClientShellAction::OpenSafeWebUrl(url)];
        };
        let label = self.endpoint_label(&self.active_endpoint_id).to_owned();
        self.receive_endpoint_unavailable(format!("Opening {label}:{port} through SSH…"));
        std::thread::spawn(move || tunnel_and_open(target, scheme, port, rest));
        Vec::new()
    }

    /// Images and PDFs open in the desktop's viewer; a remote one is copied here first.
    fn open_in_viewer(&mut self, pane_id: &str, path: String) {
        let cwd = self.pane_cwd(pane_id);
        let Some(target) = self.active_ssh_target() else {
            let path = expand_home(&path);
            let full = if path.starts_with('/') {
                path
            } else {
                format!("{}/{path}", expand_home(&cwd))
            };
            std::thread::spawn(move || {
                if let Ok(Some(mut child)) = crate::platform::open_url(&full) {
                    let _ = child.wait();
                }
            });
            return;
        };
        let label = self.endpoint_label(&self.active_endpoint_id).to_owned();
        let remote = if path.starts_with('/') || path.starts_with('~') {
            path.clone()
        } else {
            format!("{cwd}/{path}")
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        self.receive_endpoint_unavailable(format!("Fetching {name} from {label}…"));
        let folder = cache_dir().join(label.replace(['/', ' '], "_"));
        std::thread::spawn(move || {
            if std::fs::create_dir_all(&folder).is_err() {
                return;
            }
            let local = folder.join(&name);
            let copied = std::process::Command::new("scp")
                .args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
                .arg(format!("{target}:{remote}"))
                .arg(&local)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if copied {
                if let Ok(Some(mut child)) = crate::platform::open_url(&local.to_string_lossy()) {
                    let _ = child.wait();
                }
            }
        });
    }

    /// prefix+i: label everything openable in the visible panes.
    pub(super) fn open_hints(&mut self) {
        let refs = super::projects::ref_patterns();
        let mut found = Vec::new();
        for hit in &self.hits.panes {
            let Some(rows) = self.pane_text(&hit.pane_id) else {
                continue;
            };
            for (row, line) in rows
                .iter()
                .enumerate()
                .take(usize::from(hit.inner_rect.height))
            {
                for item in things_in_line(line, &refs) {
                    found.push((hit.pane_id.clone(), hit.inner_rect, row as u16, item));
                }
            }
        }
        found.truncate(26 * 26);
        if found.is_empty() {
            self.receive_endpoint_unavailable("Nothing to open on screen".into());
            return;
        }
        let labels = hint_labels(found.len());
        *hints().lock().unwrap_or_else(|e| e.into_inner()) = found
            .into_iter()
            .zip(labels)
            .map(|((pane_id, rect, row, item), label)| Hint {
                label,
                pane_id,
                x: rect.x.saturating_add(item.start as u16),
                y: rect.y.saturating_add(row),
                width: (item.end - item.start) as u16,
                thing: item.thing,
            })
            .collect();
        self.prompt(
            "open: type a hint · capital letters copy · esc cancels",
            "",
            ClientRenameTarget::OpenHint,
        );
    }

    /// The hint prompt's answer: open the labelled thing, or copy it when typed in capitals.
    pub(super) fn activate_hint(&mut self, typed: &str, outcome: &mut ClientShellInput) {
        let copy = typed.chars().any(|c| c.is_uppercase());
        let label = typed.to_lowercase();
        let hint = hints()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|hint| hint.label == label)
            .cloned();
        let Some(hint) = hint else {
            return;
        };
        if copy {
            self.copy_thing(&hint.thing, outcome);
        } else {
            self.open_thing(&hint.pane_id, hint.thing, outcome);
        }
    }

    /// Ctrl held over a reference: remember where, and load its preview in the background.
    pub(super) fn update_ref_preview(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let at = (mouse.kind == MouseEventKind::Moved
            && mouse.modifiers.contains(KeyModifiers::CONTROL)
            && self.mode == ClientShellMode::Terminal
            && self.overlay.is_none())
        .then(|| {
            let hit = self
                .hits
                .panes
                .iter()
                .find(|hit| contains(hit.inner_rect, (mouse.column, mouse.row)))?;
            let row = mouse.row.saturating_sub(hit.inner_rect.y);
            let col = usize::from(mouse.column.saturating_sub(hit.inner_rect.x));
            let rows = self.pane_text(&hit.pane_id)?;
            let refs = super::projects::ref_patterns();
            let found = things_in_line(rows.get(usize::from(row))?, &refs)
                .into_iter()
                .find(|found| (found.start..found.end).contains(&col))?;
            let Thing::Ref(id) = found.thing else {
                return None;
            };
            Some((
                PreviewAt {
                    id,
                    x: hit.inner_rect.x.saturating_add(found.start as u16),
                    y: mouse.row,
                },
                hit.pane_id.clone(),
            ))
        })
        .flatten();
        let mut state = preview_state().lock().unwrap_or_else(|e| e.into_inner());
        let new_at = at.as_ref().map(|(at, _)| at.clone());
        if state.at != new_at {
            outcome.repaint = true;
            state.at = new_at;
        }
        let Some((at, pane_id)) = at else {
            return;
        };
        if state.cache.contains_key(&at.id) {
            return;
        }
        state.cache.insert(at.id.clone(), None);
        drop(state);
        let context = self.ref_context(&pane_id);
        let Some(command) = super::projects::ref_preview_command(&at.id, Some(&context)) else {
            return;
        };
        let id = at.id;
        std::thread::spawn(move || {
            let text = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
            let line = text.map(|text| {
                let (title, subtitle, _) = super::projects::reader_document(&id, &text);
                let line = [title.trim(), subtitle.trim()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                line.chars().take(100).collect::<String>()
            });
            let mut state = preview_state().lock().unwrap_or_else(|e| e.into_inner());
            state.cache.insert(id, line.filter(|line| !line.is_empty()));
            state.dirty = true;
        });
    }

    /// Draws hint labels (while the hint prompt is open) and the reference preview.
    pub(super) fn render_open_extras(
        &self,
        frame: &mut crate::protocol::FrameData,
        occlusion: &mut crate::kitty_graphics::surface::Occlusion,
    ) {
        let typed = match &self.overlay {
            Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                target: ClientRenameTarget::OpenHint,
                input,
                ..
            })) => Some(input.as_str().to_lowercase()),
            _ => None,
        };
        let preview = {
            let state = preview_state().lock().unwrap_or_else(|e| e.into_inner());
            state.at.clone().and_then(|at| {
                let line = state.cache.get(&at.id).cloned().flatten()?;
                Some((at, line))
            })
        };
        if typed.is_none() && preview.is_none() {
            return;
        }
        let Some(mut buffer) = frame.to_ratatui_buffer() else {
            return;
        };
        let area = buffer.area;
        let palette = &self.config.palette;
        let mut put = |buffer: &mut Buffer, x: u16, y: u16, text: &str, style: Style| {
            for (offset, ch) in text.chars().enumerate() {
                let x = x.saturating_add(offset as u16);
                if x >= area.width || y >= area.height {
                    break;
                }
                buffer[(x, y)].set_symbol(&ch.to_string()).set_style(style);
            }
            occlusion.cover(Rect::new(x, y, text.chars().count() as u16, 1).intersection(area));
        };
        if let Some(typed) = typed {
            let label_style = Style::default()
                .fg(palette.surface_dim)
                .bg(palette.yellow)
                .add_modifier(Modifier::BOLD);
            for hint in hints().lock().unwrap_or_else(|e| e.into_inner()).iter() {
                if !hint.label.starts_with(&typed) || hint.y >= area.height {
                    continue;
                }
                for col in hint.x..hint.x.saturating_add(hint.width).min(area.width) {
                    let cell = &mut buffer[(col, hint.y)];
                    let style = cell.style().add_modifier(Modifier::UNDERLINED);
                    cell.set_style(style);
                }
                put(&mut buffer, hint.x, hint.y, &hint.label, label_style);
            }
        }
        if let Some((at, line)) = preview {
            let text = format!(" {line} ");
            let y = if at.y > 0 { at.y - 1 } else { at.y + 1 };
            let width = text.chars().count() as u16;
            let x = at.x.min(area.width.saturating_sub(width));
            put(
                &mut buffer,
                x,
                y,
                &text,
                Style::default().fg(palette.text).bg(palette.panel_bg),
            );
        }
        let cursor = frame.cursor.clone();
        frame.replace_from_ratatui_buffer_preserving_effects(&buffer, cursor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn finds_paths_urls_and_refs() {
        let refs = vec![regex::Regex::new("^(?:SHE-\\d+)$").unwrap()];
        let found = things_in_line(
            &chars("see (src/main.rs:42) and https://x.dev/a, SHE-100007: e.g. v0.9.3 shot.png"),
            &refs,
        );
        let things: Vec<_> = found.iter().map(|found| found.thing.clone()).collect();
        assert_eq!(
            things,
            vec![
                Thing::File {
                    path: "src/main.rs".into(),
                    line: Some(42)
                },
                Thing::Url("https://x.dev/a".into()),
                Thing::Ref("SHE-100007".into()),
                Thing::File {
                    path: "shot.png".into(),
                    line: None
                },
            ]
        );
        assert_eq!((found[0].start, found[0].end), (5, 19));
    }

    #[test]
    fn plain_words_are_not_paths() {
        for token in [
            "e.g",
            "v0.9.3",
            "example.com",
            "README",
            "1.2",
            "github.com/a/b",
            "a=b.rs",
        ] {
            assert_eq!(file_target(token), None, "{token}");
        }
        assert_eq!(file_target("plan.md"), Some(("plan.md".into(), None)));
        assert_eq!(
            file_target("~/x/y.rs:3:9"),
            Some(("~/x/y.rs".into(), Some(3)))
        );
        assert_eq!(
            file_target("file:///abs/r.pdf"),
            Some(("/abs/r.pdf".into(), None))
        );
        assert_eq!(file_target("a/.env"), Some(("a/.env".into(), None)));
    }

    #[test]
    fn local_urls() {
        assert_eq!(
            local_url_parts("http://localhost:3000/a?b=1"),
            Some(("http".into(), 3000, "/a?b=1".into()))
        );
        assert_eq!(
            local_url_parts("http://127.0.0.1:5173"),
            Some(("http".into(), 5173, String::new()))
        );
        assert_eq!(
            local_url_parts("https://[::1]:8443/x"),
            Some(("https".into(), 8443, "/x".into()))
        );
        assert_eq!(
            local_url_parts("http://localhost/"),
            Some(("http".into(), 80, "/".into()))
        );
        assert_eq!(local_url_parts("https://example.com:3000/"), None);
    }

    #[test]
    fn hint_labels_are_prefix_free() {
        assert_eq!(hint_labels(3), vec!["a", "s", "d"]);
        let many = hint_labels(30);
        assert_eq!(many.len(), 30);
        assert!(many.iter().all(|label| label.len() == 2));
        assert_eq!(many[0], "aa");
    }

    #[test]
    fn file_kinds() {
        assert_eq!(file_kind("a/b.PNG"), FileKind::Viewer);
        assert_eq!(file_kind("r.pdf"), FileKind::Viewer);
        assert_eq!(file_kind("src/a.rs"), FileKind::Document);
    }
}
