//! Commit: per repo, the changed files with a checkbox each (all in by
//! default) and a message, typed or drafted by the AI agent from the selected
//! files. Repos with files selected commit in parallel; the rest stays in the
//! working tree. The focused file's diff shows on the right; Ctrl+Enter commits.
use super::diff_pane::{DiffPane, Source};
use super::{cancel_button, Flow};
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::review::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
use crate::workspace_ui::icons::icon;
use crate::workspace_ui::kit;
use crate::toast::ToastKind;
use crate::workspace_ui::{plural, spawn, Modal, PageTab};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Window};
use gpui_component::input::{Textarea, TextareaState};
use orbit_core::git::ChangeEntry;
use orbit_core::prs::{worktree_of, worktree_on_branch};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum RepoState {
    Ready,
    Drafting,
    Committing,
    Done,
    Failed(String),
}

pub struct RepoCommit {
    pub repo: String,
    pub files: Load<Vec<ChangeEntry>>,
    pub selected: BTreeSet<String>,
    pub message: Entity<TextareaState>,
    pub state: RepoState,
    /// The AI draft failed: a note only, the message can still be typed.
    pub draft_error: Option<String>,
    pub open: bool,
}

impl RepoCommit {
    fn total(&self) -> usize {
        self.files.ready().map_or(0, Vec::len)
    }

    /// Paths to commit, or none for "everything" (`git add --all`).
    fn paths(&self) -> Vec<String> {
        if self.selected.len() < self.total() {
            self.selected.iter().cloned().collect()
        } else {
            vec![]
        }
    }

    fn pending(&self) -> bool {
        !matches!(self.state, RepoState::Done) && !self.selected.is_empty()
    }
}

pub struct CommitFlow {
    pub scope: String,
    pub repos: Vec<RepoCommit>,
    pub committing: bool,
    /// The file whose diff shows: (repo, path).
    pub focus: Option<(String, String)>,
    pub pane: Entity<DiffPane>,
}

impl CommitFlow {
    pub fn busy(&self) -> bool {
        self.committing || self.repos.iter().any(|r| r.state == RepoState::Drafting)
    }

    fn repo_mut(&mut self, repo: &str) -> Option<&mut RepoCommit> {
        self.repos.iter_mut().find(|r| r.repo == repo)
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "  commit flow ({}){}", self.scope, if self.committing { " committing" } else { "" });
        let _ = writeln!(out, "    focus {:?} diff {:?}", self.focus, self.pane.read(cx).source());
        out.push_str(&self.pane.read(cx).selection_dump(cx));
        for r in &self.repos {
            let files = match &r.files {
                Load::Ready(list) => list
                    .iter()
                    .map(|f| format!("{}{} {}", if r.selected.contains(&f.path) { "[x]" } else { "[ ]" }, f.status, f.path))
                    .collect::<Vec<_>>()
                    .join(", "),
                other => crate::workspace_ui::load_text(other),
            };
            let _ = writeln!(out, "    {} {:?} message {:?}: {files}", r.repo, r.state, r.message.read(cx).value());
            if let Some(e) = &r.draft_error {
                let _ = writeln!(out, "      draft error: {e}");
            }
        }
    }
}

impl PageTab {
    fn commit_flow(&mut self) -> Option<&mut CommitFlow> {
        match self.flow_mut() {
            Some(Flow::Commit(f)) => Some(f),
            _ => None,
        }
    }

