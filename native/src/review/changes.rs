//! The Changes page (`ChangesReview.tsx`): every changed file of every repo in
//! one scrolling list of diffs, GitHub "Files changed" style, with the
//! workspace's REVIEW.md notes inline and in the side list.
use super::ui;
use super::{notes as rnotes, Load, ReviewTab};
use crate::lsp::file_language;
use crate::overlay::MenuItem;
use crate::sessions;
use crate::theme::{self, c};
use gpui::{
    div, prelude::*, px, App, ClickEvent, Context, ElementId, Entity, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::diff::{
    Diff, DiffAnnotation, DiffDecoration, DiffFile, DiffHunkSeparator, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState,
};
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::{Input, InputEvent, InputState, Rope};
use gpui_component::{ActiveTheme, IconName};
use orbit_core::git::ChangeEntry;
use orbit_core::notes::{Note, NotesFile};
use orbit_core::workspace::PrRef;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// `repo/path`: a file's key, and its path in the shared diff.
fn file_key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
}

fn split_key(key: &str) -> (&str, &str) {
    key.split_once('/').unwrap_or((key, ""))
}

/// What one background refresh reads.
pub(super) struct Snapshot {
    changes: Vec<(String, Result<Vec<ChangeEntry>, String>)>,
    notes: Result<NotesFile, String>,
    pr_refs: Vec<PrRef>,
    /// `(key, patch)` per changed file, in list order.
    patches: Vec<(String, String)>,
    /// Working-tree text of changed files, where notes are anchored.
    texts: HashMap<String, String>,
}

fn snapshot(ws: String, dir: PathBuf, repos: Vec<String>) -> Snapshot {
    let changes: Vec<(String, Result<Vec<ChangeEntry>, String>)> =
        repos.into_iter().map(|r| (r.clone(), orbit_core::git::changes(&dir.join(&r)))).collect();
    let pr_refs = orbit_core::workspace::load_meta(&dir).map(|m| m.pr_refs).unwrap_or_default();
    let mut patches = vec![];
    let mut texts = HashMap::new();
    for (repo, list) in &changes {
        let repo_dir = dir.join(repo);
        for e in list.iter().flatten() {
            let key = file_key(repo, &e.path);
            let patch = crate::diff_view::git_patch_in(&repo_dir, &e.path, Some(repo)).unwrap_or_default();
            patches.push((key.clone(), patch));
            if e.status != "D" {
                if let Ok(t) = std::fs::read_to_string(repo_dir.join(&e.path)) {
                    texts.insert(key, t);
                }
            }
        }
    }
    Snapshot { changes, notes: rnotes::list(&ws), pr_refs, patches, texts }
}

/// Files marked viewed, per workspace (`orbit.changes.viewed.<ws>`).
fn viewed_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("review-viewed.json"))
}

fn load_viewed(ws: &str) -> BTreeSet<String> {
    let all: BTreeMap<String, BTreeSet<String>> =
        viewed_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    all.get(ws).cloned().unwrap_or_default()
}

