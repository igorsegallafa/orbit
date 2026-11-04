//! The crash log (`ErrorBoundary.tsx` for the native app): a panic hook
//! appends the message, where it happened and a backtrace to
//! `<config dir>/native-crash.log`, and the next start shows a toast with the
//! log's path. `ORBIT_TEST_PANIC=<ms>` panics on the UI thread after that
//! long, to test it.
use crate::app::OrbitApp;
use crate::toast::ToastKind;
use gpui::{App, Context, Window};
use std::path::PathBuf;
use std::time::Duration;

fn log_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("native-crash.log"))
}

/// Left by the hook; the next start reports the crash and removes it.
fn marker() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("native-crash.pending"))
}

/// Installs the panic hook (before anything else runs).
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let trace = std::backtrace::Backtrace::force_capture();
        let text = format!("panic in thread '{thread}' at {at}: {payload}\nversion {}\n\n{trace}", super::updates::version());
        if let Some(log) = log_path() {
            if let Some(dir) = log.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if orbit_core::crash::append(&log, &text).is_ok() {
                if let Some(m) = marker() {
                    let _ = std::fs::write(m, format!("{payload}\n"));
                }
                eprintln!("orbit: crash log written to {}", log.display());
            }
        }
        default(info);
    }));
}

/// Shows the last crash, if any, once the root view exists.
pub fn init(cx: &mut App) {
    cx.observe_new(|_: &mut OrbitApp, _: Option<&mut Window>, cx: &mut Context<OrbitApp>| {
        if let (Some(m), Some(log)) = (marker(), log_path()) {
            if let Ok(message) = std::fs::read_to_string(&m) {
                let _ = std::fs::remove_file(&m);
                let title = format!("Orbit closed after a crash: {}", message.trim());
                cx.spawn(async move |this, cx| {
                    // After the first frame, so the toast isn't lost in the start.
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    let _ = this.update(cx, |a, cx| a.toast(ToastKind::Error, title, Some(format!("Log: {}", log.display()).into()), cx));
                })
                .detach();
            }
        }
        if let Some(ms) = std::env::var("ORBIT_TEST_PANIC").ok().and_then(|v| v.parse::<u64>().ok()) {
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(ms)).await;
                cx.update(|_| panic!("ORBIT_TEST_PANIC: a test crash"));
            })
            .detach();
        }
    })
    .detach();
}
