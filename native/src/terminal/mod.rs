//! Terminal tab: a session on `orbit_core::pty`, parsed by `alacritty_terminal`
//! on the PTY's output thread, painted as a grid of cells by a GPUI canvas
//! each frame. Parsing holds the program back (the core's read queue is
//! bounded), so a flooding program runs at the parser's pace and the UI only
//! ever waits for one small slice of parsing.
//! Mouse drags select text (double click: words, triple: lines) unless the
//! application asked for mouse reports, which Shift overrides.
mod diacritics;
mod graphics;
mod grid;
pub mod inhibit;
mod input;

pub use grid::{FRAMES, FRAME_US_MAX, FRAME_US_TOTAL};

use crate::scrollbar;
use crate::sessions::status::{self, Hook, Status, Tracker};
use crate::sessions::SessionMeta;
use crate::theme::{self, c, hsla};
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{cell::Flags, ClipboardType, Config, Term, TermMode},
    vte::ansi::{CursorShape, Processor, Rgb},
};
use futures::{channel::mpsc, StreamExt};
use gpui::{
    canvas, div, fill, outline, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, ElementInputHandler,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollEvent, SharedString, Subscription, Task, UTF16Selection, Window,
};
use gpui_component::scroll::ScrollbarHandle;
use grid::Geometry;
use input::{key_to_bytes, mouse_report, MouseReport};
use orbit_core::pty;
use std::{
    cell::Cell,
    ops::Range,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const LINE_HEIGHT: f32 = 17.;
const PADDING: f32 = 6.;

/// Self-test runs must not overwrite the user's clipboard: copies are only recorded.
pub static CLIPBOARD_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
pub struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Answers the terminal's queries (device attributes, colors, size) and
/// records the title and the clipboard writes (OSC 52) it asks for.
#[derive(Clone)]
struct Listener {
    pty_id: u32,
    title: Arc<Mutex<Option<String>>>,
    window_size: Arc<Mutex<WindowSize>>,
    clipboard: Arc<Mutex<Vec<(ClipboardType, String)>>>,
}

fn rgb_of(hex: u32) -> Rgb {
    let hex = theme::hex(hex);
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let reply = match event {
            TermEvent::PtyWrite(s) => s,
            TermEvent::Title(t) => {
                *self.title.lock().unwrap() = Some(t);
                return;
            }
            TermEvent::ResetTitle => {
                *self.title.lock().unwrap() = None;
                return;
            }
            TermEvent::ClipboardStore(ty, text) => {
                self.clipboard.lock().unwrap().push((ty, text));
                return;
            }
            TermEvent::TextAreaSizeRequest(f) => f(*self.window_size.lock().unwrap()),
            TermEvent::ColorRequest(ix, f) => f(rgb_of(match ix {
                0..=15 => theme::ANSI[ix],
                257 => theme::BG,
                258 => theme::ACCENT,
                _ => theme::TEXT,
            })),
            _ => return,
        };
        let _ = pty::write(self.pty_id, reply.as_bytes());
    }
}

pub struct Shared {
    term: FairMutex<Term<Listener>>,
    pty_id: u32,
    size: Mutex<GridSize>,
    window_size: Arc<Mutex<WindowSize>>,
    geometry: Mutex<Option<Geometry>>,
    graphics: Mutex<graphics::Store>,
}

impl Shared {
    /// Cell size in pixels, for pictures placed without a size.
    fn cell_px(&self) -> (f32, f32) {
        self.geometry.lock().unwrap().map_or((8., LINE_HEIGHT), |g| (f32::from(g.cell_w), f32::from(g.line_h)))
    }

    /// The cursor's cell in window coordinates, hidden or not: where input method text goes.
    fn caret(&self) -> Option<Bounds<Pixels>> {
        let g = (*self.geometry.lock().unwrap())?;
        let term = self.term.lock();
        let p = term.grid().cursor.point;
        let row = p.line.0 + term.grid().display_offset() as i32;
        Some(Bounds::new(g.origin + point(g.cell_w * p.column.0 as f32, g.line_h * row as f32), size(g.cell_w, g.line_h)))
    }

    /// Output through the kitty graphics filter; `reply` answers its queries.
    fn feed(&self, kitty: &mut graphics::Kitty, parser: &mut Processor, bytes: &[u8], reply: Option<graphics::Reply>) {
        let target = graphics::Target { term: &self.term, store: &self.graphics, cell: self.cell_px(), reply };
        kitty.feed(bytes, parser, &target);
    }
}

/// A picture the agent showed: path, caption, the time it came (HH:MM).
#[derive(Clone)]
struct Shown {
    path: std::path::PathBuf,
    title: Option<String>,
    at: String,
    /// The prompt it answers.
    prompt: Option<String>,
}

/// Follows the line typed into an agent's input; returns it on Enter. Escape sequences are skipped.
// shortcut: history (Up) and mid-line edits aren't followed; claude's hook gives its prompt exactly.
fn track_line(line: &mut String, bytes: &[u8]) -> Option<String> {
    let mut sent = None;
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => {
                // CSI: ESC [ params final; SS3/Alt+key: ESC and one char.
                if chars.next_if_eq(&'[').is_some() {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                } else {
                    chars.next();
                }
            }
            '\r' => {
                let done = std::mem::take(line).trim().to_string();
                if !done.is_empty() {
                    sent = Some(done);
                }
            }
            '\x7f' | '\x08' => {
                line.pop();
            }
            '\x15' | '\x03' => line.clear(),
            '\n' | '\t' => line.push(' '),
            c if !c.is_control() => line.push(c),
            _ => {}
        }
    }
    sent
}

