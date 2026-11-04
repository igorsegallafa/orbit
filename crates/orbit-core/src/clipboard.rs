//! Clipboard text for terminals on Linux. The webview pastes on Ctrl+V only,
//! but Toshy (macOS-style keys) turns Cmd+V into Ctrl+Shift+V in terminals.

/// The clipboard's text, or None when it holds none (an image, say) or no
/// clipboard tool is available.
pub fn text() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (prog, args): (&str, &[&str]) = if wayland {
        ("wl-paste", &["--no-newline", "--type", "text"])
    } else {
        ("xclip", &["-o", "-selection", "clipboard"])
    };
    let out = crate::proc::cmd(prog).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
