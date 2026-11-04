//! Code Review (`CodeReviewPage.tsx`): open PRs from every configured repo
//! of the last 7 days, grouped by branch so a multi-repo feature is one row,
//! with review filters and a search over all PRs.
use super::pr_ui::{self, lucide, Tone};
use super::Load;
use crate::app::OrbitApp;
use crate::pages::{Host, PageView};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, App, ClickEvent, ClipboardItem, Context, Entity, FontWeight, MouseButton, MouseDownEvent, SharedString,
    Subscription, Task, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use orbit_core::github::{PrGroup, PullRequest};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Review,
    Reviewed,
    Mine,
}

const FILTERS: [(Filter, &str, &str); 4] = [
    (Filter::All, "All", "Every open PR"),
    (Filter::Review, "To review", "Not reviewed by you yet, or new commits since your review"),
    (Filter::Reviewed, "Reviewed", "You reviewed the latest commits"),
    (Filter::Mine, "Mine", "PRs you opened"),
];

/// A group's PRs read as one: the worst state wins (`groupStatus`).
#[derive(Debug, PartialEq)]
pub struct GroupStatus {
    pub decision: Option<&'static str>,
    pub checks: Option<&'static str>,
    pub conflicts: bool,
    pub mine: bool,
    pub draft: bool,
    pub my_review: Option<&'static str>,
    pub reviewed: usize,
    pub total: usize,
    pub stale: bool,
}

pub fn group_status(prs: &[PullRequest]) -> Option<GroupStatus> {
    let st: Vec<_> = prs.iter().filter_map(|p| p.status.as_ref()).collect();
    if st.is_empty() {
        return None;
    }
    let is = |o: &Option<String>, v: &str| o.as_deref() == Some(v);
    let reviewed: Vec<_> = st.iter().filter(|s| s.my_review.is_some() && !is(&s.my_review, "DISMISSED")).collect();
    Some(GroupStatus {
        decision: if st.iter().any(|s| is(&s.review_decision, "CHANGES_REQUESTED")) {
            Some("CHANGES_REQUESTED")
        } else if st.iter().all(|s| is(&s.review_decision, "APPROVED")) {
            Some("APPROVED")
        } else if st.iter().any(|s| is(&s.review_decision, "REVIEW_REQUIRED")) {
            Some("REVIEW_REQUIRED")
        } else {
            None
        },
        checks: ["fail", "pending", "pass"].into_iter().find(|v| st.iter().any(|s| is(&s.checks, v))),
        conflicts: st.iter().any(|s| s.conflicts),
        mine: st.iter().any(|s| s.mine),
        draft: prs.iter().all(|p| p.is_draft),
        my_review: if reviewed.is_empty() {
            None
        } else if reviewed.iter().any(|s| is(&s.my_review, "CHANGES_REQUESTED")) {
            Some("CHANGES_REQUESTED")
        } else if reviewed.iter().all(|s| is(&s.my_review, "APPROVED")) {
            Some("APPROVED")
        } else {
            Some("COMMENTED")
        },
        reviewed: reviewed.len(),
        total: st.len(),
        stale: reviewed.iter().any(|s| s.my_review_stale),
    })
}

fn matches(f: Filter, s: Option<&GroupStatus>) -> bool {
    if f == Filter::All {
        return true;
    }
    let Some(s) = s else { return false };
    let to_review = !s.mine && (s.reviewed < s.total || s.stale);
    match f {
        Filter::Review => to_review,
        Filter::Reviewed => !s.mine && !to_review,
        _ => s.mine,
    }
}

