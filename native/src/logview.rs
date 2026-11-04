//! A log view: the Build output and the debug console. Lines keep their ANSI
//! colours (`ansi.rs`), wrap at the view's width, follow the end while
//! output arrives unless scrolled up, and select with the mouse (drag, double
//! click for a line) for Ctrl+C / Ctrl+Shift+C; Ctrl+A selects everything.
//! A line can carry a link (a compiler message opens its file).
use crate::ansi::{self, Color, Style};
use crate::probe::probe;
use crate::theme::{self, c, hsla};
use gpui::{
    div, font, list, prelude::*, px, App, ClipboardItem, Context, FocusHandle, Focusable, FollowMode, FontStyle,
    FontWeight, Hsla, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    SharedString, StyledText, TextRun, UnderlineStyle, Window,
};
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub type Link = Rc<dyn Fn(&mut Window, &mut App)>;

struct LogLine {
    /// As given (escapes included): a re-sent last line is compared with it.
    raw: String,
    line: ansi::Line,
    /// The colour of uncoloured text.
    base: u32,
    link: Option<Link>,
    /// The SGR state the line started with, and ended with.
    start: Style,
    end: Style,
}

/// A place in the text: a line (counted from the first line ever pushed) and a byte in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    line: usize,
    ix: usize,
}

pub struct LogView {
    id: SharedString,
    lines: Vec<LogLine>,
    /// Lines dropped from the front (`max`): line numbers stay stable for the selection.
    dropped: usize,
    max: usize,
    list: ListState,
    focus: FocusHandle,
    anchor: Option<Pos>,
    head: Option<Pos>,
    /// A press that hasn't moved yet: a click (links) rather than a selection.
    pressed: Option<Pos>,
    selecting: bool,
    /// The source lines already shown (see [`LogView::sync`]).
    seen: usize,
    pub last_copied: Option<String>,
}

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 18.;

