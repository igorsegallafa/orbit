//! Ralph (`RalphView.tsx`, `RalphRunModal.tsx`): the autonomous loop over
//! the workspace's plan (PLAN.md, one agent per task) or a repo's PRD
//! (`scripts/ralph/prd.json`, one agent per story). Start, pause and stop,
//! the stories, the run's state and its live feed through orbit-core's
//! `ralph-event`s, and past runs. Writing a PRD (interview, generation) and
//! editing it are not ported.
use super::ui::{self, Tone};
use super::{Host, PageView};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, SharedString, Task, Window};
use orbit_core::events::Events;
use orbit_core::ralph::{self, RalphState, RunConfig};
use serde_json::Value;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The plan run's "repo" (core's `PLAN_REPO`).
const PLAN: &str = "plan";
const FEED_MAX: usize = 400;

/// Every open Ralph page gets every `ralph-event` (`{key, event}`).
fn subscribers() -> &'static Mutex<Vec<UnboundedSender<Value>>> {
    static S: OnceLock<Mutex<Vec<UnboundedSender<Value>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The sink runs report to: fans the events out to the pages.
fn events() -> Events {
    Events::new(|name: &str, payload: Value| {
        if name == "ralph-event" {
            subscribers().lock().unwrap().retain(|tx| tx.unbounded_send(payload.clone()).is_ok());
        }
    })
}

pub struct RalphPage {
    host: Host,
    ws: Workspace,
    /// `plan` or a repo of the workspace.
    target: String,
    state: Load<RalphState>,
    feed: Vec<Value>,
    runs: Vec<Value>,
    /// None: until complete.
    max_iterations: Option<u32>,
    push: bool,
    starting: bool,
    _events: Task<()>,
    _poll: Task<()>,
}

/// One feed line for an event (`RalphView`'s feed).
pub fn describe(ev: &Value) -> String {
    let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
    match ev["kind"].as_str().unwrap_or("") {
        "run_start" => format!("▶ run started · {} · {}", s("agent"), s("model")),
        "model" => format!("model {}", s("model")),
        "iteration_start" => match ev["story"].as_object() {
            Some(st) => format!("── iteration {} · {} {}", ev["n"], st["id"].as_str().unwrap_or(""), st["title"].as_str().unwrap_or("")),
            None => format!("── iteration {}", ev["n"]),
        },
        "text" => s("text"),
        "tool" => format!("⚙ {} {}", s("name"), s("summary")),
        "log" => format!("{}{}", if ev["stderr"].as_bool() == Some(true) { "stderr: " } else { "" }, s("text")),
        "story_passed" => format!("✓ {} passes", s("id")),
        "iteration_end" => format!(
            "iteration {} {} · {} commits · {}/{} done{}",
            ev["n"],
            s("outcome"),
            ev["commits"].as_u64().unwrap_or(0),
            ev["passed"].as_u64().unwrap_or(0),
            ev["total"].as_u64().unwrap_or(0),
            ev["costUsd"].as_f64().map(|c| format!(" · ${c:.2}")).unwrap_or_default()
        ),
        "push" if ev["ok"].as_bool() == Some(true) => "pushed".into(),
        "push" => format!("push failed: {}", s("message")),
        "limit_wait" => format!("usage limit: waiting until {}", ev["until"]),
        "stopped" => format!("■ stopped: {} after {} iterations", reason(&ev["reason"]), ev["iterations"]),
        other => format!("{other} {ev}"),
    }
}

/// A `StopReason` as JSON (`{kind, detail}`) in words.
fn reason(v: &Value) -> String {
    let detail = v["detail"].as_str().map(|d| format!(" ({d})")).unwrap_or_default();
    let kind = match v["kind"].as_str().unwrap_or("") {
        "complete" => "complete",
        "storyReached" => "reached the story",
        "maxIterations" => "max iterations",
        "stalled" => "stalled (no commits)",
        "limit" => "usage limit",
        "deadline" => "time budget",
        "failed" => "failed",
        "blocked" => "blocked",
        "stopped" => "stopped",
        "paused" => "paused",
        other => other,
    };
    format!("{kind}{detail}")
}