/// The one state worth a pill on a row (`primaryState`), with its tooltip.
pub fn primary_state(s: &GroupStatus) -> Option<(&'static str, Tone, String)> {
    let ready = s.decision == Some("APPROVED") && !matches!(s.checks, Some("fail" | "pending")) && !s.conflicts && !s.draft;
    let partial = if s.total > 1 && s.reviewed < s.total { format!(" ({} of {} PRs)", s.reviewed, s.total) } else { String::new() };
    let mine = s.my_review.map(|r| match r {
        "APPROVED" => ("You approved", Tone::Ok),
        "CHANGES_REQUESTED" => ("You requested changes", Tone::Bad),
        _ => ("You commented", Tone::Muted),
    });
    if s.conflicts {
        return Some(("Conflicts", Tone::Bad, "Conflicts with the base branch".into()));
    }
    if s.stale {
        return Some(("New commits", Tone::Warn, format!("{}{partial}; commits were pushed since", mine.map_or("You reviewed", |m| m.0))));
    }
    if ready {
        return Some(("Ready to merge", Tone::Ok, "Approved, checks passing, no conflicts".into()));
    }
    if s.mine {
        return Some(match s.decision {
            Some("CHANGES_REQUESTED") => ("Changes requested", Tone::Bad, "A reviewer asked for changes on your PR".into()),
            Some("APPROVED") => ("Approved", Tone::Ok, "Your PR is approved".into()),
            _ => ("Your PR", Tone::Info, "You opened this PR".into()),
        });
    }
    if let Some((label, tone)) = mine {
        return Some((label, tone, format!("Your latest review{partial}")));
    }
    match s.decision {
        Some("CHANGES_REQUESTED") => Some(("Changes requested", Tone::Bad, "Another reviewer asked for changes".into())),
        Some("APPROVED") => Some(("Approved", Tone::Ok, "Approved by another reviewer".into())),
        _ => None,
    }
}

/// The local workspace holding these PRs (`workspaceForPrs`).
pub fn workspace_for(prs: &[PullRequest], workspaces: &[Workspace]) -> Option<Workspace> {
    workspaces
        .iter()
        .find(|w| {
            prs.iter().any(|p| {
                w.meta.repos.contains(&p.repo)
                    && (w.meta.branch == p.branch || w.meta.pr_refs.iter().any(|r| r.repo == p.repo && r.number == p.number))
            })
        })
        .cloned()
}

/// A workspace name from a branch (`suggestName`).
pub fn suggest_name(branch: &str, taken: &[String]) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch).to_lowercase();
    let mut base = String::new();
    for ch in last.chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "review".to_string() } else { base };
    if !taken.contains(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| !taken.contains(n)).unwrap_or(base)
}

/// Runs `f` on the root view.
pub fn with_app(host: &Host, window: &mut Window, cx: &mut App, f: impl FnOnce(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>)) {
    if let Some(app) = host.0.upgrade() {
        app.update(cx, |a, cx| f(a, window, cx));
    }
}

/// "Check out as workspace": one worktree per repo on the PRs' branch.
pub fn checkout(host: &Host, prs: Vec<PullRequest>, window: &mut Window, cx: &mut App) {
    let Some(first) = prs.first().cloned() else { return };
    let taken: Vec<String> = crate::workspace::list_workspaces().into_iter().map(|w| w.meta.name).collect();
    let name = suggest_name(&first.branch, &taken);
    let h = host.clone();
    let message = format!("Creates workspace {name} with a worktree on {} in {}.", first.branch, prs.iter().map(|p| p.repo.as_str()).collect::<Vec<_>>().join(", "));
    host.confirm(
        format!("Check out {}", first.branch),
        message,
        "Check out",
        move |window, cx| {
            let (h, name, prs, base) = (h.clone(), name.clone(), prs.clone(), first.base.clone());
            h.toast(ToastKind::Info, format!("Checking out {}…", prs[0].branch), Some(format!("Workspace {name}")), cx);
            let specs: Vec<_> = prs
                .iter()
                .map(|p| orbit_core::workspace::PrCheckout { repo: p.repo.clone(), number: p.number, branch: p.branch.clone(), url: p.url.clone() })
                .collect();
            window
                .spawn(cx, async move |cx| {
                    let r = cx.background_spawn(async move { orbit_core::workspace::create_from_prs(&name, &base, &specs) }).await;
                    let _ = cx.update(|window, cx| match r {
                        Ok((ws, failures)) => {
                            let desc = if failures.is_empty() { None } else { Some(failures.join("\n")) };
                            h.toast(ToastKind::Success, format!("Workspace {} is ready", ws.name), desc, cx);
                            let name = ws.name.clone();
                            with_app(&h, window, cx, |a, w, cx| {
                                crate::workspace_ui::reload_workspaces(a, Some(&name), cx);
                                crate::workspace_ui::open(a, crate::workspace_ui::PageId::Workspace(name), w, cx);
                            });
                        }
                        Err(e) => h.error("Couldn't check out", e, cx),
                    });
                })
                .detach();
        },
        window,
        cx,
    );
}

