// Tauri commands for orbit_core::review.
use crate::blocking;
pub use orbit_core::review::*;

#[tauri::command]
pub async fn pr_review_data(owner_repo: String, number: u64, with_lines: bool) -> Result<ReviewData, String> {
    blocking(move || review_data(&owner_repo, number, with_lines)).await
}

#[tauri::command]
pub async fn pr_add_comment(owner_repo: String, number: u64, commit_id: String, comment: DraftComment) -> Result<(), String> {
    blocking(move || add_comment(&owner_repo, number, &commit_id, &comment)).await
}

#[tauri::command]
pub async fn pr_reply(owner_repo: String, number: u64, comment_id: u64, body: String) -> Result<(), String> {
    blocking(move || reply(&owner_repo, number, comment_id, &body)).await
}

#[tauri::command]
pub async fn pr_edit_comment(owner_repo: String, comment_id: u64, body: String) -> Result<(), String> {
    blocking(move || edit_comment(&owner_repo, comment_id, &body)).await
}

#[tauri::command]
pub async fn pr_delete_comment(owner_repo: String, comment_id: u64) -> Result<(), String> {
    blocking(move || delete_comment(&owner_repo, comment_id)).await
}

#[tauri::command]
pub async fn pr_resolve_thread(thread_id: String, resolved: bool) -> Result<(), String> {
    blocking(move || set_thread_resolved(&thread_id, resolved)).await
}

#[tauri::command]
pub async fn pr_submit_review(
    owner_repo: String,
    number: u64,
    commit_id: String,
    event: String,
    body: String,
    comments: Vec<DraftComment>,
) -> Result<(), String> {
    blocking(move || submit_review(&owner_repo, number, &commit_id, &event, &body, &comments)).await
}
