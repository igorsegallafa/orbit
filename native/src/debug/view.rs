//! The Debug tab of the bottom panel, laid out as Rider's Debug tool window:
//! the controls on top, Frames over Variables in a resizable column on the
//! left, and the main area for the program's output (a real terminal: gdb
//! runs the program on it) or the Debug Console (the debugger's messages and
//! the evaluate input). Both splitters are dragged and saved in the layout.
//! It shows the active session.
use super::{store, ConsoleKind, DebugStatus, Session};
use crate::app::OrbitApp;
use crate::logview::LogView;
use crate::probe::probe;
use crate::state::{DEBUG_FRAMES, DEBUG_SIDE};
use crate::theme::{self, c};
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, CursorStyle, DragMoveEvent, Empty, Entity, Subscription,
    WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MainTab {
    /// The program's terminal.
    Output,
    /// The debugger's messages and the evaluate input.
    Console,
}

pub struct DebugView {
    app: WeakEntity<OrbitApp>,
    input: Entity<InputState>,
    pub console: Entity<LogView>,
    pub tab: MainTab,
    /// The session the tab was last chosen for (a new one shows its program's output).
    program_shown: Option<gpui::EntityId>,
    /// The session the console shows.
    console_of: Option<gpui::EntityId>,
    _subscriptions: Vec<Subscription>,
}

/// A splitter being dragged.
#[derive(Clone, Copy)]
enum Split {
    /// The Frames and Variables column's width.
    Side,
    /// Frames' share of the column's height.
    Frames,
}

/// One row of the variables tree.
pub struct VarRow {
    pub depth: usize,
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
    pub reference: i64,
    pub scope: bool,
    pub open: bool,
    pub loading: bool,
}

/// The tree flattened: scopes, then expanded variables under them.
pub fn variable_rows(s: &Session, out: &mut Vec<VarRow>) {
    fn children(s: &Session, reference: i64, depth: usize, out: &mut Vec<VarRow>) {
        let Some(list) = s.vars.get(&reference) else {
            out.push(VarRow { depth, name: "Loading…".into(), value: String::new(), ty: None, reference: 0, scope: false, open: false, loading: true });
            return;
        };
        for v in list {
            let open = v.reference > 0 && s.open.contains(&v.reference);
            out.push(VarRow { depth, name: v.name.clone(), value: v.value.clone(), ty: v.ty.clone(), reference: v.reference, scope: false, open, loading: false });
            if open && depth < 12 {
                children(s, v.reference, depth + 1, out);
            }
        }
    }
    for (name, reference) in &s.scopes {
        let open = s.open.contains(reference);
        out.push(VarRow { depth: 0, name: name.clone(), value: String::new(), ty: None, reference: *reference, scope: true, open, loading: false });
        if open {
            children(s, *reference, 1, out);
        }
    }
}

const TOOLBAR: f32 = 30.;
const SPLITTER: f32 = 6.;
const MAX_CONSOLE_LINES: usize = 5000;

/// Reads the active session (an idle one when there is none).
fn read<R>(cx: &App, f: impl FnOnce(&Session) -> R) -> R {
    match super::active(cx) {
        Some(s) => f(s.read(cx)),
        None => f(&super::empty()),
    }
}

fn update(cx: &mut App, f: impl FnOnce(&mut Session, &mut Context<Session>)) {
    if let Some(s) = super::active(cx) {
        s.update(cx, f);
    }
}

