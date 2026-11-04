//! Orbit's config and workspace metadata (same files and rules as
//! `src-tauri/src/config.rs` and `workspace.rs`), read-only for now.
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default, alias = "workspaces_dir")]
    workspaces_dir: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct WorkspaceMeta {
    pub name: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub repos: Vec<String>,
}

#[derive(Clone)]
pub struct Workspace {
    pub meta: WorkspaceMeta,
    pub dir: PathBuf,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn config_dir() -> PathBuf {
    std::env::var_os("ORBIT_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/orbit"))
}

fn workspaces_dir() -> PathBuf {
    let cfg: Config = std::fs::read_to_string(config_dir().join("config.yaml"))
        .ok()
        .and_then(|raw| serde_yaml::from_str(&raw).ok())
        .unwrap_or_default();
    if let Some(dir) = cfg.workspaces_dir.filter(|d| !d.trim().is_empty()) {
        return PathBuf::from(dir);
    }
    let root = std::env::var_os("ORBIT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("Documents/orbit-workspace"));
    root.join("workspaces")
}

/// Every folder of the workspaces dir that holds a readable `.workspace.yaml`.
pub fn list_workspaces() -> Vec<Workspace> {
    let Ok(entries) = std::fs::read_dir(workspaces_dir()) else {
        return vec![];
    };
    let mut out: Vec<Workspace> = entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let raw = std::fs::read_to_string(dir.join(".workspace.yaml")).ok()?;
            let meta = serde_yaml::from_str(&raw).ok()?;
            Some(Workspace { meta, dir })
        })
        .collect();
    out.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    out
}
