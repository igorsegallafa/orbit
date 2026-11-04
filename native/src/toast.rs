//! Toasts, as `src/components/Toast.tsx`: a stack in the bottom-right corner,
//! at most four, each dismissed after a few seconds or by its ×.
use crate::app::OrbitApp;
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, Context, FontWeight, IntoElement, MouseButton, SharedString};
use gpui_component::{Icon, IconName};
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToastKind {
    #[allow(dead_code)] // for later stages (a push, a build)
    Success,
    Error,
    Info,
}

impl ToastKind {
    fn duration(self) -> Duration {
        Duration::from_millis(match self {
            ToastKind::Success => 4000,
            ToastKind::Info => 5000,
            ToastKind::Error => 8000,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: usize,
    pub kind: ToastKind,
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub action: Option<ToastAction>,
}

/// A toast's button (`action` in `Toast.tsx`); clicking it also dismisses the toast.
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub run: Rc<dyn Fn(&mut App)>,
}

impl std::fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToastAction").field("label", &self.label).finish()
    }
}

const MAX_VISIBLE: usize = 4;

#[derive(Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
    next: usize,
}

impl Toasts {
    fn push(&mut self, kind: ToastKind, title: SharedString, description: Option<SharedString>, action: Option<ToastAction>) -> usize {
        self.next += 1;
        self.items.push(Toast { id: self.next, kind, title, description, action });
        let excess = self.items.len().saturating_sub(MAX_VISIBLE);
        self.items.drain(..excess);
        self.next
    }

    pub fn dismiss(&mut self, id: usize) {
        self.items.retain(|t| t.id != id);
    }
}

impl OrbitApp {
    /// Shows a toast. `title` is the message; `description` an optional detail line.
    pub fn toast(&mut self, kind: ToastKind, title: impl Into<SharedString>, description: Option<SharedString>, cx: &mut Context<Self>) {
        let title = title.into();
        eprintln!("toast {kind:?}: {title}{}", description.as_ref().map(|d| format!(" — {d}")).unwrap_or_default());
        let id = self.toasts.push(kind, title, description, None);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(kind.duration()).await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.dismiss(id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A toast with a button; it stays until clicked or closed (`duration: 0`).
    pub fn toast_action(&mut self, title: impl Into<SharedString>, description: Option<SharedString>, action: ToastAction, cx: &mut Context<Self>) {
        let title = title.into();
        eprintln!("toast Info: {title}{} [{}]", description.as_ref().map(|d| format!(" — {d}")).unwrap_or_default(), action.label);
        self.toasts.push(ToastKind::Info, title, description, Some(action));
        cx.notify();
    }

    pub fn toast_error(&mut self, title: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast(ToastKind::Error, title, None, cx);
    }

    pub(crate) fn render_toasts(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.toasts.items.is_empty() {
            return None;
        }
        Some(
            div()
                .absolute()
                .right(px(18.))
                .bottom(px(36.))
                .w(px(380.))
                .flex()
                .flex_col()
                .gap_2()
                .children(self.toasts.items.iter().map(|t| {
                    let id = t.id;
                    let icon = match t.kind {
                        ToastKind::Success => div().bg(theme::ok_soft()).text_color(c(theme::OK)).child(Icon::new(IconName::Check).size_3()),
                        ToastKind::Error => {
                            div().bg(theme::danger_soft()).text_color(c(theme::DANGER)).child(Icon::new(IconName::Close).size_3())
                        }
                        ToastKind::Info => div().bg(c(theme::PANEL)).child(div().size(px(7.)).rounded_full().bg(c(theme::ACCENT))),
                    };
                    div()
                        .id(("toast", id))
                        .occlude()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(11.))
                        .border_1()
                        .border_color(match t.kind {
                            ToastKind::Error => theme::hsla(theme::DANGER).opacity(0.35),
                            _ => theme::hsla(theme::BORDER_STRONG),
                        })
                        .rounded(theme::radius())
                        .bg(c(theme::PANEL_RAISED))
                        .shadow(theme::shadow(10., 30., 0.45))
                        .child(icon.size(px(20.)).flex_none().rounded_full().flex().items_center().justify_center())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pt(px(1.))
                                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(t.title.clone()))
                                .children(t.description.clone().map(|d| {
                                    div().mt(px(2.)).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).line_height(px(17.)).child(d)
                                })),
                        )
                        .children(t.action.clone().map(|a| {
                            crate::pages::kit::mini(format!("toast-action:{id}"), a.label.clone(), crate::pages::kit::Kind::Primary).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.toasts.dismiss(id);
                                    cx.notify();
                                    let run = a.run.clone();
                                    window.defer(cx, move |_, cx| run(cx));
                                },
                            ))
                        }))
                        .child(
                            div()
                                .id(("toast-close", id))
                                .relative()
                                .size(px(20.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c(theme::TEXT_FAINT))
                                .hover(|s| s.text_color(c(theme::TEXT)))
                                .cursor_pointer()
                                .text_size(px(16.))
                                .child("×")
                                .child(probe(format!("toast-close:{id}")))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toasts.dismiss(id);
                                        cx.notify();
                                    }),
                                ),
                        )
                        // `toast-in`: 180ms, rising 8px.
                        .map(|el| crate::motion::rise_in(el, ("toast-in", id), crate::motion::TOAST_IN, 8., Default::default(), crate::motion::toast_curve()))
                })),
        )
    }
}
