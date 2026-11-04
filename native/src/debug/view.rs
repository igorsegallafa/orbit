//! The Debug tab of the bottom panel (`DebugView.tsx`): controls, threads
//! and call stack, the variables tree and the debug console.
use super::{store, ConsoleKind, DebugStatus, DebugStore};
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Context, Entity, ScrollHandle, Subscription, Window};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;

pub struct DebugView {
    input: Entity<InputState>,
    console: ScrollHandle,
    shown: usize,
    _subscriptions: Vec<Subscription>,
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
pub fn variable_rows(s: &DebugStore, out: &mut Vec<VarRow>) {
    fn children(s: &DebugStore, reference: i64, depth: usize, out: &mut Vec<VarRow>) {
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

impl DebugView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Evaluate expression (Enter)"));
        let subs = vec![
            cx.observe(&store(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(&input, window, |_, input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let text = input.read(cx).value().to_string();
                    if text.trim().is_empty() || store(cx).read(cx).status != DebugStatus::Stopped {
                        return;
                    }
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                    store(cx).update(cx, |s, cx| s.evaluate(text, cx));
                }
            }),
        ];
        Self { input, console: ScrollHandle::new(), shown: 0, _subscriptions: subs }
    }
}

fn tool(
    id: &'static str,
    glyph: &'static str,
    hint: &'static str,
    enabled: bool,
    cx: &mut Context<DebugView>,
    on: fn(&mut DebugStore, &mut Context<DebugStore>),
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
        .child(crate::icons::ui_icon(glyph, 13.))
        .child(probe(id))
        .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
        .on_click(cx.listener(move |_, _, _, cx| {
            if enabled {
                store(cx).update(cx, on);
            }
        }))
}

impl Render for DebugView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = store(cx);
        let s = entity.read(cx);
        let (status, stopped) = (s.status, s.status == DebugStatus::Stopped);
        let live = s.is_live();
        let state = match status {
            DebugStatus::Idle => "Shift+F9 debugs the selected configuration".to_string(),
            DebugStatus::Starting => "Starting…".into(),
            DebugStatus::Running => "Running".into(),
            DebugStatus::Stopped => format!("Paused{}", s.stop_reason.as_ref().map(|r| format!(" · {r}")).unwrap_or_default()),
            DebugStatus::Ended => "Ended".into(),
        };
        let threads = s.threads.clone();
        let thread_id = s.thread_id;
        let frames = s.frames.clone();
        let frame_id = s.frame_id;
        let mut rows = vec![];
        variable_rows(s, &mut rows);
        let console: Vec<(ConsoleKind, String)> = s.console.clone();
        if console.len() != self.shown {
            self.shown = console.len();
            self.console.scroll_to_bottom();
        }

        let toolbar = div()
            .h(px(30.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(tool("dbg-resume", "play", "Resume (F9)", stopped, cx, DebugStore::resume))
            .child(tool("dbg-pause", "pause", "Pause", status == DebugStatus::Running, cx, DebugStore::pause))
            .child(tool("dbg-over", "step-over", "Step Over (F8)", stopped, cx, DebugStore::step_over))
            .child(tool("dbg-into", "step-into", "Step Into (F7)", stopped, cx, DebugStore::step_into))
            .child(tool("dbg-out", "step-out", "Step Out (Shift+F8)", stopped, cx, DebugStore::step_out))
            .child(tool("dbg-stop", "stop", "Stop (Ctrl+F2)", live, cx, DebugStore::stop))
            .child(div().ml_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(state));

        let frame_list = div()
            .id("dbg-frames")
            .w(px(300.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(c(theme::BORDER))
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
                        .on_click(cx.listener(move |_, _, _, cx| store(cx).update(cx, |s, cx| s.select_thread(id, false, cx))))
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
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(loc))
                    .child(probe(format!("frame:{ix}")))
                    .on_click(cx.listener(move |_, _, _, cx| store(cx).update(cx, |s, cx| s.select_frame(id, true, cx))))
            }));

        let vars = div()
            .id("dbg-vars")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .when(rows.is_empty(), |d| {
                d.child(div().p_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(if stopped { "No variables" } else { "Variables show when the program pauses" }))
            })
            .children(rows.into_iter().enumerate().map(|(ix, r)| {
                let reference = r.reference;
                let chevron = if r.reference > 0 { if r.open { "▾" } else { "▸" } } else { "" };
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
                            .child(div().truncate().text_color(c(theme::TEXT)).child(r.value.clone()))
                            .children(r.ty.clone().map(|t| div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(t)))
                    })
                    .child(probe(format!("var:{}", r.name)))
                    .on_click(cx.listener(move |_, _, _, cx| store(cx).update(cx, |s, cx| s.toggle_var(reference, cx))))
            }));

        let console_view = div()
            .w(px(380.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .id("dbg-console")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.console)
                    .px_2()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.))
                    .children(console.into_iter().map(|(kind, text)| {
                        let color = match kind {
                            ConsoleKind::Err => theme::DANGER,
                            ConsoleKind::Info => theme::TEXT_FAINT,
                            ConsoleKind::Input => theme::ACCENT,
                            ConsoleKind::Result => theme::OK,
                            ConsoleKind::Out => theme::TEXT,
                        };
                        let text = if kind == ConsoleKind::Input { format!("› {text}") } else { text };
                        div().whitespace_nowrap().text_color(c(color)).child(text)
                    })),
            )
            .child(
                div()
                    .id("dbg-eval")
                    .relative()
                    .flex_none()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .when(!stopped, |d| d.opacity(0.5))
                    .child(Input::new(&self.input).appearance(false).text_size(px(12.)))
                    .child(probe("dbg-eval")),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().child(frame_list).child(vars).child(console_view))
    }
}
