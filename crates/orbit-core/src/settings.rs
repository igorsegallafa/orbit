//! Settings edits: repos (services), clone folders, groups and the AI
//! settings in `config.yaml`, plus opening a repo in an external editor.
//! Every edit loads the file, changes it and saves it, so both apps see the
//! same settings.
use crate::config::{AiSettings, Config, Service};
use crate::workspace;
use serde::Serialize;
use std::path::{Path, PathBuf};

fn edit(f: impl FnOnce(&mut Config) -> Result<(), String>) -> Result<Config, String> {
    let mut cfg = Config::load()?;
    f(&mut cfg)?;
    cfg.save()?;
    Ok(cfg)
}

fn find<'a>(cfg: &'a mut Config, name: &str) -> Result<&'a mut Service, String> {
    cfg.services.iter_mut().find(|s| s.name == name).ok_or_else(|| format!("repo '{name}' not found"))
}

/// Adds a repo. `path` (optional) is where its clone lives: an existing
/// checkout used as is, or an empty/missing folder to clone into later.
pub fn add_service(name: &str, repo: &str, path: Option<String>) -> Result<Config, String> {
    edit(|cfg| {
        if cfg.services.iter().any(|s| s.name == name) {
            return Err(format!("a repo named '{name}' already exists"));
        }
        let mut svc = Service::new(name.to_string(), repo.to_string());
        svc.path = path.filter(|p| !p.trim().is_empty());
        if let Some(p) = svc.custom_path() {
            check_clone_target(&p)?;
        }
        cfg.services.push(svc);
        cfg.validate()
    })
}

/// The `origin` URL of a local checkout, to add it as a repo.
pub fn local_repo_origin(path: &str) -> Result<String, String> {
    let dir = PathBuf::from(path);
    if !workspace::is_cloned(&dir) {
        return Err(format!("{path} is not a git repository"));
    }
    crate::proc::run("git", &["remote", "get-url", "origin"], Some(&dir))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{path} has no 'origin' remote"))
}

pub fn update_service(name: &str, repo: &str) -> Result<Config, String> {
    edit(|cfg| {
        find(cfg, name)?.repo = repo.to_string();
        Ok(())
    })
}

/// Replaces a repo's build/link settings (everything but name and URL).
pub fn update_service_settings(service: Service) -> Result<Config, String> {
    edit(|cfg| {
        let svc = find(cfg, &service.name)?;
        let repo = svc.repo.clone();
        *svc = Service { repo, ..service };
        Ok(())
    })
}

/// A custom clone location must be a git checkout or an empty folder.
pub fn check_clone_target(p: &Path) -> Result<(), String> {
    if !p.exists() || workspace::is_cloned(p) {
        return Ok(());
    }
    let empty = std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false);
    if empty {
        Ok(())
    } else {
        Err(format!("{} is not empty and is not a git repository", p.display()))
    }
}

/// Removes a repo that no group uses. Its clone is deleted only when Orbit
/// made it in the clones folder; a custom path may be the user's own checkout.
pub fn remove_service(name: &str) -> Result<Config, String> {
    let mut owned_clone = false;
    let cfg = edit(|cfg| {
        let mut in_groups: Vec<&str> = cfg.groups.iter().filter(|(_, m)| m.iter().any(|x| x == name)).map(|(g, _)| g.as_str()).collect();
        if !in_groups.is_empty() {
            in_groups.sort();
            return Err(format!("'{name}' is part of group(s): {}. Remove it from those groups first.", in_groups.join(", ")));
        }
        owned_clone = cfg.services.iter().find(|s| s.name == name).is_some_and(|s| s.custom_path().is_none());
        cfg.services.retain(|s| s.name != name);
        Ok(())
    })?;
    if owned_clone {
        let _ = crate::git::remove_clone(&workspace::repos_dir()?.join(name));
    }
    Ok(cfg)
}

/// Clones a repo into its folder (nothing to do when it is cloned already).
pub fn clone_service(name: &str) -> Result<(), String> {
    let cfg = Config::load()?;
    let svc = cfg.services.iter().find(|s| s.name == name).ok_or_else(|| format!("repo '{name}' not found"))?;
    let dest = workspace::clone_dir(svc)?;
    if workspace::is_cloned(&dest) {
        return Ok(());
    }
    crate::git::clone(&svc.repo, &dest)
}

pub fn clone_status(name: &str) -> Result<bool, String> {
    Ok(workspace::is_cloned(&workspace::clone_dir_of(name)?))
}

