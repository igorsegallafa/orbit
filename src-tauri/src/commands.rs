use crate::config::{Config, Service};
use crate::git;
use crate::github::{self, GithubRepo};
use std::path::PathBuf;
use tauri::async_runtime::spawn_blocking;

fn repos_dir() -> Result<PathBuf, String> {
    // ponytail: fixed single workspace root for the MVP; will come from
    // config once multiple workspace roots are supported.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    Ok(home.join("Documents/orbit-workspace/repos"))
}

/// Runs a blocking closure (fs/git/gh I/O) off the main thread.
///
/// Tauri dispatches non-async commands on the same thread that drives the
/// webview event loop, so any blocking call (subprocess, disk I/O) there
/// freezes the whole UI — including CSS animations — until it returns.
/// Wrapping the work in `spawn_blocking` keeps that thread free.
async fn blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub async fn get_config() -> Result<Config, String> {
    blocking(Config::load).await
}

#[tauri::command]
pub async fn add_service(name: String, repo: String) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        if cfg.services.iter().any(|s| s.name == name) {
            return Err(format!("a repo named '{name}' already exists"));
        }
        cfg.services.push(Service { name, repo });
        cfg.validate()?;
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn update_service(name: String, repo: String) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        let svc = cfg
            .services
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("repo '{name}' not found"))?;
        svc.repo = repo;
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn remove_service(name: String) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        let in_groups: Vec<&String> = cfg
            .groups
            .iter()
            .filter(|(_, members)| members.contains(&name))
            .map(|(g, _)| g)
            .collect();
        if !in_groups.is_empty() {
            return Err(format!(
                "'{name}' is part of group(s): {}. Remove it from those groups first.",
                in_groups
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        cfg.services.retain(|s| s.name != name);
        cfg.save()?;
        let dir = repos_dir()?.join(&name);
        let _ = git::remove_clone(&dir);
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn clone_service(name: String) -> Result<(), String> {
    blocking(move || {
        let cfg = Config::load()?;
        let svc = cfg
            .services
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("repo '{name}' not found"))?;
        let dest = repos_dir()?.join(&svc.name);
        git::clone(&svc.repo, &dest)
    })
    .await
}

#[tauri::command]
pub async fn service_clone_status(name: String) -> Result<bool, String> {
    blocking(move || {
        let dir = repos_dir()?;
        Ok(git::is_cloned(&dir, &name))
    })
    .await
}

#[tauri::command]
pub async fn create_group(name: String) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        if cfg.groups.contains_key(&name) {
            return Err(format!("group '{name}' already exists"));
        }
        cfg.groups.insert(name, vec![]);
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn delete_group(name: String) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        cfg.groups.remove(&name);
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn set_group_members(name: String, members: Vec<String>) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        if !cfg.groups.contains_key(&name) {
            return Err(format!("group '{name}' not found"));
        }
        cfg.groups.insert(name, members);
        cfg.validate()?;
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn github_is_authenticated() -> bool {
    blocking(|| Ok(github::is_authenticated()))
        .await
        .unwrap_or(false)
}

#[tauri::command]
pub async fn github_list_repos(force_refresh: bool) -> Result<Vec<GithubRepo>, String> {
    blocking(move || github::list_accessible_repos(force_refresh)).await
}
