//! Find Action (Ctrl+Shift+A): the registered commands, filtered by what is
//! typed, with their shortcuts. Looks like Search Everywhere's list
//! (`SearchEverywhereModal.tsx`).
use crate::app::OrbitApp;
use crate::commands::{self, Command};
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Context, Entity, Focusable, FontWeight, MouseButton, SharedString, Subscription, Window};
use gpui_component::input::{Input, InputEvent, InputState};

pub struct Palette {
    pub input: Entity<InputState>,
    /// Commands that match the query and can run now.
    pub matches: Vec<Command>,
    pub selected: usize,
    _sub: Subscription,
}

impl Palette {
    pub fn move_selection(&mut self, delta: isize) {
        let n = self.matches.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        }
    }
}

const MAX_ROWS: usize = 12;

impl OrbitApp {
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type an action…"));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_palette(cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        window.prevent_default();
        self.palette = Some(Palette { input, matches: vec![], selected: 0, _sub: sub });
        self.filter_palette(cx);
    }

    fn filter_palette(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.palette else { return };
        let query = p.input.read(cx).value().to_string();
        let matches: Vec<Command> =
            commands::all(cx).into_iter().filter(|c| c.run.is_some() && c.enabled(self, cx) && commands::matches(&c.title, &query)).collect();
        if let Some(p) = self.palette.as_mut() {
            p.matches = matches;
            p.selected = 0;
        }
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.restore_focus(window, cx);
            cx.notify();
        }
    }

    pub fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.palette.as_ref().and_then(|p| p.matches.get(p.selected)).map(|c| c.id.clone()) else { return };
        self.run_palette_command(id, window, cx);
    }

    fn run_palette_command(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        // Closed first: an action-command goes to the view that had the focus.
        self.close_palette(window, cx);
        commands::run(&id, self, window, cx);
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.palette.as_ref()?;
        let first = p.selected.saturating_sub(MAX_ROWS - 1);
        Some(
            div()
                .id("palette-scrim")
                .key_context(crate::overlay::CONTEXT)
                .absolute()
                .inset_0()
                .bg(theme::scrim())
                .flex()
                .justify_center()
                .pt(px(72.))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
                .child(
                    div()
                        .id("palette")
                        .occlude()
                        .w(px(640.))
                        .max_h(px(460.))
                        .bg(c(theme::PANEL))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(12.))
                        .shadow(theme::shadow(20., 60., 0.6))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(c(theme::BORDER))
                                .child(div().flex_1().child(Input::new(&p.input).appearance(false).text_size(px(15.))))
                                .child(
                                    div()
                                        .flex_none()
                                        .px(px(10.))
                                        .py(px(3.))
                                        .rounded_full()
                                        .bg(c(theme::PANEL_RAISED))
                                        .text_size(px(11.))
                                        .text_color(c(theme::TEXT_FAINT))
                                        .child("Actions"),
                                ),
                        )
                        .child(
                            div()
                                .p(px(6.))
                                .when(p.matches.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .p(px(40.))
                                            .flex()
                                            .justify_center()
                                            .text_size(px(13.))
                                            .text_color(c(theme::TEXT_FAINT))
                                            .child("No matching actions"),
                                    )
                                })
                                .children(p.matches.iter().enumerate().skip(first).take(MAX_ROWS).map(|(ix, cmd)| {
                                    let id = cmd.id.clone();
                                    div()
                                        .id(("palette-row", ix))
                                        .relative()
                                        .h(px(30.))
                                        .px(px(10.))
                                        .flex()
                                        .items_center()
                                        .gap(px(9.))
                                        .rounded(px(6.))
                                        .cursor_pointer()
                                        .when(ix == p.selected, |d| d.bg(theme::accent_soft()))
                                        .hover(|s| s.bg(theme::accent_soft()))
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(c(theme::TEXT))
                                                .child(cmd.title.clone()),
                                        )
                                        .children(cmd.shortcut().map(|k| {
                                            div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(k)
                                        }))
                                        .child(probe(format!("palette:{}", cmd.title)))
                                        .on_click(cx.listener(move |this, _, window, cx| this.run_palette_command(id.clone(), window, cx)))
                                })),
                        ),
                ),
        )
    }
}
