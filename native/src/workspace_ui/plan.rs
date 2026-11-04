//! The workspace's plan: the "Plan · progress" card (`PlanProgress.tsx`,
//! PLAN.md's checklist, writable) and the Plan / Replan modal
//! (`PlanModal.tsx`: interview, draft, plan in a terminal, write it myself).
use super::kit::{self, Kind};
use super::workspace_page::WsPage;
use super::{spawn, Modal, PageTab};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, Focusable, FontWeight, Window};
use gpui_component::input::{Textarea, TextareaState};
use orbit_core::agent::plan::{self as core_plan, Answer};
use orbit_core::agent::GrillQuestion;
use orbit_core::workspace::CardRef;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

impl PageTab {
    /// Re-reads PLAN.md's tasks (and whether it exists).
    pub fn load_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.ws_name() else { return };
        spawn(
            window,
            cx,
            move || {
                let exists = orbit_core::workspace::ws_dir(&name).map(|d| d.join(orbit_core::agent::PLAN_FILE).exists()).unwrap_or(false);
                (exists, orbit_core::agent::plan_tasks(&name))
            },
            |t, (exists, tasks), _, _| {
                if let Some(p) = t.ws_page() {
                    p.plan_exists = Some(exists);
                    if let Ok(tasks) = tasks {
                        p.plan = Some(tasks);
                    }
                }
            },
        );
    }

    /// Ticks a task in PLAN.md (optimistic; re-read on failure).
    fn toggle_task(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let Some(task) = p.plan.as_mut().and_then(|t| t.get_mut(index)) else { return };
        task.done = !task.done;
        let (name, done) = (p.name.clone(), task.done);
        cx.notify();
        spawn(window, cx, move || orbit_core::agent::set_plan_task(&name, index, done), |t, res, window, cx| {
            if let Err(e) = res {
                t.toast(ToastKind::Error, "Couldn't update PLAN.md", Some(e), window, cx);
                t.load_plan(window, cx);
            }
        });
    }

    pub fn open_plan_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.ws_page() else { return };
        let (name, exists) = (p.name.clone(), p.plan_exists == Some(true));
        let card = p.meta.as_ref().and_then(|m| m.card.clone());
        let placeholder = if card.is_some() {
            "Optional: approach, constraints, what to skip… (the card is the starting point)"
        } else {
            "Describe the feature: what it should do, for whom, constraints, what to leave out…"
        };
        let goal = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 8).placeholder(placeholder));
        let answer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 8).placeholder("Your answer: pick an option above or write your own"));
        let summary = cx.new(|cx| TextareaState::new(window, cx).auto_grow(12, 16));
        let handle = goal.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.modal = Some(Modal::Plan(Box::new(PlanForm {
            ws: name,
            card,
            exists,
            goal,
            answer,
            summary,
            phase: Phase::Setup,
            answers: vec![],
            unsent: vec![],
            session: None,
            round: None,
            rounds: 0,
            run: None,
            feed: Arc::new(Mutex::new(vec![])),
            started: Instant::now(),
            error: None,
        })));
        cx.notify();
    }

    fn plan_form(&mut self) -> Option<&mut PlanForm> {
        match &mut self.modal {
            Some(Modal::Plan(f)) => Some(f),
            _ => None,
        }
    }

    /// A new agent run: its id and an event sink feeding the modal.
    fn plan_begin(&mut self, phase: Phase, window: &mut Window, cx: &mut Context<Self>) -> Option<(String, orbit_core::events::Events)> {
        let f = self.plan_form()?;
        let run = format!("plan:{}:{}", f.ws, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
        f.run = Some(run.clone());
        f.phase = phase;
        f.error = None;
        f.started = Instant::now();
        f.feed.lock().unwrap().clear();
        let feed = f.feed.clone();
        let mine = run.clone();
        let events = orbit_core::events::Events::new(move |name: &str, v: serde_json::Value| {
            if name != "agent-feed" || v.get("run").and_then(|r| r.as_str()) != Some(mine.as_str()) {
                return;
            }
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
            let tool = v.get("name").and_then(|t| t.as_str()).unwrap_or("");
            if !text.is_empty() {
                feed.lock().unwrap().push(if tool.is_empty() { text } else { format!("{tool} · {text}") });
            }
        });
        // Repaints the feed while the agent works.
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let going = this.update(cx, |t, cx| {
                cx.notify();
                t.plan_form().is_some_and(|f| f.running())
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
        Some((run, events))
    }

    fn plan_ask(&mut self, finish: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        let (ws, goal, all, fresh, session) = (f.ws.clone(), f.goal.read(cx).value().to_string(), f.answers.clone(), f.unsent.clone(), f.session.clone());
        let Some((run, events)) = self.plan_begin(Phase::Thinking, window, cx) else { return };
        spawn(
            window,
            cx,
            move || core_plan::interview(&events, &run, &ws, &goal, &all, &fresh, session.as_deref(), finish),
            move |t, res, window, cx| {
                let Some(f) = t.plan_form() else { return };
                match res {
                    Ok(turn) => {
                        f.session = turn.session.or(f.session.take());
                        f.unsent.clear();
                        if turn.round.done || turn.round.questions.is_empty() {
                            let text = if turn.round.summary.trim().is_empty() { as_summary(&f.answers) } else { turn.round.summary.trim().to_string() };
                            f.round = None;
                            f.phase = Phase::Summary;
                            f.summary.update(cx, |s, cx| s.set_value(text, window, cx));
                        } else {
                            f.round = Some((turn.round.questions, 0));
                            f.rounds += 1;
                            f.phase = Phase::Asking;
                            f.answer.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                    }
                    Err(e) if finish && e != "cancelled" => {
                        let text = as_summary(&f.answers);
                        f.phase = Phase::Summary;
                        f.summary.update(cx, |s, cx| s.set_value(text, window, cx));
                    }
                    Err(e) => {
                        f.phase = Phase::Setup;
                        if e != "cancelled" {
                            f.error = Some(e);
                        }
                    }
                }
            },
        );
    }

    fn plan_submit_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        let value = f.answer.read(cx).value().trim().to_string();
        let Some((qs, cur)) = f.round.clone() else { return };
        if value.is_empty() {
            return;
        }
        let a = Answer { question: qs[cur].text.clone(), answer: value };
        f.answers.push(a.clone());
        f.unsent.push(a);
        f.answer.update(cx, |s, cx| s.set_value("", window, cx));
        if cur + 1 < qs.len() {
            f.round = Some((qs, cur + 1));
            cx.notify();
        } else {
            f.round = None;
            self.plan_ask(false, window, cx);
        }
    }

    fn plan_draft(&mut self, decisions: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        let back = if f.phase == Phase::Summary { Phase::Summary } else { Phase::Setup };
        let (ws, goal) = (f.ws.clone(), f.goal.read(cx).value().to_string());
        let Some((run, events)) = self.plan_begin(Phase::Drafting, window, cx) else { return };
        spawn(window, cx, move || core_plan::draft(&events, &run, &ws, &goal, &decisions), move |t, res, window, cx| match res {
            Ok(_) => t.plan_ready(window, cx),
            Err(e) => {
                if let Some(f) = t.plan_form() {
                    f.phase = back;
                    if e != "cancelled" {
                        f.error = Some(e);
                    }
                }
            }
        });
    }

    /// PLAN.md is there: close, re-read it and open it.
    fn plan_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal(cx);
        self.load_plan(window, cx);
        self.open_plan_file(window, cx);
    }

    fn plan_blank(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        let (ws, goal) = (f.ws.clone(), f.goal.read(cx).value().to_string());
        spawn(window, cx, move || core_plan::blank(&ws, &goal), |t, res, window, cx| match res {
            Ok(_) => t.plan_ready(window, cx),
            Err(e) => {
                if let Some(f) = t.plan_form() {
                    f.error = Some(e);
                }
            }
        });
    }

    /// Plans in an interactive session of the configured agent.
    fn plan_in_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        let (ws, goal) = (f.ws.clone(), f.goal.read(cx).value().to_string());
        spawn(
            window,
            cx,
            move || {
                let ai = orbit_core::config::Config::load()?.ai;
                core_plan::session_prompt(&ws, &goal).map(|p| (ws, ai.agent_bin(), ai.model, p))
            },
            |t, res, window, cx| match res {
                Ok((ws, agent, model, prompt)) => {
                    t.close_modal(cx);
                    t.with_app(window, cx, move |a, w, cx| crate::sessions::new_prompted_session(a, &ws, &agent, &model, &prompt, Some("plan"), false, w, cx));
                }
                Err(e) => {
                    if let Some(f) = t.plan_form() {
                        f.error = Some(e);
                    }
                }
            },
        );
    }

    /// Close, or Stop while an agent runs.
    fn plan_close(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.plan_form() else { return };
        if f.running() {
            if let Some(run) = &f.run {
                orbit_core::agent::live::cancel(run);
            }
            return;
        }
        self.close_modal(cx);
    }

    pub(super) fn render_plan(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Modal::Plan(f)) = &self.modal else { return div().into_any_element() };
        let needs_goal = f.card.is_none() && f.goal.read(cx).value().trim().is_empty();
        let step = match f.phase {
            Phase::Setup => None,
            Phase::Drafting => Some("Drafting the plan".to_string()),
            Phase::Summary => Some("Review the decisions".to_string()),
            Phase::Thinking | Phase::Asking => Some(format!("Interview · round {}", (f.rounds + usize::from(f.phase == Phase::Thinking)).max(1))),
        };
        let head = div()
            .flex()
            .items_baseline()
            .justify_between()
            .gap(px(12.))
            .px(px(18.))
            .pt(px(16.))
            .child(div().text_size(px(15.8)).font_weight(FontWeight::BOLD).child(if f.exists { "Replan this feature" } else { "Plan this feature" }))
            .when_some(step, |d, s| d.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(s)));
        let mut body = div().id("plan-body").flex().flex_col().gap(px(12.)).px(px(16.)).py(px(18.)).min_h_0().overflow_y_scroll();
        if let Some(e) = &f.error {
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(kit::alpha(c(theme::DANGER), 0.4))
                    .bg(theme::danger_soft())
                    .text_size(px(12.5))
                    .child(div().flex_1().child(e.clone())),
            );
        }
        body = match f.phase {
            Phase::Setup => {
                let choices: [Choice; 4] = [
                    (
                        "plan-interview",
                        "Interview me first",
                        Some("Recommended"),
                        "The agent explores the repos and asks you questions in rounds, then drafts the plan with your decisions.",
                        false,
                        |t, w, cx| {
                            if let Some(f) = t.plan_form() {
                                f.answers.clear();
                                f.unsent.clear();
                                f.session = None;
                                f.rounds = 0;
                            }
                            t.plan_ask(false, w, cx)
                        },
                    ),
                    (
                        "plan-draft",
                        "Draft it now",
                        None,
                        "The agent explores and writes the plan in one go. Unclear points become stated assumptions you can edit.",
                        false,
                        |t, w, cx| t.plan_draft(String::new(), w, cx),
                    ),
                    (
                        "plan-terminal",
                        "Plan in a terminal",
                        None,
                        "Talk it through freely with the agent in a session. It writes PLAN.md when you're both done.",
                        false,
                        |t, w, cx| t.plan_in_terminal(w, cx),
                    ),
                    (
                        "plan-blank",
                        if f.exists { "Open the current plan" } else { "Write it myself" },
                        None,
                        if f.exists { "Edit PLAN.md by hand." } else { "Start from the plan template: context, then tasks per repo." },
                        true,
                        |t, w, cx| t.plan_blank(w, cx),
                    ),
                ];
                body.when_some(f.card.as_ref(), |d, card: &CardRef| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .px(px(9.))
                                    .py(px(1.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(kit::alpha(c(theme::INFO), 0.25))
                                    .bg(kit::alpha(c(theme::INFO), 0.08))
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(c(theme::INFO))
                                    .child(card.id.clone()),
                            )
                            .child(div().flex_1().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(card.title.clone())),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .text_size(px(12.5))
                        .text_color(c(theme::TEXT_DIM))
                        .child(if f.card.is_some() { "Anything to add?" } else { "What should this feature do?" })
                        .child(div().id("plan-goal").relative().child(Textarea::new(&f.goal)).child(probe("plan-goal"))),
                )
                .when(f.exists, |d| {
                    d.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Drafting again replaces PLAN.md; the current one is kept in .orbit/plans."))
                })
                .child(div().grid().grid_cols(2).gap(px(8.)).children(choices.into_iter().map(|(id, title, tag, text, always, go)| {
                    let blocked = needs_goal && !always;
                    let main = tag.is_some();
                    let el = div()
                        .id(id)
                        .relative()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(if main { kit::alpha(c(theme::ACCENT), 0.45) } else { c(theme::BORDER) })
                        .bg(c(theme::PANEL_RAISED))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title)
                                .when_some(tag, |d, tag| {
                                    d.child(
                                        div()
                                            .px(px(6.))
                                            .py(px(1.))
                                            .rounded_full()
                                            .bg(theme::accent_soft())
                                            .text_size(px(10.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(c(theme::ACCENT))
                                            .child(tag),
                                    )
                                }),
                        )
                        .child(div().text_size(px(12.)).line_height(px(17.4)).text_color(c(theme::TEXT_DIM)).child(text))
                        .child(probe(id));
                    if blocked {
                        kit::hint(el.opacity(0.5), "Describe the feature first").into_any_element()
                    } else {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(c(theme::PANEL_HOVER)).border_color(if main { c(theme::ACCENT) } else { c(theme::BORDER_STRONG) }))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| go(t, w, cx)))
                            .into_any_element()
                    }
                })))
            }
            Phase::Thinking | Phase::Drafting => {
                let waiting = match f.phase {
                    Phase::Drafting => "Exploring the repositories to write the plan…",
                    _ if f.rounds == 0 => "Exploring the repositories before asking…",
                    _ => "Reading your answers…",
                };
                body.child(feed(f, waiting))
            }
            Phase::Asking => {
                let Some((qs, cur)) = &f.round else { return div().into_any_element() };
                let q: &GrillQuestion = &qs[*cur];
                let picked = f.answer.read(cx).value().to_string();
                body.child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(theme::ACCENT))
                        .child(format!("QUESTION {} OF {}", cur + 1, qs.len())),
                )
                .child(div().text_size(px(15.)).line_height(px(22.5)).font_weight(FontWeight::MEDIUM).child(q.text.clone()))
                .when(!q.options.is_empty(), |d| {
                    let desc = q.options.iter().find(|o| o.label == picked).or_else(|| q.options.iter().find(|o| o.recommended)).map(|o| o.description.clone());
                    d.child(div().flex().flex_wrap().gap(px(8.)).children(q.options.iter().enumerate().map(|(i, o)| {
                        let label = o.label.clone();
                        let on = picked == o.label;
                        div()
                            .id(gpui::ElementId::Name(format!("plan-opt:{i}").into()))
                            .relative()
                            .px(px(12.))
                            .py(px(4.))
                            .rounded_full()
                            .border_1()
                            .border_color(if o.recommended || on { c(theme::ACCENT) } else { c(theme::BORDER_STRONG) })
                            .bg(if on { kit::alpha(c(theme::ACCENT), 0.28) } else if o.recommended { theme::accent_soft() } else { c(theme::PANEL_RAISED) })
                            .text_size(px(12.5))
                            .text_color(if o.recommended { kit::mix(c(theme::INFO), c(theme::TEXT), 0.4) } else { c(theme::TEXT) })
                            .cursor_pointer()
                            .when(o.recommended, |d| d.child(div().mr(px(5.)).text_size(px(10.)).text_color(c(theme::ACCENT)).child("★")).flex().items_center())
                            .child(o.label.clone())
                            .child(probe(format!("plan-opt:{i}")))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                let label = label.clone();
                                if let Some(f) = t.plan_form() {
                                    f.answer.update(cx, |s, cx| s.set_value(label, w, cx));
                                }
                                cx.notify();
                            }))
                    })))
                    .when_some(desc.filter(|d| !d.is_empty()), |d, desc| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(desc)))
                })
                .child(div().id("plan-answer").relative().child(Textarea::new(&f.answer)).child(probe("plan-answer")))
                .when(!f.answers.is_empty(), |d| {
                    d.child(div().flex().flex_col().gap(px(6.)).children(f.answers.iter().rev().take(3).rev().map(|a| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_size(px(12.))
                            .child(div().text_color(c(theme::TEXT_FAINT)).child(a.question.clone()))
                            .child(div().text_color(c(theme::TEXT_DIM)).child(a.answer.clone()))
                    })))
                })
            }
            Phase::Summary => body
                .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("What the plan will follow. Edit anything before drafting."))
                .child(div().id("plan-summary").relative().font_family(theme::MONO_FONT).child(Textarea::new(&f.summary)).child(probe("plan-summary"))),
        };
        let running = f.running();
        let footer = kit::modal_footer()
            .child(if running {
                kit::button("plan-stop", "Stop", Kind::Secondary, kit::NORMAL)
                    .border_color(kit::alpha(c(theme::DANGER), 0.4))
                    .text_color(c(theme::DANGER))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.plan_close(cx)))
            } else {
                kit::button("modal-cancel", "Close", Kind::Secondary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.plan_close(cx)))
            })
            .when(f.phase == Phase::Asking, |d| {
                let last = f.round.as_ref().is_some_and(|(qs, cur)| cur + 1 >= qs.len());
                let empty = f.answer.read(cx).value().trim().is_empty();
                d.child(
                    kit::hint(kit::disabled(kit::button("plan-ready", "I'm ready", Kind::Secondary, kit::NORMAL), f.answers.is_empty()), "Skip the remaining questions: the agent summarizes what's decided")
                        .when(!f.answers.is_empty(), |b| {
                            b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                                if let Some(f) = t.plan_form() {
                                    f.round = None;
                                }
                                t.plan_ask(true, w, cx)
                            }))
                        }),
                )
                .child(
                    kit::disabled(kit::button("plan-next", if last { "Submit round" } else { "Next question" }, Kind::Primary, kit::NORMAL), empty)
                        .when(!empty, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.plan_submit_answer(w, cx)))),
                )
            })
            .when(f.phase == Phase::Summary, |d| {
                d.child(kit::button("plan-draft-summary", "Draft the plan", Kind::Primary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                    let Some(f) = t.plan_form() else { return };
                    let decisions = format!("{}\n\nFull interview:\n{}", f.summary.read(cx).value(), as_summary(&f.answers));
                    t.plan_draft(decisions, w, cx)
                })))
            });
        kit::modal("plan-modal", 720.).child(head).child(body).child(footer).into_any_element()
    }
}

