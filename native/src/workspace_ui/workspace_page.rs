//! A workspace's page (`WorkspaceDetailPage.tsx`): the delivery flow
//! (changes → push → pull requests → checks, each with the action that moves
//! it forward), the branch drift banner, the repos with their badges (dirty,
//! ahead/behind with Pull, merged, off branch) and the PRs with their checks.
use super::icons::icon;
use super::kit::{self, Kind};
use super::{plural, spawn, Page, PageTab};
use crate::git::drift::Drift;
use crate::git::merge::MergeItem;
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::motion::TweenExt;
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, ClickEvent, Context, FontWeight, ScrollHandle, Window};
use orbit_core::agent::PlanTask;
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
    /// A change came in while refreshing: read again once it ends.
    pub stale: bool,
    pub pulling: Option<String>,
    pub drift: Drift,
    pub error: Option<String>,
    /// PLAN.md's checklist (None: not read yet).
    pub plan: Option<Vec<PlanTask>>,
    pub plan_exists: Option<bool>,
    /// The PR row whose checks are open (`repo#n`).
    pub expanded: Option<String>,
    /// The check being re-run.
    pub rerunning: Option<String>,
    pub scroll: ScrollHandle,
    /// Open REVIEW.md notes, for the Review Changes button.
    pub open_notes: usize,
    /// The plan checklist, which scrolls inside the page.
    pub plan_scroll: ScrollHandle,
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
            stale: false,
            pulling: None,
            drift: Drift::default(),
            error: None,
            plan: None,
            plan_exists: None,
            expanded: None,
            rerunning: None,
            scroll: ScrollHandle::new(),
            open_notes: 0,
            plan_scroll: ScrollHandle::new(),
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
        let _ = writeln!(out, "  scroll page {:.0} · plan {:.0} of {:.0}", -f32::from(self.scroll.offset().y), -f32::from(self.plan_scroll.offset().y), f32::from(self.plan_scroll.max_offset().y));
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
    pub(super) fn ws_page(&mut self) -> Option<&mut WsPage> {
        match &mut self.page {
            Page::Workspace(p) => Some(p),
            _ => None,
        }
    }

    /// Local status first; then (or with `fetch`) the remotes, PRs and checks.
    pub fn load_workspace(&mut self, fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        if p.refreshing {
            p.stale = true;
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
                let notes = crate::review::notes::list(&name).map_or(0, |f| crate::review::notes::open(Some(&f)).len());
                (meta, workspace::status(&name), remote, notes)
            },
            move |t, (meta, status, remote, notes), window, cx| {
                let Some(p) = t.ws_page() else { return };
                p.refreshing = false;
                let stale = std::mem::take(&mut p.stale);
                match meta {
                    Ok(m) => {
                        p.meta = Some(m);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
                p.statuses = status.map_or_else(Load::Failed, Load::Ready);
                p.open_notes = notes;
                if let Some((prs, checks)) = remote {
                    p.prs = prs.map_or_else(Load::Failed, Load::Ready);
                    p.checks = checks.map_or_else(Load::Failed, Load::Ready);
                }
                t.auto_sync_drift(window, cx);
                t.load_plan(window, cx);
                if let Some(f) = t.pending.take() {
                    f(t, window, cx);
                }
                if first && !fetch {
                    if let Some(p) = t.ws_page() {
                        p.prs = Load::Loading;
                        p.checks = Load::Loading;
                    }
                    t.load_workspace(true, window, cx);
                } else if stale {
                    t.load_workspace(false, window, cx);
                }
            },
        );
        cx.notify();
    }

    pub(super) fn ws_name(&mut self) -> Option<String> {
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

    /// One PR's Squash and merge (its row's button).
    fn merge_one(&mut self, item: MergeItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let (name, base) = (p.name.clone(), p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_else(|| "main".into()));
        let _ = window;
        self.open_merge(name, base, vec![item], cx);
    }

    /// The failing check's PR, expanded and scrolled to.
    fn review_failing(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        if let Some(f) = p.checks.ready().and_then(|cs| cs.iter().find(|c| c.status == "fail")) {
            p.expanded = Some(format!("{}#{}", f.repo, f.pr_number));
        }
        self.scroll_to_prs(cx);
    }

    /// The "Pull requests for this feature" modal.
    pub fn start_create_prs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let Some(m) = p.meta.clone() else { return };
        let title = m.card.as_ref().map(|c| c.title.clone());
        self.open_create_prs(m.name, m.base, title, m.repos, window, cx);
    }

    pub fn open_prs_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_review(crate::review::Page::PullRequests, window, cx);
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

    /// The open PRs in Orbit's own review, as Code Review → a feature → Code Review does;
    /// with none open, the create modal.
    fn open_code_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let Some(m) = &p.meta else { return };
        let list = if let Load::Ready(list) = &p.prs { list.as_slice() } else { &[] };
        let prs: Vec<_> = list
            .iter()
            .filter(|pr| pr.state == "OPEN")
            .filter_map(|pr| {
                // https://github.com/<owner>/<repo>/pull/<n>
                let path = pr.url.split("github.com/").nth(1)?;
                let owner_repo = path.split("/pull/").next()?.to_string();
                Some(orbit_core::github::PullRequest {
                    repo: pr.repo.clone(),
                    owner_repo,
                    number: pr.number,
                    title: pr.title.clone(),
                    branch: m.branch.clone(),
                    base: m.base.clone(),
                    author: pr.author.clone(),
                    is_draft: pr.is_draft,
                    url: pr.url.clone(),
                    updated_at: pr.updated_at.clone(),
                    status: None,
                })
            })
            .collect();
        if prs.is_empty() {
            return self.start_create_prs(window, cx);
        }
        self.with_app(window, cx, move |a, w, cx| crate::review::open_prs_review(a, prs, w, cx));
    }

    fn open_review(&mut self, page: crate::review::Page, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        self.with_app(window, cx, move |a, w, cx| crate::review::open_for(a, &name, page, w, cx));
    }

    fn open_review_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_review(crate::review::Page::Changes, window, cx);
    }
    /// The repo row's click (`onOpenEditor`): the dock shows that repo.
    fn browse_repo(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        self.with_app(window, cx, move |a, _, cx| {
            let Some(ws) = a.workspaces.iter().find(|w| w.meta.name == name) else { return };
            let dir = ws.dir.join(&repo);
            if a.layout.dock_hidden {
                a.layout.dock_hidden = false;
            }
            a.tree.update(cx, |t, cx| t.reveal(&dir, cx));
            cx.notify();
        });
    }

    /// Opens the workspace's PLAN.md in an editor tab.
    pub fn open_plan_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        self.with_app(window, cx, move |a, w, cx| {
            if let Ok(dir) = workspace::ws_dir(&name) {
                a.open_file(dir.join(orbit_core::agent::PLAN_FILE), false, w, cx);
            }
        });
    }

    fn ws_command(&mut self, cmd: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        self.with_app(window, cx, move |a, w, cx| super::run_for(a, &name, cmd, w, cx));
    }

    fn scroll_to_prs(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.ws_page() {
            p.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn rerun_check(&mut self, link: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.ws_page() {
            p.rerunning = Some(link.clone());
        }
        spawn(window, cx, move || orbit_core::prs::rerun(&link), |t, res, window, cx| {
            match res {
                Ok(()) => t.toast(ToastKind::Info, "Re-running failed jobs", None, window, cx),
                Err(e) => t.toast(ToastKind::Error, "Couldn't re-run the checks", Some(e), window, cx),
            }
            t.reload(true, window, cx);
        });
    }

    pub(super) fn render_workspace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Page::Workspace(p) = &self.page else { return div().into_any_element() };
        let d = p.derived();
        let statuses = p.statuses.ready().cloned().unwrap_or_default();
        let drift = self.render_drift(&statuses, &p.drift, cx);
        let done = p.statuses.ready().is_some() && d.merged > 0 && d.open == 0 && d.dirty.is_empty() && d.ahead == 0;
        let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
        let variant_of = p.meta.as_ref().and_then(|m| m.variant_of.clone());
        let page = div()
            .id("ws-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&p.scroll)
            .px(px(36.))
            .py(px(24.))
            .when_some(variant_of, |dd, parent| {
                let agent = p.meta.as_ref().and_then(|m| m.agent.clone());
                let target = parent.clone();
                dd.child(
                    div()
                        .mb(px(16.))
                        .px(px(12.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(kit::alpha(c(theme::ACCENT), 0.35))
                        .bg(theme::accent_soft())
                        .text_size(px(12.5))
                        .child(icon("race", 13., c(theme::ACCENT)))
                        .child(div().flex_1().text_color(c(theme::TEXT_DIM)).child(format!(
                            "Race variant of {parent}{}",
                            agent.map(|a| format!(" · {a}")).unwrap_or_default()
                        )))
                        .child(kit::link("ws-race-compare", format!("Compare in {target} →")).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            let target = target.clone();
                            t.with_app(w, cx, move |a, w, cx| super::open_workspace(a, &target, w, cx));
                        }))),
                )
            })
            .child(header(p, &d, cx))
            .children(drift)
            .when_some(p.error.clone(), |dd, e| dd.child(div().mb(px(12.)).text_size(px(12.)).text_color(c(theme::DANGER)).child(e)))
            .when(done, |dd| {
                dd.child(
                    div()
                        .mb(px(18.))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(kit::alpha(c(theme::PURPLE), 0.35))
                        .bg(kit::alpha(c(theme::PURPLE), 0.08))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(12.5))
                        .text_color(c(theme::TEXT_DIM))
                        .child(icon("check", 13., c(theme::PURPLE)))
                        .child(div().flex_1().child(format!(
                            "Merged into {base}. This feature is done; removing the workspace deletes its worktrees and local branches."
                        )))
                        .child(
                            kit::button("ws-done-remove", "Remove workspace", Kind::Secondary, kit::MINI)
                                .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.confirm_remove(false, w, cx))),
                        ),
                )
            })
            .child(flow_steps(p, &d, cx))
            .children(super::plan::progress(p, cx))
            .child(section_head("Repositories", None))
            .child(repo_rows(p, &d, cx))
            .child(div().mt(px(22.)).child(section_head("Pull requests", p.prs.ready().map(Vec::len).filter(|n| *n > 0))))
            .child(pr_section(p, &d, cx));
        crate::motion::page_in(page, gpui::ElementId::Name(format!("ws-page-in:{}", p.name).into()))
    }
}

