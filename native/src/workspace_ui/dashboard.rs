//! The dashboard (`DashboardPage.tsx`): a card per workspace with its repos
//! and a status summary, the dashed "New workspace" card, and onboarding
//! when there is nothing yet.
use super::icons::icon;
use super::kit::{self, Kind};
use super::{spawn, Page, PageTab};
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, Window};
use orbit_core::workspace::{self, RepoStatus, Workspace};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

#[derive(Default)]
pub struct Dashboard {
    pub workspaces: Load<Vec<Workspace>>,
    pub statuses: BTreeMap<String, Result<Vec<RepoStatus>, String>>,
    /// Configured repositories (`config.services`).
    pub repo_count: Option<usize>,
    /// Workspaces with a Ralph run in progress.
    pub ralph: BTreeSet<String>,
}

impl Dashboard {
    pub fn dump(&self, out: &mut String) {
        let _ = writeln!(out, "  workspaces: {} · repos {:?}", super::load_text(&self.workspaces), self.repo_count);
        for ws in self.workspaces.ready().into_iter().flatten() {
            let _ = writeln!(out, "    {} [{}] {}", ws.name, ws.branch, summary(self.statuses.get(&ws.name)).join(" · "));
        }
    }
}

/// The footer's parts: what is pending across the workspace's repos.
fn summary(st: Option<&Result<Vec<RepoStatus>, String>>) -> Vec<String> {
    match st {
        None => vec!["Checking status…".into()],
        Some(Err(_)) => vec!["Status unavailable".into()],
        Some(Ok(rows)) => {
            let dirty = rows.iter().filter(|r| r.dirty).count();
            let ahead: usize = rows.iter().map(|r| r.ahead).sum();
            let behind: usize = rows.iter().map(|r| r.behind).sum();
            if dirty == 0 && ahead == 0 && behind == 0 {
                return vec!["Up to date".into()];
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
            parts
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
                let statuses = std::thread::scope(|s| {
                    let handles: Vec<_> = names.iter().map(|n| (n.clone(), s.spawn(move || workspace::status(n)))).collect();
                    handles.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("thread panicked".into())))).collect::<BTreeMap<_, _>>()
                });
                let repos = orbit_core::config::Config::load().map(|c| c.services.len()).ok();
                let running = orbit_core::ralph::running().unwrap_or_default();
                let ralph: BTreeSet<String> = names.iter().filter(|n| running.iter().any(|k| k.starts_with(&format!("{n}/")))).cloned().collect();
                (list, statuses, repos, ralph)
            },
            |t, (list, statuses, repos, ralph), _, _| {
                if let Page::Dashboard(d) = &mut t.page {
                    d.workspaces = list.map_or_else(Load::Failed, Load::Ready);
                    d.statuses = statuses;
                    d.repo_count = repos;
                    d.ralph = ralph;
                }
            },
        );
    }

    fn open_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.with_app(window, cx, move |a, w, cx| super::open_workspace(a, &name, w, cx));
    }

    /// The card's right-click menu (`menuItems` in DashboardPage.tsx).
    fn card_menu(&mut self, name: String, origin: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::overlay::MenuItem;
        let item = |label: &'static str, cmd: Option<&'static str>, name: &String| {
            let name = name.clone();
            MenuItem::new(label, move |a, w, cx| match cmd {
                None => super::open_workspace(a, &name, w, cx),
                Some(cmd) => super::run_for(a, &name, cmd, w, cx),
            })
        };
        let items = vec![
            item("Open workspace", None, &name),
            item("Open Ralph", Some("ralph.open"), &name),
            item("Open in editor", Some("workspace.openInEditor"), &name),
            item("Open folder", Some("workspace.revealFolder"), &name),
        ];
        self.with_app(window, cx, move |a, w, cx| a.open_menu(origin, items, w, cx));
    }

    fn dash_command(&mut self, name: String, cmd: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.with_app(window, cx, move |a, w, cx| super::run_for(a, &name, cmd, w, cx));
    }

    pub(super) fn render_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Page::Dashboard(d) = &self.page else { return div().into_any_element() };
        let workspaces = d.workspaces.ready().cloned().unwrap_or_default();
        let repo_count = d.repo_count.unwrap_or(0);
        let new_ws = |id: &'static str, cx: &mut Context<Self>| {
            kit::button_icon(id, Some(("plus", 14.)), "New workspace", Kind::Primary, kit::NORMAL)
                .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_create(None, w, cx)))
        };
        let head = div()
            .mb(px(18.))
            .flex()
            .items_end()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(19.)).line_height(px(26.)).font_weight(kit::W_650).text_color(c(theme::TEXT)).child("Workspaces"))
                    .child(div().text_size(px(12.)).line_height(px(17.)).text_color(c(theme::TEXT_FAINT)).child(format!(
                        "{} workspace{} · {repo_count} repositor{}",
                        workspaces.len(),
                        if workspaces.len() == 1 { "" } else { "s" },
                        if repo_count == 1 { "y" } else { "ies" }
                    ))),
            )
            .when(repo_count > 0, |dd| dd.child(new_ws("dash-new-head", cx)));
        let body = if d.repo_count == Some(0) {
            onboard(
                "Welcome to Orbit",
                "Work on one feature across several repositories at once: every workspace is a set of git worktrees on the same branch.",
                Some(&[
                    ("Add your repositories", "From GitHub or any git URL. Orbit clones each one once."),
                    ("Create a workspace", "Pick the repos and a branch, or start from a Linear or Shortcut card."),
                    ("Ship it", "Commit, push and open PRs across repos together, or let Ralph implement a PRD."),
                ]),
            )
            .child(kit::button("dash-add-repos", "Add repositories", Kind::Primary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                t.with_app(w, cx, |a, w, cx| {
                    crate::commands::run("app.settings", a, w, cx);
                })
            })))
            .into_any_element()
        } else if d.workspaces.ready().is_some() && workspaces.is_empty() {
            onboard("No workspaces yet", "Create one to start a feature: Orbit sets up a worktree per repository, all on the same branch.", None)
                .child(new_ws("dash-new", cx))
                .into_any_element()
        } else {
            let cols = ((self.width.get() + 14.) / (320. + 14.)).floor().max(1.) as u16;
            div()
                .grid()
                .grid_cols(cols)
                .gap(px(14.))
                .children(workspaces.iter().map(|ws| self.card(d, ws, cx).into_any_element()))
                .child(
                    div()
                        .id("dash-new")
                        .relative()
                        .min_h(px(150.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .rounded(px(12.))
                        .border_1()
                        .border_dashed()
                        .border_color(c(theme::BORDER_STRONG))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(theme::TEXT_FAINT))
                        .cursor_pointer()
                        .group("dash-new")
                        .hover(|s| s.bg(theme::accent_soft()).border_color(c(theme::ACCENT)).text_color(c(theme::TEXT)))
                        .child(icon("plus", 16., c(theme::TEXT_FAINT)).group_hover("dash-new", |s| s.text_color(c(theme::TEXT))))
                        .child("New workspace")
                        .child(probe("dash-new"))
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_create(None, w, cx))),
                )
                .into_any_element()
        };
        div()
            .id("dashboard")
            .size_full()
            .overflow_y_scroll()
            .px(px(44.))
            .py(px(28.))
            .child(head)
            .child(div().relative().child(super::measure(self.width.clone())).child(body))
            .into_any_element()
    }

    fn card(&self, d: &Dashboard, ws: &Workspace, cx: &mut Context<Self>) -> impl IntoElement {
        let st = d.statuses.get(&ws.name);
        let rows = st.and_then(|s| s.as_ref().ok());
        let running = d.ralph.contains(&ws.name);
        let group = format!("dash-card:{}", ws.name);
        let name = ws.name.clone();
        let repo_chip = |repo: &String| {
            let r = rows.and_then(|rows| rows.iter().find(|x| &x.repo == repo));
            let dot = match (rows, r) {
                (None, _) => c(theme::TEXT_FAINT),
                (_, Some(r)) if r.dirty => c(theme::WARN),
                (_, Some(r)) if r.ahead > 0 => c(theme::ACCENT),
                (_, Some(_)) => c(theme::OK),
                (_, None) => c(theme::TEXT_FAINT),
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(9.))
                .py(px(3.))
                .rounded_full()
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .text_size(px(11.5))
                .line_height(px(15.))
                .text_color(c(theme::TEXT_DIM))
                .child(kit::dot(dot))
                .child(repo.clone())
                .when_some(r.filter(|r| r.ahead > 0), |dd, r| {
                    dd.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::ACCENT)).child(format!("↑{}", r.ahead)))
                })
        };
        let parts = summary(st);
        let summary_el = div().flex_1().min_w_0().flex().flex_wrap().gap(px(10.)).text_size(px(12.)).line_height(px(17.)).text_color(c(theme::TEXT_DIM)).children(
            parts.iter().map(|p| {
                let color = if p.ends_with("with changes") {
                    c(theme::WARN)
                } else if p == "Up to date" {
                    c(theme::OK)
                } else if p.ends_with("behind") || p.starts_with("Checking") || p.starts_with("Status") {
                    c(theme::TEXT_FAINT)
                } else {
                    c(theme::TEXT_DIM)
                };
                div().text_color(color).child(p.clone())
            }),
        );
        let action = |id: String, icon_name: &'static str, tip: &'static str, cmd: &'static str, cx: &mut Context<Self>| {
            let name = name.clone();
            kit::hint(kit::icon_button(id, icon_name, 28., 13., false), tip).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                cx.stop_propagation();
                t.dash_command(name.clone(), cmd, w, cx)
            }))
        };
        let card_link = ws.card.clone().map(|card| {
            let url = card.url.clone();
            kit::hint(
                div()
                    .id(format!("dash-card-link:{}", ws.name))
                    .flex_none()
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
                    .hover(|s| s.bg(kit::alpha(c(theme::ACCENT), 0.28)))
                    .child(card.id.clone()),
                card.title.clone(),
            )
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.open_url(&url)
            })
        });
        let open_name = ws.name.clone();
        let menu_name = ws.name.clone();
        div()
            .id(gpui::ElementId::Name(format!("dash-ws:{}", ws.name).into()))
            .group(group.clone())
            .relative()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_color(if running { kit::alpha(c(theme::ACCENT), 0.45) } else { c(theme::BORDER) })
            .bg(c(theme::PANEL))
            .cursor_pointer()
            .hover(|s| s.border_color(c(theme::BORDER_STRONG)).shadow(theme::shadow(8., 24., 0.25)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().flex_none().size(px(34.)).rounded(px(9.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(icon(
                        "satellite",
                        15.,
                        c(theme::ACCENT),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_size(px(14.5)).line_height(px(20.)).font_weight(kit::W_650).text_color(c(theme::TEXT)).child(ws.name.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(theme::MONO_FONT)
                                    .text_size(px(11.5))
                                    .line_height(px(15.))
                                    .text_color(c(theme::TEXT_FAINT))
                                    .child(ws.branch.clone()),
                            ),
                    )
                    .when(running, |dd| {
                        let name = ws.name.clone();
                        dd.child(
                            kit::hint(
                                div()
                                    .id(format!("dash-ralph:{}", ws.name))
                                    .h(px(26.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .rounded_full()
                                    .bg(theme::accent_soft())
                                    .border_1()
                                    .border_color(kit::alpha(c(theme::ACCENT), 0.35))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(kit::mix(c(theme::INFO), c(theme::TEXT), 0.4))
                                    .child(div().size(px(7.)).rounded_full().bg(c(theme::ACCENT)))
                                    .child("Ralph"),
                                "Ralph is running · open",
                            )
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                t.dash_command(name.clone(), "ralph.open", w, cx)
                            })),
                        )
                    }),
            )
            .child(div().flex().flex_wrap().gap(px(6.)).children(ws.repos.iter().map(repo_chip)))
            .child(
                div()
                    .mt_auto()
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(summary_el)
                    .children(card_link)
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .opacity(0.45)
                            .group_hover(group, |s| s.opacity(1.))
                            .child(action(format!("dash-ralph-btn:{}", ws.name), "spark", "Ralph", "ralph.open", cx))
                            .child(action(format!("dash-editor:{}", ws.name), "code", "Open in editor", "workspace.openInEditor", cx))
                            .child(action(format!("dash-folder:{}", ws.name), "folder", "Open folder", "workspace.revealFolder", cx)),
                    ),
            )
            .child(probe(format!("dash-ws:{}", ws.name)))
            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_workspace(open_name.clone(), w, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |t, e: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    t.card_menu(menu_name.clone(), e.position, w, cx)
                }),
            )
    }
}

/// `.dash-onboard`: a centred welcome with an icon, text and optional steps.
fn onboard(title: &'static str, text: &'static str, steps: Option<&[(&'static str, &'static str)]>) -> gpui::Div {
    div()
        .max_w(px(540.))
        .mx_auto()
        .mt(px(48.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.))
        .child(div().size(px(54.)).rounded(px(14.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(icon(
            "satellite",
            26.,
            c(theme::ACCENT),
        )))
        .child(div().mt(px(4.)).text_size(px(19.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(title))
        .child(div().text_center().text_size(px(13.)).line_height(px(21.)).text_color(c(theme::TEXT_DIM)).child(text))
        .when_some(steps, |d, steps| {
            d.child(div().w_full().flex().flex_col().gap(px(8.)).children(steps.iter().enumerate().map(|(i, (title, text))| {
                div()
                    .flex()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::PANEL))
                    .child(
                        div()
                            .flex_none()
                            .size(px(20.))
                            .rounded_full()
                            .bg(theme::accent_soft())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::ACCENT))
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(*title))
                            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(*text)),
                    )
            })))
        })
}
