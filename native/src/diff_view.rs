//! Diff tab: a file's changes against HEAD in gpui-component's virtualized
//! patch view (split or unified, tree-sitter colors). In a workspace repo it
//! is also a local review: notes from REVIEW.md (the Tauri app's file) show
//! under their lines, and the `+` beside a line or a selection leaves one.
use crate::diff_select;
use crate::lsp::file_language;
use crate::probe::probe;
use crate::review::notes::{self as rnotes, Anchor};
use crate::review::ui::{self, Tone};
use crate::theme::{self, c};
use anyhow::{bail, Context as _, Result};
use gpui::{
    div, prelude::*, px, App, Context, ElementId, Entity, Focusable, KeyDownEvent, MouseButton, SharedString, Subscription, Task,
    WeakEntity, Window,
};
use gpui_component::diff::{
    Diff, DiffAnnotation, DiffDecoration, DiffFile, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState,
};
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::{Input, InputEvent, InputState, Rope};
use gpui_component::ActiveTheme;
use orbit_core::notes::Note;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

/// Unchanged lines are folded by the view, so the patch carries the whole
/// file: notes can sit on any line and folds can be expanded.
const FULL_CONTEXT: &str = "--unified=1000000";

/// The workspace repo a file belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoFile {
    pub workspace: String,
    pub repo: String,
    pub repo_dir: PathBuf,
    /// Path inside the repo, with `/`.
    pub rel: String,
}

/// `<workspaces dir>/<workspace>/<repo>/<rel>` → its parts.
pub fn locate(path: &Path) -> Option<RepoFile> {
    let root = orbit_core::workspace::workspaces_dir().ok()?;
    let rest = path.strip_prefix(&root).ok()?;
    let mut parts = rest.components();
    let workspace = parts.next()?.as_os_str().to_str()?.to_string();
    let repo = parts.next()?.as_os_str().to_str()?.to_string();
    let rel = parts.as_path().to_str()?.replace('\\', "/");
    let repo_dir = root.join(&workspace).join(&repo);
    (!rel.is_empty() && repo_dir.join(".git").exists()).then_some(RepoFile { workspace, repo, repo_dir, rel })
}

fn git(dir: &Path, args: &[&str]) -> Result<(bool, String, String)> {
    let out = Command::new("git").args(args).current_dir(dir).output().context("git not available")?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// The patch of `rel` (in `dir`) against HEAD; an untracked file is all added.
pub fn git_patch(dir: &Path, rel: &str) -> Result<String> {
    git_patch_in(dir, rel, None)
}

/// [`git_patch`] with the file's path under `prefix/` (its repo), so several
/// repos' files can share one diff view.
pub fn git_patch_in(dir: &Path, rel: &str, prefix: Option<&str>) -> Result<String> {
    let prefixes = prefix.map(|p| [format!("--src-prefix=a/{p}/"), format!("--dst-prefix=b/{p}/")]);
    let pre: Vec<&str> = prefixes.iter().flatten().map(String::as_str).collect();
    let args = |head: &[&'static str], tail: &[&str]| -> Vec<String> {
        head.iter().map(|s| s.to_string()).chain(pre.iter().map(|s| s.to_string())).chain(tail.iter().map(|s| s.to_string())).collect()
    };
    let run = |a: Vec<String>| git(dir, &a.iter().map(String::as_str).collect::<Vec<_>>());
    let (ok, patch, err) = run(args(&["diff", "--no-color", "--no-ext-diff", FULL_CONTEXT], &["HEAD", "--", rel]))?;
    if !ok {
        bail!("{}", err.trim());
    }
    if !patch.trim().is_empty() || !dir.join(rel).is_file() {
        return Ok(patch);
    }
    let (tracked, _, _) = git(dir, &["ls-files", "--error-unmatch", "--", rel])?;
    if tracked {
        return Ok(patch);
    }
    // Exits 1 when the files differ, which is the point.
    let (_, patch, _) = run(args(&["diff", "--no-index", "--no-color", FULL_CONTEXT], &["--", "/dev/null", rel]))?;
    Ok(patch)
}

/// A diff state that colors code with the active theme.
pub fn highlighted_state(files: Vec<DiffFile>, mode: DiffMode, cx: &mut App) -> Entity<DiffState> {
    let highlight_theme = cx.theme().highlight_theme.clone();
    cx.new(|cx| {
        DiffState::new(files, cx).with_mode(mode).with_syntax_highlighter(move |language, text| {
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &Rope::from(text), None);
            highlighter.styles(&(0..text.len()), &*highlight_theme)
        })
    })
}

