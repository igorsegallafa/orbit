//! Shell steps for the self-test (`--do "step;step…"`), run after the fixed
//! script steps. Mouse steps target probed elements (`probe.rs`) and go
//! through `Window::dispatch_event`; keys through `dispatch_keystroke`.
//!
//! | Step | Does |
//! |---|---|
//! | `click:<probe>` `rclick:` `mclick:` `dblclick:` | left/right/middle/double click the probe's center |
//! | `drag:<probe>:<dx>,<dy>` | drag from the probe's center by (dx, dy) px |
//! | `key:<spec>` | one keystroke (`ctrl-shift-a`, `escape`) |
//! | `type:<text>` | typed text |
//! | `shift:<n>` | `n` bare Shift taps (modifier events; `shift:2` is a double Shift) |
//! | `mouse:back` `mouse:forward` | the mouse's side buttons |
//! | `cmd:<id>` | run a command by id |
//! | `open:<path>` `preview:<path>` | open a file (relative to the workspace) |
//! | `toast:<text>` | show an info toast |
//! | `wait:<ms>` | wait |
//! | `dump:<label>` | append the full state dump to the log |
use crate::app::OrbitApp;
use crate::commands;
use crate::probe;
use crate::toast::ToastKind;
use gpui::{
    px, AnyWindowHandle, AsyncApp, Focusable, Entity, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NavigationDirection, PlatformInput, Pixels, Point,
};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

fn send(window: AnyWindowHandle, event: PlatformInput, cx: &mut AsyncApp) {
    let _ = window.update(cx, |_, window, cx| window.dispatch_event(event, cx));
}

/// Where a probe is now: drops old bounds and waits for a fresh frame.
async fn locate(window: AnyWindowHandle, name: &str, cx: &mut AsyncApp) -> Option<Point<Pixels>> {
    probe::clear();
    let _ = window.update(cx, |_, window, _| window.refresh());
    sleep(cx, 120).await;
    probe::bounds(name).map(|b| b.center())
}

async fn click(window: AnyWindowHandle, at: Point<Pixels>, button: MouseButton, count: usize, cx: &mut AsyncApp) {
    let modifiers = Modifiers::default();
    send(window, PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers }), cx);
    for n in 1..=count {
        send(
            window,
            PlatformInput::MouseDown(MouseDownEvent { button, position: at, modifiers, click_count: n, first_mouse: false }),
            cx,
        );
        send(window, PlatformInput::MouseUp(MouseUpEvent { button, position: at, modifiers, click_count: n }), cx);
        // A frame between the clicks, as with real input.
        sleep(cx, 60).await;
    }
    sleep(cx, 150).await;
}

async fn drag(window: AnyWindowHandle, from: Point<Pixels>, by: Point<Pixels>, cx: &mut AsyncApp) {
    let modifiers = Modifiers::default();
    let left = MouseButton::Left;
    send(window, PlatformInput::MouseMove(MouseMoveEvent { position: from, pressed_button: None, modifiers }), cx);
    send(window, PlatformInput::MouseDown(MouseDownEvent { button: left, position: from, modifiers, click_count: 1, first_mouse: false }), cx);
    sleep(cx, 30).await;
    let steps = 6;
    for i in 1..=steps {
        let p = from + by * (i as f32 / steps as f32);
        send(window, PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(left), modifiers }), cx);
        sleep(cx, 20).await;
    }
    send(window, PlatformInput::MouseUp(MouseUpEvent { button: left, position: from + by, modifiers, click_count: 1 }), cx);
    sleep(cx, 150).await;
}

