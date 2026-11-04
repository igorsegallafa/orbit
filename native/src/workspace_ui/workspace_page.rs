//! A workspace's page (`WorkspaceDetailPage.tsx`): the delivery flow
//! (changes → push → pull requests → checks, each with the action that moves
//! it forward), the branch drift banner, the repos with their badges (dirty,
//! ahead/behind with Pull, merged, off branch) and the PRs with their checks.
use super::{plural, spawn, Page, PageId, PageTab};
use crate::git::drift::Drift;
use crate::git::merge::MergeItem;
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::review::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, ClickEvent, Context, Rgba, SharedString, Window};
use orbit_core::github::WsPrStatus;
use orbit_core::prs::WsCheck;
use orbit_core::workspace::{self, RepoStatus, Workspace};
use std::fmt::Write as _;
use std::rc::Rc;

pub struct WsPage {
    pub name: String,
    pub meta: Option<Workspace>,
    pub statuses: Load<Vec<RepoStatus>>,
    pub prs: Load<Vec<WsPrStatus>>,
    pub checks: Load<Vec<WsCheck>>,
    pub refreshing: bool,
    pub pulling: Option<String>,
    pub drift: Drift,
    pub error: Option<String>,
}

impl WsPage {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            meta: None,
            statuses: Load::Loading,
            prs: Load::Idle,
            checks: Load::Idle,
            refreshing: false,
            pulling: None,
            drift: Drift::default(),
            error: None,
        }
    }

    /// What the page derives from the statuses and PRs (as the React page does).
    pub fn derived(&self) -> Derived {
        let statuses = self.statuses.ready().cloned().unwrap_or_default();
        let prs = self.prs.ready().cloned().unwrap_or_default();
        let repos = self.meta.as_ref().map(|m| m.repos.clone()).unwrap_or_default();
        // Repos whose PRs were all closed unmerged: Push leaves them out, or
        // it would recreate a branch GitHub deleted.
        let dropped: Vec<(String, u64)> = repos
            .iter()
            .filter_map(|r| {
                let mine: Vec<&WsPrStatus> = prs.iter().filter(|p| &p.repo == r).collect();
                (!mine.is_empty() && mine.iter().all(|p| p.state == "CLOSED")).then(|| (r.clone(), mine[0].number))
            })
            .collect();
        // Squash-merged repos keep local commits no remote has: no push needed.
        let push_repos: Vec<&RepoStatus> = statuses.iter().filter(|s| !s.integrated && !dropped.iter().any(|(r, _)| r == &s.repo)).collect();
        let ahead: usize = push_repos.iter().map(|s| s.ahead).sum();
        let pushed_content = statuses.iter().any(|s| s.pr_commits > 0);
        let open: Vec<&WsPrStatus> = prs.iter().filter(|p| p.state == "OPEN").collect();
        let merged = prs.iter().filter(|p| p.state == "MERGED").count();
        let checks: Vec<&str> = self.checks.ready().into_iter().flatten().map(|c| c.status.as_str()).collect();
        let failing = checks.contains(&"fail");
        let running = checks.contains(&"running");
        let mergeable: Vec<MergeItem> =
            open.iter().filter(|p| !p.is_draft).map(|p| MergeItem { repo: p.repo.clone(), number: p.number, title: p.title.clone() }).collect();
        Derived {
            dirty: statuses.iter().filter(|s| s.dirty).map(|s| s.repo.clone()).collect(),
            push: push_repos.iter().filter(|s| s.ahead > 0).map(|s| s.repo.clone()).collect(),
            ahead,
            pushed_content,
            landed: merged > 0 || statuses.iter().any(|s| s.integrated),
            can_create_prs: matches!(&self.prs, Load::Ready(p) if p.is_empty()) && (ahead > 0 || pushed_content),
            open: open.len(),
            merged,
            failing,
            running,
            passing: !checks.is_empty() && checks.iter().all(|s| *s == "pass"),
            can_merge: !mergeable.is_empty() && !failing && !running,
            mergeable,
            dropped,
        }
    }

    pub fn dump(&self, out: &mut String) {
        if let Some(m) = &self.meta {
            let _ = writeln!(out, "  branch {} from {} · repos {:?}", m.branch, m.base, m.repos);
        }
        if let Some(e) = &self.error {
            let _ = writeln!(out, "  error: {e}");
        }
        let _ = writeln!(out, "  statuses: {}", super::load_text(&self.statuses));
        for s in self.statuses.ready().into_iter().flatten() {
            let _ = writeln!(
                out,
                "    {} on {} dirty {} ahead {} behind {} pr_commits {}{}{}{}",
                s.repo,
                s.branch.as_deref().unwrap_or("-"),
                s.dirty,
                s.ahead,
                s.behind,
                s.pr_commits,
                if s.integrated { " integrated" } else { "" },
                if s.off_branch { format!(" OFF-BRANCH (expected {})", s.expected_branch) } else { String::new() },
                if s.branch_only { " linked" } else { "" },
            );
        }
        let _ = writeln!(out, "  prs: {}", super::load_text(&self.prs));
        for p in self.prs.ready().into_iter().flatten() {
            let _ = writeln!(out, "    {} #{} {} {}", p.repo, p.number, p.state, p.title);
        }
        let _ = writeln!(out, "  checks: {}", super::load_text(&self.checks));
        let d = self.derived();
        let _ = writeln!(
            out,
            "  flow: dirty {:?} push {:?} ahead {} can_create_prs {} open {} merged {} can_merge {}",
            d.dirty, d.push, d.ahead, d.can_create_prs, d.open, d.merged, d.can_merge
        );
        if !self.drift.dirty.is_empty() || self.drift.busy {
            let _ = writeln!(out, "  drift: dirty {:?} busy {}", self.drift.dirty, self.drift.busy);
        }
    }
}