    /// Opens the commit modal for `repos` of `scope`; repos without changes drop out.
    pub fn open_commit(&mut self, scope: String, repos: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let rows = repos
            .iter()
            .enumerate()
            .map(|(i, repo)| RepoCommit {
                repo: repo.clone(),
                files: Load::Loading,
                selected: BTreeSet::new(),
                message: cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 8).placeholder(format!("Commit message for {repo}"))),
                state: RepoState::Ready,
                draft_error: None,
                open: i == 0,
            })
            .collect();
        let pane = cx.new(|cx| DiffPane::new("Select a file to see its changes", cx));
        self.modal = Some(Modal::Flow(Flow::Commit(CommitFlow { scope, repos: rows, committing: false, focus: None, pane })));
        for repo in repos {
            self.commit_load(repo, true, window, cx);
        }
        cx.notify();
    }

    /// Reads one repo's changes; `first` selects them all and drops a clean repo.
    fn commit_load(&mut self, repo: String, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        let (scope, r) = (f.scope.clone(), repo.clone());
        spawn(window, cx, move || worktree_of(&scope, &r).and_then(|d| orbit_core::git::changes(&d)), move |t, res, window, cx| {
            let Some(f) = t.commit_flow() else { return };
            match res {
                // A clean repo drops out; with none left the modal closes.
                Ok(files) if files.is_empty() => {
                    f.repos.retain(|x| x.repo != repo);
                    if let Some(r) = f.repos.first_mut() {
                        r.open = true;
                    }
                    if f.repos.is_empty() {
                        t.close_modal(cx);
                        if first {
                            t.toast(ToastKind::Info, "Nothing to commit", None, window, cx);
                        }
                    } else if f.focus.as_ref().is_some_and(|(r, _)| r == &repo) {
                        t.commit_refocus(cx);
                    }
                }
                Ok(files) => {
                    let Some(r) = f.repo_mut(&repo) else { return };
                    let paths: BTreeSet<String> = files.iter().map(|x| x.path.clone()).collect();
                    r.selected = if first { paths.clone() } else { r.selected.intersection(&paths).cloned().collect() };
                    r.files = Load::Ready(files);
                    let gone = f.focus.as_ref().is_some_and(|(fr, fp)| fr == &repo && !paths.contains(fp));
                    if f.focus.is_none() || gone {
                        t.commit_refocus(cx);
                    }
                }
                Err(e) => {
                    if let Some(r) = f.repo_mut(&repo) {
                        r.files = Load::Failed(e);
                    }
                }
            }
        });
    }

    fn commit_toggle_file(&mut self, repo: &str, path: &str, cx: &mut Context<Self>) {
        if let Some(r) = self.commit_flow().and_then(|f| f.repo_mut(repo)) {
            if !r.selected.remove(path) {
                r.selected.insert(path.to_string());
            }
        }
        cx.notify();
    }

    /// Partial or none → all; all → none.
    fn commit_toggle_repo(&mut self, repo: &str, cx: &mut Context<Self>) {
        if let Some(r) = self.commit_flow().and_then(|f| f.repo_mut(repo)) {
            r.selected = if r.selected.len() < r.total() {
                r.files.ready().into_iter().flatten().map(|f| f.path.clone()).collect()
            } else {
                BTreeSet::new()
            };
        }
        cx.notify();
    }

    fn commit_draft(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        let scope = f.scope.clone();
        let Some(r) = f.repo_mut(&repo) else { return };
        if r.selected.is_empty() || r.state == RepoState::Drafting {
            return;
        }
        r.state = RepoState::Drafting;
        r.draft_error = None;
        let (paths, name) = (r.paths(), repo.clone());
        spawn(window, cx, move || orbit_core::agent::commit_message(&scope, &name, &paths), move |t, res, window, cx| {
            let Some(r) = t.commit_flow().and_then(|f| f.repo_mut(&repo)) else { return };
            r.state = RepoState::Ready;
            match res {
                Ok(m) => r.message.update(cx, |i, cx| i.set_value(m.message, window, cx)),
                Err(e) => r.draft_error = Some(e),
            }
        });
        cx.notify();
    }

    fn commit_draft_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repos: Vec<String> = self.commit_flow().map(|f| f.repos.iter().filter(|r| r.pending()).map(|r| r.repo.clone()).collect()).unwrap_or_default();
        for repo in repos {
            self.commit_draft(repo, window, cx);
        }
    }

    fn commit_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        if f.busy() {
            return;
        }
        let ready: Vec<(String, String, Vec<String>)> = f
            .repos
            .iter()
            .filter(|r| r.pending())
            .map(|r| (r.repo.clone(), r.message.read(cx).value().trim().to_string(), r.paths()))
            .collect();
        if ready.is_empty() {
            return;
        }
        if let Some((repo, ..)) = ready.iter().find(|(_, m, _)| m.is_empty()) {
            let repo = repo.clone();
            if let Some(r) = self.commit_flow().and_then(|f| f.repo_mut(&repo)) {
                r.state = RepoState::Failed("Write a commit message first".into());
            }
            return cx.notify();
        }
        let scope = f.scope.clone();
        f.committing = true;
        for (repo, ..) in &ready {
            if let Some(r) = f.repo_mut(repo) {
                r.state = RepoState::Committing;
            }
        }
        let jobs = ready.clone();
        spawn(
            window,
            cx,
            move || {
                std::thread::scope(|s| {
                    let handles: Vec<_> = jobs
                        .iter()
                        .map(|(repo, msg, paths)| {
                            let scope = &scope;
                            s.spawn(move || worktree_on_branch(scope, repo).and_then(|d| orbit_core::git::commit_paths(&d, msg, paths)))
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("thread panicked".into()))).collect::<Vec<_>>()
                })
            },
            move |t, results, window, cx| {
                let Some(f) = t.commit_flow() else { return };
                f.committing = false;
                let mut failures = 0;
                for ((repo, ..), r) in ready.iter().zip(results) {
                    let Some(row) = f.repo_mut(repo) else { continue };
                    match r {
                        Ok(()) => {
                            row.state = RepoState::Done;
                            row.open = false;
                        }
                        Err(e) => {
                            failures += 1;
                            row.state = RepoState::Failed(e);
                        }
                    }
                }
                if failures == 0 {
                    let desc = ready.iter().map(|(r, m, _)| format!("{r}: {}", m.lines().next().unwrap_or_default())).collect::<Vec<_>>().join("\n");
                    t.toast(ToastKind::Success, format!("Committed {}", plural(ready.len(), "repo")), Some(desc), window, cx);
                    t.close_modal(cx);
                } else {
                    t.toast(ToastKind::Error, format!("{failures} commit(s) failed"), Some("See the repos marked failed".into()), window, cx);
                }
                t.reload(false, window, cx);
            },
        );
        cx.notify();
    }

    /// Asks, then discards `paths` of `repo` (new files go to the Trash).
    fn commit_discard(&mut self, repo: String, paths: Vec<String>, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.commit_flow().map(|f| f.scope.clone()) else { return };
        let message = format!("Discard {label}? Modified files go back to their last commit; new files move to the Trash.");
        let page = cx.entity().downgrade();
        let run = Rc::new(move |_: &mut crate::app::OrbitApp, window: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
            let (scope, repo, paths, label) = (scope.clone(), repo.clone(), paths.clone(), label.clone());
            let _ = page.update(cx, |_, cx| {
                spawn(window, cx, move || worktree_of(&scope, &repo).and_then(|d| orbit_core::repo::discard_in(&d, &paths)).map(|_| repo), move |t, res, window, cx| {
                    match res {
                        Ok(repo) => {
                            t.toast(ToastKind::Success, format!("Discarded {label}"), Some("New files went to the Trash.".into()), window, cx);
                            t.commit_load(repo, false, window, cx);
                            t.reload(false, window, cx);
                        }
                        Err(e) => t.toast(ToastKind::Error, "Discard failed", Some(e), window, cx),
                    }
                });
            });
        });
        self.with_app(window, cx, move |a, w, cx| {
            a.open_dialog(
                Dialog {
                    title: "Discard changes".into(),
                    message: message.into(),
                    buttons: vec![
                        DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) },
                        DialogButton { label: "Discard".into(), style: ButtonStyle::Primary, run },
                    ],
                    on_cancel: Rc::new(|_, _, _| {}),
                },
                w,
                cx,
            )
        });
    }

    /// Shows `path` of `repo` in the diff pane.
    fn commit_focus(&mut self, repo: &str, path: &str, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        let Ok(dir) = orbit_core::workspace::repo_path(&f.scope, repo) else { return };
        f.focus = Some((repo.to_string(), path.to_string()));
        let label = format!("{repo}/{path}");
        let source = Source::Working { dir, rel: path.to_string() };
        f.pane.update(cx, |p, cx| p.show(label, source, cx));
        cx.notify();
    }

    /// Focuses the first file of the first repo that has one (or nothing).
    fn commit_refocus(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        let mut first = None;
        for r in &f.repos {
            match &r.files {
                Load::Ready(l) if !l.is_empty() => {
                    first = Some((r.repo.clone(), l[0].path.clone()));
                    break;
                }
                Load::Ready(_) | Load::Failed(_) => {}
                // An earlier repo is still loading: it gets the focus then.
                _ => return,
            }
        }
        match first {
            Some((repo, path)) => self.commit_focus(&repo, &path, cx),
            None => {
                f.focus = None;
                f.pane.update(cx, |p, cx| p.clear(cx));
            }
        }
    }

    /// Ctrl+Enter in the modal: commits when the button would.
    pub fn commit_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.commit_flow() else { return };
        let pending: Vec<&RepoCommit> = f.repos.iter().filter(|r| r.pending()).collect();
        let missing = pending.iter().any(|r| r.message.read(cx).value().trim().is_empty());
        if !f.busy() && !pending.is_empty() && !missing {
            self.commit_submit(window, cx);
        }
    }

    pub(super) fn render_commit(&self, f: &CommitFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let pending: Vec<&RepoCommit> = f.repos.iter().filter(|r| r.pending()).collect();
        let missing = pending.iter().find(|r| r.state != RepoState::Drafting && r.message.read(cx).value().trim().is_empty());
        let busy = f.busy();
        let label = if f.committing {
            "Committing…".to_string()
        } else if let Some(r) = missing {
            format!("Write a message for {}", r.repo)
        } else if pending.is_empty() {
            "Select files to commit".to_string()
        } else {
            format!("Commit {}", plural(pending.len(), "repo"))
        };
        let can_commit = !busy && !pending.is_empty() && missing.is_none();
        let mut list = div()
            .id("commit-list")
            .w(px(420.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_r_1()
            .border_color(c(theme::BORDER));
        if f.repos.is_empty() {
            list = list.child(div().p(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Reading changes…"));
        }
        for r in &f.repos {
            list = list.child(self.render_commit_repo(r, f, cx));
        }
        let head = div()
            .flex_none()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.))
            .px(px(18.))
            .pt(px(16.))
            .pb(px(12.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Commit changes"))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(c(theme::TEXT_DIM))
                            .child("Pick the files that go in, write or draft each message. Unselected changes stay uncommitted."),
                    ),
            )
            .child(
                kit::disabled(kit::button_icon("commit-draft-all", Some(("spark", 12.)), "Draft all with AI", kit::Kind::Secondary, kit::NORMAL), busy || pending.is_empty())
                    .when(!busy && !pending.is_empty(), |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.commit_draft_all(w, cx)))),
            );
        let foot = kit::modal_footer()
            .px(px(18.))
            .child(div().mr_auto().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child("⌘/Ctrl + Enter to commit"))
            .child(cancel_button("Cancel", cx))
            .child(
                kit::disabled(crate::git::btn("commit-submit", label, Tone::Primary), !can_commit)
                    .when(can_commit, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.commit_submit(w, cx)))),
            );
        kit::modal("modal-card", 1180.)
            .key_context(super::COMMIT_CONTEXT)
            .on_action(cx.listener(|t, _: &super::SubmitCommit, w, cx| t.commit_shortcut(w, cx)))
            .max_w(gpui::relative(0.94))
            .h(px(780.))
            .max_h(gpui::relative(0.94))
            .text_size(px(13.))
            .child(head)
            .child(div().flex_1().min_h_0().flex().child(list).child(div().flex_1().min_w_0().h_full().flex().bg(c(theme::BG)).child(f.pane.clone())))
            .child(foot)
    }

    fn render_commit_repo(&self, r: &RepoCommit, f: &CommitFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let committing = f.committing;
        let total = r.total();
        let count = r.selected.len();
        let done = r.state == RepoState::Done;
        let locked = done || committing;
        let repo = r.repo.clone();
        let toggle_open = |repo: String| {
            move |t: &mut PageTab, _: &ClickEvent, _: &mut Window, cx: &mut Context<PageTab>| {
                if let Some(r) = t.commit_flow().and_then(|f| f.repo_mut(&repo)) {
                    r.open = !r.open;
                }
                cx.notify();
            }
        };
        let state = match &r.state {
            RepoState::Drafting => Some(kit::spinner(gpui::ElementId::Name(format!("commit-spin:{repo}").into()), 12., c(theme::ACCENT)).into_any_element()),
            RepoState::Committing => Some(div().child("committing…").into_any_element()),
            RepoState::Done => Some(div().flex().gap(px(4.)).child(div().text_color(c(theme::OK)).child("✓")).child("committed").into_any_element()),
            RepoState::Failed(_) => Some(div().flex().gap(px(4.)).child(div().text_color(c(theme::DANGER)).child("✕")).child("failed").into_any_element()),
            RepoState::Ready => None,
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .child(
                div()
                    .id(gpui::ElementId::Name(format!("commit-open:{repo}").into()))
                    .relative()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon("disclosure", 11., c(theme::TEXT_FAINT)).when(r.open, |s| s.with_transformation(gpui::Transformation::rotate(gpui::percentage(0.25)))))
                    .child(crate::probe::probe(format!("commit-open:{repo}")))
                    .on_click(cx.listener(toggle_open(repo.clone()))),
            )
            .child(
                div()
                    .id(gpui::ElementId::Name(format!("commit-repo:{repo}").into()))
                    .relative()
                    .cursor_pointer()
                    .child(kit::check(total > 0 && count == total, count > 0 && count < total))
                    .child(crate::probe::probe(format!("commit-repo:{repo}")))
                    .on_click(cx.listener({
                        let repo = repo.clone();
                        move |t, _: &ClickEvent, _, cx| {
                            if !locked {
                                t.commit_toggle_repo(&repo, cx)
                            }
                        }
                    })),
            )
            .child(
                div()
                    .id(gpui::ElementId::Name(format!("commit-name:{repo}").into()))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(repo.clone())
                    .on_click(cx.listener(toggle_open(repo.clone()))),
            )
            .child(div().flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(match &r.files {
                Load::Ready(_) => format!("{count} of {}", plural(total, "file")),
                Load::Failed(_) => "unreadable".into(),
                _ => "…".into(),
            }))
            .child(div().ml_auto().flex().items_center().gap(px(4.)).text_size(px(11.)).text_color(c(theme::TEXT_DIM)).children(state))
            .when(!locked && total > 0, |d| {
                let (repo, paths) = (repo.clone(), r.files.ready().into_iter().flatten().map(|f| f.path.clone()).collect::<Vec<_>>());
                d.child(
                    kit::hint(
                        kit::button(format!("commit-discard-all:{repo}"), "Discard all", kit::Kind::Secondary, kit::MINI)
                            .bg(gpui::transparent_black())
                            .border_1()
                            .border_color(kit::alpha(c(theme::DANGER), 0.4))
                            .hover(|s| s.bg(kit::alpha(c(theme::DANGER), 0.15))),
                        "Discard every change in this repo",
                    )
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.commit_discard(repo.clone(), paths.clone(), format!("every change in {repo}"), w, cx))),
                )
            })
            .when(!locked, |d| {
                let repo = repo.clone();
                let off = r.state == RepoState::Drafting || count == 0;
                d.child(
                    kit::hint(
                        kit::disabled(kit::icon_button(format!("commit-draft:{repo}"), "spark", 24., 12., false).border_color(gpui::transparent_black()), off),
                        "Draft this message with AI, from the selected files",
                    )
                    .when(!off, |b| b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.commit_draft(repo.clone(), w, cx)))),
                )
            });
        let files = r.open.then(|| match &r.files {
            Load::Ready(list) => div().flex().flex_col().gap(px(1.)).children(list.iter().map(|file| {
                let on = r.selected.contains(&file.path);
                let focused = f.focus.as_ref().is_some_and(|(fr, fp)| fr == &repo && fp == &file.path);
                let (name, dir) = match file.path.rsplit_once('/') {
                    Some((d, n)) => (n.to_string(), d.to_string()),
                    None => (file.path.clone(), String::new()),
                };
                let id = format!("{repo}/{}", file.path);
                let group = format!("commit-row:{id}");
                let (repo1, repo2, repo3, path1, path2, path3) = (repo.clone(), repo.clone(), repo.clone(), file.path.clone(), file.path.clone(), file.path.clone());
                div()
                    .id(gpui::ElementId::Name(format!("commit-diff:{id}").into()))
                    .group(group.clone())
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .min_w_0()
                    .px(px(6.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .map(|d| if focused { d.bg(theme::accent_soft()) } else { d.hover(|s| s.bg(c(theme::PANEL_HOVER))) })
                    .child(crate::probe::probe(format!("commit-diff:{id}")))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.commit_focus(&repo2, &path2, cx)))
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("commit-file:{id}").into()))
                            .relative()
                            .child(kit::check(on, false))
                            .child(crate::probe::probe(format!("commit-file:{id}")))
                            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                if !locked {
                                    t.commit_toggle_file(&repo1, &path1, cx)
                                }
                            })),
                    )
                    .child(ui::status_badge(&file.status))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .overflow_hidden()
                            .child(div().flex_none().text_color(c(theme::TEXT)).child(name))
                            .when(!dir.is_empty(), |d| d.child(div().ml(px(6.)).min_w_0().truncate().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(dir))),
                    )
                    .when(file.added > 0 || file.deleted > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .flex()
                                .gap(px(5.))
                                .text_size(px(11.))
                                .when(file.added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{}", file.added))))
                                .when(file.deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{}", file.deleted)))),
                        )
                    })
                    .when(!locked, |d| {
                        d.child(
                            div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                                kit::hint(kit::icon_button(format!("commit-discard:{id}"), "undo", 22., 12., true).border_color(gpui::transparent_black()), "Discard this file's changes")
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                        cx.stop_propagation();
                                        let label = path3.rsplit('/').next().unwrap_or(&path3).to_string();
                                        t.commit_discard(repo3.clone(), vec![path3.clone()], label, w, cx)
                                    })),
                            ),
                        )
                    })
            })),
            Load::Failed(e) => div().px(px(6.)).text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone()),
            _ => div().px(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Loading…"),
        });
        let message = if r.state == RepoState::Drafting {
            div()
                .min_h(px(52.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_dashed()
                .border_color(c(theme::BORDER_STRONG))
                .text_size(px(12.))
                .text_color(c(theme::TEXT_FAINT))
                .child("The agent is drafting…")
                .into_any_element()
        } else {
            div()
                .id(gpui::ElementId::Name(format!("commit-msg:{repo}").into()))
                .relative()
                .font_family(theme::MONO_FONT)
                .text_size(px(12.))
                .child(Textarea::new(&r.message).bg(c(theme::BG)).disabled(committing || count == 0))
                .child(crate::probe::probe(format!("commit-msg:{repo}")))
                .into_any_element()
        };
        div()
            .p(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL_RAISED))
            .when(done, |d| d.opacity(0.7))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(head)
            .children(files)
            .when(!done, |d| d.child(message))
            .when(done, |d| {
                d.child(div().px(px(4.)).truncate().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(r.message.read(cx).value().lines().next().unwrap_or_default().to_string()))
            })
            .when_some(if let RepoState::Failed(e) = &r.state { Some(e.clone()) } else { None }, |d, e| d.child(div().text_size(px(11.5)).text_color(c(theme::DANGER)).child(e)))
            .when_some(r.draft_error.clone(), |d, e| {
                let e = e.split_once(": ").map_or(e.as_str(), |(_, b)| b).to_string();
                d.child(div().text_size(px(11.5)).text_color(c(theme::WARN)).child(format!("Couldn't draft with AI ({e}). Write the message yourself.")))
            })
    }
}
