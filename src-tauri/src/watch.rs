// Tauri commands for file watching (orbit_core::watch).
use crate::events;
use tauri::AppHandle;

#[tauri::command]
pub fn fs_watch(app: AppHandle, workspace: String) -> Result<(), String> {
    orbit_core::watch::watch(events(app), &workspace)
}

#[tauri::command]
pub fn fs_unwatch(workspace: String) -> Result<(), String> {
    orbit_core::watch::unwatch(&workspace)
}
