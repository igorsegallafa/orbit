//! Ralph (`RalphView.tsx` and its modals): the autonomous loop. In a
//! workspace it builds the feature plan (PLAN.md, one agent per task); on a
//! repository (scope `@repo`) it works through the clone's PRD
//! (`scripts/ralph/prd.json`, one agent per story), which it can write
//! (`prd_form`) and edit (`editor`). Start (`run_form`), pause and stop, the
//! run's state, its live feed (`feed`) from orbit-core's `ralph-event`s,
//! and past runs. Every entry point opens the same tab through `open_ralph`.
mod editor;
mod feed;
mod prd_form;
mod run_form;

use super::kit::{self, Kind};
use super::ui;
use super::{Host, PageView};
use crate::app::OrbitApp;
use crate::motion;
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use editor::PrdEditor;
use feed::{describe, duration, group_by_iteration, iteration_block, local_time, reason, reason_label, repo_tag, story_id};
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, FontWeight, ScrollHandle, SharedString, Task, Window};
use orbit_core::events::Events;
use orbit_core::ralph::{self, RalphState, RunConfig};
use prd_form::PrdForm;
use run_form::RunForm;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The plan run's "repo" (core's `PLAN_REPO`).
const PLAN: &str = "plan";
const FEED_MAX: usize = 400;

/// Open Ralph pages and the app notifier get every `(event name, payload)`.
type Subscribers = Mutex<Vec<UnboundedSender<(String, Value)>>>;

fn subscribers() -> &'static Subscribers {
    static S: OnceLock<Subscribers> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn subscribe() -> futures::channel::mpsc::UnboundedReceiver<(String, Value)> {
    let (tx, rx) = unbounded();
    subscribers().lock().unwrap().push(tx);
    rx
}

/// The sink runs and PRD generation report to (`ralph-event`, `ralph-prd-progress`).
fn events() -> Events {
    Events::new(|name: &str, payload: Value| {
        if name == "ralph-event" || name == "ralph-prd-progress" {
            subscribers().lock().unwrap().retain(|tx| tx.unbounded_send((name.to_string(), payload.clone())).is_ok());
        }
    })
}

fn default_run_config() -> RunConfig {
    RunConfig::default()
}

