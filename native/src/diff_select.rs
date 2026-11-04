//! Line selection and "comment on lines" for gpui-component's `Diff`, as
//! Monaco's review layer does it in the Tauri app.
//!
//! The Diff's own `+` (`on_add_annotation`) sits over the first characters of
//! each line's code and swallows the press, so a drag started at a line's
//! start selects nothing; and its text selection never becomes a line range.
//! Instead the `+` lives in the gutter marker lane (Monaco's glyph margin):
//! click it, drag it across lines, or click it inside a selection. A text
//! selection's lines are tracked from the line under the pointer, so the `+`,
//! `review.commentSelection` and the dump see the whole range.
use crate::terminal::CLIPBOARD_DISABLED;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, ElementId, Entity, Focusable, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, Window,
};
use gpui_component::diff::{Diff, DiffFile, DiffGutterMarker, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState};
use gpui_component::input::Copy;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub type OnComment = Rc<dyn Fn(&DiffLineRange, &mut Window, &mut App)>;

/// A press on the `+`: the line it started on and where the drag is.
struct Glyph {
    anchor: DiffLinePosition,
    cursor: DiffLinePosition,
    /// The selection the press landed inside, commented on by a plain click.
    inside: Option<DiffLineRange>,
}

#[derive(Default)]
pub struct LineSelect {
    hovered: Option<DiffLinePosition>,
    /// A press on code: the line under it, and its y.
    press: Option<(DiffLinePosition, Pixels)>,
    /// The lines of the current text selection.
    span: Option<DiffLineRange>,
    glyph: Option<Glyph>,
    markers: Rc<Vec<DiffGutterMarker>>,
    pub last_copied: Option<String>,
}

pub type Handle = Rc<RefCell<LineSelect>>;

pub fn new() -> Handle {
    Rc::new(RefCell::new(LineSelect::default()))
}

impl LineSelect {
    /// One `+` per line of `sides`, from the patch the files were parsed from.
    pub fn set_files(&mut self, files: &[DiffFile], patch: &str, sides: &[DiffSide]) {
        self.set_patches(&[(files, patch)], sides);
    }

    /// [`Self::set_files`] for files parsed from several patches.
    pub fn set_patches(&mut self, patches: &[(&[DiffFile], &str)], sides: &[DiffSide]) {
        let mut markers = vec![];
        let pairs = patches.iter().flat_map(|(files, patch)| files.iter().zip(line_extents(patch)));
        for (file, (old, new)) in pairs {
            for &side in sides {
                let last = if side == DiffSide::Modified { new } else { old };
                markers.extend((1..=last).map(|line| {
                    DiffGutterMarker::new(ElementId::NamedInteger("add".into(), line as u64), DiffLinePosition::new(file.path().clone(), side, line))
                }));
            }
        }
        self.markers = Rc::new(markers);
        self.span = None;
        self.glyph = None;
    }

    /// The selected lines: the Diff's line selection, else the text selection's.
    pub fn range(&self, state: &DiffState) -> Option<DiffLineRange> {
        state.selected_lines().or_else(|| self.span.clone())
    }
}

/// The highest old and new line number of each file in a patch, in order.
pub fn line_extents(patch: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = vec![];
    let mut in_file = false;
    for l in patch.lines() {
        if l.starts_with("diff --git ") || (!in_file && l.starts_with("--- ")) {
            out.push((0, 0));
            in_file = true;
        } else if let Some(rest) = l.strip_prefix("@@ -") {
            if out.is_empty() {
                out.push((0, 0));
            }
            let mut parts = rest.split(' ');
            let end = |s: Option<&str>| -> usize {
                let s = s.unwrap_or("").trim_start_matches('+');
                let (start, count) = s.split_once(',').unwrap_or((s, "1"));
                let (start, count) = (start.parse::<usize>().unwrap_or(0), count.parse::<usize>().unwrap_or(0));
                (start + count).saturating_sub(1)
            };
            let (old, new) = (end(parts.next()), end(parts.next()));
            let last = out.last_mut().unwrap();
            *last = (last.0.max(old), last.1.max(new));
        }
    }
    out
}

/// The lines a text selection covers: `anchor` is the line it started on,
/// `text` what it selected on that side. A selection that ends at the start
/// of a line leaves that line out, as Monaco's does.
pub fn text_span(anchor: &DiffLinePosition, text: &str, down: bool) -> DiffLineRange {
    let body = text.strip_suffix('\n').map(|b| b.strip_suffix('\r').unwrap_or(b));
    let n = body.unwrap_or(text).matches('\n').count();
    let a = anchor.line();
    let (start, end) = if down {
        (a, a + n)
    } else {
        let end = if body.is_some() { a.saturating_sub(1).max(1) } else { a };
        (end.saturating_sub(n).max(1), end)
    };
    DiffLineRange::new(anchor.path().clone(), anchor.side(), start, end.max(start))
}

