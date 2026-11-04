//! Push: nothing runs until "Push" is pressed; each repo pushes its branch
//! and sets the upstream. A branch a rebase rewrote is rejected as
//! non-fast-forward, and that row then offers a force push with lease. Repos
//! whose PR was closed are left out unless pushed on request. Afterwards it
//! offers the pull-request flow for the repos without an open PR.
use super::{card, cancel_button, footer, status_row, Flow, Mark};
use crate::review::ui::{self, Tone};
use crate::toast::ToastKind;
use crate::workspace_ui::{plural, spawn, PageTab};
use gpui::{div, prelude::*, ClickEvent, Context, Window};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushStatus {
    Idle,
    Pushing,
    Pushed,
    Skipped,
    Rejected,
    Error,
}

pub struct PushRow {
    pub repo: String,
    pub status: PushStatus,
    pub detail: String,
    /// Left out on purpose (its PR was closed): pushed only on request.
    pub dropped: bool,
}

pub struct PushFlow {
    pub scope: String,
    pub rows: Vec<PushRow>,
    pub running: bool,
    /// Open PRs by repo (None while asking): pushing to those updates them.
    pub open_prs: Option<BTreeMap<String, u64>>,
}

impl PushFlow {
    fn done(&self) -> bool {
        !self.running && self.rows.iter().all(|r| !matches!(r.status, PushStatus::Idle | PushStatus::Pushing))
    }

    pub fn dump(&self, out: &mut String) {
        let _ = writeln!(out, "  push flow ({}){}", self.scope, if self.running { " running" } else { "" });
        for r in &self.rows {
            let _ = writeln!(out, "    {} {:?}{} {}", r.repo, r.status, if r.dropped { " dropped" } else { "" }, r.detail);
        }
        if let Some(p) = &self.open_prs {
            let _ = writeln!(out, "    open prs: {p:?}");
        }
    }
}

/// How a push error reads: up to date, rejected (rewritten branch) or failed.
pub fn classify(err: &str, force: bool) -> (PushStatus, String) {
    if err.contains("up to date") || err.contains("Everything up-to-date") {
        (PushStatus::Skipped, "Already up to date".into())
    } else if !force && (err.contains("non-fast-forward") || err.contains("fetch first") || err.contains("[rejected]")) {
        (PushStatus::Rejected, "Branch diverged from origin (rebased?)".into())
    } else {
        (PushStatus::Error, err.to_string())
    }
}

impl PageTab {
    fn push_flow(&mut self) -> Option<&mut PushFlow> {
        match self.flow_mut() {
            Some(Flow::Push(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_push(&mut self, scope: String, repos: Vec<String>, skipped: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        let mut rows: Vec<PushRow> = repos.into_iter().map(|repo| PushRow { repo, status: PushStatus::Idle, detail: String::new(), dropped: false }).collect();
        rows.extend(skipped.into_iter().map(|(repo, why)| PushRow { repo, status: PushStatus::Skipped, detail: why, dropped: true }));
        self.modal = Some(crate::workspace_ui::Modal::Flow(Flow::Push(PushFlow { scope, rows, running: false, open_prs: None })));
        self.push_check_prs(window, cx);
        cx.notify();
    }

    fn push_check_prs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.push_flow().map(|f| f.scope.clone()) else { return };
        if orbit_core::workspace::repo_scope(&scope).is_some() {
            if let Some(f) = self.push_flow() {
                f.open_prs = Some(BTreeMap::new());
            }
            return;
        }
        spawn(window, cx, move || orbit_core::prs::status(&scope), |t, res, _, _| {
            if let Some(f) = t.push_flow() {
                let open = res.unwrap_or_default().into_iter().filter(|s| s.state == "OPEN").map(|s| (s.repo, s.number)).collect();
                f.open_prs = Some(open);
            }
        });
    }

    /// Pushes `repos` (each in parallel); `force` with lease.
    fn push_run(&mut self, repos: Vec<String>, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.push_flow() else { return };
        if f.running || repos.is_empty() {
            return;
        }
        f.running = true;
        for r in f.rows.iter_mut().filter(|r| repos.contains(&r.repo)) {
            r.status = PushStatus::Pushing;
            r.dropped = false;
        }
        let scope = f.scope.clone();
        let jobs = repos.clone();
        spawn(
            window,
            cx,
            move || {
                std::thread::scope(|s| {
                    let handles: Vec<_> = jobs.iter().map(|repo| s.spawn(|| orbit_core::prs::push(&scope, repo, force))).collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("thread panicked".into()))).collect::<Vec<_>>()
                })
            },
            move |t, results, window, cx| {
                let Some(f) = t.push_flow() else { return };
                f.running = false;
                for (repo, r) in repos.iter().zip(results) {
                    let Some(row) = f.rows.iter_mut().find(|x| &x.repo == repo) else { continue };
                    (row.status, row.detail) = match r {
                        Ok(()) => (PushStatus::Pushed, if force { "Force-pushed" } else { "Pushed" }.into()),
                        Err(e) => classify(&e, force),
                    };
                }
                let count = |s: PushStatus| f.rows.iter().filter(|r| repos.contains(&r.repo) && r.status == s).count();
                let (pushed, failed, rejected) = (count(PushStatus::Pushed), count(PushStatus::Error), count(PushStatus::Rejected));
                if failed > 0 {
                    t.toast(ToastKind::Error, format!("Push failed in {}", plural(failed, "repo")), Some("See the details in the dialog.".into()), window, cx);
                } else if rejected > 0 {
                    let desc = "The branch was rewritten; force push it from the dialog.".to_string();
                    t.toast(ToastKind::Error, format!("Push rejected in {}", plural(rejected, "repo")), Some(desc), window, cx);
                } else if pushed > 0 {
                    let what = if force { "Force-pushed" } else { "Pushed" };
                    t.toast(ToastKind::Success, format!("{what} {}", plural(pushed, "repo")), None, window, cx);
                }
                // A PR may have been opened meanwhile.
                t.push_check_prs(window, cx);
                t.reload(false, window, cx);
            },
        );
        cx.notify();
    }

    fn push_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repos = self.push_flow().map(|f| f.rows.iter().filter(|r| r.status == PushStatus::Idle).map(|r| r.repo.clone()).collect()).unwrap_or_default();
        self.push_run(repos, false, window, cx);
    }

