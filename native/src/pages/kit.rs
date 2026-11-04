//! The Tauri app's controls for the settings-style pages: `settings-card` /
//! `settings-row`, `button` / `button.secondary`, `.mode-toggle.seg`,
//! `.select-trigger`, `.orbit-check`, `.tag`, `.nav-item` (`App.css`).
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{
    div, prelude::*, px, rgba, App, Div, Entity, Focusable, FontWeight, Hsla, Rgba, SharedString, Stateful, Svg, Window,
};
use gpui_component::input::{Input, InputState};

/// One of `Icons.tsx`'s icons.
pub fn icon(name: &'static str, size: f32, color: impl Into<Hsla>) -> Svg {
    crate::icons::orbit_icon(name, size, color)
}

fn channels(tok: u32) -> [f32; 3] {
    let hex = theme::hex(tok);
    [((hex >> 16) & 0xff) as f32, ((hex >> 8) & 0xff) as f32, (hex & 0xff) as f32]
}

/// `color-mix(in srgb, a p, b)` of two tokens (or literal colours).
pub fn mix(a: u32, b: u32, p: f32) -> Rgba {
    let (a, b) = (channels(a), channels(b));
    let ch = |i: usize| ((a[i] * p + b[i] * (1. - p)).round() as u32).min(255);
    rgba((ch(0) << 24) | (ch(1) << 16) | (ch(2) << 8) | 0xff)
}

/// A token at `alpha` (0–1).
pub fn alpha(tok: u32, alpha: f32) -> Rgba {
    theme::alpha(tok, alpha)
}

/// Active nav text and `.mode-active`: `color-mix(info 40%, text)`.
pub fn active_text() -> Rgba {
    c(theme::NAV_ACTIVE)
}

// ---------- page frame ----------

/// `.nav-page` + `.settings-page`: a scrolling area with a centered 1080px column.
pub fn page(id: impl Into<SharedString>, column: Div) -> Stateful<Div> {
    div().id(id.into()).size_full().overflow_y_scroll().px(px(44.)).py(px(28.)).child(column)
}

/// The column with `.settings-head` (title, description, rule) on top.
pub fn column(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div().w_full().max_w(px(1080.)).mx_auto().flex().flex_col().child(
        div()
            .pb(px(18.))
            .mb(px(22.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().text_size(px(20.)).font_weight(FontWeight(650.)).text_color(c(theme::TEXT)).child(title.into()))
            .child(div().mt(px(6.)).text_size(px(13.)).line_height(px(19.5)).text_color(c(theme::TEXT_FAINT)).child(description.into())),
    )
}

/// `.section`: children 14px apart.
pub fn section() -> Div {
    div().flex().flex_col().gap(px(14.))
}

/// `.section-header`: uppercase title, optional count pill, actions on the right.
pub fn section_header(title: &str, count: Option<usize>, actions: Option<Div>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child(title.to_uppercase()))
                .when_some(count, |d, n| {
                    d.child(
                        div()
                            .ml(px(6.))
                            .px(px(7.))
                            .py(px(1.))
                            .rounded_full()
                            .bg(c(theme::PANEL_RAISED))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::TEXT_FAINT))
                            .child(n.to_string()),
                    )
                }),
        )
        .children(actions.map(|a| a.flex().items_center().gap(px(8.))))
}

/// `.settings-card`.
pub fn card() -> Div {
    div().flex().flex_col().rounded(px(theme::size::RADIUS)).border_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL)).overflow_hidden()
}

/// A card whose rows are split by 1px rules (`.settings-row + .settings-row`).
pub fn rows(rows: impl IntoIterator<Item = Div>) -> Div {
    card().children(rows.into_iter().enumerate().map(|(i, r)| r.when(i > 0, |r| r.border_t_1().border_color(c(theme::BORDER)))))
}