/// Repaints a page when one of its inputs changes.
fn watch<T: gpui::EventEmitter<gpui_component::input::InputEvent>>(input: &gpui::Entity<T>, window: &mut Window, cx: &mut Context<RalphPage>) {
    cx.subscribe_in(input, window, |_, _, ev: &gpui_component::input::InputEvent, _, cx| {
        if matches!(ev, gpui_component::input::InputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
}

/// Opens (or shows) Ralph for `scope`: a workspace name (its plan) or
/// `@repo` (that repository's PRD). Every Ralph entry point lands here.
pub fn open_ralph(app: &mut OrbitApp, scope: &str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let scope = scope.to_string();
    super::open(app, format!("page:ralph:{scope}"), move |h, w, cx| RalphPage::new(h, scope, w, cx), window, cx);
}

/// A run ending matters when no Ralph tab is open: a toast and an inbox
/// item (`ralph:<scope>`, which opens the tab), once per app.
fn ensure_notifier(host: &Host, cx: &mut App) {
    static ON: AtomicBool = AtomicBool::new(false);
    if ON.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut rx = subscribe();
    let app = host.0.clone();
    cx.spawn(async move |cx| {
        while let Some((name, payload)) = rx.next().await {
            let ev = &payload["event"];
            if name != "ralph-event" || ev["kind"].as_str() != Some("stopped") {
                continue;
            }
            let key = payload["key"].as_str().unwrap_or("");
            let (scope, repo) = key.split_once('/').unwrap_or((key, ""));
            let ok = ev["reason"]["kind"].as_str() == Some("complete");
            let n = ev["iterations"].as_u64().unwrap_or(0);
            let detail = format!("{} · {n} iteration(s)", if scope.starts_with('@') { repo.to_string() } else { format!("{scope} · {repo}") });
            let title = if ok { "Ralph finished every story".to_string() } else { format!("Ralph stopped: {}", reason_label(&ev["reason"])) };
            let inbox_key = format!("ralph:{scope}");
            let alive = app.update(cx, |a, cx| {
                a.toast(if ok { ToastKind::Success } else { ToastKind::Info }, title.clone(), Some(detail.clone().into()), cx);
                crate::sessions::store(cx).update(cx, |s, cx| s.notify(inbox_key, ok, title, detail, cx));
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

pub enum Modal {
    Run(Box<RunForm>),
    Prd(Box<PrdForm>),
}

pub struct RalphPage {
    host: Host,
    /// A workspace name, or `@repo`.
    scope: String,
    /// `plan` in a workspace, else the repo.
    repo: String,
    /// The workspace folder (PLAN.md lives there).
    plan_dir: Option<PathBuf>,
    state: Load<RalphState>,
    feed: Vec<Value>,
    runs: Vec<Value>,
    starting: bool,
    pushing: bool,
    /// The story whose details show.
    expanded: Option<String>,
    /// A past run's id and events, shown instead of the live feed.
    viewing: Option<(String, Vec<Value>)>,
    modal: Option<Modal>,
    editor: Option<PrdEditor>,
    feed_scroll: ScrollHandle,
    _events: Task<()>,
    _poll: Task<()>,
}

impl RalphPage {
    pub fn new(host: Host, scope: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_notifier(&host, cx);
        let mut rx = subscribe();
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Some((name, payload)) = rx.next().await {
                if this.update_in(cx, |t: &mut Self, w, cx| t.on_event(&name, payload, w, cx)).is_err() {
                    break;
                }
            }
        });
        // The run's counters and clock live in core: re-read them while it runs.
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
        let (repo, plan_dir) = match orbit_core::workspace::repo_scope(&scope) {
            Some(r) => (r.to_string(), None),
            None => (PLAN.to_string(), orbit_core::workspace::ws_dir(&scope).ok()),
        };
        let mut page = Self {
            host,
            scope,
            repo,
            plan_dir,
            state: Load::Loading,
            feed: vec![],
            runs: vec![],
            starting: false,
            pushing: false,
            expanded: None,
            viewing: None,
            modal: None,
            editor: None,
            feed_scroll: ScrollHandle::new(),
            _events: events,
            _poll: poll,
        };
        page.load(true, window, cx);
        page
    }

    fn key(&self) -> String {
        format!("{}/{}", self.scope, self.repo)
    }

    fn is_plan(&self) -> bool {
        self.repo == PLAN && self.plan_dir.is_some()
    }

    fn running(&self) -> bool {
        self.state.ready().and_then(|s| s.run.as_ref()).is_some_and(|r| r.running)
    }

    fn noun(&self) -> &'static str {
        if self.is_plan() {
            "task"
        } else {
            "story"
        }
    }

    /// Reads the state; `feed` also replaces the feed with the run's events.
    fn load(&mut self, feed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (scope, repo, plan) = (self.scope.clone(), self.repo.clone(), self.is_plan());
        ui::bg(
            cx,
            move || {
                let state = if plan { ralph::plan_state(scope.clone()) } else { ralph::state(scope.clone(), repo) };
                (state, ralph::past_runs(scope).unwrap_or_default())
            },
            move |t: &mut Self, (state, runs), _, _| {
                if feed {
                    t.feed = state.as_ref().ok().and_then(|s| s.run.as_ref()).map(|r| r.events.iter().cloned().collect()).unwrap_or_default();
                    t.feed_scroll.scroll_to_bottom();
                }
                t.state = state.map_or_else(Load::Failed, Load::Ready);
                let repo = t.repo.clone();
                t.runs = runs.into_iter().filter(|r| r["repo"].as_str() == Some(repo.as_str())).collect();
            },
            window,
        );
    }

    /// Whether the feed is scrolled to its end (it then follows new lines).
    fn at_bottom(&self) -> bool {
        let max = self.feed_scroll.max_offset().y;
        max - (-self.feed_scroll.offset().y) < px(40.)
    }

    fn on_event(&mut self, name: &str, payload: Value, window: &mut Window, cx: &mut Context<Self>) {
        if name == "ralph-prd-progress" {
            if let Some(Modal::Prd(f)) = &mut self.modal {
                f.push_log(payload.as_str().unwrap_or("").to_string());
                cx.notify();
            }
            return;
        }
        if payload["key"].as_str() != Some(self.key().as_str()) {
            return;
        }
        let follow = self.viewing.is_none() && self.at_bottom();
        let ev = payload["event"].clone();
        let kind = ev["kind"].as_str().unwrap_or("").to_string();
        if kind == "run_start" {
            self.feed.clear();
        }
        self.feed.push(ev);
        if self.feed.len() > FEED_MAX {
            self.feed.drain(..self.feed.len() - FEED_MAX);
        }
        if follow {
            self.feed_scroll.scroll_to_bottom();
        }
        if matches!(kind.as_str(), "iteration_end" | "stopped" | "story_passed" | "run_start") {
            self.load(false, window, cx);
        }
        cx.notify();
    }

    fn start(&mut self, config: RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        self.starting = true;
        self.viewing = None;
        self.feed.clear();
        let (scope, repo, plan) = (self.scope.clone(), self.repo.clone(), self.is_plan());
        ui::bg(
            cx,
            move || if plan { ralph::plan_start(events(), scope, config) } else { ralph::start(events(), scope, repo, config) },
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

    /// The "N commits not pushed" button.
    fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pushing = true;
        let (scope, repo) = (self.scope.clone(), self.repo.clone());
        ui::bg(
            cx,
            move || ralph::push(scope, repo),
            |t: &mut Self, r, w, cx| {
                t.pushing = false;
                match r {
                    Ok(_) => t.host.toast(ToastKind::Success, "Pushed", None, cx),
                    Err(e) => t.host.error("Couldn't push", e, cx),
                }
                t.load(false, w, cx);
            },
            window,
        );
        cx.notify();
    }

    fn stop(&mut self, pause: bool, cx: &mut Context<Self>) {
        let (scope, repo) = (self.scope.clone(), self.repo.clone());
        let r = if pause { ralph::pause(scope, repo) } else { ralph::stop(scope, repo) };
        if let Err(e) = r {
            self.host.error("Couldn't stop Ralph", e, cx);
        } else if pause {
            self.host.toast(ToastKind::Info, "Ralph stops after this iteration", None, cx);
        }
        cx.notify();
    }

    fn open_plan(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dir) = &self.plan_dir {
            self.host.open_file(dir.join(orbit_core::agent::PLAN_FILE), window, cx);
        }
    }

    /// "Plan this feature": the workspace page's Plan modal.
    fn plan_feature(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(app), name) = (self.host.0.upgrade(), self.scope.clone()) else { return };
        window.defer(cx, move |window, cx| {
            app.update(cx, |a, cx| crate::workspace_ui::run_for(a, &name, "workspace.plan", window, cx));
        });
    }

    fn view_run(&mut self, id: String, running: bool, window: &mut Window, cx: &mut Context<Self>) {
        if running {
            self.viewing = None;
            return cx.notify();
        }
        let scope = self.scope.clone();
        let rid = id.clone();
        ui::bg(
            cx,
            move || ralph::run_events(scope, rid),
            move |t: &mut Self, r, _, cx| match r {
                Ok(events) => {
                    t.viewing = Some((id, events));
                    t.feed_scroll.scroll_to_bottom();
                }
                Err(e) => t.host.error("Couldn't read the run", e, cx),
            },
            window,
        );
    }

    fn render_header(&self, st: Option<&RalphState>, cx: &mut Context<Self>) -> gpui::Div {
        let plan = self.is_plan();
        let title = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_w_0()
            .child(div().size(px(34.)).flex_none().rounded(px(9.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(kit::icon("spark", 16., c(theme::ACCENT))))
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
            );
        let mut actions = div().flex().gap(px(8.));
        if self.running() {
            actions = actions
                .child(crate::workspace_ui::kit::hint(kit::button("ralph-pause", "Pause after iteration", Kind::Secondary), "Finish the current iteration, then stop").on_click(cx.listener(
                    |t, _: &ClickEvent, _, cx| t.stop(true, cx),
                )))
                .child(crate::workspace_ui::kit::hint(kit::button("ralph-stop", "Stop", Kind::DangerOutline), "Kill the current iteration now; commits made so far stay").on_click(cx.listener(
                    |t, _: &ClickEvent, _, cx| t.stop(false, cx),
                )));
        } else if let Some(st) = st {
            if plan && st.plan_exists {
                actions = actions.child(
                    crate::workspace_ui::kit::hint(kit::button("ralph-edit-plan", "Edit plan", Kind::Secondary), "Open PLAN.md: add, reorder or reword tasks")
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx))),
                );
            }
            if !plan && st.prd.is_some() {
                actions = actions.child(kit::button("ralph-new-prd", "New PRD", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_prd_form(w, cx))));
            }
            if let Some(prd) = &st.prd {
                let all = prd.all_passed();
                let off = all || self.editor.is_some() || !st.warnings.is_empty() || self.starting;
                let label = if st.run.is_some() || !self.runs.is_empty() { "▶ Run again" } else { "▶ Start Ralph" };
                let mut button = kit::disabled(kit::button("ralph-start", label, Kind::Primary), off);
                if all {
                    button = crate::workspace_ui::kit::hint(button, format!("Every {} is done", self.noun()));
                } else if !st.warnings.is_empty() {
                    button = crate::workspace_ui::kit::hint(button, "Fix the plan first (see above the tasks)");
                }
                actions = actions.child(button.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    if !off {
                        t.open_run_form(w, cx);
                    }
                })));
            }
        }
        div().flex_none().flex().items_center().justify_between().gap(px(16.)).child(title).child(actions)
    }

    fn render_status(&self, st: &RalphState, cx: &mut Context<Self>) -> gpui::Div {
        let prd = st.prd.as_ref();
        let (passed, total) = prd.map(|p| (p.passed(), p.user_stories.len())).unwrap_or((0, 0));
        let noun = self.noun();
        // The live run, else the latest one on disk (`last` in RalphView).
        let last: Option<Value> = st.run.as_ref().and_then(|r| serde_json::to_value(r).ok()).or_else(|| self.runs.first().cloned());
        let running = self.running();
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
        let unpushed = if running { 0 } else { last.as_ref().and_then(|r| r["unpushed"].as_u64()).unwrap_or(0) };
        let push = (unpushed > 0).then(|| {
            let label = if self.pushing { "Pushing…" } else { "Push" };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.))
                .text_color(c(theme::WARN))
                .child(format!("{unpushed} commit{} not pushed", if unpushed == 1 { "" } else { "s" }))
                .child(kit::disabled(kit::button("ralph-push", label, Kind::Secondary), self.pushing).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                    if !t.pushing {
                        t.push(w, cx);
                    }
                })))
        });
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
                    // `.rv-state-run .rv-dot` pulses; the others are still.
                    .child(if running && dot == theme::ACCENT {
                        motion::rv_pulse("ralph-run-dot", 9., 4., c(theme::ACCENT), theme::accent_soft())
                    } else {
                        div().size(px(9.)).flex_none().rounded_full().bg(c(dot)).into_any_element()
                    })
                    .child(text)
                    .children(story.map(|s| div().max_w(px(520.)).truncate().font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_DIM)).child(s))),
            )
            .children(push)
            .child(stats)
    }

    fn render_stories(&self, st: &RalphState, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let current = st.run.as_ref().filter(|r| r.running).and_then(|r| r.current_story.clone());
        let mut stories = st.prd.as_ref().map(|p| p.user_stories.clone()).unwrap_or_default();
        stories.sort_by_key(|s| s.priority);
        let noun = self.noun();
        let editing = self.editor.is_some();
        let rows: Vec<_> = stories.into_iter().map(|s| {
            let now = current.as_deref() == Some(s.id.as_str());
            let open = self.expanded.as_deref() == Some(s.id.as_str());
            let repo = s.extra.get("repo").and_then(|r| r.as_str()).filter(|r| !r.is_empty()).map(str::to_string);
            let icon = if now {
                crate::workspace_ui::kit::spinner(SharedString::from(format!("ralph-spin:{}", s.id)), 12., c(theme::ACCENT)).into_any_element()
            } else if s.passes {
                kit::icon("check", 12., c(theme::OK)).into_any_element()
            } else {
                kit::icon("circle", 12., c(theme::TEXT_FAINT)).into_any_element()
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
                        .text_size(px(13.))
                        .line_height(px(19.))
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
                        .children(repo.map(repo_tag))
                        .child(probe(format!("ralph-story:{}", s.id)))
                        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            t.expanded = if t.expanded.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                            cx.notify();
                        })),
                )
                .when(open, |d| d.child(detail))
        }).collect();
        let mut head = col_head(if noun == "task" { "Tasks" } else { "Stories" });
        if !self.is_plan() {
            head = head.child(if editing {
                self.editor_actions(cx)
            } else {
                let running = self.running();
                div().child(
                    crate::workspace_ui::kit::hint(kit::disabled(kit::mini("ralph-edit", "Edit", Kind::Secondary), running), if running { "Stop Ralph to edit the PRD" } else { "Edit stories, criteria and order" })
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            if !running {
                                t.start_editing(w, cx)
                            }
                        })),
                )
            });
        }
        let list = if editing { self.render_editor(window, cx) } else { div().p(px(6.)).flex().flex_col().gap(px(1.)).children(rows) };
        let mut col = div()
            .when(editing, |d| d.flex_1().min_w(px(420.)))
            .when(!editing, |d| d.w(px(380.)).flex_none())
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(px(theme::size::RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .overflow_hidden()
            .child(head)
            .child(div().id("ralph-stories").flex_1().min_h_0().overflow_y_scroll().child(list));
        if !self.runs.is_empty() && !editing {
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
                t.feed_scroll.scroll_to_bottom();
                cx.notify();
            })));
        }
        let live = self.running() && !past;
        let body: gpui::AnyElement = if events.is_empty() {
            let text: gpui::AnyElement = if live {
                div().flex().items_center().gap(px(10.)).child(crate::workspace_ui::kit::spinner("ralph-starting", 13., c(theme::ACCENT))).child("Starting the agent…").into_any_element()
            } else {
                div().child("When Ralph runs, every file it reads or edits, every command, commit and passing story shows up here as it happens.").into_any_element()
            };
            div().size_full().flex().items_center().justify_center().child(div().max_w(px(420.)).text_center().line_height(px(20.)).text_color(c(theme::TEXT_FAINT)).child(text)).into_any_element()
        } else {
            let run = self.state.ready().and_then(|s| s.run.as_ref());
            let (activity, idle_min, now) = (run.map(|r| &r.activity), run.map_or(0., |r| r.config.idle_timeout_min), ui::now());
            div().flex().flex_col().children(group_by_iteration(events).iter().map(|g| iteration_block(g, live, activity, idle_min, now))).into_any_element()
        };
        let jump = (!events.is_empty() && !self.at_bottom()).then(|| {
            div()
                .id("ralph-jump")
                .absolute()
                .right(px(18.))
                .bottom(px(16.))
                .h(px(30.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(c(theme::BORDER_STRONG))
                .bg(c(theme::PANEL_RAISED))
                .text_size(px(12.))
                .text_color(c(theme::TEXT))
                .shadow(theme::shadow(6., 20., 0.35))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::BORDER_STRONG)))
                .child("↓ Latest")
                .child(probe("ralph-jump"))
                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.feed_scroll.scroll_to_bottom();
                    cx.notify();
                }))
        });
        div()
            .relative()
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
            .child(div().id("ralph-feed").track_scroll(&self.feed_scroll).flex_1().min_h_0().overflow_y_scroll().px(px(16.)).pt(px(8.)).pb(px(20.)).text_size(px(12.5)).child(body))
            .children(jump)
    }

    fn render_empty(&self, st: &RalphState, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let plan = self.is_plan();
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
        let button = if plan && st.plan_exists {
            kit::button("ralph-open-plan", "Open PLAN.md", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx)))
        } else if plan {
            kit::button("ralph-plan-feature", "Plan this feature", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.plan_feature(w, cx)))
        } else {
            kit::button("ralph-create-prd", format!("Create PRD for {}", self.repo), Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_prd_form(w, cx)))
        };
        let card = div()
            .max_w(px(520.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .child(div().size(px(48.)).rounded(px(12.)).bg(theme::accent_soft()).flex().items_center().justify_center().child(kit::icon("spark", 22., c(theme::ACCENT))))
            .child(div().mt(px(4.)).text_size(px(18.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(title))
            .child(div().text_center().text_size(px(13.)).line_height(px(20.8)).text_color(c(theme::TEXT_DIM)).child(text))
            .child(div().w_full().my(px(8.)).flex().flex_col().gap(px(10.)).children(steps.iter().enumerate().map(|(i, (h, d))| {
                div()
                    .flex()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::PANEL))
                    .child(
                        div()
                            .size(px(26.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme::accent_soft())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(c(theme::ACCENT))
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(*h))
                            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(*d)),
                    )
            })))
            .child(button);
        div().id("ralph-empty").flex_1().min_h_0().overflow_y_scroll().flex().items_center().justify_center().child(card)
    }
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

