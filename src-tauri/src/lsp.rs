// Tauri commands for language servers (orbit_core::lsp). Messages from a
// server reach the webview as `lsp-message` events, its exit as `lsp-exit`.
use crate::blocking;
use orbit_core::lsp::{self, CppSetup, LspStatus, StartInfo};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub use lsp::shutdown_all;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LspMessage {
    id: u32,
    /// Raw JSON-RPC message (parsed by the frontend).
    message: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LspExit {
    id: u32,
    code: Option<i32>,
    /// Last stderr lines, to explain a crash.
    log: Vec<String>,
}

/// Starts the server for `language` in the repo's folder (worktree, or the
/// clone for a "@repo" scope).
#[tauri::command]
pub async fn lsp_start(app: AppHandle, workspace: String, repo: String, language: String) -> Result<StartInfo, String> {
    blocking(move || {
        let exit_app = app.clone();
        lsp::start(
            &workspace,
            &repo,
            &language,
            move |id, message| {
                let _ = app.emit("lsp-message", LspMessage { id, message });
            },
            move |id, code, log| {
                let _ = exit_app.emit("lsp-exit", LspExit { id, code, log });
            },
        )
    })
    .await
}

#[tauri::command]
pub fn lsp_send(id: u32, message: String) -> Result<(), String> {
    lsp::send(id, &message)
}

#[tauri::command]
pub fn lsp_stop(id: u32) {
    lsp::stop(id)
}

/// Languages that have a server to run (the editor sets itself up at startup).
#[tauri::command]
pub async fn lsp_available() -> Result<Vec<String>, String> {
    blocking(lsp::available).await
}

#[tauri::command]
pub async fn lsp_status() -> Result<LspStatus, String> {
    blocking(lsp::status).await
}

/// Recent stderr of a running server (why it misbehaves).
#[tauri::command]
pub fn lsp_log(id: u32) -> Vec<String> {
    lsp::log(id)
}

#[tauri::command]
pub async fn lsp_configure(language: String, command: Option<String>, disabled: bool) -> Result<(), String> {
    blocking(move || lsp::configure(&language, command, disabled)).await
}

/// Which diagnostics the editor shows for a language.
#[tauri::command]
pub async fn lsp_diagnostics_filter(language: String, errors_only: bool, hidden: Vec<String>) -> Result<(), String> {
    blocking(move || lsp::set_diagnostics_filter(&language, errors_only, hidden)).await
}

/// A file a server pointed at, read for the peek view.
#[tauri::command]
pub async fn lsp_read_file(path: String) -> Result<String, String> {
    blocking(move || lsp::read_file(&path)).await
}

/// What clangd needs for a repo: its compilation database, or how to make one.
#[tauri::command]
pub async fn lsp_cpp_setup(workspace: String, repo: String) -> Result<CppSetup, String> {
    blocking(move || lsp::cpp_setup(&workspace, &repo)).await
}

/// Writes the script that runs `cmake args` inside the VS developer environment.
#[tauri::command]
pub async fn lsp_cmake_script(vcvars: String, args: Vec<String>) -> Result<String, String> {
    lsp::write_cmake_script(&vcvars, &args)
}
