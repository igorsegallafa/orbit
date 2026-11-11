//! The Start Ralph modal (`RalphRunModal.tsx`): scope, stop-after story,
//! agent and model for this run, extra instructions, the workspace's
//! iteration prompt, and the safety limits. The last config is remembered
//! per workspace in `<config dir>/native-ralph.json`.
use super::super::kit::{self as pkit, Kind};
use super::feed::{repo_tag, story_id};
use super::{watch, RalphPage};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::workspace_ui::kit;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, FontWeight, SharedString, Window};
use gpui_component::input::{InputState, Textarea, TextareaState};
use orbit_core::ralph::prd::Prd;
use orbit_core::ralph::RunConfig;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

const AGENTS: [(&str, &str); 5] = [("", "From AI settings"), ("claude", "Claude Code"), ("opencode", "OpenCode"), ("omp", "omp"), ("codex", "Codex")];

fn remembered_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("native-ralph.json"))
}

fn remembered_all() -> BTreeMap<String, RunConfig> {
    remembered_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// The last config started in this scope (`rememberedConfig`), never with a stop-after story.
pub fn remembered(scope: &str, fallback: &RunConfig) -> RunConfig {
    let mut config = remembered_all().remove(scope).unwrap_or_else(|| fallback.clone());
    config.limits.stop_after_story = None;
    config
}

fn remember(scope: &str, config: &RunConfig) {
    let Some(path) = remembered_path() else { return };
    let mut all = remembered_all();
    all.insert(scope.to_string(), config.clone());
    if let Ok(raw) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        let _ = std::fs::write(path, raw);
    }
}

pub struct RunForm {
    until_complete: bool,
    iterations: Entity<InputState>,
    stop_after: Option<String>,
    agent: Option<String>,
    /// None: the agent's default (the AI settings' model with no agent picked).
    model: Option<String>,
    /// The picked agent's models, once listed.
    models: Vec<String>,
    extra: Entity<TextareaState>,
    show_prompt: bool,
    prompt: Entity<TextareaState>,
    prompt_custom: bool,
    max_hours: Entity<InputState>,
    stall_after: Entity<InputState>,
    timeout: Entity<InputState>,
    idle: Entity<InputState>,
    grace: Entity<InputState>,
    wait_on_limit: bool,
    limit_wait: Entity<InputState>,
    push: bool,
}

fn num_input(value: impl ToString, window: &mut Window, cx: &mut Context<RalphPage>) -> Entity<InputState> {
    let v = value.to_string();
    cx.new(|cx| InputState::new(window, cx).default_value(v))
}

fn parse<T: std::str::FromStr + PartialOrd + Copy>(input: &Entity<InputState>, min: T, fallback: T, cx: &App) -> T {
    let v = input.read(cx).value().trim().parse::<T>().unwrap_or(fallback);
    if v < min {
        min
    } else {
        v
    }
}

