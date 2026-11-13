//! A Code Review feature before it is checked out (`FeaturePage.tsx`): the
//! workspace page's layout for PRs that only exist on GitHub.
use super::code_review::{self, with_app};
use super::pr_ui::{self, lucide, tint, Tone};
use crate::app::OrbitApp;
use crate::pages::{Host, PageView};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use crate::workspace_ui::kit;
use gpui::{div, prelude::*, px, AnyElement, App, Context, FontWeight, SharedString, Task, Window};
use orbit_core::github::{PrDetail, PrGroup, PullRequest};
use orbit_core::prs::{RemotePr, WsCheck};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Debug)]
enum StepState {
    Loading,
    Idle,
    Done,
    Running,
    Fail,
}

struct Step {
    key: &'static str,
    icon: &'static str,
    title: &'static str,
    state: StepState,
    detail: String,
    action: Option<(&'static str, Option<&'static str>)>,
    extra: Option<&'static str>,
}

pub struct FeaturePage {
    host: Host,
    pub prs: Vec<PullRequest>,
    workspace: Option<Workspace>,
    /// None while loading.
    pub checks: Option<Vec<WsCheck>>,
    refreshing: bool,
    pub details: HashMap<String, Option<PrDetail>>,
    expanded: Option<String>,
    merged: HashSet<String>,
    /// The stack this feature sits in, bottom first; just this feature when it stands alone.
    stack: Vec<PrGroup>,
    /// Commits each stacked PR lacks from the PR below it, by `key`.
    behind: HashMap<String, u64>,
    /// What "Merge stack" is doing, while it runs.
    stacking: Option<String>,
    _poll: Task<()>,
}

fn key(p: &PullRequest) -> String {
    format!("{}/{}", p.repo, p.number)
}

