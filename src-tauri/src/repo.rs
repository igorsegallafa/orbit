// Repository view: one configured repo's base clone managed directly
// (branches, sync, commits, stashes, worktrees), GitHub Desktop style,
// outside any workspace. Everything else the view shows (changes, diffs,
// files, terminals, PRs) goes through the regular commands with the repo
// scope ("@repo", see workspace::repo_scope).
use crate::{git, github, workspace};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepoOverview {
    pub name: String,
    pub path: String,
    pub cloned: bool,
    /// None = detached HEAD.
    pub branch: Option<String>,
    pub head: String,
    pub default_branch: Option<String>,
    /// Tracking branch, e.g. "origin/feat/x"; None = never pushed.
    pub upstream: Option<String>,
    /// The upstream was deleted on the remote (e.g. the PR was merged).
    pub upstream_gone: bool,
    pub ahead: usize,
    pub behind: usize,
    /// Changed files (tracked and untracked).
    pub changes: usize,
    /// "owner/repo" on GitHub.
    pub owner_repo: Option<String>,
    /// A paused "rebase" | "merge" | "cherry-pick" | "revert".
    pub operation: Option<String>,
    pub conflicts: Vec<String>,
    pub stashes: Vec<StashEntry>,
    /// Unix time of the last fetch.
    pub last_fetch: Option<i64>,
    pub branches: Vec<BranchInfo>,
    pub worktrees: Vec<WorktreeInfo>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    /// Only on origin: switching creates the local tracking branch.
    pub remote_only: bool,
    pub upstream: Option<String>,
    pub gone: bool,
    pub ahead: usize,
    pub behind: usize,
    pub subject: String,
    pub updated: String,
    pub updated_unix: i64,
    /// Checked out in another worktree (where git won't switch to it).
    pub worktree: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    /// The base clone itself.
    pub main: bool,
    /// Orbit workspace owning it (its folder is inside the workspaces dir).
    pub workspace: Option<String>,
    /// Folder deleted by hand; `git worktree prune` cleans it up.
    pub missing: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub when: String,
}

fn git_out(dir: &Path, args: &[&str]) -> Result<String, String> {
    crate::proc::run("git", args, Some(dir))
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<(), String> {
    git_out(dir, args).map(|_| ())
}

fn clone_of(name: &str) -> Result<PathBuf, String> {
    let dir = workspace::clone_dir_of(name)?;
    if !workspace::is_cloned(&dir) {
        return Err(format!("'{name}' is not cloned yet"));
    }
    Ok(dir)
}

pub fn overview(name: &str) -> Result<RepoOverview, String> {
    let dir = workspace::clone_dir_of(name)?;
    let path = dir.to_string_lossy().to_string();
    if !workspace::is_cloned(&dir) {
        return Ok(RepoOverview {
            name: name.to_string(),
            path,
            cloned: false,
            branch: None,
            head: String::new(),
            default_branch: None,
            upstream: None,
            upstream_gone: false,
            ahead: 0,
            behind: 0,
            changes: 0,
            owner_repo: None,
            operation: None,
            conflicts: vec![],
            stashes: vec![],
            last_fetch: None,
            branches: vec![],
            worktrees: vec![],
        });
    }
    let branch = git::current_branch(&dir);
    let branches = branches(&dir, branch.as_deref());
    let current = branches.iter().find(|b| b.current);
    let git_dir = git_out(&dir, &["rev-parse", "--absolute-git-dir"]).map(|s| PathBuf::from(s.trim())).ok();
    Ok(RepoOverview {
        name: name.to_string(),
        path,
        cloned: true,
        head: git_out(&dir, &["rev-parse", "--short", "HEAD"]).unwrap_or_default().trim().to_string(),
        default_branch: git::default_branch(&dir),
        upstream: current.and_then(|b| b.upstream.clone()),
        upstream_gone: current.is_some_and(|b| b.gone),
        ahead: current.map_or(0, |b| b.ahead),
        behind: current.map_or(0, |b| b.behind),
        changes: git_out(&dir, &["status", "--porcelain", "-uall"]).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).count(),
        owner_repo: git_out(&dir, &["remote", "get-url", "origin"]).ok().and_then(|u| github::parse_owner_repo(u.trim()).ok()),
        operation: git_dir.as_deref().and_then(operation_in_progress),
        conflicts: git::conflicted_files(&dir),
        stashes: stashes(&dir),
        last_fetch: git_dir.and_then(|g| std::fs::metadata(g.join("FETCH_HEAD")).ok()?.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
        worktrees: worktrees(&dir),
        branch,
        branches,
    })
}

fn operation_in_progress(git_dir: &Path) -> Option<String> {
    let op = if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        "rebase"
    } else if git_dir.join("MERGE_HEAD").exists() {
        "merge"
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        "cherry-pick"
    } else if git_dir.join("REVERT_HEAD").exists() {
        "revert"
    } else {
        return None;
    };
    Some(op.to_string())
}