pub async fn run(steps: &[String], window: AnyWindowHandle, app: &Entity<OrbitApp>, log: &mut String, cx: &mut AsyncApp) {
    for step in steps {
        let (verb, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        let _ = writeln!(log, "step {step}");
        match verb {
            "click" | "rclick" | "mclick" | "dblclick" => {
                let (button, count) = match verb {
                    "rclick" => (MouseButton::Right, 1),
                    "mclick" => (MouseButton::Middle, 1),
                    "dblclick" => (MouseButton::Left, 2),
                    _ => (MouseButton::Left, 1),
                };
                match locate(window, arg, cx).await {
                    Some(at) => click(window, at, button, count, cx).await,
                    None => {
                        let _ = writeln!(log, "  no element {arg:?} on screen");
                    }
                }
            }
            "drag" => {
                let parsed = arg.rsplit_once(':').and_then(|(name, d)| {
                    let (dx, dy) = d.split_once(',')?;
                    Some((name, dx.parse::<f32>().ok()?, dy.parse::<f32>().ok()?))
                });
                let Some((name, dx, dy)) = parsed else {
                    let _ = writeln!(log, "  bad drag {arg:?}");
                    continue;
                };
                match locate(window, name, cx).await {
                    Some(at) => drag(window, at, gpui::point(px(dx), px(dy)), cx).await,
                    None => {
                        let _ = writeln!(log, "  no element {name:?} on screen");
                    }
                }
            }
            "key" => match Keystroke::parse_with_os(arg, gpui::OperatingSystem::Linux) {
                Ok(ks) => {
                    let _ = window.update(cx, |_, window, cx| window.dispatch_keystroke(ks, cx));
                    sleep(cx, 150).await;
                }
                Err(e) => {
                    let _ = writeln!(log, "  bad key: {e:?}");
                }
            },
            "shift" => {
                for _ in 0..arg.parse().unwrap_or(1) {
                    for shift in [true, false] {
                        let modifiers = Modifiers { shift, ..Default::default() };
                        let event = gpui::ModifiersChangedEvent { modifiers, capslock: Default::default() };
                        send(window, PlatformInput::ModifiersChanged(event), cx);
                        sleep(cx, 20).await;
                    }
                }
                sleep(cx, 150).await;
            }
            "type" => {
                for ch in arg.chars() {
                    let ks = Keystroke { key: ch.to_lowercase().to_string(), key_char: Some(ch.to_string()), ..Default::default() };
                    let _ = window.update(cx, |_, window, cx| window.dispatch_keystroke(ks, cx));
                    sleep(cx, 10).await;
                }
                sleep(cx, 150).await;
            }
            "mouse" => {
                let dir = if arg == "forward" { NavigationDirection::Forward } else { NavigationDirection::Back };
                let at = gpui::point(px(700.), px(400.));
                click(window, at, MouseButton::Navigate(dir), 1, cx).await;
            }
            "cmd" => {
                let ok = window
                    .update(cx, |_, window, cx| app.update(cx, |a, cx| commands::run(arg, a, window, cx)))
                    .unwrap_or(false);
                if !ok {
                    let _ = writeln!(log, "  command {arg:?} not run");
                }
                sleep(cx, 150).await;
            }
            "open" | "preview" => {
                let rel = PathBuf::from(arg);
                let _ = window.update(cx, |_, window, cx| {
                    app.update(cx, |a, cx| {
                        let path = a.workspace().map_or(rel.clone(), |w| w.dir.join(&rel));
                        a.open_file(path, verb == "preview", window, cx);
                    })
                });
                sleep(cx, 200).await;
            }
            "toast" => {
                app.update(cx, |a, cx| a.toast(ToastKind::Info, arg.to_string(), None, cx));
                sleep(cx, 50).await;
            }
            "wait" => sleep(cx, arg.parse().unwrap_or(100)).await,
            "dump" => {
                let text = app.read_with(cx, |a, cx| crate::script::dump(a, "", cx));
                let focus = focus_text(window, app, cx);
                let _ = writeln!(log, "=== dump {arg}\n{text}{focus}=== end {arg}");
            }
            _ => {
                let _ = writeln!(log, "  unknown step");
            }
        }
    }
}

/// Which part of the window has the keyboard.
pub fn focus_text(window: AnyWindowHandle, app: &Entity<OrbitApp>, cx: &mut AsyncApp) -> String {
    window
        .update(cx, |_, window, cx| {
            let a = app.read(cx);
            let decorations = match window.window_decorations() {
                gpui::Decorations::Server => "server",
                gpui::Decorations::Client { .. } => "client",
            };
            let focus = if a.focus.is_focused(window) {
                "app".to_string()
            } else if a.overlay_focus.is_focused(window) {
                "overlay".to_string()
            } else if let Some(ix) = a.tabs.iter().position(|t| t.focus_handle(cx).contains_focused(window, cx)) {
                format!("tab {ix}")
            } else if a.palette.as_ref().is_some_and(|p| p.input.focus_handle(cx).is_focused(window)) {
                "palette input".to_string()
            } else {
                format!("other ({:?})", window.focused(cx).is_some())
            };
            format!("--- focus: {focus}, decorations: {decorations}, window {:?}\n", window.viewport_size())
        })
        .unwrap_or_default()
}

/// The shell's part of the state dump.
pub fn shell_dump(app: &OrbitApp, cx: &gpui::App) -> String {
    let mut out = String::new();
    let l = &app.layout;
    let _ = writeln!(
        out,
        "--- layout: sidebar {}{} | dock {}{} | panel {}{}",
        l.sidebar_width,
        if l.sidebar_hidden { " (hidden)" } else if l.sidebar_collapsed() { " (icons)" } else { "" },
        l.dock_width,
        if l.dock_hidden { " (hidden)" } else { "" },
        l.panel_height,
        if l.panel_open { " (open)" } else { " (closed)" },
    );
    let (back, fwd) = app.nav.depth();
    let current = app.nav.current.as_ref().map(|c| format!("{} {:?}", c.key, c.position)).unwrap_or_default();
    let _ = writeln!(out, "--- nav: {back} back, {fwd} forward; current {current}");
    if let Some(m) = &app.menu {
        let items: Vec<&str> = m.items.iter().map(|i| i.label.as_ref()).collect();
        let _ = writeln!(out, "--- menu: {items:?} highlighted {:?}", m.highlighted);
    }
    if let Some(d) = &app.dialog {
        let buttons: Vec<&str> = d.buttons.iter().map(|b| b.label.as_ref()).collect();
        let _ = writeln!(out, "--- dialog: {:?} {buttons:?}", d.title);
    }
    if let Some(p) = &app.palette {
        out.push_str(&p.dump_text(cx));
    }
    for t in &app.toasts.items {
        let _ = writeln!(out, "--- toast {:?}: {}", t.kind, t.title);
    }
    let _ = writeln!(
        out,
        "--- commands: {} registered; state file {}",
        commands::all(cx).len(),
        app.state_path().map_or("-".into(), |p| p.display().to_string())
    );
    out
}
