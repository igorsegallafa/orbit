//! New PRD (`RalphPrdModal.tsx`): a brief, an optional clarifying interview
//! in rounds, the decisions, then the agent writes `tasks/prd-<name>.md` and
//! `scripts/ralph/prd.json` (the `prd` and `ralph` skills, in one step).
use super::super::kit::{self as pkit, Kind};
use super::super::ui;
use super::{events, watch, Modal, RalphPage};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace_ui::kit;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Focusable, FontWeight, Window};
use gpui_component::input::{Textarea, TextareaState};
use orbit_core::agent::GrillRound;
use orbit_core::ralph;
use std::fmt::Write as _;

const LOG_MAX: usize = 400;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Brief,
    Interview,
    Summary,
    Writing,
}

pub struct PrdForm {
    pub phase: Phase,
    brief: Entity<TextareaState>,
    round: Option<GrillRound>,
    current: usize,
    answer: Entity<TextareaState>,
    /// (question, answer), every round.
    answers: Vec<(String, String)>,
    rounds_done: usize,
    loading: bool,
    summary: Entity<TextareaState>,
    pub log: Vec<String>,
}

impl PrdForm {
    pub fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_MAX {
            self.log.drain(..self.log.len() - LOG_MAX);
        }
    }

    fn answers_text(&self) -> String {
        self.answers.iter().map(|(q, a)| format!("- {q}\n  → {a}")).collect::<Vec<_>>().join("\n")
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(
            out,
            "  prd form: {:?} · round {} · question {} · answers {} · loading {} · brief {:?}",
            self.phase,
            self.rounds_done,
            self.current + 1,
            self.answers.len(),
            self.loading,
            self.brief.read(cx).value().to_string()
        );
        if let Some(q) = self.round.as_ref().and_then(|r| r.questions.get(self.current)) {
            let opts: Vec<String> = q.options.iter().map(|o| format!("{}{}", if o.recommended { "★" } else { "" }, o.label)).collect();
            let _ = writeln!(out, "    question {:?} options {} · answer {:?}", q.text, opts.join(" | "), self.answer.read(cx).value().to_string());
        }
        if self.phase == Phase::Summary {
            let _ = writeln!(out, "    summary {:?}", self.summary.read(cx).value().to_string());
        }
        for l in self.log.iter().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
            let _ = writeln!(out, "    log {l}");
        }
    }
}

impl RalphPage {
    fn prd_form(&mut self) -> Option<&mut PrdForm> {
        match &mut self.modal {
            Some(Modal::Prd(f)) => Some(f),
            _ => None,
        }
    }

