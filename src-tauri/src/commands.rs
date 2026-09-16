use crate::agent;
use crate::config::{AiSettings, Config, Service};
use crate::git;
use crate::github::{self, GithubRepo};
use crate::integrations::{self, CardDetail, TrackerKind};
use crate::usage::{self, AiUsage};
use crate::workspace::{self, CardRef, RepoStatus, Workspace};
use std::path::PathBuf;
use tauri::async_runtime::spawn_blocking;

fn repos_dir() -> Result<PathBuf, String> {
    workspace::repos_dir()
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

// ---------- Code Review (PRs) ----------

#[tauri::command]
pub async fn pr_list(force_refresh: bool) -> Result<Vec<github::PrGroup>, String> {
    blocking(move || github::list_prs(force_refresh)).await
}

#[tauri::command]
pub async fn pr_search(query: String) -> Result<Vec<github::PrGroup>, String> {
    blocking(move || github::search_prs(&query)).await
}

#[tauri::command]
pub async fn pr_detail(owner_repo: String, number: u64) -> Result<github::PrDetail, String> {
    blocking(move || github::pr_detail(&owner_repo, number)).await
}

#[tauri::command]
pub async fn pr_file_diff(
    owner_repo: String,
    head_sha: String,
    base_sha: String,
    path: String,
) -> Result<github::PrFileDiff, String> {
    blocking(move || github::pr_file_diff(&owner_repo, &head_sha, &base_sha, &path)).await
}

#[tauri::command]
pub async fn list_workspaces() -> Result<Vec<Workspace>, String> {
    blocking(workspace::list).await
}

#[tauri::command]
pub async fn create_workspace(
    name: String,
    branch: String,
    base: String,
    repos: Vec<String>,
    card: Option<CardRef>,
) -> Result<Workspace, String> {
    blocking(move || workspace::create(&name, &branch, &base, &repos, card)).await
}

/// Full card content (description) for the Plan flow.
#[tauri::command]
pub async fn integration_fetch_card(kind: String, id: String) -> Result<CardDetail, String> {
    blocking(move || integrations::fetch_card(TrackerKind::parse(&kind)?, &id)).await
}

/// Whether a PLAN.md already exists for the workspace.
#[tauri::command]
pub async fn workspace_plan_exists(name: String) -> Result<bool, String> {
    blocking(move || {
        let dir = workspace::workspace_root()?.join("workspaces").join(&name);
        Ok(dir.join(agent::PLAN_FILE).exists())
    })
    .await
}

/// Runs the configured agent to generate PLAN.md from the workspace card.
/// Streams progress via the `plan-progress` event.
#[tauri::command]
pub async fn generate_plan(
    app: tauri::AppHandle,
    name: String,
    card: CardRef,
) -> Result<agent::PlanResult, String> {
    let ws_dir = workspace::workspace_root()?
        .join("workspaces")
        .join(&name);
    let meta = workspace::load_meta(&ws_dir)?;
    let ai = Config::load()?.ai;
    let app = app.clone();
    spawn_blocking(move || {
        agent::generate_plan(&app, &name, &card, &meta.repos, &meta.branch, &ai)
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?
}

/// Interactive grill-me interview step (UI stepper): one round at a time.
#[tauri::command]
pub async fn grill_step(
    name: String,
    card: CardRef,
    answers: Vec<(String, String)>,
    rounds_done: usize,
    max_rounds: Option<usize>,
) -> Result<agent::GrillRound, String> {
    blocking(move || agent::grill_round(&name, &card, &answers, rounds_done, max_rounds)).await
}

/// Headless plan generation seeded with the interview's decisions.
#[tauri::command]
pub async fn generate_plan_decisions(
    app: tauri::AppHandle,
    name: String,
    card: CardRef,
    decisions: String,
) -> Result<agent::PlanResult, String> {
    let app = app.clone();
    spawn_blocking(move || {
        agent::generate_plan_with_decisions(&app, &name, &card, &decisions)
    })
    .await
    .map_err(|e| format!("background task failed: {e}"))?
}

/// Cancels the running plan generation (kills the agent process).
#[tauri::command]
pub fn cancel_plan() -> Result<(), String> {
    agent::cancel_plan()
}

/// Interactive grill-me prompt for the workspace card (interview flow).
#[tauri::command]
pub async fn grill_prompt(name: String, card: CardRef) -> Result<String, String> {
    blocking(move || agent::grill_prompt(&name, &card)).await
}

/// `- [ ]` tasks from the workspace's PLAN.md with their done state.
#[tauri::command]
pub async fn plan_tasks(name: String) -> Result<Vec<agent::PlanTask>, String> {
    blocking(move || agent::plan_tasks(&name)).await
}

/// Marks the nth PLAN.md task as done/undone (writes back to the file).
#[tauri::command]
pub async fn set_plan_task(name: String, index: usize, done: bool) -> Result<(), String> {
    blocking(move || agent::set_plan_task(&name, index, done)).await
}

// ---------- Git review (dock Git tab) ----------

fn worktree_of(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let dir = workspace::workspace_root()?
        .join("workspaces")
        .join(workspace)
        .join(repo);
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found in '{workspace}'"));
    }
    Ok(dir)
}

/// Working-tree changes of a workspace repo.
#[tauri::command]
pub async fn git_changes(workspace: String, repo: String) -> Result<Vec<git::ChangeEntry>, String> {
    blocking(move || git::changes(&worktree_of(&workspace, &repo)?)).await
}

/// Recent commits of a workspace repo.
#[tauri::command]
pub async fn git_commits(workspace: String, repo: String) -> Result<Vec<git::CommitEntry>, String> {
    blocking(move || git::commits(&worktree_of(&workspace, &repo)?, 20)).await
}

/// Original (HEAD) and current content of a file, for the diff review.
#[tauri::command]
pub async fn git_file_diff(
    workspace: String,
    repo: String,
    path: String,
) -> Result<git::FileDiff, String> {
    blocking(move || {
        let dir = worktree_of(&workspace, &repo)?;
        let original = git::rev_content(&dir, &path, "HEAD").unwrap_or_default();
        let modified = if path.is_empty() {
            return Err("empty path".into());
        } else {
            std::fs::read_to_string(dir.join(&path)).unwrap_or_default()
        };
        Ok(git::FileDiff { original, modified })
    })
    .await
}

/// Files touched by a commit.
#[tauri::command]
pub async fn git_commit_files(
    workspace: String,
    repo: String,
    sha: String,
) -> Result<Vec<git::ChangeEntry>, String> {
    blocking(move || git::commit_files(&worktree_of(&workspace, &repo)?, &sha)).await
}

/// Content pair of a file at a commit (sha^ vs sha), for commit inspection.
#[tauri::command]
pub async fn git_commit_diff(
    workspace: String,
    repo: String,
    sha: String,
    path: String,
) -> Result<git::FileDiff, String> {
    blocking(move || {
        let dir = worktree_of(&workspace, &repo)?;
        let before = format!("{sha}^");
        let original = git::rev_content(&dir, &path, &before).unwrap_or_default();
        let modified = git::rev_content(&dir, &path, &sha).unwrap_or_default();
        Ok(git::FileDiff { original, modified })
    })
    .await
}

// ---------- AI settings ----------

#[tauri::command]
pub async fn get_ai_settings() -> Result<AiSettings, String> {
    blocking(|| Ok(Config::load()?.ai)).await
}

#[tauri::command]
pub async fn set_ai_settings(ai: AiSettings) -> Result<Config, String> {
    blocking(move || {
        let mut cfg = Config::load()?;
        cfg.ai = ai;
        cfg.save()?;
        Ok(cfg)
    })
    .await
}

#[tauri::command]
pub async fn test_agent() -> Result<(), String> {
    let ai = blocking(|| Ok(Config::load()?.ai)).await?;
    blocking(move || agent::test_agent(&ai)).await
}

#[tauri::command]
pub async fn list_models(agent_name: String) -> Result<Vec<String>, String> {
    blocking(move || Ok(agent::list_models(&agent_name))).await
}

#[tauri::command]
pub async fn remove_workspace(name: String, force: bool) -> Result<(), String> {
    blocking(move || workspace::remove(&name, force)).await
}

#[tauri::command]
pub async fn workspace_status(name: String) -> Result<Vec<RepoStatus>, String> {
    blocking(move || workspace::status(&name)).await
}

/// Fetches the upstream for a repo's clone (refreshes ahead/behind data).
#[tauri::command]
pub async fn refresh_repo(name: String) -> Result<(), String> {
    blocking(move || {
        let dir = repos_dir()?.join(&name);
        if !dir.exists() {
            return Err(format!("'{name}' is not cloned"));
        }
        git::fetch(&dir)
    })
    .await
}

/// Opens a workspace repo's worktree in the user's editor (VS Code or Zed).
#[tauri::command]
pub async fn open_in_editor(workspace: String, repo: String) -> Result<(), String> {
    blocking(move || {
        let wt = workspace::workspace_root()?
            .join("workspaces")
            .join(&workspace)
            .join(&repo);
        if !wt.exists() {
            return Err(format!("worktree for '{repo}' not found"));
        }
        for editor in ["code", "zed"] {
            if which(&editor) {
                let out = std::process::Command::new(editor)
                    .arg(&wt)
                    .spawn()
                    .map_err(|e| format!("failed to launch {editor}: {e}"))?;
                std::mem::forget(out);
                return Ok(());
            }
        }
        Err("no editor found (looked for 'code' and 'zed' in PATH)".into())
    })
    .await
}

/// Opens a workspace folder in Finder.
#[tauri::command]
pub async fn reveal_workspace_folder(name: String) -> Result<(), String> {
    blocking(move || {
        let dir = workspace_dir(&name)?;
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("failed to open Finder: {e}"))?;
        Ok(())
    })
    .await
}

/// Opens the whole workspace folder (all worktrees) in the user's editor.
#[tauri::command]
pub async fn open_workspace_in_editor(name: String) -> Result<(), String> {
    blocking(move || {
        let dir = workspace_dir(&name)?;
        for editor in ["code", "zed"] {
            if which(&editor) {
                std::process::Command::new(editor)
                    .arg(&dir)
                    .spawn()
                    .map_err(|e| format!("failed to launch {editor}: {e}"))?;
                return Ok(());
            }
        }
        Err("no editor found (looked for 'code' and 'zed' in PATH)".into())
    })
    .await
}

fn workspace_dir(name: &str) -> Result<PathBuf, String> {
    let dir = workspace::workspace_root()?.join("workspaces").join(name);
    if !dir.exists() {
        return Err(format!("workspace '{name}' folder not found"));
    }
    Ok(dir)
}

/// AI token usage for a workspace, from Claude Code's local session logs.
#[tauri::command]
pub async fn workspace_ai_usage(name: String) -> Result<AiUsage, String> {
    blocking(move || {
        let ws_dir = workspace::workspace_root()?.join("workspaces").join(&name);
        if !ws_dir.exists() {
            return Err(format!("workspace '{name}' not found"));
        }
        let repos = workspace::load_meta(&ws_dir).map(|m| m.repos).unwrap_or_default();
        Ok(usage::workspace_usage(&ws_dir, &repos))
    })
    .await
}

// ---------- Integrations (Shortcut / Linear) ----------

#[tauri::command]
pub async fn integration_status(kind: String) -> Result<integrations::IntegrationStatus, String> {
    blocking(move || Ok(integrations::status(TrackerKind::parse(&kind)?))).await
}

/// Validates the token against the provider, then saves it on success.
#[tauri::command]
pub async fn integration_connect(kind: String, token: String) -> Result<String, String> {
    blocking(move || {
        let kind = TrackerKind::parse(&kind)?;
        let account = integrations::validate(kind, &token)?;
        integrations::save_token(kind, &token)?;
        Ok(account)
    })
    .await
}

#[tauri::command]
pub async fn integration_disconnect(kind: String) -> Result<(), String> {
    blocking(move || integrations::remove_token(TrackerKind::parse(&kind)?)).await
}

/// Candidate base branches from the first selected repo (for the wizard's
/// base-branch select). Always includes main/master fallbacks.
#[tauri::command]
pub async fn list_base_branches(repos: Vec<String>) -> Result<Vec<String>, String> {
    blocking(move || {
        let mut out = vec!["main".to_string(), "master".to_string()];
        let rdir = workspace::repos_dir()?;
        for repo in &repos {
            let dir = rdir.join(repo);
            if git::is_cloned(&rdir, repo) {
                for b in git::list_branches(&dir) {
                    if !out.contains(&b) {
                        out.push(b);
                    }
                }
                break; // first cloned repo is enough for suggestions
            }
        }
        Ok(out)
    })
    .await
}

/// Card search from a connected tracker (query filters server-side).
#[tauri::command]
pub async fn integration_fetch_cards(
    kind: String,
    query: Option<String>,
) -> Result<Vec<integrations::CardInfo>, String> {
    blocking(move || {
        integrations::fetch_cards(TrackerKind::parse(&kind)?, query.as_deref().unwrap_or(""))
    })
    .await
}

fn which(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
