//! Self-test steps for Build/Run/Debug (`--do`, after `steps.rs`' own):
//!
//! | Step | Does |
//! |---|---|
//! | `until:<text>[@secs]` | waits (default 60 s) until the state dump contains `text` |
//! | `gutter:<line>` | clicks the active editor's breakpoint strip at a 1-based line |
//! | `move:<probe>` | moves the mouse over a probe's centre (hover) |
//! | `paste:<text>` | inserts `text` into the focused input at once, as a paste does |
use crate::app::OrbitApp;
use gpui::{AnyWindowHandle, AsyncApp, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Runs one step; false when the verb isn't one of these.
pub async fn step(verb: &str, arg: &str, window: AnyWindowHandle, app: &Entity<OrbitApp>, log: &mut String, cx: &mut AsyncApp) -> bool {
    match verb {
        "until" => {
            let (text, secs) = match arg.rsplit_once('@') {
                Some((t, s)) if s.parse::<u64>().is_ok() => (t, s.parse().unwrap_or(60)),
                _ => (arg, 60),
            };
            let started = Instant::now();
            loop {
                let dump = app.read_with(cx, |a, cx| crate::script::dump(a, "", cx));
                if dump.contains(text) {
                    let _ = writeln!(log, "  after {} ms", started.elapsed().as_millis());
                    break;
                }
                if started.elapsed() > Duration::from_secs(secs) {
                    let _ = writeln!(log, "  timed out after {secs} s");
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(200)).await;
            }
            true
        }
        "move" => {
            // The mouse over a probe's centre (hover states for captures).
            let Some(b) = crate::probe::bounds(arg) else {
                let _ = writeln!(log, "  no probe {arg}");
                return true;
            };
            let position = b.center();
            let ev = PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() });
            let _ = window.update(cx, |_, window, cx| {
                window.dispatch_event(ev, cx);
                window.refresh();
            });
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = window.update(cx, |_, window, _| window.refresh());
            true
        }
        "paste" => {
            // One keystroke carrying the whole text: the focused input gets it
            // in one insert, as from a paste (without the clipboard).
            let ks = gpui::Keystroke { key: "v".into(), key_char: Some(arg.to_string()), ..Default::default() };
            let _ = window.update(cx, |_, window, cx| window.dispatch_keystroke(ks, cx));
            cx.background_executor().timer(Duration::from_millis(200)).await;
            true
        }
        "gutter" => {
            let line: u32 = arg.parse().unwrap_or(1);
            // A fresh layout first: the editor may not have painted its lines yet.
            let _ = window.update(cx, |_, window, _| window.refresh());
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let at = app.read_with(cx, |a, cx| a.active_editor().and_then(|e| crate::debug::gutter::strip_point(e.read(cx), line, cx)));
            let Some(at) = at else {
                let _ = writeln!(log, "  no editor laid out");
                return true;
            };
            let (button, modifiers) = (MouseButton::Left, Modifiers::default());
            let send = |event: PlatformInput, cx: &mut AsyncApp| {
                let _ = window.update(cx, |_, window, cx| window.dispatch_event(event, cx));
            };
            send(PlatformInput::MouseDown(MouseDownEvent { button, position: at, modifiers, click_count: 1, first_mouse: false }), cx);
            send(PlatformInput::MouseUp(MouseUpEvent { button, position: at, modifiers, click_count: 1 }), cx);
            cx.background_executor().timer(Duration::from_millis(200)).await;
            true
        }
        _ => false,
    }
}