fn section_head(title: &'static str, n: Option<usize>) -> impl IntoElement {
    div().mt(px(22.)).mb(px(10.)).flex().items_center().child(kit::section_title(title)).when_some(n, |d, n| d.child(kit::count(n)))
}

fn header(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> impl IntoElement {
    let meta = p.meta.as_ref();
    let card = meta.and_then(|m| m.card.clone());
    let is_variant = meta.is_some_and(|m| m.variant_of.is_some());
    let plan = p.plan_exists == Some(true);
    let tool = |id: &'static str, icon_spec: (&'static str, f32), label: &'static str, tip: String, f: fn(&mut PageTab, &mut Window, &mut Context<PageTab>), cx: &mut Context<PageTab>| {
        kit::hint(kit::button_icon(id, Some(icon_spec), label, Kind::Secondary, kit::TOOL), tip).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx)))
    };
    let base = meta.map(|m| m.base.clone()).unwrap_or_else(|| "main".into());
    let meta_row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.5))
        .line_height(px(17.))
        .when_some(meta, |dd, m| {
            dd.child(
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
                    .text_color(c(theme::TEXT))
                    .child(icon("git", 12., c(theme::TEXT)))
                    .child(m.branch.clone()),
            )
            .child(div().text_color(c(theme::TEXT_FAINT)).child("from"))
            .child(div().text_color(c(theme::TEXT_DIM)).child(m.base.clone()))
            .child(div().text_color(c(theme::TEXT_FAINT)).child("·"))
            .child(div().text_color(c(theme::TEXT_DIM)).child(plural(m.repos.len(), "repo")))
        })
        .when_some(card, |dd, card| {
            let url = card.url.clone();
            dd.child(
                kit::hint(
                    div()
                        .id("ws-card-link")
                        .h(px(22.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .bg(theme::accent_soft())
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(kit::mix(c(theme::INFO), c(theme::TEXT), 0.4))
                        .cursor_pointer()
                        .child(card.id.clone()),
                    card.title.clone(),
                )
                .on_click(move |_, _, cx| cx.open_url(&url)),
            )
        });
    let code_review = kit::hint(
        kit::button_icon("ws-code-review", Some(("diff", 14.)), "Code Review", Kind::Secondary, kit::TOOL),
        if d.open > 0 { "Review the feature's PRs: diffs, threads and verdict, one tab" } else { "No open pull request: create them first" },
    )
    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_code_review(w, cx)));
    let review_label = if p.open_notes > 0 { format!("Review Changes · {}", p.open_notes) } else { "Review Changes".into() };
    let has_review = !d.dirty.is_empty() || p.open_notes > 0;
    let review_changes = kit::hint(
        kit::disabled(kit::button_icon("ws-review-changes", Some(("commit", 14.)), review_label, Kind::Secondary, kit::TOOL), !has_review),
        if has_review {
            format!("Review uncommitted changes with notes for an agent ({})", plural(p.open_notes, "open note"))
        } else {
            "No uncommitted changes or open notes".into()
        },
    )
    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
        if has_review {
            t.open_review_changes(w, cx)
        }
    }));
    let refresh = if p.refreshing {
        kit::hint(
            div()
                .id("ws-refresh")
                .relative()
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::BORDER_STRONG))
                .opacity(0.45)
                .child(kit::spinner("ws-refresh-spin", 15., c(theme::ACCENT)))
                .child(probe("ws-refresh")),
            "Fetch and refresh status, PRs and checks",
        )
    } else {
        kit::hint(kit::icon_button("ws-refresh", "refresh", 32., 15., false), "Fetch and refresh status, PRs and checks")
            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.reload(true, w, cx)))
    };
    div()
        .mb(px(20.))
        .flex()
        .flex_wrap()
        .items_start()
        .justify_between()
        .gap(px(20.))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(px(20.25)).line_height(px(28.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(p.name.clone()))
                .child(meta_row),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(review_changes)
                .child(code_review)
                .child(tool(
                    "ws-plan",
                    ("doc", 14.),
                    if plan { "Plan" } else { "Create plan" },
                    if plan { "Open PLAN.md".into() } else { "Plan the feature: an interview, an AI draft, or your own".into() },
                    |t, w, cx| {
                        if matches!(&t.page, Page::Workspace(p) if p.plan_exists == Some(true)) {
                            t.open_plan_file(w, cx)
                        } else {
                            t.open_plan_modal(w, cx)
                        }
                    },
                    cx,
                ))
                .when(plan, |dd| {
                    dd.child(tool(
                        "ws-replan",
                        ("refresh", 13.),
                        "Replan",
                        "Replan: interview or draft a new plan (the current one is kept in .orbit/plans)".into(),
                        |t, w, cx| t.open_plan_modal(w, cx),
                        cx,
                    ))
                })
                .child(tool(
                    "ws-ralph",
                    ("spark", 14.),
                    "Ralph",
                    "Ralph builds the plan: each task by a fresh agent, in its repo, with live activity".into(),
                    |t, w, cx| t.ws_command("ralph.open", w, cx),
                    cx,
                ))
                .when(!is_variant, |dd| {
                    dd.child(tool(
                        "ws-race",
                        ("race", 14.),
                        "Race",
                        "Run the same task with several agents and keep the best result".into(),
                        |t, w, cx| t.open_race(w, cx),
                        cx,
                    ))
                })
                .child(tool("ws-rebase", ("rebase", 14.), "Rebase", format!("Rebase every repo onto origin/{base}"), |t, w, cx| t.start_rebase(w, cx), cx))
                .child(div().mx(px(4.)).w(px(1.)).h(px(22.)).bg(c(theme::BORDER)))
                .child(
                    kit::hint(kit::icon_button("ws-add-repos", "plus", 32., 15., false), "Add repositories to this workspace")
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_add_repos(w, cx))),
                )
                .child(refresh)
                .child(
                    kit::hint(kit::icon_button("ws-remove", "trash", 32., 15., true), "Remove workspace")
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.confirm_remove(false, w, cx))),
                ),
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
    icon: (&'static str, f32),
    title: &'static str,
    state: StepState,
    detail: String,
    /// Label, handler, and whether it stays secondary when next.
    action: Option<(&'static str, Act, bool)>,
    extra: Option<(&'static str, Act)>,
}

fn flow_steps(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> impl IntoElement {
    let loading = p.statuses.ready().is_none();
    let prs_loading = p.prs.ready().is_none();
    let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
    let n_prs = p.prs.ready().map_or(0, Vec::len);
    let steps = [
        Step {
            id: "flow-changes",
            icon: ("commit", 15.),
            title: "Changes",
            state: if loading { StepState::Loading } else if d.dirty.is_empty() { StepState::Done } else { StepState::Action },
            detail: if loading {
                "Checking…".into()
            } else if d.dirty.is_empty() {
                "Working tree clean".into()
            } else {
                format!("{} with uncommitted changes", plural(d.dirty.len(), "repo"))
            },
            action: (!d.dirty.is_empty()).then_some(("Commit", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_commit(w, cx)) as Act, false)),
            extra: (!d.dirty.is_empty() || p.open_notes > 0).then_some(("Review", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.open_review_changes(w, cx)) as Act)),
        },
        Step {
            id: "flow-push",
            icon: ("push", 15.),
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
            action: (d.ahead > 0).then_some(("Push", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_push(w, cx)) as Act, false)),
            extra: None,
        },
        Step {
            id: "flow-prs",
            icon: ("pull-request", 15.),
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
                "Checking GitHub…".into()
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
                Some(("Create", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_create_prs(w, cx)) as Act, false))
            } else if d.can_merge {
                Some(("Squash and merge", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.start_merge(w, cx)) as Act, true))
            } else if n_prs > 0 {
                Some(("View", (|t: &mut PageTab, _: &mut Window, cx: &mut Context<PageTab>| t.scroll_to_prs(cx)) as Act, true))
            } else {
                None
            },
            extra: None,
        },
        Step {
            id: "flow-checks",
            icon: ("check", 14.),
            title: "Checks",
            state: match &p.checks {
                Load::Ready(_) if d.failing => StepState::Fail,
                Load::Ready(_) if d.running => StepState::Running,
                Load::Ready(_) if d.passing => StepState::Done,
                Load::Ready(_) | Load::Failed(_) => StepState::Idle,
                _ => StepState::Loading,
            },
            detail: match &p.checks {
                Load::Ready(_) if d.failing => "Some checks failed".into(),
                Load::Ready(_) if d.running => "Running…".into(),
                Load::Ready(_) if d.passing => "All checks passed".into(),
                Load::Ready(_) | Load::Failed(_) => "No checks yet".into(),
                _ => "Checking CI…".into(),
            },
            action: d.failing.then_some(("Review", (|t: &mut PageTab, w: &mut Window, cx: &mut Context<PageTab>| t.review_failing(w, cx)) as Act, false)),
            extra: None,
        },
    ];
    // Only the first step that needs something is highlighted: the next move.
    let next = steps.iter().position(|s| matches!(s.state, StepState::Action | StepState::Fail));
    let last = steps.len() - 1;
    div().mb(px(22.)).grid().grid_cols(4).gap(px(10.)).children(steps.into_iter().enumerate().map(|(i, s)| {
        let is_next = Some(i) == next;
        let accent_text = kit::mix(c(theme::INFO), c(theme::TEXT), 0.7);
        let (tile_bg, tile_fg) = match s.state {
            _ if is_next && s.state != StepState::Fail => (theme::accent_soft(), accent_text),
            StepState::Done => (kit::alpha(c(theme::OK), 0.12), c(theme::OK)),
            StepState::Fail => (theme::danger_soft(), c(theme::DANGER)),
            StepState::Running => (c(theme::PANEL_RAISED), c(theme::ACCENT)),
            _ => (c(theme::PANEL_RAISED), c(theme::TEXT_DIM)),
        };
        let tile = div().flex_none().size(px(26.)).rounded(px(7.)).bg(tile_bg).flex().items_center().justify_center();
        let tile = match s.state {
            StepState::Done => tile.child(icon("check", 13., tile_fg)).into_any_element(),
            StepState::Fail => tile.child(icon("x", 12., tile_fg)).into_any_element(),
            StepState::Running | StepState::Loading => tile.child(kit::spinner(format!("{}-spin", s.id), 14., c(theme::ACCENT))).into_any_element(),
            _ => tile.child(icon(s.icon.0, s.icon.1, tile_fg)).into_any_element(),
        };
        let detail_color = match s.state {
            StepState::Fail => c(theme::DANGER),
            StepState::Done | StepState::Action => c(theme::TEXT_DIM),
            _ => c(theme::TEXT_FAINT),
        };
        let panel = c(theme::PANEL);
        let card = div()
            .relative()
            .min_h(px(108.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(14.))
            .px(px(14.))
            .pb(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(if is_next {
                kit::alpha(c(theme::ACCENT), 0.5)
            } else if s.state == StepState::Fail {
                kit::alpha(c(theme::DANGER), 0.4)
            } else {
                c(theme::BORDER)
            })
            .map(|d| {
                if is_next {
                    d.bg(gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(kit::mix(c(theme::ACCENT), panel, 0.15), 0.),
                        gpui::linear_color_stop(panel, 0.7),
                    ))
                } else {
                    d.bg(panel)
                }
            })
            .when(s.state == StepState::Idle, |d| d.opacity(0.75))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(tile)
                    .child(div().text_size(px(13.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(s.title))
                    .when(i < last, |d| d.child(div().ml_auto().text_size(px(13.)).text_color(c(theme::TEXT_FAINT)).child("→"))),
            )
            .child(div().text_size(px(12.)).line_height(px(16.8)).text_color(detail_color).child(s.detail))
            .child(probe(s.id));
        card.when(s.action.is_some() || s.extra.is_some(), |card| {
            let id = s.id;
            card.child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .when_some(s.action, |dd, (label, f, secondary)| {
                        let kind = if secondary || !is_next { Kind::Secondary } else { Kind::Primary };
                        dd.child(kit::button(format!("{id}-btn"), label, kind, kit::FLOW).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx))))
                    })
                    .when_some(s.extra, |dd, (label, f)| {
                        dd.child(kit::button(format!("{id}-extra"), label, Kind::Secondary, kit::FLOW).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx))))
                    }),
            )
        })
    }))
}

