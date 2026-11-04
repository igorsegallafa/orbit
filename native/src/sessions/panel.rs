//! The sidebar's sessions block: new-session buttons, the open sessions with
//! their status, the inbox, and the ended sessions of the scope to resume.
use super::{status::Status, InboxKind, SessionStore, AGENTS};
use crate::app::OrbitApp;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Context, Entity, SharedString, WeakEntity, Window};

pub struct SessionsPanel {
    store: Entity<SessionStore>,
    app: WeakEntity<OrbitApp>,
}

/// Recent sessions listed for the scope.
const RECENT_SHOWN: usize = 5;
const INBOX_SHOWN: usize = 8;

fn status_color(status: Status) -> u32 {
    match status {
        Status::Waiting => theme::WARN,
        Status::Exited => theme::DANGER,
        s if s.working() => theme::ACCENT,
        _ => theme::TEXT_FAINT,
    }
}

/// "now", "5m", "2h", "3d" since `at` (ms since the epoch).
fn ago(at: u64) -> String {
    let secs = super::now_ms().saturating_sub(at) / 1000;
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn dot(color: u32) -> impl IntoElement {
    div().size(px(7.)).flex_none().rounded_full().bg(c(color))
}

fn header(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(div().flex_1().child(label.into()))
}

fn link(id: impl Into<gpui::ElementId>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .cursor_pointer()
        .text_color(c(theme::TEXT_DIM))
        .hover(|s| s.text_color(c(theme::TEXT)))
        .font_weight(gpui::FontWeight::NORMAL)
        .child(label)
}

impl SessionsPanel {
    pub fn new(store: Entity<SessionStore>, app: WeakEntity<OrbitApp>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self { store, app }
    }

    /// Runs `f` on the app (a click's handler).
    fn on_app(&self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>)) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |a, cx| f(a, window, cx));
        }
    }
}

impl Render for SessionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.upgrade();
        let active = app.as_ref().and_then(|a| a.read(cx).active_terminal()).map(|t| t.read(cx).meta.id.clone());
        let scope = app.as_ref().map(|a| super::scope(a.read(cx))).unwrap_or_default();
        let store = self.store.read(cx);

        let new_buttons = div().px_3().pb_1().flex().flex_wrap().gap_x_2().text_xs().children(AGENTS.iter().enumerate().map(|(ix, a)| {
            let cmd = a.2;
            div()
                .id(("new-session", ix))
                .cursor_pointer()
                .text_color(c(theme::TEXT_DIM))
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(format!("+ {}", a.1))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.on_app(window, cx, |a, w, cx| super::new_session(a, cmd, None, w, cx))
                }))
        }));

        let sessions = store.sessions().filter_map(|(m, v)| v.map(|v| (m.clone(), v))).enumerate().map(|(ix, (meta, view))| {
            let v = view.read(cx);
            let status = v.status;
            let unread = store.unread(&meta.id);
            let selected = active.as_deref() == Some(meta.id.as_str());
            let key = meta.id.clone();
            let kind = if v.is_agent() { status.label().to_string() } else { "shell".into() };
            div()
                .id(("session", ix))
                .mx_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .items_center()
                .gap_2()
                .when(selected, |d| d.bg(theme::accent_soft()))
                .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                .child(dot(status_color(status)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().text_color(c(theme::TEXT)).overflow_hidden().whitespace_nowrap().child(meta.session_name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::TEXT_FAINT))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(format!("{} · {} · {kind}", meta.label, super::scope_label(&meta.workspace))),
                        ),
                )
                .children(unread.map(|k| dot(if k == InboxKind::Waiting { theme::WARN } else { theme::OK })))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let key = key.clone();
                    this.on_app(window, cx, move |a, w, cx| super::activate(a, &key, w, cx))
                }))
        });

        let unread = store.inbox.iter().filter(|i| !i.read).count();
        let inbox = store.inbox.iter().take(INBOX_SHOWN).enumerate().map(|(ix, item)| {
            let key = item.key.clone();
            let color = match item.kind {
                InboxKind::Waiting => theme::WARN,
                InboxKind::Done => theme::OK,
            };
            div()
                .id(("inbox", ix))
                .mx_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .items_center()
                .gap_2()
                .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                .child(dot(if item.read { theme::BORDER_STRONG } else { color }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(if item.read { theme::TEXT_DIM } else { theme::TEXT }))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::TEXT_FAINT))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(format!("{} · {}", item.detail, ago(item.at))),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let key = key.clone();
                    this.on_app(window, cx, move |a, w, cx| super::activate(a, &key, w, cx))
                }))
        });

        let recent: Vec<_> = store.recent.iter().filter(|r| r.workspace == scope).take(RECENT_SHOWN).cloned().collect();
        let recent_rows = recent.into_iter().enumerate().map(|(ix, r)| {
            let label = format!("↺ {} · {}", r.session_name, r.agent);
            div()
                .id(("recent", ix))
                .mx_2()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(c(theme::TEXT_DIM))
                .hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let r = r.clone();
                    this.on_app(window, cx, move |a, w, cx| super::resume(a, &r, w, cx))
                }))
        });

        let has_inbox = !store.inbox.is_empty();
        div()
            .id("sessions-panel")
            .flex()
            .flex_col()
            .child(header("SESSIONS"))
            .child(new_buttons)
            .children(sessions)
            .when(has_inbox, |d| {
                d.child(
                    header(if unread > 0 { format!("INBOX ({unread})") } else { "INBOX".into() })
                        .child(link("inbox-read", "read").on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.mark_all_read(cx)))))
                        .child(link("inbox-clear", "clear").on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.clear_inbox(cx))))),
                )
                .children(inbox)
            })
            .when(store.recent.iter().any(|r| r.workspace == scope), |d| d.child(header("RECENT")).children(recent_rows))
    }
}
