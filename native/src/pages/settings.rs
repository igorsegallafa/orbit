//! Settings (`SettingsPage.tsx`): its own section list with a filter, and the
//! sections AI, Languages, Appearance, Health and Updates. Repositories and
//! Groups are in `repos.rs`. Everything reads and writes `config.yaml`
//! through orbit-core, so the Tauri app sees the same settings.
use super::ui::{self, Tone};
use super::{updates, Host, PageView};
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, SharedString, Subscription, Task, Window};
use gpui_component::input::{InputEvent, InputState};
use orbit_core::config::{AiSettings, Config};
use orbit_core::health::Check;
use orbit_core::lsp::LspStatus;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    Repos,
    Groups,
    Ai,
    Languages,
    Appearance,
    Health,
    Updates,
}

struct SectionDef {
    id: Section,
    slug: &'static str,
    title: &'static str,
    description: &'static str,
    /// Extra words the filter matches besides the title.
    keywords: &'static str,
}

const NAV: &[(&str, &[SectionDef])] = &[
    (
        "WORKSPACE",
        &[
            SectionDef {
                id: Section::Repos,
                slug: "repos",
                title: "Repositories",
                description: "The repositories Orbit manages and the folders it clones them into.",
                keywords: "repos folders clone path services github",
            },
            SectionDef {
                id: Section::Groups,
                slug: "groups",
                title: "Groups",
                description: "Named sets of repositories you can spin up together as one workspace.",
                keywords: "groups sets bundle",
            },
        ],
    ),
    (
        "TOOLS",
        &[
            SectionDef {
                id: Section::Ai,
                slug: "ai",
                title: "AI",
                description: "The agent and model Orbit uses for sessions, commit messages and reviews.",
                keywords: "agent model claude codex prompt",
            },
            SectionDef {
                id: Section::Languages,
                slug: "languages",
                title: "Languages",
                description: "Language servers that power go-to-definition, hovers and diagnostics in the editor.",
                keywords: "lsp language server diagnostics editor",
            },
        ],
    ),
    (
        "SYSTEM",
        &[
            SectionDef {
                id: Section::Appearance,
                slug: "appearance",
                title: "Appearance",
                description: "The app's theme: interface, editor, diffs and terminals.",
                keywords: "theme dark light color dracula nord tokyo catppuccin solarized",
            },
            SectionDef {
                id: Section::Health,
                slug: "health",
                title: "Health",
                description: "Tools Orbit relies on, and whether each one is installed.",
                keywords: "environment doctor install git gh node",
            },
            SectionDef {
                id: Section::Updates,
                slug: "updates",
                title: "Updates",
                description: "The installed version and new releases.",
                keywords: "version release upgrade",
            },
        ],
    ),
];

fn def(section: Section) -> &'static SectionDef {
    NAV.iter().flat_map(|(_, s)| s.iter()).find(|s| s.id == section).expect("every section is listed")
}

const AGENTS: &[(&str, &str)] = &[("claude", "Claude Code"), ("opencode", "OpenCode"), ("omp", "OMP")];

/// The themes of `src/lib/theme.ts`; only Orbit Dark is ported.
const THEMES: &[(&str, bool)] = &[
    ("Orbit Dark", true),
    ("Dracula", true),
    ("Nord", true),
    ("Tokyo Night", true),
    ("Catppuccin Mocha", true),
    ("Orbit Light", false),
    ("Solarized Light", false),
];