fn repo_rows(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> impl IntoElement {
    let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
    let list = div().mt(px(4.)).flex().flex_col().rounded(px(10.)).border_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL)).overflow_hidden();
    let tile = || {
        div()
            .flex_none()
            .size(px(32.))
            .rounded(px(8.))
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("git", 15., c(theme::TEXT_DIM)))
    };
    let row = |i: usize| {
        div().flex().items_center().gap(px(14.)).px(px(14.)).py(px(12.)).when(i > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
    };
    let name_col = |name: String, sub: gpui::AnyElement| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().text_size(px(13.5)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(name))
            .child(sub)
    };
    match &p.statuses {
        Load::Ready(statuses) => list.children(statuses.iter().enumerate().map(|(i, s)| {
            let dropped = d.dropped.iter().find(|(r, _)| r == &s.repo).map(|(_, n)| *n);
            let repo = s.repo.clone();
            let pulling = p.pulling.as_deref() == Some(s.repo.as_str());
            let group = format!("ws-repo:{}", s.repo);
            let sub = div()
                .truncate()
                .font_family(theme::MONO_FONT)
                .text_size(px(11.5))
                .line_height(px(15.))
                .text_color(c(theme::TEXT_FAINT))
                .child(s.branch.clone().unwrap_or_else(|| "no branch".into()))
                .into_any_element();
            let tip = |el: gpui::Div, id: String, text: String| kit::hint(el.id(gpui::ElementId::Name(id.into())), text);
            let badges = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when_some(dropped, |dd, n| {
                    dd.child(tip(
                        kit::badge(c(theme::TEXT_DIM)).child("PR closed"),
                        format!("ws-dropped:{repo}"),
                        format!("PR #{n} was closed without merging: Push leaves this repo out"),
                    ))
                })
                .when(s.off_branch, |dd| {
                    dd.child(tip(
                        kit::badge(c(theme::WARN)).child(icon("branch", 11., c(theme::WARN))).child("Off branch"),
                        format!("ws-off:{repo}"),
                        format!("Expected {}{}", s.expected_branch, s.held_by.as_ref().map(|h| format!("; the clone is on workspace {h}'s branch")).unwrap_or_default()),
                    ))
                })
                .when_some(s.operation.clone(), |dd, op| dd.child(kit::badge(c(theme::WARN)).child(format!("{op} in progress"))))
                .child(if s.dirty {
                    kit::badge(c(theme::WARN)).child(kit::dot(c(theme::WARN))).child("Uncommitted changes")
                } else {
                    kit::badge(c(theme::TEXT_DIM)).child(kit::dot(c(theme::OK))).child("Clean")
                })
                .when(s.integrated, |dd| {
                    dd.child(tip(
                        kit::badge(c(theme::PURPLE)).font_weight(FontWeight::SEMIBOLD).child("Merged"),
                        format!("ws-merged:{repo}"),
                        format!("This branch's work is already in {base}"),
                    ))
                })
                .when(!s.integrated && s.ahead > 0, |dd| {
                    dd.child(tip(
                        kit::badge(c(theme::INFO)).font_weight(FontWeight::SEMIBOLD).child(format!("↑ {}", s.ahead)),
                        format!("ws-ahead:{repo}"),
                        format!("{} commit(s) not pushed", s.ahead),
                    ))
                })
                .when(s.behind > 0 && s.ahead > 0, |dd| {
                    dd.child(tip(kit::badge(c(theme::WARN)).child(format!("↓ {}", s.behind)), format!("ws-behind:{repo}"), format!("{} commit(s) behind the remote", s.behind)))
                })
                .when(s.behind > 0 && s.ahead == 0, |dd| {
                    let label = if pulling { "Pulling…".to_string() } else { format!("↓ {} Pull", s.behind) };
                    let repo = repo.clone();
                    dd.child(
                        tip(
                            kit::badge(c(theme::WARN)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().child(label).child(probe(format!("ws-pull:{repo}"))).relative(),
                            format!("ws-pull:{repo}"),
                            format!("{} new commit(s) on the remote — click to pull them", s.behind),
                        )
                        .hover(|s| s.border_color(c(theme::WARN)))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            cx.stop_propagation();
                            t.ws_pull(repo.clone(), w, cx)
                        })),
                    )
                });
            let open_repo = repo.clone();
            row(i)
                .id(gpui::ElementId::Name(group.clone().into()))
                .group(group.clone())
                .relative()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .child(tile())
                .child(name_col(s.repo.clone(), sub))
                .child(
                    div()
                        .mr(px(4.))
                        .text_size(px(12.))
                        .text_color(c(theme::ACCENT))
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child("Browse files →"),
                )
                .child(badges)
                .child(probe(format!("ws-repo:{}", s.repo)))
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.browse_repo(open_repo.clone(), w, cx)))
                .into_any_element()
        })),
        Load::Failed(e) => list.child(row(0).text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone())),
        _ => list.children(p.meta.iter().flat_map(|m| m.repos.clone()).enumerate().map(|(i, r)| {
            row(i)
                .child(tile())
                .child(name_col(r, crate::motion::skeleton(px(140.), 10.).into_any_element()))
                .into_any_element()
        })),
    }
}

