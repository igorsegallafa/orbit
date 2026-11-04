//! Create pull requests (`PrsModal.tsx`): each repo's readiness on the left
//! (commits ahead, pushed, PR already open), the selected repo's title and
//! description on the right. The AI drafts one repo or every checked one at
//! once; PRs are created side by side with per-repo results and retries.
use super::Flow;
use crate::motion::skeleton;
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace_ui::icons::icon;
use crate::workspace_ui::kit::{self, Kind};
use crate::workspace_ui::{plural, spawn, PageTab};
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent, Rgba, SharedString, Window};
use gpui_component::input::{InputState, Textarea, TextareaState};
use orbit_core::prs::{PlanState, PrPlan, PrSpec};
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Drafting(Instant),
    Creating,
    Created { number: u64, url: String },
    Failed,
}

pub struct PrRow {
    pub plan: PrPlan,
    pub phase: Phase,
    pub error: Option<String>,
    pub checked: bool,
    /// The AI filled it in.
    pub drafted: bool,
    default_title: String,
    pub title: Entity<InputState>,
    pub body: Entity<TextareaState>,
}

impl PrRow {
    fn creatable(&self) -> bool {
        matches!(self.plan.state, PlanState::Ready | PlanState::NeedsPush)
    }

    /// Can still be created (not running, not done).
    fn pending(&self) -> bool {
        self.creatable() && !matches!(self.phase, Phase::Creating | Phase::Created { .. })
    }

    fn to_create(&self) -> bool {
        self.checked && self.pending()
    }

    /// Holds text that closing would lose.
    fn touched(&self, cx: &gpui::App) -> bool {
        !matches!(self.phase, Phase::Created { .. })
            && (self.drafted || self.title.read(cx).value().trim() != self.default_title || !self.body.read(cx).value().trim().is_empty())
    }
}

pub struct PrsFlow {
    pub workspace: String,
    pub base: String,
    card_title: Option<String>,
    /// Repo names for the list's placeholder while the plan loads.
    placeholder: Vec<String>,
    pub rows: Option<Vec<PrRow>>,
    pub error: Option<String>,
    pub selected: usize,
    pub as_draft: bool,
    pub creating: bool,
    focus: FocusHandle,
}

impl PrsFlow {
    pub fn dump(&self, out: &mut String, cx: &gpui::App) {
        let _ = writeln!(out, "  create prs ({} → {}, draft {}):", self.workspace, self.base, self.as_draft);
        for (ix, r) in self.rows.iter().flatten().enumerate() {
            let sel = if ix == self.selected { "*" } else { " " };
            let _ = writeln!(
                out,
                "   {sel}{} {:?} {} commits {} checked {} title {:?}{}",
                r.plan.repo,
                r.plan.state,
                if let Phase::Drafting(_) = r.phase { "Drafting".to_string() } else { format!("{:?}", r.phase) },
                r.plan.commits,
                r.checked,
                r.title.read(cx).value(),
                r.error.as_deref().map(|e| format!(" error {e:?}")).unwrap_or_default()
            );
        }
    }

    fn row_mut(&mut self, repo: &str) -> Option<&mut PrRow> {
        self.rows.as_mut()?.iter_mut().find(|r| r.plan.repo == repo)
    }
}