/// Local branches plus origin branches with no local counterpart, most
/// recently updated first.
fn branches(dir: &Path, current: Option<&str>) -> Vec<BranchInfo> {
    const FMT: &str = "--format=%(refname)%1f%(refname:short)%1f%(upstream:short)%1f%(upstream:track)%1f%(contents:subject)%1f%(committerdate:relative)%1f%(committerdate:unix)%1f%(worktreepath)";
    let raw = git_out(dir, &["for-each-ref", FMT, "refs/heads", "refs/remotes/origin"]).unwrap_or_default();
    let here = normalize_path(dir);
    let mut out = parse_branches(&raw, current, &here);
    out.sort_by_key(|b| (!b.current, std::cmp::Reverse(b.updated_unix)));
    out
}

fn parse_branches(raw: &str, current: Option<&str>, clone: &str) -> Vec<BranchInfo> {
    let mut local: Vec<BranchInfo> = Vec::new();
    let mut remote: Vec<BranchInfo> = Vec::new();
    for line in raw.lines() {
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 8 {
            continue;
        }
        let (refname, short, upstream, track) = (f[0], f[1], f[2], f[3]);
        let (ahead, behind) = parse_track(track);
        let wt = Some(f[7].trim()).filter(|p| !p.is_empty()).map(normalize_path).filter(|p| p != clone);
        let mut b = BranchInfo {
            name: short.to_string(),
            current: false,
            remote_only: false,
            upstream: Some(upstream.to_string()).filter(|u| !u.is_empty()),
            gone: track.contains("gone"),
            ahead,
            behind,
            subject: f[4].to_string(),
            updated: f[5].to_string(),
            updated_unix: f[6].trim().parse().unwrap_or(0),
            worktree: wt,
        };
        if refname.starts_with("refs/heads/") {
            b.current = current == Some(short);
            local.push(b);
        } else if let Some(name) = short.strip_prefix("origin/") {
            if name == "HEAD" || short == "origin" {
                continue;
            }
            b.name = name.to_string();
            b.remote_only = true;
            b.upstream = Some(short.to_string());
            remote.push(b);
        }
    }
    remote.retain(|r| !local.iter().any(|l| l.name == r.name));
    local.extend(remote);
    local
}

/// "[ahead 2, behind 1]" → (2, 1).
fn parse_track(track: &str) -> (usize, usize) {
    let num = |key: &str| {
        track
            .split(key)
            .nth(1)
            .and_then(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (num("ahead"), num("behind"))
}

/// Comparable form of a path as git prints it (forward slashes, no trailing
/// slash, case-folded on Windows).
fn normalize_path(p: impl AsRef<Path>) -> String {
    let s = p.as_ref().to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn worktrees(dir: &Path) -> Vec<WorktreeInfo> {
    let raw = git_out(dir, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let ws_root = workspace::workspaces_dir().map(normalize_path).unwrap_or_default();
    parse_worktrees(&raw, &normalize_path(dir), &ws_root)
}

fn parse_worktrees(raw: &str, clone: &str, ws_root: &str) -> Vec<WorktreeInfo> {
    raw.split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;
            let mut prunable = false;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(p.trim().to_string());
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.trim().trim_start_matches("refs/heads/").to_string());
                } else if line.starts_with("prunable") {
                    prunable = true;
                }
            }
            let path = path?;
            let norm = normalize_path(&path);
            let workspace = (!ws_root.is_empty())
                .then(|| norm.strip_prefix(&format!("{ws_root}/")))
                .flatten()
                .and_then(|rest| rest.split('/').next())
                .map(|w| w.to_string());
            Some(WorktreeInfo {
                main: norm == clone,
                missing: prunable || !Path::new(&path).exists(),
                workspace,
                branch,
                path,
            })
        })
        .collect()
}

