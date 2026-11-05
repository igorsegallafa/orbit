//! Rebase every repo onto `origin/<base>`, after the user confirms. A repo
//! with conflicts pauses and asks before letting the AI agent resolve them
//! (then `rebase --continue` runs here, so a half resolution never lands);
//! Abort rolls that repo back to its pre-rebase state.
use super::{card, cancel_button, footer, status_row, Flow, Mark};
use crate::review::ui::Tone;
use crate::toast::ToastKind;
use crate::workspace_ui::{plural, spawn, PageTab};
use gpui::{div, prelude::*, px, ClickEvent, Context, Window};
use orbit_core::git::{self, RebaseStatus};
use orbit_core::prs::{worktree_of, worktree_on_branch};
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum RebaseState {
    Idle,
    Rebasing,
    Clean,
    Conflicts(Vec<String>),
    Resolving,
    Done(String),
    Error(String),
}

pub struct RebaseFlow {
    pub scope: String,
    pub base: String,
    pub rows: Vec<(String, RebaseState)>,
    pub started: bool,
}

impl RebaseFlow {
    pub fn busy(&self) -> bool {
        self.rows.iter().any(|(_, s)| matches!(s, RebaseState::Rebasing | RebaseState::Resolving))
    }

    fn settled(&self) -> bool {
        self.started && !self.busy()
    }

    pub fn dump(&self, out: &mut String) {
        let _ = writeln!(out, "  rebase flow ({}) onto origin/{}{}", self.scope, self.base, if self.started { " started" } else { "" });
        for (repo, s) in &self.rows {
            let _ = writeln!(out, "    {repo} {s:?}");
        }
    }
}