type PlanAct = fn(&mut PageTab, &mut Window, &mut Context<PageTab>);
/// Probe id, title, tag, text, available without a goal, action.
type Choice = (&'static str, &'static str, Option<&'static str>, &'static str, bool, PlanAct);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Setup,
    Thinking,
    Asking,
    Summary,
    Drafting,
}

pub struct PlanForm {
    pub ws: String,
    card: Option<CardRef>,
    /// A PLAN.md exists: drafting replaces it (the old one is kept).
    exists: bool,
    goal: Entity<TextareaState>,
    answer: Entity<TextareaState>,
    summary: Entity<TextareaState>,
    pub phase: Phase,
    answers: Vec<Answer>,
    /// Answers the agent hasn't received yet.
    unsent: Vec<Answer>,
    session: Option<String>,
    /// The round being answered, and the current question.
    round: Option<(Vec<GrillQuestion>, usize)>,
    rounds: usize,
    run: Option<String>,
    feed: Arc<Mutex<Vec<String>>>,
    started: Instant,
    error: Option<String>,
}

impl PlanForm {
    fn running(&self) -> bool {
        matches!(self.phase, Phase::Thinking | Phase::Drafting)
    }

    pub fn dump(&self, out: &mut String, cx: &gpui::App) {
        let _ = writeln!(
            out,
            "  plan modal ({}): {:?} exists {} goal {:?} answers {} error {:?}",
            self.ws,
            self.phase,
            self.exists,
            self.goal.read(cx).value(),
            self.answers.len(),
            self.error
        );
    }
}

