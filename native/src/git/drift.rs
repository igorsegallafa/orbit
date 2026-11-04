//! Branch drift: repos that aren't on the workspace's branch. A branch-only
//! (linked) repo's clone is shared, so creating or opening another workspace
//! switches it. This banner switches them back; uncommitted work is stashed
//! only after asking, and Orbit re-applies it when that branch comes back.
//! Repos set to auto-switch go back on their own when the page opens.
use crate::review::ui::Tone;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace_ui::{spawn, Page, PageTab};
use gpui::{div, prelude::*, px, ClickEvent, Context, Window};
use orbit_core::workspace::{self, RepoStatus};

#[derive(Default)]
pub struct Drift {
    /// Repos a switch left alone because of uncommitted work: ask to stash.
    pub dirty: Vec<String>,
    pub busy: bool,
    /// The automatic switch ran for this page.
    pub auto_done: bool,
}

impl PageTab {
    fn drift_parts(&mut self) -> Option<(String, Vec<RepoStatus>, &mut Drift)> {
        match &mut self.page {
            Page::Workspace(p) => Some((p.name.clone(), p.statuses.ready().cloned().unwrap_or_default(), &mut p.drift)),
            _ => None,
        }
    }

    /// Switches the off-branch repos back (`stash`: the pending dirty ones).
    pub fn sync_drift(&mut self, stash: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ws, statuses, drift)) = self.drift_parts() else { return };
        let repos: Vec<String> = if stash {
            drift.dirty.clone()
        } else {
            statuses.iter().filter(|s| s.off_branch && s.operation.is_none()).map(|s| s.repo.clone()).collect()
        };
        self.sync_repos(ws, repos, stash, false, window, cx);
    }

    /// The repos set to auto-switch, once per page, when nothing is in the way.
    pub fn auto_sync_drift(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ws, statuses, drift)) = self.drift_parts() else { return };
        if drift.auto_done {
            return;
        }
        drift.auto_done = true;
        let repos: Vec<String> =
            statuses.iter().filter(|s| s.off_branch && s.auto_switch && !s.dirty && s.operation.is_none()).map(|s| s.repo.clone()).collect();
        if !repos.is_empty() {
            self.sync_repos(ws, repos, false, true, window, cx);
        }
    }

    fn sync_repos(&mut self, ws: String, repos: Vec<String>, stash: bool, auto: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, _, drift)) = self.drift_parts() else { return };
        if repos.is_empty() || drift.busy {
            return;
        }
        drift.busy = true;
        cx.notify();
        spawn(
            window,
            cx,
            move || repos.into_iter().map(|r| (r.clone(), workspace::sync_branch(&ws, &r, stash))).collect::<Vec<_>>(),
            move |t, results, window, cx| {
                let (mut left, mut switched, mut notes) = (vec![], vec![], vec![]);
                for (repo, r) in results {
                    match r {
                        Ok(s) if s.dirty => left.push(repo),
                        Ok(s) => {
                            switched.push(if s.stashed {
                                format!("{repo} (changes stashed)")
                            } else if s.restored {
                                format!("{repo} (stashed changes restored)")
                            } else {
                                repo
                            });
                            notes.extend(s.note);
                        }
                        Err(e) => notes.push(e),
                    }
                }
                if let Some((_, _, drift)) = t.drift_parts() {
                    drift.busy = false;
                    drift.dirty = if auto { vec![] } else { left };
                }
                let desc = (!notes.is_empty()).then(|| notes.join("\n"));
                if !switched.is_empty() {
                    let what = if auto { "Switched automatically" } else { "Switched" };
                    t.toast(ToastKind::Success, format!("{what}: {}", switched.join(", ")), desc, window, cx);
                } else if desc.is_some() {
                    t.toast(ToastKind::Error, "Couldn't switch branches", desc, window, cx);
                }
                if !switched.is_empty() || !notes.is_empty() {
                    t.reload(false, window, cx);
                }
            },
        );
    }

    pub fn render_drift(&self, statuses: &[RepoStatus], drift: &Drift, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let off: Vec<&RepoStatus> = statuses.iter().filter(|s| s.off_branch).collect();
        if off.is_empty() {
            return None;
        }
        let switchable: Vec<&&RepoStatus> = off.iter().filter(|s| s.operation.is_none()).collect();
        let pending: Vec<&String> = drift.dirty.iter().filter(|d| off.iter().any(|s| &s.repo == *d)).collect();
        let lead = if off.len() == 1 { "A repository isn't".to_string() } else { format!("{} repositories aren't", off.len()) };
        let them = if off.len() == 1 { "it" } else { "them" };
        let switch_label = if drift.busy {
            "Switching…".to_string()
        } else if switchable.len() == 1 {
            format!("Switch to {}", switchable[0].expected_branch)
        } else {
            format!("Switch {} repos back", switchable.len())
        };
        let rows = off.iter().map(|s| {
            let mut line = format!("{} is on {}", s.repo, s.branch.as_deref().unwrap_or("a detached HEAD"));
            if let Some(h) = &s.held_by {
                line.push_str(&format!(" (workspace {h})"));
            }
            line.push_str(&format!(", expected {}", s.expected_branch));
            if let Some(op) = &s.operation {
                line.push_str(&format!(" · a {op} is in progress; finish or abort it first"));
            }
            div().text_xs().text_color(c(theme::TEXT_DIM)).child(line)
        });
        Some(
            div()
                .mb(px(16.))
                .px(px(12.))
                .py(px(10.))
                .bg(crate::workspace_ui::kit::alpha(c(theme::WARN), 0.07))
                .text_size(px(12.5))
                .rounded(px(8.))
                .border_1()
                .border_color(crate::workspace_ui::kit::alpha(c(theme::WARN), 0.35))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(div().flex_1().text_color(c(theme::WARN)).child(format!(
                            "{lead} on this workspace's branch. Commit, push, rebase and PRs are blocked for {them} until switched back."
                        )))
                        .when(!switchable.is_empty() && pending.is_empty(), |d| {
                            d.child(crate::git::btn("drift-switch", switch_label, Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.sync_drift(false, w, cx))))
                        }),
                )
                .children(rows)
                .when(!pending.is_empty(), |d| {
                    let names = pending.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                    let verb = if pending.len() == 1 { "has" } else { "have" };
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .child(div().flex_1().text_color(c(theme::TEXT)).child(format!(
                                "{names} {verb} uncommitted changes. Stash them and switch? They come back when Orbit switches to that branch again."
                            )))
                            .child(crate::git::btn("drift-cancel", "Cancel", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                if let Some((_, _, d)) = t.drift_parts() {
                                    d.dirty.clear();
                                }
                                cx.notify();
                            })))
                            .child(crate::git::btn("drift-stash", "Stash and switch", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.sync_drift(true, w, cx)))),
                    )
                }),
        )
    }
}