/// Screen cells a link covers: (row, columns).
type LinkCells = Vec<(usize, Range<usize>)>;

/// The size the last painted terminal had: new sessions start at it, since
/// TUIs size their canvas from the first PTY size.
static LAST_SIZE: Mutex<Option<pty::Size>> = Mutex::new(None);

pub fn spawn_size() -> pty::Size {
    LAST_SIZE.lock().unwrap().unwrap_or(pty::Size { cols: 120, rows: 32, pixel_width: 0, pixel_height: 0 })
}

/// Output is parsed in slices this big, so a frame waits for at most one
/// slice's parse while a program floods the terminal.
const PARSE_SLICE: usize = 16 * 1024;

pub enum TerminalEvent {
    /// The agent status changed.
    StatusChanged,
    /// The agent finished a turn, or needs the user.
    Signal(Signal),
}

#[derive(Clone, Debug)]
pub enum Signal {
    Done,
    Waiting(Option<String>),
}

/// Agents without hooks: a stretch of activity this long ending in silence counts as "done".
const FALLBACK_DONE: Duration = Duration::from_secs(10);

/// The scrollback as a `ScrollbarHandle`: a line is a viewport's height over
/// its rows, so the thumb is rows / (scrollback + rows) of the track. The
/// alternate screen has no scrollback, so it shows no bar.
#[derive(Clone)]
struct ScrollbackHandle {
    shared: Arc<Shared>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl ScrollbackHandle {
    /// Scrollback lines, rows, display offset; no scrollback in the alternate screen.
    fn lines(&self) -> (usize, usize, usize) {
        let term = self.shared.term.lock();
        let grid = term.grid();
        let history = if term.mode().contains(TermMode::ALT_SCREEN) { 0 } else { grid.history_size() };
        (history, grid.screen_lines().max(1), grid.display_offset())
    }

    fn line_px(&self, rows: usize) -> Pixels {
        self.bounds.get().size.height / rows as f32
    }
}

impl ScrollbarHandle for ScrollbackHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    fn offset(&self) -> gpui::Point<Pixels> {
        let (history, rows, offset) = self.lines();
        point(px(0.), -self.line_px(rows) * history.saturating_sub(offset) as f32)
    }

    fn set_offset(&self, to: gpui::Point<Pixels>) {
        let (history, rows, offset) = self.lines();
        let from_top = ((-to.y / self.line_px(rows)).round().max(0.) as usize).min(history);
        let delta = (history - from_top) as i32 - offset as i32;
        if delta != 0 {
            self.shared.term.lock().scroll_display(Scroll::Delta(delta));
        }
    }

    fn content_size(&self) -> gpui::Size<Pixels> {
        let (history, rows, _) = self.lines();
        let b = self.bounds.get().size;
        size(b.width, self.line_px(rows) * (history + rows) as f32)
    }
}

pub struct TerminalView {
    shared: Arc<Shared>,
    pub meta: SessionMeta,
    tracker: Arc<Mutex<Tracker>>,
    pub status: Status,
    busy_since: Option<Instant>,
    title: Arc<Mutex<Option<String>>>,
    exited: Arc<AtomicBool>,
    focus: FocusHandle,
    scroll_remainder: f32,
    scrollbar: ScrollbackHandle,
    /// A selection drag is in progress.
    selecting: bool,
    /// The cell of the last motion report, so moving within a cell sends nothing.
    last_motion_cell: Option<(usize, usize)>,
    /// The last text copied (self-test dumps).
    pub last_copied: Option<String>,
    /// The finished selection's text, still copyable after the program redraws over it.
    selected: Option<String>,
    /// Clipboard writes the program asked for (OSC 52), from the output thread.
    clipboard: Arc<Mutex<Vec<(ClipboardType, String)>>>,
    /// An input method's uncommitted text (a dead key's accent), shown at the cursor.
    marked: Option<String>,
    _pump: Task<()>,
    _status_poll: Task<()>,
    _free_images: Subscription,
    /// Pictures the agent showed through Orbit's MCP `show_image`.
    shown: Vec<Shown>,
    /// The one in the viewer; none while it's closed.
    shown_ix: Option<usize>,
    /// Chat mode: the tab shows this instead of the terminal, which runs nothing.
    pub chat: Option<gpui::Entity<crate::chat::ChatView>>,
    _chat_status: Option<Subscription>,
    /// The line being typed and the last one sent (Enter), for the viewer's caption.
    prompt: std::cell::RefCell<(String, Option<String>)>,
    /// The cell under the mouse, for the link underline.
    hover: Option<(usize, usize)>,
    /// Ctrl or Super is down: a click opens the link under the mouse.
    link_mod: bool,
    /// The press opened a link; its release goes nowhere.
    link_click: bool,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = pty::kill(self.shared.pty_id);
    }
}

