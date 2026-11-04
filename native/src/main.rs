//! Orbit's native UI (GPUI + gpui-component): the shell, terminals, file
//! tree, and the editor with language servers; see README.md.
mod app;
mod commands;
mod diff_view;
mod editor;
mod file_ops;
mod file_tree;
mod icons;
mod lsp;
mod nav;
mod overlay;
mod palette;
mod probe;
mod review;
mod script;
mod search;
mod sessions;
mod shell;
mod state;
mod steps;
mod terminal;
mod theme;
mod toast;
mod workspace;

use app::OrbitApp;
use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions};
use gpui_component::{Root, TitleBar};
use script::Script;
use std::time::Instant;

fn main() {
    let started = Instant::now();
    let script = Script::from_args();
    // A launcher's bare PATH would hide gh, clangd and the agents.
    orbit_core::proc::refresh_path();
    if script.is_active() {
        terminal::CLIPBOARD_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
        probe::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // A self-test only reads and writes the state file it is given.
    let state_path = if script.is_active() { script.state.clone() } else { state::default_path() };
    let saved = app::load_state(state_path.as_deref());
    gpui_platform::application().with_assets(icons::Assets::new()).run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::init(cx);
        // Commands and keys, per stage.
        app::init(cx);
        file_tree::bind_keys(cx);
        editor::bind_keys(cx);
        search::init(cx);
        file_ops::init(cx);
        review::init(cx);

        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("orbit-native".into()),
            // Our own titlebar (TitleBar draws the window controls and moves the window).
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(size(px(720.), px(480.))),
            ..TitleBar::window_options()
        };
        let setup = script.clone();
        let mut app_entity = None;
        let window = cx
            .open_window(options, |window, cx| {
                window.set_window_title("Orbit");
                window.on_next_frame(move |_, _| eprintln!("first frame after {} ms", started.elapsed().as_millis()));
                let view = cx.new(|cx| {
                    let mut app = OrbitApp::new(state_path.clone(), window, cx);
                    app.restore(saved.clone(), window, cx);
                    script::start(&setup, &mut app, window, cx);
                    app
                });
                app_entity = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("open window");
        cx.activate(true);
        let app_entity = app_entity.expect("root view created");
        // Language servers get a clean shutdown, and the state its last write, when the app quits.
        cx.on_app_quit({
            let app_entity = app_entity.clone();
            move |cx| {
                app_entity.update(cx, |a, cx| {
                    a.save_now(cx);
                    a.lsp.shutdown_all()
                });
                async {}
            }
        })
        .detach();
        // Closing the window quits (Linux keeps the app running otherwise).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if script.is_active() || !script.keys.is_empty() || !script.open.is_empty() {
            cx.spawn(async move |cx| script::run(script, window, app_entity, cx).await).detach();
        }
    });
}