fn pr_section(p: &WsPage, d: &Derived, cx: &mut Context<PageTab>) -> gpui::AnyElement {
    match &p.prs {
        Load::Ready(prs) if prs.is_empty() => {
            let (branch, base) = p.meta.as_ref().map(|m| (m.branch.clone(), m.base.clone())).unwrap_or_default();
            let (title, text) = if d.can_create_prs {
                ("Ready for pull requests", format!("Orbit opens one PR per repo from {branch} into {base}, with AI-drafted titles and descriptions."))
            } else if !d.dirty.is_empty() {
                ("No pull requests yet", "Commit your changes, push them, then open the PRs here.".to_string())
            } else if d.ahead > 0 {
                ("No pull requests yet", "Push your commits, then open the PRs here.".to_string())
            } else {
                ("No pull requests yet", format!("PRs for {branch} across this workspace's repos show up here, with their CI checks."))
            };
            let action = if d.can_create_prs {
                Some(kit::button("ws-prs-create", "Create pull requests", Kind::Primary, kit::SECTION).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.start_create_prs(w, cx))))
            } else if !d.dirty.is_empty() {
                Some(kit::button("ws-prs-commit", "Commit changes", Kind::Secondary, kit::SECTION).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.start_commit(w, cx))))
            } else if d.ahead > 0 {
                Some(kit::button("ws-prs-push", "Push", Kind::Secondary, kit::SECTION).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.start_push(w, cx))))
            } else {
                None
            };
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .px(px(20.))
                .py(px(18.))
                .rounded(px(10.))
                .border_1()
                .border_dashed()
                .border_color(c(theme::BORDER_STRONG))
                .child(div().flex_none().size(px(38.)).rounded(px(10.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(icon(
                    "pull-request",
                    18.,
                    c(theme::ACCENT),
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(div().text_size(px(13.5)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(title))
                        .child(div().text_size(px(12.5)).line_height(px(18.75)).text_color(c(theme::TEXT_FAINT)).child(text)),
                )
                .children(action)
                .into_any_element()
        }
        Load::Ready(prs) => {
            let base = p.meta.as_ref().map(|m| m.base.clone()).unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .rounded(px(10.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::PANEL))
                .overflow_hidden()
                .children(prs.iter().enumerate().map(|(i, pr)| pr_row(p, pr, i, &base, cx)))
                .into_any_element()
        }
        Load::Failed(e) => div().text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone()).into_any_element(),
        _ => div()
            .rounded(px(10.))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .px(px(14.))
            .py(px(12.))
            .child(crate::motion::skeleton(gpui::relative(0.6), 14.))
            .into_any_element(),
    }
}

fn pr_row(p: &WsPage, pr: &WsPrStatus, i: usize, base: &str, cx: &mut Context<PageTab>) -> gpui::AnyElement {
    let check = p.checks.ready().and_then(|cs| cs.iter().find(|c| c.repo == pr.repo && c.pr_number == pr.number)).cloned();
    let key = format!("{}#{}", pr.repo, pr.number);
    let open = p.expanded.as_deref() == Some(key.as_str());
    let state = match (pr.state.as_str(), pr.is_draft) {
        ("MERGED", _) => "merged",
        ("CLOSED", _) => "closed",
        (_, true) => "draft",
        _ => "open",
    };
    let (state_fg, state_bg) = match state {
        "open" => (c(theme::OK), kit::alpha(c(theme::OK), 0.12)),
        "merged" => (c(theme::PURPLE), kit::alpha(c(theme::PURPLE), 0.14)),
        "closed" => (c(theme::DANGER), theme::danger_soft()),
        _ => (c(theme::TEXT_FAINT), c(theme::PANEL_RAISED)),
    };
    let pill_border = match state {
        "open" => kit::alpha(c(theme::OK), 0.35),
        "merged" => kit::alpha(c(theme::PURPLE), 0.35),
        "closed" => kit::alpha(c(theme::DANGER), 0.35),
        _ => c(theme::BORDER_STRONG),
    };
    let status = check.as_ref().map_or("none".to_string(), |c| c.status.clone());
    let total = check.as_ref().map_or(0, |c| c.checks.len());
    let passed = check.as_ref().map_or(0, |c| c.checks.iter().filter(|x| x.bucket == "pass").count());
    let dot = match status.as_str() {
        "pass" => div().size(px(9.)).rounded_full().bg(c(theme::OK)).into_any_element(),
        "fail" => div().size(px(9.)).rounded_full().bg(c(theme::DANGER)).into_any_element(),
        "running" => kit::spinner(format!("pr-spin:{key}"), 10., c(theme::ACCENT)).into_any_element(),
        _ => div().size(px(9.)).rounded_full().border(px(1.5)).border_color(c(theme::TEXT_FAINT)).into_any_element(),
    };
    let url = pr.url.clone();
    let toggle_key = key.clone();
    let blocked = matches!(status.as_str(), "fail" | "running");
    let merge_item = MergeItem { repo: pr.repo.clone(), number: pr.number, title: pr.title.clone() };
    let base = base.to_string();
    div()
        .when(i > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
        .child(
            div()
                .id(gpui::ElementId::Name(format!("ws-pr:{key}").into()))
                .relative()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(12.))
                .cursor_pointer()
                .when(open, |d| d.bg(c(theme::PANEL_RAISED)))
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .child(div().flex_none().size(px(30.)).rounded(px(8.)).bg(state_bg).flex().items_center().justify_center().child(icon("pull-request", 13., state_fg)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(theme::TEXT))
                                .child(format!("{} ", pr.title))
                                .child(div().font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_FAINT)).child(format!("#{}", pr.number))),
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
                                        .border_color(pill_border)
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if state == "draft" { c(theme::TEXT_FAINT) } else { state_fg })
                                        .child(format!("{}{}", state[..1].to_uppercase(), &state[1..])),
                                )
                                .child(format!("{} · {}", plural(pr.commits as usize, "commit"), pr.author)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(12.))
                        .text_color(c(theme::TEXT_DIM))
                        .child(dot)
                        .child(if total > 0 { format!("{passed}/{total} checks") } else { "No checks".into() }),
                )
                .when(state == "open", |d| {
                    let tip = match status.as_str() {
                        "fail" => "Checks are failing".to_string(),
                        "running" => "Checks are still running".to_string(),
                        _ => format!("Squash into {base} and delete the branch on GitHub"),
                    };
                    let item = merge_item.clone();
                    d.child(
                        kit::hint(kit::disabled(kit::button(format!("ws-pr-merge:{key}"), "Squash and merge", Kind::Secondary, kit::MINI), blocked), tip).when(!blocked, |b| {
                            b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                t.merge_one(item.clone(), w, cx)
                            }))
                        }),
                    )
                })
                .child(kit::hint(kit::icon_button(format!("ws-pr-gh:{key}"), "github", 28., 14., false), "Open on GitHub ↗").on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url)
                }))
                // `.disclosure-chevron`: rotates over `--transition`.
                .child(icon("disclosure", 13., c(theme::TEXT_FAINT)).tween(
                    gpui::ElementId::Name(format!("ws-pr-chevron:{key}").into()),
                    if open { 0.25 } else { 0. },
                    crate::motion::TRANSITION,
                    crate::motion::standard(),
                    |s, v| s.with_transformation(gpui::Transformation::rotate(gpui::percentage(v))),
                ))
                .child(probe(format!("ws-pr:{key}")))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    if let Some(p) = t.ws_page() {
                        p.expanded = if p.expanded.as_deref() == Some(toggle_key.as_str()) { None } else { Some(toggle_key.clone()) };
                    }
                    cx.notify();
                })),
        )
        .when(open, |d| d.child(checks_list(p, check.as_ref(), &key, cx)))
        .into_any_element()
}

