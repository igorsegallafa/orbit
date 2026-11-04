#![recursion_limit = "512"]
// No console window behind the app on Windows (release builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! Orbit's native UI (GPUI + gpui-component): the shell, terminals, file
//! tree, and the editor with language servers; see README.md.
mod app;
mod commands;
mod debug;
mod diff_select;
mod diff_view;
mod editor;
mod file_ops;
mod file_tree;
mod git;
mod icons;
mod lsp;
mod motion;
mod nav;
mod overlay;
mod pages;
mod palette;
mod probe;
mod review;
mod run;
mod scope;
mod scroll;
mod script;
mod search;
mod sessions;
mod shell;
mod sidebar;
mod state;
mod steps;
mod terminal;
mod theme;
mod toast;
mod workspace;
mod workspace_ui;

use app::OrbitApp;
use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions};
use gpui_component::{Root, TitleBar};
use script::Script;
use std::time::Instant;

fn main() {
    // Claude Code runs `<this exe> --orbit-statusline <file>` as its status line
    // (orbit_core::agent_hooks). It must never open a window: each window would
    // restore the agent sessions, whose status line would start another window.
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--orbit-statusline") => return orbit_core::agent_hooks::statusline_helper(args.next()),
        Some("--version") => return println!("Orbit {}", pages::updates::version()),
        Some("--update") => return update_from_cli(),
        Some(flag) if flag.starts_with("--orbit-") => {
            eprintln!("orbit-native: unknown helper flag {flag}");
            std::process::exit(2);
        }
        _ => {}
    }
    run();
}

/// `orbit --update`: the in-app update without the window.
fn update_from_cli() {
    use orbit_core::updates;
    let fail = |e: String| -> ! {
        eprintln!("update failed: {e}");
        std::process::exit(1)
    };
    let current = pages::updates::version();
    let Some(release) = updates::check(current).unwrap_or_else(|e| fail(e)) else {
        return println!("Orbit {current} is the latest version");
    };
    let target = updates::Target::detect().unwrap_or_else(|e| fail(e));
    println!("updating {} from {current} to {}", target.path.display(), release.version);
    let mut last = None;
    updates::install(&release, &target, &updates::public_key(), |done, total| {
        let p = total.filter(|t| *t > 0).map(|t| done.min(t) * 100 / t);
        if p != last {
            last = p;
            eprintln!("downloaded {done} of {}", total.map(|t| t.to_string()).unwrap_or("?".into()));
        }
    })
    .unwrap_or_else(|e| fail(e));
    println!("installed Orbit {}: restart Orbit to use it", release.version);
}

fn run() {
    let started = Instant::now();
    pages::crash::install();
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
        theme::init(cx, script.theme.as_deref().or(saved.theme.as_deref()));
        // Commands and keys, per stage.
        app::init(cx);
        file_tree::bind_keys(cx);
        editor::register_languages();
        editor::bind_keys(cx);
        editor::load_diagnostic_filters(cx);
        search::init(cx);
        file_ops::init(cx);
        review::init(cx);
        run::init(cx);
        workspace_ui::init(cx);
        pages::init(cx);

        // Maximized, as the Tauri app opens (`tauri.conf.json`); self-tests pick a size.
        let (w, h) = script.window_size.unwrap_or((1280., 800.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let options = WindowOptions {
            window_bounds: Some(if script.window_size.is_some() { WindowBounds::Windowed(bounds) } else { WindowBounds::Maximized(bounds) }),
            app_id: Some("orbit".into()),
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
                    run::attach(&mut app, window, cx);
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
