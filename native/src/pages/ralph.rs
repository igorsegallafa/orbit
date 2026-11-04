//! Ralph (`RalphView.tsx`, `RalphRunModal.tsx`): the autonomous loop over
//! the workspace's plan (PLAN.md, one agent per task) or a repo's PRD
//! (`scripts/ralph/prd.json`, one agent per story). Start, pause and stop,
//! the stories, the run's state and its live feed through orbit-core's
//! `ralph-event`s, and past runs. Writing a PRD (interview, generation) and
//! editing it are not ported.
use super::kit::{self, Kind};
use super::ui;
use crate::probe::probe;
use super::{Host, PageView};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, FontWeight, SharedString, Task, Window};
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
    /// The run form (`RalphRunModal`) is open.
    run_open: bool,
    /// The story whose details show.
    expanded: Option<String>,
    /// A past run's id and events, shown instead of the live feed.
    viewing: Option<(String, Vec<Value>)>,
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
            run_open: false,
            expanded: None,
            viewing: None,
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
}

/// `reasonLabel` in `types/ralph.ts`.
fn reason_label(v: &Value) -> String {
    let detail = v["detail"].as_str().unwrap_or("");
    match v["kind"].as_str().unwrap_or("") {
        "complete" => "All stories pass".into(),
        "storyReached" => format!("Reached {detail}"),
        "maxIterations" => "Iteration limit reached".into(),
        "stalled" => "Stalled: no commits in the last iterations".into(),
        "limit" => "Stopped on usage limit".into(),
        "deadline" => "Time budget used".into(),
        "failed" => format!("Failed: {detail}"),
        "blocked" => format!("Blocked, needs your decision. {detail}"),
        "stopped" => "Stopped".into(),
        "paused" => "Paused".into(),
        _ => String::new(),
    }
}

/// `formatDuration` in `lib/ralphFeed.ts`.
fn duration(secs: u64) -> String {
    let m = secs / 60;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{m}m {}s", secs % 60),
        _ => format!("{}h {}m", m / 60, m % 60),
    }
}

/// The local UTC offset in seconds (`date +%z`, read once).
fn utc_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let z = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let digits: String = z.chars().filter(|c| c.is_ascii_digit()).collect();
        let num = |r: std::ops::Range<usize>| digits.get(r).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        sign * (num(0..2) * 3600 + num(2..4) * 60)
    })
}

/// "04/11/2026, 01:44" in local time (the run list's short date and time).
fn local_time(unix: i64) -> String {
    let t = unix + utc_offset();
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}/{m:02}/{y}, {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// One iteration of the feed (`groupByIteration`).
struct Group<'a> {
    n: u64,
    story: Option<(String, String)>,
    events: Vec<&'a Value>,
    end: Option<&'a Value>,
}

fn group_by_iteration(events: &[Value]) -> Vec<Group<'_>> {
    let mut groups = vec![];
    let mut cur = Group { n: 0, story: None, events: vec![], end: None };
    for ev in events {
        match ev["kind"].as_str() {
            Some("iteration_start") => {
                if !cur.events.is_empty() || cur.n > 0 {
                    groups.push(cur);
                }
                let story = ev["story"].as_object().map(|s| (s["id"].as_str().unwrap_or("").to_string(), s["title"].as_str().unwrap_or("").to_string()));
                cur = Group { n: ev["n"].as_u64().unwrap_or(0), story, events: vec![], end: None };
            }
            Some("iteration_end") => cur.end = Some(ev),
            _ => cur.events.push(ev),
        }
    }
    if !cur.events.is_empty() || cur.n > 0 {
        groups.push(cur);
    }
    groups
}

/// `.rv-story-id`.
fn story_id(id: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(c(theme::BORDER))
        .font_family(theme::MONO_FONT)
        .text_size(px(10.5))
        .text_color(c(theme::TEXT_FAINT))
        .child(id.into())
}

