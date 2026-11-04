// Tauri commands for review notes (orbit_core::notes).
use crate::blocking;
use orbit_core::notes::{self, NotesFile};

#[tauri::command]
pub async fn notes_list(workspace: String) -> Result<NotesFile, String> {
    blocking(move || notes::list(&workspace)).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per invoke field
pub async fn notes_add(
    workspace: String,
    repo: String,
    path: String,
    start: u32,
    end: u32,
    body: String,
    snippet: String,
) -> Result<NotesFile, String> {
    blocking(move || notes::add(&workspace, repo, path, start, end, body, snippet)).await
}

#[tauri::command]
pub async fn notes_update(workspace: String, id: String, done: Option<bool>, body: Option<String>) -> Result<NotesFile, String> {
    blocking(move || notes::update(&workspace, &id, done, body)).await
}

#[tauri::command]
pub async fn notes_delete(workspace: String, ids: Vec<String>) -> Result<NotesFile, String> {
    blocking(move || notes::delete(&workspace, &ids)).await
}
