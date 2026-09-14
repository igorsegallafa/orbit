use crate::config::{Config, Service};
use crate::git;
use crate::github::{self, GithubRepo};
use std::path::PathBuf;

fn repos_dir() -> Result<PathBuf, String> {
    // ponytail: fixed single workspace root for the MVP; will come from
    // config once multiple workspace roots are supported.
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    Ok(home.join("Documents/orbit-workspace/repos"))
}

#[tauri::command]
pub fn get_config() -> Result<Config, String> {
    Config::load()
}

#[tauri::command]
pub fn add_service(name: String, repo: String) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    if cfg.services.iter().any(|s| s.name == name) {
        return Err(format!("a repo named '{name}' already exists"));
    }
    cfg.services.push(Service { name, repo });
    cfg.validate()?;
    cfg.save()?;
    Ok(cfg)
}

#[tauri::command]
pub fn update_service(name: String, repo: String) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    let svc = cfg
        .services
        .iter_mut()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("repo '{name}' not found"))?;
    svc.repo = repo;
    cfg.save()?;
    Ok(cfg)
}

#[tauri::command]
pub fn remove_service(name: String) -> Result<Config, String> {
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
}

#[tauri::command]
pub fn clone_service(name: String) -> Result<(), String> {
    let cfg = Config::load()?;
    let svc = cfg
        .services
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("repo '{name}' not found"))?;
    let dest = repos_dir()?.join(&svc.name);
    git::clone(&svc.repo, &dest)
}

#[tauri::command]
pub fn service_clone_status(name: String) -> Result<bool, String> {
    let dir = repos_dir()?;
    Ok(git::is_cloned(&dir, &name))
}

#[tauri::command]
pub fn create_group(name: String) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    if cfg.groups.contains_key(&name) {
        return Err(format!("group '{name}' already exists"));
    }
    cfg.groups.insert(name, vec![]);
    cfg.save()?;
    Ok(cfg)
}

#[tauri::command]
pub fn delete_group(name: String) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    cfg.groups.remove(&name);
    cfg.save()?;
    Ok(cfg)
}

#[tauri::command]
pub fn set_group_members(name: String, members: Vec<String>) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    if !cfg.groups.contains_key(&name) {
        return Err(format!("group '{name}' not found"));
    }
    cfg.groups.insert(name, members);
    cfg.validate()?;
    cfg.save()?;
    Ok(cfg)
}

#[tauri::command]
pub fn github_is_authenticated() -> bool {
    github::is_authenticated()
}

#[tauri::command]
pub fn github_list_repos() -> Result<Vec<GithubRepo>, String> {
    github::list_my_repos()
}
