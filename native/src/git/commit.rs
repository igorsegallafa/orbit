//! Commit: per repo, the changed files with a checkbox each (all in by
//! default) and a message, typed or drafted by the AI agent from the selected
//! files. Repos with files selected commit in parallel; the rest stays in the
//! working tree. A file's name opens its diff tab (the React modal shows it
//! in a side pane).
use super::{card, cancel_button, footer, Flow};
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::review::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
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
        self.modal = Some(Modal::Flow(Flow::Commit(CommitFlow { scope, repos: rows, committing: false })));
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
                    }
                }
                Ok(files) => {
                    let Some(r) = f.repo_mut(&repo) else { return };
                    let paths: BTreeSet<String> = files.iter().map(|x| x.path.clone()).collect();
                    r.selected = if first { paths } else { r.selected.intersection(&paths).cloned().collect() };
                    r.files = Load::Ready(files);
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

    fn commit_open_diff(&mut self, repo: &str, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.commit_flow().map(|f| f.scope.clone()) else { return };
        let Ok(dir) = orbit_core::workspace::repo_path(&scope, repo) else { return };
        let file = dir.join(path);
        self.with_app(window, cx, move |a, w, cx| crate::review::open_diff_at(a, file, None, w, cx));
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
        let mut body = div().flex().flex_col().gap_2();
        for r in &f.repos {
            body = body.child(self.render_commit_repo(r, f.committing, cx));
        }
        card(720., "Commit changes", Some("Pick the files that go in, write or draft each message. Unselected changes stay uncommitted.".into()))
            .child(
                div().flex().justify_end().child(
                    ui::button("commit-draft-all", "✦ Draft all with AI", Tone::Plain)
                        .when(busy || pending.is_empty(), |d| d.opacity(0.5))
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.commit_draft_all(w, cx))),
                ),
            )
            .when(f.repos.is_empty(), |d| d.child(div().text_xs().text_color(c(theme::TEXT_DIM)).child("Reading changes…")))
            .child(body)
            .child(
                footer().child(cancel_button("Cancel", cx)).child(
                    ui::button("commit-submit", label, if can_commit { Tone::Primary } else { Tone::Plain })
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.commit_submit(w, cx))),
                ),
            )
    }

    fn render_commit_repo(&self, r: &RepoCommit, committing: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let total = r.total();
        let count = r.selected.len();
        let done = r.state == RepoState::Done;
        let locked = done || committing;
        let repo = r.repo.clone();
        let check = if total > 0 && count == total { "✓" } else if count > 0 { "–" } else { " " };
        let state = match &r.state {
            RepoState::Drafting => "drafting…",
            RepoState::Committing => "committing…",
            RepoState::Done => "✓ committed",
            RepoState::Failed(_) => "✕ failed",
            RepoState::Ready => "",
        };
        let head = div()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .child(
                ui::button(format!("commit-open:{repo}"), if r.open { "▾" } else { "▸" }, Tone::Plain).on_click(cx.listener({
                    let repo = repo.clone();
                    move |t, _: &ClickEvent, _, cx| {
                        if let Some(r) = t.commit_flow().and_then(|f| f.repo_mut(&repo)) {
                            r.open = !r.open;
                        }
                        cx.notify();
                    }
                })),
            )
            .child(ui::button(format!("commit-repo:{repo}"), check, Tone::Plain).w(px(22.)).on_click(cx.listener({
                let repo = repo.clone();
                move |t, _: &ClickEvent, _, cx| {
                    if !locked {
                        t.commit_toggle_repo(&repo, cx)
                    }
                }
            })))
            .child(div().font_family(theme::MONO_FONT).child(repo.clone()))
            .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(match &r.files {
                Load::Ready(_) => format!("{count} of {}", plural(total, "file")),
                Load::Failed(_) => "unreadable".into(),
                _ => "…".into(),
            }))
            .child(div().flex_1().text_xs().text_color(c(theme::TEXT_DIM)).child(state))
            .when(!locked && total > 0, |d| {
                let (repo, paths) = (repo.clone(), r.files.ready().into_iter().flatten().map(|f| f.path.clone()).collect::<Vec<_>>());
                d.child(ui::button(format!("commit-discard-all:{repo}"), "Discard all", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    t.commit_discard(repo.clone(), paths.clone(), format!("every change in {repo}"), w, cx)
                })))
            })
            .when(!locked, |d| {
                let repo = repo.clone();
                d.child(ui::button(format!("commit-draft:{repo}"), "✦", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.commit_draft(repo.clone(), w, cx))))
            });
        let files = r.open.then(|| match &r.files {
            Load::Ready(list) => div().pl_6().flex().flex_col().children(list.iter().map(|f| {
                let (letter, color) = ui::status_letter(&f.status);
                let on = r.selected.contains(&f.path);
                let (name, dir) = match f.path.rsplit_once('/') {
                    Some((d, n)) => (n.to_string(), d.to_string()),
                    None => (f.path.clone(), String::new()),
                };
                let id = format!("{repo}/{}", f.path);
                let (repo1, repo2, repo3, path1, path2, path3) = (repo.clone(), repo.clone(), repo.clone(), f.path.clone(), f.path.clone(), f.path.clone());
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(ui::button(format!("commit-file:{id}"), if on { "✓" } else { " " }, Tone::Plain).w(px(20.)).on_click(cx.listener(
                        move |t, _: &ClickEvent, _, cx| {
                            if !locked {
                                t.commit_toggle_file(&repo1, &path1, cx)
                            }
                        },
                    )))
                    .child(div().w(px(12.)).text_color(color).child(letter))
                    .child(
                        ui::row(format!("commit-diff:{id}"))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap_2()
                            .child(div().text_color(c(theme::TEXT)).child(name))
                            .child(div().truncate().text_color(c(theme::TEXT_FAINT)).child(dir))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.commit_open_diff(&repo2, &path2, w, cx))),
                    )
                    .when(f.added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{}", f.added))))
                    .when(f.deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{}", f.deleted))))
                    .when(!locked, |d| {
                        d.child(ui::button(format!("commit-discard:{id}"), "↺", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            let label = path3.rsplit('/').next().unwrap_or(&path3).to_string();
                            t.commit_discard(repo3.clone(), vec![path3.clone()], label, w, cx)
                        })))
                    })
            })),
            Load::Failed(e) => div().pl_6().text_xs().text_color(c(theme::DANGER)).child(e.clone()),
            _ => div().pl_6().text_xs().text_color(c(theme::TEXT_FAINT)).child("Loading…"),
        });
        div()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(c(theme::BORDER))
            .when(done, |d| d.opacity(0.6))
            .flex()
            .flex_col()
            .gap_1()
            .child(head)
            .children(files)
            .when(!done, |d| {
                d.child(
                    div()
                        .id(gpui::ElementId::Name(format!("commit-msg:{repo}").into()))
                        .relative()
                        .child(Textarea::new(&r.message).disabled(committing || count == 0))
                        .child(crate::probe::probe(format!("commit-msg:{repo}"))),
                )
            })
            .when(done, |d| {
                d.child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(r.message.read(cx).value().lines().next().unwrap_or_default().to_string()))
            })
            .when_some(if let RepoState::Failed(e) = &r.state { Some(e.clone()) } else { None }, |d, e| {
                d.child(div().text_xs().text_color(c(theme::DANGER)).child(e))
            })
            .when_some(r.draft_error.clone(), |d, e| {
                d.child(div().text_xs().text_color(c(theme::WARN)).child(format!("Couldn't draft with AI ({e}). Write the message yourself.")))
            })
    }
}