impl LogView {
    pub fn new(id: impl Into<SharedString>, max: usize, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            id: id.into(),
            lines: vec![],
            dropped: 0,
            max,
            list,
            focus: cx.focus_handle(),
            anchor: None,
            head: None,
            pressed: None,
            selecting: false,
            seen: 0,
            last_copied: None,
        }
    }

    /// The source lines [`LogView::sync`] has shown.
    pub fn seen(&self) -> usize {
        self.seen
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines.clear();
        self.dropped = 0;
        self.seen = 0;
        self.anchor = None;
        self.head = None;
        self.list.reset(0);
        self.list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    /// Appends one line (no `\n`).
    pub fn push(&mut self, raw: &str, base: u32, link: Option<Link>, cx: &mut Context<Self>) {
        let start = self.lines.last().map(|l| l.end).unwrap_or_default();
        let mut end = start;
        let line = ansi::parse(raw, &mut end);
        self.lines.push(LogLine { raw: raw.to_string(), line, base, link, start, end });
        let n = self.lines.len();
        self.list.splice(n - 1..n - 1, 1);
        if self.lines.len() > self.max {
            let extra = self.lines.len() - self.max;
            self.lines.drain(..extra);
            self.list.splice(0..extra, 0);
            self.dropped += extra;
        }
        cx.notify();
    }

    /// Replaces the last line's text (output that arrived in pieces).
    pub fn set_last(&mut self, raw: &str, cx: &mut Context<Self>) {
        let Some(last) = self.lines.last_mut() else { return };
        if last.raw == raw {
            return;
        }
        let mut end = last.start;
        last.line = ansi::parse(raw, &mut end);
        last.end = end;
        last.raw = raw.to_string();
        let n = self.lines.len();
        self.list.splice(n - 1..n, 1);
        cx.notify();
    }

    /// Follows a source that keeps its last lines and counts every line it
    /// ever had (`total`): pushes the ones not shown yet, and the last line
    /// again if it grew. `item` gives a line's text, colour and link.
    pub fn sync<T>(&mut self, total: usize, tail: &[T], item: impl Fn(&T) -> (String, u32, Option<Link>), cx: &mut Context<Self>) {
        if total < self.seen {
            self.clear(cx);
        }
        let new = (total - self.seen).min(tail.len());
        if new == 0 {
            if let Some(last) = tail.last().filter(|_| total > 0) {
                let (text, _, _) = item(last);
                self.set_last(&text, cx);
            }
        } else {
            if let Some(prev) = tail.len().checked_sub(new + 1).and_then(|i| tail.get(i)).filter(|_| !self.lines.is_empty()) {
                let (text, _, _) = item(prev);
                self.set_last(&text, cx);
            }
            for t in &tail[tail.len() - new..] {
                let (text, base, link) = item(t);
                self.push(&text, base, link, cx);
            }
        }
        self.seen = total;
    }

    fn line(&self, pos_line: usize) -> Option<&LogLine> {
        pos_line.checked_sub(self.dropped).and_then(|i| self.lines.get(i))
    }

    fn range(&self) -> Option<(Pos, Pos)> {
        let (a, h) = (self.anchor?, self.head?);
        (a != h).then(|| if a < h { (a, h) } else { (h, a) })
    }

    /// The selected bytes of line `n`, and whether its line break is selected too.
    fn selected_in(&self, n: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.range()?;
        if n < a.line || n > b.line {
            return None;
        }
        let from = if n == a.line { a.ix.min(len) } else { 0 };
        let to = if n == b.line { b.ix.min(len) } else { len };
        Some(from..to)
    }

    pub fn selection_text(&self) -> Option<String> {
        let (a, b) = self.range()?;
        let mut out = String::new();
        for n in a.line..=b.line {
            let Some(l) = self.line(n) else { continue };
            let r = self.selected_in(n, l.line.text.len()).unwrap_or(0..0);
            out.push_str(l.line.text.get(r).unwrap_or_default());
            if n < b.line {
                out.push('\n');
            }
        }
        Some(out)
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.lines.last() else { return };
        self.anchor = Some(Pos { line: self.dropped, ix: 0 });
        self.head = Some(Pos { line: self.dropped + self.lines.len() - 1, ix: last.line.text.len() });
        cx.notify();
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.selection_text().filter(|t| !t.is_empty()) else { return };
        if !crate::terminal::CLIPBOARD_DISABLED.load(Ordering::Relaxed) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
        self.last_copied = Some(text);
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.control && ks.key == "c" {
            self.copy(cx);
            cx.stop_propagation();
        } else if m.control && !m.shift && ks.key == "a" {
            self.select_all(cx);
            cx.stop_propagation();
        } else if ks.key == "escape" && self.range().is_some() {
            self.anchor = None;
            self.head = None;
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn press(&mut self, at: Pos, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if clicks >= 2 {
            // The whole line.
            let len = self.line(at.line).map_or(0, |l| l.line.text.len());
            self.anchor = Some(Pos { line: at.line, ix: 0 });
            self.head = Some(Pos { line: at.line, ix: len });
            self.pressed = None;
            self.selecting = false;
        } else {
            self.anchor = Some(at);
            self.head = Some(at);
            self.pressed = Some(at);
            self.selecting = true;
        }
        cx.notify();
    }

    fn drag(&mut self, at: Pos, cx: &mut Context<Self>) {
        if !self.selecting || self.head == Some(at) {
            return;
        }
        self.head = Some(at);
        self.pressed = None;
        // Selecting while output arrives: stay put.
        self.list.pause_following_tail();
        cx.notify();
    }

    fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if let Some(at) = self.pressed.take() {
            if let Some(link) = self.line(at.line).and_then(|l| l.link.clone()) {
                self.anchor = None;
                self.head = None;
                link(window, cx);
            }
        }
        cx.notify();
    }

    fn color(color: Color, bold: bool) -> Hsla {
        match color {
            Color::Indexed(ix) if ix < 8 && bold => hsla(theme::ANSI[ix as usize + 8]),
            Color::Indexed(ix) if ix < 16 => hsla(theme::ANSI[ix as usize]),
            Color::Indexed(ix) => {
                let (r, g, b) = ansi::xterm_rgb(ix);
                hsla(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
            }
            Color::Rgb(r, g, b) => hsla(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        }
    }

    fn runs(&self, l: &LogLine, selected: Option<Range<usize>>) -> Vec<TextRun> {
        let base = hsla(l.base);
        let mut sel_bg = hsla(theme::ACCENT);
        sel_bg.a = 0.35;
        let mut cuts: Vec<usize> = l.line.spans.iter().flat_map(|s| [s.range.start, s.range.end]).collect();
        if let Some(r) = &selected {
            cuts.extend([r.start, r.end]);
        }
        cuts.retain(|&c| c <= l.line.text.len());
        cuts.sort_unstable();
        cuts.dedup();
        let mut runs = vec![];
        for w in cuts.windows(2) {
            let (from, to) = (w[0], w[1]);
            if from == to || !l.line.text.is_char_boundary(from) || !l.line.text.is_char_boundary(to) {
                continue;
            }
            let style = l.line.spans.iter().find(|s| s.range.start <= from && from < s.range.end).map(|s| s.style).unwrap_or_default();
            let mut fg = style.fg.map_or(base, |c| Self::color(c, style.bold));
            let mut bg = style.bg.map(|c| Self::color(c, false));
            if style.inverse {
                let back = bg.unwrap_or(hsla(theme::PANEL));
                bg = Some(fg);
                fg = back;
            }
            if style.dim {
                fg.a *= 0.65;
            }
            if selected.as_ref().is_some_and(|r| r.start <= from && to <= r.end) {
                bg = Some(sel_bg);
            }
            let mut f = font(theme::MONO_FONT);
            if style.bold {
                f.weight = FontWeight::BOLD;
            }
            if style.italic {
                f.style = FontStyle::Italic;
            }
            runs.push(TextRun {
                len: to - from,
                font: f,
                color: fg,
                background_color: bg,
                underline: style.underline.then(|| UnderlineStyle { thickness: px(1.), color: Some(fg), wavy: false }),
                strikethrough: None,
            });
        }
        runs
    }

    fn render_line(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(l) = self.lines.get(ix) else { return div().into_any_element() };
        let n = ix + self.dropped;
        let len = l.line.text.len();
        let selected = self.selected_in(n, len).filter(|r| !r.is_empty());
        // A selected empty line (inside a selection) shows as a marker.
        let empty_selected = len == 0 && self.range().is_some_and(|(a, b)| a.line < n && n < b.line);
        let text: SharedString = if len == 0 { " ".into() } else { l.line.text.clone().into() };
        let runs = if len == 0 {
            vec![TextRun { len: 1, font: font(theme::MONO_FONT), color: hsla(l.base), background_color: None, underline: None, strikethrough: None }]
        } else {
            self.runs(l, selected)
        };
        let styled = StyledText::new(text).with_runs(runs);
        let layout = styled.layout().clone();
        let hit = move |pos: gpui::Point<gpui::Pixels>| -> Pos {
            let i = match layout.index_for_position(pos) {
                Ok(i) | Err(i) => i,
            };
            Pos { line: n, ix: i.min(len) }
        };
        let hit2 = hit.clone();
        let linked = l.link.is_some();
        div()
            .id(("log-line", n))
            .w_full()
            .px_3()
            .min_h(px(LINE_HEIGHT))
            .when(empty_selected, |d| d.bg(theme::accent_soft()))
            .when(linked, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))))
            .child(styled)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.press(hit(ev.position), ev.click_count, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    this.drag(hit2(ev.position), cx);
                }
            }))
            .into_any_element()
    }

    /// The state for the self-test dump.
    pub fn dump(&self) -> String {
        let (a, b) = self.range().map_or((None, None), |(a, b)| (Some((a.line, a.ix)), Some((b.line, b.ix))));
        let colored = self.lines.iter().filter(|l| l.line.spans.iter().any(|s| s.style != Style::default())).count();
        format!(
            "log {}: {} lines ({} coloured), following {}, selection {:?}..{:?}, copied {:?}",
            self.id,
            self.lines.len(),
            colored,
            self.list.is_following_tail(),
            a,
            b,
            self.last_copied
        )
    }
}

impl Focusable for LogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id.clone();
        div()
            .id(id.clone())
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, window, cx| this.release(window, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.selecting = false))
            .font_family(theme::MONO_FONT)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(c(theme::TEXT))
            .child(list(self.list.clone(), cx.processor(|this, ix, _, cx| this.render_line(ix, cx))).size_full().py_1())
            .child(probe(id.to_string()))
    }
}
