//! Rider-style run widget for the title bar (`RunWidget.tsx`): pick a run
//! configuration and CMake preset, then Build, Run, Debug or Stop.
use super::{store, RunTarget};
use crate::app::OrbitApp;
use crate::commands;
use crate::debug;
use crate::overlay::MenuItem;
use crate::probe::probe;
use crate::theme::{self, c, size};
use gpui::{div, prelude::*, px, Context, MouseButton, MouseDownEvent, SharedString, Subscription, WeakEntity, Window};
use gpui_component::{tooltip::Tooltip, Icon, IconName, Sizable};

pub struct RunWidget {
    app: WeakEntity<OrbitApp>,
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
        Self { app, _subscriptions: subs }
    }

    fn run(&self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.app.update(cx, |a, cx| commands::run(id, a, window, cx));
    }

    fn menu(&self, at: gpui::Point<gpui::Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut Context<Self>) {
        let at = gpui::point(at.x - px(8.), px(size::TITLEBAR + 2.));
        let _ = self.app.update(cx, |a, cx| a.open_menu(at, items, window, cx));
    }

    fn config_menu(&mut self, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let s = store(cx).read(cx);
        let Some(target) = s.target.clone() else { return };
        let multi = s.setups.len() > 1;
        let items = s
            .setups
            .iter()
            .flat_map(|(repo, setup)| setup.configs.iter().map(move |c| (repo.clone(), c.name.clone())))
            .map(|(repo, name)| {
                let current = repo == target.repo && name == target.name;
                let label = if multi { format!("{repo} · {name}") } else { name.clone() };
                let label = if current { format!("✓  {label}") } else { format!("    {label}") };
                let next = RunTarget {
                    workspace: target.workspace.clone(),
                    preset: if repo == target.repo { target.preset.clone() } else { None },
                    repo,
                    name,
                };
                MenuItem::new(label, move |_, _, cx| {
                    let next = next.clone();
                    store(cx).update(cx, |s, cx| s.choose(next, cx));
                })
            })
            .collect();
        self.menu(at, items, window, cx);
    }

    fn preset_menu(&mut self, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let s = store(cx).read(cx);
        let (Some(target), Some(config)) = (s.target.clone(), s.config()) else { return };
        let Some(setup) = s.setup(&target.repo) else { return };
        let current = config.preset.clone();
        let items = setup
            .presets
            .iter()
            .map(|p| {
                let label = if Some(p) == current.as_ref() { format!("✓  {p}") } else { format!("    {p}") };
                let next = RunTarget { preset: Some(p.clone()), ..target.clone() };
                MenuItem::new(label, move |_, _, cx| {
                    let next = next.clone();
                    store(cx).update(cx, |s, cx| s.choose(next, cx));
                })
            })
            .collect();
        self.menu(at, items, window, cx);
    }
}

/// A dropdown button (`.run-select`).
fn select(id: &'static str, label: String, cx: &mut Context<RunWidget>, on: fn(&mut RunWidget, gpui::Point<gpui::Pixels>, &mut Window, &mut Context<RunWidget>)) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .h(px(size::TITLEBAR_BTN_H))
        .max_w(px(220.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(6.))
        .text_size(px(size::TAB_FONT))
        .text_color(c(theme::TEXT_DIM))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
        .child(div().truncate().child(label))
        .child(Icon::new(IconName::ChevronDown).xsmall())
        .child(probe(id))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                on(this, ev.position, window, cx);
            }),
        )
}

/// An icon button (`.run-btn`): `glyph` is one of `icons.rs`' UI glyphs.
fn button(
    id: &'static str,
    glyph: &'static str,
    color: u32,
    enabled: bool,
    hint: SharedString,
    command: &'static str,
    cx: &mut Context<RunWidget>,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .w(px(size::TITLEBAR_BTN_W))
        .h(px(size::TITLEBAR_BTN_H))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(c(color))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED))))
        .when(!enabled, |d| d.opacity(0.35))
        .child(crate::icons::ui_icon(glyph, 14., c(color)))
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
        let (target, config, multi, presets, building, running) = {
            let s = store(cx).read(cx);
            let (Some(target), Some(config)) = (s.target.clone(), s.config().cloned()) else { return div().id("run-widget") };
            let presets = s.setup(&target.repo).map_or(0, |p| p.presets.len());
            (target, config, s.setups.len() > 1, presets, s.is_building(), s.is_running())
        };
        let live = debug::store(cx).read(cx).is_live();
        let label = if multi { format!("{} · {}", target.repo, config.name) } else { config.name.clone() };
        let program = config.program.clone();
        let no_program: SharedString = "No program in this configuration".into();
        div()
            .id("run-widget")
            .flex()
            .items_center()
            .gap_0p5()
            .mr_2()
            .child(select("run-config", label, cx, RunWidget::config_menu))
            .when(presets > 0 && config.kind == "cmake", |d| {
                d.child(select("run-preset", config.preset.clone().unwrap_or_else(|| "preset".into()), cx, RunWidget::preset_menu))
            })
            .child(button(
                "run-build",
                if building { "spinner" } else { "hammer" },
                theme::TEXT_DIM,
                !building,
                format!("Build (Ctrl+F9)\n{}", config.build_label).into(),
                "run.build",
                cx,
            ))
            .child(button(
                "run-run",
                "play",
                theme::OK,
                !building && program.is_some(),
                program.as_ref().map_or(no_program.clone(), |p| format!("Run (Shift+F10)\n{p}").into()),
                "run.run",
                cx,
            ))
            .child(button(
                "run-debug",
                "bug",
                theme::WARN,
                !building && program.is_some() && !live,
                if program.is_some() { "Debug (Shift+F9)".into() } else { no_program },
                "run.debug",
                cx,
            ))
            .child(button("run-stop", "stop", theme::DANGER, building || running || live, "Stop (Ctrl+F2)".into(), "run.stop", cx))
    }
}