impl RalphPage {
    pub fn new(host: Host, ws: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = unbounded::<Value>();
        subscribers().lock().unwrap().push(tx);
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Some(payload) = rx.next().await {
                if this.update_in(cx, |t: &mut Self, w, cx| t.on_event(payload, w, cx)).is_err() {
                    break;
                }
            }
        });
        // The run's counters live in core: re-read them while it runs.
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let alive = this.update_in(cx, |t: &mut Self, w, cx| {
                if t.running() {
                    t.load(false, w, cx);
                }
            });
            if alive.is_err() {
                break;
            }
        });
        let mut page = Self {
            host,
            ws,
            target: PLAN.into(),
            state: Load::Loading,
            feed: vec![],
            runs: vec![],
            max_iterations: Some(10),
            push: false,
            starting: false,
            _events: events,
            _poll: poll,
        };
        page.load(true, window, cx);
        page
    }

    fn key(&self) -> String {
        format!("{}/{}", self.ws.meta.name, self.target)
    }

    fn running(&self) -> bool {
        self.state.ready().and_then(|s| s.run.as_ref()).is_some_and(|r| r.running)
    }

    fn noun(&self) -> &'static str {
        if self.target == PLAN {
            "task"
        } else {
            "story"
        }
    }

    /// Reads the state; `feed` also replaces the feed with the run's events.
    fn load(&mut self, feed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (ws, target) = (self.ws.meta.name.clone(), self.target.clone());
        ui::bg(
            cx,
            move || {
                let state = if target == PLAN { ralph::plan_state(ws.clone()) } else { ralph::state(ws.clone(), target) };
                (state, ralph::past_runs(ws).unwrap_or_default())
            },
            move |t: &mut Self, (state, runs), _, _| {
                if feed {
                    t.feed = state.as_ref().ok().and_then(|s| s.run.as_ref()).map(|r| r.events.iter().cloned().collect()).unwrap_or_default();
                }
                if let Ok(s) = &state {
                    // The run's own settings, once.
                    if let (true, Some(r)) = (feed, &s.run) {
                        t.max_iterations = (!r.config.limits.until_complete).then_some(r.config.limits.max_iterations);
                        t.push = r.config.push;
                    }
                }
                t.state = state.map_or_else(Load::Failed, Load::Ready);
                let prefix = format!("-{}", t.target);
                t.runs = runs.into_iter().filter(|r| r["id"].as_str().is_some_and(|id| id.ends_with(&prefix))).collect();
            },
            window,
        );
    }

    fn on_event(&mut self, payload: Value, window: &mut Window, cx: &mut Context<Self>) {
        if payload["key"].as_str() != Some(self.key().as_str()) {
            return;
        }
        let ev = payload["event"].clone();
        let kind = ev["kind"].as_str().unwrap_or("").to_string();
        self.feed.push(ev);
        if self.feed.len() > FEED_MAX {
            self.feed.drain(..self.feed.len() - FEED_MAX);
        }
        if matches!(kind.as_str(), "iteration_end" | "stopped" | "story_passed" | "run_start") {
            self.load(false, window, cx);
        }
        if kind == "stopped" {
            let text = describe(&self.feed[self.feed.len() - 1]);
            self.host.toast(ToastKind::Info, format!("Ralph · {}", self.ws.meta.name), Some(text), cx);
        }
        cx.notify();
    }

    fn set_target(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        if target != self.target {
            self.target = target;
            self.state = Load::Loading;
            self.feed.clear();
            self.load(true, window, cx);
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.state.ready() else { return };
        let mut config: RunConfig = st.default_config.clone();
        config.limits.until_complete = self.max_iterations.is_none();
        if let Some(n) = self.max_iterations {
            config.limits.max_iterations = n;
        }
        config.push = self.push;
        self.starting = true;
        self.feed.clear();
        let (ws, target) = (self.ws.meta.name.clone(), self.target.clone());
        ui::bg(
            cx,
            move || if target == PLAN { ralph::plan_start(events(), ws, config) } else { ralph::start(events(), ws, target, config) },
            |t: &mut Self, r, w, cx| {
                t.starting = false;
                match r {
                    Ok(()) => t.load(false, w, cx),
                    Err(e) => t.host.error("Couldn't start Ralph", e, cx),
                }
            },
            window,
        );
        cx.notify();
    }

    fn stop(&mut self, pause: bool, cx: &mut Context<Self>) {
        let (ws, target) = (self.ws.meta.name.clone(), self.target.clone());
        let r = if pause { ralph::pause(ws, target) } else { ralph::stop(ws, target) };
        if let Err(e) = r {
            self.host.error("Couldn't stop Ralph", e, cx);
        } else if pause {
            self.host.toast(ToastKind::Info, "Ralph stops after this iteration", None, cx);
        }
        cx.notify();
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity().downgrade();
        let mut targets: Vec<(String, &str)> = vec![(PLAN.to_string(), "Plan")];
        targets.extend(self.ws.meta.repos.iter().map(|r| (r.clone(), r.as_str())));
        let target_seg = ui::segmented("ralph-target", &targets, &self.target, move |v, w, cx| {
            let _ = me.update(cx, |t, cx| t.set_target(v, w, cx));
        });
        let me = cx.entity().downgrade();
        let iters = ui::segmented("ralph-iterations", &[(Some(3), "3"), (Some(5), "5"), (Some(10), "10"), (None, "Until done")], &self.max_iterations, move |v, _, cx| {
            let _ = me.update(cx, |t, cx| {
                t.max_iterations = v;
                cx.notify();
            });
        });
        let running = self.running();
        let can_start = self.state.ready().is_some_and(|s| s.prd.as_ref().is_some_and(|p| !p.all_passed()) && s.warnings.is_empty());
        let mut bar = div().flex().flex_wrap().items_center().gap_3().child(target_seg);
        if running {
            bar = bar
                .child(div().flex_1())
                .child(ui::button("ralph-pause", "Pause after iteration", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.stop(true, cx))))
                .child(ui::button("ralph-stop", "Stop", Tone::Danger).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.stop(false, cx))));
        } else {
            bar = bar
                .child(ui::faint("Iterations"))
                .child(iters)
                .child(ui::check("ralph-push", "Push after each commit", self.push).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.push = !t.push;
                    cx.notify();
                })))
                .child(div().flex_1())
                .when(can_start, |d| {
                    let last = self.state.ready().is_some_and(|s| s.run.is_some());
                    d.child(
                        ui::button("ralph-start", if self.starting { "Starting…" } else if last { "▶ Run again" } else { "▶ Start Ralph" }, Tone::Primary)
                            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                                if !t.starting {
                                    t.start(w, cx)
                                }
                            })),
                    )
                });
        }
        bar
    }
}