/// `.rv-col-head`.
fn col_head(label: &str) -> gpui::Div {
    div()
        .flex_none()
        .px(px(14.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .border_b_1()
        .border_color(c(theme::BORDER))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(label.to_uppercase())
}

/// A feed line (`FeedLine`).
fn feed_line(ev: &Value) -> Option<gpui::AnyElement> {
    let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
    let event = |text: String, color: u32, bold: bool| {
        div()
            .my(px(6.))
            .text_size(px(12.))
            .text_color(c(color))
            .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
            .child(text)
            .into_any_element()
    };
    Some(match ev["kind"].as_str().unwrap_or("") {
        "tool" => {
            let name = s("name");
            let (bg, fg) = match name.as_str() {
                "Read" => (kit::alpha(theme::ACCENT, 0.14), c(theme::INFO)),
                "Grep" | "Glob" | "WebSearch" | "WebFetch" | "List" | "Webfetch" | "Websearch" => (kit::alpha(theme::PURPLE, 0.14), kit::mix(theme::PURPLE, theme::TEXT, 0.6)),
                "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Patch" => (kit::alpha(theme::WARN, 0.14), c(theme::WARN)),
                "Bash" => (kit::alpha(theme::OK, 0.12), kit::mix(theme::OK, theme::TEXT, 0.7)),
                _ => (c(theme::PANEL_RAISED), c(theme::TEXT_DIM)),
            };
            div()
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(54.))
                        .flex_none()
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(bg)
                        .flex()
                        .justify_center()
                        .text_size(px(10.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(fg)
                        .child(name),
                )
                .child(div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(s("summary")))
                .into_any_element()
        }
        "thinking" => div().truncate().italic().text_color(c(theme::TEXT_FAINT)).child(s("text").split_whitespace().collect::<Vec<_>>().join(" ")).into_any_element(),
        "text" => {
            let text = s("text").replace("<promise>COMPLETE</promise>", "").replace("<task-done/>", "").trim().to_string();
            if text.is_empty() {
                return None;
            }
            div().my(px(6.)).pl(px(12.)).py(px(2.)).border_l_2().border_color(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)).child(text).into_any_element()
        }
        "log" => div()
            .font_family(theme::MONO_FONT)
            .text_size(px(11.))
            .text_color(c(if ev["stderr"].as_bool() == Some(true) { theme::DANGER } else { theme::TEXT_FAINT }))
            .child(s("text"))
            .into_any_element(),
        "story_passed" => event(format!("✓ {} passes", s("id")), theme::OK, true),
        "push" if ev["ok"].as_bool() == Some(true) => {
            event(format!("↑ Pushed {}to origin", ev["repo"].as_str().map(|r| format!("{r} ")).unwrap_or_default()), theme::TEXT_FAINT, false)
        }
        "push" => event(format!("Push failed{}: {}", ev["repo"].as_str().map(|r| format!(" in {r}")).unwrap_or_default(), s("message")), theme::DANGER, false),
        "limit_wait" => event("Usage limit reached · waiting to retry".into(), theme::WARN, false),
        "run_start" => event(format!("Run started with {} · {}", s("agent"), s("model")), theme::TEXT_FAINT, false),
        "model" => event(format!("Agent is running {}", s("model")), theme::TEXT_FAINT, false),
        "stopped" => {
            let n = ev["iterations"].as_u64().unwrap_or(0);
            let ok = ev["reason"]["kind"].as_str() == Some("complete");
            event(format!("{} · {n} iteration{}", reason_label(&ev["reason"]), if n == 1 { "" } else { "s" }), if ok { theme::OK } else { theme::TEXT_FAINT }, ok)
        }
        _ => return None,
    })
}

fn iteration_block(g: &Group, live: bool) -> gpui::Div {
    let mut block = div().pt(px(12.));
    if g.n > 0 {
        let story: gpui::AnyElement = match &g.story {
            Some((id, title)) => div().flex().items_center().gap(px(6.)).min_w_0().child(story_id(id.clone())).child(div().truncate().child(title.clone())).into_any_element(),
            None => div().child("Iteration").into_any_element(),
        };
        let meta = match g.end {
            Some(end) => {
                let outcome = end["outcome"].as_str().map(|o| o.split('(').next().unwrap_or("").trim().to_string()).filter(|o| !o.is_empty() && o != "Done" && o != "Complete");
                let commits = end["commits"].as_u64().unwrap_or(0);
                let mut text = format!("{commits} commit{}", if commits == 1 { "" } else { "s" });
                if let Some(ms) = end["durationMs"].as_u64() {
                    text += &format!(" · {}", duration(ms / 1000));
                }
                if let Some(cost) = end["costUsd"].as_f64().filter(|c| *c > 0.) {
                    text += &format!(" · ${cost:.2}");
                }
                Some(div().flex().gap(px(6.)).children(outcome.map(|o| div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::DANGER)).child(o))).child(text))
            }
            None if live => Some(div().child("working…")),
            None => None,
        };
        block = block.child(
            div()
                .py(px(8.))
                .mb(px(6.))
                .flex()
                .items_center()
                .gap(px(10.))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(c(theme::ACCENT)).child(format!("#{}", g.n)))
                .child(div().flex_1().min_w_0().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(story))
                .children(meta.map(|m| m.flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)))),
        );
    }
    block.child(div().flex().flex_col().gap(px(2.)).children(g.events.iter().filter_map(|e| feed_line(e))))
}