pub struct Derived {
    pub dirty: Vec<String>,
    pub push: Vec<String>,
    pub ahead: usize,
    pub pushed_content: bool,
    pub landed: bool,
    pub can_create_prs: bool,
    pub open: usize,
    pub merged: usize,
    pub failing: bool,
    pub running: bool,
    pub passing: bool,
    pub can_merge: bool,
    pub mergeable: Vec<MergeItem>,
    pub dropped: Vec<(String, u64)>,
}

impl PageTab {
    fn ws_page(&mut self) -> Option<&mut WsPage> {
        match &mut self.page {
            Page::Workspace(p) => Some(p),
            _ => None,
        }
    }

    /// Local status first; then (or with `fetch`) the remotes, PRs and checks.
    pub fn load_workspace(&mut self, fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        if p.refreshing {
            return;
        }
        p.refreshing = true;
        let first = matches!(p.prs, Load::Idle);
        let name = p.name.clone();
        spawn(
            window,
            cx,
            move || {
                let meta = workspace::ws_dir(&name).and_then(|d| workspace::load_meta(&d));
                let repos = meta.as_ref().map(|m| m.repos.clone()).unwrap_or_default();
                let remote = fetch.then(|| {
                    std::thread::scope(|s| {
                        for r in &repos {
                            s.spawn(move || orbit_core::repo::fetch(r));
                        }
                        let prs = s.spawn(|| orbit_core::prs::status(&name));
                        let checks = s.spawn(|| orbit_core::prs::checks(&name));
                        let failed = || "thread panicked".to_string();
                        (prs.join().unwrap_or_else(|_| Err(failed())), checks.join().unwrap_or_else(|_| Err(failed())))
                    })
                });
                (meta, workspace::status(&name), remote)
            },
            move |t, (meta, status, remote), window, cx| {
                let Some(p) = t.ws_page() else { return };
                p.refreshing = false;
                match meta {
                    Ok(m) => {
                        p.meta = Some(m);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
                p.statuses = status.map_or_else(Load::Failed, Load::Ready);
                if let Some((prs, checks)) = remote {
                    p.prs = prs.map_or_else(Load::Failed, Load::Ready);
                    p.checks = checks.map_or_else(Load::Failed, Load::Ready);
                }
                t.auto_sync_drift(window, cx);
                if let Some(f) = t.pending.take() {
                    f(t, window, cx);
                }
                if first && !fetch {
                    if let Some(p) = t.ws_page() {
                        p.prs = Load::Loading;
                        p.checks = Load::Loading;
                    }
                    t.load_workspace(true, window, cx);
                }
            },
        );
        cx.notify();
    }

    fn ws_name(&mut self) -> Option<String> {
        self.ws_page().map(|p| p.name.clone())
    }

    pub fn start_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let repos = match &p.statuses {
            Load::Ready(_) => p.derived().dirty,
            _ => p.meta.as_ref().map(|m| m.repos.clone()).unwrap_or_default(),
        };
        let name = p.name.clone();
        if repos.is_empty() {
            return self.toast(ToastKind::Info, "Nothing to commit", None, window, cx);
        }
        self.open_commit(name, repos, window, cx);
    }

    pub fn start_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let d = p.derived();
        let repos = match &p.statuses {
            Load::Ready(_) => d.push,
            _ => p.meta.as_ref().map(|m| m.repos.clone()).unwrap_or_default(),
        };
        let skipped = d.dropped.iter().map(|(r, n)| (r.clone(), format!("PR #{n} was closed without merging"))).collect();
        let name = p.name.clone();
        self.open_push(name, repos, skipped, window, cx);
    }

