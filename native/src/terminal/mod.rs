//! Terminal tab: a session on `orbit_core::pty`, parsed by `alacritty_terminal`
//! on the PTY's output thread, painted as a grid of cells by a GPUI canvas
//! each frame. Parsing holds the program back (the core's read queue is
//! bounded), so a flooding program runs at the parser's pace and the UI only
//! ever waits for one small slice of parsing.
//! Mouse drags select text (double click: words, triple: lines) unless the
//! application asked for mouse reports, which Shift overrides.
mod grid;
mod input;

pub use grid::{FRAMES, FRAME_US_MAX, FRAME_US_TOTAL};

use crate::sessions::status::{self, Hook, Status, Tracker};
use crate::sessions::SessionMeta;
use crate::theme::{self, c, hsla};
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{CursorShape, Processor, Rgb},
};
use futures::{channel::mpsc, StreamExt};
use gpui::{
    canvas, div, fill, outline, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, FocusHandle,
    EventEmitter, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollEvent,
    SharedString, Task, Window,
};
use grid::Geometry;
use input::{key_to_bytes, mouse_report, MouseReport};
use orbit_core::pty;
use std::{
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
/// records the title it sets.
#[derive(Clone)]
struct Listener {
    pty_id: u32,
    title: Arc<Mutex<Option<String>>>,
    window_size: Arc<Mutex<WindowSize>>,
}

fn rgb_of(hex: u32) -> Rgb {
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
}

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
    /// A selection drag is in progress.
    selecting: bool,
    /// The cell of the last motion report, so moving within a cell sends nothing.
    last_motion_cell: Option<(usize, usize)>,
    /// The last text copied (self-test dumps).
    pub last_copied: Option<String>,
    _pump: Task<()>,
    _status_poll: Task<()>,
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
    /// Starts `spec` on a PTY. `replay` is printed first (a restored tab's
    /// previous output); `input` is typed once the program runs.
    pub fn new(meta: SessionMeta, spec: pty::Spawn, replay: Option<Vec<u8>>, input: Option<String>, cx: &mut Context<Self>) -> Self {
        let size = GridSize { cols: spec.size.cols.max(2) as usize, rows: spec.size.rows.max(1) as usize };
        let pty_id = pty::reserve_id();
        let title = Arc::new(Mutex::new(None));
        let window_size = Arc::new(Mutex::new(WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: 8,
            cell_height: LINE_HEIGHT as u16,
        }));
        let listener = Listener { pty_id, title: title.clone(), window_size: window_size.clone() };
        let term = Term::new(Config::default(), &size, listener);
        let shared = Arc::new(Shared { term: FairMutex::new(term), pty_id, size: Mutex::new(size), window_size, geometry: Mutex::new(None) });
        let mut parser: Processor = Processor::new();
        if let Some(saved) = replay.filter(|r| !r.is_empty()) {
            parser.advance(&mut *shared.term.lock(), &restored_output(&saved, size.rows));
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
            move |out: pty::Output| {
                match out {
                    pty::Output::Data(bytes) => {
                        for slice in bytes.chunks(PARSE_SLICE) {
                            parser.advance(&mut *shared.term.lock(), slice);
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
        if let Err(e) = pty::spawn(pty_id, spec, on_output) {
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
        Self {
            shared,
            meta,
            tracker,
            status: Status::Idle,
            busy_since: None,
            title,
            exited,
            focus: cx.focus_handle(),
            scroll_remainder: 0.,
            selecting: false,
            last_motion_cell: None,
            last_copied: None,
            _pump: pump,
            _status_poll: status_poll,
        }
    }

    pub fn is_agent(&self) -> bool {
        self.meta.cmd.is_some()
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.publish_status(cx);
    }

    /// Recomputes the status; on a change tells the session list, and for
    /// agents without hooks turns a long busy stretch ending in idle into "done".
    fn publish_status(&mut self, cx: &mut Context<Self>) {
        let (status, hooks) = {
            let t = self.tracker.lock().unwrap();
            (t.status(Instant::now()), t.has_hooks())
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

    pub fn selection_text(&self) -> Option<String> {
        self.shared.term.lock().selection_to_string()
    }

    /// Window position of a viewport cell's center, once painted (self-test).
    pub fn cell_position(&self, col: usize, row: usize) -> Option<gpui::Point<gpui::Pixels>> {
        self.shared.geometry.lock().unwrap().map(|g| g.cell_center(col, row))
    }

    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.shared.term.lock().scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    fn write(&self, bytes: &[u8]) {
        self.send(bytes);
    }

    fn paste(&self, text: &str) {
        let bracketed = self.shared.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.write(format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).as_bytes());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
    }

    fn copy(&mut self, primary: bool, cx: &mut Context<Self>) {
        let Some(text) = self.selection_text().filter(|t| !t.is_empty()) else { return };
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

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.control && m.shift && ks.key == "c" {
            self.copy(false, cx);
            cx.stop_propagation();
            return;
        }
        if m.control && m.shift && ks.key == "v" || m.shift && ks.key == "insert" {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.paste(&text);
            }
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
            {
                let mut term = self.shared.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            self.write(&bytes);
            cx.stop_propagation();
        }
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

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((col, row, right_half)) = self.cell_at(ev.position) else { return };
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
                self.selecting = true;
                cx.notify();
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            MouseButton::Middle => {
                if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
                    self.paste(&text);
                }
            }
            _ => {}
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((col, row, right_half)) = self.cell_at(ev.position) else { return };
        if let Some(mode) = self.reporting(ev.modifiers.shift) {
            if self.last_motion_cell.replace((col, row)) == Some((col, row)) {
                return;
            }
            if let Some(bytes) = mouse_report(mode, MouseReport::Motion(ev.pressed_button), col, row, ev.modifiers) {
                self.write(&bytes);
            }
            return;
        }
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            let side = if right_half { Side::Right } else { Side::Left };
            let point = self.grid_point(col, row);
            if let Some(sel) = self.shared.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(mode) = self.reporting(ev.modifiers.shift) {
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shared = self.shared.clone();
        let focused = self.focus.is_focused(window);
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(c(theme::BG))
            .cursor_text()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .child(
                canvas(
                    move |bounds, window, _cx| (std::time::Instant::now(), grid::layout(&shared, bounds, window)),
                    move |bounds, (started, frame), window, cx| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            for (b, color) in &frame.rects {
                                window.paint_quad(fill(*b, *color));
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
                        });
                        grid::record_frame(started.elapsed().as_micros() as u64);
                    },
                )
                .size_full(),
            )
    }
}
