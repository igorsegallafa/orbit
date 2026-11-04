//! Rider-style run widget for the title bar (`RunWidget.tsx`): pick a run
//! configuration and CMake preset, then Build (or Build All), Run, Debug or
//! Stop. The configuration list groups the saved configurations before the
//! discovered CMake targets, filters as you type once it is long, and ends
//! with Edit Configurations….
use super::{store, RunTarget};
use crate::app::OrbitApp;
use crate::commands;
use crate::debug;
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{anchored, deferred, div, Anchor, prelude::*, px, Context, FocusHandle, FontWeight, KeyDownEvent, MouseButton, SharedString, Subscription, WeakEntity, Window};
use gpui_component::tooltip::Tooltip;
use orbit_core::run::Origin;

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Config,
    Preset,
    Build,
}

/// A list with more configurations than this shows the type-to-filter line.
const FILTER_FROM: usize = 10;

enum Row {
    Header(String),
    Option { label: String, selected: bool, next: RunTarget },
    Command { label: String, hint: Option<String>, command: &'static str },
}

pub struct RunWidget {
    app: WeakEntity<OrbitApp>,
    open: Option<Open>,
    filter: String,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RunWidget {
    pub fn new(app: WeakEntity<OrbitApp>, cx: &mut Context<Self>) -> Self {
        let mut subs = vec![cx.observe(&store(cx), |_, _, cx| cx.notify()), cx.observe(&debug::store(cx), |_, _, cx| cx.notify())];
        // Configurations follow the selected workspace.
        if let Some(entity) = app.upgrade() {
            subs.push(cx.observe(&entity, |_, app, cx| {
                let ws = app.read(cx).workspace().map(|w| (w.meta.name.clone(), w.meta.repos.clone()));
                store(cx).update(cx, |s, cx| s.sync_workspace(ws, cx));
            }));
        }
        Self { app, open: None, filter: String::new(), focus: cx.focus_handle(), _subscriptions: subs }
    }

    fn run(&self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.app.update(cx, |a, cx| commands::run(id, a, window, cx));
    }

    fn config_count(cx: &Context<Self>) -> usize {
        store(cx).read(cx).setups.iter().map(|(_, s)| s.configs.len()).sum()
    }

    /// The open list's rows.
    fn rows(&self, which: Open, cx: &Context<Self>) -> Vec<Row> {
        let s = store(cx).read(cx);
        let Some(target) = s.target.clone() else { return vec![] };
        match which {
            Open::Config => {
                let multi = s.setups.len() > 1;
                let filter = self.filter.to_lowercase();
                let mut rows = Vec::new();
                for (repo, setup) in &s.setups {
                    for (group, title) in [(Origin::Project, "Project"), (Origin::Local, "Local"), (Origin::Discovered, "CMake targets")] {
                        let members: Vec<_> = setup
                            .configs
                            .iter()
                            .filter(|c| c.origin == group && (filter.is_empty() || c.name.to_lowercase().contains(&filter)))
                            .collect();
                        if members.is_empty() {
                            continue;
                        }
                        rows.push(Row::Header(if multi { format!("{repo} · {title}") } else { title.to_string() }));
                        for c in members {
                            let current = *repo == target.repo && c.name == target.name;
                            let next = RunTarget {
                                workspace: target.workspace.clone(),
                                preset: if *repo == target.repo { target.preset.clone() } else { None },
                                repo: repo.clone(),
                                name: c.name.clone(),
                            };
                            rows.push(Row::Option { label: c.name.clone(), selected: current, next });
                        }
                    }
                }
                let mine = s.members(&target);
                if super::live_targets(cx).iter().any(|t| !mine.iter().any(|m| super::same(m, t))) {
                    rows.push(Row::Command { label: "Stop all".into(), hint: None, command: "run.stopAll" });
                }
                rows.push(Row::Command { label: "Edit Configurations…".into(), hint: None, command: "run.editConfigurations" });
                rows
            }
            Open::Preset => {
                let current = s.config().and_then(|c| c.preset.clone());
                s.setup(&target.repo)
                    .map(|setup| {
                        setup
                            .presets
                            .iter()
                            .map(|p| Row::Option { label: p.clone(), selected: Some(p) == current.as_ref(), next: RunTarget { preset: Some(p.clone()), ..target.clone() } })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Open::Build => {
                let Some(config) = s.config() else { return vec![] };
                let mut rows = vec![Row::Command {
                    label: if config.target.is_empty() { format!("Build {}", config.name) } else { format!("Build {}", config.target) },
                    hint: Some("Ctrl+F9".into()),
                    command: "run.build",
                }];
                if s.can_build_all() {
                    rows.push(Row::Command { label: "Build All".into(), hint: Some("Ctrl+Shift+F9".into()), command: "run.buildAll" });
                }
                rows
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.filter.clear();
        cx.notify();
    }

    /// `.select-list` under a trigger.
    fn render_list(&self, which: Open, cx: &mut Context<Self>) -> impl IntoElement {
        let width = match which {
            Open::Config => 260.,
            Open::Preset => 260.,
            Open::Build => 220.,
        };
        let filtering = which == Open::Config && Self::config_count(cx) > FILTER_FROM;
        let filter = self.filter.clone();
        let item = |id: (&'static str, usize)| {
            div()
                .id(id)
                .relative()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(5.))
                .text_size(px(12.5))
                .cursor_pointer()
        };
        let rows = self.rows(which, cx).into_iter().enumerate().map(|(ix, row)| match row {
            Row::Header(title) => div()
                .px(px(10.))
                .pt(px(if ix == 0 { 4. } else { 8. }))
                .pb(px(3.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(theme::TEXT_FAINT))
                .child(title.to_uppercase())
                .into_any_element(),
            Row::Option { label, selected, next } => item(("run-option", ix))
                .text_color(if selected { theme::hsla(theme::INFO).blend(theme::hsla(theme::TEXT).opacity(0.6)) } else { theme::hsla(theme::TEXT) })
                .when(selected, |d| d.bg(c(theme::PANEL)))
                .hover(|s| s.bg(c(theme::PANEL)))
                .child(div().flex_1().truncate().child(label.clone()))
                .when(selected, |d| d.child(div().text_size(px(11.)).text_color(c(theme::ACCENT)).child("✓")))
                .child(probe(format!("menu:{label}")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close(cx);
                        let next = next.clone();
                        store(cx).update(cx, |s, cx| s.choose(next, cx));
                    }),
                )
                .into_any_element(),
            Row::Command { label, hint, command } => div()
                .when(which == Open::Config, |d| d.mt(px(4.)).pt(px(4.)).border_t_1().border_color(c(theme::BORDER)))
                .child(
                    item(("run-command", ix))
                        .text_color(c(theme::TEXT))
                        .hover(|s| s.bg(c(theme::PANEL)))
                        .child(div().flex_1().truncate().child(label.clone()))
                        .children(hint.map(|h| div().flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(h)))
                        .child(probe(format!("menu:{label}")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(cx);
                                this.run(command, window, cx);
                            }),
                        ),
                )
                .into_any_element(),
        });
        let filter_line = filtering.then(|| {
            div()
                .mx(px(4.))
                .mb(px(4.))
                .px(px(8.))
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .text_size(px(12.))
                .text_color(c(if filter.is_empty() { theme::TEXT_FAINT } else { theme::TEXT }))
                .child(if filter.is_empty() { "Type to filter".to_string() } else { filter.clone() })
        });
        div().absolute().left_0().top(px(30.)).child(
            deferred(
                anchored().anchor(Anchor::TopLeft).snap_to_window().child(
                    div()
                        .id("run-list")
                        .occlude()
                        .w(px(width))
                        .p(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .bg(c(theme::PANEL_RAISED))
                        .shadow(theme::shadow(12., 40., 0.55))
                        .font_family(theme::UI_FONT)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
                        .children(filter_line)
                        .child(div().id("run-list-rows").max_h(px(360.)).overflow_y_scroll().children(rows)),
                ),
            )
            .with_priority(2),
        )
    }

    /// Typing filters the configuration list; Esc closes any list.
    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        let k = &ev.keystroke;
        match k.key.as_str() {
            "escape" => self.close(cx),
            "backspace" if self.open == Some(Open::Config) => {
                self.filter.pop();
            }
            _ if self.open == Some(Open::Config) && !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform => {
                match k.key_char.as_deref() {
                    Some(ch) if !ch.chars().any(char::is_control) => self.filter.push_str(ch),
                    _ => return,
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn toggle(&mut self, which: Open, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.clear();
        self.open = if self.open == Some(which) { None } else { Some(which) };
        if self.open.is_some() {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// A dropdown trigger (`.run-select .select-trigger`).
    fn select(&self, id: &'static str, which: Open, label: String, dim: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open == Some(which);
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id(id)
                    .relative()
                    .h(px(26.))
                    .min_w(px(90.))
                    .max_w(px(if which == Open::Preset { 220. } else { 200. }))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c(if dim { theme::TEXT_DIM } else { theme::TEXT }))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)).border_color(c(theme::BORDER)))
                    .child(div().truncate().child(label))
                    .child(div().flex_none().font_family(theme::MONO_FONT).text_size(px(10.)).text_color(c(theme::TEXT_FAINT)).child(if open { "▴" } else { "▾" }))
                    .child(probe(id))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.toggle(which, window, cx);
                        }),
                    ),
            )
            .when(open, |d| d.child(self.render_list(which, cx)))
    }

    /// The build split button (Rider's): the hammer builds the configuration,
    /// the chevron half (or a right click on the hammer) opens Build
    /// <target> / Build All. One bordered group, a divider between halves.
    fn build_split(&self, building: bool, hint: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open == Some(Open::Build);
        let enabled = !building;
        let hammer = div()
            .id("run-build")
            .relative()
            .w(px(28.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))))
            .child(crate::icons::ui_icon(if building { "spinner" } else { "hammer" }, 14., c(theme::TEXT_DIM)))
            .child(probe("run-build"))
            .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if enabled {
                        this.run("run.build", window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if enabled {
                        this.toggle(Open::Build, window, cx);
                    }
                }),
            );
        let chevron = div()
            .id("run-build-menu")
            .relative()
            .w(px(18.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .when(open, |d| d.bg(c(theme::PANEL_HOVER)))
            .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))))
            .child(crate::workspace_ui::icons::icon("chevron-down", 12., c(if open { theme::TEXT } else { theme::TEXT_DIM })))
            .child(probe("run-build-menu"))
            .when(!open, |d| d.tooltip(|window, cx| Tooltip::new("Build options: Build <target>, Build All (Ctrl+Shift+F9)").build(window, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if enabled {
                        this.toggle(Open::Build, window, cx);
                    }
                }),
            );
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id("run-build-split")
                    .relative()
                    .h(px(26.))
                    .flex()
                    .overflow_hidden()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(c(if open { theme::BORDER_STRONG } else { theme::BORDER }))
                    .when(!enabled, |d| d.opacity(0.35))
                    .child(hammer)
                    .child(chevron)
                    .child(probe("run-build-split")),
            )
            .when(open, |d| d.child(self.render_list(Open::Build, cx)))
    }
}

