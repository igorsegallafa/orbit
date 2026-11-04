//! Lazy file tree: a flat list of visible rows. A folder's children are read on
//! the background executor when it's expanded, so deep or huge folders never
//! block the UI thread; `.gitignore` is respected through the `ignore` crate.
//!
//! Also: reveal (open the folders down to a path and select it), refresh of
//! open folders after disk changes (keeping what is expanded), and the file
//! operations of the context menu (`FileTreePanel.tsx`) through
//! `orbit_core::files`. What needs the app (menus, toasts, terminals) goes
//! out as a [`TreeRequest`], handled in `file_ops.rs`.
use crate::icons;
use crate::theme::{self, c};
use gpui::{
    actions, div, prelude::*, px, uniform_list, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, KeyBinding, MouseButton, Pixels, Point, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

actions!(
    file_tree,
    [SelectPrev, SelectNext, SelectFirst, SelectLast, ExpandOrDescend, CollapseOrAscend, Confirm, RenameSelected, DeleteSelected, CancelPrompt]
);

const PROMPT_CONTEXT: &str = "FileTreePrompt";

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("FileTree");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("home", SelectFirst, ctx),
        KeyBinding::new("end", SelectLast, ctx),
        KeyBinding::new("right", ExpandOrDescend, ctx),
        KeyBinding::new("left", CollapseOrAscend, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("f2", RenameSelected, ctx),
        KeyBinding::new("delete", DeleteSelected, ctx),
        // The prompt's field binds Escape itself.
        KeyBinding::new("escape", CancelPrompt, Some("FileTreePrompt > Input")),
    ]);
}

pub enum FileTreeEvent {
    /// `preview`: a single click, which reuses the preview tab.
    OpenFile { path: PathBuf, preview: bool },
    /// Every folder load started so far has finished.
    Settled,
}

/// What the tree asks of the app.
pub enum TreeRequest {
    ContextMenu { path: PathBuf, position: Point<Pixels> },
    Notify { error: bool, title: String, detail: Option<String> },
}

struct Entry {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    loading: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PromptKind {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
}

/// The name field above the tree for new files/folders and renames.
struct Prompt {
    kind: PromptKind,
    input: Entity<InputState>,
    error: Option<String>,
    _sub: Subscription,
}

pub struct FileTree {
    entries: Vec<Entry>,
    selected: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The workspace the roots belong to (`orbit_core::files` takes its name).
    workspace: Option<String>,
    /// Folder loads in flight.
    pending: usize,
    /// While set, folders loaded above this depth expand themselves (self-test).
    auto_expand_below: Option<usize>,
    /// Bumped when the roots change, so loads for old roots are dropped.
    generation: u64,
    /// When the current burst of loads started, and how long the last one took.
    loads_started: Option<std::time::Instant>,
    pub last_load_time: Option<std::time::Duration>,
    /// A path being revealed: folders on the way open as they load.
    pending_reveal: Option<PathBuf>,
    prompt: Option<Prompt>,
    /// Copies, reveals and refreshes, for the self-test dump.
    pub log: Vec<String>,
}

impl EventEmitter<FileTreeEvent> for FileTree {}
impl EventEmitter<TreeRequest> for FileTree {}

impl Focusable for FileTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Directory children, dirs first, ignoring what git ignores and `.git` itself.
fn read_children(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .follow_links(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .flatten()
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            (e.into_path(), is_dir)
        })
        .collect();
    out.sort_by(|(a, ad), (b, bd)| {
        bd.cmp(ad).then_with(|| {
            let an = a.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            let bn = b.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            an.cmp(&bn)
        })
    });
    out
}

fn self_test() -> bool {
    crate::terminal::CLIPBOARD_DISABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_string()
}

fn new_entry(path: PathBuf, is_dir: bool, depth: usize) -> Entry {
    Entry { name: file_name(&path).into(), path, depth, is_dir, expanded: false, loading: false }
}

/// Index after the last row of `ix`'s subtree.
fn subtree_end(entries: &[Entry], ix: usize) -> usize {
    let depth = entries[ix].depth;
    entries[ix + 1..].iter().position(|e| e.depth <= depth).map_or(entries.len(), |p| ix + 1 + p)
}

fn is_root(entries: &[Entry], path: &Path) -> bool {
    entries.iter().any(|e| e.depth == 0 && e.path == path)
}