    pub fn start_rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let Some(m) = p.meta.clone() else {
            return self.toast(ToastKind::Info, "Still reading the workspace", None, window, cx);
        };
        self.open_rebase(m.name, m.base, m.repos, cx);
    }

    pub fn start_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        if !matches!(p.prs, Load::Ready(_)) {
            return self.toast(ToastKind::Info, "Still checking the pull requests", None, window, cx);
        }
        let d = p.derived();
        let (name, base) = (p.name.clone(), p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_else(|| "main".into()));
        if d.mergeable.is_empty() {
            return self.toast(ToastKind::Info, "No open pull request ready to merge", None, window, cx);
        }
        self.open_merge(name, base, d.mergeable, cx);
    }

    fn ws_pull(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        p.pulling = Some(repo.clone());
        let name = p.name.clone();
        let r = repo.clone();
        spawn(window, cx, move || orbit_core::prs::worktree_on_branch(&name, &r).and_then(|d| orbit_core::git::pull_ff(&d)), move |t, res, window, cx| {
            if let Some(p) = t.ws_page() {
                p.pulling = None;
            }
            match res {
                Ok(()) => t.toast(ToastKind::Success, format!("Pulled {repo}"), None, window, cx),
                Err(e) => t.toast(ToastKind::Error, format!("Pull failed in {repo}"), Some(e), window, cx),
            }
            t.reload(false, window, cx);
        });
        cx.notify();
    }

    /// Asks, then removes the workspace (worktrees and local branches without
    /// unpushed work). Uncommitted changes ask again before forcing.
    pub fn confirm_remove(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        let page = cx.entity().downgrade();
        let ws = name.clone();
        let run = Rc::new(move |_: &mut crate::app::OrbitApp, window: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
            let ws = ws.clone();
            let _ = page.update(cx, |_, cx| {
                spawn(window, cx, move || workspace::remove(&ws, force).map(|kept| (ws, kept)), move |t, res, window, cx| match res {
                    Ok((ws, kept)) => {
                        let key = t.key();
                        if kept.is_empty() {
                            t.toast(ToastKind::Success, format!("Workspace {ws} removed"), None, window, cx);
                        } else {
                            t.toast(ToastKind::Info, format!("Workspace {ws} removed; branches with unpushed work kept"), Some(kept.join("\n")), window, cx);
                        }
                        t.with_app(window, cx, move |a, w, cx| {
                            super::reload_workspaces(a, None, cx);
                            if let Some(ix) = a.tab_index(&key, cx) {
                                a.close_tab(ix, true, w, cx);
                            }
                        });
                    }
                    Err(e) if !force && e.contains("uncommitted changes") => t.confirm_remove(true, window, cx),
                    Err(e) => t.toast(ToastKind::Error, "Couldn't remove the workspace", Some(e), window, cx),
                });
            });
        });
        let (title, message, label) = if force {
            ("Remove anyway?", format!("{name} has uncommitted changes. Removing it throws them away."), "Remove anyway")
        } else {
            ("Remove workspace", format!("Remove {name}? Its worktrees and local branches go; branches with unpushed work are kept."), "Remove")
        };
        self.with_app(window, cx, move |a, w, cx| {
            a.open_dialog(
                Dialog {
                    title: title.into(),
                    message: message.into(),
                    buttons: vec![
                        DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) },
                        DialogButton { label: label.into(), style: ButtonStyle::Primary, run },
                    ],
                    on_cancel: Rc::new(|_, _, _| {}),
                },
                w,
                cx,
            )
        });
    }

    fn open_review(&mut self, page: crate::review::Page, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        self.with_app(window, cx, move |a, w, cx| {
            if let Some(ix) = a.workspaces.iter().position(|ws| ws.meta.name == name) {
                if a.selected != Some(ix) {
                    a.select_workspace(ix, cx);
                }
                crate::review::open(a, page, w, cx);
            }
        });
    }

    pub(super) fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Page::Workspace(p) = &self.page else { return div() };
        let d = p.derived();
        let head = header(p, cx);
        let statuses = p.statuses.ready().cloned().unwrap_or_default();
        let drift = self.render_drift(&statuses, &p.drift, cx);
        let done = p.statuses.ready().is_some() && d.merged > 0 && d.open == 0 && d.dirty.is_empty() && d.ahead == 0;
        let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
        let body = div()
            .id("ws-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb_6()
            .child(head)
            .children(drift)
            .when_some(p.error.clone(), |d, e| d.child(div().mx_4().mt_2().text_xs().text_color(c(theme::DANGER)).child(e)))
            .when(done, |dd| {
                dd.child(
                    div()
                        .mx_4()
                        .mt_3()
                        .p_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(theme::OK))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(div().flex_1().child(format!(
                            "Merged into {base}. This feature is done; removing the workspace deletes its worktrees and local branches."
                        )))
                        .child(ui::button("ws-done-remove", "Remove workspace", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.confirm_remove(false, w, cx)))),
                )
            })
            .child(flow_steps(p, &d, cx))
            .child(ui::section("REPOSITORIES"))
            .child(repo_rows(p, &d, cx))
            .child(ui::section(format!("PULL REQUESTS{}", p.prs.ready().map(|l| format!(" · {}", l.len())).unwrap_or_default())))
            .child(pr_rows(p, &d));
        div().size_full().flex().flex_col().child(body)
    }
}