impl DebugView {
    pub fn new(app: WeakEntity<OrbitApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Evaluate expression while paused (Enter)"));
        let console = cx.new(|cx| LogView::new("dbg-console", MAX_CONSOLE_LINES, cx));
        let subs = vec![
            cx.observe(&store(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    if text.trim().is_empty() || read(cx, |s| s.status) != DebugStatus::Stopped {
                        return;
                    }
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                    this.tab = MainTab::Console;
                    update(cx, |s, cx| s.evaluate(text, cx));
                }
            }),
        ];
        Self { app, input, console, tab: MainTab::Output, program_shown: None, console_of: None, _subscriptions: subs }
    }

    fn sizes(&self, cx: &App) -> (f32, f32) {
        self.app
            .upgrade()
            .map(|a| {
                let l = &a.read(cx).layout;
                (l.debug_side_width, l.debug_frames_ratio)
            })
            .unwrap_or((DEBUG_SIDE.default, DEBUG_FRAMES.default))
    }

    fn drag_split(&mut self, split: Split, ev: &DragMoveEvent<Split>, cx: &mut Context<Self>) {
        let b = ev.bounds;
        let pos = ev.event.position;
        let Some(app) = self.app.upgrade() else { return };
        app.update(cx, |a, cx| {
            match split {
                Split::Side => {
                    let max = (f32::from(b.size.width) - 240.).max(DEBUG_SIDE.min);
                    a.layout.debug_side_width = DEBUG_SIDE.clamp(f32::from(pos.x - b.left())).min(max);
                }
                Split::Frames => {
                    let top = b.top() + px(TOOLBAR);
                    let h = f32::from(b.bottom() - top).max(1.);
                    a.layout.debug_frames_ratio = DEBUG_FRAMES.clamp(f32::from(pos.y - top) / h);
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Keeps the console's lines in step with the store's.
    fn sync_console(&mut self, cx: &mut Context<Self>) {
        let of = store(cx).read(cx).active.as_ref().map(|s| s.entity_id());
        if of != self.console_of {
            self.console_of = of;
            self.console.update(cx, |v, cx| v.clear(cx));
        }
        let seen = self.console.read(cx).seen();
        let (total, tail) = read(cx, |s| {
            let fresh = s.console_total.saturating_sub(seen).min(s.console.len());
            // One more: the last shown line may have grown.
            let from = s.console.len() - (fresh + 1).min(s.console.len());
            (s.console_total, s.console[from..].to_vec())
        });
        self.console.update(cx, |v, cx| {
            v.sync(
                total,
                &tail,
                |(kind, text)| {
                    let color = match kind {
                        ConsoleKind::Err => theme::DANGER,
                        ConsoleKind::Info => theme::TEXT_FAINT,
                        ConsoleKind::Input => theme::ACCENT,
                        ConsoleKind::Result => theme::OK,
                        ConsoleKind::Out => theme::TEXT,
                    };
                    let text = if *kind == ConsoleKind::Input { format!("› {text}") } else { text.clone() };
                    (text, color, None)
                },
                cx,
            )
        });
    }
}

fn tool(
    id: &'static str,
    glyph: &'static str,
    hint: &'static str,
    enabled: bool,
    cx: &mut Context<DebugView>,
    on: fn(&mut Session, &mut Context<Session>),
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .text_color(c(theme::TEXT_DIM))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT))))
        .when(!enabled, |d| d.opacity(0.35))
        .child(crate::icons::ui_icon(glyph, 13., c(theme::TEXT_DIM)))
        .child(probe(id))
        .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
        .on_click(cx.listener(move |_, _, _, cx| {
            if enabled {
                update(cx, on);
            }
        }))
}

/// A drag handle between two parts: a thin line, easier to grab than to see.
fn splitter(id: &'static str, split: Split, vertical: bool) -> impl IntoElement {
    let handle = div()
        .id(id)
        .absolute()
        .when(vertical, |d| d.top_0().bottom_0().left(px(-SPLITTER / 2.)).w(px(SPLITTER)).cursor(CursorStyle::ResizeLeftRight))
        .when(!vertical, |d| d.left_0().right_0().top(px(-SPLITTER / 2.)).h(px(SPLITTER)).cursor(CursorStyle::ResizeUpDown))
        .hover(|s| s.bg(c(theme::ACCENT)))
        .child(probe(id))
        .on_drag(split, |_, _, _, cx| cx.new(|_| Empty));
    div()
        .relative()
        .flex_none()
        .when(vertical, |d| d.w(px(1.)).h_full())
        .when(!vertical, |d| d.h(px(1.)).w_full())
        .bg(c(theme::BORDER))
        .child(handle)
}