impl RunForm {
    pub fn new(config: &RunConfig, prompt: &str, prompt_custom: bool, window: &mut Window, cx: &mut Context<RalphPage>) -> Self {
        let l = &config.limits;
        let extra_text = config.extra_instructions.clone();
        let extra = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(2, 6).placeholder("e.g. Use the project's own test command. Don't touch the public API.");
            s.set_value(extra_text, window, cx);
            s
        });
        let prompt_text = prompt.to_string();
        let prompt = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).rows(14);
            s.set_value(prompt_text, window, cx);
            s
        });
        let form = Self {
            until_complete: l.until_complete,
            iterations: num_input(l.max_iterations, window, cx),
            stop_after: None,
            agent: config.agent.clone().filter(|a| !a.is_empty()),
            model: config.model.clone().filter(|m| !m.is_empty()),
            models: vec![],
            extra,
            show_prompt: false,
            prompt,
            prompt_custom,
            max_hours: num_input(l.max_hours, window, cx),
            stall_after: num_input(l.stall_after, window, cx),
            timeout: num_input(config.iteration_timeout_min, window, cx),
            idle: num_input(config.idle_timeout_min, window, cx),
            grace: num_input(config.exit_grace_sec, window, cx),
            wait_on_limit: l.wait_on_limit,
            limit_wait: num_input(l.limit_wait_min, window, cx),
            push: config.push,
        };
        for input in [&form.iterations, &form.max_hours, &form.stall_after, &form.timeout, &form.idle, &form.grace, &form.limit_wait] {
            watch(input, window, cx);
        }
        form
    }

    pub fn config(&self, base: &RunConfig, cx: &App) -> RunConfig {
        let mut config = base.clone();
        let l = &mut config.limits;
        l.until_complete = self.until_complete;
        l.max_iterations = parse(&self.iterations, 1, l.max_iterations, cx);
        l.stop_after_story = self.stop_after.clone();
        l.max_hours = parse(&self.max_hours, 0., l.max_hours, cx);
        l.stall_after = parse(&self.stall_after, 1, l.stall_after, cx);
        l.wait_on_limit = self.wait_on_limit;
        l.limit_wait_min = parse(&self.limit_wait, 1, l.limit_wait_min, cx);
        config.iteration_timeout_min = parse(&self.timeout, 5, config.iteration_timeout_min, cx);
        config.idle_timeout_min = parse(&self.idle, 0., config.idle_timeout_min, cx);
        config.exit_grace_sec = parse(&self.grace, 0, config.exit_grace_sec, cx);
        config.agent = self.agent.clone();
        config.model = self.model.clone();
        config.extra_instructions = self.extra.read(cx).value().to_string();
        config.push = self.push;
        config
    }

    pub fn set_prompt_custom(&mut self, custom: bool, prompt: Option<String>, window: &mut Window, cx: &mut App) {
        self.prompt_custom = custom;
        if let Some(p) = prompt {
            self.prompt.update(cx, |s, cx| s.set_value(p, window, cx));
        }
        if !custom {
            self.show_prompt = false;
        }
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let config = self.config(&super::default_run_config(), cx);
        let l = &config.limits;
        let _ = writeln!(
            out,
            "  run form: until done {} · iterations {} · stop after {:?} · agent {:?} · model {:?} · extra {:?} · hours {} · stall {} · timeout {} · idle {} · grace {} · wait {} {}min · push {} · prompt open {} custom {}",
            l.until_complete,
            l.max_iterations,
            l.stop_after_story,
            config.agent,
            config.model,
            config.extra_instructions,
            l.max_hours,
            l.stall_after,
            config.iteration_timeout_min,
            config.idle_timeout_min,
            config.exit_grace_sec,
            l.wait_on_limit,
            l.limit_wait_min,
            config.push,
            self.show_prompt,
            self.prompt_custom
        );
    }
}

/// `.run-section h4`.
fn section(title: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child(title.to_uppercase()))
}

/// `.field`: a label over its control, with an optional hint.
fn field(label: impl Into<SharedString>, control: impl IntoElement, hint: Option<&str>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .min_w_0()
        .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child(label.into()))
        .child(control)
        .children(hint.map(|h| div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(h.to_string())))
}

/// `.choice`: a radio card.
fn choice(id: &'static str, on: bool, title: String, text: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex_1()
        .flex()
        .items_start()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(8.))
        .border_1()
        .cursor_pointer()
        .when(on, |d| d.border_color(c(theme::ACCENT)).bg(theme::accent_soft()))
        .when(!on, |d| d.border_color(c(theme::BORDER)).hover(|s| s.border_color(c(theme::BORDER_STRONG))))
        .child(
            div()
                .mt(px(1.))
                .flex_none()
                .size(px(16.))
                .rounded_full()
                .border(px(1.5))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.bg(c(theme::ACCENT)).border_color(c(theme::ACCENT)).child(pkit::icon("check", 11., c(theme::ON_ACCENT))))
                .when(!on, |d| d.bg(c(theme::BG)).border_color(c(theme::TEXT_FAINT))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(title))
                .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(text)),
        )
        .child(probe(id))
}