fn stashes(dir: &Path) -> Vec<StashEntry> {
    git_out(dir, &["stash", "list", "--format=%gs%x1f%cr"])
        .unwrap_or_default()
        .lines()
        .enumerate()
        .filter_map(|(index, l)| {
            let (message, when) = l.split_once('\x1f')?;
            Some(StashEntry { index, message: message.to_string(), when: when.to_string() })
        })
        .collect()
}

/// Switches the clone to `branch`: a local branch, or an origin-only one
/// (creating the tracking branch). `stash` first shelves uncommitted work
/// that would otherwise block or travel with the switch.
pub fn switch(name: &str, branch: &str, stash: bool) -> Result<(), String> {
    let dir = clone_of(name)?;
    if stash {
        let msg = format!("orbit: before switching to {branch}");
        git_ok(&dir, &["stash", "push", "--include-untracked", "-m", &msg])?;
    }
    if git::branch_exists(&dir, branch) {
        git_ok(&dir, &["switch", branch])
    } else {
        git_ok(&dir, &["switch", "--create", branch, "--track", &format!("origin/{branch}")])
    }
}

/// Creates `branch` from `from` (default: the current HEAD) and switches to it.
pub fn create_branch(name: &str, branch: &str, from: Option<&str>) -> Result<(), String> {
    let dir = clone_of(name)?;
    let branch = branch.trim();
    git_ok(&dir, &["check-ref-format", "--branch", branch]).map_err(|_| format!("'{branch}' is not a valid branch name"))?;
    match from.map(str::trim).filter(|f| !f.is_empty()) {
        Some(start) => git_ok(&dir, &["switch", "--create", branch, "--no-track", start]),
        None => git_ok(&dir, &["switch", "--create", branch]),
    }
}

/// Deletes a local branch. Without `force`, git refuses when its commits
/// aren't merged anywhere, and so does this.
pub fn delete_branch(name: &str, branch: &str, force: bool) -> Result<(), String> {
    let dir = clone_of(name)?;
    git_ok(&dir, &["branch", if force { "-D" } else { "-d" }, branch])
}

/// Pulls the upstream: fast-forward only unless `rebase` (local commits
/// replayed on top; conflicts leave the rebase paused for the view).
pub fn pull(name: &str, rebase: bool) -> Result<(), String> {
    let dir = clone_of(name)?;
    let args: &[&str] = if rebase { &["pull", "--rebase"] } else { &["pull", "--ff-only"] };
    match git_out(&dir, args) {
        Ok(_) => Ok(()),
        Err(_) if rebase && !git::conflicted_files(&dir).is_empty() => Ok(()),
        Err(e) if e.contains("Not possible to fast-forward") || e.contains("diverg") => {
            Err("Your branch and its upstream have diverged: pull with rebase to replay your commits on top.".into())
        }
        Err(e) => Err(e),
    }
}

/// Commits just `paths` (all their changes, new and deleted files included);
/// anything else stays uncommitted.
pub fn commit(name: &str, message: &str, paths: &[String]) -> Result<(), String> {
    let dir = clone_of(name)?;
    if message.trim().is_empty() {
        return Err("commit message is required".into());
    }
    if paths.is_empty() {
        return Err("select at least one file to commit".into());
    }
    let mut add = vec!["add", "--all", "--"];
    add.extend(paths.iter().map(String::as_str));
    git_ok(&dir, &add)?;
    let mut commit = vec!["commit", "-m", message, "--"];
    commit.extend(paths.iter().map(String::as_str));
    git_ok(&dir, &commit)
}