fn caption(text: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(text)
}

impl DebugView {
    fn render_frames(&self, cx: &mut Context<Self>) -> AnyElement {
        let (stopped, threads, thread_id, frames, frame_id) =
            read(cx, |s| (s.status == DebugStatus::Stopped, s.threads.clone(), s.thread_id, s.frames.clone(), s.frame_id));
        div()
            .id("dbg-frames")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(threads.len() > 1, |d| {
                d.children(threads.iter().map(|(id, name)| {
                    let id = *id;
                    let label = if name.is_empty() { format!("Thread {id}") } else { name.clone() };
                    div()
                        .id(("dbg-thread", id as u64))
                        .relative()
                        .px_2()
                        .py(px(2.))
                        .text_xs()
                        .cursor_pointer()
                        .text_color(c(if Some(id) == thread_id { theme::ACCENT } else { theme::TEXT_FAINT }))
                        .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                        .child(label)
                        .child(probe(format!("thread:{id}")))
                        .on_click(cx.listener(move |_, _, _, cx| update(cx, |s, cx| s.select_thread(id, false, cx))))
                }))
            })
            .when(frames.is_empty(), |d| {
                d.child(div().p_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(if stopped { "No frames" } else { "The call stack shows when the program pauses" }))
            })
            .children(frames.iter().enumerate().map(|(ix, f)| {
                let id = f.id;
                let active = Some(id) == frame_id;
                let loc = f.file.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line)).unwrap_or_default();
                div()
                    .id(("dbg-frame", ix))
                    .relative()
                    .px_2()
                    .py(px(2.))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::accent_soft()))
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .child(div().flex_1().truncate().text_color(c(if f.in_repo { theme::TEXT } else { theme::TEXT_FAINT })).child(f.name.clone()))
                    .child(div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(loc))
                    .child(probe(format!("frame:{ix}")))
                    .on_click(cx.listener(move |_, _, _, cx| update(cx, |s, cx| s.select_frame(id, true, cx))))
            }))
            .into_any_element()
    }

    fn render_vars(&self, cx: &mut Context<Self>) -> AnyElement {
        let (stopped, rows) = read(cx, |s| {
            let mut rows = vec![];
            variable_rows(s, &mut rows);
            (s.status == DebugStatus::Stopped, rows)
        });
        div()
            .id("dbg-vars")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(rows.is_empty(), |d| {
                d.child(div().p_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(if stopped { "No variables" } else { "Variables show when the program pauses" }))
            })
            .children(rows.into_iter().enumerate().map(|(ix, r)| {
                let reference = r.reference;
                let chevron = if r.reference > 0 { if r.open { "▾" } else { "▸" } } else { "" };
                let full = format!("{} = {}", r.name, r.value);
                div()
                    .id(("dbg-var", ix))
                    .relative()
                    .pl(px(8. + r.depth as f32 * 14.))
                    .pr_2()
                    .py(px(1.))
                    .flex()
                    .gap_1p5()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .when(reference > 0, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))))
                    .child(div().w(px(10.)).flex_none().text_color(c(theme::TEXT_FAINT)).child(chevron))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(if r.scope { theme::TEXT_DIM } else if r.loading { theme::TEXT_FAINT } else { 0xc792ea }))
                            .when(r.scope, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                            .child(r.name.clone()),
                    )
                    .when(!r.scope && !r.loading, |d| {
                        d.child(div().text_color(c(theme::TEXT_FAINT)).child("="))
                            .child(div().min_w_0().truncate().text_color(c(theme::TEXT)).child(r.value.clone()))
                            .children(r.ty.clone().map(|t| div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(t)))
                            .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                    })
                    .child(probe(format!("var:{}", r.name)))
                    .on_click(cx.listener(move |_, _, _, cx| update(cx, |s, cx| s.toggle_var(reference, cx))))
            }))
            .into_any_element()
    }

    fn main_tab(&self, id: &'static str, tab: MainTab, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tab == tab;
        div()
            .id(id)
            .relative()
            .h_full()
            .px_2()
            .flex()
            .items_center()
            .cursor_pointer()
            .border_b_2()
            .border_color(if active { c(theme::ACCENT) } else { gpui::transparent_black().into() })
            .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
            .hover(|s| s.text_color(c(theme::TEXT)))
            .child(label)
            .child(probe(id))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
    }
}

