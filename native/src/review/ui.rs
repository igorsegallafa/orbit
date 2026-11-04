//! Small pieces shared by the review views: buttons, pills, status letters.
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Div, ElementId, Rgba, SharedString, Stateful};
use gpui_component::{Icon, IconName};

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

type Handler = Box<dyn Fn(&mut gpui::Window, &mut gpui::App)>;
type SplitHandler = Box<dyn Fn(bool, &mut gpui::Window, &mut gpui::App)>;

/// What [`diff_filebar`] shows and does.
pub struct DiffBar {
    /// Mono path text: `sha · repo/path`.
    pub label: String,
    /// `(current change, changes)`, when there are changes to step through.
    pub nav: Option<(Option<usize>, usize)>,
    pub split: bool,
    pub prev: Handler,
    pub next: Handler,
    pub set_split: SplitHandler,
    /// Shown before the change navigation.
    pub extra: Option<gpui::AnyElement>,
}

/// `.editor-filebar` of `ReviewPane.tsx`: the path, ‹ n/m › and the split / unified toggle.
pub fn diff_filebar(bar: DiffBar) -> Div {
    let DiffBar { label, nav, split, prev, next, set_split, extra } = bar;
    let set_split = std::rc::Rc::new(set_split);
    let nav_btn = |id: &'static str, glyph: &'static str, f: Handler| {
        div()
            .id(id)
            .relative()
            .h(px(22.))
            .w(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .text_color(c(theme::TEXT_DIM))
            .text_size(px(11.5))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|s| s.text_color(c(theme::TEXT)))
            .child(glyph)
            .child(probe(id))
            .on_click(move |_, w, cx| f(w, cx))
    };
    let mode_btn = |id: &'static str, glyph: &'static str, on: bool, value: bool| {
        let set = set_split.clone();
        div()
            .id(id)
            .relative()
            .h(px(22.))
            .w(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.5))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .cursor_pointer()
            .map(|d| if on { d.bg(theme::accent_soft()).text_color(c(theme::TEXT)) } else { d.text_color(c(theme::TEXT_FAINT)).hover(|s| s.text_color(c(theme::TEXT_DIM))) })
            .child(glyph)
            .child(probe(id))
            .on_click(move |_, w, cx| set(value, w, cx))
    };
    div()
        .min_h(px(30.))
        .flex_none()
        .px(px(10.))
        .py(px(3.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .bg(c(theme::PANEL))
        .border_b_1()
        .border_color(c(theme::BORDER))
        .child(div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(label))
        .child(
            div()
                .flex()
                .items_center()
                .flex_none()
                .gap(px(10.))
                .children(extra)
                .when_some(nav, |d, (ix, count)| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .mr(px(8.))
                            .child(nav_btn("diff-prev", "‹", prev))
                            .child(
                                div()
                                    .min_w(px(34.))
                                    .flex()
                                    .justify_center()
                                    .text_size(px(11.))
                                    .text_color(c(theme::TEXT_DIM))
                                    .child(format!("{}/{count}", ix.map_or("–".to_string(), |i| (i + 1).to_string()))),
                            )
                            .child(nav_btn("diff-next", "›", next)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .bg(c(theme::BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .child(mode_btn("diff-split", "⇄", split, true))
                        .child(mode_btn("diff-unified", "↕", !split, false)),
                ),
        )
}

/// Runs of changed lines in a patch: what ‹ › steps through.
pub fn change_blocks(patch: &str) -> usize {
    let mut blocks = 0;
    let mut in_change = false;
    let mut in_hunk = false;
    for l in patch.lines() {
        if l.starts_with("@@") {
            in_hunk = true;
            in_change = false;
            continue;
        }
        if !in_hunk {
            continue;
        }
        let changed = l.starts_with('+') || l.starts_with('-');
        if changed && !in_change {
            blocks += 1;
        }
        in_change = changed;
    }
    blocks
}

/// Any Lucide icon by file name (`trash`), tinted.
pub fn lucide(name: &str, size: f32, color: Rgba) -> gpui::Svg {
    gpui::svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

/// `color-mix(in srgb, <color> <alpha>, transparent)`.
pub fn tint(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// `.git-status-badge`: the 14px letter square of a changed file.
pub fn status_badge(status: &str) -> Div {
    let (letter, color) = match status {
        "A" => ("A", c(theme::OK)),
        "D" => ("D", c(theme::DANGER)),
        "U" => ("U", c(theme::INFO)),
        _ => ("M", c(theme::WARN)),
    };
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .bg(tint(color, 0.12))
        .text_color(color)
        .text_size(px(9.))
        .line_height(px(9.))
        .font_weight(gpui::FontWeight::BOLD)
        .child(letter)
}

/// `.orbit-check`: a 16px checkbox; `name` is its probe.
pub fn checkbox(name: impl Into<SharedString>, checked: bool) -> Stateful<Div> {
    let name: SharedString = name.into();
    let accent = c(theme::ACCENT);
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border(px(1.5))
        .cursor_pointer()
        .map(|d| if checked { d.border_color(accent).bg(accent) } else { d.border_color(c(theme::TEXT_FAINT)).bg(c(theme::BG)) })
        .hover(move |s| s.border_color(accent))
        .when(checked, |d| d.child(Icon::new(IconName::Check).size(px(11.)).text_color(c(theme::ON_ACCENT))))
        .child(probe(name.to_string()))
}

/// `.btn-mini`: a 24px button, primary or `.secondary`.
pub fn mini_button(name: impl Into<SharedString>, label: impl Into<SharedString>, primary: bool, enabled: bool) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .h(px(24.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .border_1()
        .text_size(px(11.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .map(|d| {
            if primary {
                d.bg(c(theme::ACCENT)).border_color(tint(c(theme::ACCENT), 0.)).text_color(c(theme::ON_ACCENT))
            } else {
                d.bg(c(theme::PANEL_RAISED)).border_color(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT))
            }
        })
        .when(!enabled, |d| d.opacity(0.5))
        .when(enabled, |d| d.cursor_pointer().hover(move |s| if primary { s.bg(c(theme::ACCENT_HOVER)) } else { s.bg(c(theme::PANEL_HOVER)) }))
        .child(label.into())
        .child(probe(name.to_string()))
}

/// `.btn-link`: accent text that underlines on hover.
pub fn link(name: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .cursor_pointer()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(c(theme::ACCENT))
        .hover(|s| s.text_color(c(theme::ACCENT_HOVER)).underline())
        .child(label.into())
        .child(probe(name.to_string()))
}

/// `.icon-button`: 28px and bordered.
pub fn icon_button(name: impl Into<SharedString>, icon: IconName, size: f32) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
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
        .child(Icon::new(icon).size(px(size)).text_color(c(theme::TEXT_DIM)))
        .child(probe(name.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_change_blocks() {
        let patch = "--- a/x\n+++ b/x\n@@ -1,6 +1,6 @@\n a\n-b\n+B\n c\n d\n-e\n+E\n+F\n";
        assert_eq!(super::change_blocks(patch), 2);
        assert_eq!(super::change_blocks("--- a/x\n+++ b/x\n"), 0);
    }
}
