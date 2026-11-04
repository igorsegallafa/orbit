//! Pull requests for the feature (`PrsModal.tsx`): repos that already have a
//! PR link to it; the others get a title and description (drafted by the AI
//! from the repo's PR template on request) and are created together.
use super::{cancel_button, card, footer, Flow};
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace_ui::kit::{self, Kind};
use crate::workspace_ui::{spawn, PageTab};
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, FontWeight, Window};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use orbit_core::prs::PrSpec;
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum PrState {
    Checking,
    HasPr(String),
    Ready,
    Nothing,
    Drafting,
    Creating,
    Failed(String),
}

pub struct PrRow {
    pub repo: String,
    pub state: PrState,
    pub included: bool,
    pub title: Entity<InputState>,
    pub body: Entity<TextareaState>,
}

pub struct PrsFlow {
    pub workspace: String,
    pub base: String,
    pub rows: Vec<PrRow>,
    pub creating: bool,
}

impl PrsFlow {
    pub fn dump(&self, out: &mut String, cx: &gpui::App) {
        let _ = writeln!(out, "  create prs ({} → {}):", self.workspace, self.base);
        for r in &self.rows {
            let _ = writeln!(out, "    {} {:?} included {} title {:?}", r.repo, r.state, r.included, r.title.read(cx).value());
        }
    }

    fn row_mut(&mut self, repo: &str) -> Option<&mut PrRow> {
        self.rows.iter_mut().find(|r| r.repo == repo)
    }
}

impl PageTab {
    fn prs_flow(&mut self) -> Option<&mut PrsFlow> {
        match self.flow_mut() {
            Some(Flow::Prs(f)) => Some(f),
            _ => None,
        }
    }

