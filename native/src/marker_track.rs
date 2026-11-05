//! Marks on a scrollbar track (VS Code's overview ruler) for the diff views:
//! removed lines red on the left half, added lines green on the right (so a
//! change shows both however short), at their share of the rows; a click
//! on a mark scrolls to its line, elsewhere the scrollbar takes it. (The code
//! editor draws its own through gpui-component's overview ruler, `editor/changes.rs`.)
//!
//! gpui-component's diff keeps its rows private, so they are projected again
//! from the patch as the view lays them out: a header and hunk row each, the
//! context around changes, a fold row for the rest, and in split view a
//! change's removed and added lines side by side. Opened folds and notes
//! make it approximate; clicks always land on the line.
use crate::theme::{self, hsla};
use gpui::{canvas, fill, point, prelude::*, px, size, App, Bounds, Entity, Hsla, MouseDownEvent, Pixels, Window};
use gpui_component::diff::{DiffLinePosition, DiffMode, DiffSide, DiffState};
use gpui_component::scroll::Scrollbar;
use std::ops::Range;
use std::rc::Rc;

/// Shortest mark drawn, and the height a click finds it in.
const MIN_MARK: f32 = 3.;
const HIT: f32 = 6.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark<T> {
    pub rows: Range<usize>,
    pub color: Hsla,
    pub lane: Lane,
    pub target: T,
}

/// `marks` over `rows` rows, in row order.
pub struct Track<T> {
    pub rows: usize,
    pub marks: Vec<Mark<T>>,
}