fn save_viewed(ws: &str, keys: &BTreeSet<String>) {
    let Some(path) = viewed_path() else { return };
    let mut all: BTreeMap<String, BTreeSet<String>> =
        std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    all.insert(ws.to_string(), keys.clone());
    if let Ok(json) = serde_json::to_vec(&all) {
        let _ = std::fs::write(path, json);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComposerKind {
    New { key: String, start: u32, end: u32 },
    Edit(String),
}

pub struct Composer {
    pub kind: ComposerKind,
    pub input: Entity<InputState>,
    _sub: Subscription,
}

/// The Changes page's own state.
pub struct ChangesView {
    pub diff: Entity<DiffState>,
    sig: u64,
    pub viewed: BTreeSet<String>,
    texts: HashMap<String, String>,
    /// First added line per file: where a note goes without a selection.
    first_added: HashMap<String, u32>,
    pub composer: Option<Composer>,
    pub sel: crate::diff_select::Handle,
}

impl ChangesView {
    pub fn new(ws: &str, cx: &mut App) -> Self {
        let highlight_theme = cx.theme().highlight_theme.clone();
        let diff = cx.new(|cx| {
            DiffState::new(Vec::<DiffFile>::new(), cx)
                .with_mode(DiffMode::Unified)
                .with_context_lines(Some(3))
                .with_min_collapsed_lines(6)
                .with_expansion_lines(20)
                .with_syntax_highlighter(move |language, text| {
                    let mut highlighter = SyntaxHighlighter::new(language);
                    highlighter.update(None, &Rope::from(text), None);
                    highlighter.styles(&(0..text.len()), &*highlight_theme)
                })
        });
        Self { diff, sig: 0, viewed: load_viewed(ws), texts: HashMap::new(), first_added: HashMap::new(), composer: None, sel: crate::diff_select::new() }
    }
}

impl ReviewTab {
    pub(super) fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        if self.changes_busy {
            return;
        }
        self.changes_busy = true;
        for (_, l) in &mut self.changes {
            if matches!(l, Load::Idle) {
                *l = Load::Loading;
            }
        }
        let (ws, dir, repos) = (self.workspace.clone(), self.dir.clone(), self.repos.clone());
        cx.spawn(async move |this, cx| {
            let snap = cx.background_spawn(async move { snapshot(ws, dir, repos) }).await;
            let _ = this.update(cx, |t, cx| {
                t.changes_busy = false;
                t.apply_snapshot(snap, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        self.changes = snap.changes.into_iter().map(|(r, res)| (r, res.map_or_else(Load::Failed, Load::Ready))).collect();
        match snap.notes {
            Ok(f) => self.notes = Some(f),
            Err(e) => self.message = Some(e),
        }
        self.pr_refs = snap.pr_refs;
        self.chg.texts = snap.texts;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        snap.patches.hash(&mut h);
        let sig = h.finish();
        if sig == self.chg.sig {
            return;
        }
        self.chg.sig = sig;
        self.chg.first_added =
            snap.patches.iter().filter_map(|(k, p)| crate::diff_view::first_added_line(p).map(|l| (k.clone(), l))).collect();
        let parsed: Vec<(Vec<DiffFile>, &str)> = snap
            .patches
            .iter()
            .map(|(key, patch)| {
                let language = file_language(Path::new(split_key(key).1)).highlight;
                (DiffFile::parse(patch).unwrap_or_default().into_iter().map(|f| f.with_language(language)).collect(), patch.as_str())
            })
            .collect();
        let per_patch: Vec<(&[DiffFile], &str)> = parsed.iter().map(|(f, p)| (f.as_slice(), *p)).collect();
        self.chg.sel.borrow_mut().set_patches(&per_patch, &[DiffSide::Modified]);
        let files: Vec<DiffFile> = parsed.iter().flat_map(|(f, _)| f.iter().cloned()).collect();
        let viewed = self.chg.viewed.clone();
        self.chg.diff.update(cx, |s, cx| {
            s.set_files(files, cx);
            for key in &viewed {
                s.set_file_collapsed(key, true, cx);
            }
        });
    }

    pub fn open_notes(&self) -> Vec<&Note> {
        self.notes.iter().flat_map(|f| f.notes.iter()).filter(|n| !n.done).collect()
    }

    fn shown_notes(&self) -> Vec<Note> {
        self.notes.iter().flat_map(|f| &f.notes).filter(|n| self.show_done || !n.done).cloned().collect()
    }

    fn in_diff(&self, key: &str, cx: &App) -> bool {
        self.chg.diff.read(cx).files().iter().any(|f| f.path().as_ref() == key)
    }

    fn notes_result(&mut self, result: Result<NotesFile, String>, cx: &mut Context<Self>) -> bool {
        let ok = match result {
            Ok(f) => {
                self.notes = Some(f);
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

    pub fn set_note_done(&mut self, id: &str, done: bool, cx: &mut Context<Self>) {
        let r = rnotes::set_done(&self.workspace, id, done);
        self.notes_result(r, cx);
    }

    pub fn delete_notes(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let r = rnotes::delete(&self.workspace, ids);
        self.notes_result(r, cx);
    }

    fn set_viewed(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.chg.viewed.insert(key.to_string());
        } else {
            self.chg.viewed.remove(key);
        }
        save_viewed(&self.workspace, &self.chg.viewed);
        self.chg.diff.update(cx, |s, cx| s.set_file_collapsed(key, on, cx));
        cx.notify();
    }

    /// The chevron: un-views a viewed file, else folds it.
    fn toggle_file(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.chg.viewed.contains(key) {
            return self.set_viewed(key, false, cx);
        }
        self.chg.diff.update(cx, |s, cx| {
            let collapsed = s.is_file_collapsed(key);
            s.set_file_collapsed(key, !collapsed, cx)
        });
        cx.notify();
    }

    fn scroll_to_file(&mut self, key: &str, cx: &mut Context<Self>) {
        self.chg.diff.update(cx, |s, cx| {
            s.set_file_collapsed(key, false, cx);
            s.scroll_to_file(key, cx);
        });
    }

    fn open_in_editor(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (repo, rel) = split_key(key);
        let path = self.dir.join(repo).join(rel);
        self.with_app(window, cx, |a, w, cx| {
            a.open_file(path, false, w, cx);
        });
    }

    /// A note's file: its diff here while it has changes, else the file itself.
    fn jump_to_note(&mut self, note: &Note, window: &mut Window, cx: &mut Context<Self>) {
        let key = file_key(&note.repo, &note.path);
        if self.in_diff(&key, cx) {
            let line = rnotes::anchor(note, self.chg.texts.get(&key).map(String::as_str)).start;
            return self.chg.diff.update(cx, |s, cx| {
                s.set_file_collapsed(&key, false, cx);
                s.scroll_to_line(DiffLinePosition::new(key.clone(), DiffSide::Modified, line as usize), cx);
            });
        }
        let path = self.dir.join(&note.repo).join(&note.path);
        let line = note.start;
        self.with_app(window, cx, |a, w, cx| {
            if let Some(e) = a.open_file(path, false, w, cx) {
                let p = lsp_types::Position::new(line.saturating_sub(1), 0);
                e.update(cx, |e, cx| e.reveal(lsp_types::Range::new(p, p), w, cx));
            }
        });
    }

    fn open_composer(&mut self, kind: ComposerKind, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match kind {
            ComposerKind::New { .. } => "What should change here? (saved to REVIEW.md)",
            ComposerKind::Edit(_) => "Edit note",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(initial.to_string()));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.submit_composer(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.chg.composer = Some(Composer { kind, input, _sub: sub });
        cx.notify();
    }

    /// The `+` beside a line or a selection.
    fn comment_on(&mut self, range: &DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        if range.side() != DiffSide::Modified || range.end_side() != DiffSide::Modified {
            self.message = Some("Notes go on the new code: pick lines on the right side".into());
            return cx.notify();
        }
        let (start, end) = (range.start().min(range.end()) as u32, range.start().max(range.end()) as u32);
        self.open_composer(ComposerKind::New { key: range.path().to_string(), start, end }, "", window, cx);
    }

    /// The selected lines, else the first file's first changed line.
    pub fn comment_on_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.chg.diff.read(cx);
        let range = self.chg.sel.borrow().range(state).or_else(|| {
            let key = state.files().first()?.path().clone();
            let line = *self.chg.first_added.get(key.as_ref())? as usize;
            Some(DiffLineRange::new(key, DiffSide::Modified, line, line))
        });
        match range {
            Some(r) => self.comment_on(&r, window, cx),
            None => {
                self.message = Some("Select lines on the new code to leave a note".into());
                cx.notify();
            }
        }
    }

    fn cancel_composer(&mut self, cx: &mut Context<Self>) {
        self.chg.composer = None;
        cx.notify();
    }

    fn submit_composer(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = &self.chg.composer else { return };
        let text = composer.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let result = match &composer.kind {
            ComposerKind::New { key, start, end } => {
                let (repo, rel) = split_key(key);
                let snippet = rnotes::snippet(self.chg.texts.get(key).map_or("", String::as_str), *start, *end);
                rnotes::add(&self.workspace, repo, rel, *start, *end, &text, snippet)
            }
            ComposerKind::Edit(id) => rnotes::edit(&self.workspace, id, &text),
        };
        if self.notes_result(result, cx) {
            self.cancel_composer(cx);
        }
    }

    /// "Fix N notes with agent": the workspace's agent sessions, or a new one.
    fn fix_menu(&self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = &self.notes else { return };
        let prompt = rnotes::prompt(std::path::Path::new(&file.file));
        let count = self.open_notes().len();
        let ws = self.workspace.clone();
        let agents: Vec<(String, String)> = sessions::store(cx)
            .read(cx)
            .sessions()
            .filter(|(m, v)| m.workspace == ws && m.cmd.is_some() && v.is_some())
            .map(|(m, _)| (m.id.clone(), m.session_name.clone()))
            .collect();
        let mut items: Vec<MenuItem> = agents
            .into_iter()
            .map(|(id, name)| {
                let prompt = prompt.clone();
                MenuItem::new(format!("Send to {name}"), move |a, _, cx| {
                    let Some(view) = sessions::store(cx).read(cx).view(&id) else { return };
                    view.read(cx).send(prompt.as_bytes());
                    view.read(cx).send(b"\r");
                    let s = if count == 1 { "" } else { "s" };
                    a.toast(crate::toast::ToastKind::Success, format!("Sent {count} note{s} to {name}"), None, cx);
                })
            })
            .collect();
        items.push(MenuItem::new("New agent session", move |a, w, cx| {
            let bin = orbit_core::config::Config::load().map(|c| c.ai.agent_bin()).unwrap_or_else(|_| "claude".into());
            sessions::new_session(a, Some(&bin), Some(prompt.clone()), w, cx);
        }));
        let at = ev.position;
        self.with_app(window, cx, |a, w, cx| a.open_menu(at, items, w, cx));
    }

    pub(super) fn render_changes(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().child(self.render_side(cx)).child(self.render_diff_col(cx))
    }

    /// `.chg-side`: the file list per repo, then the notes.
    fn render_side(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.changes.iter().map(|(_, l)| l.ready().map_or(0, |v| v.len())).sum::<usize>();
        let loading = self.changes.iter().any(|(_, l)| matches!(l, Load::Loading | Load::Idle));
        let s = |n: usize| if n == 1 { "" } else { "s" };
        let open = self.open_notes();
        let all_notes = self.notes.as_ref().map_or(0, |f| f.notes.len());
        let has_done = self.notes.iter().flat_map(|f| &f.notes).any(|n| n.done);
        let multi = self.repos.len() > 1;

        let mut side = div()
            .id("chg-side")
            .w(px(280.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .pb_3()
            .child(files_label("Changes", if loading { "…".into() } else { format!("{files} file{}", s(files)) }));
        for (repo, load) in &self.changes {
            let Some(list) = load.ready().filter(|l| !l.is_empty()) else { continue };
            if multi {
                side = side.child(
                    div()
                        .pt(px(8.))
                        .px(px(12.))
                        .pb(px(2.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(theme::TEXT_DIM))
                        .child(repo.clone()),
                );
            }
            for e in list {
                side = side.child(self.file_row(repo, e, &open, cx));
            }
        }
        if !loading && files == 0 {
            side = side.child(empty_hint("No changes — working trees clean"));
        }

        let toggle = ui::link("review-show-done", if self.show_done { "Hide done" } else { "Show done" }).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
            t.show_done = !t.show_done;
            cx.notify();
        }));
        side = side.child(
            files_label("Notes", format!("{} open", open.len()))
                .mt(px(14.))
                .pt(px(12.))
                .border_t_1()
                .border_color(c(theme::BORDER))
                .child(toggle),
        );
        let shown = self.shown_notes();
        if shown.is_empty() {
            side = side.child(empty_hint(if all_notes > 0 { "All notes done" } else { "Click + next to a line (or drag across lines) to leave a note" }));
        }
        for (k, n) in shown.into_iter().enumerate() {
            side = side.child(self.note_row(k + 1, n, cx));
        }
        if has_done {
            side = side.child(div().mx(px(12.)).mt(px(6.)).child(ui::link("review-clear-done", "Clear done notes").text_size(px(11.5)).on_click(
                cx.listener(|t, _: &ClickEvent, _, cx| {
                    let ids: Vec<String> = t.notes.iter().flat_map(|f| &f.notes).filter(|n| n.done).map(|n| n.id.clone()).collect();
                    t.delete_notes(&ids, cx);
                }),
            )));
        }
        side
    }

    fn file_row(&self, repo: &str, e: &ChangeEntry, open: &[&Note], cx: &mut Context<Self>) -> impl IntoElement {
        let key = file_key(repo, &e.path);
        let count = open.iter().filter(|n| n.repo == repo && n.path == e.path).count();
        let (dir, name) = match e.path.rsplit_once('/') {
            Some((dir, name)) => (dir.to_string(), name.to_string()),
            None => (String::new(), e.path.clone()),
        };
        let viewed = self.chg.viewed.contains(&key);
        ui::row(format!("review-file:{key}"))
            .h(px(19.))
            .px_0()
            .py_0()
            .rounded(px(3.))
            .flex()
            .items_center()
            .gap(px(7.))
            .when(viewed, |d| d.opacity(0.55))
            .child(ui::status_badge(&e.status))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c(theme::TEXT_DIM))
                    .child(name)
                    .when(!dir.is_empty(), |d| d.child(div().ml(px(6.)).truncate().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(dir))),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .mr(px(8.))
                        .px(px(6.))
                        .rounded(px(8.))
                        .bg(ui::tint(c(theme::WARN), 0.18))
                        .text_color(c(theme::WARN))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(count.to_string()),
                )
            })
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.scroll_to_file(&key, cx)))
    }

    fn note_row(&self, k: usize, n: Note, cx: &mut Context<Self>) -> impl IntoElement {
        let loc = format!(
            "{}:{}",
            n.path.rsplit('/').next().unwrap_or(&n.path),
            if n.start == n.end { n.start.to_string() } else { format!("{}–{}", n.start, n.end) }
        );
        let first = n.body.lines().next().unwrap_or_default().to_string();
        let (id, done) = (n.id.clone(), n.done);
        div()
            .flex()
            .items_start()
            .gap(px(6.))
            .px(px(10.))
            .py(px(4.))
            .child(
                ui::checkbox(format!("review-note:{k}:done"), done)
                    .mt(px(2.))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.set_note_done(&id, !done, cx))),
            )
            .child(
                ui::row(format!("review-note:{k}"))
                    .flex_1()
                    .min_w_0()
                    .px(px(4.))
                    .py(px(1.))
                    .flex()
                    .flex_col()
                    .child(div().font_family(theme::MONO_FONT).text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(loc))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(c(if done { theme::TEXT_FAINT } else { theme::TEXT }))
                            .when(done, |d| d.line_through())
                            .child(first),
                    )
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.jump_to_note(&n, w, cx))),
            )
    }

    /// `.pr-diff-col`: the summary bar and the diffs.
    fn render_diff_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entries: Vec<(String, &ChangeEntry)> =
            self.changes.iter().flat_map(|(r, l)| l.ready().into_iter().flatten().map(move |e| (file_key(r, &e.path), e))).collect();
        let loading = self.changes.iter().any(|(_, l)| matches!(l, Load::Loading | Load::Idle));
        let (add, del) = entries.iter().fold((0, 0), |(a, d), (_, e)| (a + e.added, d + e.deleted));
        let viewed = entries.iter().filter(|(k, _)| self.chg.viewed.contains(k)).count();
        let open = self.open_notes().len();
        let pr_notes = self.pr_refs.iter().any(|p| self.open_notes().iter().any(|n| n.repo == p.repo));
        let s = |n: usize| if n == 1 { "" } else { "s" };

        let summary = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(c(theme::TEXT_DIM))
            .child(format!("{} file{}", entries.len(), s(entries.len())))
            .child(div().text_color(c(theme::OK)).child(format!("+{add}")))
            .child(div().text_color(c(theme::DANGER)).child(format!("−{del}")))
            .when(!entries.is_empty(), |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child(format!("· {viewed}/{} viewed", entries.len()))));
        let fix_label = format!("Fix {}note{} with agent ▾", if open > 0 { format!("{open} ") } else { String::new() }, s(open));
        let bar = div()
            .h(px(31.))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(c(theme::PANEL))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(summary)
            .child(div().flex_1())
            .when_some(self.message.clone(), |d, m| d.child(div().text_xs().text_color(c(theme::WARN)).child(m)))
            .when(pr_notes, |d| {
                d.child(
                    ui::mini_button("review-send-pr", if self.sending { "Sending…" } else { "Send to PR…" }, false, !self.sending)
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.send_to_pr(w, cx))),
                )
            })
            .child({
                let b = ui::mini_button("review-fix", fix_label, true, open > 0);
                if open > 0 {
                    b.on_mouse_down(MouseButton::Left, cx.listener(|t, ev: &MouseDownEvent, w, cx| t.fix_menu(ev, w, cx)))
                } else {
                    b
                }
            });

        let body = if !loading && entries.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .pt(px(80.))
                .text_size(px(13.))
                .text_color(c(theme::TEXT_DIM))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("No local changes"))
                .child("Edits made by you or an agent show up here as they happen.")
                .into_any_element()
        } else if loading && entries.is_empty() {
            super::pr_review::table_loading("chg-diff-spin", "Loading diff…")
        } else {
            self.render_diff(cx).into_any_element()
        };
        div().flex_1().min_w_0().h_full().flex().flex_col().child(bar).child(body)
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let files: Vec<SharedString> = self.chg.diff.read(cx).files().iter().map(|f| f.path().clone()).collect();
        let mut annotations: Vec<DiffAnnotation> =
            files.iter().map(|p| DiffAnnotation::file(ElementId::Name(format!("head:{p}").into()), p.clone())).collect();
        let mut ranges = vec![];
        for n in self.shown_notes() {
            let key = file_key(&n.repo, &n.path);
            if !files.iter().any(|p| p.as_ref() == key) {
                continue;
            }
            let at = rnotes::anchor(&n, self.chg.texts.get(&key).map(String::as_str));
            annotations.push(DiffAnnotation::line(ElementId::Name(n.id.clone().into()), DiffLinePosition::new(key.clone(), DiffSide::Modified, at.end as usize)));
            if at.end > at.start {
                ranges.push((key, at.start, at.end));
            }
        }
        if let Some(ComposerKind::New { key, start, end }) = self.chg.composer.as_ref().map(|c| &c.kind) {
            annotations.push(DiffAnnotation::line("composer", DiffLinePosition::new(key.clone(), DiffSide::Modified, *end as usize)));
            ranges.push((key.clone(), *start, *end));
        }
        let decorations: Vec<DiffDecoration> = ranges
            .into_iter()
            .map(|(k, s, e)| DiffDecoration::new(DiffLineRange::new(k, DiffSide::Modified, s as usize, e as usize)).with_background(theme::accent_soft()))
            .collect();
        let first = files.first().cloned();
        let (w1, w2) = (weak.clone(), weak);
        let diff = Diff::new(&self.chg.diff)
            .size_full()
            .header_visible(false)
            .hunk_separator(DiffHunkSeparator::LineInfo)
            .annotations(annotations)
            .decorations(decorations)
            .render_annotation(move |a, _, cx| render_annotation(&w1, a, first.as_ref(), cx));
        let diff = crate::diff_select::attach(
            diff,
            &self.chg.sel,
            &self.chg.diff,
            &[DiffSide::Modified],
            Some(std::rc::Rc::new(move |range, window, cx| {
                let _ = w2.update(cx, |t, cx| t.comment_on(range, window, cx));
            })),
            cx,
        );
        div()
            .id("chg-scroll")
            .flex_1()
            .min_h_0()
            .pt(px(12.))
            .px(px(16.))
            .child(div().size_full().relative().child(diff).child(crate::probe::probe("chg-diff")))
    }
}