/// The repo (root row) holding `path`, and the path relative to it ("/" separated).
fn locate(entries: &[Entry], path: &Path) -> Option<(String, String)> {
    let root = entries.iter().filter(|e| e.depth == 0).find(|e| path.starts_with(&e.path))?;
    let rel = path.strip_prefix(&root.path).ok()?.to_string_lossy().replace('\\', "/");
    Some((root.name.to_string(), rel))
}

/// Puts `dir`'s new child list in place; returns whether it changed.
fn replace_children(entries: &mut Vec<Entry>, selected: &mut Option<usize>, dir: &Path, children: Vec<(PathBuf, bool)>) -> bool {
    let Some(ix) = entries.iter().position(|e| e.path == dir && e.expanded && !e.loading) else { return false };
    let depth = entries[ix].depth + 1;
    let end = subtree_end(entries, ix);
    let same = {
        let old: Vec<(&Path, bool)> = entries[ix + 1..end].iter().filter(|e| e.depth == depth).map(|e| (e.path.as_path(), e.is_dir)).collect();
        old.len() == children.len() && old.iter().zip(&children).all(|(a, b)| a.0 == b.0 && a.1 == b.1)
    };
    if same {
        return false;
    }
    let selected_path = selected.and_then(|ix| entries.get(ix)).map(|e| e.path.clone());
    // Each old child with its subtree, so expanded folders stay expanded.
    let mut blocks: HashMap<PathBuf, Vec<Entry>> = HashMap::new();
    let mut current: Option<PathBuf> = None;
    for e in entries.drain(ix + 1..end) {
        if e.depth == depth {
            current = Some(e.path.clone());
        }
        if let Some(key) = &current {
            blocks.entry(key.clone()).or_default().push(e);
        }
    }
    let rows: Vec<Entry> = children
        .into_iter()
        .flat_map(|(path, is_dir)| match blocks.remove(&path) {
            Some(block) if block[0].is_dir == is_dir => block,
            _ => vec![new_entry(path, is_dir, depth)],
        })
        .collect();
    entries.splice(ix + 1..ix + 1, rows);
    *selected = match selected_path {
        Some(p) => entries.iter().position(|e| e.path == p).or(Some(ix)),
        None => selected.map(|s| s.min(entries.len().saturating_sub(1))),
    };
    true
}

