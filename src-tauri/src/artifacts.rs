// Tauri commands for CI artifacts (orbit_core::artifacts).
use crate::blocking;
use orbit_core::artifacts::{self, Artifact};

/// Unexpired artifacts from the latest run of each workflow on `branch`.
#[tauri::command]
pub async fn branch_artifacts(owner_repo: String, branch: String) -> Result<Vec<Artifact>, String> {
    blocking(move || Ok(artifacts::branch_artifacts(&owner_repo, &branch))).await
}

/// Downloads one artifact, then opens its folder in the file manager.
#[tauri::command]
pub async fn artifact_download(owner_repo: String, run_id: u64, name: String, folder: String) -> Result<String, String> {
    blocking(move || {
        let dest = artifacts::download(&owner_repo, run_id, &name, &folder)?;
        let _ = tauri_plugin_opener::open_path(&dest, None::<&str>);
        Ok(dest.to_string_lossy().to_string())
    })
    .await
}