/// `.rv-files-label`: an uppercase label with a count.
fn files_label(label: &'static str, count: String) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(10.))
        .px(px(10.))
        .pb(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(label.to_uppercase())
        .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(count))
}

/// `.git-empty`.
fn empty_hint(text: &'static str) -> gpui::Div {
    div().px(px(6.)).py(px(4.)).mx(px(4.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(text)
}

type NoteAction = Box<dyn Fn(&mut ReviewTab, &mut Window, &mut Context<ReviewTab>)>;

/// A file header, a note or the composer, by annotation id.
fn render_annotation(tab: &WeakEntity<ReviewTab>, a: &DiffAnnotation, first: Option<&SharedString>, cx: &mut App) -> gpui::AnyElement {
    let Some(entity) = tab.upgrade() else { return div().into_any_element() };
    let ElementId::Name(id) = a.id() else { return div().into_any_element() };
    if let Some(key) = id.strip_prefix("head:") {
        let is_first = first.is_some_and(|f| f.as_ref() == key);
        return file_head(&entity, key, is_first, cx).into_any_element();
    }
    let t = entity.read(cx);
    if id.as_ref() == "composer" {
        let Some(composer) = &t.chg.composer else { return div().into_any_element() };
        let ComposerKind::New { start, end, .. } = composer.kind else { return div().into_any_element() };
        let label = if start < end { format!("Note on lines {start}–{end}") } else { format!("Note on line {end}") };
        return thread(false)
            .p(px(12.))
            .gap(px(8.))
            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(label))
            .child(composer_box(tab, composer, "Add note"))
            .into_any_element();
    }
    let Some(note) = t.notes.iter().flat_map(|f| &f.notes).find(|n| n.id == id.as_ref()).cloned() else { return div().into_any_element() };
    let key = file_key(&note.repo, &note.path);
    let at = rnotes::anchor(&note, t.chg.texts.get(&key).map(String::as_str));
    let editing = t.chg.composer.as_ref().filter(|c| c.kind == ComposerKind::Edit(note.id.clone()));
    let act = |name: &'static str, icon: &'static str, f: NoteAction| {
        let tab = tab.clone();
        let danger = name == "delete";
        div()
            .id(name)
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .hover(move |s| s.bg(c(theme::PANEL_RAISED)))
            .child(ui::lucide(icon, 12., c(if danger { theme::DANGER } else { theme::TEXT_DIM })))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                let _ = tab.update(cx, |t, cx| f(t, window, cx));
            })
    };
    let (id_done, id_edit, id_del, done, body) = (note.id.clone(), note.id.clone(), note.id.clone(), note.done, note.body.clone());
    let tab_done = tab.clone();
    let head = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .mb(px(4.))
        .text_size(px(12.))
        .child(ui::checkbox("note-done", done).on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            let _ = tab_done.update(cx, |t, cx| t.set_note_done(&id_done, !done, cx));
        }))
        .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Note"))
        .when(done, |d| d.child(rv_pill("Done", c(theme::TEXT_FAINT))))
        .when(at.moved, |d| d.child(rv_pill("moved", c(theme::TEXT_FAINT))))
        .when(at.lost, |d| d.child(rv_pill("code changed", c(theme::WARN))))
        .when(note.start != note.end, |d| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("Lines {}–{}", note.start, note.end))))
        .child(div().flex_1())
        .when(editing.is_none(), |d| {
            d.child(
                div()
                    .flex()
                    .gap(px(2.))
                    .opacity(0.)
                    .group_hover("rv-note", |s| s.opacity(1.))
                    .child(act("edit", "pencil", Box::new(move |t, w, cx| t.open_composer(ComposerKind::Edit(id_edit.clone()), &body, w, cx))))
                    .child(act("delete", "trash", Box::new(move |t, _, cx| t.delete_notes(std::slice::from_ref(&id_del), cx)))),
            )
        });
    let mut main = div().flex_1().min_w_0().child(head);
    match editing {
        Some(composer) => main = main.child(composer_box(tab, composer, "Save")),
        None => {
            main = main.children(note.body.split("\n\n").map(|para| {
                let reply = para.strip_prefix(rnotes::REPLY_PREFIX);
                div()
                    .text_size(px(12.5))
                    .text_color(c(theme::TEXT))
                    .when(reply.is_some(), |d| d.pl_3().border_l_2().border_color(c(theme::BORDER_STRONG)))
                    .child(reply.unwrap_or(para).to_string())
            }))
        }
    }
    thread(true).group("rv-note").when(done, |d| d.opacity(0.6)).child(div().flex().gap(px(10.)).py(px(10.)).px(px(12.)).child(main)).into_any_element()
}