/// The patch of `rel` in commit `sha` (against its parent; a root commit adds all).
pub fn commit_patch(dir: &Path, sha: &str, rel: &str) -> Result<String> {
    let (ok, patch, err) = git(dir, &["show", "--format=", "--no-color", "--no-ext-diff", FULL_CONTEXT, sha, "--", rel])?;
    if !ok {
        bail!("{}", err.trim());
    }
    Ok(patch)
}

/// The first added line of a patch: where "comment" goes without a selection.
pub fn first_added_line(patch: &str) -> Option<u32> {
    let mut line = 0u32;
    for l in patch.lines() {
        if let Some(head) = l.strip_prefix("@@ ") {
            let new = head.split_whitespace().nth(1)?.trim_start_matches('+');
            line = new.split(',').next()?.parse().ok()?;
        } else if line > 0 {
            match l.as_bytes().first() {
                Some(b'+') => return Some(line),
                Some(b'-') | Some(b'\\') => {}
                _ => line += 1,
            }
        }
    }
    None
}

/// What opening or refreshing a diff tab reads from disk.
pub struct LoadedDiff {
    files: Vec<DiffFile>,
    patch: String,
    first_added: Option<u32>,
    changes: usize,
    review: Option<(RepoFile, String, Vec<Note>, Option<SystemTime>)>,
    stamp: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The notes of one file.
fn file_notes(all: &[Note], at: &RepoFile) -> Vec<Note> {
    all.iter().filter(|n| n.repo == at.repo && n.path == at.rel).cloned().collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComposerKind {
    New { start: u32, end: u32 },
    Edit(String),
    Reply(String),
}

pub struct Composer {
    pub kind: ComposerKind,
    pub input: Entity<InputState>,
    _sub: Subscription,
}

/// The local review of a workspace file.
pub struct FileReview {
    pub at: RepoFile,
    /// The working-tree text, where notes are anchored.
    pub text: String,
    pub notes: Vec<Note>,
    notes_stamp: Option<SystemTime>,
    pub show_done: bool,
}

impl FileReview {
    /// Notes shown (open ones unless `show_done`), with where they are now, by line.
    pub fn shown(&self) -> Vec<(Note, Anchor)> {
        let mut out: Vec<(Note, Anchor)> = self
            .notes
            .iter()
            .filter(|n| self.show_done || !n.done)
            .map(|n| (n.clone(), rnotes::anchor(n, Some(&self.text))))
            .collect();
        out.sort_by_key(|(_, a)| (a.end, a.start));
        out
    }
}

pub struct DiffTab {
    pub path: PathBuf,
    pub title: SharedString,
    pub state: Entity<DiffState>,
    pub additions: usize,
    pub deletions: usize,
    pub review: Option<FileReview>,
    pub composer: Option<Composer>,
    pub message: Option<String>,
    pub sel: diff_select::Handle,
    first_added: Option<u32>,
    changes: usize,
    change_ix: Option<usize>,
    file_stamp: Option<SystemTime>,
    refreshing: bool,
    _poll: Task<()>,
}

impl DiffTab {
    /// Everything `new` shows for `path`.
    pub fn load(path: &Path) -> Result<LoadedDiff> {
        let at = locate(path);
        let patch = match &at {
            Some(at) => git_patch(&at.repo_dir, &at.rel)?,
            None => {
                let dir = path.parent().context("file has no folder")?;
                git_patch(dir, &path.file_name().context("no file name")?.to_string_lossy())?
            }
        };
        if patch.trim().is_empty() {
            bail!("no changes against HEAD");
        }
        let language = file_language(path).highlight;
        let files = DiffFile::parse(&patch)
            .map_err(|e| anyhow::anyhow!("{}", e.message()))?
            .into_iter()
            .map(|f| f.with_language(language))
            .collect();
        let review = at.map(|at| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let stamp = rnotes::stamp(&at.workspace).and_then(|(_, t)| t);
            let notes = rnotes::list(&at.workspace).map(|f| file_notes(&f.notes, &at)).unwrap_or_default();
            (at, text, notes, stamp)
        });
        Ok(LoadedDiff { files, first_added: first_added_line(&patch), patch: patch.clone(), changes: ui::change_blocks(&patch), review, stamp: mtime(path) })
    }

    pub fn new(path: PathBuf, loaded: LoadedDiff, split: bool, cx: &mut Context<Self>) -> Self {
        let LoadedDiff { files, patch, first_added, changes, review, stamp } = loaded;
        let sel = diff_select::new();
        sel.borrow_mut().set_files(&files, &patch, &[DiffSide::Modified]);
        let (additions, deletions) = counts(&files);
        let highlight_theme = cx.theme().highlight_theme.clone();
        let mode = if split { DiffMode::Split } else { DiffMode::Unified };
        let state = cx.new(|cx| {
            DiffState::new(files, cx).with_mode(mode).with_syntax_highlighter(move |language, text| {
                let mut highlighter = SyntaxHighlighter::new(language);
                highlighter.update(None, &Rope::from(text), None);
                highlighter.styles(&(0..text.len()), &*highlight_theme)
            })
        });
        let title = format!("{} (diff)", path.file_name().unwrap_or_default().to_string_lossy());
        let review = review.map(|(at, text, notes, notes_stamp)| FileReview { at, text, notes, notes_stamp, show_done: false });
        // Follows the disk: an agent editing the file or ticking a note in REVIEW.md.
        let poll = cx.spawn(async move |this: WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            if this.update(cx, |t, cx| t.poll(cx)).is_err() {
                break;
            }
        });
        Self {
            path,
            title: title.into(),
            state,
            additions,
            deletions,
            review,
            composer: None,
            message: None,
            sel,
            first_added,
            changes,
            change_ix: None,
            file_stamp: stamp,
            refreshing: false,
            _poll: poll,
        }
    }