fn contains(range: &DiffLineRange, p: &DiffLinePosition) -> bool {
    let (lo, hi) = (range.start().min(range.end()), range.start().max(range.end()));
    range.path() == p.path() && range.side() == p.side() && range.end_side() == p.side() && (lo..=hi).contains(&p.line())
}

fn between(a: &DiffLinePosition, b: &DiffLinePosition) -> DiffLineRange {
    DiffLineRange::new(a.path().clone(), a.side(), a.line().min(b.line()), a.line().max(b.line()))
}

/// The `+` in the marker lane: shown on the hovered line and at the end of
/// the selection.
fn plus(sel: &Handle, state: &Entity<DiffState>, marker: &DiffGutterMarker, cx: &App) -> AnyElement {
    let pos = marker.position().clone();
    let range = sel.borrow().range(state.read(cx));
    let at_end = range.as_ref().is_some_and(|r| r.path() == pos.path() && r.end_side() == pos.side() && r.start().max(r.end()) == pos.line());
    let (sel, state) = (sel.clone(), state.clone());
    div()
        .id(marker.id().clone())
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .bg(cx.theme().primary)
        .text_color(cx.theme().primary_foreground)
        .when(!at_end, |d| d.opacity(0.).group_hover("diff-line", |s| s.opacity(1.)))
        .child(Icon::new(IconName::Plus).xsmall())
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            let mut s = sel.borrow_mut();
            let inside = s.range(state.read(cx)).filter(|r| contains(r, &pos) && r.start() != r.end());
            s.glyph = Some(Glyph { anchor: pos.clone(), cursor: pos.clone(), inside });
            window.refresh();
        })
        .into_any_element()
}

/// The Diff with its `+` lane on `sides` (when `on_comment` is given),
/// text-selection line tracking and Ctrl+C, inside a full-size wrapper.
///
/// Split view wraps long lines. Unwrapped, the Diff sizes both columns to half
/// the widest line of either side and scrolls them together, so a long line
/// pushes the new side out of view; wrapped, each side takes half the width.
pub fn attach(diff: Diff, sel: &Handle, state: &Entity<DiffState>, sides: &[DiffSide], on_comment: Option<OnComment>, cx: &App) -> impl IntoElement {
    let split = state.read(cx).mode() == DiffMode::Split;
    let mut diff = diff.soft_wrap(split).on_line_hover({
        let (sel, state) = (sel.clone(), state.clone());
        move |p, window, cx| {
            let mut s = sel.borrow_mut();
            // A row's leave may come after the next row's enter: only enters count.
            let Some(p) = p else { return };
            s.hovered = Some(p.clone());
            let Some(g) = &mut s.glyph else { return };
            if p.path() == g.anchor.path() && p.side() == g.anchor.side() && *p != g.cursor {
                g.cursor = p.clone();
                g.inside = None;
                let range = between(&g.anchor, p);
                drop(s);
                state.update(cx, |st, cx| st.set_selected_lines(Some(range), cx));
                window.refresh();
            }
        }
    });
    if on_comment.is_some() {
        let markers = sel.borrow().markers.clone();
        let (s2, st2) = (sel.clone(), state.clone());
        let markers = markers.iter().filter(|m| sides.contains(&m.position().side())).cloned();
        diff = diff.gutter_markers(markers).render_gutter_marker(move |m, _, cx| plus(&s2, &st2, m, cx));
    }
    let release = {
        let (sel, state) = (sel.clone(), state.clone());
        move |ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
            let glyph = sel.borrow_mut().glyph.take();
            if let Some(g) = glyph {
                let range = g.inside.unwrap_or_else(|| between(&g.anchor, &g.cursor));
                if g.anchor != g.cursor {
                    state.update(cx, |st, cx| st.set_selected_lines(None, cx));
                }
                if let Some(f) = &on_comment {
                    f(&range, window, cx);
                }
                return;
            }
            let mut s = sel.borrow_mut();
            let Some((anchor, y)) = s.press.clone() else { return };
            let st = state.read(cx);
            let text = st.selected_text(cx);
            s.span = (st.selected_lines().is_none() && !text.is_empty()).then(|| {
                let down = match &s.hovered {
                    Some(h) if h.path() == anchor.path() && h.side() == anchor.side() => h.line() >= anchor.line(),
                    _ => ev.position.y >= y,
                };
                text_span(&anchor, &text, down)
            });
            window.refresh();
        }
    };
    let release_out = release.clone();
    let (s_down, s_out, s_copy, st_copy) = (sel.clone(), sel.clone(), sel.clone(), state.clone());
    let s_hover = sel.clone();
    div()
        .id("diff-select")
        .size_full()
        .child(diff)
        .on_hover(move |on, _, _| {
            if !on {
                s_hover.borrow_mut().hovered = None;
            }
        })
        // Bubbles here unless the `+` or a card took the press.
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
            let mut s = s_down.borrow_mut();
            let kept = ev.modifiers.shift && s.press.is_some();
            if !kept {
                s.press = s.hovered.clone().map(|h| (h, ev.position.y));
            }
            s.span = None;
        })
        .on_mouse_down_out(move |_, _, _| {
            let mut s = s_out.borrow_mut();
            s.press = None;
            s.span = None;
        })
        .on_mouse_up(MouseButton::Left, move |ev, w, cx| release(ev, w, cx))
        .on_mouse_up_out(MouseButton::Left, move |ev, w, cx| release_out(ev, w, cx))
        .capture_action(move |_: &Copy, window, cx| {
            if !st_copy.read(cx).focus_handle(cx).is_focused(window) {
                return;
            }
            let text = st_copy.read(cx).selected_text(cx);
            if text.is_empty() {
                return;
            }
            if !CLIPBOARD_DISABLED.load(Ordering::Relaxed) {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            }
            s_copy.borrow_mut().last_copied = Some(text);
            cx.stop_propagation();
        })
}