/// Paints `track` over the right edge of a `relative()` parent, over its
/// scrollbar; a click on a mark calls `on_click` with its target.
pub fn render<T: 'static>(track: Track<T>, on_click: impl Fn(&T, &mut Window, &mut App) + 'static) -> impl IntoElement {
    let track = Rc::new(track);
    let on_click = Rc::new(on_click);
    canvas(
        {
            let track = track.clone();
            move |bounds, _, _| layout(&track, bounds)
        },
        move |bounds, quads: Vec<(Bounds<Pixels>, Hsla, usize)>, window, _| {
            for (b, color, _) in &quads {
                window.paint_quad(fill(*b, *color));
            }
            let (track, on_click) = (track.clone(), on_click.clone());
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble() || !bounds.contains(&ev.position) {
                    return;
                }
                let hit = quads.iter().find(|(b, _, _)| {
                    let grow = (px(HIT) - b.size.height).max(px(0.)) / 2.;
                    (b.top() - grow..=b.bottom() + grow).contains(&ev.position.y)
                });
                if let Some((_, _, ix)) = hit {
                    cx.stop_propagation();
                    on_click(&track.marks[*ix].target, window, cx);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .bottom_0()
    .right_0()
    .w(Scrollbar::width())
}

/// Quads for the marks, adjacent ones of a colour and lane merged, each with
/// the first mark it stands for.
fn layout<T>(track: &Track<T>, bounds: Bounds<Pixels>) -> Vec<(Bounds<Pixels>, Hsla, usize)> {
    if track.rows == 0 {
        return vec![];
    }
    let scale = bounds.size.height / track.rows as f32;
    let half = bounds.size.width / 2.;
    let mut out: Vec<(Bounds<Pixels>, Hsla, usize)> = Vec::new();
    for (ix, m) in track.marks.iter().enumerate() {
        let left = if m.lane == Lane::Left { px(0.) } else { half };
        let top = bounds.top() + scale * m.rows.start as f32;
        let height = (scale * m.rows.len() as f32).max(px(MIN_MARK));
        let top = top.min(bounds.bottom() - height);
        let x = bounds.left() + left;
        let merged = out.iter_mut().rev().find(|(b, c, _)| b.left() == x && *c == m.color).filter(|(b, _, _)| top <= b.bottom() + px(1.));
        match merged {
            Some((b, _, _)) => b.size.height = b.size.height.max(top + height - b.top()),
            None => out.push((Bounds::new(point(x, top), size(half, height)), m.color, ix)),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Run {
    Same(usize),
    Change { removed: usize, added: usize },
}

#[derive(Debug, Default, PartialEq)]
struct Hunk {
    old: usize,
    new: usize,
    runs: Vec<Run>,
}

/// One file of a patch: its hunks as runs of unchanged and changed lines.
#[derive(Debug, Default, PartialEq)]
pub struct FileShape(Vec<Hunk>);

/// The shape of each file in `patch`, in the order `DiffFile::parse` gives them.
pub fn shapes(patch: &str) -> Vec<FileShape> {
    let mut files: Vec<FileShape> = Vec::new();
    let mut in_hunk = false;
    for l in patch.lines() {
        if l.starts_with("diff --git ") {
            files.push(FileShape::default());
            in_hunk = false;
            continue;
        }
        if let Some(head) = l.strip_prefix("@@ ") {
            let mut nums = head.split_whitespace().take(2).map(|n| n[1..].split(',').next().and_then(|s| s.parse().ok()).unwrap_or(1));
            let (old, new) = (nums.next().unwrap_or(1), nums.next().unwrap_or(1));
            if files.is_empty() {
                files.push(FileShape::default());
            }
            files.last_mut().unwrap().0.push(Hunk { old, new, runs: vec![] });
            in_hunk = true;
            continue;
        }
        let Some(hunk) = files.last_mut().and_then(|f| f.0.last_mut()).filter(|_| in_hunk) else { continue };
        let runs = &mut hunk.runs;
        match (l.as_bytes().first(), runs.last_mut()) {
            (Some(b'-'), Some(Run::Change { removed, .. })) => *removed += 1,
            (Some(b'+'), Some(Run::Change { added, .. })) => *added += 1,
            (Some(b'-'), _) => runs.push(Run::Change { removed: 1, added: 0 }),
            (Some(b'+'), _) => runs.push(Run::Change { removed: 0, added: 1 }),
            (Some(b'\\'), _) => {}
            (_, Some(Run::Same(n))) => *n += 1,
            _ => runs.push(Run::Same(1)),
        }
    }
    files
}

/// Rows a run of `n` unchanged lines takes with `context` lines kept next to
/// each change (`before`/`after` say which sides have one).
fn same_rows(n: usize, before: bool, after: bool, context: Option<usize>, min_folded: usize) -> usize {
    let Some(context) = context else { return n };
    let kept = context * (before as usize + after as usize);
    if n <= kept || n - kept < min_folded {
        n
    } else {
        kept + 1
    }
}

/// The track of `files` as `state` shows them; `header`: file headers are shown.
pub fn diff_track(files: &[FileShape], state: &DiffState, header: bool) -> Track<DiffLinePosition> {
    let split = state.mode() == DiffMode::Split;
    let (context, min_folded) = (state.context_lines(), state.min_collapsed_lines());
    let (red, green) = (hsla(theme::DANGER), hsla(theme::OK));
    let mut rows = 0;
    let mut marks = Vec::new();
    for (shape, file) in files.iter().zip(state.files()) {
        rows += header as usize;
        if state.is_file_collapsed(file.path()) {
            continue;
        }
        if shape.0.is_empty() {
            rows += 1;
        }
        for hunk in &shape.0 {
            rows += 1;
            let (mut old, mut new) = (hunk.old, hunk.new);
            for (ix, run) in hunk.runs.iter().enumerate() {
                match *run {
                    Run::Same(n) => {
                        rows += same_rows(n, ix > 0, ix + 1 < hunk.runs.len(), context, min_folded);
                        old += n;
                        new += n;
                    }
                    Run::Change { removed, added } => {
                        let mut mark = |at: usize, len: usize, color, lane, side, line| {
                            if len > 0 {
                                let target = DiffLinePosition::new(file.path().clone(), side, line);
                                marks.push(Mark { rows: at..at + len, color, lane, target });
                            }
                        };
                        // Unified view lists the removed lines, then the added.
                        let added_at = if split { rows } else { rows + removed };
                        mark(rows, removed, red, Lane::Left, DiffSide::Original, old);
                        mark(added_at, added, green, Lane::Right, DiffSide::Modified, new);
                        rows += if split { removed.max(added) } else { removed + added };
                        old += removed;
                        new += added;
                    }
                }
            }
        }
    }
    Track { rows, marks }
}

/// The diff's marker track for a `relative()` parent of its `Diff`.
pub fn diff(files: &[FileShape], state: &Entity<DiffState>, header: bool, cx: &App) -> impl IntoElement {
    let track = diff_track(files, state.read(cx), header);
    let state = state.downgrade();
    render(track, move |target, _, cx| {
        let _ = state.update(cx, |s, cx| s.scroll_to_line(target.clone(), cx));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,12 +1,12 @@\n a\n-b\n+B\n+B2\n c\n d\n e\n f\n g\n h\n i\n-j\n k\n\\ No newline at end of file\n";

    #[test]
    fn parses_runs_and_line_numbers() {
        let files = shapes(PATCH);
        assert_eq!(files.len(), 1);
        let h = &files[0].0[0];
        assert_eq!((h.old, h.new), (1, 1));
        assert_eq!(
            h.runs,
            vec![Run::Same(1), Run::Change { removed: 1, added: 2 }, Run::Same(7), Run::Change { removed: 1, added: 0 }, Run::Same(1)]
        );
    }

    #[test]
    fn folds_long_unchanged_runs() {
        // 3 lines of context each side of 7, the 1 left over is too short to fold.
        assert_eq!(same_rows(7, true, true, Some(3), 2), 7);
        assert_eq!(same_rows(9, true, true, Some(3), 2), 7);
        assert_eq!(same_rows(10, false, true, Some(3), 2), 4);
        assert_eq!(same_rows(10, true, true, None, 2), 10);
    }

    #[test]
    fn layout_merges_touching_marks() {
        let mark = |rows: Range<usize>, lane| Mark { rows, color: gpui::red(), lane, target: () };
        let track = Track { rows: 100, marks: vec![mark(0..10, Lane::Right), mark(10..20, Lane::Right), mark(50..51, Lane::Left)] };
        let quads = layout(&track, Bounds::new(point(px(0.), px(0.)), size(px(16.), px(200.))));
        assert_eq!(quads.len(), 2);
        assert_eq!((quads[0].0.left(), quads[0].0.size), (px(8.), size(px(8.), px(40.))));
        assert_eq!((quads[1].0.origin, quads[1].0.size), (point(px(0.), px(100.)), size(px(8.), px(3.))));
        assert_eq!(quads[1].2, 2);
    }
}