impl Render for RalphPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.target == PLAN;
        let mut page = ui::page(
            "ralph-body",
            format!("Ralph · {}", self.ws.meta.name),
            if plan {
                "Builds the feature plan task by task · a fresh agent per task, in that task's repository"
            } else {
                "Autonomous loop over the repo's PRD (scripts/ralph/prd.json) · one story per agent run"
            },
        )
        .child(self.render_controls(cx));
        let st = match &self.state {
            Load::Ready(s) => s,
            Load::Failed(e) => return page.child(ui::error(e.clone())),
            _ => return page.child(ui::faint("Loading…")),
        };

        // The run.
        if let Some(run) = &st.run {
            let status = if run.running {
                format!(
                    "Running · iteration {} · {} {} · {}/{} done · {} commits · ${:.2}{}",
                    run.iteration,
                    self.noun(),
                    run.current_story.clone().unwrap_or_default(),
                    run.passed,
                    run.total,
                    run.commits,
                    run.cost_usd,
                    run.model.as_ref().map(|m| format!(" · {m}")).unwrap_or_default()
                )
            } else {
                let why = run.reason.as_ref().map(|r| reason(&serde_json::to_value(r).unwrap_or_default())).unwrap_or_default();
                format!("Last run: {why} · {} iterations · {}/{} done · {} commits · ${:.2}", run.iteration, run.passed, run.total, run.commits, run.cost_usd)
            };
            page = page.child(
                ui::card()
                    .p_3()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(ui::dot(c(if run.running { theme::OK } else { theme::TEXT_FAINT })))
                    .child(div().text_sm().text_color(c(theme::TEXT)).child(status)),
            );
        }
        for w in &st.warnings {
            page = page.child(ui::error(format!("Fix the plan before starting: {w}")));
        }

        // Stories / tasks.
        match &st.prd {
            None if plan => {
                page = page.child(ui::card().p_4().child(ui::muted(if st.plan_exists {
                    "The plan has no tasks yet: add \"- [ ] …\" lines to PLAN.md."
                } else {
                    "No PLAN.md yet. Plan the feature first (Plan this feature in the Tauri app, or write PLAN.md at the workspace root), then let Ralph build it."
                })));
            }
            None => {
                page = page.child(ui::card().p_4().child(ui::muted(format!(
                    "No scripts/ralph/prd.json in {} yet. Writing a PRD (interview and generation) is in the Tauri app for now; Ralph runs it here once it exists.",
                    self.target
                ))));
            }
            Some(prd) => {
                let current = st.run.as_ref().filter(|r| r.running).and_then(|r| r.current_story.clone());
                let mut stories = prd.user_stories.clone();
                stories.sort_by_key(|s| s.priority);
                page = page.child(ui::heading(format!("{} · {}/{} done", if plan { "Tasks" } else { "Stories" }, prd.passed(), prd.user_stories.len())));
                if !prd.description.trim().is_empty() {
                    page = page.child(ui::muted(prd.description.clone()));
                }
                let mut card = ui::card();
                for s in &stories {
                    let now = current.as_deref() == Some(s.id.as_str());
                    card = card.child(
                        div()
                            .px_4()
                            .py_2()
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_b_1()
                            .border_color(c(theme::BORDER))
                            .when(now, |d| d.bg(theme::accent_soft()))
                            .child(div().w(px(16.)).text_color(c(if s.passes { theme::OK } else { theme::TEXT_FAINT })).child(if s.passes { "✓" } else { "○" }))
                            .child(div().w(px(70.)).flex_none().child(ui::code(s.id.clone())))
                            .child(div().flex_1().min_w_0().text_sm().text_color(c(theme::TEXT)).child(s.title.clone()))
                            .when(!s.acceptance_criteria.is_empty(), |d| d.child(ui::faint(format!("{} criteria", s.acceptance_criteria.len())))),
                    );
                }
                page = page.child(card);
            }
        }

        // Live feed.
        if !self.feed.is_empty() {
            page = page.child(ui::heading("Feed")).child(
                div()
                    .id("ralph-feed")
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded(px(6.))
                    .bg(c(theme::PANEL))
                    .font_family(theme::MONO_FONT)
                    .text_xs()
                    .text_color(c(theme::TEXT_DIM))
                    .children(self.feed.iter().map(|e| div().child(describe(e))).collect::<Vec<_>>()),
            );
        }

        // Past runs.
        if !self.runs.is_empty() {
            let mut card = ui::card();
            for r in self.runs.iter().take(10) {
                card = card.child(
                    div().px_4().py_1().flex().gap_3().text_xs().child(ui::code(r["id"].as_str().unwrap_or("").to_string())).child(ui::muted(format!(
                        "{} · {} iterations · {}/{} done · {} commits · ${:.2}",
                        reason(&r["reason"]),
                        r["iteration"],
                        r["passed"],
                        r["total"],
                        r["commits"],
                        r["costUsd"].as_f64().unwrap_or(0.)
                    ))),
                );
            }
            page = page.child(ui::heading("Past runs")).child(card);
        }
        page
    }
}