/// The http(s) or file URL that the character at `at` belongs to, if any,
/// and its char range. One char per cell, so a TUI's box borders and brackets end it too.
fn url_at(text: &[char], at: usize) -> Option<(String, Range<usize>)> {
    let stop = |c: char| c.is_whitespace() || "\"'<>`()[]{}│┃|".contains(c);
    if text.get(at).is_none_or(|&c| stop(c)) {
        return None;
    }
    let start = text[..at].iter().rposition(|&c| stop(c)).map_or(0, |i| i + 1);
    let end = text[at..].iter().position(|&c| stop(c)).map_or(text.len(), |i| at + i);
    let word: String = text[start..end].iter().collect();
    let scheme = ["https://", "http://", "file://"].iter().filter_map(|s| word.find(s)).min()?;
    let url = word[scheme..].trim_end_matches(['.', ',', ';', ':', '!', '?']);
    let (from, to) = (start + word[..scheme].chars().count(), start + word[..scheme].chars().count() + url.chars().count());
    (url.len() > "https://".len() && (from..to).contains(&at)).then(|| (url.to_string(), from..to))
}

/// Saved output made safe to replay above a new session: screen clears become
/// pushes into scrollback (so the replay survives them and the new session's
/// own clear), then a marker line.
pub fn restored_output(saved: &[u8], rows: usize) -> Vec<u8> {
    let push = "\r\n".repeat(rows).into_bytes();
    let mut out = Vec::with_capacity(saved.len() + 256);
    let mut rest = saved;
    while let Some(at) = rest.windows(4).position(|w| w == b"\x1b[2J") {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(&push);
        rest = &rest[at + 4..];
    }
    out.extend_from_slice(rest);
    out.extend_from_slice("\r\n\x1b[90m── restored after restart ──\x1b[0m".as_bytes());
    out.extend_from_slice(&push);
    out
}

impl TerminalView {
    /// Starts `spec` on a PTY (none for a session in chat mode, see `set_chat`).
    /// `replay` is printed first (a restored tab's previous output); `input` is typed once the program runs.
    pub fn new(meta: SessionMeta, spec: Option<pty::Spawn>, replay: Option<Vec<u8>>, input: Option<String>, cx: &mut Context<Self>) -> Self {
        let spawn_size = spec.as_ref().map_or_else(spawn_size, |s| s.size);
        let size = GridSize { cols: spawn_size.cols.max(2) as usize, rows: spawn_size.rows.max(1) as usize };
        let pty_id = pty::reserve_id();
        let title = Arc::new(Mutex::new(None));
        let window_size = Arc::new(Mutex::new(WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: 8,
            cell_height: LINE_HEIGHT as u16,
        }));
        let clipboard = Arc::new(Mutex::new(Vec::new()));
        let listener = Listener { pty_id, title: title.clone(), window_size: window_size.clone(), clipboard: clipboard.clone() };
        let term = Term::new(Config::default(), &size, listener);
        let shared = Arc::new(Shared {
            term: FairMutex::new(term),
            pty_id,
            size: Mutex::new(size),
            window_size,
            geometry: Mutex::new(None),
            graphics: Mutex::default(),
        });
        let mut parser: Processor = Processor::new();
        let mut kitty = graphics::Kitty::default();
        if let Some(saved) = replay.filter(|r| !r.is_empty()) {
            shared.feed(&mut kitty, &mut parser, &restored_output(&saved, size.rows), None);
            kitty = graphics::Kitty::default();
        }

