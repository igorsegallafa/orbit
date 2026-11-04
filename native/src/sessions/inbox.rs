//! The titlebar bell (`Inbox.tsx`): unread count and a popover of the recent
//! agent events.
use super::{InboxKind, SessionStore};
use crate::app::OrbitApp;
use crate::probe::probe;
use crate::theme::{self, c, size};
use gpui::{
    anchored, deferred, div, prelude::*, px, svg, Context, Anchor, Entity, FontWeight, MouseButton, WeakEntity, Window,
};
use gpui_component::{IconName, IconNamed};

pub struct InboxButton {
    store: Entity<SessionStore>,
    app: WeakEntity<OrbitApp>,
    open: bool,
}

impl InboxButton {
    pub fn new(app: WeakEntity<OrbitApp>, cx: &mut Context<Self>) -> Self {
        let store = super::store(cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self { store, app, open: false }
    }

}

/// "just now", "5 min ago", "2 h ago", "3 d ago" (`timeAgo`).
fn ago(at: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    match now.saturating_sub(at) / 1000 {
        0..=59 => "just now".into(),
        s @ 60..=3599 => format!("{} min ago", s / 60),
        s @ 3600..=86399 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86400),
    }
}

impl Render for InboxButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let unread = store.inbox.iter().filter(|i| !i.read).count();
        let empty = store.inbox.is_empty();
        let link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(FontWeight::NORMAL)
                .text_color(c(theme::TEXT_FAINT))
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(label)
        };
        let items: Vec<_> = store
            .inbox
            .iter()
            .enumerate()
            .map(|(ix, it)| {
                let key = it.key.clone();
                let dot = match (it.read, it.kind) {
                    (true, _) => theme::TEXT_FAINT,
                    (false, InboxKind::Done) => theme::OK,
                    (false, InboxKind::Waiting) => theme::WARN,
                };
                div()
                    .id(("inbox-item", ix))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(c(if it.read { theme::TEXT_DIM } else { theme::TEXT }))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(div().mt(px(4.)).size(px(8.)).flex_none().rounded_full().bg(c(dot)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .min_w_0()
                            .child(div().text_size(px(12.5)).when(!it.read, |d| d.font_weight(FontWeight::SEMIBOLD)).child(it.title.clone()))
                            .child(div().truncate().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(format!("{} · {}", it.detail, ago(it.at)))),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open = false;
                        let key = key.clone();
                        if let Some(app) = this.app.upgrade() {
                            app.update(cx, |a, cx| super::activate(a, &key, window, cx));
                        }
                        cx.notify();
                    }))
            })
            .collect();
        let pop = div()
            .id("inbox-pop")
            .occlude()
            .mt(px(6.))
            .w(px(340.))
            .rounded(px(10.))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .bg(c(theme::PANEL))
            .shadow(theme::shadow(16., 40., 0.5))
            .overflow_hidden()
            .font_family(theme::UI_FONT)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open = false;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .text_size(px(12.5))
                    .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Notifications"))
                    .when(!empty, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .when(unread > 0, |d| {
                                    d.child(link("inbox-read", "Mark all read").on_click(cx.listener(|this, _, _, cx| {
                                        this.store.update(cx, |s, cx| s.mark_all_read(cx));
                                    })))
                                })
                                .child(link("inbox-clear", "Clear").on_click(cx.listener(|this, _, _, cx| {
                                    this.store.update(cx, |s, cx| s.clear_inbox(cx));
                                }))),
                        )
                    }),
            )
            .map(|d| {
                if empty {
                    d.child(
                        div()
                            .px(px(14.))
                            .py(px(18.))
                            .text_size(px(12.))
                            .line_height(gpui::relative(1.5))
                            .text_color(c(theme::TEXT_FAINT))
                            .child("You'll hear here when an agent finishes or needs you."),
                    )
                } else {
                    d.child(div().id("inbox-list").max_h(px(360.)).overflow_y_scroll().p(px(4.)).children(items))
                }
            });
        let open = self.open;
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id("tb-inbox")
                    .relative()
                    .w(px(size::TITLEBAR_BTN_W))
                    .h(px(size::TITLEBAR_BTN_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(open, |d| d.bg(c(theme::PANEL_RAISED)))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(svg().path(IconName::Bell.path()).size(px(14.)).text_color(c(if open { theme::TEXT } else { theme::TEXT_DIM })))
                    .when(unread > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-3.))
                                .right(px(-1.))
                                .min_w(px(14.))
                                .h(px(14.))
                                .px(px(3.))
                                .rounded_full()
                                .bg(c(theme::ACCENT))
                                .text_color(c(theme::ON_ACCENT))
                                .text_size(px(9.))
                                .font_weight(FontWeight::BOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(if unread > 9 { "9+".to_string() } else { unread.to_string() }),
                        )
                    })
                    .child(probe("tb-inbox"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.open = !this.open;
                            cx.notify();
                        }),
                    ),
            )
            .when(open, |d| {
                d.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(size::TITLEBAR_BTN_H))
                        .child(deferred(anchored().anchor(Anchor::TopRight).snap_to_window().child(pop)).with_priority(2)),
                )
            })
    }
}
