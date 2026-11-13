//! Pieces shared by Code Review, the feature page and the PR review tab
//! (`cr-*`, `ws-*`, `pr-*`, `rv-*` rules in `App.css`).
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, svg, Div, ElementId, FontWeight, Rgba, SharedString, Stateful, Svg};
use gpui_component::tooltip::Tooltip;

/// Orbit's own glyphs (`icons.rs`, from `Icons.tsx`).
const GLYPHS: [&str; 9] = ["git", "commit", "push", "pr", "repo", "trash", "ring", "diff", "doc"];

/// An icon: Orbit's glyph of that name, else Lucide from gpui-kit-assets.
pub fn lucide(name: &str, size: f32, color: Rgba) -> Svg {
    let path = if GLYPHS.contains(&name) { format!("filetypes/{name}.svg") } else { format!("icons/{name}.svg") };
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

/// `color-mix(in srgb, <hex> <alpha>, transparent)`.
pub fn tint(hex: u32, alpha: f32) -> Rgba {
    let mut col = c(hex);
    col.a = alpha;
    col
}

/// `.btn-mini.secondary`: 24px, 11.5px/500.
pub fn btn_mini(name: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
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
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(theme::TEXT_DIM))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::BG)).text_color(c(theme::TEXT)))
        .child(label.into())
        .child(probe(name.to_string()))
}

/// `button.secondary` / `button` (primary) at a given height.
pub fn button(name: impl Into<SharedString>, label: impl Into<SharedString>, primary: bool, height: f32) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .h(px(height))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(7.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |d| d.bg(c(theme::ACCENT)).text_color(c(theme::ON_ACCENT)).hover(|s| s.bg(c(theme::ACCENT_HOVER))))
        .when(!primary, |d| {
            d.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)).hover(|s| s.bg(c(theme::PANEL_HOVER)))
        })
        .child(label.into())
        .child(probe(name.to_string()))
}

/// `.icon-button`: a bordered square with an icon.
pub fn icon_button(name: impl Into<SharedString>, icon: &str, size: f32, icon_size: f32) -> Stateful<Div> {
    let name: SharedString = name.into();
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
        .child(lucide(icon, icon_size, c(theme::TEXT_DIM)))
        .child(probe(name.to_string()))
}

/// `icon_button` while busy: `spin` turns the icon (`.spin`, 0.8s), otherwise
/// a `.spinner` replaces it.
pub fn icon_button_busy(name: impl Into<SharedString>, icon: &str, size: f32, icon_size: f32, busy: bool, spin: bool) -> Stateful<Div> {
    use gpui::{percentage, Animation, AnimationExt, Transformation};
    let name: SharedString = name.into();
    if !busy {
        return icon_button(name, icon, size, icon_size);
    }
    let glyph = if spin {
        lucide(icon, icon_size, c(theme::TEXT_DIM))
            .with_animation(ElementId::Name(format!("{name}-spin").into()), Animation::new(crate::motion::STATUS_SPIN).repeat(), |s, t| {
                s.with_transformation(Transformation::rotate(percentage(t)))
            })
            .into_any_element()
    } else {
        crate::workspace_ui::kit::spinner(ElementId::Name(format!("{name}-spin").into()), icon_size, c(theme::ACCENT)).into_any_element()
    };
    div()
        .id(ElementId::Name(name.clone()))
        .relative()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
        .child(glyph)
        .child(probe(name.to_string()))
}

pub fn tip<E: StatefulInteractiveElement>(el: E, text: impl Into<SharedString>) -> E {
    let text: SharedString = text.into();
    el.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

/// Status tones of `.cr-status-*` / `.rv-decision-*`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Ok,
    Bad,
    Warn,
    Info,
    Muted,
}

impl Tone {
    pub fn color(self) -> Rgba {
        c(match self {
            Tone::Ok => theme::OK,
            Tone::Bad => theme::DANGER,
            Tone::Warn => theme::WARN,
            Tone::Info => theme::INFO,
            Tone::Muted => theme::TEXT_DIM,
        })
    }

    fn hex(self) -> Option<u32> {
        match self {
            Tone::Ok => Some(theme::OK),
            Tone::Bad => Some(theme::DANGER),
            Tone::Warn => Some(theme::WARN),
            Tone::Info => Some(theme::INFO),
            Tone::Muted => None,
        }
    }
}

/// `.cr-status`: a filled pill, 11px/500.
pub fn status_pill(label: impl Into<SharedString>, tone: Tone) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(tone.color())
        .bg(tone.hex().map_or(c(theme::PANEL_RAISED), |h| tint(h, 0.1)))
        .child(label.into())
}

/// `.rv-decision`: an outlined pill, 11px/600.
pub fn decision_pill(label: impl Into<SharedString>, tone: Tone) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(tone.hex().map_or(c(theme::BORDER_STRONG), |h| tint(h, 0.35)))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tone.color())
        .child(label.into())
}

/// A review decision's label and tone.
pub fn decision(d: &str) -> Option<(&'static str, Tone)> {
    match d {
        "APPROVED" => Some(("Approved", Tone::Ok)),
        "CHANGES_REQUESTED" => Some(("Changes requested", Tone::Bad)),
        "REVIEW_REQUIRED" => Some(("Review required", Tone::Muted)),
        _ => None,
    }
}

/// `.ws-section-head h3`: 12px/600 uppercase dim.
pub fn section_head(label: &str) -> Div {
    div()
        .mb(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_DIM))
        .child(label.to_uppercase())
}

/// A stack's rail: a node, with the line running up to the row above and down to the row below.
pub fn rail(up: bool, down: bool, color: Rgba, filled: bool) -> Div {
    let line = |on: bool| div().flex_1().w(px(2.)).when(on, |d| d.bg(c(theme::BORDER_STRONG)));
    div()
        .w(px(20.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .child(line(up))
        .child(div().size(px(10.)).flex_none().rounded_full().border_2().border_color(color).when(filled, |d| d.bg(color)))
        .child(line(down))
}

/// `.section-count`.
pub fn section_count(n: usize) -> Div {
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

/// `.git-stat-add` / `.git-stat-del`.
pub fn stats(add: u64, del: u64, size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .gap(px(6.))
        .text_size(px(size))
        .when(add > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{add}"))))
        .when(del > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{del}"))))
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix seconds of an ISO-8601 UTC time (`2026-11-03T12:30:00Z`).
pub fn parse_iso(iso: &str) -> Option<i64> {
    let (date, time) = iso.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '.']).next()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
    Some(days_from_civil(y, m, day) * 86_400 + h * 3600 + min * 60 + s)
}

/// `relTime` of `CodeReviewPage.tsx`: "5m ago", "4h ago", "3d ago".
pub fn rel_time(iso: &str, now: i64) -> String {
    let Some(at) = parse_iso(iso) else { return String::new() };
    let s = (now - at).max(0);
    match s {
        0..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso_and_ages() {
        assert_eq!(parse_iso("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(parse_iso("2024-03-01T01:02:03Z"), Some(1_709_254_923));
        let at = parse_iso("2026-11-03T10:00:00Z").unwrap();
        assert_eq!(rel_time("2026-11-03T10:00:00Z", at + 4 * 3600), "4h ago");
        assert_eq!(rel_time("2026-11-03T10:00:00Z", at + 3 * 86_400 + 5), "3d ago");
        assert_eq!(rel_time("bad", at), "");
    }
}