fn header(p: &WsPage, cx: &mut Context<PageTab>) -> impl IntoElement {
    let meta = p.meta.as_ref();
    let sub = meta.map(|m| format!("{} from {} · {}", m.branch, m.base, plural(m.repos.len(), "repo"))).unwrap_or_default();
    let card = meta.and_then(|m| m.card.clone());
    let tool = |id: &'static str, label: &'static str, f: fn(&mut PageTab, &mut Window, &mut Context<PageTab>), cx: &mut Context<PageTab>| {
        ui::button(id, label, Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx)))
    };
    div()
        .px_4()
        .pt_4()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_xl().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(p.name.clone()))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .text_xs()
                        .text_color(c(theme::TEXT_DIM))
                        .child(sub)
                        .when_some(card, |d, card| d.child(div().text_color(c(theme::ACCENT)).child(format!("{} · {}", card.id, card.title)))),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .justify_end()
                .child(tool("ws-commit", "Commit", |t, w, cx| t.start_commit(w, cx), cx))
                .child(tool("ws-push", "Push", |t, w, cx| t.start_push(w, cx), cx))
                .child(tool("ws-rebase", "Rebase", |t, w, cx| t.start_rebase(w, cx), cx))
                .child(tool("ws-merge", "Merge", |t, w, cx| t.start_merge(w, cx), cx))
                .child(tool("ws-changes", "Changes", |t, w, cx| t.open_review(crate::review::Page::Changes, w, cx), cx))
                .child(tool("ws-prs", "PRs", |t, w, cx| t.open_review(crate::review::Page::PullRequests, w, cx), cx))
                .child(tool("ws-add-repos", "+ Repos", |t, w, cx| t.open_add_repos(w, cx), cx))
                .child(tool("ws-refresh", if p.refreshing { "Refreshing…" } else { "Refresh" }, |t, w, cx| t.reload(true, w, cx), cx))
                .child(ui::button("ws-remove", "Remove", Tone::Danger).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.confirm_remove(false, w, cx)))),
        )
}

#[derive(Clone, Copy, PartialEq)]
enum StepState {
    Loading,
    Action,
    Done,
    Idle,
    Fail,
    Running,
}

type Act = fn(&mut PageTab, &mut Window, &mut Context<PageTab>);