    pub fn mode(&self, cx: &App) -> DiffMode {
        self.state.read(cx).mode()
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        let next = if self.mode(cx) == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
        self.state.update(cx, |s, cx| s.set_mode(next, cx));
        cx.notify();
    }

    /// F7 / Shift+F7.
    pub fn next_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.changes > 0 {
            self.change_ix = Some(match (self.change_ix, forward) {
                (None, _) => 0,
                (Some(i), true) => (i + 1).min(self.changes - 1),
                (Some(i), false) => i.saturating_sub(1),
            });
            cx.notify();
        }
        self.state.update(cx, |s, cx| if forward { s.next_change(cx) } else { s.previous_change(cx) });
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        if mtime(&self.path) != self.file_stamp {
            return self.refresh(cx);
        }
        let Some(r) = &self.review else { return };
        let stamp = rnotes::stamp(&r.at.workspace).and_then(|(_, t)| t);
        if stamp != r.notes_stamp {
            self.reload_notes(cx);
        }
    }

    /// Reads the patch again in the background (the file changed on disk).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { DiffTab::load(&path) }).await;
            let _ = this.update(cx, |t, cx| {
                t.refreshing = false;
                t.file_stamp = mtime(&t.path);
                match loaded {
                    Ok(l) => t.apply(l, cx),
                    Err(e) => t.message = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, loaded: LoadedDiff, cx: &mut Context<Self>) {
        (self.additions, self.deletions) = counts(&loaded.files);
        self.first_added = loaded.first_added;
        self.changes = loaded.changes;
        self.change_ix = None;
        self.sel.borrow_mut().set_files(&loaded.files, &loaded.patch, &[DiffSide::Modified]);
        self.state.update(cx, |s, cx| s.set_files(loaded.files, cx));
        if let (Some(r), Some((_, text, notes, stamp))) = (self.review.as_mut(), loaded.review) {
            (r.text, r.notes, r.notes_stamp) = (text, notes, stamp);
        }
        self.message = None;
    }

    fn reload_notes(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.review else { return };
        r.notes_stamp = rnotes::stamp(&r.at.workspace).and_then(|(_, t)| t);
        match rnotes::list(&r.at.workspace) {
            Ok(f) => r.notes = file_notes(&f.notes, &r.at),
            Err(e) => self.message = Some(e),
        }
        cx.notify();
    }

    /// Applies a REVIEW.md change made here.
    fn notes_changed(&mut self, result: Result<orbit_core::notes::NotesFile, String>, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.review else { return false };
        let ok = match result {
            Ok(f) => {
                r.notes = file_notes(&f.notes, &r.at);
                r.notes_stamp = rnotes::stamp(&r.at.workspace).and_then(|(_, t)| t);
                self.message = None;
                true
            }
            Err(e) => {
                self.message = Some(e);
                false
            }
        };
        cx.notify();
        ok
    }

    /// The diff's path for line positions (the patch's file).
    fn diff_path(&self, cx: &App) -> Option<SharedString> {
        self.state.read(cx).files().first().map(|f| f.path().clone())
    }

    /// Opens the note composer on `range` (the `+` of a line or selection).
    pub fn comment_on(&mut self, range: &DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        if range.side() != DiffSide::Modified || range.end_side() != DiffSide::Modified {
            self.message = Some("Notes go on the new code: pick lines on the right side".into());
            return cx.notify();
        }
        let (start, end) = (range.start().min(range.end()) as u32, range.start().max(range.end()) as u32);
        self.open_composer(ComposerKind::New { start, end }, "", window, cx);
    }

    /// Comments on the selected lines, or the first added line without a selection.
    pub fn comment_on_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.sel.borrow().range(self.state.read(cx));
        let range = match (selected, self.diff_path(cx)) {
            (Some(r), _) => r,
            (None, Some(path)) => DiffLineRange::new(path, DiffSide::Modified, self.first_added.unwrap_or(1) as usize, self.first_added.unwrap_or(1) as usize),
            (None, None) => return,
        };
        self.comment_on(&range, window, cx);
    }

    pub fn open_composer(&mut self, kind: ComposerKind, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_none() {
            self.message = Some("Notes need a file in a workspace repository".into());
            return cx.notify();
        }
        let placeholder = match kind {
            ComposerKind::New { .. } => "What should change here? (saved to REVIEW.md)",
            ComposerKind::Edit(_) => "Edit note",
            ComposerKind::Reply(_) => "Reply (appended to the note)",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(initial.to_string()));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.submit_composer(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.composer = Some(Composer { kind, input, _sub: sub });
        cx.notify();
    }

    pub fn cancel_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer = None;
        window.focus(&self.state.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(composer), Some(r)) = (&self.composer, &self.review) else { return };
        let text = composer.input.read(cx).value().to_string();
        let ws = r.at.workspace.clone();
        let result = match &composer.kind {
            ComposerKind::New { start, end } => {
                rnotes::add(&ws, &r.at.repo, &r.at.rel, *start, *end, &text, rnotes::snippet(&r.text, *start, *end))
            }
            ComposerKind::Edit(id) => rnotes::edit(&ws, id, &text),
            ComposerKind::Reply(id) => match r.notes.iter().find(|n| &n.id == id) {
                Some(n) => rnotes::reply(&ws, n, &text),
                None => Err("that note changed meanwhile; try again".into()),
            },
        };
        if self.notes_changed(result, cx) {
            self.cancel_composer(window, cx);
        }
    }

    pub fn set_done(&mut self, id: &str, done: bool, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        let result = rnotes::set_done(&r.at.workspace, id, done);
        self.notes_changed(result, cx);
    }

    pub fn delete_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(r) = &self.review else { return };
        let result = rnotes::delete(&r.at.workspace, &[id.to_string()]);
        self.notes_changed(result, cx);
    }

    pub fn toggle_done_shown(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = &mut self.review {
            r.show_done = !r.show_done;
            cx.notify();
        }
    }

    /// Scrolls to a note's line (from the review list).
    pub fn reveal_line(&mut self, line: u32, cx: &mut Context<Self>) {
        if let Some(path) = self.diff_path(cx) {
            self.state.update(cx, |s, cx| s.scroll_to_line(DiffLinePosition::new(path, DiffSide::Modified, line as usize), cx));
        }
    }

    /// Note annotations, the composer's, and the ranges they cover.
    fn annotations(&self, cx: &App) -> (Vec<DiffAnnotation>, Vec<DiffDecoration>) {
        let (Some(r), Some(path)) = (&self.review, self.diff_path(cx)) else { return (vec![], vec![]) };
        let mut annotations = vec![];
        let mut ranges = vec![];
        for (note, at) in r.shown() {
            annotations.push(DiffAnnotation::line(ElementId::Name(note.id.clone().into()), DiffLinePosition::new(path.clone(), DiffSide::Modified, at.end as usize)));
            if at.end > at.start {
                ranges.push((at.start, at.end));
            }
        }
        if let Some(ComposerKind::New { start, end }) = self.composer.as_ref().map(|c| &c.kind) {
            annotations.push(DiffAnnotation::line("composer", DiffLinePosition::new(path.clone(), DiffSide::Modified, *end as usize)));
            ranges.push((*start, *end));
        }
        let tint = theme::accent_soft();
        let decorations = ranges
            .into_iter()
            .map(|(s, e)| DiffDecoration::new(DiffLineRange::new(path.clone(), DiffSide::Modified, s as usize, e as usize)).with_background(tint))
            .collect();
        (annotations, decorations)
    }
}