/// `.settings-row`: title and description left, the control right.
pub fn setting(title: impl Into<SharedString>, description: impl IntoElement, control: impl IntoElement) -> Div {
    div()
        .px(px(20.))
        .py(px(18.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .child(row_text(title, description))
        .child(div().flex_none().flex().items_center().justify_end().gap(px(12.)).child(control))
}

/// `.settings-row-text`.
pub fn row_text(title: impl Into<SharedString>, description: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .min_w_0()
        .max_w(px(560.))
        .child(div().text_size(px(13.5)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(title.into()))
        .child(description)
}

/// `.settings-row-text span`: 12px, faint, 1.5 line height.
pub fn hint(text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).line_height(px(18.)).text_color(c(theme::TEXT_FAINT)).child(text.into())
}

/// `.settings-error`.
pub fn error_box(text: impl Into<SharedString>) -> Div {
    div()
        .mt(px(6.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(theme::danger_soft())
        .font_family(theme::MONO_FONT)
        .text_size(px(11.5))
        .text_color(c(theme::DANGER))
        .child(text.into())
}

// ---------- buttons ----------

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// `button`: accent.
    Primary,
    /// `button.secondary`.
    Secondary,
    /// `button.secondary.danger-outline`.
    DangerOutline,
}

/// A 34px button (`--control-height`); `name` is its probe.
pub fn button(name: impl Into<SharedString>, label: impl Into<SharedString>, kind: Kind) -> Stateful<Div> {
    sized_button(name, None, label, kind, 34., 13., 14.)
}

/// `.btn-mini`: 24px.
pub fn mini(name: impl Into<SharedString>, label: impl Into<SharedString>, kind: Kind) -> Stateful<Div> {
    sized_button(name, None, label, kind, 24., 11.5, 10.).rounded(px(5.))
}

fn sized_button(name: impl Into<SharedString>, icon_name: Option<&'static str>, label: impl Into<SharedString>, kind: Kind, h: f32, text: f32, pad: f32) -> Stateful<Div> {
    let name: SharedString = name.into();
    let el = div()
        .id(name.clone())
        .relative()
        .flex_none()
        .h(px(h))
        .px(px(pad))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(6.))
        .border_1()
        .text_size(px(text))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer();
    let el = match kind {
        Kind::Primary => el
            .bg(c(theme::ACCENT))
            .border_color(c(theme::ACCENT))
            .text_color(c(theme::ON_ACCENT))
            .hover(|s| s.bg(c(theme::ACCENT_HOVER)).border_color(c(theme::ACCENT_HOVER))),
        Kind::Secondary => el
            .bg(c(theme::PANEL_RAISED))
            .border_color(c(theme::PANEL_RAISED))
            .text_color(c(theme::TEXT))
            .hover(|s| s.bg(c(theme::PANEL_HOVER)).border_color(c(theme::PANEL_HOVER))),
        Kind::DangerOutline => el
            .bg(c(theme::PANEL_RAISED))
            .border_color(alpha(theme::DANGER, 0.35))
            .text_color(c(theme::DANGER))
            .hover(|s| s.bg(theme::danger_soft())),
    };
    let fg = if kind == Kind::Primary { c(theme::ON_ACCENT) } else if kind == Kind::DangerOutline { c(theme::DANGER) } else { c(theme::TEXT) };
    el.children(icon_name.map(|i| icon(i, 14., fg))).child(label.into()).child(probe(name.to_string()))
}

/// A button with a leading 14px icon.
pub fn icon_label_button(name: impl Into<SharedString>, icon_name: &'static str, label: impl Into<SharedString>, kind: Kind) -> Stateful<Div> {
    sized_button(name, Some(icon_name), label, kind, 34., 13., 14.)
}

/// `.icon-button`: 28px, bordered.
pub fn icon_button(name: impl Into<SharedString>, icon_name: &'static str) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(name.clone())
        .relative()
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
        .child(icon(icon_name, 14., c(theme::TEXT_DIM)))
        .child(probe(name.to_string()))
}

/// `button.link`: accent text.
pub fn link(name: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(name.clone())
        .relative()
        .flex_none()
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(theme::ACCENT))
        .cursor_pointer()
        .hover(|s| s.bg(theme::accent_soft()))
        .child(label.into())
        .child(probe(name.to_string()))
}

/// Dims a disabled control (`button:disabled`).
pub fn disabled<E: Styled>(el: E, off: bool) -> E {
    if off {
        el.opacity(0.45)
    } else {
        el
    }
}

/// `.mode-toggle.seg`; `name:<label>` is each button's probe.
pub fn seg<T: Clone + PartialEq + 'static>(
    name: &str,
    options: &[(T, &str)],
    current: &T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
) -> Div {
    let mut d = div().flex().bg(c(theme::BG)).border_1().border_color(c(theme::BORDER)).rounded(px(6.)).overflow_hidden();
    for (value, label) in options {
        let on = value == current;
        let (v, f) = (value.clone(), on_pick.clone());
        let id = format!("{name}:{}", label.to_lowercase().replace(' ', "-"));
        d = d.child(
            div()
                .id(SharedString::from(id.clone()))
                .relative()
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(theme::accent_soft()).text_color(active_text()))
                .when(!on, |d| d.text_color(c(theme::TEXT_FAINT)).hover(|s| s.text_color(c(theme::TEXT_DIM))))
                .child(label.to_string())
                .child(probe(id))
                .on_click(move |_, w, cx| f(v.clone(), w, cx)),
        );
    }
    d
}

