//! Rider-style run widget for the title bar (`RunWidget.tsx`): pick a run
//! configuration and CMake preset, then Build, Run, Debug or Stop.
use super::{store, RunTarget};
use crate::app::OrbitApp;
use crate::commands;
use crate::debug;
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{anchored, deferred, div, Anchor, prelude::*, px, Context, FontWeight, MouseButton, SharedString, Subscription, WeakEntity, Window};
use gpui_component::tooltip::Tooltip;

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Config,
    Preset,
}

pub struct RunWidget {
    app: WeakEntity<OrbitApp>,
    open: Option<Open>,
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
        Self { app, open: None, _subscriptions: subs }
    }

    fn run(&self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.app.update(cx, |a, cx| commands::run(id, a, window, cx));
    }

    /// The open list's options: (label, selected, target).
    fn options(&self, which: Open, cx: &Context<Self>) -> Vec<(String, bool, RunTarget)> {
        let s = store(cx).read(cx);
        let Some(target) = s.target.clone() else { return vec![] };
        match which {
            Open::Config => {
                let multi = s.setups.len() > 1;
                s.setups
                    .iter()
                    .flat_map(|(repo, setup)| setup.configs.iter().map(move |c| (repo.clone(), c.name.clone())))
                    .map(|(repo, name)| {
                        let current = repo == target.repo && name == target.name;
                        let label = if multi { format!("{repo} · {name}") } else { name.clone() };
                        let next = RunTarget {
                            workspace: target.workspace.clone(),
                            preset: if repo == target.repo { target.preset.clone() } else { None },
                            repo,
                            name,
                        };
                        (label, current, next)
                    })
                    .collect()
            }
            Open::Preset => {
                let current = s.config().and_then(|c| c.preset.clone());
                s.setup(&target.repo)
                    .map(|setup| {
                        setup
                            .presets
                            .iter()
                            .map(|p| (p.clone(), Some(p) == current.as_ref(), RunTarget { preset: Some(p.clone()), ..target.clone() }))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
    }

    /// `.select-list` under a trigger.
    fn render_list(&self, which: Open, cx: &mut Context<Self>) -> impl IntoElement {
        let width = if which == Open::Config { 240. } else { 260. };
        let rows = self.options(which, cx).into_iter().enumerate().map(|(ix, (label, selected, next))| {
            div()
                .id(("run-option", ix))
                .relative()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(12.5))
                .cursor_pointer()
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
                        this.open = None;
                        let next = next.clone();
                        store(cx).update(cx, |s, cx| s.choose(next, cx));
                        cx.notify();
                    }),
                )
        });
        div().absolute().left_0().top(px(30.)).child(
            deferred(
                anchored().anchor(Anchor::TopLeft).snap_to_window().child(
                    div()
                        .id("run-list")
                        .occlude()
                        .w(px(width))
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .p(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .bg(c(theme::PANEL_RAISED))
                        .shadow(theme::shadow(12., 40., 0.55))
                        .font_family(theme::UI_FONT)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.open = None;
                            cx.notify();
                        }))
                        .children(rows),
                ),
            )
            .with_priority(2),
        )
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
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open = if this.open == Some(which) { None } else { Some(which) };
                            cx.notify();
                        }),
                    ),
            )
            .when(open, |d| d.child(self.render_list(which, cx)))
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
        let (target, config, multi, presets, building, running) = {
            let s = store(cx).read(cx);
            let (Some(target), Some(config)) = (s.target.clone(), s.config().cloned()) else { return div().id("run-widget") };
            let presets = s.setup(&target.repo).map_or(0, |p| p.presets.len());
            (target, config, s.setups.len() > 1, presets, s.is_building(), s.is_running())
        };
        let live = debug::store(cx).read(cx).is_live();
        let label = if multi { format!("{} · {}", target.repo, config.name) } else { config.name.clone() };
        let program = config.program.clone();
        let no_program: SharedString = "No program to run in this configuration".into();
        let stoppable = building || running || live;
        div()
            .id("run-widget")
            .flex()
            .items_center()
            .gap(px(2.))
            .mr(px(10.))
            .child(self.select("run-config", Open::Config, label, false, cx))
            .when(presets > 0 && config.kind == "cmake", |d| {
                d.child(self.select("run-preset", Open::Preset, config.preset.clone().unwrap_or_else(|| "preset".into()), true, cx))
            })
            .child(button(
                "run-build",
                if building { "spinner" } else { "hammer" },
                14.,
                theme::TEXT_DIM,
                !building,
                format!("Build (Ctrl+F9)\n{}", config.build_label).into(),
                "run.build",
                cx,
            ))
            .child(button(
                "run-run",
                "play-fill",
                13.,
                theme::OK,
                !building && program.is_some(),
                program.as_ref().map_or(no_program.clone(), |p| format!("Run (Shift+F10)\n{p}").into()),
                "run.run",
                cx,
            ))
            .child(button(
                "run-debug",
                "bug",
                14.,
                theme::OK,
                !building && program.is_some() && !live,
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
    }
}
