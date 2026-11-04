//! Helpers for the pages: load labels, background work and ages. The
//! settings-style controls are in `kit.rs`.
use gpui::AppContext as _;
pub fn load_label<T>(l: &crate::review::Load<T>) -> String {
    use crate::review::Load;
    match l {
        Load::Idle => "idle".into(),
        Load::Loading => "loading".into(),
        Load::Ready(_) => "ready".into(),
        Load::Failed(e) => format!("failed: {e}"),
    }
}

/// Runs `work` off the UI thread, then `done` on the view.
pub fn bg<V: 'static, T: Send + 'static>(
    cx: &mut gpui::Context<V>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut V, T, &mut gpui::Window, &mut gpui::Context<V>) + 'static,
    window: &mut gpui::Window,
) {
    cx.spawn_in(window, async move |this, cx| {
        let r = cx.background_spawn(async move { work() }).await;
        let _ = this.update_in(cx, |v, window, cx| {
            done(v, r, window, cx);
            cx.notify();
        });
    })
    .detach();
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "3m ago" from a unix time.
pub fn ago(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let s = (now() as i64 - unix).max(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ages_read_like_the_react_app() {
        let now = super::now() as i64;
        assert_eq!(super::ago(0), "");
        assert_eq!(super::ago(now - 5), "just now");
        assert_eq!(super::ago(now - 125), "2m ago");
        assert_eq!(super::ago(now - 7200), "2h ago");
        assert_eq!(super::ago(now - 3 * 86_400), "3d ago");
    }
}
