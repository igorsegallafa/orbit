// Tauri commands for orbit_core::search.
use crate::blocking;
use orbit_core::search::{search, SearchQuery, SearchResult};

#[tauri::command]
pub async fn search_workspace(workspace: String, query: SearchQuery) -> Result<SearchResult, String> {
    blocking(move || search(&workspace, &query)).await
}