impl FileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: vec![],
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            workspace: None,
            pending: 0,
            auto_expand_below: None,
            generation: 0,
            loads_started: None,
            last_load_time: None,
            pending_reveal: None,
            prompt: None,
            log: vec![],
        }
    }

    /// One root row per repo. Roots keep the workspace path (a symlink for
    /// shared repos) so every path the tree hands out is under the workspace.
    pub fn set_roots(&mut self, roots: Vec<(String, PathBuf)>, cx: &mut Context<Self>) {
        self.generation += 1;
        self.pending = 0;
        self.pending_reveal = None;
        self.prompt = None;
        self.entries = roots
            .into_iter()
            .map(|(name, path)| Entry { path, name: name.into(), depth: 0, is_dir: true, expanded: false, loading: false })
            .collect();
        self.selected = (!self.entries.is_empty()).then_some(0);
        cx.notify();
    }

    pub fn set_workspace(&mut self, name: Option<String>) {
        self.workspace = name;
    }

    pub fn row_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_settled(&self) -> bool {
        self.pending == 0
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected.and_then(|ix| self.entries.get(ix)).map(|e| e.path.as_path())
    }

    /// The visible rows as indented text, and the selection, prompt and log (self-test dumps).
    pub fn rows_text(&self, limit: usize) -> String {
        let mut out: String = self
            .entries
            .iter()
            .enumerate()
            .take(limit)
            .map(|(ix, e)| {
                let mark = match (e.is_dir, e.expanded) {
                    (true, true) => "v ",
                    (true, false) => "> ",
                    _ => "  ",
                };
                let sel = if self.selected == Some(ix) { "*" } else { " " };
                format!("{sel}{}{mark}{}\n", "  ".repeat(e.depth), e.name)
            })
            .collect();
        if let Some(e) = self.selected.and_then(|ix| self.entries.get(ix)) {
            out.push_str(&format!("--- tree selected: row {} {}\n", self.selected.unwrap_or(0), e.path.display()));
        }
        if let Some(p) = &self.prompt {
            out.push_str(&format!("--- tree prompt: {:?} error {:?}\n", p.kind, p.error));
        }
        for line in self.log.iter().rev().take(8).rev() {
            out.push_str(&format!("--- tree log: {line}\n"));
        }
        out
    }

    fn expand(&mut self, ix: usize, cx: &mut Context<Self>) {
        let entry = &mut self.entries[ix];
        if !entry.is_dir || entry.expanded || entry.loading {
            return;
        }
        entry.expanded = true;
        entry.loading = true;
        if self.pending == 0 {
            self.loads_started = Some(std::time::Instant::now());
        }
        self.pending += 1;
        let path = entry.path.clone();
        let generation = self.generation;
        let read = cx.background_spawn({
            let path = path.clone();
            async move { read_children(&path) }
        });
        cx.spawn(async move |this, cx| {
            let children = read.await;
            let _ = this.update(cx, |this, cx| this.insert_children(generation, &path, children, cx));
        })
        .detach();
        cx.notify();
    }

    fn insert_children(&mut self, generation: u64, dir: &Path, children: Vec<(PathBuf, bool)>, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.pending -= 1;
        // Rows may have moved (or the folder collapsed) while it was loading.
        if let Some(ix) = self.entries.iter().position(|e| e.path == dir) {
            self.entries[ix].loading = false;
            if self.entries[ix].expanded {
                let depth = self.entries[ix].depth + 1;
                let n = children.len();
                let rows = children.into_iter().map(|(path, is_dir)| new_entry(path, is_dir, depth));
                self.entries.splice(ix + 1..ix + 1, rows);
                if let Some(sel) = self.selected.as_mut().filter(|s| **s > ix) {
                    *sel += n;
                }
                if self.auto_expand_below.is_some_and(|limit| depth < limit) {
                    let dirs: Vec<PathBuf> = self.entries[ix + 1..]
                        .iter()
                        .take_while(|e| e.depth >= depth)
                        .filter(|e| e.depth == depth && e.is_dir)
                        .map(|e| e.path.clone())
                        .collect();
                    for d in dirs {
                        if let Some(i) = self.entries.iter().position(|e| e.path == d) {
                            self.expand(i, cx);
                        }
                    }
                }
            }
        }
        self.step_reveal(cx);
        if self.pending == 0 {
            self.last_load_time = self.loads_started.take().map(|t| t.elapsed());
            self.auto_expand_below = None;
            cx.emit(FileTreeEvent::Settled);
        }
        cx.notify();
    }

    fn subtree_end(&self, ix: usize) -> usize {
        subtree_end(&self.entries, ix)
    }

    fn collapse(&mut self, ix: usize) {
        self.entries[ix].expanded = false;
        let end = self.subtree_end(ix);
        self.entries.drain(ix + 1..end);
        self.selected = self.selected.map(|s| if s > ix && s < end { ix } else if s >= end { s - (end - ix - 1) } else { s });
    }

    /// Expands every folder down to `levels` below the roots, in the
    /// background (self-test); `Settled` is emitted when it's done.
    pub fn expand_levels(&mut self, levels: usize, cx: &mut Context<Self>) {
        if levels == 0 {
            return;
        }
        self.auto_expand_below = Some(levels);
        for ix in (0..self.entries.len()).rev() {
            if self.entries[ix].depth == 0 {
                self.expand(ix, cx);
            }
        }
    }

    /// Opens the folders down to `path` (loading them as needed) and selects it.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.pending_reveal = Some(path.to_path_buf());
        self.step_reveal(cx);
    }

    fn step_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_reveal.clone() else { return };
        let best = self.entries.iter().enumerate().filter(|(_, e)| target.starts_with(&e.path)).max_by_key(|(_, e)| e.depth).map(|(ix, _)| ix);
        let Some(ix) = best else {
            self.pending_reveal = None;
            return;
        };
        let e = &self.entries[ix];
        if e.path == target || !e.is_dir || (e.expanded && !e.loading) {
            // Found, or as deep as the tree goes (an ignored folder).
            self.pending_reveal = None;
            self.select(ix, cx);
        } else if !e.expanded {
            self.expand(ix, cx);
        }
        // Loading: `insert_children` steps again.
    }

    /// Re-reads open folders (all of them for `None`), keeping what is expanded below them.
    pub fn refresh(&mut self, dirs: Option<&[PathBuf]>, cx: &mut Context<Self>) {
        let targets: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.is_dir && e.expanded && !e.loading && dirs.is_none_or(|d| d.contains(&e.path)))
            .map(|e| e.path.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        let generation = self.generation;
        let read = cx.background_spawn(async move { targets.into_iter().map(|d| (read_children(&d), d)).collect::<Vec<_>>() });
        cx.spawn(async move |this, cx| {
            let lists = read.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                let mut changed = 0;
                for (children, dir) in lists {
                    changed += this.replace_children(&dir, children) as usize;
                }
                if changed > 0 {
                    this.log.push(format!("refreshed {changed} folder(s)"));
                }
                this.step_reveal(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn replace_children(&mut self, dir: &Path, children: Vec<(PathBuf, bool)>) -> bool {
        replace_children(&mut self.entries, &mut self.selected, dir, children)
    }

    fn open(&mut self, ix: usize, preview: bool, cx: &mut Context<Self>) {
        if self.entries[ix].is_dir {
            if self.entries[ix].expanded {
                self.collapse(ix);
            } else {
                self.expand(ix, cx);
            }
        } else {
            cx.emit(FileTreeEvent::OpenFile { path: self.entries[ix].path.clone(), preview });
        }
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.selected.map_or(0, |s| s.saturating_sub(1));
        self.select(ix, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        let ix = self.selected.map_or(0, |s| (s + 1).min(self.entries.len() - 1));
        self.select(ix, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        if !self.entries.is_empty() {
            self.select(0, cx);
        }
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(last) = self.entries.len().checked_sub(1) {
            self.select(last, cx);
        }
    }

    fn expand_or_descend(&mut self, _: &ExpandOrDescend, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let e = &self.entries[ix];
        if e.is_dir && !e.expanded {
            self.expand(ix, cx);
        } else if e.is_dir && self.entries.get(ix + 1).is_some_and(|n| n.depth > e.depth) {
            self.select(ix + 1, cx);
        }
    }

    fn collapse_or_ascend(&mut self, _: &CollapseOrAscend, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let e = &self.entries[ix];
        if e.is_dir && e.expanded {
            self.collapse(ix);
            cx.notify();
        } else if e.depth > 0 {
            let depth = e.depth;
            if let Some(parent) = self.entries[..ix].iter().rposition(|p| p.depth < depth) {
                self.select(parent, cx);
            }
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.open(ix, false, cx);
        }
    }

    fn rename_selected(&mut self, _: &RenameSelected, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path().filter(|p| self.is_inside_repo(p)).map(Path::to_path_buf) {
            self.start_prompt(PromptKind::Rename(path), window, cx);
        }
    }

    fn delete_selected(&mut self, _: &DeleteSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path().filter(|p| self.is_inside_repo(p)).map(Path::to_path_buf) {
            self.delete(path, cx);
        }
    }

    // ---------- file operations ----------

    /// A repo's root row (which can't be renamed or deleted from here).
    pub fn is_root(&self, path: &Path) -> bool {
        is_root(&self.entries, path)
    }

    pub fn is_inside_repo(&self, path: &Path) -> bool {
        !self.is_root(path) && self.locate(path).is_some()
    }

    pub fn is_dir(&self, path: &Path) -> bool {
        self.entries.iter().find(|e| e.path == path).map_or_else(|| path.is_dir(), |e| e.is_dir)
    }

    /// Workspace, repo and repo-relative path ("/" separated), as `orbit_core::files` takes them.
    pub fn locate(&self, path: &Path) -> Option<(String, String, String)> {
        let (repo, rel) = locate(&self.entries, path)?;
        Some((self.workspace.clone()?, repo, rel))
    }

    pub fn start_prompt(&mut self, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) {
        let initial = match &kind {
            PromptKind::Rename(path) => file_name(path),
            _ => String::new(),
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
        // The name without its extension is selected, as in IDEs.
        let stem = initial.rfind('.').filter(|&i| i > 0).unwrap_or(initial.len());
        input.update(cx, |i, cx| i.set_selected_range(0..stem, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.confirm_prompt(window, cx),
            InputEvent::Blur => this.cancel_prompt(window, cx),
            _ => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        self.prompt = Some(Prompt { kind, input, error: None, _sub: sub });
        cx.notify();
    }

    fn cancel_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn on_cancel_prompt(&mut self, _: &CancelPrompt, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_prompt(window, cx);
    }

    /// Creates or renames with the typed name; errors stay in the prompt.
    pub fn confirm_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.prompt else { return };
        let name = p.input.read(cx).value().trim().to_string();
        let kind = p.kind.clone();
        if name.is_empty() {
            return self.cancel_prompt(window, cx);
        }
        let target = match &kind {
            PromptKind::NewFile(dir) | PromptKind::NewFolder(dir) => dir.clone(),
            PromptKind::Rename(path) => path.clone(),
        };
        let Some((ws, repo, rel)) = self.locate(&target) else { return };
        let result = match &kind {
            PromptKind::NewFile(_) => orbit_core::files::create_node(&ws, &repo, &rel, &name, false),
            PromptKind::NewFolder(_) => orbit_core::files::create_node(&ws, &repo, &rel, &name, true),
            PromptKind::Rename(_) => orbit_core::files::rename_node(&ws, &repo, &rel, &name),
        };
        if let Err(e) = result {
            if let Some(p) = self.prompt.as_mut() {
                p.error = Some(e);
            }
            return cx.notify();
        }
        self.prompt = None;
        window.focus(&self.focus, cx);
        let (parent, created) = match &kind {
            PromptKind::Rename(path) => (path.parent().map(Path::to_path_buf).unwrap_or_default(), path.with_file_name(&name)),
            PromptKind::NewFile(dir) | PromptKind::NewFolder(dir) => (dir.clone(), dir.join(&name)),
        };
        self.log.push(format!("{}: {}", if matches!(kind, PromptKind::Rename(_)) { "renamed" } else { "created" }, created.display()));
        // Show the result: open its folder, refresh it and select the new row.
        if let Some(ix) = self.entries.iter().position(|e| e.path == parent) {
            if self.entries[ix].expanded {
                self.refresh(Some(&[parent]), cx);
            } else {
                self.expand(ix, cx);
            }
        }
        self.pending_reveal = Some(created.clone());
        if matches!(kind, PromptKind::NewFile(_)) {
            cx.emit(FileTreeEvent::OpenFile { path: created, preview: false });
        }
        cx.notify();
    }

    /// Moves to the Trash (recoverable), as the Tauri app.
    pub fn delete(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((ws, repo, rel)) = self.locate(&path).filter(|_| !self.is_root(&path)) else { return };
        let task = cx.background_spawn(async move { orbit_core::files::delete_node(&ws, &repo, &rel) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let name = file_name(&path);
                match result {
                    Ok(()) => {
                        this.log.push(format!("trashed: {}", path.display()));
                        if let Some(parent) = path.parent() {
                            this.refresh(Some(&[parent.to_path_buf()]), cx);
                        }
                        cx.emit(TreeRequest::Notify { error: false, title: format!("Moved {name} to the Trash"), detail: None });
                    }
                    Err(e) => cx.emit(TreeRequest::Notify { error: true, title: format!("Couldn't delete {name}"), detail: Some(e) }),
                }
            });
        })
        .detach();
    }

    /// Absolute (workspace) path, or the repo-relative one.
    pub fn copy_path(&mut self, path: &Path, relative: bool, cx: &mut Context<Self>) {
        let text = match self.locate(path) {
            Some((_, _, rel)) if relative => rel,
            _ => path.to_string_lossy().to_string(),
        };
        self.log.push(format!("copied: {text}"));
        if !self_test() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn reveal_in_file_manager(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.log.push(format!("revealed in file manager: {}", path.display()));
        if !self_test() {
            cx.reveal_path(path);
        }
    }

    // ---------- rendering ----------

    fn render_prompt(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.prompt.as_ref()?;
        let rel = |path: &Path| self.locate(path).map(|(_, repo, rel)| if rel.is_empty() { repo } else { format!("{repo}/{rel}") }).unwrap_or_default();
        let label = match &p.kind {
            PromptKind::NewFile(dir) => format!("New file in {}", rel(dir)),
            PromptKind::NewFolder(dir) => format!("New folder in {}", rel(dir)),
            PromptKind::Rename(path) => format!("Rename {}", file_name(path)),
        };
        Some(
            div()
                .key_context(PROMPT_CONTEXT)
                .on_action(cx.listener(Self::on_cancel_prompt))
                .flex_none()
                .mx_2()
                .mb_2()
                .p(px(6.))
                .rounded(px(6.))
                .bg(c(theme::PANEL_RAISED))
                .border_1()
                .border_color(c(if p.error.is_some() { theme::DANGER } else { theme::ACCENT }))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(label))
                .child(Input::new(&p.input).appearance(false).text_size(px(13.)))
                .children(p.error.clone().map(|e| div().text_xs().text_color(c(theme::DANGER)).child(e))),
        )
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.contains_focused(window, cx);
        div()
            .key_context("FileTree")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::expand_or_descend))
            .on_action(cx.listener(Self::collapse_or_ascend))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::rename_selected))
            .on_action(cx.listener(Self::delete_selected))
            .size_full()
            .flex()
            .flex_col()
            .children(self.render_prompt(cx))
            .child(
                uniform_list(
                    "file-tree",
                    self.entries.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let e = &this.entries[ix];
                                let selected = this.selected == Some(ix);
                                let chevron = match (e.is_dir, e.expanded, e.loading) {
                                    (true, _, true) => "…",
                                    (true, true, _) => "▾",
                                    (true, false, _) => "▸",
                                    _ => " ",
                                };
                                div()
                                    .id(ix)
                                    .relative()
                                    .h(px(22.))
                                    .pl(px(8. + e.depth as f32 * 12.))
                                    .pr_2()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_sm()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .text_color(c(if e.is_dir { theme::TEXT } else { theme::TEXT_DIM }))
                                    .when(selected && focused, |d| d.bg(theme::accent_soft()))
                                    .when(selected && !focused, |d| d.bg(c(theme::PANEL_RAISED)))
                                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                                    .child(div().w(px(10.)).text_color(c(theme::TEXT_FAINT)).child(chevron))
                                    .child(if e.is_dir { icons::folder_icon(e.expanded, 14.) } else { icons::file_icon(&e.name, 14.) })
                                    .child(e.name.clone())
                                    .child(crate::probe::probe(format!("tree:{}", e.name)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                                            window.focus(&this.focus, cx);
                                            this.selected = Some(ix);
                                            // Single click previews a file, double click keeps it open.
                                            let is_dir = this.entries[ix].is_dir;
                                            if !is_dir || ev.click_count == 1 {
                                                this.open(ix, ev.click_count < 2, cx);
                                            }
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                                            window.focus(&this.focus, cx);
                                            this.selected = Some(ix);
                                            cx.notify();
                                            let path = this.entries[ix].path.clone();
                                            cx.emit(TreeRequest::ContextMenu { path, position: ev.position });
                                        }),
                                    )
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(rows: &[(&str, usize, bool, bool)]) -> Vec<Entry> {
        rows.iter()
            .map(|&(p, depth, is_dir, expanded)| Entry { path: p.into(), name: file_name(Path::new(p)).into(), depth, is_dir, expanded, loading: false })
            .collect()
    }

    #[test]
    fn replace_children_keeps_expanded_subtrees_and_the_selection() {
        let mut e = rows(&[("/w/r", 0, true, true), ("/w/r/a", 1, true, true), ("/w/r/a/x.rs", 2, false, false), ("/w/r/b.rs", 1, false, false)]);
        let mut sel = Some(2);
        let new = vec![("/w/r/a".into(), true), ("/w/r/c".into(), true), ("/w/r/b.rs".into(), false)];
        assert!(replace_children(&mut e, &mut sel, Path::new("/w/r"), new));
        let paths: Vec<_> = e.iter().map(|e| e.path.to_string_lossy().to_string()).collect();
        assert_eq!(paths, ["/w/r", "/w/r/a", "/w/r/a/x.rs", "/w/r/c", "/w/r/b.rs"]);
        assert_eq!(sel, Some(2));
        // Removing the folder drops its subtree; the selection falls back to the parent.
        assert!(replace_children(&mut e, &mut sel, Path::new("/w/r"), vec![("/w/r/b.rs".into(), false)]));
        assert_eq!(e.len(), 2);
        assert_eq!(sel, Some(0));
        assert!(!replace_children(&mut e, &mut sel, Path::new("/w/r"), vec![("/w/r/b.rs".into(), false)]));
    }

    #[test]
    fn locates_paths_in_repos() {
        let e = rows(&[("/w/api", 0, true, false), ("/w/web", 0, true, false)]);
        assert_eq!(locate(&e, Path::new("/w/web/src/a.ts")), Some(("web".into(), "src/a.ts".into())));
        assert_eq!(locate(&e, Path::new("/w/web")), Some(("web".into(), "".into())));
        assert!(locate(&e, Path::new("/elsewhere")).is_none());
        assert!(is_root(&e, Path::new("/w/api")) && !is_root(&e, Path::new("/w/api/x")));
    }
}
