//! Widgets for the workspace pages (usage, artifacts): the review's buttons,
//! headings, cards and text styles, plus helpers. The settings-style controls
//! are in `kit.rs`.
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Div, SharedString, Stateful};

pub use crate::review::ui::{button, row, Tone};

/// A scrolling page body (`.nav-page`) with `.page-header` and a description.
pub fn page(id: &'static str, title: impl Into<SharedString>, description: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .px(px(44.))
        .py(px(28.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(px(19.)).font_weight(gpui::FontWeight(650.)).text_color(c(theme::TEXT)).child(title.into()))
                .child(div().text_size(px(13.)).text_color(c(theme::TEXT_FAINT)).child(description.into())),
        )
}

/// `.section-header h3`: 13px, uppercase, dim.
pub fn heading(label: impl Into<SharedString>) -> Div {
    let label: SharedString = label.into();
    div()
        .pt(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_DIM))
        .child(label.to_uppercase())
}

pub fn card() -> Div {
    div().flex().flex_col().rounded(px(theme::size::RADIUS)).border_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL))
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_xs().text_color(c(theme::TEXT_DIM)).child(text.into())
}

pub fn faint(text: impl Into<SharedString>) -> Div {
    div().text_xs().text_color(c(theme::TEXT_FAINT)).child(text.into())
}

pub fn error(text: impl Into<SharedString>) -> Div {
    div().text_xs().text_color(c(theme::DANGER)).child(text.into())
}

pub fn load_label<T>(l: &crate::review::Load<T>) -> String {
    use crate::review::Load;
    match l {
        Load::Idle => "idle".into(),
        Load::Loading => "loading".into(),
        Load::Ready(_) => "ready".into(),
        Load::Failed(e) => format!("failed: {e}"),
    }
}

/// Runs `work` off the UI thread, then `done` on the view.
pub fn bg<V: 'static, T: Send + 'static>(
    cx: &mut gpui::Context<V>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut V, T, &mut gpui::Window, &mut gpui::Context<V>) + 'static,
    window: &mut gpui::Window,
) {
    cx.spawn_in(window, async move |this, cx| {
        let r = cx.background_spawn(async move { work() }).await;
        let _ = this.update_in(cx, |v, window, cx| {
            done(v, r, window, cx);
            cx.notify();
        });
    })
    .detach();
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "3m ago" from a unix time.
pub fn ago(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let s = (now() as i64 - unix).max(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ages_read_like_the_react_app() {
        let now = super::now() as i64;
        assert_eq!(super::ago(0), "");
        assert_eq!(super::ago(now - 5), "just now");
        assert_eq!(super::ago(now - 125), "2m ago");
        assert_eq!(super::ago(now - 7200), "2h ago");
        assert_eq!(super::ago(now - 3 * 86_400), "3d ago");
    }
}