/// `.chg-file-head`: chevron, status, path, repo, counts and "Viewed".
fn file_head(entity: &Entity<ReviewTab>, key: &str, first: bool, cx: &mut App) -> impl IntoElement {
    let t = entity.read(cx);
    let (repo, rel) = split_key(key);
    let entry = t.changes.iter().find(|(r, _)| r == repo).and_then(|(_, l)| l.ready()).and_then(|l| l.iter().find(|e| e.path == rel)).cloned();
    let viewed = t.chg.viewed.contains(key);
    let collapsed = viewed || t.chg.diff.read(cx).is_file_collapsed(key);
    let (dir, name) = match rel.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), rel.to_string()),
    };
    let (added, deleted, status) = entry.map_or((0, 0, "M".to_string()), |e| (e.added, e.deleted, e.status));
    let weak = entity.downgrade();
    let (k1, k2, k3) = (key.to_string(), key.to_string(), key.to_string());
    let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
    // Covers the annotation's padding; the gap above draws the cards apart.
    div()
        .mx(px(-12.))
        .my(px(-12.))
        .flex()
        .flex_col()
        .bg(c(theme::BG))
        .when(!first, |d| d.child(div().h(px(12.))))
        .child(
            div()
                .h(px(34.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(c(theme::PANEL))
                .border_1()
                .border_color(c(theme::BORDER))
                .rounded_t(px(8.))
                .when(collapsed, |d| d.rounded_b(px(8.)))
                .text_size(px(12.5))
                .child(
                    ui::icon_button(format!("chg-toggle:{key}"), if collapsed { IconName::ChevronUp } else { IconName::ChevronRight }, 12.)
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = w1.update(cx, |t, cx| t.toggle_file(&k1, cx));
                        }),
                )
                .child(ui::status_badge(&status))
                .child(
                    div()
                        .id("chg-path")
                        .min_w_0()
                        .flex()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .font_family(theme::MONO_FONT)
                        .text_size(px(12.))
                        .when(!dir.is_empty(), |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child(dir)))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(name))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = w2.update(cx, |t, cx| t.open_in_editor(&k2, window, cx));
                        }),
                )
                .child(div().flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(repo.to_string()))
                .child(
                    div()
                        .flex()
                        .gap(px(5.))
                        .text_size(px(10.))
                        .when(added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{added}"))))
                        .when(deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{deleted}")))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("chg-viewed")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(c(theme::TEXT_DIM))
                        .child(ui::checkbox(format!("chg-viewed:{key}"), viewed))
                        .child("Viewed")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = w3.update(cx, |t, cx| t.set_viewed(&k3, !viewed, cx));
                        }),
                ),
        )
}