impl PageView for RalphPage {
    fn title(&self) -> SharedString {
        format!("Ralph · {}", self.ws.meta.name).into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(false, window, cx);
    }

    fn dump(&self, out: &mut String, _: &App) {
        let _ = writeln!(out, "  target {} · max iterations {:?} · push {}", self.target, self.max_iterations, self.push);
        match &self.state {
            Load::Ready(s) => {
                if let Some(prd) = &s.prd {
                    let _ = writeln!(out, "  prd {} · {}/{} pass", prd.project, prd.passed(), prd.user_stories.len());
                    for st in &prd.user_stories {
                        let _ = writeln!(out, "    [{}] {} {}", if st.passes { "x" } else { " " }, st.id, st.title);
                    }
                } else {
                    let _ = writeln!(out, "  no prd (plan exists {})", s.plan_exists);
                }
                if let Some(r) = &s.run {
                    let _ = writeln!(
                        out,
                        "  run {} running {} iteration {} story {:?} {}/{} commits {} reason {:?}",
                        r.id, r.running, r.iteration, r.current_story, r.passed, r.total, r.commits, r.reason
                    );
                }
                for w in &s.warnings {
                    let _ = writeln!(out, "  warning {w}");
                }
            }
            other => {
                let _ = writeln!(out, "  state: {}", ui::load_label(other));
            }
        }
        let _ = writeln!(out, "  feed {} events", self.feed.len());
        for e in self.feed.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev() {
            let _ = writeln!(out, "    {}", describe(e));
        }
        for r in self.runs.iter().take(5) {
            let _ = writeln!(out, "  past run {} {}", r["id"].as_str().unwrap_or(""), reason(&r["reason"]));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn describes_feed_events() {
        let d = super::describe;
        assert_eq!(d(&json!({"kind": "iteration_start", "n": 2, "story": {"id": "US-002", "title": "Login"}})), "── iteration 2 · US-002 Login");
        assert_eq!(d(&json!({"kind": "tool", "name": "Edit", "summary": "src/a.rs"})), "⚙ Edit src/a.rs");
        assert_eq!(d(&json!({"kind": "story_passed", "id": "US-1"})), "✓ US-1 passes");
        assert_eq!(
            d(&json!({"kind": "stopped", "reason": {"kind": "maxIterations"}, "iterations": 3})),
            "■ stopped: max iterations after 3 iterations"
        );
        assert_eq!(d(&json!({"kind": "iteration_end", "n": 1, "outcome": "Done", "commits": 1, "passed": 1, "total": 2, "costUsd": 0.5})), "iteration 1 Done · 1 commits · 1/2 done · $0.50");
    }
}