struct Step {
    id: &'static str,
    title: &'static str,
    state: StepState,
    detail: String,
    action: Option<(&'static str, Act)>,
}

fn flow_steps(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> impl IntoElement {
    let loading = p.statuses.ready().is_none();
    let prs_loading = p.prs.ready().is_none();
    let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
    let n_prs = p.prs.ready().map_or(0, Vec::len);
    let steps = [
        Step {
            id: "flow-changes",
            title: "Changes",
            state: if loading { StepState::Loading } else if d.dirty.is_empty() { StepState::Done } else { StepState::Action },
            detail: if loading {
                "Checking…".into()
            } else if d.dirty.is_empty() {
                "Working tree clean".into()
            } else {
                format!("{} with uncommitted changes", plural(d.dirty.len(), "repo"))
            },
            action: (!d.dirty.is_empty()).then_some(("Commit", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_commit(w, cx)) as Act)),
        },
        Step {
            id: "flow-push",
            title: "Push",
            state: if loading || prs_loading {
                StepState::Loading
            } else if d.ahead > 0 {
                StepState::Action
            } else if d.pushed_content || d.landed {
                StepState::Done
            } else {
                StepState::Idle
            },
            detail: if loading || prs_loading {
                "Checking…".into()
            } else if d.ahead > 0 {
                format!("{} to push", plural(d.ahead, "commit"))
            } else if d.pushed_content {
                "Everything is on origin".into()
            } else if d.landed {
                format!("Merged into {base}")
            } else {
                "No commits yet".into()
            },
            action: (d.ahead > 0).then_some(("Push", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_push(w, cx)) as Act)),
        },
        Step {
            id: "flow-prs",
            title: "Pull requests",
            state: if prs_loading {
                StepState::Loading
            } else if d.can_create_prs {
                StepState::Action
            } else if d.open > 0 || d.merged > 0 {
                StepState::Done
            } else {
                StepState::Idle
            },
            detail: if prs_loading {
                match &p.prs {
                    Load::Failed(e) => e.clone(),
                    _ => "Checking GitHub…".into(),
                }
            } else if n_prs > 0 {
                let parts: Vec<String> =
                    [(d.open, "open"), (d.merged, "merged")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect();
                if parts.is_empty() { format!("{n_prs} closed") } else { parts.join(" · ") }
            } else if d.can_create_prs {
                "Ready to open".into()
            } else {
                "Push your work first".into()
            },
            action: if d.can_create_prs {
                Some(("Create", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.open_review(crate::review::Page::PullRequests, w, cx)) as Act))
            } else if d.can_merge {
                Some(("Squash and merge", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_merge(w, cx)) as Act))
            } else if n_prs > 0 {
                Some(("View", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.open_review(crate::review::Page::PullRequests, w, cx)) as Act))
            } else {
                None
            },
        },
        Step {
            id: "flow-checks",
            title: "Checks",
            state: match &p.checks {
                Load::Ready(_) if d.failing => StepState::Fail,
                Load::Ready(_) if d.running => StepState::Running,
                Load::Ready(_) if d.passing => StepState::Done,
                Load::Ready(_) => StepState::Idle,
                _ => StepState::Loading,
            },
            detail: match &p.checks {
                Load::Ready(_) if d.failing => "Some checks failed".into(),
                Load::Ready(_) if d.running => "Running…".into(),
                Load::Ready(_) if d.passing => "All checks passed".into(),
                Load::Ready(_) => "No checks yet".into(),
                Load::Failed(e) => e.clone(),
                _ => "Checking CI…".into(),
            },
            action: d.failing.then_some(("Review", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.open_review(crate::review::Page::PullRequests, w, cx)) as Act)),
        },
    ];
    // Only the first step that needs something is highlighted: the next move.
    let next = steps.iter().position(|s| matches!(s.state, StepState::Action | StepState::Fail));
    div().px_4().pt_3().flex().gap_2().children(steps.into_iter().enumerate().map(|(i, s)| {
        let (mark, color): (&str, Rgba) = match s.state {
            StepState::Done => ("✓", c(theme::OK)),
            StepState::Fail => ("✕", c(theme::DANGER)),
            StepState::Loading | StepState::Running => ("◌", c(theme::TEXT_FAINT)),
            StepState::Action => ("●", c(theme::ACCENT)),
            StepState::Idle => ("○", c(theme::TEXT_FAINT)),
        };
        let primary = Some(i) == next;
        div()
            .flex_1()
            .min_w_0()
            .p_2()
            .rounded(px(8.))
            .border_1()
            .border_color(if primary { c(theme::ACCENT) } else { c(theme::BORDER) })
            .flex()
            .flex_col()
            .gap_1()
            .child(div().flex().gap_2().text_sm().child(div().text_color(color).child(mark)).child(div().text_color(c(theme::TEXT)).child(s.title)))
            .child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(s.detail))
            .when_some(s.action, |dd, (label, f)| {
                dd.child(
                    div().flex().child(
                        ui::button(s.id, label, if primary { Tone::Primary } else { Tone::Plain })
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx))),
                    ),
                )
            })
    }))
}

fn badge(label: impl Into<SharedString>, color: Rgba) -> gpui::Div {
    ui::pill(label, color)
}