/// The dump lines for a diff's selection.
pub fn dump(sel: &Handle, state: &Entity<DiffState>, cx: &App) -> String {
    let s = sel.borrow();
    let st = state.read(cx);
    let fmt = |r: &DiffLineRange| format!("{} {:?} {}-{}", r.path(), r.side(), r.start(), r.end());
    let mut out = String::new();
    let lines = st.selected_lines();
    let text = st.selected_text(cx);
    if lines.is_some() || s.span.is_some() || !text.is_empty() {
        out.push_str(&format!(
            "  selection lines {:?} span {:?} text {:?}\n",
            lines.as_ref().map(fmt),
            s.span.as_ref().map(fmt),
            text
        ));
    }
    if let Some(c) = &s.last_copied {
        out.push_str(&format!("  copied {c:?}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize) -> DiffLinePosition {
        DiffLinePosition::new("web/main.rs", DiffSide::Modified, line)
    }

    fn lines(r: DiffLineRange) -> (usize, usize) {
        (r.start(), r.end())
    }

    #[test]
    fn spans_follow_the_selected_text() {
        // Dragged down from line 5 into line 9.
        assert_eq!(lines(text_span(&at(5), "n line_5()\na\nb\nc\nfn li", true)), (5, 9));
        // Ends at the start of line 10: line 10 is left out.
        assert_eq!(lines(text_span(&at(5), "a\nb\nc\nd\ne\n", true)), (5, 9));
        // Dragged up from line 9 to line 5.
        assert_eq!(lines(text_span(&at(9), "x\nb\nc\nd\nfn", false)), (5, 9));
        // Up from the start of line 10: line 10 is left out.
        assert_eq!(lines(text_span(&at(10), "e5\n6\n7\n8\n9\n", false)), (5, 9));
        // A word.
        assert_eq!(lines(text_span(&at(7), "line_7", true)), (7, 7));
        assert_eq!(text_span(&at(7), "x\ny", true).side(), DiffSide::Modified);
    }

    /// A range note is written as the Tauri app writes it (`ChangesReview.tsx`:
    /// `lines.slice(start - 1, end)` as the snippet, through `notes_add`).
    #[test]
    fn range_notes_match_the_tauri_format() {
        let text: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        let r = text_span(&at(5), "ne 5\nline 6\nline 7\nline 8\nli", true);
        let (start, end) = (r.start() as u32, r.end() as u32);
        let snippet = crate::review::notes::snippet(&text, start, end);
        let react: Vec<&str> = text.split('\n').collect::<Vec<_>>()[start as usize - 1..end as usize].to_vec();
        assert_eq!(snippet, react.join("\n"));
        let note = orbit_core::notes::Note {
            id: String::new(),
            done: false,
            repo: "web".into(),
            path: "main.rs".into(),
            start,
            end,
            body: "Range".into(),
            snippet,
        };
        let md = orbit_core::notes::render(&[note], None);
        assert!(md.contains("- [ ] `web/main.rs:5-9`\n  Range\n  ```rs\n  line 5\n"), "{md}");
        assert!(md.contains("  line 9\n  ```\n"), "{md}");
    }

    #[test]
    fn reads_line_extents_per_file() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,4 @@\n a\n+b\n c\n d\n@@ -40,2 +41,3 @@\n x\n+y\n z\n\
                     diff --git a/b b/b\n--- /dev/null\n+++ b/b\n@@ -0,0 +1,2 @@\n+n\n+m\n";
        assert_eq!(line_extents(patch), [(41, 43), (0, 2)]);
        assert_eq!(line_extents("--- a/x\n+++ b/x\n@@ -3 +3 @@\n-a\n+b\n"), [(3, 3)]);
    }

    #[test]
    fn ranges_cover_both_ends() {
        assert_eq!(lines(between(&at(9), &at(5))), (5, 9));
        let r = DiffLineRange::new("web/main.rs", DiffSide::Modified, 5, 9);
        assert!(contains(&r, &at(5)) && contains(&r, &at(9)) && !contains(&r, &at(10)));
        assert!(!contains(&r, &DiffLinePosition::new("web/main.rs", DiffSide::Original, 6)));
    }
}