impl Render for RalphPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = div().relative().size_full().flex().flex_col().gap(px(14.)).px(px(28.)).pt(px(22.)).pb(px(18.)).overflow_hidden();
        let st = match &self.state {
            Load::Ready(s) => s,
            Load::Failed(e) => return view.child(self.render_header(None, cx)).child(kit::error_box(e.clone())).into_any_element(),
            _ => return view.child(self.render_header(None, cx)).child(kit::skeleton(gpui::relative(1.), 64.)).into_any_element(),
        };
        view = view.child(self.render_header(Some(st), cx));
        match &st.prd {
            None => view = view.child(self.render_empty(st, cx)),
            Some(prd) => {
                let current = st.run.as_ref().filter(|r| r.running).and_then(|r| r.current_story.clone());
                let mut stories = prd.user_stories.clone();
                stories.sort_by_key(|s| s.priority);
                view = view.child(self.render_status(st, cx)).child(div().flex_none().h(px(6.)).flex().gap(px(3.)).children(stories.iter().map(|s| {
                    let now = current.as_deref() == Some(s.id.as_str());
                    seg(&s.id, now, s.passes)
                })));
                if self.is_plan() && !st.warnings.is_empty() {
                    view = view.child(
                        div()
                            .flex_none()
                            .px(px(12.))
                            .py(px(10.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(kit::alpha(theme::WARN, 0.4))
                            .bg(kit::alpha(theme::WARN, 0.08))
                            .text_size(px(12.5))
                            .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Fix the plan before starting:"))
                            .children(st.warnings.iter().map(|w| div().pl(px(6.)).text_color(c(theme::TEXT_DIM)).child(format!("• {w}"))))
                            .child(kit::mini("ralph-warn-edit", "Edit plan", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_plan(w, cx)))),
                    );
                }
                view = view.child(div().flex_1().min_h_0().flex().gap(px(14.)).child(self.render_stories(st, window, cx)).child(self.render_activity(cx)));
            }
        }
        let modal = match &self.modal {
            Some(Modal::Run(_)) => st.prd.as_ref().map(|prd| self.render_run_form(prd, window, cx)),
            Some(Modal::Prd(_)) => Some(self.render_prd_form(cx)),
            None => None,
        };
        view.when_some(modal, |d, m| d.child(crate::git::scrim(m))).into_any_element()
    }
}

