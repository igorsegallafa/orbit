//! The pages' controls, as App.css draws them: buttons (`button`,
//! `.secondary`, `.btn-mini`, `.link`, `.icon-button`), badges, the Orbit
//! checkbox, section headings and tooltips.
use super::icons::icon;
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Div, ElementId, FontWeight, Rgba, SharedString, Stateful};
use gpui_component::tooltip::Tooltip;

/// `color` at `a` opacity.
pub fn alpha(color: Rgba, a: f32) -> Rgba {
    Rgba { a: color.a * a, ..color }
}

/// `color-mix(in srgb, a t, b)`.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let m = |x: f32, y: f32| x * t + y * (1. - t);
    Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: m(a.a, b.a) }
}

pub const W_650: FontWeight = FontWeight(650.);

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// `button`: accent fill.
    Primary,
    /// `button.secondary`.
    Secondary,
}

/// Metrics of a button variant: height, font size, horizontal padding, radius.
#[derive(Clone, Copy)]
pub struct Size(pub f32, pub f32, pub f32, pub f32);

/// `button` (control height 34).
pub const NORMAL: Size = Size(34., 13., 14., 6.);
/// `.ws-tool` (32, 12.5).
pub const TOOL: Size = Size(32., 12.5, 12., 6.);
/// `.flow-btn` (28, 12.5).
pub const FLOW: Size = Size(28., 12.5, 14., 6.);
/// `.ws-section-head button` (30, 12.5).
pub const SECTION: Size = Size(30., 12.5, 14., 6.);
/// `.btn-mini` (24, 11.5).
pub const MINI: Size = Size(24., 11.5, 10., 5.);

fn colors(kind: Kind) -> (Rgba, Rgba, Rgba) {
    match kind {
        Kind::Primary => (c(theme::ACCENT), c(theme::ACCENT_HOVER), c(theme::ON_ACCENT)),
        Kind::Secondary => (c(theme::PANEL_RAISED), c(theme::PANEL_HOVER), c(theme::TEXT)),
    }
}

/// A text button, optionally with a leading icon; `id` is also its probe.
pub fn button(id: impl Into<SharedString>, label: impl Into<SharedString>, kind: Kind, size: Size) -> Stateful<Div> {
    button_icon(id, None, label, kind, size)
}

/// A button with a leading icon (`name`, px).
pub fn button_icon(id: impl Into<SharedString>, icon_spec: Option<(&'static str, f32)>, label: impl Into<SharedString>, kind: Kind, size: Size) -> Stateful<Div> {
    let id: SharedString = id.into();
    let (bg, hover, fg) = colors(kind);
    let Size(h, font, pad, radius) = size;
    div()
        .id(ElementId::Name(id.clone()))
        .relative()
        .flex_none()
        .h(px(h))
        .px(px(pad))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(if size.0 == TOOL.0 { 7. } else { 6. }))
        .rounded(px(radius))
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(bg)
        .text_color(fg)
        .text_size(px(font))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(icon_spec, |d, (name, sz)| d.child(icon(name, sz, fg)))
        .child(label.into())
        .child(probe(id.to_string()))
}

/// Dims a button and drops its pointer (`button:disabled`).
pub fn disabled(b: Stateful<Div>, off: bool) -> Stateful<Div> {
    if off {
        b.opacity(0.45).cursor_default()
    } else {
        b
    }
}

/// `button.link`: accent text, accent-soft on hover.
pub fn link(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(ElementId::Name(id.clone()))
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
        .child(probe(id.to_string()))
}

/// `.icon-button`: bordered, square, an icon in text-dim (text and a fill on hover).
pub fn icon_button(id: impl Into<SharedString>, name: &'static str, side: f32, icon_size: f32, danger: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    let hover_fg = if danger { c(theme::DANGER) } else { c(theme::TEXT) };
    div()
        .id(ElementId::Name(id.clone()))
        .group("icon-button")
        .relative()
        .flex_none()
        .size(px(side))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
        .child(icon(name, icon_size, c(theme::TEXT_DIM)).group_hover("icon-button", move |s| s.text_color(hover_fg)))
        .child(probe(id.to_string()))
}

/// The custom tooltip on any element.
pub fn hint(el: Stateful<Div>, text: impl Into<SharedString>) -> Stateful<Div> {
    let text: SharedString = text.into();
    el.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

/// `.dash-repo-dot`: a 6px status dot.
pub fn dot(color: Rgba) -> Div {
    div().flex_none().size(px(6.)).rounded_full().bg(color)
}

/// `.ws-badge`: a pill on the page background.
pub fn badge(color: Rgba) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::BG))
        .text_size(px(11.5))
        .line_height(px(15.))
        .text_color(color)
}

/// `.orbit-check`: the 16px checkbox (`mixed` draws a dash).
pub fn check(checked: bool, mixed: bool) -> Div {
    let on = checked || mixed;
    div()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border(px(1.5))
        .border_color(if on { c(theme::ACCENT) } else { c(theme::TEXT_FAINT) })
        .bg(if on { c(theme::ACCENT) } else { c(theme::BG) })
        .when(checked, |d| d.child(icon("tick", 10., c(theme::ON_ACCENT))))
        .when(mixed && !checked, |d| d.child(icon("dash", 10., c(theme::ON_ACCENT))))
}

/// `.ws-section-head h3`: 12px uppercase, text-dim.
pub fn section_title(text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child(text.into().to_uppercase())
}

/// `.section-count`: a small count pill next to a heading.
pub fn count(n: usize) -> Div {
    div()
        .ml(px(6.))
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .bg(c(theme::PANEL_RAISED))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(n.to_string())
}

/// `.spinner` / `SpinnerIcon`: a ring with a turning arc.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: Rgba) -> impl IntoElement {
    use gpui::{percentage, Animation, AnimationExt, Transformation};
    icon("spinner", size, color).with_animation(id, Animation::new(crate::motion::SPIN).repeat_synced(), |s, t| {
        s.with_transformation(Transformation::rotate(percentage(t)))
    })
}

/// `.modal`: the card (width px); the caller adds header, body and footer.
pub fn modal(id: &'static str, width: f32) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .w(px(width))
        .max_h(px(760.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(12.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL))
        .shadow(theme::shadow(20., 60., 0.55))
        .text_color(c(theme::TEXT))
}

/// `.modal-header h3`: 14px semibold over a bottom border.
pub fn modal_header(title: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(16.))
        .py(px(14.))
        .border_b_1()
        .border_color(c(theme::BORDER))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

/// `.modal-footer`: right-aligned buttons over a top border.
pub fn modal_footer() -> Div {
    div().flex_none().flex().items_center().justify_end().gap(px(8.)).px(px(16.)).py(px(12.)).border_t_1().border_color(c(theme::BORDER))
}