/// "3 PRs into main · from feat/x · 7 commits" for the repos about to be created.
pub fn summary(picked: &[&PrPlan], fallback_base: &str) -> String {
    if picked.is_empty() {
        return "No repository selected".into();
    }
    let uniq = |f: fn(&PrPlan) -> &str| {
        let mut v: Vec<&str> = picked.iter().map(|p| f(p)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (bases, branches) = (uniq(|p| &p.base), uniq(|p| &p.branch));
    let commits: usize = picked.iter().map(|p| p.commits).sum();
    let pushes = picked.iter().filter(|p| p.state == PlanState::NeedsPush).count();
    let mut s = format!(
        "{} into {} · from {} · {}",
        plural(picked.len(), "PR"),
        if bases.len() == 1 { bases[0] } else { fallback_base },
        if branches.len() == 1 { branches[0].to_string() } else { format!("{} branches", branches.len()) },
        plural(commits, "commit")
    );
    if pushes > 0 {
        let _ = write!(s, " · {pushes} to push first");
    }
    s
}

/// The status pill: label and color (None: muted).
fn pill(p: &PrPlan) -> (String, Option<u32>) {
    match p.state {
        PlanState::Ready => ("Ready".into(), Some(theme::OK)),
        PlanState::NeedsPush => ((if p.pushed { "Push + PR" } else { "Not pushed" }).into(), Some(theme::WARN)),
        PlanState::NoCommits => ("No commits".into(), None),
        PlanState::HasPr => (format!("PR #{} open ↗", p.pr.as_ref().map_or(0, |p| p.number)), Some(theme::INFO)),
        PlanState::Blocked => ("Blocked".into(), Some(theme::DANGER)),
    }
}

/// `.tag`: a small pill, tinted by `color`.
fn tag(label: impl Into<SharedString>, color: Option<u32>) -> gpui::Div {
    let d = div().flex_none().px(px(9.)).py(px(1.)).rounded_full().border_1().text_size(px(11.)).font_weight(FontWeight::MEDIUM).whitespace_nowrap();
    match color {
        Some(col) => d.text_color(c(col)).border_color(kit::alpha(c(col), 0.25)).bg(kit::alpha(c(col), 0.08)),
        None => d.text_color(c(theme::TEXT_FAINT)).border_color(c(theme::BORDER)).bg(c(theme::PANEL_RAISED)),
    }
    .child(label.into())
}

fn id(prefix: &str, repo: &str) -> gpui::ElementId {
    gpui::ElementId::Name(format!("{prefix}:{repo}").into())
}

impl PageTab {
    fn prs_flow(&mut self) -> Option<&mut PrsFlow> {
        match self.flow_mut() {
            Some(Flow::Prs(f)) => Some(f),
            _ => None,
        }
    }

    /// Opens the modal and checks every repo (`orbit_core::prs::plan`).
    pub fn open_create_prs(&mut self, workspace: String, base: String, card_title: Option<String>, repos: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.modal = Some(crate::workspace_ui::Modal::Flow(Flow::Prs(PrsFlow {
            workspace: workspace.clone(),
            base: base.clone(),
            card_title,
            placeholder: repos,
            rows: None,
            error: None,
            selected: 0,
            as_draft: false,
            creating: false,
            focus,
        })));
        spawn(window, cx, move || orbit_core::prs::plan(&workspace, &base), |t, res, window, cx| {
            let Some(f) = t.prs_flow() else { return };
            let plans = match res {
                Ok(p) => p,
                Err(e) => return f.error = Some(e),
            };
            let card = f.card_title.clone().filter(|s| !s.trim().is_empty());
            let rows: Vec<PrRow> = plans
                .into_iter()
                .map(|plan| {
                    let default_title = card.clone().unwrap_or_else(|| orbit_core::prs::title_from_branch(&plan.branch));
                    let t0 = default_title.clone();
                    let checked = matches!(plan.state, PlanState::Ready | PlanState::NeedsPush);
                    PrRow {
                        plan,
                        phase: Phase::Idle,
                        error: None,
                        checked,
                        drafted: false,
                        default_title,
                        title: cx.new(|cx| {
                            let mut s = InputState::new(window, cx).placeholder("PR title");
                            s.set_value(t0, window, cx);
                            s
                        }),
                        body: cx.new(|cx| TextareaState::new(window, cx).placeholder("What changed, why and how — short and objective.")),
                    }
                })
                .collect();
            f.selected = rows.iter().position(PrRow::creatable).unwrap_or(0);
            f.rows = Some(rows);
        });
        cx.notify();
    }

    fn prs_draft(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prs_flow() else { return };
        let ws = f.workspace.clone();
        let Some(r) = f.row_mut(&repo) else { return };
        r.phase = Phase::Drafting(Instant::now());
        r.error = None;
        let repo2 = repo.clone();
        spawn(window, cx, move || orbit_core::agent::pr_draft(&ws, &repo2).map_err(|e| e.to_string()), move |t, res, window, cx| {
            let Some(r) = t.prs_flow().and_then(|f| f.row_mut(&repo)) else { return };
            r.phase = Phase::Idle;
            match res {
                Ok(d) => {
                    r.drafted = true;
                    r.title.update(cx, |i, cx| i.set_value(d.title, window, cx));
                    r.body.update(cx, |i, cx| i.set_value(d.body, window, cx));
                }
                Err(e) => r.error = Some(format!("Draft failed: {e}")),
            }
        });
        cx.notify();
    }

    /// Drafts every checked repo the user hasn't written in, side by side.
    fn prs_draft_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repos: Vec<String> = match self.prs_flow() {
            Some(PrsFlow { rows: Some(rows), .. }) => rows.iter().filter(|r| r.to_create() && r.phase == Phase::Idle && !r.touched(cx)).map(|r| r.plan.repo.clone()).collect(),
            _ => return,
        };
        for repo in repos {
            self.prs_draft(repo, window, cx);
        }
    }

    fn prs_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prs_flow() else { return };
        let Some(rows) = f.rows.as_mut() else { return };
        let picked: Vec<&mut PrRow> = rows.iter_mut().filter(|r| r.to_create()).collect();
        let blocked = picked.iter().any(|r| matches!(r.phase, Phase::Drafting(_)) || r.title.read(cx).value().trim().is_empty());
        if f.creating || picked.is_empty() || blocked {
            return;
        }
        let draft = f.as_draft;
        let mut specs = Vec::new();
        for r in picked {
            specs.push(PrSpec { repo: r.plan.repo.clone(), title: r.title.read(cx).value().trim().to_string(), body: r.body.read(cx).value().to_string(), draft });
            r.phase = Phase::Creating;
            r.error = None;
        }
        f.creating = true;
        let (ws, base) = (f.workspace.clone(), f.base.clone());
        for spec in specs {
            let (ws, base, repo) = (ws.clone(), base.clone(), spec.repo.clone());
            spawn(window, cx, move || orbit_core::prs::create(&ws, &base, &spec), move |t, res, window, cx| {
                let Some(f) = t.prs_flow() else { return };
                if let Some(r) = f.row_mut(&repo) {
                    match res {
                        Ok(pr) => {
                            r.phase = Phase::Created { number: pr.number, url: pr.url };
                            r.checked = false;
                        }
                        Err(e) => {
                            // Past the push step: the branch is on origin now.
                            if !e.contains("push failed") && r.plan.state == PlanState::NeedsPush {
                                (r.plan.state, r.plan.unpushed, r.plan.pushed, r.plan.reason) = (PlanState::Ready, 0, true, String::new());
                            }
                            r.phase = Phase::Failed;
                            r.error = Some(e);
                        }
                    }
                }
                let rows = f.rows.as_deref().unwrap_or_default();
                if rows.iter().all(|r| r.phase != Phase::Creating) {
                    f.creating = false;
                    let (ok, failed) = (rows.iter().filter(|r| matches!(r.phase, Phase::Created { .. })).count(), rows.iter().filter(|r| r.phase == Phase::Failed).count());
                    if failed == 0 {
                        t.toast(ToastKind::Success, format!("Opened {}", plural(ok, "pull request")), None, window, cx);
                    }
                }
            });
        }
        cx.notify();
    }

    /// Closes, asking first when titles or descriptions would be lost.
    pub(crate) fn prs_request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prs_flow() else { return };
        if f.creating {
            return;
        }
        let rows = f.rows.as_deref().unwrap_or_default();
        let created = rows.iter().any(|r| matches!(r.phase, Phase::Created { .. }));
        if !rows.iter().any(|r| r.touched(cx)) {
            return self.prs_close(created, window, cx);
        }
        let page = cx.entity().downgrade();
        let discard = Rc::new(move |_: &mut crate::app::OrbitApp, window: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
            let _ = page.update(cx, |t, cx| t.prs_close(created, window, cx));
        });
        self.with_app(window, cx, move |a, w, cx| {
            a.open_dialog(
                Dialog {
                    title: "Discard PR drafts?".into(),
                    message: "The titles and descriptions you wrote or drafted with AI will be lost.".into(),
                    buttons: vec![
                        DialogButton { label: "Keep editing".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) },
                        DialogButton { label: "Discard".into(), style: ButtonStyle::Primary, run: discard },
                    ],
                    on_cancel: Rc::new(|_, _, _| {}),
                },
                w,
                cx,
            )
        });
    }

    fn prs_close(&mut self, created: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal(cx);
        if created {
            self.reload(true, window, cx);
        }
    }

    fn prs_select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.prs_flow() {
            let n = f.rows.as_ref().map_or(0, Vec::len);
            f.selected = ix.min(n.saturating_sub(1));
        }
        cx.notify();
    }

    fn prs_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let Some(f) = self.prs_flow() else { return };
        let editing = f.rows.as_ref().and_then(|r| r.get(f.selected)).is_some_and(|r| {
            r.title.focus_handle(cx).is_focused(window) || r.body.focus_handle(cx).is_focused(window)
        });
        let sel = f.selected;
        match k.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.prs_request_close(window, cx);
            }
            "enter" if k.modifiers.control || k.modifiers.platform => {
                cx.stop_propagation();
                self.prs_submit(window, cx);
            }
            "up" | "down" if k.modifiers.alt || !editing => {
                cx.stop_propagation();
                self.prs_select(if k.key == "up" { sel.saturating_sub(1) } else { sel + 1 }, cx);
            }
            _ => {}
        }
    }

    fn prs_open_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal(cx);
        self.reload(true, window, cx);
        self.open_prs_review(window, cx);
    }

    pub(super) fn render_prs(&self, f: &PrsFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: &[PrRow] = f.rows.as_deref().unwrap_or_default();
        let picked: Vec<&PrRow> = rows.iter().filter(|r| r.to_create()).collect();
        let drafting = rows.iter().filter(|r| matches!(r.phase, Phase::Drafting(_))).count();
        let draftable = rows.iter().any(|r| r.to_create() && r.phase == Phase::Idle && !r.touched(cx));
        let created = rows.iter().filter(|r| matches!(r.phase, Phase::Created { .. })).count();
        let waiting = picked.iter().any(|r| matches!(r.phase, Phase::Drafting(_)));
        let untitled = picked.iter().any(|r| r.title.read(cx).value().trim().is_empty());
        let retry = !picked.is_empty() && picked.iter().all(|r| r.phase == Phase::Failed);
        let any_open = rows.iter().any(|r| r.plan.state == PlanState::HasPr);
        let summary_text = match &f.rows {
            None => "Checking each repository…".to_string(),
            Some(_) => summary(&picked.iter().map(|r| &r.plan).collect::<Vec<_>>(), &f.base),
        };

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().flex_none().size(px(32.)).rounded(px(8.)).bg(c(theme::ACCENT_SOFT)).flex().items_center().justify_center().child(icon("pull-request", 16., c(theme::ACCENT))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Pull requests for this feature"))
                    .child(div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).truncate().child(summary_text)),
            )
            .child({
                let off = f.rows.is_none() || !draftable || f.creating;
                let label = if drafting > 0 { format!("Drafting {drafting}…") } else { "Draft all with AI".into() };
                let b = div()
                    .id("prs-draft-all")
                    .relative()
                    .flex_none()
                    .h(px(34.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(kit::alpha(c(theme::ACCENT), 0.35))
                    .bg(c(theme::PANEL_RAISED))
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .text_color(c(theme::ACCENT))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(if drafting > 0 {
                        kit::spinner("prs-draft-all-spin", 13., c(theme::ACCENT)).into_any_element()
                    } else {
                        icon("spark", 13., c(theme::ACCENT)).into_any_element()
                    })
                    .child(label)
                    .child(crate::probe::probe("prs-draft-all"));
                kit::hint(kit::disabled(b, off), "The AI reads each checked repo's PR template and diff and drafts its title and description. Repos you edited are kept.")
                    .when(!off, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prs_draft_all(w, cx))))
            });

        let mut list = div().id("prs-list").flex_none().w(px(330.)).h_full().overflow_y_scroll().p(px(8.)).bg(c(theme::BG)).border_r_1().border_color(c(theme::BORDER)).flex().flex_col().gap(px(2.));
        if f.rows.is_none() && f.error.is_none() {
            // No repos known yet: three unnamed rows, as React.
            let names: Vec<String> = if f.placeholder.is_empty() { vec![String::new(); 3] } else { f.placeholder.clone() };
            for (ix, name) in names.iter().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(8.))
                        .child(kit::spinner(gpui::ElementId::Name(format!("prs-load:{ix}").into()), 14., c(theme::ACCENT)))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(if name.is_empty() {
                                    skeleton(gpui::relative(0.6), 12.).into_any_element()
                                } else {
                                    div().font_family(theme::MONO_FONT).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(name.clone()).into_any_element()
                                })
                                .child(skeleton(gpui::relative(0.8), 9.)),
                        ),
                );
            }
        }
        for (ix, r) in rows.iter().enumerate() {
            let selected = ix == f.selected;
            let repo = r.plan.repo.clone();
            let can = r.pending() && !f.creating;
            let sub = div()
                .flex()
                .gap(px(4.))
                .min_w_0()
                .text_size(px(11.))
                .text_color(c(theme::TEXT_FAINT))
                .child(div().min_w_0().truncate().child(format!("{} → {}", r.plan.branch, r.plan.base)))
                .when(r.plan.commits > 0, |d| d.child(div().flex_none().whitespace_nowrap().child(format!("· {}", plural(r.plan.commits, "commit")))));
            let status = match &r.phase {
                Phase::Drafting(_) | Phase::Creating => div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .text_color(c(theme::ACCENT))
                    .child(kit::spinner(id("prs-spin", &repo), 12., c(theme::ACCENT)))
                    .child(if r.phase == Phase::Creating { "Creating" } else { "Drafting" })
                    .into_any_element(),
                Phase::Created { number, url } => {
                    let url = url.clone();
                    div().id(id("prs-link", &repo)).cursor_pointer().child(tag(format!("#{number} ↗"), Some(theme::OK))).on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()
                }
                Phase::Failed => tag("Failed", Some(theme::DANGER)).into_any_element(),
                Phase::Idle => {
                    let (label, color) = pill(&r.plan);
                    match r.plan.pr.clone() {
                        Some(pr) => div().id(id("prs-link", &repo)).cursor_pointer().child(tag(label, color)).on_click(move |_, _, cx| cx.open_url(&pr.url)).into_any_element(),
                        None => kit::hint(div().id(id("prs-pill", &repo)).child(tag(label, color)), if r.plan.reason.is_empty() { "A PR can be opened".to_string() } else { r.plan.reason.clone() }).into_any_element(),
                    }
                }
            };
            let r1 = repo.clone();
            list = list.child(
                div()
                    .id(id("prs-row", &repo))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if selected { kit::alpha(c(theme::ACCENT), 0.3) } else { gpui::transparent_black().into() })
                    .when(selected, |d| d.bg(c(theme::ACCENT_SOFT)))
                    .when(!selected, |d| d.hover(|s| s.bg(c(theme::PANEL_RAISED))))
                    .cursor_pointer()
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.prs_select(ix, cx)))
                    .child(
                        div()
                            .id(id("prs-include", &repo))
                            .relative()
                            .when(!can, |d| d.opacity(0.5))
                            .child(kit::check(r.checked, false))
                            .child(crate::probe::probe(format!("prs-include:{repo}")))
                            .when(can, |d| {
                                d.cursor_pointer().on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(r) = t.prs_flow().and_then(|f| f.row_mut(&r1)) {
                                        r.checked = !r.checked;
                                    }
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .when(!r.creatable(), |d| d.opacity(0.6))
                            .child(div().font_family(theme::MONO_FONT).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).truncate().child(repo.clone()))
                            .child(sub),
                    )
                    .child(status)
                    .child(crate::probe::probe(format!("prs-row:{repo}"))),
            );
        }
        if let Some(e) = &f.error {
            list = list.child(error_box(e.clone()));
        }

        let editor = div().id("prs-editor").flex_1().min_w_0().h_full().overflow_y_scroll().px(px(18.)).py(px(16.)).flex().flex_col().gap(px(8.));
        let editor = match rows.get(f.selected) {
            Some(r) => editor.child(self.render_prs_editor(r, f.creating, cx)),
            // `.prs-editor-skeleton`.
            None if f.rows.is_none() && f.error.is_none() => editor.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(skeleton(gpui::relative(0.4), 14.))
                    .child(skeleton(gpui::relative(1.), 34.))
                    .child(skeleton(gpui::relative(1.), 220.)),
            ),
            None => editor,
        };

        let n = picked.len();
        let primary = if f.rows.is_none() {
            kit::disabled(kit::button("prs-submit", "Checking repositories…", Kind::Primary, kit::NORMAL), true)
        } else if f.creating {
            kit::disabled(kit::button("prs-submit", "Creating…", Kind::Primary, kit::NORMAL), true)
        } else if (created > 0 && n == 0) || (n == 0 && any_open) {
            kit::button("prs-submit", "Open in review", Kind::Primary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prs_open_review(w, cx)))
        } else {
            let label = if retry { format!("Retry {n} failed") } else { format!("Create {}", plural(n, "pull request")) };
            let off = n == 0 || waiting || untitled;
            let b = kit::disabled(kit::button("prs-submit", label, Kind::Primary, kit::NORMAL), off).when(!off, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prs_submit(w, cx))));
            if waiting {
                kit::hint(b, "Waiting for the AI drafts")
            } else if untitled {
                kit::hint(b, "Every PR needs a title")
            } else {
                b
            }
        };
        let kbd = |k: &'static str| div().px(px(4.)).min_w(px(16.)).flex().justify_center().rounded(px(4.)).border_1().border_color(c(theme::BORDER_STRONG)).bg(c(theme::PANEL_RAISED)).text_size(px(10.5)).child(k);
        let footer = kit::modal_footer()
            .child(
                kit::hint(
                    div()
                        .id("prs-as-draft")
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.5))
                        .text_color(c(theme::TEXT_DIM))
                        .cursor_pointer()
                        .child(kit::check(f.as_draft, false))
                        .child("Open as draft")
                        .child(crate::probe::probe("prs-as-draft"))
                        .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            if let Some(f) = t.prs_flow().filter(|f| !f.creating) {
                                f.as_draft = !f.as_draft;
                            }
                            cx.notify();
                        })),
                    "gh pr create --draft: reviewers aren't asked yet",
                ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(3.))
                    .mr(px(6.))
                    .text_size(px(11.))
                    .text_color(c(theme::TEXT_FAINT))
                    .child(kbd("↑"))
                    .child(kbd("↓"))
                    .child(" repos ·  ")
                    .child(kbd("Ctrl"))
                    .child("+")
                    .child(kbd("Enter"))
                    .child(" create"),
            )
            .child(
                kit::disabled(kit::button("modal-cancel", if created > 0 && n == 0 { "Close" } else { "Cancel" }, Kind::Secondary, kit::NORMAL), f.creating)
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prs_request_close(w, cx))),
            )
            .child(primary);

        kit::modal("modal-card", 920.)
            .h(px(660.))
            .track_focus(&f.focus)
            .on_key_down(cx.listener(|t, ev: &KeyDownEvent, w, cx| t.prs_key(ev, w, cx)))
            .text_size(px(13.))
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(list).child(editor))
            .child(footer)
    }

    fn render_prs_editor(&self, r: &PrRow, creating: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &r.plan;
        let mut head = div()
            .flex()
            .items_baseline()
            .gap(px(8.))
            .mb(px(4.))
            .child(div().font_family(theme::MONO_FONT).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(p.repo.clone()))
            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(format!("{} → {}", p.branch, p.base)));
        if p.commits > 0 {
            head = head.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(format!("· {}", plural(p.commits, "commit"))));
        }
        let out = div().flex_1().flex().flex_col().gap(px(8.)).child(head);

        if let Phase::Created { number, url } = &r.phase {
            let url = url.clone();
            return out.child(
                notice(Some(theme::OK), format!("Opened PR #{number}"), r.title.read(cx).value().to_string())
                    .child(kit::link("prs-open-created", "Open on GitHub ↗").ml(px(-8.)).on_click(move |_, _, cx| cx.open_url(&url))),
            );
        }
        if !r.creatable() {
            let title = match p.state {
                PlanState::HasPr => "Already has a pull request",
                PlanState::NoCommits => "Nothing to open a PR for",
                _ => "Can't open a PR",
            };
            let mut n = notice(if p.state == PlanState::HasPr { Some(theme::INFO) } else { None }, title, p.reason.clone());
            if let Some(pr) = p.pr.clone() {
                let label = format!("Open PR #{}{} ↗", pr.number, if pr.is_draft { " (draft)" } else { "" });
                n = n.child(kit::link("prs-open-existing", label).ml(px(-8.)).on_click(move |_, _, cx| cx.open_url(&pr.url)));
            }
            return out.child(n);
        }

        let busy = creating || matches!(r.phase, Phase::Drafting(_) | Phase::Creating);
        let repo = p.repo.clone();
        let label = |t: &'static str| div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child(t);
        out.when(p.state == PlanState::NeedsPush, |d| d.child(div().text_size(px(12.)).text_color(c(theme::WARN)).child(format!("{}.", p.reason))))
            .when_some(r.error.clone(), |d, e| d.child(error_box(e)))
            .child(label("Title"))
            .child(div().id("prs-title").relative().child(kit::input(&r.title).disabled(busy)).child(crate::probe::probe("prs-title")))
            .child(
                div()
                    .mt(px(6.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().flex().gap(px(6.)).child(label("Description")).child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Markdown")))
                    .child(
                        kit::hint(
                            kit::disabled(
                                kit::button_icon(format!("prs-draft:{repo}"), Some(("spark", 11.)), if r.drafted { "Redraft with AI" } else { "Draft with AI" }, Kind::Secondary, kit::MINI),
                                busy,
                            ),
                            "The AI reads the repo's PR template and the diff, and drafts the title and description",
                        )
                        .when(!busy, |b| b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.prs_draft(repo.clone(), w, cx)))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(200.))
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.5))
                    .child(Textarea::new(&r.body).h_full().disabled(busy))
                    .when_some(if let Phase::Drafting(t0) = r.phase { Some(t0.elapsed().as_secs()) } else { None }, |d, secs| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(px(6.))
                                .bg(kit::alpha(c(theme::BG), 0.85))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(px(8.))
                                .font_family(theme::UI_FONT)
                                .text_color(c(theme::TEXT_DIM))
                                .child(kit::spinner("prs-drafting", 18., c(theme::ACCENT)))
                                .child(format!("AI is reading the template and the diff… {secs}s"))
                                .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child("You can edit the other repos meanwhile.")),
                        )
                    }),
            )
    }
}