fn repo_rows(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> impl IntoElement {
    let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
    let list = div().px_2().flex().flex_col();
    match &p.statuses {
        Load::Ready(statuses) => list.children(statuses.iter().map(|s| {
            let dropped = d.dropped.iter().find(|(r, _)| r == &s.repo).map(|(_, n)| *n);
            let repo = s.repo.clone();
            let pulling = p.pulling.as_deref() == Some(s.repo.as_str());
            ui::row(format!("ws-repo:{}", s.repo))
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(s.repo.clone()))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(c(theme::TEXT_FAINT)).child(format!(
                    "{}{}",
                    s.branch.clone().unwrap_or_else(|| "no branch".into()),
                    if s.branch_only { " · linked clone" } else { "" }
                )))
                .when_some(dropped, |dd, n| dd.child(badge(format!("PR #{n} closed"), c(theme::TEXT_DIM))))
                .when(s.off_branch, |dd| dd.child(badge("Off branch", c(theme::WARN))))
                .when_some(s.operation.clone(), |dd, op| dd.child(badge(format!("{op} in progress"), c(theme::WARN))))
                .child(if s.dirty { badge("Uncommitted changes", c(theme::WARN)) } else { badge("Clean", c(theme::OK)) })
                .when(s.integrated, |dd| dd.child(badge(format!("Merged into {base}"), c(theme::ACCENT))))
                .when(!s.integrated && s.ahead > 0, |dd| dd.child(badge(format!("↑ {}", s.ahead), c(theme::ACCENT))))
                .when(s.behind > 0 && s.ahead > 0, |dd| dd.child(badge(format!("↓ {}", s.behind), c(theme::WARN))))
                .when(s.behind > 0 && s.ahead == 0, |dd| {
                    let label = if pulling { "Pulling…".to_string() } else { format!("↓ {} Pull", s.behind) };
                    let repo = repo.clone();
                    dd.child(ui::button(format!("ws-pull:{repo}"), label, Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        cx.stop_propagation();
                        t.ws_pull(repo.clone(), w, cx)
                    })))
                })
                .child(ui::button(format!("ws-repo-page:{repo}"), "Repo →", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    cx.stop_propagation();
                    let repo = repo.clone();
                    t.with_app(w, cx, move |a, w, cx| {
                        super::open(a, PageId::Repo(repo), w, cx);
                    });
                })))
        })),
        Load::Failed(e) => list.child(div().px_2().text_xs().text_color(c(theme::DANGER)).child(e.clone())),
        _ => list.children(p.meta.iter().flat_map(|m| m.repos.clone()).map(|r| div().px_2().py_1().text_sm().text_color(c(theme::TEXT_DIM)).child(format!("{r} …")))),
    }
}

fn pr_rows(p: &WsPage, d: &Derived) -> impl IntoElement {
    let list = div().px_4().flex().flex_col().gap_1();
    match &p.prs {
        Load::Ready(prs) if prs.is_empty() => {
            let branch = p.meta.as_ref().map(|m| m.branch.clone()).unwrap_or_default();
            let text = if d.can_create_prs {
                "Ready for pull requests: open them from the PRs page, one per repo.".to_string()
            } else if !d.dirty.is_empty() {
                "Commit your changes, push them, then open the PRs.".to_string()
            } else if d.ahead > 0 {
                "Push your commits, then open the PRs.".to_string()
            } else {
                format!("PRs for {branch} across this workspace's repos show up here, with their CI checks.")
            };
            list.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(text))
        }
        Load::Ready(prs) => list.children(prs.iter().map(|pr| {
            let check = p.checks.ready().and_then(|cs| cs.iter().find(|c| c.repo == pr.repo && c.pr_number == pr.number)).map(|c| c.status.clone());
            let url = pr.url.clone();
            ui::row(format!("ws-pr:{}#{}", pr.repo, pr.number))
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().text_color(c(theme::ACCENT)).child(format!("{} #{}", pr.repo, pr.number)))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT)).child(pr.title.clone()))
                .when(pr.is_draft, |dd| dd.child(badge("draft", c(theme::TEXT_DIM))))
                .child(badge(pr.state.to_lowercase(), if pr.state == "OPEN" { c(theme::OK) } else { c(theme::TEXT_DIM) }))
                .when_some(check, |dd, s| dd.child(badge(format!("checks {s}"), ui::bucket_color(&s))))
                .on_click(move |_, _, cx| cx.open_url(&url))
        })),
        Load::Failed(e) => list.child(div().text_xs().text_color(c(theme::DANGER)).child(e.clone())),
        _ => list.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("Checking GitHub…")),
    }
}