pub struct CodeReviewPage {
    host: Host,
    pub groups: Load<Vec<PrGroup>>,
    refreshing: bool,
    pub filter: Filter,
    query: Entity<InputState>,
    /// The search shown, and its results.
    pub search: Option<(String, Load<Vec<PrGroup>>)>,
    workspaces: Vec<Workspace>,
    _debounce: Option<Task<()>>,
    _sub: Subscription,
}

impl CodeReviewPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search any PR by title, branch or author"));
        let sub = cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query_changed(window, cx);
            }
        });
        let mut page = Self {
            host,
            groups: Load::Idle,
            refreshing: false,
            filter: Filter::All,
            query,
            search: None,
            workspaces: crate::workspace::list_workspaces(),
            _debounce: None,
            _sub: sub,
        };
        page.load(false, window, cx);
        page
    }

    pub fn load(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        if self.groups.ready().is_none() {
            self.groups = Load::Loading;
        }
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { orbit_core::github::list_prs(force) }).await;
            let _ = this.update(cx, |p, cx| {
                p.refreshing = false;
                match r {
                    Ok(g) => p.groups = Load::Ready(g),
                    Err(e) => {
                        if p.groups.ready().is_none() {
                            p.groups = Load::Failed(e.clone());
                        }
                        p.host.error("Couldn't load pull requests", e, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.query.read(cx).value().trim().to_string();
        if q.is_empty() {
            self.search = None;
            self._debounce = None;
            return cx.notify();
        }
        self.search = Some((q.clone(), Load::Loading));
        self._debounce = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let query = q.clone();
            let r = cx.background_spawn(async move { orbit_core::github::search_prs(&query) }).await;
            let _ = this.update(cx, |p, cx| {
                if p.search.as_ref().is_some_and(|(s, _)| *s == q) {
                    p.search = Some((q, r.map_or_else(Load::Failed, Load::Ready)));
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn open_group(&self, prs: Vec<PullRequest>, window: &mut Window, cx: &mut App) {
        let host = self.host.clone();
        with_app(&self.host, window, cx, move |a, w, cx| super::feature::open(a, host, prs, w, cx));
    }

    fn row_menu(&self, g: &PrGroup, window: &mut Window, cx: &mut Context<Self>) {
        let ws = workspace_for(&g.prs, &self.workspaces);
        let (prs, prs2, url, branch) = (g.prs.clone(), g.prs.clone(), g.prs[0].url.clone(), g.branch.clone());
        let (h1, h2, h3) = (self.host.clone(), self.host.clone(), self.host.clone());
        let mut items: Vec<(String, crate::pages::Action)> = vec![(
            "Review".into(),
            std::rc::Rc::new(move |w: &mut Window, cx: &mut App| {
                let (h, prs) = (h1.clone(), prs.clone());
                with_app(&h1, w, cx, move |a, w, cx| super::feature::open(a, h, prs, w, cx));
            }),
        )];
        items.push(match ws {
            Some(ws) => {
                let name = ws.meta.name.clone();
                (
                    format!("Open workspace {name}"),
                    std::rc::Rc::new(move |w: &mut Window, cx: &mut App| {
                        let name = name.clone();
                        with_app(&h2, w, cx, move |a, w, cx| {
                            crate::workspace_ui::open(a, crate::workspace_ui::PageId::Workspace(name), w, cx);
                        });
                    }),
                )
            }
            None => ("Check out as workspace…".into(), std::rc::Rc::new(move |w: &mut Window, cx: &mut App| checkout(&h2, prs2.clone(), w, cx))),
        });
        items.push(("Open on GitHub ↗".into(), std::rc::Rc::new(move |_: &mut Window, cx: &mut App| cx.open_url(&url))));
        items.push((
            "Copy branch name".into(),
            std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()));
                h3.toast(ToastKind::Success, "Branch name copied", Some(branch.clone()), cx);
            }),
        ));
        self.host.menu(items, window, cx);
    }

    fn render_row(&self, ix: usize, g: &PrGroup, now: i64, cx: &mut Context<Self>) -> impl IntoElement {
        let multi = g.prs.len() > 1;
        let first = &g.prs[0];
        let status = group_status(&g.prs);
        let state = status.as_ref().and_then(primary_state);
        let draft = g.prs.iter().all(|p| p.is_draft);
        let ws = workspace_for(&g.prs, &self.workspaces);
        let name = format!("cr-row:{ix}");
        let sep = || div().mx(px(7.)).text_color(c(theme::BORDER_STRONG)).child("·");
        let repo = |p: &PullRequest| {
            div()
                .flex()
                .flex_none()
                .gap(px(4.))
                .text_color(c(theme::TEXT_DIM))
                .child(p.repo.clone())
                .child(div().text_color(c(theme::TEXT_FAINT)).child(format!("#{}", p.number)))
        };
        let mut meta = div().flex().items_center().min_w_0().overflow_hidden().text_size(px(11.5)).line_height(px(15.)).text_color(c(theme::TEXT_FAINT));
        if multi {
            for (i, p) in g.prs.iter().enumerate() {
                if i > 0 {
                    meta = meta.child(sep());
                }
                meta = meta.child(repo(p));
            }
        } else {
            meta = meta
                .child(repo(first))
                .child(sep())
                .child(div().flex_none().child(first.author.clone()))
                .child(sep())
                .child(div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(11.)).text_color(c(theme::TEXT_DIM)).child(first.branch.clone()));
        }
        let icon_tip = if draft { "Draft".to_string() } else if multi { format!("Feature across {} repos", g.prs.len()) } else { "Open".into() };
        let checks = status.as_ref().and_then(|s| s.checks);
        let group = format!("cr-row-{ix}");
        let (prs, prs_menu, prs_co) = (g.prs.clone(), g.clone(), g.prs.clone());
        let url = first.url.clone();
        div()
            .id(SharedString::from(name.clone()))
            .group(SharedString::from(group.clone()))
            .relative()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(10.))
            .min_h(px(56.))
            .cursor_pointer()
            .when(ix > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
            .child(probe(name))
            .on_click(cx.listener(move |p, _: &ClickEvent, window, cx| p.open_group(prs.clone(), window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |p, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    p.row_menu(&prs_menu, window, cx)
                }),
            )
            .child(pr_ui::tip(
                div()
                    .id(SharedString::from(format!("cr-icon:{ix}")))
                    .relative()
                    .flex_none()
                    .size(px(15.))
                    .child(lucide("pr", 15., c(if draft { theme::TEXT_FAINT } else { theme::OK })))
                    .when(multi, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(-7.))
                                .bottom(px(-5.))
                                .min_w(px(13.))
                                .h(px(13.))
                                .px(px(3.))
                                .rounded(px(7.))
                                .bg(c(theme::ACCENT))
                                .border_2()
                                .border_color(c(theme::PANEL))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(c(theme::ON_ACCENT))
                                .child(g.prs.len().to_string()),
                        )
                    }),
                icon_tip,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.5))
                                    .line_height(px(18.))
                                    .font_weight(if draft { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD })
                                    .text_color(c(if draft { theme::TEXT_DIM } else { theme::TEXT }))
                                    .child(if multi { g.branch.clone() } else { first.title.clone() }),
                            )
                            .when(draft, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .px(px(5.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(c(theme::BORDER_STRONG))
                                        .text_size(px(10.5))
                                        .text_color(c(theme::TEXT_FAINT))
                                        .child("Draft"),
                                )
                            }),
                    )
                    .child(meta),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(12.))
                    .when_some(state, |d, (label, tone, hint)| {
                        d.child(pr_ui::tip(div().id(SharedString::from(format!("cr-state:{ix}"))).child(pr_ui::status_pill(label, tone)), hint))
                    })
                    .child(
                        div().w(px(14.)).flex().justify_center().children(checks.map(|ch| match ch {
                            "pass" => lucide("check", 13., c(theme::OK)).into_any_element(),
                            "fail" => lucide("x", 13., c(theme::DANGER)).into_any_element(),
                            _ => div().size(px(7.)).rounded_full().bg(c(theme::WARN)).into_any_element(),
                        })),
                    )
                    .child(
                        div()
                            .w(px(52.))
                            .text_right()
                            .text_size(px(11.5))
                            .text_color(c(theme::TEXT_FAINT))
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(pr_ui::rel_time(&first.updated_at, now)),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .flex()
                            .gap(px(6.))
                            .pl(px(16.))
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(
                                pr_ui::btn_mini(format!("cr-checkout:{ix}"), if ws.is_some() { "Workspace" } else { "Check out" }).on_click(cx.listener(
                                    move |p, _: &ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        match &ws {
                                            Some(ws) => {
                                                let name = ws.meta.name.clone();
                                                with_app(&p.host, window, cx, move |a, w, cx| {
                                                    crate::workspace_ui::open(a, crate::workspace_ui::PageId::Workspace(name), w, cx);
                                                });
                                            }
                                            None => checkout(&p.host, prs_co.clone(), window, cx),
                                        }
                                    },
                                )),
                            )
                            .child(pr_ui::btn_mini(format!("cr-github:{ix}"), "↗").on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                cx.open_url(&url);
                            })),
                    ),
            )
    }

    fn render_toolbar(&self, show_segments: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = self.groups.ready().cloned().unwrap_or_default();
        let mut segs = div().flex().flex_none().gap(px(2.)).p(px(3.)).bg(c(theme::PANEL)).border_1().border_color(c(theme::BORDER)).rounded(px(8.));
        for (f, label, hint) in FILTERS {
            let on = self.filter == f;
            let count = groups.iter().filter(|g| matches(f, group_status(&g.prs).as_ref())).count();
            let id = format!("cr-filter:{}", label.to_lowercase().replace(' ', "-"));
            segs = segs.child(pr_ui::tip(
                div()
                    .id(SharedString::from(id.clone()))
                    .relative()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c(theme::PANEL_RAISED)).border_color(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                    .when(!on, |d| {
                        d.border_color(gpui::transparent_black())
                            .text_color(c(theme::TEXT_DIM))
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                    })
                    .child(label)
                    .child(div().text_size(px(11.)).text_color(c(if on { theme::TEXT_DIM } else { theme::TEXT_FAINT })).child(count.to_string()))
                    .child(probe(id))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.filter = f;
                        cx.notify();
                    })),
                hint,
            ));
        }
        let count = self.search.as_ref().map(|(_, l)| match l {
            Load::Ready(r) => format!("{} found", r.len()),
            Load::Failed(_) => "failed".into(),
            _ => "searching…".into(),
        });
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mb(px(14.))
            .when(show_segments, |d| d.child(segs))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(180.))
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(10.))
                    .pr(px(10.))
                    .bg(c(theme::PANEL))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .child(lucide("search", 14., c(theme::TEXT_FAINT)))
                    .child(div().flex_1().child(Input::new(&self.query).appearance(false).text_size(px(13.))))
                    .when_some(count, |d, n| d.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(n)))
                    .child(probe("cr-search")),
            )
    }

    fn render_empty(&self, searching: bool) -> impl IntoElement {
        let label = FILTERS.iter().find(|f| f.0 == self.filter).map_or("", |f| f.1);
        let (title, body) = if searching {
            ("No PRs match your search".to_string(), "Try another term — search covers every configured repository.")
        } else if self.filter != Filter::All {
            (format!("No PRs in \"{label}\""), "Open PRs from the last 7 days that match this filter show up here.")
        } else {
            ("No open PRs in the last 7 days".to_string(), "PRs updated recently will appear here. Use the search above to find older or merged PRs.")
        };
        div()
            .py(px(48.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .child(lucide("search", 28., c(theme::TEXT_FAINT)))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(title))
            .child(div().text_size(px(12.5)).text_color(c(theme::TEXT_FAINT)).child(body))
    }
}

