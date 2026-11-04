//! Lazy file tree: a flat list of visible rows. A folder's children are read on
//! the background executor when it's expanded, so deep or huge folders never
//! block the UI thread; `.gitignore` is respected through the `ignore` crate.
use crate::icons;
use crate::theme::{self, c};
use gpui::{
    actions, div, prelude::*, px, uniform_list, App, Context, EventEmitter, FocusHandle, Focusable,
    KeyBinding, MouseButton, ScrollStrategy, SharedString, UniformListScrollHandle, Window,
};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

actions!(file_tree, [SelectPrev, SelectNext, ExpandOrDescend, CollapseOrAscend, Confirm]);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("FileTree");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("right", ExpandOrDescend, ctx),
        KeyBinding::new("left", CollapseOrAscend, ctx),
        KeyBinding::new("enter", Confirm, ctx),
    ]);
}

pub enum FileTreeEvent {
    /// `preview`: a single click, which reuses the preview tab.
    OpenFile { path: PathBuf, preview: bool },
    /// Every folder load started so far has finished.
    Settled,
}

struct Entry {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    loading: bool,
}

pub struct FileTree {
    entries: Vec<Entry>,
    selected: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Folder loads in flight.
    pending: usize,
    /// While set, folders loaded above this depth expand themselves (self-test).
    auto_expand_below: Option<usize>,
    /// Bumped when the roots change, so loads for old roots are dropped.
    generation: u64,
    /// When the current burst of loads started, and how long the last one took.
    loads_started: Option<std::time::Instant>,
    pub last_load_time: Option<std::time::Duration>,
}

impl EventEmitter<FileTreeEvent> for FileTree {}

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

impl FileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: vec![],
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            pending: 0,
            auto_expand_below: None,
            generation: 0,
            loads_started: None,
            last_load_time: None,
        }
    }

    /// One root row per repo. Roots keep the workspace path (a symlink for
    /// shared repos) so every path the tree hands out is under the workspace.
    pub fn set_roots(&mut self, roots: Vec<(String, PathBuf)>, cx: &mut Context<Self>) {
        self.generation += 1;
        self.pending = 0;
        self.entries = roots
            .into_iter()
            .map(|(name, path)| Entry { path, name: name.into(), depth: 0, is_dir: true, expanded: false, loading: false })
            .collect();
        self.selected = (!self.entries.is_empty()).then_some(0);
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_settled(&self) -> bool {
        self.pending == 0
    }

    /// The visible rows as indented text (self-test dumps).
    pub fn rows_text(&self, limit: usize) -> String {
        self.entries
            .iter()
            .take(limit)
            .map(|e| {
                let mark = match (e.is_dir, e.expanded) {
                    (true, true) => "v ",
                    (true, false) => "> ",
                    _ => "  ",
                };
                format!("{}{mark}{}\n", "  ".repeat(e.depth), e.name)
            })
            .collect()
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
                let rows = children.into_iter().map(|(path, is_dir)| Entry {
                    name: path.file_name().unwrap_or_default().to_string_lossy().to_string().into(),
                    path,
                    depth,
                    is_dir,
                    expanded: false,
                    loading: false,
                });
                self.entries.splice(ix + 1..ix + 1, rows);
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
        if self.pending == 0 {
            self.last_load_time = self.loads_started.take().map(|t| t.elapsed());
            self.auto_expand_below = None;
            cx.emit(FileTreeEvent::Settled);
        }
        cx.notify();
    }

    fn collapse(&mut self, ix: usize) {
        let depth = self.entries[ix].depth;
        self.entries[ix].expanded = false;
        let end = self.entries[ix + 1..].iter().position(|e| e.depth <= depth).map_or(self.entries.len(), |p| ix + 1 + p);
        self.entries.drain(ix + 1..end);
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
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
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
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.contains_focused(window, cx);
        div()
            .key_context("FileTree")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::expand_or_descend))
            .on_action(cx.listener(Self::collapse_or_ascend))
            .on_action(cx.listener(Self::confirm))
            .size_full()
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
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
    }
}
