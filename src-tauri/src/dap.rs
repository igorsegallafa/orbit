// Tauri commands for debug adapters (orbit_core::dap). Messages reach the
// webview as `dap-message` events, the adapter's exit as `dap-exit`.
use orbit_core::dap::{self, Adapter};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone)]
struct DapMessage {
    id: u32,
    message: String,
}

#[derive(Serialize, Clone)]
struct DapExit {
    id: u32,
    code: Option<i32>,
}

#[tauri::command]
pub async fn dap_adapter(requested: Option<String>) -> Result<Adapter, String> {
    dap::adapter(requested.as_deref())
}

#[tauri::command]
pub async fn dap_start(app: AppHandle, requested: Option<String>, cwd: String) -> Result<(u32, Adapter), String> {
    let a = dap::adapter(requested.as_deref())?;
    let exit_app = app.clone();
    let id = dap::start(
        &a,
        &cwd,
        move |id, message| {
            let _ = app.emit("dap-message", DapMessage { id, message });
        },
        move |id, code| {
            let _ = exit_app.emit("dap-exit", DapExit { id, code });
        },
    )?;
    Ok((id, a))
}

#[tauri::command]
pub async fn dap_send(id: u32, message: String) -> Result<(), String> {
    dap::send(id, &message)
}

#[tauri::command]
pub async fn dap_stop(id: u32) -> Result<(), String> {
    dap::stop(id);
    Ok(())
}