impl FeaturePage {
    pub fn new(host: Host, prs: Vec<PullRequest>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            if this.update_in(cx, |p, window, cx| p.load_checks(window, cx)).is_err() {
                break;
            }
        });
        let workspace = code_review::workspace_for(&prs, &crate::workspace::list_workspaces());
        let stack = prs.first().map(|p| PrGroup { branch: p.branch.clone(), prs: prs.clone() }).into_iter().collect();
        let mut page = Self {
            host,
            prs,
            workspace,
            checks: None,
            refreshing: false,
            details: HashMap::new(),
            expanded: None,
            merged: HashSet::new(),
            stack,
            behind: HashMap::new(),
            stacking: None,
            _poll: poll,
        };
        page.load_stack(false, window, cx);
        page.load_checks(window, cx);
        page.load_details(window, cx);
        page
    }

    fn load_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let remote: Vec<RemotePr> =
            self.prs.iter().map(|p| RemotePr { repo: p.repo.clone(), owner_repo: p.owner_repo.clone(), number: p.number }).collect();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { orbit_core::prs::group_checks(remote) }).await;
            let _ = this.update(cx, |p, cx| {
                p.refreshing = false;
                p.checks = Some(r);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn load_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for p in self.prs.clone() {
            cx.spawn_in(window, async move |this, cx| {
                let (or, n) = (p.owner_repo.clone(), p.number);
                let d = cx.background_spawn(async move { orbit_core::github::pr_detail(&or, n) }).await;
                let _ = this.update(cx, |page, cx| {
                    page.details.insert(p.repo.clone(), d.ok());
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn load_stack(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mine) = self.prs.first().map(|p| PrGroup { branch: p.branch.clone(), prs: self.prs.clone() }) else { return };
        cx.spawn_in(window, async move |this, cx| {
            let (fresh, stack) = cx
                .background_spawn(async move {
                    // shortcut: finds only stack levels in the Code Review list (updated in the last 7 days); the merge guard still asks GitHub.
                    let mut groups = orbit_core::github::list_prs(force).unwrap_or_default();
                    let at = groups.iter().position(|g| g.branch == mine.branch);
                    let fresh = at.map(|g| groups[g].prs.clone());
                    let g = at.unwrap_or_else(|| {
                        groups.push(mine);
                        groups.len() - 1
                    });
                    (fresh, orbit_core::stack::stack(&groups, g).into_iter().map(|i| groups[i].clone()).collect::<Vec<_>>())
                })
                .await;
            let _ = this.update_in(cx, |p, window, cx| {
                // A merge below retargets this feature's PRs: take their new base.
                if let Some(prs) = fresh {
                    p.prs = prs;
                }
                if p.workspace.is_none() {
                    let all: Vec<PullRequest> = stack.iter().flat_map(|g| g.prs.clone()).collect();
                    p.workspace = code_review::workspace_for(&all, &crate::workspace::list_workspaces());
                }
                p.stack = stack;
                p.load_behind(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_behind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for p in self.stack.iter().flat_map(|g| &g.prs) {
            let Some(below) = self.below(p).cloned() else { continue };
            let (k, or, branch) = (key(p), p.owner_repo.clone(), p.branch.clone());
            cx.spawn_in(window, async move |this, cx| {
                let Ok(n) = cx.background_spawn(async move { orbit_core::stack::behind(&or, &branch, &below.branch) }).await else { return };
                let _ = this.update(cx, |page, cx| {
                    page.behind.insert(k, n);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The open PR `p` is stacked on.
    fn below(&self, p: &PullRequest) -> Option<&PullRequest> {
        self.stack.iter().flat_map(|g| &g.prs).find(|a| a.repo == p.repo && a.branch == p.base && !self.merged.contains(&key(a)))
    }

    fn update_branch(&mut self, p: PullRequest, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (or, n) = (p.owner_repo.clone(), p.number);
            let r = cx.background_spawn(async move { orbit_core::stack::update_branch(&or, n) }).await;
            let _ = this.update_in(cx, |page, window, cx| match r {
                Ok(()) => {
                    host.toast(ToastKind::Success, format!("Updated {} #{} with the PR below", p.repo, p.number), None, cx);
                    page.behind.remove(&key(&p));
                    page.load_checks(window, cx);
                }
                Err(e) => host.error(format!("Couldn't update #{}: likely a conflict, check it out to resolve", p.number), e, cx),
            });
        })
        .detach();
    }

    fn check_of(&self, p: &PullRequest) -> Option<&WsCheck> {
        self.checks.as_ref()?.iter().find(|c| c.repo == p.repo && c.pr_number == p.number)
    }

    /// Why a PR can't be merged now (`mergeBlock`).
    fn merge_block(&self, p: &PullRequest) -> Option<String> {
        if self.merged.contains(&key(p)) {
            return Some("Already merged".into());
        }
        if let Some(b) = self.below(p) {
            return Some(format!("Stacked on #{}: merge it first, or this lands in {} instead of the default branch", b.number, b.branch));
        }
        if self.stacking.is_some() {
            return Some("Merging the stack…".into());
        }
        if p.is_draft {
            return Some("Draft: mark it ready for review on GitHub first".into());
        }
        if self.checks.is_none() {
            return Some("Checking CI…".into());
        }
        match self.check_of(p).map(|c| c.status.as_str()) {
            Some("fail") => Some("Checks are failing".into()),
            Some("running") => Some("Checks are still running".into()),
            _ => None,
        }
    }

    /// Why "Merge stack" can't start: the bottom must be mergeable now, and no level a draft.
    fn stack_block(&self) -> Option<String> {
        let open = |p: &&PullRequest| !self.merged.contains(&key(p));
        if let Some(d) = self.stack.iter().flat_map(|g| &g.prs).filter(open).find(|p| p.is_draft) {
            return Some(format!("#{} is a draft", d.number));
        }
        let bottom = self.stack.iter().find(|g| g.prs.iter().any(|p| open(&p)))?;
        let block = bottom.prs.iter().filter(open).find_map(|p| self.merge_block_any(p));
        block.map(|b| format!("Bottom of the stack: {b}"))
    }

    /// `merge_block` for any stack level: CI is known only for this feature's own PRs.
    fn merge_block_any(&self, p: &PullRequest) -> Option<String> {
        if self.prs.iter().any(|m| key(m) == key(p)) {
            return self.merge_block(p);
        }
        match p.status.as_ref().and_then(|s| s.checks.as_deref()) {
            Some("fail") => Some(format!("checks are failing on #{}", p.number)),
            Some("pending") => Some(format!("checks are still running on #{}", p.number)),
            _ => None,
        }
    }

    /// Bottom-up; each level above the first waits for the checks its restack re-ran. Stops at the first failure.
    fn merge_stack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let levels: Vec<Vec<PullRequest>> = self
            .stack
            .iter()
            .map(|g| g.prs.iter().filter(|p| !self.merged.contains(&key(p))).cloned().collect::<Vec<_>>())
            .filter(|l| !l.is_empty())
            .collect();
        let order = levels.iter().map(|l| l.iter().map(|p| format!("#{}", p.number)).collect::<Vec<_>>().join(" + ")).collect::<Vec<_>>().join(" → ");
        let weak = cx.entity().downgrade();
        let host = self.host.clone();
        self.host.confirm(
            "Merge stack",
            format!("Squash-merges {order}, bottom first. After each merge the next PR is moved onto the default branch and updated, then merged once its checks pass. Stops at the first failure."),
            "Merge stack",
            move |window, cx| {
                let (levels, host, weak) = (levels.clone(), host.clone(), weak.clone());
                window
                    .spawn(cx, async move |cx| {
                        let status = |cx: &mut gpui::AsyncWindowContext, s: Option<String>| {
                            let _ = cx.update(|_, cx| {
                                let _ = weak.update(cx, |page, cx| {
                                    page.stacking = s;
                                    cx.notify();
                                });
                            });
                        };
                        'levels: for (i, level) in levels.into_iter().enumerate() {
                            for p in level {
                                let (or, n) = (p.owner_repo.clone(), p.number);
                                if i > 0 {
                                    status(cx, Some(format!("Waiting for checks on #{n}…")));
                                    let or = or.clone();
                                    if let Err(e) = cx.background_spawn(async move { orbit_core::stack::wait_checks(&or, n) }).await {
                                        let _ = cx.update(|_, cx| host.error(format!("Merge stack stopped at {} #{n}", p.repo), e, cx));
                                        break 'levels;
                                    }
                                }
                                status(cx, Some(format!("Merging #{n}…")));
                                let r = cx.background_spawn(async move { orbit_core::prs::merge_remote(&or, n) }).await;
                                let ok = r.is_ok();
                                let _ = cx.update(|_, cx| match r {
                                    Ok(_) => {
                                        host.toast(ToastKind::Success, format!("Merged {} #{n}", p.repo), None, cx);
                                        let _ = weak.update(cx, |page, _| page.merged.insert(key(&p)));
                                    }
                                    Err(e) => host.error(format!("Merge stack stopped at {} #{n}", p.repo), e, cx),
                                });
                                if !ok {
                                    break 'levels;
                                }
                            }
                        }
                        status(cx, None);
                        let _ = cx.update(|window, cx| {
                            let _ = weak.update(cx, |page, cx| {
                                page.load_stack(true, window, cx);
                                page.load_checks(window, cx);
                            });
                        });
                    })
                    .detach();
            },
            window,
            cx,
        );
    }

    fn open_review(&self, window: &mut Window, cx: &mut App) {
        let (host, prs) = (self.host.clone(), self.prs.clone());
        with_app(&self.host, window, cx, move |a, w, cx| super::pr_review::open(a, host, prs, w, cx));
    }

    fn merge(&mut self, prs: Vec<PullRequest>, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let base = prs.first().map(|p| p.base.clone()).unwrap_or_default();
        let names = prs.iter().map(|p| format!("{} #{}", p.repo, p.number)).collect::<Vec<_>>().join(", ");
        let host = self.host.clone();
        self.host.confirm(
            "Squash and merge",
            format!("Squash {names} into {base} and delete the branch on GitHub."),
            "Squash and merge",
            move |window, cx| {
                let (prs, host, weak) = (prs.clone(), host.clone(), weak.clone());
                window
                    .spawn(cx, async move |cx| {
                        for p in prs {
                            let (or, n) = (p.owner_repo.clone(), p.number);
                            let r = cx.background_spawn(async move { orbit_core::prs::merge_remote(&or, n) }).await;
                            let _ = cx.update(|window, cx| match r {
                                Ok(notes) => {
                                    let desc = (!notes.is_empty()).then(|| notes.join("\n"));
                                    host.toast(ToastKind::Success, format!("Merged {} #{}", p.repo, p.number), desc, cx);
                                    let _ = weak.update(cx, |page, cx| {
                                        page.merged.insert(key(&p));
                                        page.load_checks(window, cx);
                                        page.load_stack(true, window, cx);
                                    });
                                }
                                Err(e) => host.error(format!("Couldn't merge {} #{}", p.repo, p.number), e, cx),
                            });
                        }
                    })
                    .detach();
            },
            window,
            cx,
        );
    }

    fn steps(&self) -> Vec<Step> {
        let checks = self.checks.as_deref();
        let failing = checks.is_some_and(|c| c.iter().any(|c| c.status == "fail"));
        let running = checks.is_some_and(|c| c.iter().any(|c| c.status == "running"));
        let passing = checks.is_some_and(|c| !c.is_empty() && c.iter().all(|c| c.status == "pass" || c.status == "none"));
        let open = self.prs.iter().filter(|p| !p.is_draft).count();
        let drafts = self.prs.len() - open;
        let local = self.workspace.is_some();
        let all_mergeable = self.prs.len() > 1 && self.prs.iter().all(|p| self.merge_block(p).is_none());
        vec![
            Step {
                key: "changes",
                icon: "commit",
                title: "Changes",
                state: StepState::Idle,
                detail: if local { "See the workspace".into() } else { "Check out to see local changes".into() },
                action: None,
                extra: None,
            },
            Step {
                key: "push",
                icon: "push",
                title: "Push",
                state: StepState::Idle,
                detail: if local { "See the workspace".into() } else { "Check out to commit and push".into() },
                action: None,
                extra: None,
            },
            Step {
                key: "prs",
                icon: "pr",
                title: "Pull requests",
                state: StepState::Done,
                detail: if drafts > 0 { format!("{open} open, {drafts} draft") } else { format!("{open} open") },
                action: Some(("View", None)),
                extra: all_mergeable.then_some("Merge all"),
            },
            Step {
                key: "checks",
                icon: "check",
                title: "Checks",
                state: match checks {
                    None => StepState::Loading,
                    Some(_) if failing => StepState::Fail,
                    Some(_) if running => StepState::Running,
                    Some(_) if passing => StepState::Done,
                    Some(_) => StepState::Idle,
                },
                detail: match checks {
                    None => "Checking CI…".into(),
                    Some(_) if failing => "Some checks failed".into(),
                    Some(_) if running => "Running…".into(),
                    Some(_) if passing => "All checks passed".into(),
                    Some(_) => "No checks yet".into(),
                },
                action: failing.then_some(("Investigate all", Some("Check out the feature first: this needs the code on your machine"))),
                extra: failing.then_some("Review"),
            },
        ]
    }

    fn render_step(&self, ix: usize, s: Step, next: bool, last: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon_bg, icon_fg) = match s.state {
            _ if next => (theme::accent_soft(), c(theme::INFO)),
            StepState::Done => (tint(theme::OK, 0.12), c(theme::OK)),
            StepState::Fail => (theme::danger_soft(), c(theme::DANGER)),
            StepState::Running => (c(theme::PANEL_RAISED), c(theme::ACCENT)),
            _ => (c(theme::PANEL_RAISED), c(theme::TEXT_DIM)),
        };
        let icon = match s.state {
            StepState::Done => lucide("check", 13., icon_fg).into_any_element(),
            StepState::Fail => lucide("x", 12., icon_fg).into_any_element(),
            StepState::Running | StepState::Loading => kit::spinner(("ft-step-spin", ix), 14., c(theme::ACCENT)).into_any_element(),
            _ => lucide(s.icon, 15., icon_fg).into_any_element(),
        };
        let detail_color = match s.state {
            StepState::Done => theme::TEXT_DIM,
            StepState::Fail => theme::DANGER,
            _ => theme::TEXT_FAINT,
        };
        let step_key = s.key;
        div()
            .flex_1()
            .min_w_0()
            .min_h(px(108.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(14.))
            .px(px(14.))
            .pb(px(12.))
            .border_1()
            .rounded(px(10.))
            .bg(c(theme::PANEL))
            .border_color(if next {
                tint(theme::ACCENT, 0.5)
            } else if s.state == StepState::Fail {
                tint(theme::DANGER, 0.4)
            } else {
                c(theme::BORDER)
            })
            .when(s.state == StepState::Idle, |d| d.opacity(0.75))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(26.)).flex_none().rounded(px(7.)).bg(icon_bg).flex().items_center().justify_center().child(icon))
                    .child(div().flex_1().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(s.title))
                    .when(!last, |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child("→"))),
            )
            .child(div().text_size(px(12.)).text_color(c(detail_color)).child(s.detail))
            .when(s.action.is_some(), |d| {
                let (label, disabled) = s.action.unwrap_or(("", None));
                d.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .mt(px(2.))
                        .child({
                            let b = pr_ui::button(format!("flow:{step_key}"), label, next && disabled.is_none(), 28.).px(px(12.));
                            let b = if let Some(why) = disabled {
                                pr_ui::tip(b.opacity(0.5).cursor_default(), why).into_any_element()
                            } else {
                                b.on_click(cx.listener(move |p, _, window, cx| p.step_action(step_key, false, window, cx))).into_any_element()
                            };
                            b
                        })
                        .when_some(s.extra, |d, extra| {
                            d.child(
                                pr_ui::button(format!("flow:{step_key}:extra"), extra, false, 28.)
                                    .on_click(cx.listener(move |p, _, window, cx| p.step_action(step_key, true, window, cx))),
                            )
                        }),
                )
            })
            .child(probe(format!("flow-step:{ix}")))
    }

    fn step_action(&mut self, step: &str, extra: bool, window: &mut Window, cx: &mut Context<Self>) {
        match (step, extra) {
            ("prs", true) => {
                let prs = self.prs.clone();
                self.merge(prs, window, cx);
            }
            ("checks", true) => {
                let failing = self.checks.iter().flatten().find(|c| c.status == "fail").map(|c| format!("{}/{}", c.repo, c.pr_number));
                self.expanded = failing;
                cx.notify();
            }
            _ => {}
        }
    }

    fn render_repo(&self, ix: usize, pr: &PullRequest, cx: &mut Context<Self>) -> impl IntoElement {
        let size: AnyElement = match self.details.get(&pr.repo) {
            None => crate::motion::skeleton(px(90.), 10.).into_any_element(),
            Some(None) => div().into_any_element(),
            Some(Some(d)) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .mr(px(4.))
                .text_size(px(11.5))
                .text_color(c(theme::TEXT_FAINT))
                .child(div().text_color(c(theme::OK)).child(format!("+{}", d.additions)))
                .child(div().text_color(c(theme::DANGER)).child(format!("−{}", d.deletions)))
                .child(format!("{} file{}", d.files.len(), if d.files.len() == 1 { "" } else { "s" }))
                .into_any_element(),
        };
        let badge = |label: String| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(9.))
                .py(px(3.))
                .rounded_full()
                .text_size(px(11.5))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .text_color(c(theme::TEXT_DIM))
                .child(label)
        };
        pr_ui::tip(
            div()
                .id(SharedString::from(format!("ft-repo:{ix}")))
                .relative()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(14.))
                .py(px(12.))
                .cursor_pointer()
                .when(ix > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .on_click(cx.listener(|p, _, window, cx| p.open_review(window, cx)))
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(px(8.))
                        .bg(c(theme::BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(lucide("git", 15., c(theme::TEXT_DIM))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(pr.repo.clone()))
                        .child(div().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(pr.branch.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(size)
                        .child(badge(format!("#{}", pr.number)).text_color(c(theme::INFO)).font_weight(FontWeight::SEMIBOLD))
                        .child(if self.workspace.is_some() {
                            badge("Checked out".into()).child(div().size(px(6.)).rounded_full().bg(c(theme::OK)))
                        } else {
                            badge("Not checked out".into())
                        }),
                )
                .child(probe(format!("ft-repo:{ix}"))),
            "Open in Code Review",
        )
    }

    fn render_pr(&self, ix: usize, pr: &PullRequest, cx: &mut Context<Self>) -> impl IntoElement {
        let k = key(pr);
        let open = self.expanded.as_deref() == Some(k.as_str());
        let check = self.check_of(pr);
        let total = check.map_or(0, |c| c.checks.len());
        let passed = check.map_or(0, |c| c.checks.iter().filter(|c| c.bucket == "pass").count());
        let status = if self.checks.is_none() { "running".to_string() } else { check.map_or("none".into(), |c| c.status.clone()) };
        let checks_label = if self.checks.is_none() {
            "Loading checks…".to_string()
        } else if total > 0 {
            format!("{passed}/{total} checks")
        } else {
            "No checks".into()
        };
        let draft = pr.is_draft;
        let merged = self.merged.contains(&k);
        let block = self.merge_block(pr);
        let decision = pr.status.as_ref().and_then(|s| s.review_decision.as_deref()).and_then(pr_ui::decision);
        let dot = match status.as_str() {
            "pass" => div().size(px(9.)).rounded_full().bg(c(theme::OK)).into_any_element(),
            "fail" => div().size(px(9.)).rounded_full().bg(c(theme::DANGER)).into_any_element(),
            "running" => kit::spinner(("ft-pr-spin", ix), 10., c(theme::ACCENT)).into_any_element(),
            _ => div().size(px(9.)).rounded_full().border(px(1.5)).border_color(c(theme::TEXT_FAINT)).into_any_element(),
        };
        let (k2, url, one) = (k.clone(), pr.url.clone(), vec![pr.clone()]);
        let base = pr.base.clone();
        div()
            .flex()
            .flex_col()
            .when(ix > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
            .child(
                div()
                    .id(SharedString::from(format!("ft-pr:{ix}")))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(12.))
                    .cursor_pointer()
                    .when(open, |d| d.bg(c(theme::PANEL_RAISED)))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.expanded = if p.expanded.as_deref() == Some(k2.as_str()) { None } else { Some(k2.clone()) };
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(30.))
                            .flex_none()
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if draft { c(theme::PANEL_RAISED) } else { tint(theme::OK, 0.12) })
                            .child(lucide("pr", 13., c(if draft { theme::TEXT_FAINT } else { theme::OK }))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(5.))
                                    .min_w_0()
                                    .text_size(px(13.5))
                                    .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(pr.title.clone()))
                                    .child(div().flex_none().text_color(c(theme::TEXT_FAINT)).child(format!("#{}", pr.number))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(11.5))
                                    .text_color(c(theme::TEXT_FAINT))
                                    .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT_DIM)).child(pr.repo.clone()))
                                    .child(
                                        div()
                                            .px(px(7.))
                                            .rounded_full()
                                            .border_1()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .when(draft, |d| d.border_color(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT_DIM)).child("Draft"))
                                            .when(!draft, |d| d.border_color(tint(theme::OK, 0.35)).text_color(c(theme::OK)).child("Open")),
                                    )
                                    .child(pr.author.clone()),
                            ),
                    )
                    .when_some(decision, |d, (label, tone)| d.child(pr_ui::status_pill(label, tone)))
                    .child(div().flex().items_center().gap(px(7.)).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(dot).child(checks_label))
                    .when(!draft && !merged, |d| {
                        let hint = block.clone().unwrap_or_else(|| format!("Squash into {base} and delete the branch on GitHub"));
                        let b = pr_ui::btn_mini(format!("ft-merge:{ix}"), "Squash and merge").bg(c(theme::PANEL_RAISED));
                        let b = if block.is_some() { b.child(lucide("lock", 11., c(theme::TEXT_FAINT))).gap(px(5.)) } else { b };
                        let b = if block.is_some() {
                            b.opacity(0.5).cursor_default()
                        } else {
                            b.on_click(cx.listener(move |p, _, window, cx| {
                                cx.stop_propagation();
                                p.merge(one.clone(), window, cx);
                            }))
                        };
                        d.child(pr_ui::tip(b, hint))
                    })
                    .when(merged, |d| d.child(pr_ui::status_pill("merged", Tone::Muted).text_color(c(theme::PURPLE))))
                    .child(pr_ui::tip(
                        pr_ui::icon_button(format!("ft-github:{ix}"), "github", 28., 14.).on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            cx.open_url(&url);
                        }),
                        "Open on GitHub ↗",
                    ))
                    .child(lucide(if open { "chevron-down" } else { "chevron-right" }, 13., c(theme::TEXT_FAINT)))
                    .child(probe(format!("ft-pr:{ix}"))),
            )
            .when(open, |d| d.child(self.render_checks(check, cx)))
    }

    fn render_stack(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.stack.len();
        let here = self.prs.first().map(|p| p.branch.clone()).unwrap_or_default();
        let rows = self.stack.iter().enumerate().rev().map(|(level, g)| {
            let first = &g.prs[0];
            let (top, bottom, current) = (level + 1 == n, level == 0, g.branch == here);
            let merged = g.prs.iter().all(|p| self.merged.contains(&key(p)));
            let below = g.prs.iter().find_map(|p| self.below(p)).map(|b| b.number);
            let stale = g.prs.iter().find(|p| self.behind.get(&key(p)).is_some_and(|&b| b > 0)).cloned();
            let (label, tone) = match (merged, below) {
                (true, _) => ("Merged".to_string(), Tone::Muted),
                (_, None) => ("Next to merge".into(), Tone::Ok),
                (_, Some(b)) if stale.is_some() => (format!("Behind #{b}"), Tone::Warn),
                (_, Some(b)) => (format!("After #{b}"), Tone::Muted),
            };
            let node = match (merged, below) {
                (true, _) => theme::PURPLE,
                (_, None) => theme::OK,
                _ => theme::TEXT_FAINT,
            };
            let prs = g.prs.clone();
            div()
                .id(SharedString::from(format!("ft-stack:{level}")))
                .relative()
                .flex()
                .gap(px(10.))
                .px(px(14.))
                .when(!bottom, |d| d.border_b_1().border_color(c(theme::BORDER)))
                .when(current, |d| d.bg(tint(theme::ACCENT, 0.07)))
                .when(!current, |d| {
                    d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED))).on_click(cx.listener(move |p, _, window, cx| {
                        let (host, prs) = (p.host.clone(), prs.clone());
                        with_app(&p.host, window, cx, move |a, w, cx| open(a, host, prs, w, cx));
                    }))
                })
                .child(probe(format!("ft-stack:{level}")))
                .child(pr_ui::rail(!top, !bottom, c(node), below.is_none() || merged))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .py(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .min_w_0()
                                .text_size(px(13.))
                                .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(first.title.clone()))
                                .child(div().flex_none().text_color(c(theme::TEXT_FAINT)).child(g.prs.iter().map(|p| format!("#{}", p.number)).collect::<Vec<_>>().join(" ")))
                                .when(current, |d| d.child(div().flex_none().text_size(px(11.)).text_color(c(theme::ACCENT)).child("· this feature"))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(5.))
                                .font_family(theme::MONO_FONT)
                                .text_size(px(11.))
                                .text_color(c(theme::TEXT_FAINT))
                                .child(div().text_color(c(theme::TEXT_DIM)).child(g.branch.clone()))
                                .child("→")
                                .child(first.base.clone()),
                        ),
                )
                .child(div().flex().items_center().gap(px(8.)).child(pr_ui::status_pill(label, tone)).when_some(stale.filter(|_| !merged), |d, p| {
                    d.child(pr_ui::tip(
                        pr_ui::btn_mini(format!("ft-stack-update:{level}"), "Update").on_click(cx.listener(move |page, _, window, cx| {
                            cx.stop_propagation();
                            page.update_branch(p.clone(), window, cx);
                        })),
                        "Merge the PR below into this one on GitHub (no force-push)",
                    ))
                }))
        });
        let block = self.stack_block();
        let button = match (&self.stacking, block) {
            (Some(s), _) => div().flex().items_center().gap(px(6.)).text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(kit::spinner("ft-stacking", 12., c(theme::ACCENT))).child(s.clone()).into_any_element(),
            (None, Some(why)) => pr_ui::tip(pr_ui::btn_mini("ft-merge-stack", "Merge stack").opacity(0.5).cursor_default(), why).into_any_element(),
            (None, None) => pr_ui::tip(
                pr_ui::btn_mini("ft-merge-stack", "Merge stack").on_click(cx.listener(|p, _, window, cx| p.merge_stack(window, cx))),
                "Merge every level bottom-up, waiting for each one's checks",
            )
            .into_any_element(),
        };
        div()
            .mb(px(22.))
            .child(
                pr_ui::section_head("Stack")
                    .child(pr_ui::section_count(n))
                    .child(div().flex_1().font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_FAINT)).child("merges from the bottom up"))
                    .child(button),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .rounded(px(theme::size::RADIUS))
                    .bg(c(theme::PANEL))
                    .overflow_hidden()
                    .children(rows.map(|r| r.into_any_element()).collect::<Vec<_>>()),
            )
    }

    fn render_checks(&self, check: Option<&WsCheck>, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().flex().flex_col().gap(px(4.)).pt(px(4.)).pb(px(12.)).pl(px(56.)).pr(px(14.)).bg(c(theme::PANEL_RAISED));
        let Some(check) = check.filter(|c| !c.checks.is_empty()) else {
            return list.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("No CI checks on this PR."));
        };
        list.children(check.checks.iter().enumerate().map(|(i, ch)| {
            let failed = ch.bucket == "fail";
            let icon = match ch.bucket.as_str() {
                "pending" | "queued" => kit::spinner(SharedString::from(format!("ft-check-spin:{}:{i}", check.repo)), 12., c(theme::ACCENT)).into_any_element(),
                "fail" => lucide("x", 11., c(theme::DANGER)).into_any_element(),
                "pass" => lucide("check", 11., c(theme::OK)).into_any_element(),
                _ => lucide("ring", 10., c(theme::TEXT_FAINT)).into_any_element(),
            };
            let (link, rerun) = (ch.link.clone(), ch.link.clone());
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .text_size(px(12.))
                .when(failed, |d| d.bg(tint(theme::DANGER, 0.07)))
                .child(div().w(px(14.)).flex().justify_center().child(icon))
                .child(
                    div()
                        .id(SharedString::from(format!("ft-check:{i}")))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO_FONT)
                        .when(!link.is_empty(), |d| d.cursor_pointer().hover(|s| s.text_color(c(theme::ACCENT))))
                        .on_click(move |_, _, cx| {
                            if !link.is_empty() {
                                cx.open_url(&link)
                            }
                        })
                        .child(ch.name.clone()),
                )
                .when(failed, |d| {
                    d.child(pr_ui::btn_mini(format!("ft-rerun:{i}"), "Re-run").on_click(cx.listener(move |p, _, window, cx| {
                        let (link, host) = (rerun.clone(), p.host.clone());
                        cx.spawn_in(window, async move |this, cx| {
                            let r = cx.background_spawn(async move { orbit_core::prs::rerun(&link) }).await;
                            let _ = this.update_in(cx, |p, window, cx| match r {
                                Ok(()) => {
                                    host.toast(ToastKind::Info, "Re-running failed jobs", None, cx);
                                    p.load_checks(window, cx);
                                }
                                Err(e) => host.error("Couldn't re-run", e, cx),
                            });
                        })
                        .detach();
                    })))
                })
        }))
    }
}