/// `.check-item`.
#[derive(Clone, Copy)]
enum Pick {
    StopAfter,
    Agent,
    Model,
}

fn check_item(id: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div().id(id).relative().flex().items_center().gap(px(8.)).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).cursor_pointer().child(pkit::checkbox(on)).child(probe(id))
}

impl RalphPage {
    pub(super) fn run_form(&mut self) -> Option<&mut RunForm> {
        match &mut self.modal {
            Some(super::Modal::Run(f)) => Some(f),
            _ => None,
        }
    }

    pub(super) fn open_run_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.state.ready() else { return };
        let config = remembered(&self.scope, &st.default_config);
        let (prompt, custom) = (st.prompt.clone(), st.prompt_custom);
        self.modal = Some(super::Modal::Run(Box::new(RunForm::new(&config, &prompt, custom, window, cx))));
        self.load_run_models(window, cx);
        cx.notify();
    }

    /// Lists the models of the form's agent (the AI settings' when none is picked).
    fn load_run_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.run_form().map(|f| f.agent.clone()) else { return };
        super::super::ui::bg(
            cx,
            move || {
                let agent = agent.or_else(|| orbit_core::config::Config::load().ok().map(|c| c.ai.agent)).unwrap_or_default();
                orbit_core::agent::list_models(&agent)
            },
            |t: &mut Self, list, _, cx| {
                if let Some(f) = t.run_form() {
                    f.models = list;
                }
                cx.notify();
            },
            window,
        );
    }

    pub(super) fn submit_run_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(base) = self.state.ready().map(|s| s.default_config.clone()) else { return };
        let Some(f) = self.run_form() else { return };
        let config = f.config(&base, cx);
        remember(&self.scope, &config);
        self.modal = None;
        self.start(config, window, cx);
    }

    /// Saves the workspace's iteration prompt; None restores the default.
    fn save_prompt(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.run_form() else { return };
        let text = (!restore).then(|| f.prompt.read(cx).value().to_string());
        let (scope, repo, plan) = (self.scope.clone(), self.repo.clone(), self.is_plan());
        super::super::ui::bg(
            cx,
            move || {
                orbit_core::ralph::save_prompt(scope.clone(), text)?;
                if plan {
                    orbit_core::ralph::plan_state(scope)
                } else {
                    orbit_core::ralph::state(scope, repo)
                }
            },
            move |t: &mut Self, r, w, cx| match r {
                Ok(st) => {
                    t.host.toast(crate::toast::ToastKind::Success, if restore { "Default prompt restored" } else { "Prompt saved" }, None, cx);
                    let (custom, prompt) = (st.prompt_custom, st.prompt.clone());
                    t.state = crate::review::Load::Ready(st);
                    if let Some(f) = t.run_form() {
                        f.set_prompt_custom(custom, Some(prompt), w, cx);
                    }
                }
                Err(e) => t.host.error("Couldn't save the prompt", e, cx),
            },
            window,
        );
    }

    fn pick_menu(&self, items: Vec<(String, Option<String>)>, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        let rows = items
            .into_iter()
            .map(|(label, value)| {
                let me = me.clone();
                let run: super::super::Action = std::rc::Rc::new(move |window, cx: &mut App| {
                    let value = value.clone();
                    let _ = me.update(cx, |t, cx| {
                        let Some(f) = t.run_form() else { return };
                        match pick {
                            Pick::StopAfter => f.stop_after = value,
                            Pick::Model => f.model = value,
                            Pick::Agent if f.agent != value => {
                                // A model names one agent's model: the new agent starts on its default.
                                (f.agent, f.model, f.models) = (value, None, vec![]);
                                t.load_run_models(window, cx);
                            }
                            Pick::Agent => {}
                        }
                        cx.notify();
                    });
                });
                (label, run)
            })
            .collect();
        self.host.menu(rows, window, cx);
    }

    pub(super) fn render_run_form(&self, prd: &Prd, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(super::Modal::Run(f)) = &self.modal else { return div().into_any_element() };
        let plan = self.is_plan();
        let noun = self.noun();
        let mut pending: Vec<_> = prd.user_stories.iter().filter(|s| !s.passes).collect();
        pending.sort_by_key(|s| s.priority);
        let next = pending.first();
        let next_box = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::BG))
            .text_size(px(13.))
            .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child("NEXT UP"))
            .children(next.map(|s| story_id(s.id.clone())))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(next.map(|s| s.title.clone()).unwrap_or_default()))
            .children(next.and_then(|s| s.extra.get("repo")).and_then(|r| r.as_str()).filter(|r| plan && !r.is_empty()).map(|r| repo_tag(r.to_string())))
            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(format!("{} of {} left", pending.len(), prd.user_stories.len())));

        let stop_label = f
            .stop_after
            .as_ref()
            .and_then(|id| pending.iter().find(|s| &s.id == id).map(|s| format!("{} · {}", s.id, s.title)))
            .unwrap_or_else(|| "Don't stop early".into());
        let stop_items: Vec<(String, Option<String>)> =
            std::iter::once(("Don't stop early".to_string(), None)).chain(pending.iter().map(|s| (format!("{} · {}", s.id, s.title), Some(s.id.clone())))).collect();
        let agent_label = AGENTS.iter().find(|(v, _)| Some(*v) == f.agent.as_deref().or(Some(""))).map(|(_, l)| *l).unwrap_or("From AI settings");
        let agent_items: Vec<(String, Option<String>)> = AGENTS.iter().map(|(v, l)| (l.to_string(), Some(v.to_string()).filter(|v| !v.is_empty()))).collect();
        let default_model = match f.agent.as_deref() {
            Some(a) => format!("Default ({})", orbit_core::settings::default_model(a)),
            None => "From AI settings".into(),
        };
        let model_label = f.model.clone().unwrap_or_else(|| default_model.clone());
        let model_items: Vec<(String, Option<String>)> = std::iter::once((default_model, None))
            .chain(f.model.iter().filter(|m| !f.models.contains(m)).map(|m| (m.clone(), Some(m.clone()))))
            .chain(f.models.iter().map(|m| (m.clone(), Some(m.clone()))))
            .collect();

        let scope = section("Scope")
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(choice("ralph-run-fixed", !f.until_complete, "Fixed iterations".into(), format!("Each iteration is one agent run on one {noun}.")).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = t.run_form() {
                            f.until_complete = false;
                        }
                        cx.notify();
                    })))
                    .child(
                        choice("ralph-run-until", f.until_complete, if plan { "Until every task is done" } else { "Until every story passes" }.into(), "Still bounded by the safety limits below.".into()).on_click(cx.listener(
                            |t, _: &ClickEvent, _, cx| {
                                if let Some(f) = t.run_form() {
                                    f.until_complete = true;
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(12.))
                    .child(field("Iterations", pkit::disabled(pkit::field("ralph-run-iterations", &f.iterations, window, cx), f.until_complete), None))
                    .child(field(
                        format!("Stop after {noun}"),
                        pkit::select("ralph-run-stop-after", stop_label).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.pick_menu(stop_items.clone(), Pick::StopAfter, w, cx))),
                        None,
                    )),
            );

        let prompt_link = (!plan).then(|| {
            pkit::link("ralph-run-prompt-toggle", format!("{} the iteration prompt{}", if f.show_prompt { "Hide" } else { "Customize" }, if f.prompt_custom { " · customized for this workspace" } else { "" }))
                .px(px(0.))
                .text_size(px(12.))
                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    if let Some(f) = t.run_form() {
                        f.show_prompt = !f.show_prompt;
                    }
                    cx.notify();
                }))
        });
        let prompt_edit = (!plan && f.show_prompt).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().id("ralph-run-prompt").relative().font_family(theme::MONO_FONT).text_size(px(11.)).child(Textarea::new(&f.prompt)).child(probe("ralph-run-prompt")))
                .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child("Placeholders: {prd_path} {progress_path} {branch} {repo}. Applies to every repo of this workspace."))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(pkit::button("ralph-prompt-save", "Save prompt", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.save_prompt(false, w, cx))))
                        .when(f.prompt_custom, |d| {
                            d.child(pkit::button("ralph-prompt-restore", "Restore default", Kind::DangerOutline).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.save_prompt(true, w, cx))))
                        }),
                )
        });
        let agent = section("Agent")
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(12.))
                    .child(field("Agent", pkit::select("ralph-run-agent", agent_label).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.pick_menu(agent_items.clone(), Pick::Agent, w, cx))), None))
                    .child(field("Model", pkit::select("ralph-run-model", model_label).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.pick_menu(model_items.clone(), Pick::Model, w, cx))), None)),
            )
            .child(field("Extra instructions for this run", div().id("ralph-run-extra").relative().child(Textarea::new(&f.extra)).child(probe("ralph-run-extra")), None))
            .children(prompt_link)
            .children(prompt_edit);

        let warning = div()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(pkit::alpha(theme::WARN, 0.35))
            .bg(pkit::alpha(theme::WARN, 0.07))
            .text_size(px(12.))
            .line_height(px(17.))
            .text_color(c(theme::TEXT_DIM))
            .child(format!(
                "Each iteration runs the agent with every permission: it edits files, runs commands and commits without asking. Review the {noun}s first, and keep an eye on the activity feed."
            ));

        let limit_wait = div().w(px(64.)).child(pkit::field_sized("ralph-run-wait-min", &f.limit_wait, 26., 12., window, cx));
        let limits = section("Safety limits")
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap(px(12.))
                    .child(field("Time budget (hours)", pkit::field("ralph-run-hours", &f.max_hours, window, cx), Some("0 = no limit")))
                    .child(field("Stop when stuck after", pkit::field("ralph-run-stall", &f.stall_after, window, cx), Some("iterations without a commit")))
                    .child(field("Max per iteration (min)", pkit::field("ralph-run-timeout", &f.timeout, window, cx), Some("kills a hung agent"))),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(12.))
                    .child(field("Idle timeout (min)", pkit::field("ralph-run-idle", &f.idle, window, cx), Some("no agent output this long kills its processes (4× while a command runs) · 0 = off")))
                    .child(field("Exit grace (s)", pkit::field("ralph-run-grace", &f.grace, window, cx), Some("a finished agent still running after this is killed · 0 = off"))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(20.))
                    .gap_y(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .text_color(c(theme::TEXT_DIM))
                            .child(check_item("ralph-run-wait", f.wait_on_limit).child("On usage limits, wait").on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                if let Some(f) = t.run_form() {
                                    f.wait_on_limit = !f.wait_on_limit;
                                }
                                cx.notify();
                            })))
                            .child(pkit::disabled(limit_wait, !f.wait_on_limit))
                            .child("min and retry"),
                    )
                    .child(check_item("ralph-run-push", f.push).child("Push after each iteration with commits").on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = t.run_form() {
                            f.push = !f.push;
                        }
                        cx.notify();
                    }))),
            );

        let title = if plan { "Start Ralph".to_string() } else { format!("Start Ralph · {}", self.repo) };
        kit::modal("ralph-run-modal", 720.).max_h(window.viewport_size().height * 0.9)
            .child(kit::modal_header(title))
            .child(
                div()
                    .id("ralph-run-body")
                    .flex_1()
                    .min_h_0()
                    .max_h(window.viewport_size().height * 0.72)
                    .overflow_y_scroll()
                    .px(px(16.))
                    .py(px(18.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .text_size(px(13.))
                    .child(next_box)
                    .child(scope)
                    .child(agent)
                    .child(warning)
                    .child(limits),
            )
            .child(
                kit::modal_footer()
                    .child(pkit::button("ralph-run-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.modal = None;
                        cx.notify();
                    })))
                    .child(pkit::button("ralph-run-start", "▶ Start", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.submit_run_form(w, cx)))),
            )
            .into_any_element()
    }
}