pub struct SettingsPage {
    pub(super) host: Host,
    pub section: Section,
    filter: Entity<InputState>,
    pub(super) config: Load<Config>,
    pub(super) repos: super::repos::ReposState,
    pub(super) groups: super::repos::GroupsState,
    models: Load<Vec<String>>,
    test: Option<Load<()>>,
    languages: Load<LspStatus>,
    drafts: HashMap<String, Entity<InputState>>,
    logs: HashMap<u32, Vec<String>>,
    health: Load<Vec<Check>>,
    _poll: Task<()>,
    _subs: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(host: Host, section: Section, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let sub = cx.subscribe_in(&filter, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                if let Some(first) = this.matching(cx).first().map(|s| s.id) {
                    this.select(first, window, cx);
                }
            }
            _ => cx.notify(),
        });
        // Running language servers change on their own.
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let ok = this.update(cx, |t: &mut Self, cx| {
                if t.section == Section::Languages {
                    t.load_languages(cx);
                }
            });
            if ok.is_err() {
                break;
            }
        });
        let mut page = Self {
            repos: super::repos::ReposState::new(window, cx),
            groups: super::repos::GroupsState::new(window, cx),
            host,
            section,
            filter,
            config: Load::Idle,
            models: Load::Idle,
            test: None,
            languages: Load::Idle,
            drafts: HashMap::new(),
            logs: HashMap::new(),
            health: Load::Idle,
            _poll: poll,
            _subs: vec![sub],
        };
        page.load_config(window, cx);
        page.select(section, window, cx);
        page
    }

    pub fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.section = section;
        match section {
            Section::Repos => self.load_folders(window, cx),
            Section::Languages => {
                self.languages = Load::Loading;
                self.load_languages(cx);
            }
            Section::Health if !matches!(self.health, Load::Ready(_)) => self.load_health(window, cx),
            Section::Ai if matches!(self.models, Load::Idle) => self.load_models(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn matching(&self, cx: &App) -> Vec<&'static SectionDef> {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        NAV.iter().flat_map(|(_, s)| s.iter()).filter(|s| q.is_empty() || format!("{} {}", s.title, s.keywords).to_lowercase().contains(&q)).collect()
    }

    // ---------- config ----------

    pub(super) fn load_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::bg(
            cx,
            Config::load,
            |t: &mut Self, r, window, cx| {
                t.config = r.map_or_else(Load::Failed, Load::Ready);
                t.refresh_clone_status(window, cx);
            },
            window,
        );
    }

    /// Runs a config edit off the UI thread, then shows the saved config.
    pub(super) fn edit(
        &mut self,
        what: &'static str,
        work: impl FnOnce() -> Result<Config, String> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ui::bg(
            cx,
            work,
            move |t: &mut Self, r, window, cx| match r {
                Ok(cfg) => {
                    t.config = Load::Ready(cfg);
                    then(t, window, cx);
                }
                Err(e) => t.host.error(what, e, cx),
            },
            window,
        );
    }

    pub(super) fn cfg(&self) -> Option<&Config> {
        self.config.ready()
    }

    // ---------- AI ----------

    fn ai(&self) -> Option<AiSettings> {
        self.cfg().map(|c| c.ai.clone())
    }

    fn load_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent = self.ai().map(|a| a.agent).unwrap_or_else(|| "claude".into());
        self.models = Load::Loading;
        ui::bg(cx, move || orbit_core::agent::list_models(&agent), |t: &mut Self, list, _, _| t.models = Load::Ready(list), window);
    }

    fn set_ai(&mut self, change: impl FnOnce(&mut AiSettings), reload_models: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut ai) = self.ai() else { return };
        change(&mut ai);
        // Shown at once; the save follows.
        if let Load::Ready(cfg) = &mut self.config {
            cfg.ai = ai.clone();
        }
        self.test = None;
        self.edit(
            "Couldn't save the AI settings",
            move || orbit_core::settings::set_ai(ai),
            move |t, window, cx| {
                if reload_models {
                    t.load_models(window, cx);
                }
            },
            window,
            cx,
        );
    }

    fn pick_agent(&mut self, agent: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai().is_some_and(|a| a.agent == agent) {
            return;
        }
        self.models = Load::Loading;
        self.set_ai(
            |ai| {
                ai.model = orbit_core::settings::default_model(&agent).into();
                ai.agent = agent;
                ai.fast_model = None;
            },
            true,
            window,
            cx,
        );
    }

    /// A menu of the agent's models; `fast` picks the fast model.
    fn model_menu(&mut self, fast: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ai) = self.ai() else { return };
        let models = self.models.ready().cloned().unwrap_or_default();
        let mut options: Vec<(String, Option<String>)> = Vec::new();
        if fast {
            let label = if ai.agent == "claude" { "Default (claude-haiku-4-5)" } else { "Same as Model" };
            options.push((label.into(), None));
        } else if !models.contains(&ai.model) {
            // The saved model stays listed even when the agent doesn't list it.
            options.push((ai.model.clone(), Some(ai.model.clone())));
        }
        options.extend(models.into_iter().map(|m| (m.clone(), Some(m))));
        let me = cx.entity().downgrade();
        let items = options
            .into_iter()
            .map(|(label, value)| {
                let me = me.clone();
                let run: super::Action = Rc::new(move |window, cx| {
                    let value = value.clone();
                    let _ = me.update(cx, |t, cx| {
                        t.set_ai(
                            |ai| match (fast, value) {
                                (true, v) => ai.fast_model = v,
                                (false, Some(v)) => ai.model = v,
                                (false, None) => {}
                            },
                            false,
                            window,
                            cx,
                        )
                    });
                });
                (label, run)
            })
            .collect();
        self.host.menu(items, window, cx);
    }

    fn test_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ai) = self.ai() else { return };
        self.test = Some(Load::Loading);
        ui::bg(
            cx,
            move || orbit_core::agent::test_agent(&ai),
            |t: &mut Self, r, _, _| t.test = Some(r.map_or_else(Load::Failed, Load::Ready)),
            window,
        );
    }

    // ---------- languages ----------

    fn load_languages(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = cx.background_spawn(async { orbit_core::lsp::status() }).await;
            let _ = this.update(cx, |t: &mut Self, cx| {
                if let Ok(status) = &r {
                    crate::editor::set_diagnostic_filters(&status.languages, cx);
                }
                t.languages = r.map_or_else(Load::Failed, Load::Ready);
                cx.notify();
            });
        })
        .detach();
    }

    /// The custom command input of a language, created on first render.
    fn draft(&mut self, id: &str, value: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(d) = self.drafts.get(id) {
            return d.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.unwrap_or_default()));
        let lang = id.to_string();
        cx.subscribe_in(&input, window, move |t: &mut Self, input, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let command = input.read(cx).value().trim().to_string();
                t.commit_command(&lang, command, window, cx);
            }
        })
        .detach();
        self.drafts.insert(id.to_string(), input.clone());
        input
    }

    fn commit_command(&mut self, lang: &str, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = self.languages.ready().and_then(|s| s.languages.iter().find(|l| l.id == lang)) else { return };
        let command = (!command.is_empty()).then_some(command);
        if command == l.custom_command {
            return;
        }
        let (id, disabled) = (l.id.clone(), l.disabled);
        self.configure(move || orbit_core::lsp::configure(&id, command, disabled), window, cx);
    }

    fn configure(&mut self, work: impl FnOnce() -> Result<(), String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        ui::bg(
            cx,
            work,
            |t: &mut Self, r, _, cx| match r {
                Ok(()) => t.load_languages(cx),
                Err(e) => t.host.error("Couldn't save the language setting", e, cx),
            },
            window,
        );
    }

    // ---------- health ----------

    fn load_health(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.health = Load::Loading;
        ui::bg(cx, orbit_core::health::run, |t: &mut Self, list, _, _| t.health = Load::Ready(list), window);
    }

    // ---------- rendering ----------

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.matching(cx);
        let mut nav = div()
            .id("settings-nav")
            .w(px(210.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .py_2()
            .child(div().px_2().pb_1().child(ui::input("settings-filter", &self.filter)));
        for (group, sections) in NAV {
            let items: Vec<_> = sections.iter().filter(|s| shown.iter().any(|m| m.id == s.id)).collect();
            if items.is_empty() {
                continue;
            }
            nav = nav.child(ui::nav_title(group));
            for s in items {
                let (id, active) = (s.id, s.id == self.section);
                let name = format!("settings-nav:{}", s.slug);
                nav = nav.child(
                    div()
                        .id(SharedString::from(name.clone()))
                        .relative()
                        .mx_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .when(active, |d| d.bg(theme::accent_soft()).text_color(c(theme::TEXT)))
                        .when(!active, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_HOVER))))
                        .child(s.title)
                        .child(probe(name))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.select(id, w, cx))),
                );
            }
        }
        if shown.is_empty() {
            nav = nav.child(div().px_3().child(ui::faint(format!("No settings match “{}”", self.filter.read(cx).value().trim()))));
        }
        nav
    }

    fn render_ai(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(ai) = self.ai() else { return ui::faint("Loading…").into_any_element() };
        let me = cx.entity().downgrade();
        let agents: Vec<(String, &str)> = AGENTS.iter().map(|(id, l)| (id.to_string(), *l)).collect();
        let agent_seg = ui::segmented("ai-agent", &agents, &ai.agent, move |v, w, cx| {
            let _ = me.update(cx, |t, cx| t.pick_agent(v, w, cx));
        });
        let model_hint = match ai.agent.as_str() {
            "opencode" => "Provider/model as listed by `opencode models`.",
            "omp" => "As listed by `omp models` (fuzzy match supported).",
            _ => "Passed to `claude --model`. Each AI feature uses it unless a run overrides it.",
        };
        let loading = matches!(self.models, Load::Loading);
        let picker = |name: &'static str, label: String, fast: bool, cx: &mut Context<Self>| {
            ui::button(name, if loading { "Loading models…".to_string() } else { format!("{label} ▾") }, Tone::Plain)
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.model_menu(fast, w, cx)))
        };
        let fast_label = ai.fast_model.clone().unwrap_or_else(|| if ai.agent == "claude" { "Default (claude-haiku-4-5)".into() } else { "Same as Model".into() });
        let me = cx.entity().downgrade();
        let remote = ui::segmented("ai-remote", &[(false, "Off"), (true, "On")], &ai.remote_control, move |on, w, cx| {
            let _ = me.update(cx, |t, cx| t.set_ai(|ai| ai.remote_control = on, false, w, cx));
        });
        let test_state = match &self.test {
            Some(Load::Ready(())) => div().flex().items_center().gap_1().child(ui::dot(c(theme::OK))).child(ui::muted("Working")),
            Some(Load::Failed(_)) => div().text_xs().text_color(c(theme::DANGER)).child("Failed"),
            _ => div(),
        };
        let testing = matches!(self.test, Some(Load::Loading));
        let mut connection_text = "Sends a tiny prompt to check the agent is installed and the model answers.".to_string();
        if let Some(Load::Failed(e)) = &self.test {
            connection_text = format!("{connection_text}\n{e}");
        }
        ui::card()
            .child(ui::setting("Agent", "The CLI Orbit runs for plans, commit messages, PR drafts, CI investigation and Ralph.", agent_seg))
            .child(ui::setting("Model", model_hint, picker("ai-model", ai.model.clone(), false, cx)))
            .child(ui::setting("Fast model", "Used for commit messages and PR drafts, where speed matters more than depth.", picker("ai-fast-model", fast_label, true, cx)))
            .when(ai.agent == "claude", |d| {
                d.child(ui::setting(
                    "Remote Control",
                    "Claude sessions opened in Orbit can be continued from claude.ai or the Claude app. Applies to sessions started after the change.",
                    remote,
                ))
            })
            .child(ui::setting(
                "Connection",
                connection_text,
                div().flex().items_center().gap_2().child(test_state).child(
                    ui::button("ai-test", if testing { "Testing…" } else { "Test agent" }, Tone::Plain)
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.test_agent(w, cx))),
                ),
            ))
            .into_any_element()
    }

    fn render_languages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let status = match &self.languages {
            Load::Ready(s) => s.clone(),
            Load::Failed(e) => return ui::error(e.clone()).into_any_element(),
            _ => return ui::faint("Loading…").into_any_element(),
        };
        let names: HashMap<String, String> = status.languages.iter().map(|l| (l.id.clone(), l.name.clone())).collect();
        let mut card = ui::card();
        for l in &status.languages {
            let draft = self.draft(&l.id, l.custom_command.clone(), window, cx);
            let (state, color) = if l.disabled {
                ("disabled", theme::TEXT_FAINT)
            } else if l.command.is_some() {
                ("ready", theme::OK)
            } else {
                ("not installed", theme::WARN)
            };
            let mut text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().flex().items_center().gap_2().child(div().text_sm().text_color(c(theme::TEXT)).child(l.name.clone())).child(ui::pill(state, c(color))));
            if !l.disabled {
                text = match &l.command {
                    Some(cmd) => text.child(ui::code(cmd.clone())),
                    None => {
                        let bins: Vec<_> = l.servers.iter().map(|s| s.bin.as_str()).collect();
                        text.child(ui::muted(format!("Install {}: {}", bins.join(" or "), l.servers.first().map(|s| s.install.as_str()).unwrap_or(""))))
                    }
                };
            }
            if l.id == "cpp" {
                text = text.child(ui::faint(
                    "clangd reads compile_commands.json for include paths and flags; the editor offers to generate it with CMake when a repo has none.",
                ));
            }
            if !l.disabled {
                let (id, errors_only, hidden) = (l.id.clone(), l.errors_only, l.hidden_diagnostics.clone());
                let mut filters = div().flex().flex_wrap().items_center().gap_2().child(
                    ui::check(format!("lang:{}:errors-only", l.id), "Errors only", l.errors_only).on_click(cx.listener({
                        let (id, hidden) = (id.clone(), hidden.clone());
                        move |t, _: &ClickEvent, w, cx| {
                            let (id, hidden) = (id.clone(), hidden.clone());
                            t.configure(move || orbit_core::lsp::set_diagnostics_filter(&id, !errors_only, hidden), w, cx)
                        }
                    })),
                );
                for code in &hidden {
                    let rest: Vec<String> = hidden.iter().filter(|c| *c != code).cloned().collect();
                    let id = id.clone();
                    filters = filters.child(
                        ui::button(format!("lang:{}:show:{code}", l.id), format!("{code} ×"), Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            let (id, rest) = (id.clone(), rest.clone());
                            t.configure(move || orbit_core::lsp::set_diagnostics_filter(&id, errors_only, rest), w, cx)
                        })),
                    );
                }
                if hidden.is_empty() && !errors_only {
                    filters = filters.child(ui::faint("Hide a kind of diagnostic from its quick fix menu in the editor (Ctrl+.)."));
                }
                text = text.child(filters);
            }
            let (id, custom, disabled) = (l.id.clone(), l.custom_command.clone(), l.disabled);
            card = card.child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .gap_4()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(text)
                    .child(
                        div()
                            .w(px(260.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(ui::input(format!("lang:{}:command", l.id), &draft))
                            .child(ui::check(format!("lang:{}:enabled", l.id), "Enabled", !l.disabled).on_click(cx.listener(
                                move |t, _: &ClickEvent, w, cx| {
                                    let (id, custom) = (id.clone(), custom.clone());
                                    t.configure(move || orbit_core::lsp::configure(&id, custom, !disabled), w, cx)
                                },
                            ))),
                    ),
            );
        }
        let mut running = ui::card();
        if status.running.is_empty() {
            running = running.child(div().px_4().py_3().child(ui::muted("None right now: open a source file to start its language server.")));
        }
        for r in &status.running {
            let id = r.id;
            let log = self.logs.get(&id);
            running = running.child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .gap_4()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(c(theme::TEXT))
                                    .child(names.get(&r.language).cloned().unwrap_or_else(|| r.language.clone()))
                                    .child(ui::faint(r.root.clone())),
                            )
                            .child(ui::code(r.command.clone()))
                            .when_some(log, |d, lines| {
                                let text = if lines.is_empty() { "(no output)".to_string() } else { lines.join("\n") };
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("lsp-log-{id}")))
                                        .max_h(px(240.))
                                        .overflow_y_scroll()
                                        .p_2()
                                        .rounded(px(6.))
                                        .bg(c(theme::BG))
                                        .font_family(theme::MONO_FONT)
                                        .text_xs()
                                        .text_color(c(theme::TEXT_DIM))
                                        .children(text.lines().map(|l| div().child(l.to_string())).collect::<Vec<_>>()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(ui::button(format!("lsp:{id}:log"), if log.is_some() { "Hide log" } else { "Log" }, Tone::Plain).on_click(cx.listener(
                                move |t, _: &ClickEvent, _, cx| {
                                    if t.logs.remove(&id).is_none() {
                                        let lines = orbit_core::lsp::log(id);
                                        t.logs.insert(id, lines[lines.len().saturating_sub(80)..].to_vec());
                                    }
                                    cx.notify();
                                },
                            )))
                            .child(ui::button(format!("lsp:{id}:stop"), "Stop", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                orbit_core::lsp::stop(id);
                                t.load_languages(cx);
                            }))),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::muted(
                "Orbit's editor talks to the same language servers VS Code uses: go to definition (F12, Ctrl+click), references (Shift+F12), symbols (Ctrl+Shift+O, Ctrl+T), completion and errors. A server starts when you open a file of its language, one per repository folder.",
            ))
            .child(card)
            .child(ui::heading("Running servers"))
            .child(running)
            .into_any_element()
    }

    fn render_appearance(&self) -> gpui::AnyElement {
        let mut grid = div().flex().flex_wrap().gap_3();
        for (name, dark) in THEMES {
            let on = *name == "Orbit Dark";
            grid = grid.child(
                div()
                    .w(px(150.))
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(if on { theme::ACCENT } else { theme::BORDER }))
                    .when(!on, |d| d.opacity(0.5))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(c(theme::TEXT)).child(*name))
                    .child(ui::faint(if on { "Dark · in use" } else if *dark { "Dark · Tauri app only" } else { "Light · Tauri app only" })),
            );
        }
        ui::card()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(c(theme::TEXT)).child("Theme"))
                    .child(ui::muted(
                        "Colors for the whole app, the editor, diffs and terminals. The native app has Orbit Dark only for now; the Tauri app keeps its own choice on this machine.",
                    ))
                    .child(grid),
            )
            .into_any_element()
    }

    fn render_health(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let checks = self.health.ready();
        let failing = checks.map(|l| l.iter().filter(|c| c.required && !c.ok).count());
        let summary = match failing {
            Some(0) => ui::muted("Everything Orbit needs is installed."),
            Some(n) => ui::error(format!("{n} required check{} failing.", if n > 1 { "s" } else { "" })),
            None => ui::faint("Checking…"),
        };
        let mut out = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(div().flex_1().flex().flex_col().child(ui::heading("Environment")).child(summary))
                .child(ui::button("health-recheck", "Re-check", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load_health(w, cx)))),
        );
        let Some(checks) = checks else { return out.into_any_element() };
        const GROUPS: &[(&str, &str, &str)] = &[
            ("orbit", "Orbit", ""),
            ("agent", "AI agent", ""),
            ("build", "Build tools", "Detected from your repositories' build commands. None configured yet."),
            ("language", "Language servers", "Detected from the languages your cloned repositories use. None found yet."),
        ];
        for (id, title, empty) in GROUPS {
            out = out.child(ui::faint(title.to_uppercase()).pt_2());
            let rows: Vec<_> = checks.iter().filter(|c| c.group == *id).collect();
            if rows.is_empty() {
                out = out.child(ui::muted(*empty));
                continue;
            }
            let mut card = ui::card();
            for ch in rows {
                let color = if ch.ok {
                    theme::OK
                } else if ch.required {
                    theme::DANGER
                } else {
                    theme::WARN
                };
                card = card.child(
                    div()
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .border_b_1()
                        .border_color(c(theme::BORDER))
                        .child(ui::dot(c(color)))
                        .child(div().w(px(170.)).flex_none().text_sm().text_color(c(theme::TEXT)).child(ch.name.clone()))
                        .child(div().flex_1().min_w_0().text_xs().text_color(c(if ch.ok { theme::TEXT_DIM } else { color })).child(ch.detail.clone()))
                        .when(!ch.used_by.is_empty(), |d| d.child(ui::faint(format!("used by {}", ch.used_by.join(", "))))),
                );
            }
            out = out.child(card);
        }
        out.into_any_element()
    }

    fn render_updates(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let st = updates::state(cx);
        let busy = matches!(st.status, updates::Status::Checking);
        let action = match &st.status {
            updates::Status::Available(_) => ui::button("updates-open", "Open release page", Tone::Primary).on_click(|_, _, cx| cx.open_url(updates::RELEASES_URL)),
            _ => ui::button("updates-check", if busy { "Checking…" } else { "Check for updates" }, Tone::Plain).on_click(|_, _, cx| updates::check(true, cx)),
        };
        let mut text = st.describe();
        if let updates::Status::Failed(e) = &st.status {
            text = format!("{text}\n{e}");
        }
        ui::card()
            .child(ui::setting(
                "Version",
                "Orbit checks GitHub for a new release at startup and every hour while it's open.",
                ui::code(updates::version()),
            ))
            .child(ui::setting("Updates", text, action))
            .into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let d = def(self.section);
        let body = match self.section {
            Section::Repos => self.render_repos(window, cx),
            Section::Groups => self.render_groups(cx),
            Section::Ai => self.render_ai(cx),
            Section::Languages => self.render_languages(window, cx),
            Section::Appearance => self.render_appearance(),
            Section::Health => self.render_health(cx),
            Section::Updates => self.render_updates(cx),
        };
        div()
            .size_full()
            .flex()
            .child(self.render_nav(cx))
            .child(div().flex_1().min_w_0().h_full().child(ui::page("settings-body", d.title, d.description).child(body)))
    }
}

