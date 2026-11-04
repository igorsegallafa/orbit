//! The Pull requests page: the workspace's open PRs and their live status,
//! CI checks with failed-check logs and re-runs, and creating PRs for the
//! repos that have none (`PrsModal.tsx`, the workspace page's tracker).
use super::ui::{self, Tone};
use super::{toolbar, Load, ReviewTab};
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, Window};
use gpui_component::input::InputState;
use orbit_core::prs::{self, PrSpec};

/// A repo PRs can be created for, with its title and description.
pub struct CreateRow {
    pub repo: String,
    pub include: bool,
    pub title: Entity<InputState>,
    pub body: Entity<InputState>,
    pub drafting: bool,
}

/// A title from the branch: `feat/add-login` → "Add login".
pub fn title_from_branch(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch).replace(['-', '_'], " ");
    let mut chars = last.trim().chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl ReviewTab {
    pub(super) fn refresh_prs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prs = Load::Loading;
        self.status = Load::Loading;
        self.checks = Load::Loading;
        let ws = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (prs, status, checks) = cx
                .background_spawn(async move {
                    // Independent gh round trips: side by side.
                    std::thread::scope(|s| {
                        let a = s.spawn(|| prs::list(&ws));
                        let b = s.spawn(|| prs::status(&ws));
                        let c = s.spawn(|| prs::checks(&ws));
                        let failed = || "thread panicked".to_string();
                        (
                            a.join().unwrap_or_else(|_| Err(failed())),
                            b.join().unwrap_or_else(|_| Err(failed())),
                            c.join().unwrap_or_else(|_| Err(failed())),
                        )
                    })
                })
                .await;
            let _ = this.update_in(cx, |t, window, cx| {
                t.prs = prs.map_or_else(Load::Failed, Load::Ready);
                t.status = status.map_or_else(Load::Failed, Load::Ready);
                t.checks = checks.map_or_else(Load::Failed, Load::Ready);
                t.sync_create_rows(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// One row per repo without an open PR.
    fn sync_create_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_pr: Vec<String> = self.prs.ready().into_iter().flatten().map(|p| p.repo.clone()).collect();
        self.create.retain(|r| !has_pr.contains(&r.repo));
        let title = title_from_branch(&self.branch);
        for repo in self.repos.clone() {
            if has_pr.contains(&repo) || self.create.iter().any(|r| r.repo == repo) {
                continue;
            }
            let t = title.clone();
            let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title").default_value(t));
            let body = cx.new(|cx| InputState::new(window, cx).placeholder("Description (optional)"));
            self.create.push(CreateRow { repo, include: true, title, body, drafting: false });
        }
    }

    fn create_prs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let specs: Vec<PrSpec> = self
            .create
            .iter()
            .filter(|r| r.include)
            .map(|r| PrSpec { repo: r.repo.clone(), title: r.title.read(cx).value().to_string(), body: r.body.read(cx).value().to_string(), draft: false })
            .collect();
        if specs.is_empty() {
            self.message = Some("Pick at least one repository".into());
            return cx.notify();
        }
        if let Some(r) = specs.iter().find(|s| s.title.trim().is_empty()) {
            self.message = Some(format!("{}: the PR needs a title", r.repo));
            return cx.notify();
        }
        self.creating = true;
        self.message = None;
        let (ws, base) = (self.workspace.clone(), self.base.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { prs::create_all(&ws, &base, &specs) }).await;
            let _ = this.update_in(cx, |t, window, cx| {
                t.creating = false;
                match result {
                    Ok(list) => {
                        let n = list.len();
                        t.with_app(window, cx, |a, _, cx| {
                            a.toast(crate::toast::ToastKind::Success, format!("{n} pull request{} open", if n == 1 { "" } else { "s" }), None, cx)
                        });
                        t.refresh_prs(window, cx);
                    }
                    Err(e) => t.message = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// AI title and description for one repo (`ws_pr_draft`).
    fn draft(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.create.get_mut(ix) else { return };
        row.drafting = true;
        let (ws, repo) = (self.workspace.clone(), row.repo.clone());
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { orbit_core::agent::pr_draft(&ws, &repo) }).await;
            let _ = this.update_in(cx, |t, window, cx| {
                let Some(row) = t.create.get_mut(ix) else { return };
                row.drafting = false;
                match r {
                    Ok(d) => {
                        row.title.update(cx, |i, cx| i.set_value(d.title, window, cx));
                        row.body.update(cx, |i, cx| i.set_value(d.body, window, cx));
                    }
                    Err(e) => t.message = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_log(&mut self, title: String, link: String, window: &mut Window, cx: &mut Context<Self>) {
        self.log = Some((title, Load::Loading));
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { prs::check_logs(&link) }).await;
            let _ = this.update(cx, |t, cx| {
                if let Some((_, l)) = &mut t.log {
                    *l = r.map_or_else(Load::Failed, Load::Ready);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn rerun(&mut self, link: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { prs::rerun(&link) }).await;
            let _ = this.update_in(cx, |t, window, cx| {
                match r {
                    Ok(()) => t.with_app(window, cx, |a, _, cx| a.toast(crate::toast::ToastKind::Success, "Re-running the failed jobs", None, cx)),
                    Err(e) => t.message = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_prs(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = toolbar(div().child(format!("{} → {}", self.branch, self.base)))
            .when_some(self.message.clone(), |d, m| d.child(div().text_xs().text_color(c(theme::WARN)).child(m)))
            .child(ui::button("prs-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.refresh_prs(w, cx))));
        let mut body = div().id("prs-body").flex_1().min_h_0().overflow_y_scroll().pb_6();

        // Open PRs, with the tracker's live state.
        body = body.child(ui::section("PULL REQUESTS"));
        body = match &self.prs {
            Load::Ready(list) if list.is_empty() => body.child(dim("No open pull request for this branch")),
            Load::Ready(list) => body.children(list.iter().map(|p| {
                let state = self.status.ready().and_then(|s| s.iter().find(|s| s.repo == p.repo && s.number == p.number));
                let url = p.url.clone();
                ui::row(format!("pr:{}#{}", p.repo, p.number))
                    .mx_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(div().text_color(c(theme::ACCENT)).child(format!("{} #{}", p.repo, p.number)))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT)).child(p.title.clone()))
                    .when(p.is_draft, |d| d.child(ui::pill("draft", c(theme::TEXT_DIM))))
                    .when_some(state, |d, s| d.child(ui::pill(s.state.to_lowercase(), c(theme::OK))).child(dim_inline(format!("{} commits", s.commits))))
                    .child(dim_inline(p.author.clone()))
                    .on_click(move |_, _, cx| cx.open_url(&url))
            })),
            other => body.child(load_line(other)),
        };
        // Saved PRs no longer open (merged, closed).
        let closed: Vec<_> = self
            .status
            .ready()
            .into_iter()
            .flatten()
            .filter(|s| s.state != "OPEN")
            .map(|s| dim(format!("{} #{} {} · {}", s.repo, s.number, s.state.to_lowercase(), s.title)))
            .collect();
        body = body.children(closed);

        // Checks.
        body = body.child(ui::section("CHECKS"));
        body = match &self.checks {
            Load::Ready(list) if list.is_empty() => body.child(dim("No saved pull requests to check")),
            Load::Ready(list) => {
                let mut b = body;
                for w in list {
                    b = b.child(
                        div()
                            .px_3()
                            .pt_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(div().size(px(8.)).rounded_full().bg(ui::bucket_color(&w.status)))
                            .child(div().text_color(c(theme::TEXT)).child(format!("{} #{}", w.repo, w.pr_number)))
                            .child(dim_inline(w.status.clone())),
                    );
                    for (k, ch) in w.checks.iter().enumerate() {
                        let actions = ch.bucket == "fail" && orbit_core::github::run_id_from_link(&ch.link).is_some();
                        let (link, link2, title) = (ch.link.clone(), ch.link.clone(), format!("{} · {}", w.repo, ch.name));
                        let probe = format!("check:{}#{}:{k}", w.repo, w.pr_number);
                        b = b.child(
                            div()
                                .pl_6()
                                .pr_3()
                                .py(px(2.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .child(div().w(px(56.)).flex_none().text_color(ui::bucket_color(&ch.bucket)).child(ch.bucket.clone()))
                                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT)).child(ch.name.clone()))
                                .child(dim_inline(ch.workflow.clone()))
                                .when(actions, |d| {
                                    d.child(
                                        ui::button(format!("{probe}:logs"), "Logs", Tone::Plain)
                                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.show_log(title.clone(), link.clone(), w, cx))),
                                    )
                                    .child(
                                        ui::button(format!("{probe}:rerun"), "Re-run", Tone::Plain)
                                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.rerun(link2.clone(), w, cx))),
                                    )
                                }),
                        );
                    }
                }
                b
            }
            other => body.child(load_line(other)),
        };
        if let Some((title, log)) = &self.log {
            body = body.child(
                ui::section(format!("LOG · {title}"))
                    .child(div().flex_1())
                    .child(ui::button("log-close", "Close", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.log = None;
                        cx.notify();
                    }))),
            );
            body = match log {
                Load::Ready(text) => body.child(
                    div()
                        .id("check-log")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .mx_3()
                        .p_2()
                        .rounded(px(6.))
                        .bg(c(theme::PANEL))
                        .font_family(theme::MONO_FONT)
                        .text_xs()
                        .text_color(c(theme::TEXT_DIM))
                        .children(text.lines().map(|l| div().whitespace_nowrap().child(l.to_string()))),
                ),
                other => body.child(load_line(other)),
            };
        }

        // Create.
        if !self.create.is_empty() {
            body = body.child(ui::section(format!("CREATE PULL REQUESTS INTO {}", self.base)));
            for (ix, row) in self.create.iter().enumerate() {
                let include = row.include;
                body = body.child(
                    div()
                        .mx_3()
                        .my_1()
                        .p_2()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .child(
                                    ui::button(format!("create:{}:include", row.repo), if include { "✓" } else { " " }, Tone::Plain)
                                        .w(px(20.))
                                        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                            if let Some(r) = t.create.get_mut(ix) {
                                                r.include = !r.include;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(div().flex_1().text_color(c(theme::TEXT)).child(row.repo.clone()))
                                .child(
                                    ui::button(format!("create:{}:draft", row.repo), if row.drafting { "Drafting…" } else { "Draft with AI" }, Tone::Plain)
                                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.draft(ix, w, cx))),
                                ),
                        )
                        .when(include, |d| d.child(crate::workspace_ui::kit::input(&row.title)).child(crate::workspace_ui::kit::input(&row.body))),
                );
            }
            body = body.child(
                div().px_3().pt_2().flex().child(
                    ui::button("create-prs", if self.creating { "Creating…" } else { "Create pull requests" }, Tone::Primary)
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                            if !t.creating {
                                t.create_prs(w, cx)
                            }
                        })),
                ),
            );
        }
        div().size_full().flex().flex_col().child(bar).child(body)
    }
}

fn dim(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div().px_3().py_1().text_xs().text_color(c(theme::TEXT_FAINT)).child(text.into())
}

fn dim_inline(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(text.into())
}

fn load_line<T>(l: &Load<T>) -> gpui::Div {
    match l {
        Load::Failed(e) => div().px_3().py_1().text_xs().text_color(c(theme::DANGER)).child(e.clone()),
        _ => crate::motion::skeleton_table(3, 3),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles_come_from_the_branch() {
        assert_eq!(super::title_from_branch("feat/add-login_page"), "Add login page");
        assert_eq!(super::title_from_branch("fix"), "Fix");
        assert_eq!(super::title_from_branch(""), "");
    }
}