    /// `repos`: (repo, commits of its own on origin). Repos without any have nothing to PR.
    pub fn open_create_prs(&mut self, workspace: String, base: String, title: String, repos: Vec<(String, usize)>, window: &mut Window, cx: &mut Context<Self>) {
        let rows = repos
            .iter()
            .map(|(repo, commits)| PrRow {
                repo: repo.clone(),
                state: if *commits == 0 { PrState::Nothing } else { PrState::Checking },
                included: *commits > 0,
                title: cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder("PR title");
                    s.set_value(title.clone(), window, cx);
                    s
                }),
                body: cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 10).placeholder("Description — what, why and how (short, objective)")),
            })
            .collect();
        self.modal = Some(crate::workspace_ui::Modal::Flow(Flow::Prs(PrsFlow { workspace: workspace.clone(), base, rows, creating: false })));
        spawn(window, cx, move || orbit_core::prs::list(&workspace), |t, res, _, _| {
            let Some(f) = t.prs_flow() else { return };
            let prs = res.unwrap_or_default();
            for r in f.rows.iter_mut().filter(|r| r.state == PrState::Checking) {
                match prs.iter().find(|p| p.repo == r.repo) {
                    Some(p) => {
                        r.state = PrState::HasPr(p.url.clone());
                        r.included = false;
                    }
                    None => r.state = PrState::Ready,
                }
            }
        });
        cx.notify();
    }

    fn prs_draft(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prs_flow() else { return };
        let ws = f.workspace.clone();
        if let Some(r) = f.row_mut(&repo) {
            r.state = PrState::Drafting;
            r.included = true;
        }
        spawn(window, cx, move || orbit_core::agent::pr_draft(&ws, &repo).map(|d| (repo, d)).map_err(|e| e.to_string()), |t, res, window, cx| {
            let Some(f) = t.prs_flow() else { return };
            match res {
                Ok((repo, d)) => {
                    if let Some(r) = f.row_mut(&repo) {
                        r.state = PrState::Ready;
                        r.title.update(cx, |i, cx| i.set_value(d.title, window, cx));
                        r.body.update(cx, |i, cx| i.set_value(d.body, window, cx));
                    }
                }
                Err(e) => {
                    for r in f.rows.iter_mut().filter(|r| r.state == PrState::Drafting) {
                        r.state = PrState::Failed(format!("draft failed: {e}"));
                    }
                }
            }
        });
        cx.notify();
    }

    fn prs_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prs_flow() else { return };
        let specs: Vec<PrSpec> = f
            .rows
            .iter()
            .filter(|r| r.included && matches!(r.state, PrState::Ready | PrState::Failed(_)))
            .map(|r| PrSpec { repo: r.repo.clone(), title: r.title.read(cx).value().trim().to_string(), body: r.body.read(cx).value().to_string() })
            .collect();
        if specs.is_empty() || f.creating {
            return;
        }
        f.creating = true;
        for r in f.rows.iter_mut().filter(|r| specs.iter().any(|s| s.repo == r.repo)) {
            r.state = PrState::Creating;
        }
        let (ws, base) = (f.workspace.clone(), f.base.clone());
        spawn(window, cx, move || orbit_core::prs::create_all(&ws, &base, &specs), |t, res, window, cx| match res {
            Ok(prs) => {
                t.close_modal(cx);
                t.toast(ToastKind::Success, format!("Opened {}", crate::workspace_ui::plural(prs.len(), "pull request")), None, window, cx);
                t.reload(true, window, cx);
                t.open_prs_review(window, cx);
            }
            Err(e) => {
                if let Some(f) = t.prs_flow() {
                    f.creating = false;
                    for r in f.rows.iter_mut().filter(|r| r.state == PrState::Creating) {
                        r.state = PrState::Failed(e.clone());
                    }
                }
            }
        });
        cx.notify();
    }

    pub(super) fn render_prs(&self, f: &PrsFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let checking = f.rows.iter().any(|r| r.state == PrState::Checking);
        let drafting = f.rows.iter().any(|r| r.state == PrState::Drafting);
        let targets: Vec<&PrRow> = f.rows.iter().filter(|r| matches!(r.state, PrState::Ready | PrState::Drafting | PrState::Creating | PrState::Failed(_))).collect();
        let existing: Vec<&PrRow> = f.rows.iter().filter(|r| matches!(r.state, PrState::HasPr(_))).collect();
        let n = targets.iter().filter(|r| r.included).count();
        let mut body = div().flex().flex_col().gap(px(10.));
        for r in &existing {
            let PrState::HasPr(url) = &r.state else { continue };
            let url = url.clone();
            body = body.child(
                div()
                    .id(gpui::ElementId::Name(format!("prs-existing:{}", r.repo).into()))
                    .flex()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .child(div().font_family(theme::MONO_FONT).child(r.repo.clone()))
                    .child("— PR already open ↗")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        if !checking && !drafting && targets.is_empty() && existing.is_empty() {
            body = body.child(div().text_size(px(12.5)).text_color(c(theme::TEXT_FAINT)).child(if f.rows.iter().any(|r| r.state == PrState::Nothing) {
                "Nothing to PR — the feature branches have no commits beyond the base."
            } else {
                "Every repo already has its pull request."
            }));
        }
        for r in f.rows.iter().filter(|r| r.state == PrState::Checking || targets.iter().any(|t| t.repo == r.repo)) {
            let repo = r.repo.clone();
            let (r1, r2) = (repo.clone(), repo.clone());
            let state = match &r.state {
                PrState::Checking => Some("looking for PRs…".to_string()),
                PrState::Drafting => Some("AI is reading the template and the diff…".to_string()),
                PrState::Creating => Some("creating PR…".to_string()),
                _ => None,
            };
            let busy = matches!(r.state, PrState::Drafting | PrState::Creating | PrState::Checking) || f.creating;
            body = body.child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::PANEL_RAISED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .id(gpui::ElementId::Name(format!("prs-include:{repo}").into()))
                                    .relative()
                                    .cursor_pointer()
                                    .child(kit::check(r.included, false))
                                    .child(crate::probe::probe(format!("prs-include:{repo}")))
                                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                        if let Some(r) = t.prs_flow().and_then(|f| f.row_mut(&r1)) {
                                            r.included = !r.included;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(div().font_family(theme::MONO_FONT).text_size(px(13.)).child(repo.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(11.5))
                                    .text_color(c(theme::TEXT_FAINT))
                                    .when_some(state, |d, s| d.child(kit::spinner(gpui::ElementId::Name(format!("prs-spin:{repo}").into()), 12., c(theme::ACCENT))).child(s)),
                            )
                            .when(r.state != PrState::Checking, |d| {
                                d.child(
                                    kit::hint(
                                        kit::disabled(kit::button_icon(format!("prs-draft:{repo}"), Some(("spark", 11.)), "Draft with AI", Kind::Primary, kit::MINI), busy),
                                        "AI reads the repo's PR template and the diff, drafts title + description",
                                    )
                                    .when(!busy, |b| b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.prs_draft(r2.clone(), w, cx)))),
                                )
                            }),
                    )
                    .when(r.state != PrState::Checking, |d| {
                        d.child(div().id(gpui::ElementId::Name(format!("prs-title:{repo}").into())).relative().child(Input::new(&r.title)).child(crate::probe::probe(format!("prs-title:{repo}"))))
                            .child(div().font_family(theme::MONO_FONT).child(Textarea::new(&r.body)))
                    })
                    .when_some(if let PrState::Failed(e) = &r.state { Some(e.clone()) } else { None }, |d, e| d.child(div().text_size(px(12.)).text_color(c(theme::DANGER)).child(e))),
            );
        }
        let primary = if checking {
            kit::disabled(kit::button("prs-submit", "Checking PRs…", Kind::Secondary, kit::NORMAL), true)
        } else if drafting {
            kit::disabled(kit::button("prs-submit", "AI is drafting…", Kind::Secondary, kit::NORMAL), true)
        } else if !targets.is_empty() {
            kit::disabled(
                kit::button("prs-submit", if f.creating { "Creating…".to_string() } else { format!("Create {}", crate::workspace_ui::plural(n, "PR")) }, Kind::Primary, kit::NORMAL),
                f.creating || n == 0,
            )
            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prs_submit(w, cx)))
        } else {
            kit::button("prs-submit", "Review PRs", Kind::Primary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                t.close_modal(cx);
                t.open_prs_review(w, cx)
            }))
        };
        card(500., "Pull requests for this feature", None)
            .text_size(px(13.))
            .child(body.font_weight(FontWeight::NORMAL))
            .child(footer().child(cancel_button("Close", cx)).child(primary))
    }
}