/// `.cr-rows`: the bordered list card.
fn rows_card() -> gpui::Div {
    div().flex().flex_col().border_1().border_color(c(theme::BORDER)).rounded(px(theme::size::RADIUS)).bg(c(theme::PANEL)).overflow_hidden()
}

impl Render for CodeReviewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let searching = self.search.is_some();
        let display: Option<Vec<PrGroup>> = match &self.search {
            Some((_, l)) => l.ready().cloned(),
            None => self.groups.ready().map(|g| g.iter().filter(|g| matches(self.filter, group_status(&g.prs).as_ref())).cloned().collect()),
        };
        let failed = match (&self.search, &self.groups) {
            (None, Load::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        let show_segments = !searching && self.groups.ready().is_some_and(|g| !g.is_empty());
        let now = pr_ui::now();
        let body = match display {
            None if failed.is_some() => {
                div().py(px(48.)).flex().justify_center().text_color(c(theme::DANGER)).text_size(px(12.5)).child(failed.unwrap_or_default()).into_any_element()
            }
            None => rows_card()
                .children((0..5).map(|i| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(14.))
                        .py(px(10.))
                        .min_h(px(56.))
                        .when(i > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
                        .child(div().size(px(16.)).rounded(px(4.)).bg(c(theme::PANEL_RAISED)))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(div().w(gpui::relative(0.45)).h(px(12.)).rounded(px(4.)).bg(c(theme::PANEL_RAISED)))
                                .child(div().w(gpui::relative(0.3)).h(px(10.)).rounded(px(4.)).bg(c(theme::PANEL_RAISED))),
                        )
                }))
                .into_any_element(),
            Some(list) if list.is_empty() => self.render_empty(searching).into_any_element(),
            Some(list) => rows_card().children(list.iter().enumerate().map(|(i, g)| self.render_row(i, g, now, cx))).into_any_element(),
        };
        let refreshing = self.refreshing;
        div()
            .id("code-review")
            .size_full()
            .overflow_y_scroll()
            .px(px(44.))
            .py(px(28.))
            .font_family(theme::UI_FONT)
            .text_color(c(theme::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(18.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().text_size(px(19.)).font_weight(FontWeight(650.)).child("Code Review"))
                            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Open PRs across your repositories from the last 7 days")),
                    )
                    .child(pr_ui::tip(
                        pr_ui::icon_button("cr-refresh", if refreshing { "loader-circle" } else { "refresh-cw" }, 28., 14.)
                            .on_click(cx.listener(|p, _, window, cx| p.load(true, window, cx))),
                        "Reload open PRs from the last 7 days",
                    )),
            )
            .child(self.render_toolbar(show_segments, cx))
            .child(body)
    }
}