impl Render for FeaturePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(first) = self.prs.first().cloned() else { return div().id("feature").into_any_element() };
        let multi = self.prs.len() > 1;
        let steps = self.steps();
        let next = steps.iter().position(|s| s.state == StepState::Fail);
        let n = steps.len();
        let sep = |t: &str| div().text_color(c(theme::TEXT_FAINT)).child(t.to_string());
        let muted = |t: String| div().text_color(c(theme::TEXT_DIM)).child(t);
        let refreshing = self.refreshing;
        let workspace = self.workspace.clone();
        let mut flow = div().flex().gap(px(10.)).mb(px(22.));
        for (i, s) in steps.into_iter().enumerate() {
            flow = flow.child(self.render_step(i, s, next == Some(i), i + 1 == n, cx));
        }
        let repos = div()
            .flex()
            .flex_col()
            .mt(px(4.))
            .border_1()
            .border_color(c(theme::BORDER))
            .rounded(px(theme::size::RADIUS))
            .bg(c(theme::PANEL))
            .overflow_hidden()
            .children(self.prs.iter().enumerate().map(|(i, p)| self.render_repo(i, p, cx).into_any_element()).collect::<Vec<_>>());
        let prs = div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(c(theme::BORDER))
            .rounded(px(theme::size::RADIUS))
            .bg(c(theme::PANEL))
            .overflow_hidden()
            .children(self.prs.iter().enumerate().map(|(i, p)| self.render_pr(i, p, cx).into_any_element()).collect::<Vec<_>>());
        div()
            .id("feature")
            .size_full()
            .overflow_y_scroll()
            .px(px(36.))
            .py(px(24.))
            .font_family(theme::UI_FONT)
            .text_color(c(theme::TEXT))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(20.))
                    .mb(px(20.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().text_size(px(20.25)).font_weight(FontWeight::BOLD).child(if multi { first.branch.clone() } else { first.title.clone() }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .px(px(9.))
                                            .py(px(2.))
                                            .rounded(px(6.))
                                            .bg(c(theme::PANEL_RAISED))
                                            .font_family(theme::MONO_FONT)
                                            .text_size(px(12.))
                                            .child(lucide("git", 12., c(theme::TEXT)))
                                            .child(first.branch.clone()),
                                    )
                                    .child(sep("into"))
                                    .child(muted(first.base.clone()))
                                    .child(sep("·"))
                                    .child(muted(format!("{} repo{}", self.prs.len(), if multi { "s" } else { "" })))
                                    .child(sep("·"))
                                    .child(muted(first.author.clone())),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(6.))
                            .child(pr_ui::tip(
                                pr_ui::button("ft-code-review", "Code Review", false, 32.)
                                    .child(lucide("diff", 14., c(theme::TEXT)))
                                    .flex_row_reverse()
                                    .on_click(cx.listener(|p, _, window, cx| p.open_review(window, cx))),
                                "Review the PRs: diffs, threads and verdict, one tab",
                            ))
                            .child(match workspace {
                                Some(ws) => {
                                    let name = ws.meta.name.clone();
                                    pr_ui::tip(
                                        pr_ui::button("ft-open-ws", "Open workspace", true, 32.).on_click(cx.listener(move |p, _, window, cx| {
                                            let name = name.clone();
                                            with_app(&p.host, window, cx, move |a, w, cx| {
                                                crate::workspace_ui::open(a, crate::workspace_ui::PageId::Workspace(name), w, cx);
                                            });
                                        })),
                                        format!("Checked out in workspace {}", ws.meta.name),
                                    )
                                }
                                None => pr_ui::tip(
                                    pr_ui::button("ft-checkout", "Check out", true, 32.)
                                        .on_click(cx.listener(|p, _, window, cx| match p.stack.len() {
                                            1 => code_review::checkout(&p.host, p.prs.clone(), window, cx),
                                            _ => code_review::checkout_stack(&p.host, &p.stack, window, cx),
                                        })),
                                    if self.stack.len() > 1 { "Create one workspace for the whole stack, on its top branch" } else { "Create a local workspace on this branch to build, run and fix it" },
                                ),
                            })
                            .child(div().w(px(1.)).h(px(22.)).mx(px(4.)).bg(c(theme::BORDER)))
                            .child(pr_ui::tip(
                                pr_ui::icon_button_busy("ft-refresh", "refresh-cw", 32., 15., refreshing, false)
                                    .on_click(cx.listener(|p, _, window, cx| p.load_checks(window, cx))),
                                "Refresh checks",
                            )),
                    ),
            )
            .when(self.workspace.is_none(), |d| {
                d.child(
                    div()
                        .mb(px(14.))
                        .px(px(12.))
                        .py(px(8.))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(8.))
                        .bg(c(theme::BG))
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .text_size(px(12.5))
                        .text_color(c(theme::TEXT_DIM))
                        .child("Not on this machine yet.")
                        .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Check out"))
                        .child("to get the full workspace: build, agents, commits and AI fixes for failing checks."),
                )
            })
            .child(flow)
            .when(self.stack.len() > 1, |d| d.child(self.render_stack(cx)))
            .child(div().mb(px(22.)).child(pr_ui::section_head("Repositories")).child(repos))
            .child(div().mb(px(22.)).child(pr_ui::section_head("Pull requests").child(pr_ui::section_count(self.prs.len()))).child(prs))
            .map(|page| crate::motion::page_in(page, "feature-in"))
    }
}