fn as_summary(list: &[Answer]) -> String {
    if list.is_empty() {
        return "- No answers were given.".into();
    }
    list.iter().map(|a| format!("- {}\n  → {}", a.question, a.answer)).collect::<Vec<_>>().join("\n")
}

/// `AgentFeed`: what the agent is doing, live.
fn feed(f: &PlanForm, waiting: &'static str) -> impl IntoElement {
    let lines = f.feed.lock().unwrap().clone();
    let now = lines.last().cloned().unwrap_or_else(|| waiting.to_string());
    let secs = f.started.elapsed().as_secs();
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::BG))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(10.))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .text_size(px(12.5))
                .child(kit::spinner("plan-feed-spin", 14., c(theme::ACCENT)))
                .child(div().flex_1().min_w_0().truncate().child(now))
                .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("{}:{:02}", secs / 60, secs % 60))),
        )
        .child(
            div()
                .id("plan-feed")
                .h(px(300.))
                .overflow_y_scroll()
                .px(px(12.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .when(lines.is_empty(), |d| d.child(div().py(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(waiting)))
                .children(lines.into_iter().rev().take(200).rev().map(|l| div().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(l))),
        )
}

/// The "Plan · progress" card, when PLAN.md has tasks.
pub fn progress(p: &WsPage, cx: &mut Context<PageTab>) -> Option<impl IntoElement> {
    let tasks = p.plan.as_ref().filter(|t| !t.is_empty())?;
    let done = tasks.iter().filter(|t| t.done).count();
    let total = tasks.len();
    let pct = (done * 100 + total / 2) / total;
    Some(
        div()
            .p(px(16.))
            .rounded(px(10.))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().py(px(16.)).text_size(px(15.8)).line_height(px(22.)).font_weight(FontWeight::BOLD).child("Plan · progress"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .when(done < total, |d| {
                                d.child(
                                    kit::hint(kit::button("plan-ralph", "Run with Ralph", Kind::Primary, kit::MINI), "Ralph implements the open tasks one by one, each by a fresh agent")
                                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                                            let Some(name) = t.ws_name() else { return };
                                            t.with_app(w, cx, move |a, w, cx| super::run_for(a, &name, "ralph.open", w, cx));
                                        })),
                                )
                            })
                            .child(kit::link("plan-open", "open PLAN.md").on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan_file(w, cx)))),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .mb(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(6.))
                            .rounded_full()
                            .bg(c(theme::PANEL_RAISED))
                            .overflow_hidden()
                            .child(div().h_full().w(gpui::relative(pct as f32 / 100.)).rounded_full().bg(c(theme::ACCENT))),
                    )
                    .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(format!("{done}/{total} · {pct}%"))),
            )
            .child(div().id("plan-tasks").max_h(px(220.)).overflow_y_scroll().flex().flex_col().gap(px(6.)).children(tasks.iter().enumerate().map(|(i, t)| {
                div()
                    .id(gpui::ElementId::Name(format!("plan-task:{i}").into()))
                    .relative()
                    .flex()
                    .items_start()
                    .gap(px(9.))
                    .text_size(px(12.5))
                    .line_height(px(18.125))
                    .cursor_pointer()
                    .child(div().mt(px(1.)).child(kit::check(t.done, false)))
                    .child(
                        div()
                            .min_w_0()
                            .when(t.done, |d| d.text_color(c(theme::TEXT_FAINT)).line_through())
                            .when(!t.done, |d| d.text_color(c(theme::TEXT)))
                            .child(t.text.clone()),
                    )
                    .child(probe(format!("plan-task:{i}")))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.toggle_task(i, w, cx)))
            }))),
    )
}
