// Workspace = one feature worked across multiple repos, each repo getting
// its own git worktree under <root>/workspaces/<name>/<repo>.
// Metadata lives in <root>/workspaces/<name>/.workspace.yaml (same source
// of truth pattern as the generosity-workspace CLI).
use crate::config::Config;
use crate::git;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Workspace {
    pub name: String,
    pub branch: String,
    pub base: String,
    pub repos: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepoStatus {
    pub repo: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: usize,
    pub behind: usize,
}

pub fn workspace_root() -> Result<PathBuf, String> {
    // ponytail: single fixed workspace root for now; configurable when
    // multi-root support lands.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    Ok(home.join("Documents/orbit-workspace"))
}

pub fn repos_dir() -> Result<PathBuf, String> {
    Ok(workspace_root()?.join("repos"))
}

fn workspaces_dir() -> Result<PathBuf, String> {
    Ok(workspace_root()?.join("workspaces"))
}

fn meta_path(ws_dir: &Path) -> PathBuf {
    ws_dir.join(".workspace.yaml")
}

/// Loads the metadata for a workspace directory.
pub fn load_meta(ws_dir: &Path) -> Result<Workspace, String> {
    let raw = std::fs::read_to_string(meta_path(ws_dir))
        .map_err(|e| format!("failed to read {}: {e}", meta_path(ws_dir).display()))?;
    serde_yaml::from_str(&raw).map_err(|e| format!("invalid .workspace.yaml: {e}"))
}

/// Lists all workspaces by scanning the workspaces dir for .workspace.yaml.
pub fn list() -> Result<Vec<Workspace>, String> {
    let dir = workspaces_dir()?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let ws_dir = entry.path();
        if !ws_dir.is_dir() || !meta_path(&ws_dir).exists() {
            continue;
        }
        if let Ok(ws) = load_meta(&ws_dir) {
            out.push(ws);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Creates a workspace: one worktree per selected repo, on branch
/// `<branch>` based on `base`. Clones repos that aren't cloned yet.
/// Idempotent: existing worktrees are reused.
pub fn create(name: &str, branch: &str, base: &str, repos: &[String]) -> Result<Workspace, String> {
    if name.trim().is_empty() {
        return Err("workspace name is required".into());
    }
    if repos.is_empty() {
        return Err("select at least one repository".into());
    }

    let cfg = Config::load()?;
    let rdir = repos_dir()?;
    let ws_dir = workspaces_dir()?.join(name);

    for repo in repos {
        let Some(svc) = cfg.services.iter().find(|s| &s.name == repo) else {
            return Err(format!("unknown repository: {repo}"));
        };
        let clone_dir = rdir.join(repo);
        if !git::is_cloned(&rdir, repo) {
            git::clone(&svc.repo, &clone_dir)?;
        }
        git::fetch(&clone_dir)?;
        git::worktree_add(&clone_dir, &ws_dir.join(repo), branch, base)?;
    }

    std::fs::create_dir_all(&ws_dir).map_err(|e| e.to_string())?;
    let ws = Workspace {
        name: name.to_string(),
        branch: branch.to_string(),
        base: base.to_string(),
        repos: repos.to_vec(),
    };
    let raw = serde_yaml::to_string(&ws).map_err(|e| e.to_string())?;
    std::fs::write(meta_path(&ws_dir), raw).map_err(|e| e.to_string())?;
    Ok(ws)
}

/// Removes a workspace: deletes each worktree and its branch, then the
/// workspace directory. Refuses when any worktree is dirty unless forced.
pub fn remove(name: &str, force: bool) -> Result<(), String> {
    let ws_dir = workspaces_dir()?.join(name);
    let ws = load_meta(&ws_dir)?;

    for repo in &ws.repos {
        let wt = ws_dir.join(repo);
        if wt.exists() && !force && git::is_dirty(&wt) {
            return Err(format!(
                "'{repo}' has uncommitted changes. Commit them or use force remove."
            ));
        }
    }
    for repo in &ws.repos {
        let clone_dir = repos_dir()?.join(repo);
        if clone_dir.exists() {
            let _ = git::worktree_remove(&clone_dir, &ws_dir.join(repo));
            let _ = git::branch_delete(&clone_dir, &ws.branch);
        }
    }
    std::fs::remove_dir_all(&ws_dir).map_err(|e| e.to_string())?;
    Ok(())
}

/// Collects per-repo status for a workspace.
pub fn status(name: &str) -> Result<Vec<RepoStatus>, String> {
    let ws_dir = workspaces_dir()?.join(name);
    let ws = load_meta(&ws_dir)?;

    let statuses: Vec<RepoStatus> = ws
        .repos
        .iter()
        .map(|repo| {
            let wt = ws_dir.join(repo);
            if !wt.exists() {
                return RepoStatus {
                    repo: repo.clone(),
                    branch: None,
                    dirty: false,
                    ahead: 0,
                    behind: 0,
                };
            }
            let (ahead, behind) = git::ahead_behind(&wt);
            RepoStatus {
                repo: repo.clone(),
                branch: git::current_branch(&wt),
                dirty: git::is_dirty(&wt),
                ahead,
                behind,
            }
        })
        .collect();
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rejects_unknown_repo_and_empty_selection() {
        // No config on a fresh environment: any repo selection is unknown.
        let err = create("ws", "feat/ws", "main", &["ghost-repo".into()])
            .expect_err("should reject unknown repo");
        assert!(err.contains("unknown repository"));

        let err = create("ws", "feat/ws", "main", &[]).expect_err("should reject empty selection");
        assert!(err.contains("at least one"));
    }
}