fn checks_list(p: &WsPage, check: Option<&WsCheck>, key: &str, cx: &mut Context<PageTab>) -> impl IntoElement {
    let wrap = div().flex().flex_col().gap(px(4.)).pt(px(4.)).pb(px(12.)).pl(px(56.)).pr(px(14.)).border_t_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL_RAISED));
    let Some(check) = check.filter(|c| !c.checks.is_empty()) else {
        return wrap.child(div().py(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("No CI checks on this PR."));
    };
    wrap.children(check.checks.iter().enumerate().map(|(i, ch)| {
        let failed = ch.bucket == "fail";
        let running = ch.bucket == "pending" || ch.bucket == "queued";
        let glyph = if running {
            kit::spinner(format!("check-spin:{key}:{i}"), 12., c(theme::ACCENT)).into_any_element()
        } else if failed {
            icon("x", 11., c(theme::DANGER)).into_any_element()
        } else if ch.bucket == "pass" {
            icon("check", 11., c(theme::OK)).into_any_element()
        } else {
            icon("circle", 10., c(theme::TEXT_FAINT)).into_any_element()
        };
        let link = ch.link.clone();
        let rerun = ch.link.clone();
        let rerunning = p.rerunning.as_deref() == Some(ch.link.as_str());
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .text_size(px(12.))
            .when(failed, |d| d.bg(kit::alpha(c(theme::DANGER), 0.07)))
            .child(div().w(px(14.)).flex_none().flex().justify_center().child(glyph))
            .child(
                div()
                    .id(gpui::ElementId::Name(format!("check:{key}:{i}").into()))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO_FONT)
                    .text_color(c(theme::TEXT_DIM))
                    .when(!link.is_empty(), |d| d.cursor_pointer().hover(|s| s.text_color(c(theme::ACCENT))).on_click(move |_, _, cx| cx.open_url(&link)))
                    .child(ch.name.clone()),
            )
            .when(failed && !rerunning, |d| {
                d.child(
                    kit::hint(kit::button(format!("check-rerun:{key}:{i}"), "↻ Re-run", Kind::Primary, kit::MINI), "Re-run the failed jobs (fixes intermittent failures)")
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.rerun_check(rerun.clone(), w, cx))),
                )
            })
            .when(rerunning, |d| d.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child("re-running…")))
    }))
}
