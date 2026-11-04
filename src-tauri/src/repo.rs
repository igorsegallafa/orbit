// Tauri commands for orbit_core::repo.
use crate::blocking;
use crate::{git, github};
pub use orbit_core::repo::*;

#[tauri::command]
pub async fn repo_overview(name: String) -> Result<RepoOverview, String> {
    blocking(move || overview(&name)).await
}

#[tauri::command]
pub async fn repo_switch(name: String, branch: String, stash: bool) -> Result<(), String> {
    blocking(move || switch(&name, &branch, stash)).await
}

#[tauri::command]
pub async fn repo_create_branch(name: String, branch: String, from: Option<String>) -> Result<(), String> {
    blocking(move || create_branch(&name, &branch, from.as_deref())).await
}

#[tauri::command]
pub async fn repo_delete_branch(name: String, branch: String, force: bool) -> Result<(), String> {
    blocking(move || delete_branch(&name, &branch, force)).await
}

#[tauri::command]
pub async fn repo_pull(name: String, rebase: bool) -> Result<(), String> {
    blocking(move || pull(&name, rebase)).await
}

#[tauri::command]
pub async fn repo_commit(name: String, message: String, paths: Vec<String>) -> Result<(), String> {
    blocking(move || commit(&name, &message, &paths)).await
}

#[tauri::command]
pub async fn repo_discard(name: String, paths: Vec<String>) -> Result<(), String> {
    blocking(move || discard(&name, &paths)).await
}

#[tauri::command]
pub async fn repo_stash(name: String) -> Result<(), String> {
    blocking(move || stash(&name)).await
}

#[tauri::command]
pub async fn repo_stash_pop(name: String, index: usize) -> Result<(), String> {
    blocking(move || stash_pop(&name, index)).await
}

#[tauri::command]
pub async fn repo_stash_drop(name: String, index: usize) -> Result<(), String> {
    blocking(move || stash_drop(&name, index)).await
}

#[tauri::command]
pub async fn repo_undo_commit(name: String) -> Result<(), String> {
    blocking(move || undo_commit(&name)).await
}

#[tauri::command]
pub async fn repo_history(name: String, limit: usize) -> Result<Vec<git::CommitEntry>, String> {
    blocking(move || history(&name, limit)).await
}

#[tauri::command]
pub async fn repo_branch_prs(name: String) -> Result<Vec<github::WsPrStatus>, String> {
    blocking(move || branch_prs(&name)).await
}

#[tauri::command]
pub async fn repo_prune_worktrees(name: String) -> Result<(), String> {
    blocking(move || prune_worktrees(&name)).await
}

/// Sidebar status of every configured repo, in parallel.
#[tauri::command]
pub async fn repo_briefs() -> Result<Vec<RepoBrief>, String> {
    blocking(briefs)
    .await
}

#[tauri::command]
pub async fn repo_compare(name: String, other: String) -> Result<Comparison, String> {
    blocking(move || compare(&name, &other)).await
}

#[tauri::command]
pub async fn repo_cherry_pick(name: String, sha: String) -> Result<(), String> {
    blocking(move || cherry_pick(&name, &sha)).await
}

#[tauri::command]
pub async fn repo_revert(name: String, sha: String) -> Result<(), String> {
    blocking(move || revert(&name, &sha)).await
}

#[tauri::command]
pub async fn repo_operation(name: String, action: String) -> Result<(), String> {
    blocking(move || operation(&name, &action)).await
}