impl RalphPage {
    fn open_plan(&self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.ws.dir.join(orbit_core::agent::PLAN_FILE);
        self.host.open_file(path, window, cx);
    }

    fn view_run(&mut self, id: String, running: bool, window: &mut Window, cx: &mut Context<Self>) {
        if running {
            self.viewing = None;
            return cx.notify();
        }
        let ws = self.ws.meta.name.clone();
        let rid = id.clone();
        ui::bg(
            cx,
            move || ralph::run_events(ws, rid),
            move |t: &mut Self, r, _, cx| match r {
                Ok(events) => t.viewing = Some((id, events)),
                Err(e) => t.host.error("Couldn't read the run", e, cx),
            },
            window,
        );
    }

    fn render_header(&self, st: Option<&RalphState>, cx: &mut Context<Self>) -> gpui::Div {
        let plan = self.target == PLAN;
        let me = cx.entity().downgrade();
        let mut targets: Vec<(String, &str)> = vec![(PLAN.to_string(), "Plan")];
        targets.extend(self.ws.meta.repos.iter().map(|r| (r.clone(), r.as_str())));
        let target_seg = kit::seg("ralph-target", &targets, &self.target, move |v, w, cx| {
            let _ = me.update(cx, |t, cx| t.set_target(v, w, cx));
        });
        let title = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_w_0()
            .child(
                div()
                    .size(px(34.))
                    .flex_none()
                    .rounded(px(9.))
                    .bg(theme::accent_soft())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(kit::icon("spark", 16., c(theme::ACCENT))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(18.)).line_height(px(21.6)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Ralph"))
                    .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(if plan {
                        "Builds the feature plan task by task · a fresh agent per task"
                    } else {
                        "Autonomous loop · one story per agent run"
                    })),
            )
            .child(div().ml(px(8.)).child(target_seg));
        let mut actions = div().flex().gap(px(8.));
        if self.running() {
            actions = actions
                .child(kit::button("ralph-pause", "Pause after iteration", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.stop(true, cx))))
                .child(kit::button("ralph-stop", "Stop", Kind::DangerOutline).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.stop(false, cx))));
        } else if let Some(st) = st {
            if plan && st.plan_exists {
                actions = actions.child(kit::button("ralph-edit-plan", "Edit plan", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx))));
            }
            if let Some(prd) = &st.prd {
                let off = prd.all_passed() || !st.warnings.is_empty();
                let label = if st.run.is_some() || !self.runs.is_empty() { "▶ Run again" } else { "▶ Start Ralph" };
                actions = actions.child(kit::disabled(kit::button("ralph-start", label, Kind::Primary), off).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    if !off {
                        t.run_open = !t.run_open;
                        cx.notify();
                    }
                })));
            }
        }
        div().flex_none().flex().items_center().justify_between().gap(px(16.)).child(title).child(actions)
    }

    /// `RalphRunModal`'s limits, inline under the header.
    fn render_run_form(&self, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity().downgrade();
        let iters = kit::seg("ralph-iterations", &[(Some(3), "3"), (Some(5), "5"), (Some(10), "10"), (None, "Until done")], &self.max_iterations, move |v, _, cx| {
            let _ = me.update(cx, |t, cx| {
                t.max_iterations = v;
                cx.notify();
            });
        });
        kit::card()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(14.))
            .px(px(16.))
            .py(px(12.))
            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child("Iterations"))
            .child(iters)
            .child(kit::check("ralph-push", "Push after each commit", self.push).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.push = !t.push;
                cx.notify();
            })))
            .child(div().flex_1())
            .child(kit::button("ralph-run-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.run_open = false;
                cx.notify();
            })))
            .child(kit::button("ralph-run", if self.starting { "Starting…" } else { "Start" }, Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                if !t.starting {
                    t.run_open = false;
                    t.start(w, cx)
                }
            })))
    }

    fn render_status(&self, st: &RalphState) -> gpui::Div {
        let prd = st.prd.as_ref();
        let (passed, total) = prd.map(|p| (p.passed(), p.user_stories.len())).unwrap_or((0, 0));
        let noun = self.noun();
        // The live run, else the latest one on disk (`last` in RalphView).
        let last: Option<Value> = st.run.as_ref().and_then(|r| serde_json::to_value(r).ok()).or_else(|| self.runs.first().cloned());
        let running = last.as_ref().is_some_and(|r| r["running"].as_bool() == Some(true)) && st.run.is_some();
        let now = ui::now();
        let (text, dot, color, story) = match &last {
            Some(r) if running && r["limitUntil"].as_u64().is_some_and(|u| u > now) => {
                (format!("Usage limit · retrying in {}", duration(r["limitUntil"].as_u64().unwrap_or(now) - now)), theme::WARN, theme::WARN, None)
            }
            Some(r) if running => {
                let current = r["currentStory"].as_str();
                let story = current.and_then(|id| prd?.user_stories.iter().find(|s| s.id == id)).map(|s| format!("{} {}", s.id, s.title));
                (format!("Running · iteration {}", r["iteration"].as_u64().unwrap_or(1).max(1)), theme::ACCENT, theme::TEXT, story)
            }
            Some(r) => {
                let ok = r["reason"]["kind"].as_str() == Some("complete");
                let label = if total > 0 && passed == total {
                    if noun == "task" { "All tasks are done".to_string() } else { "All stories pass".to_string() }
                } else {
                    format!("Last run: {}", reason_label(&r["reason"]))
                };
                (label, if ok { theme::OK } else { theme::TEXT_FAINT }, if ok { theme::TEXT } else { theme::TEXT_DIM }, None)
            }
            None => ("Ready to start".into(), theme::TEXT_FAINT, theme::TEXT_DIM, None),
        };
        let stat = |value: String, label: &str| {
            div()
                .flex()
                .flex_col()
                .items_end()
                .child(div().text_size(px(15.)).line_height(px(18.)).font_weight(FontWeight(650.)).text_color(c(theme::TEXT)).child(value))
                .child(div().text_size(px(11.)).line_height(px(13.2)).text_color(c(theme::TEXT_FAINT)).child(label.to_string()))
        };
        let mut stats = div().flex().gap(px(22.)).child(stat(format!("{passed}/{total}"), if noun == "task" { "tasks" } else { "stories" }));
        if let Some(r) = &last {
            let commits = r["commits"].as_u64().unwrap_or(0);
            stats = stats.child(stat(commits.to_string(), if commits == 1 { "commit" } else { "commits" }));
            if let Some(m) = r["model"].as_str() {
                stats = stats.child(stat(m.to_string(), "model"));
            }
            let cost = r["costUsd"].as_f64().unwrap_or(0.);
            if cost > 0. {
                stats = stats.child(stat(format!("${cost:.2}"), "cost"));
            }
            let start = r["startedAt"].as_u64().unwrap_or(now);
            stats = stats.child(stat(duration(r["finishedAt"].as_u64().unwrap_or(now).saturating_sub(start)), "time"));
        }
        div()
            .flex_none()
            .px(px(16.))
            .py(px(14.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .rounded(px(theme::size::RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_w_0()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(color))
                    .child(
                        div().size(px(9.)).flex_none().rounded_full().bg(c(dot)).when(running, |d| {
                            d.shadow(vec![gpui::BoxShadow {
                                color: theme::accent_soft().into(),
                                offset: gpui::point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(4.),
                                inset: false,
                            }])
                        }),
                    )
                    .child(text)
                    .children(story.map(|s| div().max_w(px(520.)).truncate().font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_DIM)).child(s))),
            )
            .child(stats)
    }

    fn render_stories(&self, st: &RalphState, cx: &mut Context<Self>) -> gpui::Div {
        let current = st.run.as_ref().filter(|r| r.running).and_then(|r| r.current_story.clone());
        let mut stories = st.prd.as_ref().map(|p| p.user_stories.clone()).unwrap_or_default();
        stories.sort_by_key(|s| s.priority);
        let noun = self.noun();
        let rows = stories.into_iter().map(|s| {
            let now = current.as_deref() == Some(s.id.as_str());
            let open = self.expanded.as_deref() == Some(s.id.as_str());
            let repo = s.extra.get("repo").and_then(|r| r.as_str()).filter(|r| !r.is_empty()).map(str::to_string);
            let icon = if now {
                kit::icon("spinner", 12., c(theme::ACCENT))
            } else if s.passes {
                kit::icon("check", 12., c(theme::OK))
            } else {
                kit::icon("circle", 12., c(theme::TEXT_FAINT))
            };
            let id = s.id.clone();
            let mut detail = div().pl(px(34.)).pr(px(12.)).pb(px(12.)).pt(px(4.)).text_size(px(12.5)).line_height(px(18.75)).text_color(c(theme::TEXT_DIM));
            if !s.description.is_empty() {
                detail = detail.child(div().mb(px(8.)).child(s.description.clone()));
            }
            for a in &s.acceptance_criteria {
                detail = detail.child(div().flex().gap(px(6.)).child("•").child(a.clone()));
            }
            if !s.notes.is_empty() {
                detail = detail.child(div().mt(px(8.)).italic().text_color(c(theme::TEXT_FAINT)).child(s.notes.clone()));
            }
            div()
                .rounded(px(7.))
                .when(now, |d| d.bg(theme::accent_soft()))
                .child(
                    div()
                        .id(SharedString::from(format!("ralph-story:{}", s.id)))
                        .relative()
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded(px(7.))
                        .text_size(px(13.)).line_height(px(19.))
                        .cursor_pointer()
                        .when(open, |d| d.bg(c(theme::PANEL_RAISED)))
                        .when(!now, |d| d.hover(|s| s.bg(c(theme::PANEL_RAISED))))
                        .child(div().w(px(14.)).flex_none().flex().justify_center().child(icon))
                        .child(story_id(s.id.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(c(if s.passes { theme::TEXT_FAINT } else { theme::TEXT }))
                                .child(if s.title.is_empty() { format!("Untitled {noun}") } else { s.title.clone() }),
                        )
                        .children(repo.map(|r| {
                            div()
                                .flex_none()
                                .px(px(6.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .bg(c(theme::PANEL_RAISED))
                                .font_family(theme::MONO_FONT)
                                .text_size(px(10.5))
                                .text_color(c(theme::TEXT_DIM))
                                .child(r)
                        }))
                        .child(probe(format!("ralph-story:{}", s.id)))
                        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            t.expanded = if t.expanded.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                            cx.notify();
                        })),
                )
                .when(open, |d| d.child(detail))
        });
        let mut col = div()
            .w(px(380.))
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(px(theme::size::RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .overflow_hidden()
            .child(col_head(if noun == "task" { "Tasks" } else { "Stories" }))
            .child(div().id("ralph-stories").flex_1().min_h_0().overflow_y_scroll().p(px(6.)).flex().flex_col().gap(px(1.)).children(rows));
        if !self.runs.is_empty() {
            let runs = self.runs.iter().take(8).map(|r| {
                let id = r["id"].as_str().unwrap_or("").to_string();
                let running = r["running"].as_bool() == Some(true);
                let ok = r["reason"]["kind"].as_str() == Some("complete");
                let active = self.viewing.as_ref().is_some_and(|(v, _)| *v == id);
                let commits = r["commits"].as_u64().unwrap_or(0);
                let cost = r["costUsd"].as_f64().unwrap_or(0.);
                let when = r["startedAt"].as_i64().map(local_time).unwrap_or_default();
                let mut meta = if running { "running".to_string() } else { format!("{commits} commit{}", if commits == 1 { "" } else { "s" }) };
                if cost > 0. {
                    meta += &format!(" · ${cost:.2}");
                }
                let rid = id.clone();
                div()
                    .id(SharedString::from(format!("ralph-run:{id}")))
                    .relative()
                    .px(px(14.))
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
                    .when(active, |d| d.bg(c(theme::PANEL_RAISED)))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                    .child(div().size(px(7.)).flex_none().rounded_full().bg(c(if running {
                        theme::ACCENT
                    } else if ok {
                        theme::OK
                    } else {
                        theme::TEXT_FAINT
                    })))
                    .child(div().flex_1().child(when))
                    .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(meta))
                    .child(probe(format!("ralph-run:{id}")))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.view_run(rid.clone(), running, w, cx)))
            });
            col = col.child(
                div()
                    .id("ralph-runs")
                    .flex_none()
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .child(col_head("Runs").border_b_0().pb(px(4.)))
                    .children(runs),
            );
        }
        col
    }

    fn render_activity(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (events, past) = match &self.viewing {
            Some((_, events)) => (events.as_slice(), true),
            None => (self.feed.as_slice(), false),
        };
        let mut head = col_head(if past { "Past run" } else { "Activity" });
        if past {
            head = head.child(kit::mini("ralph-latest", "Back to latest", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.viewing = None;
                cx.notify();
            })));
        }
        let live = self.running() && !past;
        let body: gpui::AnyElement = if events.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().max_w(px(420.)).text_center().line_height(px(20.)).text_color(c(theme::TEXT_FAINT)).child(if live {
                    "Starting the agent…"
                } else {
                    "When Ralph runs, every file it reads or edits, every command, commit and passing story shows up here as it happens."
                }))
                .into_any_element()
        } else {
            div().flex().flex_col().children(group_by_iteration(events).iter().map(|g| iteration_block(g, live))).into_any_element()
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(px(theme::size::RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .overflow_hidden()
            .child(head)
            .child(div().id("ralph-feed").flex_1().min_h_0().overflow_y_scroll().px(px(16.)).pt(px(8.)).pb(px(20.)).text_size(px(12.5)).child(body))
    }

    fn render_empty(&self, st: &RalphState, cx: &mut Context<Self>) -> gpui::Div {
        let plan = self.target == PLAN;
        let (title, text, steps): (&str, &str, [(&str, &str); 3]) = if plan {
            (
                if st.plan_exists { "The plan has no tasks yet" } else { "Plan it, then let Ralph build it" },
                "Ralph works through the feature plan one task at a time, across its repositories. Each task runs in a fresh agent session that implements it, runs the checks and commits; Orbit ticks it off in the plan. You watch every step here.",
                [
                    ("Plan the feature", "An interview, an AI draft or your own: small tasks, each in one repo, with checks."),
                    ("Review the tasks", "Edit PLAN.md freely: Ralph re-reads it before every task."),
                    ("Start Ralph", "Set limits, then follow tool calls, commits and cost live."),
                ],
            )
        } else {
            (
                "Plan it, then let Ralph build it",
                "Ralph works through a PRD one small story at a time. Each story runs in a fresh agent session that implements it, runs the checks and commits, while you watch every step here.",
                [
                    ("Describe the feature", "Orbit can interview you to settle scope and edge cases."),
                    ("Review the stories", "The PRD is split into small, ordered stories with checkable criteria."),
                    ("Start Ralph", "Set limits, then follow tool calls, commits and cost live."),
                ],
            )
        };
        let mut card = kit::card()
            .max_w(px(560.))
            .p(px(28.))
            .gap(px(12.))
            .items_center()
            .child(div().size(px(44.)).rounded(px(11.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(kit::icon("spark", 22., c(theme::ACCENT))))
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(title))
            .child(div().text_center().text_size(px(13.)).line_height(px(20.)).text_color(c(theme::TEXT_DIM)).child(text))
            .child(div().w_full().flex().flex_col().gap(px(8.)).children(steps.iter().enumerate().map(|(i, (h, d))| {
                div()
                    .flex()
                    .gap(px(10.))
                    .child(div().size(px(20.)).flex_none().rounded_full().bg(c(theme::PANEL_RAISED)).flex().items_center().justify_center().text_size(px(11.)).text_color(c(theme::TEXT_DIM)).child((i + 1).to_string()))
                    .child(div().flex().flex_col().child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(*h)).child(kit::hint(*d)))
            })));
        if plan && st.plan_exists {
            card = card.child(kit::button("ralph-open-plan", "Open PLAN.md", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx))));
        } else if plan {
            card = card.child(kit::hint("Plan the feature from the workspace page (Plan), then come back here."));
        } else {
            card = card.child(kit::hint(format!("No scripts/ralph/prd.json in {} yet. Writing a PRD is in the Tauri app for now.", self.target)));
        }
        div().flex_1().flex().items_center().justify_center().child(card)
    }
}