impl PageTab {
    fn rebase_flow(&mut self) -> Option<&mut RebaseFlow> {
        match self.flow_mut() {
            Some(Flow::Rebase(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_rebase(&mut self, scope: String, base: String, repos: Vec<String>, cx: &mut Context<Self>) {
        let rows = repos.into_iter().map(|r| (r, RebaseState::Idle)).collect();
        self.modal = Some(crate::workspace_ui::Modal::Flow(Flow::Rebase(RebaseFlow { scope, base, rows, started: false })));
        cx.notify();
    }

    fn rebase_set(&mut self, repo: &str, state: RebaseState) {
        if let Some(row) = self.rebase_flow().and_then(|f| f.rows.iter_mut().find(|(r, _)| r == repo)) {
            row.1 = state;
        }
    }

    fn rebase_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.rebase_flow() else { return };
        if f.started {
            return;
        }
        f.started = true;
        let (scope, base) = (f.scope.clone(), f.base.clone());
        let repos: Vec<String> = f.rows.iter().map(|(r, _)| r.clone()).collect();
        for repo in repos {
            self.rebase_set(&repo, RebaseState::Rebasing);
            let (scope, base, name) = (scope.clone(), base.clone(), repo.clone());
            spawn(window, cx, move || worktree_on_branch(&scope, &name).and_then(|d| git::rebase_onto(&d, &base)), move |t, res, window, cx| {
                t.rebase_set(
                    &repo,
                    match res {
                        Ok(RebaseStatus::Conflicts(files)) if !files.is_empty() => RebaseState::Conflicts(files),
                        Ok(_) => RebaseState::Clean,
                        Err(e) => RebaseState::Error(e),
                    },
                );
                t.rebase_settled(window, cx);
            });
        }
        cx.notify();
    }

    /// Once every repo is through: refresh the page and say how it went.
    fn rebase_settled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.rebase_flow() else { return };
        if !f.settled() {
            return;
        }
        let count = |p: fn(&RebaseState) -> bool| f.rows.iter().filter(|(_, s)| p(s)).count();
        let conflicts = count(|s| matches!(s, RebaseState::Conflicts(_)));
        let errors = count(|s| matches!(s, RebaseState::Error(_)));
        let base = f.base.clone();
        if conflicts > 0 {
            let desc = "Resolve them with AI or abort in the dialog.".to_string();
            self.toast(ToastKind::Info, format!("{} paused with conflicts", plural(conflicts, "repo")), Some(desc), window, cx);
        } else if errors > 0 {
            self.toast(ToastKind::Error, format!("Rebase failed in {}", plural(errors, "repo")), None, window, cx);
        } else {
            self.toast(ToastKind::Success, format!("Rebased onto origin/{base}"), None, window, cx);
        }
        self.reload(false, window, cx);
    }

    /// The agent resolves and stages; `rebase --continue` runs here. Any
    /// failure aborts, so the repo is never left mid-rebase.
    fn rebase_resolve(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.rebase_flow().map(|f| f.scope.clone()) else { return };
        self.rebase_set(&repo, RebaseState::Resolving);
        let name = repo.clone();
        spawn(
            window,
            cx,
            move || {
                let dir = worktree_of(&scope, &name)?;
                let r = orbit_core::agent::resolve_conflicts(&scope, &name).and_then(|summary| git::rebase_continue(&dir).map(|_| summary));
                if r.is_err() {
                    let _ = git::rebase_abort(&dir);
                }
                r
            },
            move |t, res, window, cx| {
                t.rebase_set(
                    &repo,
                    match res {
                        Ok(summary) => RebaseState::Done(if summary.trim().is_empty() { "conflicts resolved by AI".into() } else { summary }),
                        Err(e) => RebaseState::Error(e),
                    },
                );
                t.rebase_settled(window, cx);
            },
        );
        cx.notify();
    }

    fn rebase_abort(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.rebase_flow().map(|f| f.scope.clone()) else { return };
        let name = repo.clone();
        spawn(window, cx, move || worktree_of(&scope, &name).and_then(|d| git::rebase_abort(&d)), move |t, res, window, cx| {
            t.rebase_set(&repo, RebaseState::Error(res.err().unwrap_or_else(|| "aborted — back to pre-rebase state".into())));
            t.reload(false, window, cx);
        });
    }

    pub(super) fn render_rebase(&self, f: &RebaseFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap(px(6.));
        for (repo, s) in &f.rows {
            let (mark, detail) = if !f.started {
                (Mark::Pending, format!("onto origin/{}", f.base))
            } else {
                match s {
                    RebaseState::Idle | RebaseState::Rebasing => (Mark::Working, "rebasing…".into()),
                    RebaseState::Resolving => (Mark::Working, "AI is resolving conflicts…".into()),
                    RebaseState::Clean => (Mark::Ok, "rebased".into()),
                    RebaseState::Done(d) => (Mark::Ok, d.clone()),
                    RebaseState::Conflicts(files) => (Mark::Warn, format!("{} conflicted file(s): {}", files.len(), files.join(", "))),
                    RebaseState::Error(e) => (Mark::Error, e.clone()),
                }
            };
            let conflicts = matches!(s, RebaseState::Conflicts(_));
            let (r1, r2) = (repo.clone(), repo.clone());
            list = list.child(status_row(mark, repo.clone(), detail).when(conflicts, |d| {
                d.child(
                    crate::workspace_ui::kit::hint(crate::git::btn(format!("rebase-ai:{repo}"), "Resolve with AI", Tone::Plain), "AI resolves and stages; continue runs here")
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.rebase_resolve(r1.clone(), w, cx))),
                )
                .child(crate::git::btn(format!("rebase-abort:{repo}"), "Abort", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.rebase_abort(r2.clone(), w, cx))))
            }));
        }
        let foot = if !f.started {
            footer().child(cancel_button("Cancel", cx)).child(
                crate::git::btn("rebase-submit", format!("Rebase {}", plural(f.rows.len(), "repo")), Tone::Primary)
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.rebase_start(w, cx))),
            )
        } else {
            footer().child(cancel_button("Close", cx))
        };
        let hint = (!f.started).then(|| {
            format!("Every repo's feature branch will be rebased onto origin/{} (fetch first). Conflicts pause that repo for review; nothing is force-pushed.", f.base).into()
        });
        card(560., format!("Rebase onto origin/{}", f.base), hint).child(list).child(foot)
    }
}
