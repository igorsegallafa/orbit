// Lists repos from GitHub using the `gh` CLI (reuses the user's existing
// `gh auth login` session — no OAuth flow to build/maintain ourselves).
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubRepo {
    pub name: String,
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    #[serde(rename = "sshUrl")]
    pub ssh_url: String,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
}

pub fn is_authenticated() -> bool {
    Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn list_my_repos() -> Result<Vec<GithubRepo>, String> {
    let out = Command::new("gh")
        .args([
            "repo",
            "list",
            "--limit",
            "200",
            "--json",
            "name,nameWithOwner,sshUrl,isPrivate",
        ])
        .output()
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("failed to parse gh output: {e}"))
}
