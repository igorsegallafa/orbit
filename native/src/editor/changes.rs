//! Change marks in the editor, as JetBrains shows them: a bar in the gutter
//! strip for each line changed since the branch forked from its base (purple)
//! or since HEAD (green added, blue modified, grey for deleted lines), and the
//! same marks on the scrollbar track (the left lane, Monaco's convention),
//! with problems in the right lane and Find matches and the caret in the middle.
//!
//! The buffer is diffed in memory against the HEAD and fork-point blobs, off
//! the UI thread, after each pause in typing; the blobs are read again when
//! git changes (`file_ops`).
use super::EditorTab;
use crate::theme::{self, hsla};
use gpui::{AppContext as _, Context, Hsla, Task};
use gpui_component::input::{
    GutterDecoration, GutterDecorationCollection, OverviewRulerDecoration, OverviewRulerDecorationCollection, OverviewRulerLane,
    Rope, RopeExt,
};
use lsp_types::DiagnosticSeverity;
use similar::{DiffTag, TextDiff};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(300);
/// A diff that takes longer gives up on finding the smallest one.
const DIFF_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

/// Changed lines (0-based, of the buffer); a deletion is an empty range
/// before the line that follows it. `branch`: committed on the branch, not since HEAD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub change: Change,
    pub branch: bool,
    pub lines: Range<u32>,
}

/// The file at HEAD and where HEAD forked from the base branch (the same
/// `Arc` when they are equal). `None`: no such side to compare with.
#[derive(Default)]
pub struct Bases {
    head: Option<Arc<str>>,
    branch: Option<Arc<str>>,
}

/// The editor's marks: what they compare with and where they are drawn.
pub struct Marks {
    bases: Option<Arc<Bases>>,
    pub lines: Vec<Mark>,
    gutter: GutterDecorationCollection,
    git_ruler: OverviewRulerDecorationCollection,
    problem_ruler: OverviewRulerDecorationCollection,
    find_ruler: OverviewRulerDecorationCollection,
    /// The caret line and the Find matches last drawn, to redraw only on change.
    find_key: (u32, usize, usize),
    _task: Task<()>,
}

pub fn line_changes(old: &str, new: &str) -> Vec<(Change, Range<u32>)> {
    let diff = TextDiff::configure().timeout(DIFF_TIMEOUT).diff_lines(old, new);
    let mut out = Vec::new();
    // A run of deletes and inserts between equal lines is one change.
    let mut run: Option<(bool, Range<u32>)> = None;
    let mut flush = |run: &mut Option<(bool, Range<u32>)>| {
        if let Some((deleted, lines)) = run.take() {
            let change = match (deleted, lines.is_empty()) {
                (_, true) => Change::Deleted,
                (true, false) => Change::Modified,
                (false, false) => Change::Added,
            };
            out.push((change, lines));
        }
    };
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            flush(&mut run);
            continue;
        }
        let r = run.get_or_insert((false, new.start as u32..new.start as u32));
        r.0 |= !old.is_empty();
        r.1.end = new.end as u32;
    }
    flush(&mut run);
    out
}

/// Uncommitted changes, then what the branch changed on other lines.
pub fn combine(local: Vec<(Change, Range<u32>)>, branch: Vec<(Change, Range<u32>)>) -> Vec<Mark> {
    let cover: Vec<&Range<u32>> = local.iter().map(|(_, r)| r).filter(|r| !r.is_empty()).collect();
    let mut out: Vec<Mark> = local.iter().map(|(change, lines)| Mark { change: *change, branch: false, lines: lines.clone() }).collect();
    for (change, r) in branch {
        let mut push = |lines: Range<u32>| out.push(Mark { change, branch: true, lines });
        if r.is_empty() {
            if !local.iter().any(|(_, l)| l.start <= r.start && r.start <= l.end) {
                push(r);
            }
            continue;
        }
        let mut start = r.start;
        let first = cover.partition_point(|c| c.end <= r.start);
        for c in cover[first..].iter().take_while(|c| c.start < r.end) {
            if c.start > start {
                push(start..c.start);
            }
            start = start.max(c.end);
        }
        if start < r.end {
            push(start..r.end);
        }
    }
    out.sort_by_key(|m| (m.lines.start, m.lines.end));
    out
}

