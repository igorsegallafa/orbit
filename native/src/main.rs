//! Orbit's native UI (GPUI + gpui-component). Stage 1: the shell, terminals,
//! file tree, and the editor with language servers; see README.md.
mod app;
mod diff_view;
mod editor;
mod file_tree;
mod lsp;
mod script;
mod terminal;
mod theme;
mod workspace;

use app::{OrbitApp, Quit};
use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;
use script::Script;
use std::time::Instant;

fn main() {
    let started = Instant::now();
    let script = Script::from_args();
    // A launcher's bare PATH would hide gh, clangd and the agents.
    orbit_core::proc::refresh_path();
    if script.is_active() {
        terminal::CLIPBOARD_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    gpui_platform::application().with_assets(gpui_kit_assets::Assets::new("")).run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::init(cx);
        app::bind_keys(cx);
        file_tree::bind_keys(cx);
        editor::bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("orbit-native".into()),
            titlebar: Some(gpui::TitlebarOptions { title: Some("Orbit".into()), ..Default::default() }),
            ..Default::default()
        };
        let setup = script.clone();
        let mut app_entity = None;
        let window = cx
            .open_window(options, |window, cx| {
                window.on_next_frame(move |_, _| eprintln!("first frame after {} ms", started.elapsed().as_millis()));
                let view = cx.new(|cx| {
                    let mut app = OrbitApp::new(window, cx);
                    script::start(&setup, &mut app, window, cx);
                    app
                });
                app_entity = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open window");
        cx.activate(true);
        let app_entity = app_entity.expect("root view created");
        // Language servers get a clean shutdown when the window goes.
        cx.on_app_quit({
            let app_entity = app_entity.clone();
            move |cx| {
                app_entity.update(cx, |a, _| a.lsp.shutdown_all());
                async {}
            }
        })
        .detach();
        if script.is_active() || !script.keys.is_empty() || !script.open.is_empty() {
            cx.spawn(async move |cx| script::run(script, window, app_entity, cx).await).detach();
        }
    });
}