fn notice(color: Option<u32>, title: impl Into<SharedString>, text: impl Into<SharedString>) -> gpui::Div {
    let (border, bg): (Rgba, Rgba) = match color {
        Some(col) => (kit::alpha(c(col), 0.3), kit::alpha(c(col), 0.07)),
        None => (c(theme::BORDER), c(theme::PANEL_RAISED)),
    };
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(6.))
        .p(px(14.))
        .rounded(px(8.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(px(12.5))
        .text_color(c(theme::TEXT_DIM))
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(title.into()))
        .child(text.into())
}

fn error_box(e: String) -> gpui::Div {
    div()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(kit::alpha(c(theme::DANGER), 0.3))
        .bg(c(theme::DANGER_SOFT))
        .text_size(px(12.))
        .text_color(c(theme::DANGER))
        .child(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(repo: &str, state: PlanState, commits: usize) -> PrPlan {
        PrPlan { repo: repo.into(), branch: "feat/x".into(), base: "main".into(), commits, unpushed: 0, pushed: true, state, reason: String::new(), pr: None }
    }

    #[test]
    fn summarizes_the_repos_to_create() {
        let (a, b) = (plan("a", PlanState::Ready, 2), plan("b", PlanState::NeedsPush, 1));
        assert_eq!(summary(&[&a, &b], "main"), "2 PRs into main · from feat/x · 3 commits · 1 to push first");
        assert_eq!(summary(&[&a], "main"), "1 PR into main · from feat/x · 2 commits");
        assert_eq!(summary(&[], "main"), "No repository selected");
        assert_eq!(pill(&plan("c", PlanState::NoCommits, 0)), ("No commits".to_string(), None));
    }
}