        // The output thread parses everything; the UI only learns that
        // something changed (at most one pending wake-up) and repaints once
        // per frame, so bursts of output cost one render, not one per read.
        let (tx, mut rx) = mpsc::unbounded::<()>();
        let dirty = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let on_output = {
            let (shared, dirty, exited, tracker) = (shared.clone(), dirty.clone(), exited.clone(), tracker.clone());
            let reply = move |b: &[u8]| {
                let _ = pty::write(pty_id, b);
            };
            move |out: pty::Output| {
                match out {
                    pty::Output::Data(bytes) => {
                        for slice in bytes.chunks(PARSE_SLICE) {
                            shared.feed(&mut kitty, &mut parser, slice, Some(&reply));
                        }
                        let tail = &bytes[bytes.len().saturating_sub(status::TAIL_BYTES)..];
                        tracker.lock().unwrap().feed(&String::from_utf8_lossy(tail), Instant::now());
                    }
                    pty::Output::Exit => {
                        parser.advance(&mut *shared.term.lock(), b"\r\n\x1b[90m[process exited]\x1b[0m\r\n");
                        tracker.lock().unwrap().mark_exited();
                        exited.store(true, Ordering::Release);
                    }
                }
                if !dirty.swap(true, Ordering::AcqRel) {
                    let _ = tx.unbounded_send(());
                }
            }
        };
        if let Err(e) = spec.map_or(Ok(()), |spec| pty::spawn(pty_id, spec, on_output)) {
            let msg = format!("\x1b[31mcould not start: {e}\x1b[0m\r\n");
            Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut *shared.term.lock(), msg.as_bytes());
            tracker.lock().unwrap().mark_exited();
            exited.store(true, Ordering::Release);
        }
        let pump = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                dirty.store(false, Ordering::Release);
                if this.update(cx, |this, cx| this.refresh_status(cx)).is_err() {
                    break;
                }
            }
        });
        // Re-evaluates "idle" when output stops arriving.
        let status_poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            if this.update(cx, |this, cx| this.publish_status(cx)).is_err() {
                break;
            }
        });
        if let Some(input) = input {
            let _ = pty::write(pty_id, format!("{input}\r").as_bytes());
        }
        // Pictures leave the GPU atlas with the terminal.
        let free_images = cx.on_release(|this, cx| {
            let images = this.shared.graphics.lock().unwrap().take_all();
            for image in images {
                cx.drop_image(image, None);
            }
        });
        let focus = cx.focus_handle();
        inhibit::register(&focus);
        Self {
            scrollbar: ScrollbackHandle { shared: shared.clone(), bounds: Rc::default() },
            shared,
            meta,
            tracker,
            status: Status::Idle,
            busy_since: None,
            title,
            exited,
            focus,
            scroll_remainder: 0.,
            selecting: false,
            last_motion_cell: None,
            last_copied: None,
            selected: None,
            clipboard,
            marked: None,
            _pump: pump,
            _status_poll: status_poll,
            _free_images: free_images,
            shown: vec![],
            shown_ix: None,
            chat: None,
            _chat_status: None,
            prompt: Default::default(),
            hover: None,
            link_mod: false,
            link_click: false,
        }
    }

    /// Shows `chat` in place of the terminal; its status becomes the session's.
    pub fn set_chat(&mut self, chat: gpui::Entity<crate::chat::ChatView>, cx: &mut Context<Self>) {
        self._chat_status = Some(cx.observe(&chat, |this, _, cx| this.publish_status(cx)));
        self.chat = Some(chat);
    }

    pub fn busy(&self, cx: &App) -> bool {
        self.chat.as_ref().map_or(self.status == Status::Busy, |c| c.read(cx).busy())
    }

    /// Ends the program (or the chat's agent); `stopped` says when it is gone.
    pub fn stop(&self, cx: &App) {
        match &self.chat {
            Some(chat) => chat.read(cx).stop(),
            None => {
                let _ = pty::kill(self.shared.pty_id);
            }
        }
    }

    pub fn stopped(&self, cx: &App) -> bool {
        self.chat.as_ref().map_or_else(|| self.exited(), |c| c.read(cx).stopped())
    }

    pub fn is_agent(&self) -> bool {
        self.meta.cmd.is_some()
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let stores = std::mem::take(&mut *self.clipboard.lock().unwrap());
        for (ty, text) in stores {
            self.store(text, ty == ClipboardType::Selection, cx);
        }
        cx.notify();
        self.publish_status(cx);
    }

    /// Recomputes the status; on a change tells the session list, and for
    /// agents without hooks turns a long busy stretch ending in idle into "done".
    fn publish_status(&mut self, cx: &mut Context<Self>) {
        let (status, hooks) = match &self.chat {
            // The chat reports its own Done and Waiting to the inbox.
            Some(chat) => (chat.read(cx).status(), true),
            None => {
                let t = self.tracker.lock().unwrap();
                (t.status(Instant::now()), t.has_hooks())
            }
        };
        if self.is_agent() && !hooks {
            if status != Status::Idle && status != Status::Exited {
                self.busy_since.get_or_insert_with(Instant::now);
            } else if let Some(since) = self.busy_since.take() {
                if status == Status::Idle && since.elapsed() >= FALLBACK_DONE {
                    cx.emit(TerminalEvent::Signal(Signal::Done));
                }
            }
        }
        if status != self.status {
            self.status = status;
            cx.emit(TerminalEvent::StatusChanged);
            cx.notify();
        }
    }

    /// An `agent-event` from this session's Claude Code hooks.
    /// Opens the viewer on a picture the agent showed.
    pub fn show_image(&mut self, path: String, title: Option<String>, cx: &mut Context<Self>) {
        let at = chrono::Local::now().format("%H:%M").to_string();
        let prompt = self.prompt.borrow().1.clone();
        self.shown.push(Shown { path: path.into(), title, at, prompt });
        self.shown_ix = Some(self.shown.len() - 1);
        cx.notify();
    }

    /// The picture viewer, a panel beside the terminal (which narrows for it).
    fn viewer(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::pages::kit;
        let shown = self.shown[ix].clone();
        let caption = shown.title.clone().unwrap_or_else(|| shown.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
        let path = shown.path.clone();
        div()
            .id("shown-image")
            .w(gpui::relative(0.4))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .border_l_1()
            .border_color(c(theme::BORDER))
            .cursor_default()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .child(div().flex_1().truncate().text_color(c(theme::TEXT)).child(caption))
                    .child(div().text_color(c(theme::TEXT_DIM)).child(shown.at))
                    .child(div().id("shown-close").cursor_pointer().p_1().child(kit::icon("x", 12., c(theme::TEXT_DIM))).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.shown_ix = None;
                            cx.notify();
                        },
                    ))),
            )
            .when_some(shown.prompt.clone(), |d, prompt| {
                d.child(
                    div()
                        .px_2()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(c(theme::TEXT_DIM))
                        .line_clamp(2)
                        .child(format!("↳ {prompt}")),
                )
            })
            .child(
                div()
                    .id("shown-open")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| cx.open_with_system(&path))
                    .child(gpui::img(shown.path).size_full()),
            )
            .when(self.shown.len() > 1, |d| {
                d.child(
                    div()
                        .id("shown-strip")
                        .flex()
                        .gap_1()
                        .p_2()
                        .border_t_1()
                        .border_color(c(theme::BORDER))
                        .overflow_x_scroll()
                        .children(self.shown.iter().enumerate().map(|(i, s)| {
                            div()
                                .id(("shown-thumb", i))
                                .flex_none()
                                .w(px(72.))
                                .h(px(48.))
                                .p(px(2.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(c(if i == ix { theme::ACCENT } else { theme::BORDER }))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.shown_ix = Some(i);
                                    cx.notify();
                                }))
                                .child(gpui::img(s.path.clone()).size_full())
                        })),
                )
            })
    }

    /// Reopens a closed viewer: a small chip at the terminal's top right.
    fn viewer_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("shown-reopen")
            .absolute()
            .top(px(6.))
            .right(px(if self.meta.cmd.as_deref().is_some_and(pty::is_claude) { 90. } else { 16. }))
            .px_2()
            .py(px(2.))
            .rounded(px(4.))
            .bg(c(theme::PANEL_RAISED))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .text_size(px(11.))
            .text_color(c(theme::TEXT_DIM))
            .cursor_pointer()
            .occlude()
            .hover(|s| s.text_color(c(theme::TEXT)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.shown_ix = Some(this.shown.len() - 1);
                cx.notify();
            }))
            .child(format!("Images ({})", self.shown.len()))
    }

    pub fn on_hook(&mut self, hook: Hook, message: Option<String>, cx: &mut Context<Self>) {
        self.tracker.lock().unwrap().set_hook(hook);
        self.publish_status(cx);
        match hook {
            Hook::Done => cx.emit(TerminalEvent::Signal(Signal::Done)),
            Hook::Waiting => cx.emit(TerminalEvent::Signal(Signal::Waiting(message))),
            Hook::Working => {}
        }
    }

    /// The program's own title (OSC 0/2), if it set one.
    pub fn program_title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }

    pub fn title(&self) -> SharedString {
        let mut t = self.meta.session_name.clone();
        if self.is_agent() && self.status != Status::Idle && self.status != Status::Exited {
            t = format!("{t} · {}", self.status.label());
        }
        if self.exited() {
            t.push_str(" [exited]");
        }
        t.into()
    }

    /// The terminal device of a `tty_only` terminal (see `pty::Spawn`).
    pub fn tty_path(&self) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        return pty::tty_path(self.shared.pty_id);
        #[cfg(not(unix))]
        None
    }

    /// A `tty_only` terminal ends once its program has closed it.
    pub fn release_tty(&self) {
        pty::release_tty(self.shared.pty_id);
    }

    /// Types `bytes` into the program.
    pub fn send(&self, bytes: &[u8]) {
        let _ = pty::write(self.shared.pty_id, bytes);
    }

    /// The visible grid as text, one line per row (self-test dumps).
    pub fn grid_text(&self) -> String {
        let term = self.shared.term.lock();
        let mut out = String::new();
        let mut row = None;
        for cell in term.renderable_content().display_iter {
            if row != Some(cell.point.line) {
                if row.is_some() {
                    let trimmed = out.trim_end_matches(' ').len();
                    out.truncate(trimmed);
                    out.push('\n');
                }
                row = Some(cell.point.line);
            }
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                out.push(cell.c);
            }
        }
        out
    }

    /// Kitty graphics held by this terminal (self-test dumps).
    pub fn graphics_summary(&self) -> String {
        self.shared.graphics.lock().unwrap().summary()
    }

    pub fn selection_text(&self) -> Option<String> {
        self.shared.term.lock().selection_to_string()
    }

    /// Window position of a viewport cell's center, once painted (self-test).
    pub fn cell_position(&self, col: usize, row: usize) -> Option<gpui::Point<gpui::Pixels>> {
        self.shared.geometry.lock().unwrap().map(|g| g.cell_center(col, row))
    }

    /// The scrollback position and whether the scrollbar has anything to show (self-test dumps).
    pub fn scroll_summary(&self) -> String {
        let (history, rows, offset) = self.scrollbar.lines();
        let alt = self.shared.term.lock().mode().contains(TermMode::ALT_SCREEN);
        format!("scroll: display offset {offset} of {history}, rows {rows}, alt screen {alt}, scrollbar {}", history > 0)
    }

    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.shared.term.lock().scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    fn write(&self, bytes: &[u8]) {
        if self.meta.cmd.is_some() {
            let mut p = self.prompt.borrow_mut();
            if let Some(line) = track_line(&mut p.0, bytes) {
                p.1 = Some(line);
            }
        }
        self.send(bytes);
    }

    /// The prompt claude's `UserPromptSubmit` hook reports, over the typed one.
    pub fn set_prompt(&self, prompt: String) {
        self.prompt.borrow_mut().1 = Some(prompt);
    }

    fn paste_text(&self, text: &str) {
        let bracketed = self.shared.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.write(format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).as_bytes());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
    }

    /// False when nothing is selected.
    fn copy(&mut self, primary: bool, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selection_text().filter(|t| !t.is_empty()).or_else(|| self.selected.clone()) else { return false };
        self.store(text, primary, cx);
        true
    }

    fn store(&mut self, text: String, primary: bool, cx: &mut Context<Self>) {
        if !CLIPBOARD_DISABLED.load(Ordering::Relaxed) {
            let item = ClipboardItem::new_string(text.clone());
            match primary {
                // The primary selection exists on Linux only.
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                true => cx.write_to_primary(item),
                #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                true => {}
                false => cx.write_to_clipboard(item),
            }
        }
        self.last_copied = Some(text);
    }

    /// Pastes the clipboard's text; with an image only, sends ^V, which is how
    /// Claude Code and omp learn to read the clipboard themselves.
    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        match cx.read_from_clipboard().and_then(|c| c.text()) {
            Some(text) => self.paste_text(&text),
            None => self.write(b"\x16"),
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        // A switcher press that beat the inhibitor's release (Alt and Tab at once, as
        // Toshy's Cmd+Tab sends them): the next Tab reaches GNOME.
        if m.alt && (ks.key == "tab" || ks.key == "`") && inhibit::releases_for_alt() {
            inhibit::set_alt(true, window, cx);
            cx.stop_propagation();
            return;
        }
        if m.control && m.shift && ks.key == "c" {
            self.copy(false, cx);
            cx.stop_propagation();
            return;
        }
        // Cmd+C under Toshy arrives as Ctrl+C: with a selection it copies and drops it, so the next one interrupts.
        if cfg!(target_os = "linux") && m.control && !m.alt && ks.key == "c" && self.copy(false, cx) {
            self.shared.term.lock().selection = None;
            self.selected = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        // Linux with macOS-style remapping (Toshy, Kinto): outside terminals the
        // physical Ctrl key arrives as Super, so Super+letter means Ctrl+letter.
        if cfg!(target_os = "linux") && m.platform && !m.control && !m.alt && ks.key.len() == 1 && ks.key.as_bytes()[0].is_ascii_alphabetic() {
            self.write(&[ks.key.as_bytes()[0].to_ascii_lowercase() - b'a' + 1]);
            cx.stop_propagation();
            return;
        }
        // macOS: Cmd+C copies the selection and Cmd+V pastes, as in Terminal.app.
        if cfg!(target_os = "macos") && m.platform && !m.control && !m.alt && !m.shift && (ks.key == "c" || ks.key == "v") {
            if ks.key == "c" {
                self.copy(false, cx);
            } else {
                self.paste_clipboard(cx);
            }
            cx.stop_propagation();
            return;
        }
        // Ctrl+V pastes like Ctrl+Shift+V (Cmd+V under Toshy arrives as Ctrl+V).
        if m.control && ks.key == "v" && !m.alt || m.shift && ks.key == "insert" {
            self.paste_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        if m.shift && (ks.key == "pageup" || ks.key == "pagedown") {
            let scroll = if ks.key == "pageup" { Scroll::PageUp } else { Scroll::PageDown };
            self.shared.term.lock().scroll_display(scroll);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let app_cursor = self.shared.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = key_to_bytes(ks, app_cursor) {
            self.type_bytes(&bytes);
            cx.stop_propagation();
        }
    }

    /// Input from the keyboard: back to the bottom, the selection dropped.
    fn type_bytes(&mut self, bytes: &[u8]) {
        {
            let mut term = self.shared.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.selected = None;
        self.write(bytes);
    }

    /// The application's mouse mode, unless Shift asks for selection instead.
    fn reporting(&self, shift: bool) -> Option<TermMode> {
        let mode = *self.shared.term.lock().mode();
        (mode.intersects(TermMode::MOUSE_MODE) && !shift).then_some(mode)
    }

    fn cell_at(&self, pos: gpui::Point<gpui::Pixels>) -> Option<(usize, usize, bool)> {
        self.shared.geometry.lock().unwrap().map(|g| g.cell_at(pos))
    }

    fn grid_point(&self, col: usize, row: usize) -> GridPoint {
        let offset = self.shared.term.lock().grid().display_offset() as i32;
        GridPoint::new(Line(row as i32 - offset), Column(col))
    }

    /// The link under a screen cell and the cells it covers: an OSC 8 hyperlink, else a URL in its wrapped line.
    fn link_at(&self, col: usize, row: usize) -> Option<(String, LinkCells)> {
        let term = self.shared.term.lock();
        let grid = term.grid();
        let offset = grid.display_offset() as i32;
        let line = Line(row as i32 - offset);
        let cols = grid.columns();
        if line < grid.topmost_line() || line > grid.bottommost_line() || col >= cols {
            return None;
        }
        if let Some(link) = grid[line][Column(col)].hyperlink() {
            let same = |c: usize| grid[line][Column(c)].hyperlink().as_ref() == Some(&link);
            let start = (0..col).rev().take_while(|&c| same(c)).last().unwrap_or(col);
            let end = (col..cols).take_while(|&c| same(c)).last().unwrap_or(col) + 1;
            return Some((link.uri().to_string(), vec![(row, start..end)]));
        }
        let wraps = |l: Line| grid[l][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let (mut start, mut end) = (line, line);
        while start > grid.topmost_line() && wraps(start - 1) {
            start -= 1;
        }
        while end < grid.bottommost_line() && wraps(end) {
            end += 1;
        }
        let text: Vec<char> = (start.0..=end.0).flat_map(|l| (0..cols).map(move |c| grid[Line(l)][Column(c)].c)).collect();
        let (url, chars) = url_at(&text, (line.0 - start.0) as usize * cols + col)?;
        // Chars back to screen rows; rows scrolled out of view are left out.
        let first = start.0 + offset;
        let cells = (chars.start / cols..=(chars.end - 1) / cols)
            .filter_map(|r| {
                let screen = usize::try_from(first + r as i32).ok()?;
                let from = if r == chars.start / cols { chars.start % cols } else { 0 };
                let to = if r == (chars.end - 1) / cols { (chars.end - 1) % cols + 1 } else { cols };
                Some((screen, from..to))
            })
            .collect();
        Some((url, cells))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((col, row, right_half)) = self.cell_at(ev.position) else { return };
        // Ctrl+click opens a link; Super too, which is Ctrl under Toshy and Cmd on macOS.
        if ev.button == MouseButton::Left && (ev.modifiers.control || ev.modifiers.platform) {
            if let Some((url, _)) = self.link_at(col, row) {
                cx.open_url(&url);
                // Its release isn't reported either: a TUI that opens links itself would open it again.
                self.link_click = true;
                return;
            }
        }
        if let Some(mode) = self.reporting(ev.modifiers.shift) {
            if let Some(bytes) = mouse_report(mode, MouseReport::Press(ev.button), col, row, ev.modifiers) {
                self.write(&bytes);
            }
            return;
        }
        match ev.button {
            MouseButton::Left => {
                let ty = match ev.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                let side = if right_half { Side::Right } else { Side::Left };
                let point = self.grid_point(col, row);
                self.shared.term.lock().selection = Some(Selection::new(ty, point, side));
                self.selected = None;
                self.selecting = true;
                cx.notify();
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            MouseButton::Middle => {
                if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
                    self.paste_text(&text);
                }
            }
            _ => {}
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((col, row, right_half)) = self.cell_at(ev.position) else { return };
        let link_mod = ev.modifiers.control || ev.modifiers.platform;
        if self.hover != Some((col, row)) || self.link_mod != link_mod {
            (self.hover, self.link_mod) = (Some((col, row)), link_mod);
            cx.notify();
        }
        // A Shift+drag selection keeps selecting once Shift is let go.
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            let side = if right_half { Side::Right } else { Side::Left };
            let point = self.grid_point(col, row);
            if let Some(sel) = self.shared.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
            return;
        }
        if let Some(mode) = self.reporting(ev.modifiers.shift) {
            if self.last_motion_cell.replace((col, row)) == Some((col, row)) {
                return;
            }
            if let Some(bytes) = mouse_report(mode, MouseReport::Motion(ev.pressed_button), col, row, ev.modifiers) {
                self.write(&bytes);
            }
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.link_click) {
            return;
        }
        if let Some(mode) = self.reporting(ev.modifiers.shift).filter(|_| !self.selecting) {
            if let Some((col, row, _)) = self.cell_at(ev.position) {
                if let Some(bytes) = mouse_report(mode, MouseReport::Release(ev.button), col, row, ev.modifiers) {
                    self.write(&bytes);
                }
            }
            return;
        }
        if self.selecting && ev.button == MouseButton::Left {
            self.selecting = false;
            // A plain click leaves an empty selection behind: drop it.
            let empty = self.shared.term.lock().selection.as_ref().is_none_or(|s| s.is_empty());
            if empty {
                self.shared.term.lock().selection = None;
            } else {
                self.selected = self.selection_text();
                // X11/Wayland convention: a finished selection goes to the primary clipboard.
                self.copy(true, cx);
            }
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_remainder += f32::from(ev.delta.pixel_delta(px(LINE_HEIGHT)).y) / LINE_HEIGHT;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;
        if let Some(mode) = self.reporting(ev.modifiers.shift) {
            let (col, row, _) = self.cell_at(ev.position).unwrap_or((0, 0, false));
            let report = if lines > 0 { MouseReport::WheelUp } else { MouseReport::WheelDown };
            for _ in 0..lines.unsigned_abs().min(10) {
                if let Some(bytes) = mouse_report(mode, report, col, row, ev.modifiers) {
                    self.write(&bytes);
                }
            }
            return;
        }
        let mut term = self.shared.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN) {
            // Full-screen apps without mouse mode get arrow keys for the wheel.
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            drop(term);
            for _ in 0..lines.unsigned_abs().min(10) {
                self.write(seq);
            }
        } else {
            term.scroll_display(Scroll::Delta(lines));
            drop(term);
            cx.notify();
        }
    }
}

/// Typed text and dead-key or input method compositions, as in Zed's terminal.
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        // Full-screen programs get no text field, so macOS repeats held keys instead of offering accents.
        let alt_screen = self.shared.term.lock().mode().contains(TermMode::ALT_SCREEN);
        (!alt_screen).then_some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if !text.is_empty() {
            self.type_bytes(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        self.shared.caret()
    }

    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl TerminalView {
    fn mode_chip(&self) -> Option<gpui::Stateful<gpui::Div>> {
        let label = if self.chat.is_some() { "Terminal" } else { "Chat" };
        self.meta.cmd.as_deref().is_some_and(pty::is_claude).then(|| {
            div()
                .id("mode-toggle")
                .relative()
                .px_2()
                .py(px(2.))
                .rounded(px(4.))
                .bg(c(theme::PANEL_RAISED))
                .border_1()
                .border_color(c(theme::BORDER_STRONG))
                .text_size(px(11.))
                .text_color(c(theme::TEXT_DIM))
                .cursor_pointer()
                .occlude()
                .hover(|s| s.text_color(c(theme::TEXT)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::sessions::ToggleChatMode), cx))
                .child(label)
                .child(crate::probe::probe("mode-toggle"))
        })
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(chat) = &self.chat {
            return div()
                .track_focus(&self.focus)
                .flex()
                .flex_col()
                .size_full()
                .bg(c(theme::BG))
                .font_family(theme::UI_FONT)
                .child(div().flex().justify_end().px_4().pt_1p5().children(self.mode_chip()))
                .child(div().flex_1().min_h_0().child(chat.clone()))
                .into_any_element();
        }
        let shared = self.shared.clone();
        let focused = self.focus.is_focused(window);
        let viewport = self.scrollbar.bounds.clone();
        let (focus, view, marked, painted) = (self.focus.clone(), cx.entity(), self.marked.clone(), self.shared.clone());
        let link = self.hover.and_then(|(col, row)| self.link_at(col, row)).map(|(_, cells)| cells);
        let pointer = link.is_some() && self.link_mod;
        let terminal = div()
            .id("terminal")
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(c(theme::BG))
            .when(pointer, |d| d.cursor_pointer())
            .when(!pointer, |d| d.cursor_text())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, window, cx| {
                let m = ev.modifiers;
                // Cmd+Shift (Ctrl+Shift under Toshy) also lets go: Toshy's Cmd+Shift+3/4/5 become GNOME's Print screenshots.
                inhibit::set_alt(m.alt || m.control && m.shift, window, cx);
                if this.link_mod != (m.control || m.platform) {
                    this.link_mod = m.control || m.platform;
                    cx.notify();
                }
            }))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .capture_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if ev.button == MouseButton::Left && scrollbar::page(&this.scrollbar, ev.position) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .child(
                canvas(
                    move |bounds, window, _cx| {
                        viewport.set(bounds);
                        (std::time::Instant::now(), grid::layout(&shared, bounds, window))
                    },
                    move |bounds, (started, frame), window, cx| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            for (b, color) in &frame.rects {
                                window.paint_quad(fill(*b, *color));
                            }
                            for (clip, image_bounds, image) in &frame.images {
                                let _ = window.paint_image(*clip, *image_bounds, Default::default(), image.clone(), 0, false);
                            }
                            if let Some((b, shape)) = frame.cursor {
                                let accent = hsla(theme::ACCENT);
                                let quad = match (focused, shape) {
                                    (true, CursorShape::Beam) => fill(Bounds::new(b.origin, size(px(2.), b.size.height)), accent),
                                    (true, CursorShape::Underline) => fill(
                                        Bounds::new(b.origin + point(px(0.), b.size.height - px(2.)), size(b.size.width, px(2.))),
                                        accent,
                                    ),
                                    (true, _) => fill(b, accent),
                                    (false, _) => outline(b, accent, gpui::BorderStyle::Solid),
                                };
                                window.paint_quad(quad);
                            }
                            for (origin, line) in &frame.lines {
                                let _ = line.paint(*origin, frame.line_height, gpui::TextAlign::Left, None, window, cx);
                            }
                            if let Some(text) = &marked {
                                grid::paint_marked(&painted, text, window, cx);
                            }
                            // The link under the mouse is underlined.
                            if let (Some(cells), Some(g)) = (&link, *painted.geometry.lock().unwrap()) {
                                for (row, cols) in cells {
                                    let at = g.origin + point(g.cell_w * cols.start as f32, g.line_h * (*row as f32 + 1.) - px(1.));
                                    window.paint_quad(fill(Bounds::new(at, size(g.cell_w * cols.len() as f32, px(1.))), hsla(theme::ACCENT)));
                                }
                            }
                        });
                        window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx);
                        grid::record_frame(started.elapsed().as_micros() as u64);
                    },
                )
                .size_full(),
            )
            .child(scrollbar::vertical("term", &self.scrollbar))
            .when(self.shown_ix.is_none() && !self.shown.is_empty(), |d| d.child(self.viewer_chip(cx)))
            .children(self.mode_chip().map(|chip| chip.absolute().top(px(6.)).right(px(16.))));
        div()
            .flex()
            .size_full()
            .child(terminal)
            .when_some(self.shown_ix, |d, ix| d.child(self.viewer(ix, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{track_line, url_at};

    fn at(text: &str, needle: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        url_at(&chars, text[..text.find(needle).unwrap()].chars().count()).map(|(url, _)| url)
    }

    #[test]
    fn follows_the_typed_prompt() {
        let mut line = String::new();
        assert_eq!(track_line(&mut line, b"me mostra\x7f\x7f\x7f\x7fstra"), None);
        assert_eq!(track_line(&mut line, b" um print\x1b[D\x1b[C"), None);
        assert_eq!(track_line(&mut line, b"\r"), Some("me mostra um print".into()));
        assert_eq!(track_line(&mut line, b"\x1b[200~colado\naqui\x1b[201~\r"), Some("colado aqui".into()));
        assert_eq!(track_line(&mut line, b"lixo\x15ok\r"), Some("ok".into()));
        assert_eq!(track_line(&mut line, b"\r"), None);
    }

    #[test]
    fn finds_the_url_under_the_click() {
        assert_eq!(at("see https://x.dev/a?b=1. done", "x.dev"), Some("https://x.dev/a?b=1".into()));
        assert_eq!(at("│ [docs](https://x.dev/p) │", "dev"), Some("https://x.dev/p".into()));
        assert_eq!(at("open file:///tmp/a.png now", "tmp"), Some("file:///tmp/a.png".into()));
        assert_eq!(at("see https://x.dev now", "see"), None);
        assert_eq!(at("PR:https://x.dev/1", "PR"), None);
        assert_eq!(at("just https:// alone", "https"), None);
    }
}