/// `.rv-thread`: the note card; `draft` takes the warn border.
fn thread(draft: bool) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .border_1()
        .border_color(if draft { ui::tint(c(theme::WARN), 0.4) } else { c(theme::BORDER_STRONG) })
        .bg(c(theme::PANEL))
        .shadow(theme::shadow(4., 16., 0.25))
        .font_family(theme::UI_FONT)
        .text_size(px(12.5))
        .text_color(c(theme::TEXT))
}

/// `.rv-pill`.
fn rv_pill(label: &'static str, color: gpui::Rgba) -> gpui::Div {
    div()
        .px(px(7.))
        .rounded(px(999.))
        .border_1()
        .border_color(if color == c(theme::WARN) { ui::tint(color, 0.4) } else { c(theme::BORDER_STRONG) })
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(label)
}

fn composer_box(tab: &WeakEntity<ReviewTab>, composer: &Composer, submit: &'static str) -> impl IntoElement {
    let (t1, t2, t3) = (tab.clone(), tab.clone(), tab.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .on_key_down(move |ev: &KeyDownEvent, _, cx| {
            if ev.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = t3.update(cx, |t, cx| t.cancel_composer(cx));
            }
        })
        .child(div().relative().child(Input::new(&composer.input)).child(crate::probe::probe("composer:input")))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(ui::mini_button("composer:cancel", "Cancel", false, true).on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = t2.update(cx, |t, cx| t.cancel_composer(cx));
                }))
                .child(ui::mini_button("composer:submit", submit, true, true).on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = t1.update(cx, |t, cx| t.submit_composer(window, cx));
                })),
        )
}
