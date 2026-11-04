//! Widgets for the pages: the review's buttons and pills plus settings rows,
//! cards, segmented toggles and status dots (`settings-row`, `settings-card`,
//! `mode-toggle` in `App.css`).
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, AnyElement, Div, Entity, Rgba, SharedString, Stateful};
use gpui_component::input::{Input, InputState};

pub use crate::review::ui::{button, pill, row, Tone};

pub fn nav_title(label: &'static str) -> Div {
    div().px_3().pt_3().pb_1().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child(label)
}

/// A scrolling page body with a heading and a description.
pub fn page(id: &'static str, title: impl Into<SharedString>, description: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .px_6()
        .py_4()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_lg().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(title.into()))
                .child(muted(description)),
        )
}

pub fn heading(label: impl Into<SharedString>) -> Div {
    div().pt_2().flex().items_center().gap_2().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(label.into())
}

pub fn card() -> Div {
    div().flex().flex_col().rounded(px(theme::size::RADIUS)).border_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL))
}

/// A card row: title and description on the left, a control on the right.
pub fn setting(title: impl Into<SharedString>, description: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .px_4()
        .py_3()
        .flex()
        .items_center()
        .gap_4()
        .border_b_1()
        .border_color(c(theme::BORDER))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_sm().text_color(c(theme::TEXT)).child(title.into()))
                .child(muted(description)),
        )
        .child(div().flex_none().flex().items_center().gap_2().child(control))
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

pub fn code(text: impl Into<SharedString>) -> Div {
    div().font_family(theme::MONO_FONT).text_xs().text_color(c(theme::TEXT_DIM)).child(text.into())
}

pub fn dot(color: Rgba) -> Div {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

/// Segmented buttons; `name:<value>` is each one's probe.
pub fn segmented<T: Clone + PartialEq + 'static>(
    name: &str,
    options: &[(T, &str)],
    current: &T,
    on_pick: impl Fn(T, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
) -> Div {
    let mut d = div().flex().rounded(px(6.)).border_1().border_color(c(theme::BORDER_STRONG)).overflow_hidden();
    for (value, label) in options {
        let on = value == current;
        let (v, f) = (value.clone(), on_pick.clone());
        let id = format!("{name}:{}", label.to_lowercase().replace(' ', "-"));
        d = d.child(
            div()
                .id(SharedString::from(id.clone()))
                .relative()
                .px_3()
                .py(px(3.))
                .text_xs()
                .cursor_pointer()
                .when(on, |d| d.bg(c(theme::ACCENT)).text_color(c(theme::ON_ACCENT)))
                .when(!on, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_HOVER))))
                .child(label.to_string())
                .child(probe(id))
                .on_click(move |_, w, cx| f(v.clone(), w, cx)),
        );
    }
    d
}

/// A checkbox-like toggle with a label.
pub fn check(name: impl Into<SharedString>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(name.clone())
        .relative()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .text_xs()
        .text_color(c(theme::TEXT))
        .child(
            div()
                .size(px(14.))
                .rounded(px(3.))
                .border_1()
                .border_color(c(if on { theme::ACCENT } else { theme::BORDER_STRONG }))
                .when(on, |d| d.bg(c(theme::ACCENT)))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::ON_ACCENT))
                .text_size(px(10.))
                .when(on, |d| d.child("✓")),
        )
        .child(label.into())
        .child(probe(name.to_string()))
}

/// A text input with a probe (`click:<name>` focuses it in a self-test).
pub fn input(name: impl Into<String>, state: &Entity<InputState>) -> AnyElement {
    // Full width, shrinking in a row: growing would stretch it (and its
    // probe) past the input in a column.
    div().relative().w_full().min_w_0().child(Input::new(state)).child(probe(name.into())).into_any_element()
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