    /// The pull requests page, which creates PRs for the repos without one.
    fn push_to_prs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.push_flow().map(|f| f.scope.clone()) else { return };
        self.close_modal(cx);
        self.with_app(window, cx, move |a, w, cx| {
            if let Some(ix) = a.workspaces.iter().position(|ws| ws.meta.name == scope) {
                if a.selected != Some(ix) {
                    a.select_workspace(ix, cx);
                }
                crate::review::open(a, crate::review::Page::PullRequests, w, cx);
            }
        });
    }

    pub(super) fn render_push(&self, f: &PushFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col();
        for r in &f.rows {
            let mark = match r.status {
                PushStatus::Idle => Mark::Pending,
                PushStatus::Pushing => Mark::Working,
                PushStatus::Pushed | PushStatus::Skipped => Mark::Ok,
                PushStatus::Rejected => Mark::Warn,
                PushStatus::Error => Mark::Error,
            };
            let detail = match r.status {
                PushStatus::Pushing => "Pushing…".to_string(),
                PushStatus::Idle => "Ready to push".to_string(),
                _ => r.detail.clone(),
            };
            let repo = r.repo.clone();
            let row = status_row(mark, r.repo.clone(), detail)
                .when(r.dropped, |d| {
                    let repo = repo.clone();
                    d.child(
                        ui::button(format!("push-anyway:{repo}"), "Push anyway", Tone::Plain)
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.push_run(vec![repo.clone()], false, w, cx))),
                    )
                })
                .when(r.status == PushStatus::Rejected, |d| {
                    d.child(
                        ui::button(format!("push-force:{repo}"), "Force push", Tone::Danger)
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.push_run(vec![repo.clone()], true, w, cx))),
                    )
                });
            list = list.child(row);
        }
        let done = f.done();
        let to_push = f.rows.iter().filter(|r| r.status == PushStatus::Idle).count();
        let pushed: Vec<&str> = f.rows.iter().filter(|r| r.status == PushStatus::Pushed).map(|r| r.repo.as_str()).collect();
        let (without_pr, updated): (Vec<&str>, Vec<&str>) = match &f.open_prs {
            Some(open) => pushed.iter().partition(|r| !open.contains_key(**r)),
            None => (vec![], vec![]),
        };
        let note = (done && !updated.is_empty()).then(|| {
            let open = f.open_prs.as_ref();
            let list = updated.iter().map(|r| format!("{r} #{}", open.and_then(|o| o.get(*r)).copied().unwrap_or_default())).collect::<Vec<_>>().join(", ");
            let lead = if without_pr.is_empty() { "Pushed to the open pull requests" } else { "Updated the open pull requests" };
            div().text_xs().text_color(crate::theme::c(crate::theme::TEXT_DIM)).child(format!("{lead}: {list}."))
        });
        let mut foot = footer();
        if !done {
            foot = foot.child(cancel_button("Cancel", cx)).child(
                ui::button("push-submit", if f.running { "Pushing…".to_string() } else { format!("Push {}", plural(to_push, "repo")) }, Tone::Primary)
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.push_all(w, cx))),
            );
        } else {
            foot = foot.child(cancel_button("Close", cx));
            if !without_pr.is_empty() && orbit_core::workspace::repo_scope(&f.scope).is_none() {
                let label = if without_pr.len() == pushed.len() { "Create pull requests →".to_string() } else { format!("Create PR for {} →", without_pr.join(", ")) };
                foot = foot.child(ui::button("push-create-prs", label, Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.push_to_prs(w, cx))));
            }
        }
        card(480., "Push to origin", Some("Each repo pushes its feature branch and sets the upstream. Nothing is force-pushed unless you ask for it.".into()))
            .child(list)
            .children(note)
            .child(foot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_push_errors() {
        assert_eq!(classify("Everything up-to-date", false).0, PushStatus::Skipped);
        assert_eq!(classify("! [rejected] feat/x -> feat/x (non-fast-forward)", false).0, PushStatus::Rejected);
        assert_eq!(classify("! [rejected] (fetch first)", true).0, PushStatus::Error);
        assert_eq!(classify("fatal: no remote", false).0, PushStatus::Error);
    }
}