    pub(super) fn open_prd_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let brief = cx.new(|cx| TextareaState::new(window, cx).auto_grow(7, 12).placeholder("What should be built, for whom, and why?"));
        let answer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 8));
        let summary = cx.new(|cx| TextareaState::new(window, cx).auto_grow(7, 14));
        watch(&brief, window, cx);
        watch(&answer, window, cx);
        let handle = brief.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.modal = Some(Modal::Prd(Box::new(PrdForm {
            phase: Phase::Brief,
            brief,
            round: None,
            current: 0,
            answer,
            answers: vec![],
            rounds_done: 0,
            loading: false,
            summary,
            log: vec![],
        })));
        cx.notify();
    }

    /// One interview round; `finish` asks the agent to wrap up now ("I'm ready").
    fn prd_ask(&mut self, finish: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (scope, repo) = (self.scope.clone(), self.repo.clone());
        let Some(f) = self.prd_form() else { return };
        f.loading = true;
        let (brief, answers, rounds) = (f.brief.read(cx).value().to_string(), f.answers.clone(), f.rounds_done);
        let max = finish.then_some(rounds + 1);
        cx.notify();
        ui::bg(
            cx,
            move || ralph::interview(scope, repo, brief, answers, rounds, max),
            move |t: &mut Self, r, w, cx| {
                let host = t.host.clone();
                let Some(f) = t.prd_form() else { return };
                f.loading = false;
                match r {
                    Ok(round) if round.done || round.questions.is_empty() => {
                        let text = if round.summary.trim().is_empty() { f.answers_text() } else { round.summary.clone() };
                        f.summary.update(cx, |s, cx| s.set_value(text, w, cx));
                        f.phase = Phase::Summary;
                    }
                    Ok(round) => {
                        f.rounds_done += 1;
                        let pick = round.questions[0].options.iter().find(|o| o.recommended).map(|o| o.label.clone()).unwrap_or_default();
                        f.round = Some(round);
                        f.current = 0;
                        f.answer.update(cx, |s, cx| s.set_value(pick, w, cx));
                        f.phase = Phase::Interview;
                    }
                    Err(e) => {
                        host.error("The interview failed", e, cx);
                        if finish {
                            let text = f.answers_text();
                            f.summary.update(cx, |s, cx| s.set_value(text, w, cx));
                            f.phase = Phase::Summary;
                        }
                    }
                }
            },
            window,
        );
    }

    /// Records the answer; the next question, or the next round.
    fn prd_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.prd_form() else { return };
        let value = f.answer.read(cx).value().trim().to_string();
        let Some(q) = f.round.as_ref().and_then(|r| r.questions.get(f.current)).cloned() else { return };
        if value.is_empty() {
            return;
        }
        f.answers.push((q.text, value));
        let count = f.round.as_ref().map_or(0, |r| r.questions.len());
        if f.current + 1 < count {
            f.current += 1;
            let pick = f.round.as_ref().and_then(|r| r.questions[f.current].options.iter().find(|o| o.recommended)).map(|o| o.label.clone()).unwrap_or_default();
            f.answer.update(cx, |s, cx| s.set_value(pick, window, cx));
            cx.notify();
        } else {
            self.prd_ask(false, window, cx);
        }
    }

    /// The agent writes the PRD (streams `ralph-prd-progress` into the log).
    fn prd_generate(&mut self, with_decisions: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (scope, repo) = (self.scope.clone(), self.repo.clone());
        let Some(f) = self.prd_form() else { return };
        let brief = f.brief.read(cx).value().to_string();
        let decisions = if with_decisions { f.summary.read(cx).value().to_string() } else { String::new() };
        f.phase = Phase::Writing;
        f.log.clear();
        cx.notify();
        let repo_name = repo.clone();
        ui::bg(
            cx,
            move || ralph::generate_prd(events(), scope, repo, brief, decisions),
            move |t: &mut Self, r, w, cx| match r {
                Ok(prd) => {
                    t.modal = None;
                    let n = prd.user_stories.len();
                    t.host.toast(ToastKind::Success, "PRD ready", Some(format!("{n} stories for {repo_name}. Review them, then start Ralph.")), cx);
                    t.load(false, w, cx);
                }
                Err(e) => {
                    t.host.error("Couldn't write the PRD", e, cx);
                    if let Some(f) = t.prd_form() {
                        let has_summary = !f.summary.read(cx).value().trim().is_empty();
                        f.phase = if has_summary { Phase::Summary } else { Phase::Brief };
                    }
                }
            },
            window,
        );
    }

    /// Esc: closes the open modal unless the agent is working in it.
    pub(super) fn escape_modal(&mut self, cx: &mut Context<Self>) -> bool {
        let busy = match &self.modal {
            None => return false,
            Some(Modal::Prd(f)) => f.loading || f.phase == Phase::Writing,
            Some(Modal::Run(_)) => false,
        };
        if !busy {
            self.modal = None;
            cx.notify();
        }
        true
    }

    fn prd_cancel_generate(&mut self, cx: &mut Context<Self>) {
        let _ = ralph::cancel_generate(self.scope.clone(), self.repo.clone());
        cx.notify();
    }

    pub(super) fn render_prd_form(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(Modal::Prd(f)) = &self.modal else { return div().into_any_element() };
        let intro = |text: String| div().mb(px(12.)).text_size(px(13.)).line_height(px(19.5)).text_color(c(theme::TEXT_DIM)).child(text);
        let actions = || div().mt(px(14.)).flex().items_center().justify_end().gap(px(8.));
        let cancel = |cx: &mut Context<Self>| {
            pkit::button("prd-cancel", "Cancel", Kind::DangerOutline).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.modal = None;
                cx.notify();
            }))
        };
        let loading = |text: &'static str, id: &'static str| {
            div().py(px(40.)).flex().items_center().justify_center().gap(px(10.)).text_size(px(13.)).text_color(c(theme::TEXT_FAINT)).child(kit::spinner(id, 14., c(theme::ACCENT))).child(text)
        };
        let body: gpui::AnyElement = match f.phase {
            Phase::Brief => {
                let empty = f.brief.read(cx).value().trim().is_empty();
                let off = empty || f.loading;
                div()
                    .flex()
                    .flex_col()
                    .child(intro(format!(
                        "Describe the feature. The agent can interview you first to settle scope, then writes tasks/prd-{}.md and scripts/ralph/prd.json with small, ordered user stories.",
                        self.scope.trim_start_matches('@')
                    )))
                    .child(div().id("prd-brief").relative().child(Textarea::new(&f.brief)).child(probe("prd-brief")))
                    .child(
                        actions()
                            .child(pkit::button("prd-close", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                t.modal = None;
                                cx.notify();
                            })))
                            .child(pkit::disabled(pkit::button("prd-write-now", "Write PRD now", Kind::Secondary), off).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                if !off {
                                    t.prd_generate(false, w, cx)
                                }
                            })))
                            .child(pkit::disabled(pkit::button_busy("prd-interview", if f.loading { "Thinking…" } else { "Interview me first" }, Kind::Primary, f.loading), off).on_click(cx.listener(
                                move |t, _: &ClickEvent, w, cx| {
                                    if !off {
                                        t.prd_ask(false, w, cx)
                                    }
                                },
                            ))),
                    )
                    .into_any_element()
            }
            Phase::Interview if f.loading => loading("Thinking of the next questions…", "prd-thinking").into_any_element(),
            Phase::Interview => {
                let Some(round) = &f.round else { return div().into_any_element() };
                let q = &round.questions[f.current];
                let picked = f.answer.read(cx).value().to_string();
                let desc = q.options.iter().find(|o| o.label == picked).or_else(|| q.options.iter().find(|o| o.recommended)).map(|o| o.description.clone()).filter(|d| !d.is_empty());
                let last = f.current + 1 >= round.questions.len();
                let empty = picked.trim().is_empty();
                let no_answers = f.answers.is_empty();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(10.))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::ACCENT))
                            .child(format!("ROUND {} · QUESTION {} OF {}", f.rounds_done, f.current + 1, round.questions.len())),
                    )
                    .child(div().mb(px(18.)).text_size(px(15.)).line_height(px(22.5)).font_weight(FontWeight(550.)).child(q.text.clone()))
                    .child(div().flex().flex_wrap().gap(px(8.)).children(q.options.iter().enumerate().map(|(i, o)| {
                        let label = o.label.clone();
                        let on = picked == o.label;
                        div()
                            .id(gpui::ElementId::Name(format!("prd-opt:{i}").into()))
                            .relative()
                            .px(px(12.))
                            .py(px(4.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .border_1()
                            .border_color(if o.recommended || on { c(theme::ACCENT) } else { c(theme::BORDER_STRONG) })
                            .bg(if on { pkit::alpha(theme::ACCENT, 0.28) } else if o.recommended { theme::accent_soft() } else { c(theme::PANEL_RAISED) })
                            .text_size(px(12.5))
                            .text_color(if o.recommended { pkit::mix(theme::INFO, theme::TEXT, 0.4) } else { c(theme::TEXT) })
                            .cursor_pointer()
                            .when(o.recommended, |d| d.child(div().mr(px(5.)).text_size(px(10.)).text_color(c(theme::ACCENT)).child("★")))
                            .child(o.label.clone())
                            .child(probe(format!("prd-opt:{i}")))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                let label = label.clone();
                                if let Some(f) = t.prd_form() {
                                    f.answer.update(cx, |s, cx| s.set_value(label, w, cx));
                                }
                                cx.notify();
                            }))
                    })))
                    .children(desc.map(|d| div().mt(px(6.)).text_size(px(11.5)).line_height(px(17.)).text_color(c(theme::TEXT_FAINT)).child(d)))
                    .child(div().id("prd-answer").relative().mt(px(16.)).child(Textarea::new(&f.answer)).child(probe("prd-answer")))
                    .child(
                        actions()
                            .child(div().mr_auto().child(cancel(cx)))
                            .child(pkit::disabled(pkit::button("prd-ready", "I'm ready", Kind::Secondary), no_answers).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                if !no_answers {
                                    t.prd_ask(true, w, cx)
                                }
                            })))
                            .child(pkit::disabled(pkit::button("prd-next", if last { "Submit round" } else { "Next question" }, Kind::Primary), empty).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prd_submit(w, cx)))),
                    )
                    .into_any_element()
            }
            Phase::Summary => div()
                .flex()
                .flex_col()
                .child(intro("Decisions (editable). Confirm to write the PRD.".into()))
                .child(div().id("prd-summary").relative().child(Textarea::new(&f.summary)).child(probe("prd-summary")))
                .child(
                    actions().child(div().mr_auto().child(cancel(cx))).child(pkit::button("prd-write", "Write PRD", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.prd_generate(true, w, cx)))),
                )
                .into_any_element(),
            Phase::Writing => div()
                .flex()
                .flex_col()
                .child(
                    loading("Writing the PRD and splitting it into stories…", "prd-writing")
                        .child(pkit::mini("prd-gen-cancel", "Cancel", Kind::DangerOutline).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.prd_cancel_generate(cx)))),
                )
                .when(!f.log.is_empty(), |d| {
                    d.child(
                        div()
                            .id("prd-log")
                            .mt(px(14.))
                            .h(px(220.))
                            .overflow_y_scroll()
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .bg(c(theme::BG))
                            .font_family(theme::MONO_FONT)
                            .text_size(px(11.5))
                            .line_height(px(16.))
                            .text_color(c(theme::TEXT_DIM))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(f.log.iter().map(|l| div().child(l.clone()))),
                    )
                })
                .into_any_element(),
        };
        kit::modal("ralph-prd-modal", 560.)
            .child(kit::modal_header(format!("New PRD · {}", self.repo)))
            .child(div().id("prd-body").min_h(px(240.)).overflow_y_scroll().px(px(16.)).py(px(18.)).text_size(px(13.)).child(body))
            .into_any_element()
    }
}
