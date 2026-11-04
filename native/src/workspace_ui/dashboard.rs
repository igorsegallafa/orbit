//! The dashboard (`DashboardPage.tsx`): a card per workspace with its repos
//! and a status summary (changes, to push, behind), onboarding when there is
//! nothing yet, and the configured repositories, each opening its page.
use super::{plural, spawn, Page, PageId, PageTab};
use crate::review::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, ClickEvent, Context, Window};
use orbit_core::repo::RepoBrief;
use orbit_core::workspace::{self, RepoStatus, Workspace};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Default)]
pub struct Dashboard {
    pub workspaces: Load<Vec<Workspace>>,
    pub statuses: BTreeMap<String, Result<Vec<RepoStatus>, String>>,
    pub repos: Load<Vec<RepoBrief>>,
}

impl Dashboard {
    pub fn dump(&self, out: &mut String) {
        let _ = writeln!(out, "  workspaces: {}", super::load_text(&self.workspaces));
        for ws in self.workspaces.ready().into_iter().flatten() {
            let _ = writeln!(out, "    {} [{}] {}", ws.name, ws.branch, summary(self.statuses.get(&ws.name)));
        }
        let _ = writeln!(out, "  repositories: {}", super::load_text(&self.repos));
        for r in self.repos.ready().into_iter().flatten() {
            let _ = writeln!(
                out,
                "    {} {} changes {} ahead {} behind {}",
                r.name,
                r.branch.as_deref().unwrap_or(if r.cloned { "-" } else { "(not cloned)" }),
                r.changes,
                r.ahead,
                r.behind
            );
        }
    }
}

/// "Up to date", or what is pending across the workspace's repos.
fn summary(st: Option<&Result<Vec<RepoStatus>, String>>) -> String {
    match st {
        None => "Checking status…".into(),
        Some(Err(_)) => "Status unavailable".into(),
        Some(Ok(rows)) => {
            let dirty = rows.iter().filter(|r| r.dirty).count();
            let ahead: usize = rows.iter().map(|r| r.ahead).sum();
            let behind: usize = rows.iter().map(|r| r.behind).sum();
            if dirty == 0 && ahead == 0 && behind == 0 {
                return "Up to date".into();
            }
            let mut parts = vec![];
            if dirty > 0 {
                parts.push(format!("{dirty} with changes"));
            }
            if ahead > 0 {
                parts.push(format!("{ahead} to push"));
            }
            if behind > 0 {
                parts.push(format!("{behind} behind"));
            }
            parts.join(" · ")
        }
    }
}