impl Render for DebugView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_console(cx);
        let (side_w, frames_ratio) = self.sizes(cx);
        let (status, live, reason, program) = read(cx, |s| (s.status, s.is_live(), s.stop_reason.clone(), s.program.clone()));
        let stopped = status == DebugStatus::Stopped;
        let state = match status {
            DebugStatus::Idle => "Shift+F9 debugs the selected configuration".to_string(),
            DebugStatus::Starting => "Starting…".into(),
            DebugStatus::Running => "Running".into(),
            DebugStatus::Stopped => format!("Paused{}", reason.map(|r| format!(" · {r}")).unwrap_or_default()),
            DebugStatus::Ended => "Ended".into(),
        };
        // A new session's program: show its output.
        if program.as_ref().map(|p| p.entity_id()) != self.program_shown {
            self.program_shown = program.as_ref().map(|p| p.entity_id());
            self.tab = if program.is_some() { MainTab::Output } else { MainTab::Console };
        }
        let tab = if program.is_none() { MainTab::Console } else { self.tab };
        let frames = self.render_frames(cx);
        let vars = self.render_vars(cx);

        let toolbar = div()
            .h(px(TOOLBAR))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(tool("dbg-resume", "play", "Resume (F9)", stopped, cx, Session::resume))
            .child(tool("dbg-pause", "pause", "Pause", status == DebugStatus::Running, cx, Session::pause))
            .child(tool("dbg-over", "step-over", "Step Over (F8)", stopped, cx, Session::step_over))
            .child(tool("dbg-into", "step-into", "Step Into (F7)", stopped, cx, Session::step_into))
            .child(tool("dbg-out", "step-out", "Step Out (Shift+F8)", stopped, cx, Session::step_out))
            .child(tool("dbg-stop", "stop", "Stop (Ctrl+F2)", live, cx, Session::stop))
            .child(div().ml_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(state));

        let side = div()
            .w(px(side_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(gpui::relative(frames_ratio))
                    .flex_none()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(caption("Frames"))
                    .child(frames),
            )
            .child(splitter("dbg-split-frames", Split::Frames, false))
            .child(div().flex_1().min_h_0().flex().flex_col().child(caption("Variables")).child(vars));

        let tabs = div()
            .h(px(26.))
            .flex_none()
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .when(program.is_some(), |d| d.child(self.main_tab("dbg-tab-output", MainTab::Output, "Program Output", cx)))
            .child(self.main_tab("dbg-tab-console", MainTab::Console, "Debug Console", cx));

        let body: AnyElement = match (tab, program) {
            (MainTab::Output, Some(p)) => div().flex_1().min_h_0().child(p).into_any_element(),
            _ => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(self.console.clone()))
                .child(
                    div()
                        .id("dbg-eval")
                        .relative()
                        .flex_none()
                        .px_1()
                        .border_t_1()
                        .border_color(c(theme::BORDER))
                        .when(!stopped, |d| d.opacity(0.5))
                        .child(Input::new(&self.input).appearance(false).text_size(px(12.)))
                        .child(probe("dbg-eval")),
                )
                .into_any_element(),
        };
        let main = div().flex_1().min_w_0().h_full().flex().flex_col().child(tabs).child(body);

        div()
            .id("dbg-view")
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<Split>, _, cx| {
                let split = *ev.drag(cx);
                this.drag_split(split, ev, cx);
            }))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().child(side).child(splitter("dbg-split-side", Split::Side, true)).child(main))
    }
}