impl Render for RalphPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = div().size_full().flex().flex_col().gap(px(14.)).px(px(28.)).pt(px(22.)).pb(px(18.)).overflow_hidden();
        let st = match &self.state {
            Load::Ready(s) => s,
            Load::Failed(e) => return view.child(self.render_header(None, cx)).child(kit::error_box(e.clone())),
            _ => return view.child(self.render_header(None, cx)).child(kit::skeleton(gpui::relative(1.), 64.)),
        };
        view = view.child(self.render_header(Some(st), cx));
        if self.run_open && !self.running() {
            view = view.child(self.render_run_form(cx));
        }
        let Some(prd) = &st.prd else { return view.child(self.render_empty(st, cx)) };
        let current = st.run.as_ref().filter(|r| r.running).and_then(|r| r.current_story.clone());
        let mut stories = prd.user_stories.clone();
        stories.sort_by_key(|s| s.priority);
        view = view.child(self.render_status(st)).child(div().flex_none().h(px(6.)).flex().gap(px(3.)).children(stories.iter().map(|s| {
            let now = current.as_deref() == Some(s.id.as_str());
            div().flex_1().h_full().rounded(px(3.)).bg(c(if now {
                theme::ACCENT
            } else if s.passes {
                theme::OK
            } else {
                theme::PANEL_RAISED
            }))
        })));
        if self.target == PLAN && !st.warnings.is_empty() {
            view = view.child(
                div()
                    .flex_none()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(kit::alpha(theme::WARN, 0.35))
                    .bg(kit::alpha(theme::WARN, 0.08))
                    .text_size(px(12.5))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("Fix the plan before starting:"))
                    .children(st.warnings.iter().map(|w| div().text_color(c(theme::TEXT_DIM)).child(format!("• {w}"))))
                    .child(div().child(kit::mini("ralph-warn-edit", "Edit plan", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx))))),
            );
        }
        view.child(div().flex_1().min_h_0().flex().gap(px(14.)).child(self.render_stories(st, cx)).child(self.render_activity(cx)))
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
    fn formats_durations_and_groups_iterations() {
        assert_eq!(super::duration(59), "59s");
        assert_eq!(super::duration(125), "2m 5s");
        assert_eq!(super::duration(7 * 3600 + 19 * 60), "7h 19m");
        let events = vec![
            json!({"kind": "run_start"}),
            json!({"kind": "iteration_start", "n": 1, "story": {"id": "T1", "title": "A"}}),
            json!({"kind": "tool", "name": "Read"}),
            json!({"kind": "iteration_end", "n": 1, "commits": 1}),
        ];
        let groups = super::group_by_iteration(&events);
        assert_eq!(groups.len(), 2);
        assert_eq!((groups[1].n, groups[1].events.len(), groups[1].end.is_some()), (1, 1, true));
    }

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
