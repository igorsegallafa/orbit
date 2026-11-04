// Terminal sessions for the webview: `orbit_core::pty` runs them; output
// streams to the pane through a per-session channel (an empty message once
// it ended), input flows back through `pty_write`.
use orbit_core::pty::{self, Output, Size, Spawn};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per invoke field
pub async fn pty_spawn(
    app: AppHandle,
    workspace: String,
    repo: Option<String>,
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cols: u16,
    rows: u16,
    pixel_width: Option<u16>,
    pixel_height: Option<u16>,
    key: Option<String>,
    // A run configuration's working folder and variables (the game runs in
    // its data folder, outside the repo).
    cwd: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
    on_output: Channel<InvokeResponseBody>,
) -> Result<u32, String> {
    let cwd = pty::session_dir(&workspace, repo.as_deref(), cwd.as_deref())?;
    let id = pty::reserve_id();
    let spec = Spawn {
        cwd,
        cmd,
        args: args.unwrap_or_default(),
        env: env.into_iter().flatten().collect(),
        size: Size { cols, rows, pixel_width: pixel_width.unwrap_or(0), pixel_height: pixel_height.unwrap_or(0) },
        scrollback: key.is_some(),
        key,
        program_version: env!("CARGO_PKG_VERSION").into(),
    };
    let spawned = tauri::async_runtime::spawn_blocking(move || {
        pty::spawn(id, spec, move |out| {
            let _ = on_output.send(InvokeResponseBody::Raw(match out {
                Output::Data(bytes) => bytes,
                Output::Exit => Vec::new(),
            }));
        })
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?;
    // A failed start is reported as an event; the pane keeps its id.
    if let Err(e) = spawned {
        let _ = app.emit("pty-error", serde_json::json!({ "error": e }));
    }
    Ok(id)
}

/// Saved output of a tab's previous session (empty when none).
#[tauri::command]
pub fn pty_scrollback(key: String) -> Vec<u8> {
    pty::scrollback(&key)
}

/// A tab closed for good: drop what was kept to restore it.
#[tauri::command]
pub fn pty_forget(key: String) {
    pty::forget(&key)
}

/// True when Claude Code has a transcript for this session id in the
/// workspace folder, i.e. `--resume <id>` will find it.
#[tauri::command]
pub fn claude_session_exists(workspace: String, session_id: String) -> bool {
    pty::claude_session_exists(&workspace, &session_id)
}

/// The opencode session a terminal tab started, to resume it after the tab closed.
#[tauri::command]
pub async fn opencode_session_id(workspace: String, repo: Option<String>, since_ms: u64) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || pty::opencode_session_id(&workspace, repo.as_deref(), since_ms))
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub fn pty_write(id: u32, data: String) -> Result<(), String> {
    pty::write(id, data.as_bytes())
}

/// The pane's draw queue is full (`true`) or has drained (`false`).
#[tauri::command]
pub fn pty_flow(id: u32, paused: bool) {
    pty::set_paused(id, paused)
}

#[tauri::command]
pub fn pty_resize(id: u32, cols: u16, rows: u16, pixel_width: Option<u16>, pixel_height: Option<u16>) -> Result<(), String> {
    pty::resize(id, Size { cols, rows, pixel_width: pixel_width.unwrap_or(0), pixel_height: pixel_height.unwrap_or(0) })
}

#[tauri::command]
pub fn pty_kill(id: u32) -> Result<(), String> {
    pty::kill(id)
}