/// `text` ending with a newline, so a missing last newline isn't a change.
fn terminated(mut text: String) -> String {
    if text.contains("\r\n") {
        text = text.replace("\r\n", "\n");
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    orbit_core::proc::run("git", args, Some(dir)).ok()
}

/// Reads `rel`'s blobs (blocking): at HEAD, and at the fork point from
/// `workspace`'s base branch (origin's default branch outside a workspace).
pub fn load_bases(dir: &Path, rel: &str, workspace: &str) -> Bases {
    if git(dir, &["check-ignore", "-q", "--", rel]).is_some() {
        return Bases::default();
    }
    let Some(_) = git(dir, &["rev-parse", "--verify", "-q", "HEAD"]) else { return Bases::default() };
    // A file not in a commit is all added.
    let blob = |rev: &str| -> Arc<str> { git(dir, &["show", &format!("{rev}:{rel}")]).map(terminated).unwrap_or_default().into() };
    let head = blob("HEAD");
    let base = orbit_core::workspace::ws_dir(workspace)
        .and_then(|d| orbit_core::workspace::load_meta(&d))
        .map(|w| w.base)
        .ok()
        .or_else(|| orbit_core::git::default_branch(dir))
        .unwrap_or_else(|| "main".into());
    let start = orbit_core::git::base_ref(dir, &base);
    let branch = git(dir, &["merge-base", "HEAD", &start]).map(|mb| {
        let fork = blob(mb.trim());
        if fork == head {
            head.clone()
        } else {
            fork
        }
    });
    Bases { head: Some(head), branch }
}

fn compute(bases: &Bases, text: &str) -> Vec<Mark> {
    let Some(head) = &bases.head else { return vec![] };
    let local = line_changes(head, text);
    let branch = match &bases.branch {
        Some(b) if !Arc::ptr_eq(b, head) => line_changes(b, text),
        _ => vec![],
    };
    combine(local, branch)
}

fn color(m: &Mark) -> Hsla {
    match (m.branch, m.change) {
        (true, Change::Modified) => hsla(theme::mix(theme::hex(theme::PURPLE), theme::hex(theme::BG), 0.7)),
        (true, _) => hsla(theme::PURPLE),
        (false, Change::Added) => hsla(theme::OK),
        (false, Change::Modified) => hsla(theme::INFO),
        (false, Change::Deleted) => hsla(theme::TEXT_FAINT),
    }
}

/// The byte range of 0-based `lines`; empty at the start of the line after a deletion.
fn byte_range(text: &Rope, lines: &Range<u32>) -> Option<Range<usize>> {
    let count = text.lines_len();
    let start = lines.start as usize;
    if lines.is_empty() {
        return (start <= count).then(|| if start < count { text.line_start_offset(start) } else { text.len() }).map(|o| o..o);
    }
    let end = (lines.end as usize).min(count).checked_sub(1)?;
    (start <= end).then(|| text.line_start_offset(start)..text.line_end_offset(end))
}

impl EditorTab {
    pub(super) fn create_marks(state: &gpui::Entity<gpui_component::input::EditorState>, cx: &mut Context<Self>) -> Marks {
        let (gutter, git_ruler, problem_ruler, find_ruler) = state.update(cx, |s, cx| {
            (
                s.create_gutter_decorations_collection(vec![], cx),
                s.create_overview_ruler_decorations_collection(vec![], cx),
                s.create_overview_ruler_decorations_collection(vec![], cx),
                s.create_overview_ruler_decorations_collection(vec![], cx),
            )
        });
        Marks { bases: None, lines: vec![], gutter, git_ruler, problem_ruler, find_ruler, find_key: (u32::MAX, 0, 0), _task: Task::ready(()) }
    }

    /// Reads the blobs to compare with again (on open and when git changes), then re-diffs.
    pub fn reload_git_bases(&mut self, cx: &mut Context<Self>) {
        let Some((key, root)) = &self.repo else { return };
        let Ok(rel) = self.path.strip_prefix(root) else { return };
        let (dir, rel, workspace) = (root.clone(), rel.to_string_lossy().replace('\\', "/"), key.workspace.clone());
        let text = self.state.read(cx).text().clone();
        self.marks._task = cx.spawn(async move |this, cx| {
            let (bases, lines) = cx
                .background_spawn(async move {
                    let bases = Arc::new(load_bases(&dir, &rel, &workspace));
                    let lines = compute(&bases, &terminated(text.to_string()));
                    (bases, lines)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.marks.bases = Some(bases);
                this.show_marks(lines, cx);
            });
        });
    }

    /// Re-diffs the buffer once typing pauses.
    pub(super) fn schedule_marks(&mut self, cx: &mut Context<Self>) {
        let Some(bases) = self.marks.bases.clone() else { return };
        self.marks._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let Ok(text) = this.update(cx, |this, cx| this.state.read(cx).text().clone()) else { return };
            let lines = cx.background_spawn(async move { compute(&bases, &terminated(text.to_string())) }).await;
            let _ = this.update(cx, |this, cx| this.show_marks(lines, cx));
        });
    }

    fn show_marks(&mut self, lines: Vec<Mark>, cx: &mut Context<Self>) {
        let text = self.state.read(cx).text().clone();
        let ranges: Vec<(Range<usize>, Hsla)> = lines.iter().filter_map(|m| Some((byte_range(&text, &m.lines)?, color(m)))).collect();
        self.marks.gutter.set(ranges.iter().map(|(r, c)| GutterDecoration::new(r.clone()).with_color(*c)).collect(), cx);
        self.marks.git_ruler.set(
            ranges.into_iter().map(|(r, c)| OverviewRulerDecoration::new(r).with_color(c).with_lane(OverviewRulerLane::Left)).collect(),
            cx,
        );
        self.marks.lines = lines;
    }

    /// Errors and warnings on the scrollbar track.
    pub(super) fn show_problem_marks(&self, cx: &mut Context<Self>) {
        let text = self.state.read(cx).text().clone();
        let marks = self
            .diagnostics
            .iter()
            .filter_map(|d| {
                let color = match d.severity {
                    Some(DiagnosticSeverity::ERROR) | None => theme::DANGER,
                    Some(DiagnosticSeverity::WARNING) => theme::WARN,
                    _ => return None,
                };
                let line = d.range.start.line;
                let range = byte_range(&text, &(line..line + 1))?;
                Some(OverviewRulerDecoration::new(range).with_color(hsla(color)).with_lane(OverviewRulerLane::Right))
            })
            .collect();
        self.marks.problem_ruler.set(marks, cx);
    }

    /// Find matches and the caret line on the scrollbar track, when either moved.
    pub(super) fn update_find_marks(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let search = state.search_session();
        let matches = search.is_active().then(|| search.matcher.matched_ranges());
        let line = state.cursor_position().line;
        let key = (line, matches.as_ref().map_or(0, |m| std::rc::Rc::as_ptr(m) as usize), matches.as_ref().map_or(0, |m| m.len()));
        if key == self.marks.find_key {
            return;
        }
        self.marks.find_key = key;
        let text = state.text().clone();
        let found = hsla(theme::ACCENT);
        let mut marks: Vec<_> =
            matches.iter().flat_map(|m| m.iter()).map(|r| OverviewRulerDecoration::new(r.clone()).with_color(found).with_lane(OverviewRulerLane::Center)).collect();
        if let Some(r) = byte_range(&text, &(line..line + 1)) {
            marks.push(OverviewRulerDecoration::new(r).with_color(hsla(theme::TEXT_DIM)).with_lane(OverviewRulerLane::Center));
        }
        self.marks.find_ruler.set(marks, cx);
    }

    /// The change marks (self-test dump): `+`/`~`/`-`, `b` for the branch's, 1-based lines.
    pub fn marks_text(&self) -> String {
        self.marks
            .lines
            .iter()
            .map(|m| {
                let sign = match m.change {
                    Change::Added => '+',
                    Change::Modified => '~',
                    Change::Deleted => '-',
                };
                let lines = if m.lines.is_empty() { format!("{}", m.lines.start + 1) } else { format!("{}-{}", m.lines.start + 1, m.lines.end) };
                format!("{}{sign}{lines}", if m.branch { "b" } else { "" })
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: Change, r: Range<u32>) -> (Change, Range<u32>) {
        (c, r)
    }

    #[test]
    fn classifies_added_modified_and_deleted_lines() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nnew\nnew2\nd\n";
        assert_eq!(
            line_changes(old, new),
            vec![ch(Change::Modified, 1..2), ch(Change::Added, 3..5), ch(Change::Deleted, 6..6)]
        );
        assert_eq!(line_changes("", "x\ny\n"), vec![ch(Change::Added, 0..2)]);
        assert!(line_changes(old, old).is_empty());
    }

    #[test]
    fn uncommitted_lines_win_over_the_branch() {
        let local = vec![ch(Change::Modified, 3..5), ch(Change::Deleted, 9..9)];
        let branch = vec![ch(Change::Added, 0..8), ch(Change::Deleted, 9..9), ch(Change::Deleted, 12..12)];
        let got: Vec<_> = combine(local, branch).into_iter().map(|m| (m.branch, m.change, m.lines)).collect();
        assert_eq!(
            got,
            vec![
                (true, Change::Added, 0..3),
                (false, Change::Modified, 3..5),
                (true, Change::Added, 5..8),
                (false, Change::Deleted, 9..9),
                (true, Change::Deleted, 12..12),
            ]
        );
    }

    #[test]
    fn line_ranges_map_to_bytes() {
        let text = Rope::from("ab\ncd\nef");
        assert_eq!(byte_range(&text, &(1..2)), Some(3..5));
        assert_eq!(byte_range(&text, &(0..3)), Some(0..8));
        assert_eq!(byte_range(&text, &(1..1)), Some(3..3));
        assert_eq!(byte_range(&text, &(3..3)), Some(8..8));
        assert_eq!(byte_range(&text, &(5..6)), None);
    }
}
