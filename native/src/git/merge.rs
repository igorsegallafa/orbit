//! Squash and merge: nothing runs until "Merge" is pressed. PRs merge one at
//! a time, so a failure (review required, conflicts) stops the rest instead
//! of leaving a feature half merged.
use super::{card, cancel_button, footer, status_row, Flow, Mark};
use crate::review::ui::Tone;
use crate::toast::ToastKind;
use crate::workspace_ui::{plural, spawn, PageTab};
use gpui::{div, prelude::*, ClickEvent, Context, Window};
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum MergeState {
    Idle,
    Merging,
    Merged,
    Error(String),
}

#[derive(Clone)]
pub struct MergeItem {
    pub repo: String,
    pub number: u64,
    pub title: String,
}

pub struct MergeFlow {
    pub scope: String,
    pub base: String,
    pub prs: Vec<(MergeItem, MergeState)>,
    pub running: bool,
}

impl MergeFlow {
    pub fn dump(&self, out: &mut String) {
        let _ = writeln!(out, "  merge flow ({}) into {}{}", self.scope, self.base, if self.running { " running" } else { "" });
        for (p, s) in &self.prs {
            let _ = writeln!(out, "    {} #{} {s:?} {}", p.repo, p.number, p.title);
        }
    }
}

impl PageTab {
    fn merge_flow(&mut self) -> Option<&mut MergeFlow> {
        match self.flow_mut() {
            Some(Flow::Merge(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_merge(&mut self, scope: String, base: String, prs: Vec<MergeItem>, cx: &mut Context<Self>) {
        let prs = prs.into_iter().map(|p| (p, MergeState::Idle)).collect();
        self.modal = Some(crate::workspace_ui::Modal::Flow(Flow::Merge(MergeFlow { scope, base, prs, running: false })));
        cx.notify();
    }

    fn merge_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.merge_flow() else { return };
        if f.running || f.prs.iter().any(|(_, s)| *s != MergeState::Idle) {
            return;
        }
        f.running = true;
        self.merge_next(0, window, cx);
    }

    fn merge_next(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.merge_flow() else { return };
        let Some((item, state)) = f.prs.get_mut(ix) else {
            let (n, base) = (f.prs.len(), f.base.clone());
            f.running = false;
            self.toast(ToastKind::Success, format!("Merged {} into {base}", plural(n, "pull request")), None, window, cx);
            // The merge deleted the remote branches: fetch so the status sees it.
            return self.reload(true, window, cx);
        };
        *state = MergeState::Merging;
        let (scope, repo, number) = (f.scope.clone(), item.repo.clone(), item.number);
        spawn(window, cx, move || orbit_core::prs::merge(&scope, &repo, number), move |t, res, window, cx| {
            let Some(f) = t.merge_flow() else { return };
            let Some((item, state)) = f.prs.get_mut(ix) else { return };
            match res {
                Ok(()) => {
                    *state = MergeState::Merged;
                    t.merge_next(ix + 1, window, cx);
                }
                Err(e) => {
                    let title = format!("Merge stopped at {} #{}", item.repo, item.number);
                    *state = MergeState::Error(e);
                    f.running = false;
                    t.toast(ToastKind::Error, title, Some("See the details in the dialog.".into()), window, cx);
                    t.reload(true, window, cx);
                }
            }
        });
        cx.notify();
    }

    pub(super) fn render_merge(&self, f: &MergeFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col();
        for (p, s) in &f.prs {
            let (mark, detail) = match s {
                MergeState::Idle => (Mark::Pending, p.title.clone()),
                MergeState::Merging => (Mark::Working, "Merging…".into()),
                MergeState::Merged => (Mark::Ok, format!("Squashed into {}", f.base)),
                MergeState::Error(e) => (Mark::Error, e.clone()),
            };
            list = list.child(status_row(mark, format!("{} #{}", p.repo, p.number), detail));
        }
        let started = f.prs.iter().any(|(_, s)| *s != MergeState::Idle);
        let foot = if started && !f.running {
            footer().child(cancel_button("Close", cx))
        } else {
            footer().child(cancel_button("Cancel", cx)).child(
                crate::git::btn_busy("merge-submit", if f.running { "Merging…".to_string() } else { format!("Merge {}", plural(f.prs.len(), "PR")) }, Tone::Primary, f.running)
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.merge_start(w, cx))),
            )
        };
        let hint = format!(
            "Each PR lands in {} as a single commit titled after the PR, and its branch is deleted on GitHub. The workspace stays until you remove it.",
            f.base
        );
        card(520., "Squash and merge", Some(hint.into())).child(list).child(foot)
    }
}
