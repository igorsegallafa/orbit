//! Small pieces shared by the review views: buttons, pills, status letters.
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Div, ElementId, Rgba, SharedString, Stateful};

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Plain,
    Primary,
    Danger,
}

/// A compact text button; `name` is also its self-test probe.
pub fn button(name: impl Into<SharedString>, label: impl Into<SharedString>, tone: Tone) -> Stateful<Div> {
    let name: SharedString = name.into();
    let (bg, fg) = match tone {
        Tone::Primary => (c(theme::ACCENT), c(theme::ON_ACCENT)),
        Tone::Danger => (theme::danger_soft(), c(theme::DANGER)),
        Tone::Plain => (c(theme::PANEL_RAISED), c(theme::TEXT)),
    };
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .px_2()
        .py(px(2.))
        .rounded(px(5.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::ACCENT)))
        .child(label.into())
        .child(probe(name.to_string()))
}

/// A row-sized clickable area with a probe.
pub fn row(name: impl Into<SharedString>) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .px_2()
        .py(px(3.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_HOVER)))
        .child(probe(name.to_string()))
}

pub fn pill(label: impl Into<SharedString>, color: Rgba) -> Div {
    div().flex_none().px(px(5.)).rounded(px(8.)).border_1().border_color(color).text_color(color).text_xs().child(label.into())
}

/// Git status letter and its color (`STATUS_LABEL` in `GitPanel.tsx`).
pub fn status_letter(status: &str) -> (&'static str, Rgba) {
    match status {
        "A" => ("A", c(theme::OK)),
        "D" => ("D", c(theme::DANGER)),
        "U" => ("U", c(theme::OK)),
        _ => ("M", c(theme::WARN)),
    }
}

pub fn section(label: impl Into<SharedString>) -> Div {
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
        .child(label.into())
}

/// The bucket of a check or the aggregate of a PR's checks, as a color.
pub fn bucket_color(bucket: &str) -> Rgba {
    match bucket {
        "pass" => c(theme::OK),
        "fail" => c(theme::DANGER),
        "pending" | "queued" | "running" => c(theme::WARN),
        _ => c(theme::TEXT_FAINT),
    }
}