/// Throws away the changes of `paths`: tracked files go back to HEAD; files
/// HEAD doesn't have go to the Trash (recoverable) rather than being deleted.
pub fn discard(name: &str, paths: &[String]) -> Result<(), String> {
    let dir = clone_of(name)?;
    for p in paths {
        if git_ok(&dir, &["cat-file", "-e", &format!("HEAD:{p}")]).is_ok() {
            git_ok(&dir, &["restore", "--source=HEAD", "--staged", "--worktree", "--", p])?;
        } else {
            let _ = git_ok(&dir, &["rm", "--cached", "--quiet", "--ignore-unmatch", "--", p]);
            let file = dir.join(p);
            if file.exists() {
                trash::delete(&file).map_err(|e| format!("{p}: could not move to Trash: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn stash(name: &str) -> Result<(), String> {
    git_ok(&clone_of(name)?, &["stash", "push", "--include-untracked"])
}

/// Re-applies stash `index` and drops it (kept when it conflicts).
pub fn stash_pop(name: &str, index: usize) -> Result<(), String> {
    git_ok(&clone_of(name)?, &["stash", "pop", &format!("stash@{{{index}}}")])
}

pub fn stash_drop(name: &str, index: usize) -> Result<(), String> {
    git_ok(&clone_of(name)?, &["stash", "drop", &format!("stash@{{{index}}}")])
}

/// Undoes the last commit, keeping its changes in the working tree. Refused
/// once the commit is on the remote: rewriting pushed history needs a force push.
pub fn undo_commit(name: &str) -> Result<(), String> {
    let dir = clone_of(name)?;
    let on_remote = git_out(&dir, &["branch", "--remotes", "--contains", "HEAD"]).unwrap_or_default();
    if !on_remote.trim().is_empty() {
        return Err("the last commit is already pushed; undoing it would rewrite shared history".into());
    }
    git_ok(&dir, &["rev-parse", "--verify", "--quiet", "HEAD~1"]).map_err(|_| "this is the first commit of the repository".to_string())?;
    git_ok(&dir, &["reset", "--soft", "HEAD~1"])
}

/// Commits of the current branch, newest first.
pub fn history(name: &str, limit: usize) -> Result<Vec<git::CommitEntry>, String> {
    git::commits(&clone_of(name)?, limit)
}

/// PRs whose head is the clone's current branch (any state).
pub fn branch_prs(name: &str) -> Result<Vec<github::WsPrStatus>, String> {
    let dir = clone_of(name)?;
    let Some(branch) = git::current_branch(&dir) else { return Ok(vec![]) };
    let url = git_out(&dir, &["remote", "get-url", "origin"])?;
    let owner_repo = github::parse_owner_repo(url.trim())?;
    github::pr_status_for_branch(&owner_repo, &branch)
}

pub fn prune_worktrees(name: &str) -> Result<(), String> {
    git_ok(&clone_of(name)?, &["worktree", "prune"])
}

// ---------- Tauri commands ----------

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("background task failed: {e}"))?
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_tracking() {
        assert_eq!(parse_track("[ahead 2, behind 1]"), (2, 1));
        assert_eq!(parse_track("[behind 3]"), (0, 3));
        assert_eq!(parse_track("[gone]"), (0, 0));
        assert_eq!(parse_track(""), (0, 0));
    }

    #[test]
    fn merges_local_and_origin_only_branches() {
        let raw = [
            "refs/heads/main\x1fmain\x1forigin/main\x1f[behind 1]\x1finit\x1f2 days ago\x1f100\x1f/c/repo",
            "refs/heads/feat/x\x1ffeat/x\x1forigin/feat/x\x1f[gone]\x1fx\x1f1 day ago\x1f200\x1f/c/ws/one/repo",
            "refs/remotes/origin/HEAD\x1forigin/HEAD\x1f\x1f\x1f\x1f\x1f0\x1f",
            "refs/remotes/origin/main\x1forigin/main\x1f\x1f\x1finit\x1f2 days ago\x1f100\x1f",
            "refs/remotes/origin/feat/y\x1forigin/feat/y\x1f\x1f\x1fy\x1f1 hour ago\x1f300\x1f",
        ]
        .join("\n");
        let b = parse_branches(&raw, Some("main"), "/c/repo");
        let names: Vec<_> = b.iter().map(|b| (b.name.as_str(), b.remote_only)).collect();
        assert_eq!(names, [("main", false), ("feat/x", false), ("feat/y", true)]);
        assert!(b[0].current && b[0].behind == 1 && b[0].worktree.is_none(), "the clone's own checkout isn't 'elsewhere'");
        assert!(b[1].gone && b[1].worktree.as_deref() == Some("/c/ws/one/repo"));
        assert_eq!(b[2].upstream.as_deref(), Some("origin/feat/y"));
    }

    #[test]
    fn maps_worktrees_to_workspaces() {
        let raw = "worktree C:/orbit/repos/web\nHEAD abc\nbranch refs/heads/main\n\nworktree C:/orbit/workspaces/one/web\nHEAD def\nbranch refs/heads/feat/one\n\nworktree C:/tmp/gone\nHEAD 123\ndetached\nprunable gitdir file points to non-existent location\n";
        let clone = normalize_path("C:/orbit/repos/web");
        let wt = parse_worktrees(raw, &clone, &normalize_path("C:/orbit/workspaces"));
        assert_eq!(wt.len(), 3);
        assert!(wt[0].main && wt[0].workspace.is_none());
        assert_eq!((wt[1].workspace.as_deref(), wt[1].branch.as_deref()), (Some("one"), Some("feat/one")));
        assert!(wt[2].missing && wt[2].branch.is_none());
    }

    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"];
        full.extend_from_slice(args);
        crate::proc::run("git", &full, Some(dir)).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    }

    /// Commit, discard, undo and stash on a real clone, through the config
    /// (the repo is found by name like the view does).
    #[test]
    fn manages_a_clone_like_github_desktop() {
        let _g = crate::workspace::tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-repo-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repos/web")).unwrap();
        std::env::set_var("ORBIT_WORKSPACE_ROOT", &root);
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")] {
            std::env::set_var(k, v);
        }
        let dir = root.join("repos/web");
        git(&dir, &["init"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-m", "init"]);

        // Commit only the selected file.
        std::fs::write(dir.join("a.txt"), "a2").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        commit("web", "change a", &["a.txt".into()]).unwrap();
        let o = overview("web").unwrap();
        assert_eq!((o.branch.as_deref(), o.changes), (Some("main"), 1), "b.txt stays uncommitted");

        // Discard: the untracked file goes to the Trash, a tracked edit is reverted.
        std::fs::write(dir.join("a.txt"), "dirty").unwrap();
        discard("web", &["a.txt".into(), "b.txt".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a2");
        assert!(!dir.join("b.txt").exists());

        // Undo the local commit: its change is back in the working tree.
        undo_commit("web").unwrap();
        assert_eq!(history("web", 10).unwrap().len(), 1);
        assert_eq!(overview("web").unwrap().changes, 1);

        // Stash, branch, switch back and pop.
        stash("web").unwrap();
        assert_eq!(overview("web").unwrap().stashes.len(), 1);
        create_branch("web", "feat/z", None).unwrap();
        assert_eq!(overview("web").unwrap().branch.as_deref(), Some("feat/z"));
        switch("web", "main", false).unwrap();
        stash_pop("web", 0).unwrap();
        let o = overview("web").unwrap();
        assert_eq!((o.changes, o.stashes.len()), (1, 0));
        assert!(o.branches.iter().any(|b| b.name == "feat/z" && !b.current));
        delete_branch("web", "feat/z", false).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
