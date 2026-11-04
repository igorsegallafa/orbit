//! Terminal tab: a PTY (`portable-pty`) parsed by `alacritty_terminal` on a
//! reader thread, painted as a grid of cells by a GPUI canvas each frame.
//! Mouse drags select text (double click: words, triple: lines) unless the
//! application asked for mouse reports, which Shift overrides.
mod grid;
mod input;

pub use grid::{FRAMES, FRAME_US_MAX, FRAME_US_TOTAL};

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
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollEvent,
    SharedString, Task, Window,
};
use grid::Geometry;
use input::{key_to_bytes, mouse_report, MouseReport};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
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

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Answers the terminal's queries (device attributes, colors, size) and
/// records the title it sets.
#[derive(Clone)]
struct Listener {
    writer: Writer,
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
        let _ = self.writer.lock().unwrap().write_all(reply.as_bytes());
    }
}

pub struct Shared {
    term: FairMutex<Term<Listener>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    size: Mutex<GridSize>,
    window_size: Arc<Mutex<WindowSize>>,
    geometry: Mutex<Option<Geometry>>,
}

/// A spawned shell on a PTY, not yet attached to a view.
pub struct Pty {
    shared: Arc<Shared>,
    reader: Box<dyn Read + Send>,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
    title: Arc<Mutex<Option<String>>>,
}

impl Pty {
    pub fn spawn(cwd: &Path) -> anyhow::Result<Self> {
        let size = GridSize { cols: 80, rows: 24 };
        let pair = native_pty_system().openpty(PtySize { rows: size.rows as u16, cols: size.cols as u16, pixel_width: 0, pixel_height: 0 })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let title = Arc::new(Mutex::new(None));
        let window_size = Arc::new(Mutex::new(WindowSize {
            num_lines: size.rows as u16,
            num_cols: size.cols as u16,
            cell_width: 8,
            cell_height: LINE_HEIGHT as u16,
        }));
        let listener = Listener { writer: writer.clone(), title: title.clone(), window_size: window_size.clone() };
        let term = Term::new(Config::default(), &size, listener);
        let shared = Arc::new(Shared {
            term: FairMutex::new(term),
            master: Mutex::new(pair.master),
            size: Mutex::new(size),
            window_size,
            geometry: Mutex::new(None),
        });
        Ok(Self { shared, reader, writer, child, title })
    }
}

pub struct TerminalView {
    shared: Arc<Shared>,
    writer: Writer,
    child: Box<dyn Child + Send + Sync>,
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
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl TerminalView {
    pub fn new(pty: Pty, initial_input: Option<String>, cx: &mut Context<Self>) -> Self {
        let Pty { shared, mut reader, writer, child, title } = pty;
        // The reader thread parses everything it reads; the UI only learns that
        // something changed (at most one pending wake-up) and repaints once per
        // frame, so bursts of output cost one render, not one per read.
        let (tx, mut rx) = mpsc::unbounded::<()>();
        let dirty = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        {
            let shared = shared.clone();
            let dirty = dirty.clone();
            let exited = exited.clone();
            std::thread::Builder::new()
                .name("pty-reader".into())
                .spawn(move || {
                    let mut parser: Processor = Processor::new();
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                parser.advance(&mut *shared.term.lock(), &buf[..n]);
                                if !dirty.swap(true, Ordering::AcqRel) {
                                    let _ = tx.unbounded_send(());
                                }
                            }
                        }
                    }
                    exited.store(true, Ordering::Release);
                    let _ = tx.unbounded_send(());
                })
                .expect("spawn pty reader thread");
        }
        let pump = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                dirty.store(false, Ordering::Release);
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        if let Some(input) = initial_input {
            let _ = writer.lock().unwrap().write_all(format!("{input}\r").as_bytes());
        }
        Self {
            shared,
            writer,
            child,
            title,
            exited,
            focus: cx.focus_handle(),
            scroll_remainder: 0.,
            selecting: false,
            last_motion_cell: None,
            last_copied: None,
            _pump: pump,
        }
    }

    pub fn title(&self) -> SharedString {
        let base = self.title.lock().unwrap().clone().unwrap_or_else(|| "Terminal".into());
        if self.exited.load(Ordering::Acquire) {
            format!("{base} [exited]").into()
        } else {
            base.into()
        }
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
        let _ = self.writer.lock().unwrap().write_all(bytes);
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
            if primary {
                cx.write_to_primary(item);
            } else {
                cx.write_to_clipboard(item);
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
