// Tauri commands for workspace files (orbit_core::files).
use orbit_core::files::{self, FileNode, FileSearchEntry};

#[tauri::command]
pub async fn list_files(workspace: String, repo: String, path: String) -> Result<Vec<FileNode>, String> {
    files::list_files(&workspace, &repo, &path)
}

#[tauri::command]
pub async fn read_file(workspace: String, repo: String, path: String) -> Result<String, String> {
    files::read_file(&workspace, &repo, &path)
}

#[tauri::command]
pub async fn write_file(workspace: String, repo: String, path: String, content: String) -> Result<(), String> {
    files::write_file(&workspace, &repo, &path, &content)
}

#[tauri::command]
pub async fn move_file(workspace: String, repo: String, src: String, dest_dir: String) -> Result<(), String> {
    files::move_file(&workspace, &repo, &src, &dest_dir)
}

#[tauri::command]
pub async fn rename_node(workspace: String, repo: String, path: String, new_name: String) -> Result<(), String> {
    files::rename_node(&workspace, &repo, &path, &new_name)
}

#[tauri::command]
pub async fn delete_node(workspace: String, repo: String, path: String) -> Result<(), String> {
    files::delete_node(&workspace, &repo, &path)
}

#[tauri::command]
pub async fn create_node(workspace: String, repo: String, dir: String, name: String, is_dir: bool) -> Result<(), String> {
    files::create_node(&workspace, &repo, &dir, &name, is_dir)
}

#[tauri::command]
pub async fn node_abs_path(workspace: String, repo: String, path: String) -> Result<String, String> {
    files::node_abs_path(&workspace, &repo, &path)
}

#[tauri::command]
pub async fn import_files(
    workspace: String,
    repo: String,
    dest_dir: String,
    sources: Vec<String>,
) -> Result<usize, String> {
    files::import_files(&workspace, &repo, &dest_dir, &sources)
}

#[tauri::command]
pub async fn list_workspace_files(workspace: String) -> Result<Vec<FileSearchEntry>, String> {
    files::list_workspace_files(&workspace)
}

/// Reveals a file/folder in the OS file manager (selected in its parent).
#[tauri::command]
pub async fn reveal_node(workspace: String, repo: String, path: String) -> Result<(), String> {
    let p = files::resolve(&workspace, &repo, &path)?;
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    tauri_plugin_opener::reveal_item_in_dir(&p).map_err(|e| format!("failed to reveal: {e}"))
}

/// Frontend render crashes (ErrorBoundary) land here so the full stack —
/// including app frames the on-screen card truncates — is diagnosable.
#[tauri::command]
pub async fn log_render_crash(message: String) -> Result<(), String> {
    orbit_core::crash::append(&orbit_core::crash::render_crash_log(), &message)
}