/// An icon button (`button.run-btn`): `glyph` is one of `icons.rs`' UI glyphs.
#[allow(clippy::too_many_arguments)]
fn button(
    id: &'static str,
    glyph: &'static str,
    size: f32,
    color: u32,
    enabled: bool,
    hint: SharedString,
    command: &'static str,
    cx: &mut Context<RunWidget>,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .w(px(28.))
        .h(px(26.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED))))
        .when(!enabled, |d| d.opacity(0.35))
        .child(crate::icons::ui_icon(glyph, size, c(color)))
        .child(probe(id))
        .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if enabled {
                    this.run(command, window, cx);
                }
            }),
        )
}

impl Render for RunWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (target, config, multi, presets, building) = {
            let s = store(cx).read(cx);
            let (Some(target), Some(config)) = (s.target.clone(), s.config().cloned()) else { return div().id("run-widget") };
            let presets = s.setup(&target.repo).map_or(0, |p| p.presets.len());
            (target, config, s.setups.len() > 1, presets, s.is_building())
        };
        let running = super::live_targets(cx).len();
        let label = if multi { format!("{} · {}", target.repo, config.name) } else { config.name.clone() };
        let compound = config.raw.is_compound();
        let program = if compound { Some(config.raw.configs.join(", ")) } else { config.program.clone() };
        let no_program: SharedString = "No program to run in this configuration".into();
        let stoppable = building || running > 0;
        div()
            .id("run-widget")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .flex()
            .items_center()
            .gap(px(2.))
            .mr(px(10.))
            .child(self.select("run-config", Open::Config, label, false, cx))
            .when(presets > 0 && config.kind == "cmake", |d| {
                d.child(self.select("run-preset", Open::Preset, config.preset.clone().unwrap_or_else(|| "preset".into()), true, cx))
            })
            .when(!compound, |d| d.child(self.build_split(building, format!("Build (Ctrl+F9)\n{}\nRight click: Build All", config.build_label).into(), cx)))
            .child(button(
                "run-run",
                "play-fill",
                13.,
                theme::OK,
                program.is_some(),
                program.as_ref().map_or(no_program.clone(), |p| format!("Run (Shift+F10)\n{p}").into()),
                "run.run",
                cx,
            ))
            .child(button(
                "run-debug",
                "bug",
                14.,
                theme::OK,
                program.is_some(),
                if program.is_some() { "Debug (Shift+F9)".into() } else { "No program to debug in this configuration".into() },
                "run.debug",
                cx,
            ))
            .child(button(
                "run-stop",
                "stop-fill",
                12.,
                if stoppable { theme::DANGER } else { theme::TEXT_DIM },
                stoppable,
                "Stop (Ctrl+F2)".into(),
                "run.stop",
                cx,
            ))
            .when(running > 1, |d| {
                d.child(
                    div()
                        .id("run-count")
                        .relative()
                        .px(px(5.))
                        .rounded(px(8.))
                        .text_size(px(10.5))
                        .bg(c(theme::PANEL_RAISED))
                        .text_color(c(theme::TEXT_DIM))
                        .child(running.to_string())
                        .child(probe("run-count"))
                        .tooltip(move |window, cx| Tooltip::new(format!("{running} running")).build(window, cx)),
                )
            })
    }
}