fn counts(files: &[DiffFile]) -> (usize, usize) {
    files.iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions()))
}

type NoteAction = Box<dyn Fn(&mut DiffTab, &mut Window, &mut Context<DiffTab>)>;

/// The card under a note's line, or the composer's.
fn render_annotation(tab: &WeakEntity<DiffTab>, annotation: &DiffAnnotation, cx: &mut App) -> gpui::AnyElement {
    let Some(entity) = tab.upgrade() else { return div().into_any_element() };
    let t = entity.read(cx);
    let ElementId::Name(id) = annotation.id() else { return div().into_any_element() };
    let Some(r) = &t.review else { return div().into_any_element() };
    if id.as_ref() == "composer" {
        let Some(composer) = &t.composer else { return div().into_any_element() };
        let label = match composer.kind {
            ComposerKind::New { start, end } if start < end => format!("Note on lines {start}–{end}"),
            ComposerKind::New { end, .. } => format!("Note on line {end}"),
            _ => String::new(),
        };
        return card(false).child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(label)).child(composer_box(tab, composer, "Add note")).into_any_element();
    }
    let shown = r.shown();
    let Some(ix) = shown.iter().position(|(n, _)| n.id == id.as_ref()) else { return div().into_any_element() };
    let (note, at) = shown[ix].clone();
    let k = ix + 1;
    let act = |name: &str, label: &str, tone: Tone, f: NoteAction| {
        let tab = tab.clone();
        ui::button(format!("note:{k}:{name}"), label.to_string(), tone).on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            let _ = tab.update(cx, |t, cx| f(t, window, cx));
        })
    };
    let (id_done, id_reply, id_edit, id_del) = (note.id.clone(), note.id.clone(), note.id.clone(), note.id.clone());
    let body = note.body.clone();
    let done = note.done;
    let mut head = div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("Note"))
        .when(done, |d| d.child(ui::pill("Done", c(theme::OK))))
        .when(at.moved, |d| d.child(ui::pill("moved", c(theme::TEXT_DIM))))
        .when(at.lost, |d| d.child(ui::pill("code changed", c(theme::WARN))))
        .when(at.start != at.end, |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child(format!("Lines {}–{}", at.start, at.end))))
        .child(div().flex_1());
    let editing = t.composer.as_ref().filter(|c| matches!(&c.kind, ComposerKind::Edit(i) | ComposerKind::Reply(i) if *i == note.id));
    if editing.is_none() {
        head = head
            .child(act("done", if done { "Reopen" } else { "Resolve" }, Tone::Plain, Box::new(move |t, _, cx| t.set_done(&id_done, !done, cx))))
            .child(act("reply", "Reply", Tone::Plain, Box::new(move |t, w, cx| t.open_composer(ComposerKind::Reply(id_reply.clone()), "", w, cx))))
            .child(act("edit", "Edit", Tone::Plain, Box::new(move |t, w, cx| t.open_composer(ComposerKind::Edit(id_edit.clone()), &body, w, cx))))
            .child(act("delete", "Delete", Tone::Danger, Box::new(move |t, _, cx| t.delete_note(&id_del, cx))));
    }
    let mut el = card(done).child(head);
    let editing_body = matches!(editing.map(|c| &c.kind), Some(ComposerKind::Edit(_)));
    if !editing_body {
        el = el.children(note.body.split("\n\n").map(|para| {
            let reply = para.strip_prefix(rnotes::REPLY_PREFIX);
            div()
                .text_sm()
                .when(reply.is_some(), |d| d.pl_3().border_l_2().border_color(c(theme::BORDER_STRONG)))
                .text_color(c(if done { theme::TEXT_DIM } else { theme::TEXT }))
                .child(reply.unwrap_or(para).to_string())
        }));
    }
    if let Some(composer) = editing {
        let label = if editing_body { "Save" } else { "Reply" };
        el = el.child(composer_box(tab, composer, label));
    }
    el.into_any_element()
}