impl PageView for SettingsPage {
    fn title(&self) -> SharedString {
        "Settings".into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_config(window, cx);
    }

    fn dump(&self, out: &mut String, cx: &App) {
        let d = def(self.section);
        let shown: Vec<&str> = self.matching(cx).iter().map(|s| s.slug).collect();
        let _ = writeln!(out, "  section {} · nav {:?}", d.slug, shown);
        match &self.config {
            Load::Ready(cfg) => {
                let _ = writeln!(
                    out,
                    "  config: ai {} / {} / fast {:?} / remote {} · repos dir {:?} · workspaces dir {:?}",
                    cfg.ai.agent, cfg.ai.model, cfg.ai.fast_model, cfg.ai.remote_control, cfg.repos_dir, cfg.workspaces_dir
                );
                for s in &cfg.services {
                    let build = s.build.as_ref().and_then(|b| b.for_current_os()).unwrap_or("-");
                    let _ = writeln!(
                        out,
                        "  repo {} {} path {:?} build {build:?} worktree {} clone {}",
                        s.name,
                        s.repo,
                        s.path,
                        s.worktree,
                        self.repos.clone_label(&s.name)
                    );
                }
                let mut groups: Vec<_> = cfg.groups.iter().collect();
                groups.sort();
                for (g, m) in groups {
                    let _ = writeln!(out, "  group {g}: {m:?}");
                }
            }
            other => {
                let _ = writeln!(out, "  config: {}", ui::load_label(other));
            }
        }
        self.repos.dump(out, cx);
        self.groups.dump(out, cx);
        let _ = writeln!(out, "  models: {}", ui::load_label(&self.models));
        if let Some(t) = &self.test {
            let _ = writeln!(out, "  agent test: {}", ui::load_label(t));
        }
        if let Load::Ready(s) = &self.languages {
            for l in &s.languages {
                let _ = writeln!(
                    out,
                    "  language {} command {:?} custom {:?} disabled {} errors-only {} hidden {:?} input {:?}",
                    l.id,
                    l.command,
                    l.custom_command,
                    l.disabled,
                    l.errors_only,
                    l.hidden_diagnostics,
                    self.drafts.get(&l.id).map(|d| d.read(cx).value().to_string()).unwrap_or_default()
                );
            }
            for r in &s.running {
                let _ = writeln!(out, "  running server {} {} {}", r.id, r.language, r.root);
            }
        }
        match &self.health {
            Load::Ready(list) => {
                for ch in list {
                    let _ = writeln!(out, "  health {} [{}] {} {}", ch.name, ch.group, if ch.ok { "ok" } else if ch.required { "FAIL" } else { "warn" }, ch.detail);
                }
            }
            Load::Idle => {}
            other => {
                let _ = writeln!(out, "  health: {}", ui::load_label(other));
            }
        }
    }
}