impl PageView for RalphPage {
    fn title(&self) -> SharedString {
        format!("Ralph · {}", self.scope.trim_start_matches('@')).into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(false, window, cx);
    }

    fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "  scope {} · target {}", self.scope, self.repo);
        match &self.state {
            Load::Ready(s) => {
                if let Some(prd) = &s.prd {
                    let _ = writeln!(out, "  prd {} · {}/{} pass · rules {:?}", prd.project, prd.passed(), prd.user_stories.len(), prd.description);
                    let mut stories = prd.user_stories.clone();
                    stories.sort_by_key(|s| s.priority);
                    for st in &stories {
                        let _ = writeln!(out, "    [{}] {} {} ({} criteria)", if st.passes { "x" } else { " " }, st.id, st.title, st.acceptance_criteria.len());
                    }
                } else {
                    let _ = writeln!(out, "  no prd (plan exists {})", s.plan_exists);
                }
                let _ = writeln!(out, "  prompt custom {}", s.prompt_custom);
                if let Some(r) = &s.run {
                    let (idle, warn) = feed::idleness(r.activity.last_event_at, ui::now(), r.config.idle_timeout_min);
                    let _ = writeln!(
                        out,
                        "  activity last event {}s ago warn {warn} · tool {:?} · background {:?} · unpushed {} · idle {} min · grace {}s",
                        idle, r.activity.tool, r.activity.background, r.unpushed, r.config.idle_timeout_min, r.config.exit_grace_sec
                    );
                    let _ = writeln!(
                        out,
                        "  run {} running {} iteration {} story {:?} {}/{} commits {} reason {:?} · agent {:?} model {:?} · until done {} max {} · push {}",
                        r.id,
                        r.running,
                        r.iteration,
                        r.current_story,
                        r.passed,
                        r.total,
                        r.commits,
                        r.reason,
                        r.config.agent,
                        r.config.model,
                        r.config.limits.until_complete,
                        r.config.limits.max_iterations,
                        r.config.push
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
        match &self.modal {
            Some(Modal::Run(f)) => f.dump(out, cx),
            Some(Modal::Prd(f)) => f.dump(out, cx),
            None => {}
        }
        if let Some(ed) = &self.editor {
            ed.dump(out, cx);
        }
        let _ = writeln!(out, "  feed {} events{}", self.feed.len(), if self.viewing.is_some() { " (viewing a past run)" } else { "" });
        for e in self.feed.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev() {
            let _ = writeln!(out, "    {}", describe(e));
        }
        for r in self.runs.iter().take(5) {
            let _ = writeln!(out, "  past run {} {}", r["id"].as_str().unwrap_or(""), reason(&r["reason"]));
        }
    }
}

/// `.rv-seg`: `background 300ms ease` between pending (0), current (1) and
/// done (2); the current one also blinks (`rv-blink`).
fn seg(id: &str, current: bool, done: bool) -> gpui::AnyElement {
    use motion::TweenExt;
    let target = if current { 1. } else if done { 2. } else { 0. };
    let bar = div().size_full().rounded(px(3.)).tween(SharedString::from(format!("rv-seg:{id}")), target, motion::SEG, motion::ease(), |d, v| {
        let (raised, accent, ok) = (c(theme::PANEL_RAISED), c(theme::ACCENT), c(theme::OK));
        d.bg(if v <= 1. { motion::mix(raised, accent, v) } else { motion::mix(accent, ok, v - 1.) })
    });
    // The holder keeps one id either way, so the tween's state survives the blink toggling.
    let holder = div().flex_1().h_full().child(bar);
    let key = SharedString::from(format!("rv-seg-blink:{id}"));
    if current {
        motion::blink(holder, key, motion::RV_BLINK, 0.45)
    } else {
        holder.id(key).into_any_element()
    }
}
