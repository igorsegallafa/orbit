//! Context menus and confirm dialogs, styled as `ContextMenu.tsx` and
//! `ConfirmDialog.tsx`. One of each can be open; both take the keyboard
//! (↑/↓/Enter/Esc) through the `OrbitOverlay` key context.
use crate::app::OrbitApp;
use crate::commands::Handler;
use crate::probe::probe;
use crate::theme::{self, c, size};
use gpui::{
    actions, div, prelude::*, px, App, Context, FontWeight, KeyBinding, MouseButton, Pixels, Point, SharedString,
    Window,
};
use std::rc::Rc;

actions!(orbit_overlay, [OverlayUp, OverlayDown, OverlayConfirm, OverlayCancel]);

pub const CONTEXT: &str = "OrbitOverlay";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", OverlayUp, Some(CONTEXT)),
        KeyBinding::new("down", OverlayDown, Some(CONTEXT)),
        KeyBinding::new("enter", OverlayConfirm, Some(CONTEXT)),
        KeyBinding::new("escape", OverlayCancel, Some(CONTEXT)),
    ]);
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub danger: bool,
    pub run: Handler,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static) -> Self {
        Self { label: label.into(), danger: false, run: Rc::new(run) }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

pub struct ContextMenu {
    /// Window position of the top-left corner (the click).
    pub origin: Point<Pixels>,
    pub items: Vec<MenuItem>,
    pub highlighted: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
}

#[derive(Clone)]
pub struct DialogButton {
    pub label: SharedString,
    pub style: ButtonStyle,
    pub run: Handler,
}

pub struct Dialog {
    pub title: SharedString,
    pub message: SharedString,
    /// Left to right; the last is the default (Enter).
    pub buttons: Vec<DialogButton>,
    /// Esc and a click outside.
    pub on_cancel: Handler,
}

impl OrbitApp {
    pub fn open_menu(&mut self, origin: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        self.menu = Some(ContextMenu { origin, items, highlighted: None });
        window.focus(&self.overlay_focus, cx);
        // Opened by a mouse down: keep the click from moving the focus again.
        window.prevent_default();
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.restore_focus(window, cx);
            cx.notify();
        }
    }

    /// Runs a menu item by index (a click, Enter, or a script).
    pub fn pick_menu_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.menu.as_ref().and_then(|m| m.items.get(ix)).cloned() else { return };
        self.close_menu(window, cx);
        (item.run)(self, window, cx);
    }

    pub fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.dialog = Some(dialog);
        window.focus(&self.overlay_focus, cx);
        window.prevent_default();
        cx.notify();
    }

    /// Runs a dialog button (or the cancel handler with `None`) and closes the dialog.
    pub fn answer_dialog(&mut self, button: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else { return };
        self.restore_focus(window, cx);
        let run = match button.and_then(|ix| dialog.buttons.get(ix)) {
            Some(b) => b.run.clone(),
            None => dialog.on_cancel.clone(),
        };
        run(self, window, cx);
        cx.notify();
    }

    pub(crate) fn on_overlay_up(&mut self, _: &OverlayUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.menu.as_mut() {
            let n = m.items.len();
            m.highlighted = Some(m.highlighted.map_or(n - 1, |h| (h + n - 1) % n));
            cx.notify();
        } else if let Some(p) = self.palette.as_mut() {
            p.move_selection(-1);
            cx.notify();
        }
    }

    pub(crate) fn on_overlay_down(&mut self, _: &OverlayDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.menu.as_mut() {
            let n = m.items.len();
            m.highlighted = Some(m.highlighted.map_or(0, |h| (h + 1) % n));
            cx.notify();
        } else if let Some(p) = self.palette.as_mut() {
            p.move_selection(1);
            cx.notify();
        }
    }

    pub(crate) fn on_overlay_confirm(&mut self, _: &OverlayConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.menu.as_ref().and_then(|m| m.highlighted) {
            self.pick_menu_item(h, window, cx);
        } else if let Some(d) = &self.dialog {
            let last = d.buttons.len().checked_sub(1);
            self.answer_dialog(last, window, cx);
        } else if self.palette.is_some() {
            self.confirm_palette(window, cx);
        }
    }

    pub(crate) fn on_overlay_cancel(&mut self, _: &OverlayCancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_menu(window, cx);
        } else if self.dialog.is_some() {
            self.answer_dialog(None, window, cx);
        } else if self.palette.is_some() {
            self.close_palette(window, cx);
        }
    }

    pub(crate) fn render_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.menu.as_ref()?;
        // Kept inside the window, like the React menu.
        let viewport = window.viewport_size();
        let height = menu.items.len() as f32 * size::MENU_ITEM + 2. * size::MENU_PAD + 2.;
        let x = menu.origin.x.min(viewport.width - px(size::MENU_MIN_W + 8.)).max(px(4.));
        let y = menu.origin.y.min(viewport.height - px(height + 8.)).max(px(4.));
        Some(
            div()
                .id("menu-backdrop")
                .key_context(CONTEXT)
                .track_focus(&self.overlay_focus)
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.close_menu(window, cx)))
                .child(
                    div()
                        .id("context-menu")
                        .occlude()
                        .absolute()
                        .left(x)
                        .top(y)
                        .min_w(px(size::MENU_MIN_W))
                        .p(px(size::MENU_PAD))
                        .bg(c(theme::PANEL_RAISED))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(8.))
                        .shadow(theme::shadow(12., 32., 0.5))
                        .children(menu.items.iter().enumerate().map(|(ix, item)| {
                            let lit = menu.highlighted == Some(ix);
                            let (hover_bg, text) = if item.danger {
                                (theme::danger_soft(), theme::DANGER)
                            } else {
                                (theme::accent_soft(), theme::TEXT)
                            };
                            div()
                                .id(("menu-item", ix))
                                .relative()
                                .h(px(size::MENU_ITEM))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded(px(5.))
                                .whitespace_nowrap()
                                .text_size(px(size::TAB_FONT))
                                .text_color(c(text))
                                .cursor_pointer()
                                .when(lit, |d| d.bg(hover_bg))
                                .hover(move |s| s.bg(hover_bg))
                                .child(item.label.clone())
                                .child(probe(format!("menu:{}", item.label)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, window, cx| this.pick_menu_item(ix, window, cx)))
                        })),
                ),
        )
    }

    pub(crate) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.dialog.as_ref()?;
        Some(
            div()
                .id("dialog-scrim")
                .key_context(CONTEXT)
                .track_focus(&self.overlay_focus)
                .absolute()
                .inset_0()
                .bg(theme::scrim())
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.answer_dialog(None, window, cx)))
                .child(
                    div()
                        .id("dialog")
                        .occlude()
                        .w(px(400.))
                        .bg(c(theme::PANEL))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(12.))
                        .shadow(theme::shadow(20., 60., 0.55))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .px(px(16.))
                                .py(px(18.))
                                .child(
                                    div()
                                        .mb(px(8.))
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c(theme::TEXT))
                                        .child(dialog.title.clone()),
                                )
                                .child(div().text_color(c(theme::TEXT_DIM)).line_height(px(20.)).child(dialog.message.clone())),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .px(px(16.))
                                .py(px(12.))
                                .border_t_1()
                                .border_color(c(theme::BORDER))
                                .children(dialog.buttons.iter().enumerate().map(|(ix, b)| {
                                    button(("dialog-button", ix), b.label.clone(), b.style)
                                        .child(probe(format!("dialog:{}", b.label)))
                                        .on_click(cx.listener(move |this, _, window, cx| this.answer_dialog(Some(ix), window, cx)))
                                })),
                        ),
                ),
        )
    }
}

/// `button` / `button.secondary` of App.css.
pub fn button(id: impl Into<gpui::ElementId>, label: SharedString, style: ButtonStyle) -> gpui::Stateful<gpui::Div> {
    let (bg, hover, text) = match style {
        ButtonStyle::Primary => (theme::ACCENT, theme::ACCENT_HOVER, theme::ON_ACCENT),
        ButtonStyle::Secondary => (theme::PANEL_RAISED, theme::PANEL_HOVER, theme::TEXT),
    };
    div()
        .id(id)
        .relative()
        .h(px(size::CONTROL))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(c(bg))
        .hover(move |s| s.bg(c(hover)))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(text))
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label)
}