impl PageTab {
    pub fn load_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn(
            window,
            cx,
            || {
                let list = workspace::list();
                let names: Vec<String> = list.iter().flatten().map(|w| w.name.clone()).collect();
                let (statuses, briefs) = std::thread::scope(|s| {
                    let handles: Vec<_> = names.iter().map(|n| (n.clone(), s.spawn(move || workspace::status(n)))).collect();
                    let briefs = s.spawn(orbit_core::repo::briefs);
                    let statuses: BTreeMap<_, _> =
                        handles.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("thread panicked".into())))).collect();
                    (statuses, briefs.join().unwrap_or_else(|_| Err("thread panicked".into())))
                });
                (list, statuses, briefs)
            },
            |t, (list, statuses, briefs), _, _| {
                if let Page::Dashboard(d) = &mut t.page {
                    d.workspaces = list.map_or_else(Load::Failed, Load::Ready);
                    d.statuses = statuses;
                    d.repos = briefs.map_or_else(Load::Failed, Load::Ready);
                }
            },
        );
    }

    fn open_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.with_app(window, cx, move |a, w, cx| {
            if let Some(ix) = a.workspaces.iter().position(|ws| ws.meta.name == name) {
                a.select_workspace(ix, cx);
            }
            super::open(a, PageId::Workspace(name), w, cx);
        });
    }

    pub(super) fn render_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Page::Dashboard(d) = &self.page else { return div().into_any_element() };
        let workspaces = d.workspaces.ready().cloned().unwrap_or_default();
        let repo_count = d.repos.ready().map_or(0, Vec::len);
        let head = div()
            .px_4()
            .pt_4()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .child(div().text_xl().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("Workspaces"))
                    .child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(format!(
                        "{} · {}",
                        plural(workspaces.len(), "workspace"),
                        if repo_count == 1 { "1 repository".to_string() } else { format!("{repo_count} repositories") }
                    ))),
            )
            .child(ui::button("dash-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.reload(false, w, cx))))
            .when(repo_count > 0, |dd| {
                dd.child(ui::button("dash-new", "+ New workspace", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_create(None, w, cx))))
            });
        let onboarding = |title: &'static str, text: &'static str| {
            div()
                .mx_4()
                .mt_4()
                .p_4()
                .rounded(px(10.))
                .border_1()
                .border_color(c(theme::BORDER))
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_base().text_color(c(theme::TEXT)).child(title))
                .child(div().text_sm().text_color(c(theme::TEXT_DIM)).child(text))
        };
        let cards = if d.repos.ready().is_some() && repo_count == 0 {
            onboarding(
                "Welcome to Orbit",
                "Work on one feature across several repositories at once: every workspace is a set of git worktrees on the same branch. Add your repositories in Settings first.",
            )
        } else if d.workspaces.ready().is_some() && workspaces.is_empty() {
            onboarding("No workspaces yet", "Create one to start a feature: Orbit sets up a worktree per repository, all on the same branch.")
        } else {
            div().px_4().pt_3().flex().flex_wrap().gap_3().children(workspaces.iter().map(|ws| {
                let st = d.statuses.get(&ws.name);
                let rows = st.and_then(|s| s.as_ref().ok());
                let name = ws.name.clone();
                ui::row(format!("dash-ws:{}", ws.name))
                    .w(px(300.))
                    .p_3()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .rounded(px(10.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_base().text_color(c(theme::TEXT)).child(ws.name.clone()))
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(ws.branch.clone()))
                    .child(div().flex().flex_wrap().gap_1().children(ws.repos.iter().map(|repo| {
                        let r = rows.and_then(|rows| rows.iter().find(|x| &x.repo == repo));
                        let color = match r {
                            None => c(theme::TEXT_FAINT),
                            Some(r) if r.dirty => c(theme::WARN),
                            Some(r) if r.ahead > 0 => c(theme::ACCENT),
                            Some(_) => c(theme::OK),
                        };
                        let ahead = r.filter(|r| r.ahead > 0).map(|r| format!(" ↑{}", r.ahead)).unwrap_or_default();
                        ui::pill(format!("{repo}{ahead}"), color)
                    })))
                    .child(div().flex().gap_2().text_xs().text_color(c(theme::TEXT_DIM)).child(summary(st)).when_some(ws.card.clone(), |dd, card| {
                        dd.child(div().text_color(c(theme::ACCENT)).child(card.id))
                    }))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_workspace(name.clone(), w, cx)))
            }))
        };
        let repos = match &d.repos {
            Load::Ready(list) => div().px_2().flex().flex_col().children(list.iter().map(|r| {
                let name = r.name.clone();
                let state = if !r.cloned {
                    "not cloned".to_string()
                } else {
                    let mut s = r.branch.clone().unwrap_or_else(|| "detached".into());
                    if r.changes > 0 {
                        s.push_str(&format!(" · {} changed", r.changes));
                    }
                    if r.ahead > 0 {
                        s.push_str(&format!(" · ↑{}", r.ahead));
                    }
                    if r.behind > 0 {
                        s.push_str(&format!(" · ↓{}", r.behind));
                    }
                    s
                };
                ui::row(format!("dash-repo:{}", r.name))
                    .flex()
                    .gap_2()
                    .text_sm()
                    .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(r.name.clone()))
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(state))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let name = name.clone();
                        t.with_app(w, cx, move |a, w, cx| {
                            super::open(a, PageId::Repo(name), w, cx);
                        })
                    }))
            })),
            Load::Failed(e) => div().px_4().text_xs().text_color(c(theme::DANGER)).child(e.clone()),
            _ => div().px_4().text_xs().text_color(c(theme::TEXT_FAINT)).child("Loading…"),
        };
        div()
            .id("dashboard")
            .size_full()
            .overflow_y_scroll()
            .pb_6()
            .child(head)
            .child(cards)
            .child(ui::section("REPOSITORIES"))
            .child(repos)
            .into_any_element()
    }
}