impl PageView for CodeReviewPage {
    fn title(&self) -> SharedString {
        "Code Review".into()
    }

    fn dump(&self, out: &mut String, _: &App) {
        let _ = writeln!(out, "  code review: {} filter {:?}", crate::pages::ui::load_label(&self.groups), self.filter);
        for g in self.groups.ready().into_iter().flatten() {
            let st = group_status(&g.prs);
            let state = st.as_ref().and_then(primary_state).map(|s| s.0).unwrap_or("-");
            let _ = writeln!(out, "    {} ({} prs) state {state} checks {:?}", g.branch, g.prs.len(), st.as_ref().and_then(|s| s.checks));
        }
        if let Some((q, l)) = &self.search {
            let _ = writeln!(out, "  search {q:?}: {}", crate::pages::ui::load_label(l));
        }
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspaces = crate::workspace::list_workspaces();
        self.load(false, window, cx);
    }
}

/// Opens (or shows) the Code Review page.
pub fn open_code_review(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    crate::pages::open(app, "page:code-review".into(), CodeReviewPage::new, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_core::github::PrReviewStatus;

    fn pr(repo: &str, status: Option<PrReviewStatus>) -> PullRequest {
        PullRequest {
            repo: repo.into(),
            owner_repo: format!("o/{repo}"),
            number: 1,
            title: "t".into(),
            branch: "feat/x".into(),
            base: "main".into(),
            author: "a".into(),
            is_draft: false,
            url: String::new(),
            updated_at: String::new(),
            status,
        }
    }

    #[test]
    fn worst_state_wins_and_filters_match() {
        let approved = PrReviewStatus { review_decision: Some("APPROVED".into()), checks: Some("pass".into()), ..Default::default() };
        let conflicted = PrReviewStatus { conflicts: true, checks: Some("fail".into()), ..Default::default() };
        let s = group_status(&[pr("a", Some(approved.clone())), pr("b", Some(conflicted))]).unwrap();
        assert_eq!(s.checks, Some("fail"));
        assert_eq!(primary_state(&s).unwrap().0, "Conflicts");
        assert!(matches(Filter::Review, Some(&s)));
        let ready = group_status(&[pr("a", Some(approved))]).unwrap();
        assert_eq!(primary_state(&ready).unwrap().0, "Ready to merge");
        assert!(group_status(&[pr("a", None)]).is_none());
        assert!(!matches(Filter::Mine, None));
    }

    #[test]
    fn suggests_free_names() {
        assert_eq!(suggest_name("feat/EX-186 spawn", &[]), "ex-186-spawn");
        assert_eq!(suggest_name("x", &["x".into(), "x-2".into()]), "x-3");
        assert_eq!(suggest_name("feat/--", &[]), "review");
    }
}