/// A repo's base clone, for revealing it; an error when it isn't there.
pub fn clone_folder(name: &str) -> Result<PathBuf, String> {
    let dir = workspace::clone_dir_of(name)?;
    if !dir.exists() {
        return Err(format!("'{name}' is not cloned"));
    }
    Ok(dir)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folders {
    pub repos_dir: String,
    pub workspaces_dir: String,
    /// Used when the matching setting is empty.
    pub default_repos_dir: String,
    pub default_workspaces_dir: String,
}

pub fn folders() -> Result<Folders, String> {
    let root = workspace::workspace_root()?;
    Ok(Folders {
        repos_dir: workspace::repos_dir()?.to_string_lossy().into(),
        workspaces_dir: workspace::workspaces_dir()?.to_string_lossy().into(),
        default_repos_dir: root.join("repos").to_string_lossy().into(),
        default_workspaces_dir: root.join("workspaces").to_string_lossy().into(),
    })
}

/// Sets the clones/workspaces folders (None or empty: the default).
pub fn set_folders(repos_dir: Option<String>, workspaces_dir: Option<String>) -> Result<Config, String> {
    edit(|cfg| {
        cfg.repos_dir = repos_dir.filter(|p| !p.trim().is_empty());
        cfg.workspaces_dir = workspaces_dir.filter(|p| !p.trim().is_empty());
        Ok(())
    })
}

pub fn create_group(name: &str) -> Result<Config, String> {
    edit(|cfg| {
        if cfg.groups.contains_key(name) {
            return Err(format!("group '{name}' already exists"));
        }
        cfg.groups.insert(name.to_string(), vec![]);
        Ok(())
    })
}

pub fn delete_group(name: &str) -> Result<Config, String> {
    edit(|cfg| {
        cfg.groups.remove(name);
        Ok(())
    })
}

pub fn set_group_members(name: &str, members: Vec<String>) -> Result<Config, String> {
    edit(|cfg| {
        if !cfg.groups.contains_key(name) {
            return Err(format!("group '{name}' not found"));
        }
        cfg.groups.insert(name.to_string(), members);
        cfg.validate()
    })
}

pub fn set_ai(ai: AiSettings) -> Result<Config, String> {
    edit(|cfg| {
        cfg.ai = ai;
        Ok(())
    })
}

/// The default model of each agent, for when the agent changes (the model
/// format follows the agent).
pub fn default_model(agent: &str) -> &'static str {
    match agent {
        "opencode" => "aihub/aihub/best",
        "omp" => "aihub/glm-5.3",
        "codex" => "gpt-6-luna",
        _ => "claude-sonnet-5-5",
    }
}

/// Opens `dir` in the user's editor (VS Code or Zed), detached.
pub fn launch_editor(dir: &Path) -> Result<(), String> {
    for editor in ["code", "zed"] {
        if crate::proc::exists(editor) {
            crate::proc::cmd(editor).arg(dir).spawn().map_err(|e| format!("failed to launch {editor}: {e}"))?;
            return Ok(());
        }
    }
    Err("no editor found (looked for 'code' and 'zed' in PATH)".into())
}

/// Opens a workspace repo's worktree in the user's editor.
pub fn open_in_editor(workspace: &str, repo: &str) -> Result<(), String> {
    let wt = workspace::repo_path(workspace, repo)?;
    if !wt.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    launch_editor(&wt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ENV_LOCK;

    #[test]
    fn edits_repos_groups_folders_and_ai_in_config_yaml() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));

        add_service("web", "https://github.com/o/web.git", None).unwrap();
        assert!(add_service("web", "x", None).unwrap_err().contains("already exists"));
        // A custom path must be empty or a checkout.
        let busy = root.join("busy");
        std::fs::create_dir_all(&busy).unwrap();
        std::fs::write(busy.join("f"), "x").unwrap();
        assert!(add_service("api", "u", Some(busy.to_string_lossy().into())).unwrap_err().contains("not empty"));
        add_service("api", "https://github.com/o/api.git", Some(root.join("api").to_string_lossy().into())).unwrap();
        update_service("api", "git@github.com:o/api.git").unwrap();

        let mut svc = Config::load().unwrap().services[1].clone();
        svc.worktree = false;
        svc.repo = "ignored".into();
        let cfg = update_service_settings(svc).unwrap();
        assert_eq!(cfg.services[1].repo, "git@github.com:o/api.git");
        assert!(!cfg.services[1].worktree);

        create_group("core").unwrap();
        assert!(create_group("core").is_err());
        assert!(set_group_members("core", vec!["nope".into()]).is_err());
        set_group_members("core", vec!["web".into()]).unwrap();
        assert!(remove_service("web").unwrap_err().contains("group(s): core"));
        delete_group("core").unwrap();
        let cfg = remove_service("web").unwrap();
        assert_eq!(cfg.services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["api"]);

        set_folders(Some(root.join("clones").to_string_lossy().into()), Some(" ".into())).unwrap();
        let f = folders().unwrap();
        assert!(f.repos_dir.ends_with("clones"));
        assert_eq!(f.workspaces_dir, f.default_workspaces_dir);

        let mut ai = Config::load().unwrap().ai;
        ai.agent = "opencode".into();
        ai.model = default_model("opencode").into();
        set_ai(ai).unwrap();
        assert_eq!(Config::load().unwrap().ai.model, "aihub/aihub/best");
        assert!(clone_folder("api").is_err());
        assert!(!clone_status("api").unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}
