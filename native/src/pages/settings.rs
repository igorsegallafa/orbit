//! Settings (`SettingsPage.tsx`): its own section list with a filter, and the
//! sections AI, Languages, Appearance, Health and Updates. Repositories and
//! Groups are in `repos.rs`. Everything reads and writes `config.yaml`
//! through orbit-core, so the Tauri app sees the same settings.
use super::kit::{self, Kind};
use super::ui;
use super::{updates, Host, PageView};
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Focusable, SharedString, Subscription, Task, Window};
use gpui_component::input::{Input, InputEvent, InputState};
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
    Notifications,
    Keyboard,
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
    /// Lucide icon (`SettingsPage.tsx`'s nav icons).
    icon: &'static str,
}

const NAV: &[(&str, &[SectionDef])] = &[
    (
        "WORKSPACE",
        &[
            SectionDef {
                id: Section::Repos,
                icon: "repo",
                slug: "repos",
                title: "Repositories",
                description: "The repositories Orbit manages and the folders it clones them into.",
                keywords: "repos folders clone path services github",
            },
            SectionDef {
                id: Section::Groups,
                icon: "layers",
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
                icon: "spark",
                slug: "ai",
                title: "AI",
                description: "The agent and model Orbit uses for sessions, commit messages and reviews.",
                keywords: "agent model claude codex prompt",
            },
            SectionDef {
                id: Section::Languages,
                icon: "code-view",
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
                icon: "palette",
                slug: "appearance",
                title: "Appearance",
                description: "The app's theme: interface, editor, diffs and terminals.",
                keywords: "theme dark light color dracula nord tokyo catppuccin solarized",
            },
            SectionDef {
                id: Section::Notifications,
                icon: "bell",
                slug: "notifications",
                title: "Notifications",
                description: "Desktop notifications when an agent or a Ralph run needs you.",
                keywords: "notify alert desktop os toast inbox",
            },
            SectionDef {
                id: Section::Keyboard,
                icon: "terminal",
                slug: "keyboard",
                title: "Keyboard",
                description: "How terminals get keys the desktop would otherwise take.",
                keywords: "keys shortcuts gnome super toshy inhibit terminal",
            },
            SectionDef {
                id: Section::Health,
                icon: "pulse",
                slug: "health",
                title: "Health",
                description: "Tools Orbit relies on, and whether each one is installed.",
                keywords: "environment doctor install git gh node",
            },
            SectionDef {
                id: Section::Updates,
                icon: "download",
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
    gnome: Option<crate::terminal::inhibit::Gnome>,
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
            gnome: None,
            _poll: poll,
            _subs: vec![sub, cx.observe_global::<updates::Updater>(|_, cx| cx.notify())],
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
            Section::Keyboard => self.load_gnome(window, cx),
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
    fn draft(&mut self, id: &str, value: Option<String>, placeholder: String, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(d) = self.drafts.get(id) {
            return d.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value.unwrap_or_default()));
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

    fn load_gnome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::bg(cx, crate::terminal::inhibit::gnome_status, |t: &mut Self, g, _, _| t.gnome = Some(g), window);
    }

    fn load_health(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.health = Load::Loading;
        ui::bg(cx, orbit_core::health::run, |t: &mut Self, list, _, _| t.health = Load::Ready(list), window);
    }

    // ---------- rendering ----------

    /// Settings' own navigation (`SettingsNav`): it stands in for the app sidebar.
    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.matching(cx);
        let focused = self.filter.read(cx).focus_handle(cx).is_focused(window);
        let search = div()
            .relative()
            .h(px(30.))
            .mb(px(8.))
            .pl(px(28.))
            .pr(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(c(if focused { theme::ACCENT } else { theme::BORDER }))
            .bg(c(theme::BG))
            .child(div().absolute().left(px(9.)).top(px(8.)).child(kit::icon("search", 13., c(theme::TEXT_FAINT))))
            .child(Input::new(&self.filter).appearance(false).px(px(0.)).text_size(px(12.5)).w_full())
            .child(probe("settings-filter"));
        let mut nav = div()
            .id("settings-nav")
            .w(px(220.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .px(px(12.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                kit::nav_item("settings-back", "arrow-left", "Back to app", false)
                    .text_color(c(theme::TEXT_FAINT))
                    .mb(px(6.))
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.host.close(super::SETTINGS_KEY, w, cx))),
            )
            .child(search);
        for (group, sections) in NAV {
            let items: Vec<_> = sections.iter().filter(|s| shown.iter().any(|m| m.id == s.id)).collect();
            if items.is_empty() {
                continue;
            }
            nav = nav.child(kit::nav_section(group));
            for s in items {
                let id = s.id;
                nav = nav.child(
                    kit::nav_item(format!("settings-nav:{}", s.slug), s.icon, s.title, s.id == self.section)
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.select(id, w, cx))),
                );
            }
        }
        if shown.is_empty() {
            nav = nav.child(
                div()
                    .p(px(10.))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_FAINT))
                    .child(format!("No settings match “{}”", self.filter.read(cx).value().trim())),
            );
        }
        nav
    }

    fn render_ai(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(ai) = self.ai() else {
            return kit::section().child(kit::skeleton(gpui::relative(0.4), 20.)).child(kit::skeleton(gpui::relative(1.), 34.)).into_any_element();
        };
        let me = cx.entity().downgrade();
        let agents: Vec<(String, &str)> = AGENTS.iter().map(|(id, l)| (id.to_string(), *l)).collect();
        let agent_seg = kit::seg("ai-agent", &agents, &ai.agent, move |v, w, cx| {
            let _ = me.update(cx, |t, cx| t.pick_agent(v, w, cx));
        });
        let model_hint = match ai.agent.as_str() {
            "opencode" => "Provider/model as listed by `opencode models`.",
            "omp" => "As listed by `omp models` (fuzzy match supported).",
            _ => "Passed to `claude --model`. Each AI feature uses it unless a run overrides it.",
        };
        let loading = matches!(self.models, Load::Loading);
        let picker = |name: &'static str, label: String, fast: bool, cx: &mut Context<Self>| -> gpui::AnyElement {
            let control = if loading {
                kit::skeleton(gpui::relative(1.), 34.).into_any_element()
            } else {
                kit::select(name, label).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.model_menu(fast, w, cx))).into_any_element()
            };
            div().w(px(340.)).child(control).into_any_element()
        };
        let fast_label = ai.fast_model.clone().unwrap_or_else(|| if ai.agent == "claude" { "Default (claude-haiku-4-5)".into() } else { "Same as Model".into() });
        let me = cx.entity().downgrade();
        let remote = kit::seg("ai-remote", &[(false, "Off"), (true, "On")], &ai.remote_control, move |on, w, cx| {
            let _ = me.update(cx, |t, cx| t.set_ai(|ai| ai.remote_control = on, false, w, cx));
        });
        let testing = matches!(self.test, Some(Load::Loading));
        let state = match &self.test {
            Some(Load::Ready(())) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .child(repo_dot())
                    .child("Working")
                    .into_any_element(),
            ),
            Some(Load::Failed(_)) => {
                Some(div().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::DANGER)).child("Failed").into_any_element())
            }
            _ => None,
        };
        let mut connection = div().flex().flex_col().gap(px(4.)).child(kit::hint("Sends a tiny prompt to check the agent is installed and the model answers."));
        if let Some(Load::Failed(e)) = &self.test {
            connection = connection.child(kit::error_box(e.clone()));
        }
        let test_btn = kit::disabled(kit::button_busy("ai-test", if testing { "Testing…" } else { "Test agent" }, Kind::Secondary, testing), testing)
            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.test_agent(w, cx)));
        let mut rows = vec![
            kit::setting("Agent", kit::hint("The CLI Orbit runs for plans, commit messages, PR drafts, CI investigation and Ralph."), agent_seg),
            kit::setting("Model", kit::hint(model_hint), picker("ai-model", ai.model.clone(), false, cx)),
            kit::setting("Fast model", kit::hint("Used for commit messages and PR drafts, where speed matters more than depth."), picker("ai-fast-model", fast_label, true, cx)),
        ];
        if ai.agent == "claude" {
            rows.push(kit::setting(
                "Remote Control",
                kit::hint("Claude sessions opened in Orbit can be continued from claude.ai or the Claude app. Applies to sessions started after the change."),
                remote,
            ));
        }
        rows.push(kit::setting("Connection", connection, div().flex().items_center().gap(px(12.)).children(state).child(test_btn)));
        kit::section().child(kit::rows(rows)).into_any_element()
    }

    fn render_languages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let status = match &self.languages {
            Load::Ready(s) => s.clone(),
            Load::Failed(e) => return kit::error_box(e.clone()).into_any_element(),
            _ => return kit::section().child(kit::skeleton(gpui::relative(1.), 18.)).child(kit::skeleton(gpui::relative(1.), 200.)).into_any_element(),
        };
        let names: HashMap<String, String> = status.languages.iter().map(|l| (l.id.clone(), l.name.clone())).collect();
        let note = |text: &str| div().text_size(px(12.)).line_height(px(18.)).italic().text_color(c(theme::TEXT_FAINT)).child(text.to_string());
        let mono = |text: String, color: u32| div().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(color)).child(text);
        let mut rows = vec![];
        for l in &status.languages {
            let bins = l.servers.iter().map(|s| s.bin.as_str()).collect::<Vec<_>>().join(" / ");
            let draft = self.draft(&l.id, l.custom_command.clone(), bins, window, cx);
            let state = if l.disabled {
                kit::outline_pill("disabled", c(theme::TEXT_FAINT), c(theme::BORDER_STRONG))
            } else if l.command.is_some() {
                kit::outline_pill("ready", c(theme::OK), kit::alpha(theme::OK, 0.35))
            } else {
                kit::outline_pill("not installed", c(theme::WARN), kit::alpha(theme::WARN, 0.35))
            };
            let mut text = div().flex().flex_col().gap(px(4.)).min_w_0().max_w(px(560.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.5))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(c(theme::TEXT))
                    .child(l.name.clone())
                    .child(state),
            );
            if !l.disabled {
                text = match &l.command {
                    Some(cmd) => text.child(mono(cmd.clone(), theme::TEXT_DIM)),
                    None => {
                        let bins: Vec<_> = l.servers.iter().map(|s| s.bin.as_str()).collect();
                        text.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(4.))
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(c(theme::TEXT_FAINT))
                                .child(format!("Install {}:", bins.join(" or ")))
                                .child(mono(l.servers.first().map(|s| s.install.clone()).unwrap_or_default(), theme::TEXT_FAINT)),
                        )
                    }
                };
            }
            if l.id == "cpp" {
                text = text.child(note(
                    "clangd reads compile_commands.json for include paths and flags; the editor offers to generate it with CMake when a repo has none.",
                ));
            }
            if !l.disabled {
                let (id, errors_only, hidden) = (l.id.clone(), l.errors_only, l.hidden_diagnostics.clone());
                let mut filters = div().mt(px(4.)).flex().flex_wrap().items_center().gap_x(px(10.)).gap_y(px(6.)).child(
                    kit::check(format!("lang:{}:errors-only", l.id), "Errors only", l.errors_only).on_click(cx.listener({
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
                        div()
                            .id(SharedString::from(format!("lang:{}:show:{code}", l.id)))
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .pl(px(7.))
                            .pr(px(3.))
                            .rounded_full()
                            .border_1()
                            .border_color(c(theme::BORDER_STRONG))
                            .bg(c(theme::PANEL_RAISED))
                            .font_family(theme::MONO_FONT)
                            .text_size(px(11.))
                            .text_color(c(theme::TEXT_DIM))
                            .cursor_pointer()
                            .child(code.clone())
                            .child(div().text_color(c(theme::TEXT_FAINT)).hover(|s| s.text_color(c(theme::DANGER))).child("×"))
                            .child(probe(format!("lang:{}:show:{code}", l.id)))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                let (id, rest) = (id.clone(), rest.clone());
                                t.configure(move || orbit_core::lsp::set_diagnostics_filter(&id, errors_only, rest), w, cx)
                            })),
                    );
                }
                if hidden.is_empty() && !errors_only {
                    filters = filters.child(note("Hide a kind of diagnostic from its quick fix menu in the editor (Ctrl+.)."));
                }
                text = text.child(filters);
            }
            let (id, custom, disabled) = (l.id.clone(), l.custom_command.clone(), l.disabled);
            rows.push(
                div()
                    .px(px(20.))
                    .py(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.))
                    .child(text)
                    .child(
                        div()
                            .w(px(340.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().flex_1().min_w_0().child(kit::field_sized(format!("lang:{}:command", l.id), &draft, 30., 12., window, cx)))
                            .child(kit::check(format!("lang:{}:enabled", l.id), "Enabled", !l.disabled).on_click(cx.listener(
                                move |t, _: &ClickEvent, w, cx| {
                                    let (id, custom) = (id.clone(), custom.clone());
                                    t.configure(move || orbit_core::lsp::configure(&id, custom, !disabled), w, cx)
                                },
                            ))),
                    ),
            );
        }
        let mut running = vec![];
        if status.running.is_empty() {
            running.push(div().px(px(20.)).py(px(18.)).child(kit::hint("None right now: open a source file to start its language server.")));
        }
        for r in &status.running {
            let id = r.id;
            let log = self.logs.get(&id);
            let text = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .text_size(px(13.5))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(c(theme::TEXT))
                        .child(names.get(&r.language).cloned().unwrap_or_else(|| r.language.clone()))
                        .child(div().font_family(theme::MONO_FONT).font_weight(gpui::FontWeight::NORMAL).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(r.root.clone())),
                )
                .child(mono(r.command.clone(), theme::TEXT_DIM))
                .when_some(log, |d, lines| {
                    let text = if lines.is_empty() { "(no output)".to_string() } else { lines.join("\n") };
                    d.child(
                        div()
                            .id(SharedString::from(format!("lsp-log-{id}")))
                            .mt(px(6.))
                            .max_h(px(220.))
                            .overflow_y_scroll()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .bg(c(theme::BG))
                            .font_family(theme::MONO_FONT)
                            .text_size(px(11.))
                            .text_color(c(theme::TEXT_DIM))
                            .children(text.lines().map(|l| div().child(l.to_string())).collect::<Vec<_>>()),
                    )
                });
            running.push(
                div()
                    .px(px(20.))
                    .py(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.))
                    .child(text)
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(kit::mini(format!("lsp:{id}:log"), if log.is_some() { "Hide log" } else { "Log" }, Kind::Secondary).on_click(cx.listener(
                                move |t, _: &ClickEvent, _, cx| {
                                    if t.logs.remove(&id).is_none() {
                                        let lines = orbit_core::lsp::log(id);
                                        t.logs.insert(id, lines[lines.len().saturating_sub(80)..].to_vec());
                                    }
                                    cx.notify();
                                },
                            )))
                            .child(kit::mini(format!("lsp:{id}:stop"), "Stop", Kind::Secondary).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                orbit_core::lsp::stop(id);
                                t.load_languages(cx);
                            }))),
                    ),
            );
        }
        kit::section()
            .child(
                div().text_size(px(12.)).line_height(px(18.)).text_color(c(theme::TEXT_FAINT)).child(
                    "Orbit's editor talks to the same language servers VS Code uses: go to definition (F12, Ctrl+click), references (Shift+F12), symbols (Ctrl+Shift+O, Ctrl+T), completion, errors and semantic colors. A server starts when you open a file of its language, one per repository folder, and stops a few minutes after its last file closes.",
                ),
            )
            .child(kit::rows(rows))
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c(theme::TEXT_DIM))
                    .child("RUNNING SERVERS"),
            )
            .child(kit::rows(running))
            .into_any_element()
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pref = theme::pref();
        // "System" follows the OS: Orbit Dark or Orbit Light.
        let dark_os = !matches!(cx.window_appearance(), gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight);
        let all = theme::list();
        let system = all.iter().find(|t| t.id == if dark_os { "orbit-dark" } else { "orbit-light" }).unwrap_or(&all[0]);
        let mut cards: Vec<(String, &theme::ThemeDef, String, String)> =
            vec![("system".into(), system, "System".into(), format!("Now {}", if system.dark { "dark" } else { "light" }))];
        for t in all.iter().filter(|t| t.dark).chain(all.iter().filter(|t| !t.dark)) {
            cards.push((t.id.to_string(), t, t.label.to_string(), if t.dark { "Dark".into() } else { "Light".into() }));
        }
        // `repeat(auto-fill, minmax(168px, 1fr))` over the 1038px row: five columns.
        let width = (1038. - 4. * 12.) / 5.;
        let grid = div().flex().flex_wrap().gap(px(12.)).children(cards.into_iter().map(|(id, t, label, sub)| {
            let on = id == pref;
            let pid = id.clone();
            div()
                .id(SharedString::from(format!("theme:{id}")))
                .relative()
                .w(px(width))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(theme::size::RADIUS))
                .border_1()
                .bg(c(theme::PANEL_RAISED))
                .cursor_pointer()
                .when(on, |d| d.border_color(c(theme::ACCENT)).shadow(ring()))
                .when(!on, |d| d.border_color(c(theme::BORDER)).hover(|s| s.border_color(c(theme::BORDER_STRONG))))
                .child(theme_preview(t))
                .child(
                    div()
                        .px(px(2.))
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(6.))
                        .child(div().text_size(px(12.5)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(label))
                        .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(sub)),
                )
                .child(probe(format!("theme:{id}")))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    theme::set(&pid, cx);
                    cx.notify();
                }))
        }));
        kit::section()
            .child(
                kit::card().child(
                    div()
                        .px(px(20.))
                        .py(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(kit::row_text(
                            "Theme",
                            kit::hint("Colors for the whole app, the editor, diffs and terminals. System follows your OS light/dark setting."),
                        ))
                        .child(grid),
                ),
            )
            .into_any_element()
    }

    fn render_notifications(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let on = crate::notifications::enabled();
        kit::section()
            .child(kit::rows([
                kit::setting(
                    "Desktop notifications",
                    kit::hint("When an agent finishes or needs your attention, or a Ralph run ends, while Orbit is in the background or another tab is open. The inbox gets them either way."),
                    kit::check("notify-enabled", if on { "On" } else { "Off" }, on).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        crate::notifications::set_enabled(!crate::notifications::enabled(), cx);
                        cx.notify();
                    })),
                ),
                kit::setting(
                    "Test",
                    kit::hint("Shows one now, so the system can ask for permission."),
                    kit::button("notify-test", "Send test", Kind::Secondary).on_click(cx.listener(|_, _: &ClickEvent, _, cx| crate::notifications::test(cx))),
                ),
            ]))
            .into_any_element()
    }

    fn render_keyboard(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use crate::terminal::inhibit;
        let on = inhibit::enabled();
        kit::section()
            .child(kit::rows(
                [kit::setting(
                    "Terminal keys over GNOME shortcuts",
                    kit::hint("While a terminal has focus, GNOME's Super shortcuts (Super+V, Super+A, Super+S…) go to the terminal; under Toshy those are Ctrl+V, Ctrl+A, Ctrl+S. Alt+Tab still switches windows. GNOME asks once to allow it; GNOME Settings → Apps → Orbit changes the answer. Wayland only: on X11 GNOME's grabs cannot be lifted by an app."),
                    kit::check("keys-inhibit", if on { "On" } else { "Off" }, on).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        inhibit::set_enabled(!inhibit::enabled(), cx);
                        cx.notify();
                    })),
                )]
            .into_iter()
            .chain(self.gnome.filter(|g| *g != inhibit::Gnome::Unknown).map(|g| {
                let (label, hint) = match g {
                    inhibit::Gnome::Allowed => ("Allowed", "GNOME lets Orbit's terminals take these keys."),
                    inhibit::Gnome::Denied => ("Denied", "GNOME keeps these keys and won't ask again. Ask again resets the answer; then restart Orbit and focus a terminal to get GNOME's prompt."),
                    _ => ("Not asked yet", "Click into a terminal tab: GNOME shows an Allow / Deny dialog (closing it counts as Deny). If Orbit was open when the answer was reset, restart it first."),
                };
                kit::setting(
                    format!("GNOME permission: {label}"),
                    kit::hint(hint),
                    div().when(g == inhibit::Gnome::Denied, |d| {
                        d.child(kit::button("keys-ask-again", "Ask again", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                            t.gnome = None;
                            ui::bg(cx, || { inhibit::gnome_forget(); inhibit::gnome_status() }, |t: &mut Self, g, _, _| t.gnome = Some(g), w);
                            cx.notify();
                        })))
                    }),
                )
            })),
            ))
            .into_any_element()
    }

    fn render_health(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let checks = self.health.ready();
        let failing = checks.map(|l| l.iter().filter(|c| c.required && !c.ok).count());
        let summary = failing.map(|n| {
            div().mt(px(2.)).text_size(px(12.)).text_color(c(if n > 0 { theme::DANGER } else { theme::TEXT_FAINT })).child(if n == 0 {
                "Everything Orbit needs is installed.".to_string()
            } else {
                format!("{n} required check{} failing.", if n > 1 { "s" } else { "" })
            })
        });
        let recheck = kit::disabled(kit::button("health-recheck", "Re-check", Kind::Secondary), checks.is_none())
            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load_health(w, cx)));
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child("ENVIRONMENT"))
                    .children(summary),
            )
            .child(recheck);
        let mut out = kit::section().child(head);
        let Some(checks) = checks else {
            // `SkeletonTable rows={6} cols={3}`.
            let table = kit::card().child(crate::motion::skeleton_table(6, 3));
            return out.child(table).into_any_element();
        };
        const GROUPS: &[(&str, &str, &str)] = &[
            ("orbit", "Orbit", ""),
            ("agent", "AI agent", ""),
            ("build", "Build tools", "Detected from your repositories' build commands. None configured yet."),
            ("language", "Language servers", "Detected from the languages your cloned repositories use. None found yet."),
        ];
        for (id, title, empty) in GROUPS {
            let mut group = div().mt(px(4.)).child(
                div().mb(px(8.)).text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child(title.to_uppercase()),
            );
            let rows: Vec<_> = checks.iter().filter(|c| c.group == *id).collect();
            if rows.is_empty() {
                group = group.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(*empty));
                out = out.child(group);
                continue;
            }
            let list = rows.into_iter().map(|ch| {
                let color = if ch.ok {
                    theme::OK
                } else if ch.required {
                    theme::DANGER
                } else {
                    theme::WARN
                };
                div()
                    .px(px(14.))
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(px(13.))
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(c(color)))
                    .child(div().w(px(180.)).flex_none().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(ch.name.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(c(if ch.ok { theme::TEXT_FAINT } else { theme::TEXT_DIM })).child(ch.detail.clone()))
                    .when(!ch.used_by.is_empty(), |d| d.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(format!("used by {}", ch.used_by.join(", ")))))
            });
            out = out.child(group.child(kit::rows(list)));
        }
        out.into_any_element()
    }

    fn render_updates(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let st = updates::state(cx);
        let busy = matches!(st.status, updates::Status::Checking);
        let installable = updates::target().is_ok();
        let action = match &st.status {
            updates::Status::Available(_) if installable => kit::button("updates-install", "Install update", Kind::Primary).on_click(|_, _, cx| updates::install(cx)),
            updates::Status::Available(_) => kit::button("updates-open", "Open release page", Kind::Primary).on_click(|_, _, cx| cx.open_url(updates::RELEASES_URL)),
            updates::Status::Downloading { .. } => kit::disabled(kit::button_busy("updates-install", "Installing…", Kind::Primary, true), true),
            updates::Status::Ready(_) => kit::button("updates-restart", "Restart to update", Kind::Primary).on_click(|_, _, cx| updates::restart(cx)),
            _ => kit::disabled(kit::button_busy("updates-check", if busy { "Checking…" } else { "Check for updates" }, Kind::Secondary, busy), busy)
                .on_click(|_, _, cx| updates::check(true, cx)),
        };
        let mut text = div().flex().flex_col().gap(px(4.)).child(kit::hint(st.describe()));
        if let updates::Status::Failed(e) = &st.status {
            text = text.child(kit::error_box(e.clone()));
        }
        kit::section()
            .child(kit::rows([
                kit::setting(
                    "Version",
                    kit::hint("Orbit checks GitHub for a new release at startup and every hour while it's open."),
                    div().font_family(theme::MONO_FONT).text_size(px(13.)).text_color(c(theme::TEXT)).child(updates::version()),
                ),
                kit::setting("Updates", text, action),
            ]))
            .into_any_element()
    }
}