/// `.select-trigger`: a closed select showing `label` (opens a menu on click).
pub fn select(name: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(name.clone())
        .relative()
        .w_full()
        .h(px(theme::size::CONTROL))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::BG))
        .text_size(px(13.))
        .text_color(c(theme::TEXT))
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::ACCENT)))
        .child(div().min_w_0().truncate().child(label.into()))
        .child(div().text_size(px(9.)).text_color(c(theme::TEXT_FAINT)).child("▼"))
        .child(probe(name.to_string()))
}

/// `.orbit-check`: the 16px box alone.
pub fn checkbox(on: bool) -> Div {
    div()
        .flex_none()
        .size(px(16.))
        .rounded(px(5.))
        .border(px(1.5))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| d.bg(c(theme::ACCENT)).border_color(c(theme::ACCENT)).child(icon("check", 12., c(theme::ON_ACCENT))))
        .when(!on, |d| d.bg(c(theme::BG)).border_color(c(theme::TEXT_FAINT)))
}

/// A checkbox with its label (`.lang-enabled`): 12px, dim.
pub fn check(name: impl Into<SharedString>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(name.clone())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(c(theme::TEXT_DIM))
        .child(checkbox(on))
        .child(label.into())
        .child(probe(name.to_string()))
}

/// A text input styled like the app's `input` (34px, `--bg`, strong border;
/// accent border while focused).
pub fn field(name: impl Into<String>, state: &Entity<InputState>, window: &Window, cx: &App) -> Div {
    field_sized(name, state, theme::size::CONTROL, 13., window, cx)
}

pub fn field_sized(name: impl Into<String>, state: &Entity<InputState>, h: f32, text: f32, window: &Window, cx: &App) -> Div {
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    div()
        .relative()
        .w_full()
        .min_w_0()
        .h(px(h))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(if focused { theme::ACCENT } else { theme::BORDER_STRONG }))
        .bg(c(theme::BG))
        .text_size(px(text))
        .child(Input::new(state).appearance(false).px(px(0.)).text_size(px(text)).w_full())
        .child(probe(name.into()))
}

/// `.tag`.
pub fn tag(label: impl Into<SharedString>, muted: bool) -> Div {
    div()
        .flex_none()
        .px(px(9.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::PANEL_RAISED))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(if muted { theme::TEXT_FAINT } else { theme::TEXT_DIM }))
        .child(label.into())
}

/// `.repo-tag` / `.lang-state`: a small outlined pill.
pub fn outline_pill(label: impl Into<SharedString>, text: Rgba, border: Rgba) -> Div {
    div()
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(border)
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(text)
        .child(label.into())
}

/// `.empty-state.small`: a dashed box with centered faint text.
pub fn empty(text: impl Into<SharedString>) -> Div {
    div()
        .p(px(24.))
        .rounded(px(theme::size::RADIUS))
        .border_1()
        .border_dashed()
        .border_color(c(theme::BORDER_STRONG))
        .flex()
        .justify_center()
        .child(div().max_w(px(440.)).text_center().text_size(px(13.5)).line_height(px(21.)).text_color(c(theme::TEXT_FAINT)).child(text.into()))
}

pub use crate::motion::skeleton;


// ---------- nav ----------

/// `.nav-section`.
pub fn nav_section(label: &str) -> Div {
    div()
        .px(px(10.))
        .pt(px(10.))
        .pb(px(6.))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(label.to_uppercase())
}

/// `.nav-item`: 32px, icon slot 18px, 13px/500; active is accent-soft.
pub fn nav_item(name: impl Into<SharedString>, icon_name: &'static str, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    let name: SharedString = name.into();
    let color = if active { active_text() } else { c(theme::TEXT_DIM) };
    div()
        .id(name.clone())
        .relative()
        .flex_none()
        .h(px(32.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .cursor_pointer()
        .when(active, |d| d.bg(theme::accent_soft()))
        .when(!active, |d| d.hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT))))
        .child(div().w(px(18.)).flex_none().flex().justify_center().child(icon(icon_name, 15., color)))
        .child(label.into())
        .child(probe(name.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn mixes_like_css() {
        assert_eq!(super::mix(0xffffff, 0x000000, 0.4), gpui::rgba(0x666666ff));
    }
}