impl PageView for FeaturePage {
    fn title(&self) -> SharedString {
        match self.prs.as_slice() {
            [p] => format!("#{} {}", p.number, p.repo).into(),
            [p, ..] => p.branch.clone().into(),
            [] => "Feature".into(),
        }
    }

    fn dump(&self, out: &mut String, _: &App) {
        let checks = match &self.checks {
            None => "loading".to_string(),
            Some(c) => c.iter().map(|c| format!("{}#{} {}", c.repo, c.pr_number, c.status)).collect::<Vec<_>>().join(", "),
        };
        let _ = writeln!(out, "  feature {} ({} prs) checks: {checks}", self.prs.first().map_or("", |p| p.branch.as_str()), self.prs.len());
        if self.stack.len() > 1 {
            let _ = writeln!(out, "    stack: {}", self.stack.iter().map(|g| g.branch.as_str()).collect::<Vec<_>>().join(" → "));
        }
        for (repo, d) in &self.details {
            match d {
                Some(d) => {
                    let _ = writeln!(out, "    {repo}: +{} -{} {} files", d.additions, d.deletions, d.files.len());
                }
                None => {
                    let _ = writeln!(out, "    {repo}: no detail");
                }
            }
        }
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_checks(window, cx);
    }
}

/// Opens (or shows) the feature page of a group of PRs.
pub fn open(app: &mut OrbitApp, _host: Host, prs: Vec<PullRequest>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(first) = prs.first() else { return };
    let key = format!("page:feature:{}:{}#{}", first.branch, first.owner_repo, first.number);
    crate::pages::open(app, key, move |h, w, cx| FeaturePage::new(h, prs, w, cx), window, cx);
}