/// `.repo-dot`: the green "working"/"cloned" dot with its soft ring.
pub(super) fn repo_dot() -> gpui::Div {
    div().size(px(7.)).flex_none().rounded_full().bg(c(theme::OK)).shadow(vec![gpui::BoxShadow {
        color: kit::alpha(theme::OK, 0.12).into(),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
        inset: false,
    }])
}

/// `box-shadow: 0 0 0 1px var(--accent)`.
fn ring() -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme::hsla(theme::ACCENT),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(1.),
        inset: false,
    }]
}

/// `ThemePreview`: a miniature of the app in the theme's colours.
fn theme_preview(t: &theme::ThemeDef) -> gpui::Div {
    let h = |tok: u32| t.hex(tok);
    let bar = |tok: u32, w: f32, opacity: f32| div().h(px(4.)).w(gpui::relative(w)).rounded(px(2.)).bg(gpui::rgb(h(tok))).opacity(opacity);
    let dot = |tok: u32| div().size(px(8.)).rounded_full().bg(gpui::rgb(h(tok)));
    div()
        .h(px(84.))
        .flex()
        .rounded(px(6.))
        .overflow_hidden()
        .border_1()
        .border_color(gpui::rgb(h(theme::BORDER)))
        .bg(gpui::rgb(h(theme::BG)))
        .child(
            div()
                .w(gpui::relative(0.28))
                .px(px(6.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .border_r_1()
                .border_color(gpui::rgb(h(theme::BORDER)))
                .bg(gpui::rgb(h(theme::PANEL)))
                .child(bar(theme::ACCENT, 1., 1.))
                .child(bar(theme::TEXT_FAINT, 1., 0.8))
                .child(bar(theme::TEXT_FAINT, 1., 0.8)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(bar(theme::TEXT, 0.7, 1.))
                .child(bar(theme::TEXT_DIM, 0.5, 1.))
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .p(px(5.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(gpui::rgb(h(theme::BORDER)))
                        .bg(gpui::rgb(h(theme::PANEL_RAISED)))
                        .child(dot(theme::OK))
                        .child(dot(theme::WARN))
                        .child(dot(theme::DANGER))
                        .child(dot(theme::PURPLE)),
                )
                .child(div().flex_1())
                .child(div().w(gpui::relative(0.36)).h(px(9.)).rounded(px(3.)).bg(gpui::rgb(h(theme::ACCENT)))),
        )
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let d = def(self.section);
        let body = match self.section {
            Section::Repos => self.render_repos(window, cx),
            Section::Groups => self.render_groups(window, cx),
            Section::Ai => self.render_ai(cx),
            Section::Languages => self.render_languages(window, cx),
            Section::Appearance => self.render_appearance(cx),
            Section::Notifications => self.render_notifications(cx),
            Section::Keyboard => self.render_keyboard(cx),
            Section::Health => self.render_health(cx),
            Section::Updates => self.render_updates(cx),
        };
        div()
            .size_full()
            .flex()
            .bg(c(theme::BG))
            .child(self.render_nav(window, cx))
            .child(div().flex_1().min_w_0().h_full().child(
                // `.page` keyed by the section: page-in on each switch.
                crate::motion::page_in(kit::page("settings-body", kit::column(d.title, d.description).child(body)), gpui::ElementId::Name(format!("settings-in:{}", d.slug).into())),
            ))
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
        let _ = writeln!(out, "  section {} · nav {:?} · theme {} · notifications {}", d.slug, shown, theme::pref(), crate::notifications::enabled());
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