fn card(done: bool) -> gpui::Div {
    div()
        .mx_3()
        .my_1()
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .rounded(px(6.))
        .border_1()
        .border_color(c(if done { theme::BORDER } else { theme::BORDER_STRONG }))
        .bg(c(theme::PANEL_RAISED))
        .font_family(theme::UI_FONT)
}

fn composer_box(tab: &WeakEntity<DiffTab>, composer: &Composer, submit: &str) -> impl IntoElement {
    let (t1, t2, t3) = (tab.clone(), tab.clone(), tab.clone());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .on_key_down(move |ev: &KeyDownEvent, window, cx| {
            if ev.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = t3.update(cx, |t, cx| t.cancel_composer(window, cx));
            }
        })
        .child(div().relative().child(Input::new(&composer.input)).child(probe("composer:input")))
        .child(
            div()
                .flex()
                .gap_2()
                .child(ui::button("composer:submit", submit.to_string(), Tone::Primary).on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = t1.update(cx, |t, cx| t.submit_composer(window, cx));
                }))
                .child(ui::button("composer:cancel", "Cancel", Tone::Plain).on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = t2.update(cx, |t, cx| t.cancel_composer(window, cx));
                })),
        )
}

impl Render for DiffTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let split = self.mode(cx) == DiffMode::Split;
        let weak = cx.entity().downgrade();
        let (annotations, decorations) = self.annotations(cx);
        let note_counts = self.review.as_ref().map(|r| (r.notes.iter().filter(|n| !n.done).count(), r.notes.len(), r.show_done));
        let mut diff = Diff::new(&self.state).size_full();
        let mut on_comment: Option<diff_select::OnComment> = None;
        if self.review.is_some() {
            let (w1, w2) = (weak.clone(), weak.clone());
            diff = diff.annotations(annotations).decorations(decorations).render_annotation(move |a, _, cx| render_annotation(&w1, a, cx));
            on_comment = Some(Rc::new(move |range, window, cx| {
                let _ = w2.update(cx, |t, cx| t.comment_on(range, window, cx));
            }));
        }
        let diff = diff_select::attach(diff, &self.sel, &self.state, &[DiffSide::Modified], on_comment);
        let toolbar_btn = |id: &'static str, label: String| {
            div()
                .id(id)
                .relative()
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(c(theme::ACCENT))
                .hover(|s| s.underline())
                .child(label)
                .child(probe(id))
        };
        let label = match &self.review {
            Some(r) => format!("{}/{}", r.at.repo, r.at.rel),
            None => self.path.to_string_lossy().to_string(),
        };
        let extra = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(11.5))
            .text_color(c(theme::TEXT_DIM))
            .when_some(self.message.clone(), |d, m| d.child(div().text_color(c(theme::WARN)).child(m)))
            .child(div().text_color(c(theme::OK)).child(format!("+{}", self.additions)))
            .child(div().text_color(c(theme::DANGER)).child(format!("−{}", self.deletions)))
            .when_some(note_counts, |d, (open, all, show_done)| {
                d.child(format!("{open} open note{}", if open == 1 { "" } else { "s" })).when(all > open, |d| {
                    d.child(
                        toolbar_btn("diff-show-done", if show_done { "Hide resolved".into() } else { "Show resolved".into() })
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.toggle_done_shown(cx))),
                    )
                })
            });
        let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
        let bar = ui::diff_filebar(ui::DiffBar {
            label,
            nav: (self.changes > 0).then_some((self.change_ix, self.changes)),
            split,
            prev: Box::new(move |_, cx| {
                let _ = w1.update(cx, |t, cx| t.next_change(false, cx));
            }),
            next: Box::new(move |_, cx| {
                let _ = w2.update(cx, |t, cx| t.next_change(true, cx));
            }),
            set_split: Box::new(move |on, _, cx| {
                let _ = w3.update(cx, |t, cx| {
                    if (t.mode(cx) == DiffMode::Split) != on {
                        t.toggle_mode(cx);
                    }
                });
            }),
            extra: Some(extra.into_any_element()),
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(bar)
            .child(div().flex_1().min_h_0().relative().child(diff).child(probe("diff-body")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_added_line() {
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,4 +1,5 @@\n a\n-b\n+B\n+C\n c\n";
        assert_eq!(first_added_line(patch), Some(2));
        assert_eq!(first_added_line("@@ -3,2 +3,1 @@\n x\n-y\n"), None);
        assert_eq!(first_added_line("--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+b\n"), Some(1));
    }
}
