// Thin wrappers around the `git` binary via shell-out (same approach as the
// generosity-workspace CLI: worktree support in pure Rust libs is weak).
use std::path::Path;
use std::process::Command;

pub fn is_cloned(repos_dir: &Path, name: &str) -> bool {
    repos_dir.join(name).join(".git").exists()
}

pub fn remove_clone(dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(dest).map_err(|e| e.to_string())
}

pub fn clone(repo_url: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Err(format!("destination already exists: {}", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    git(&["clone", repo_url, &dest.to_string_lossy()])
}

/// Adds a worktree at `path` on branch `branch`, based on `base`
/// of the repo cloned at `repo_dir`. Idempotent: if the worktree path
/// already exists it is reused; if the branch already exists (leftover
/// from a removed workspace), it is reused instead of failing on `-b`.
pub fn worktree_add(repo_dir: &Path, path: &Path, branch: &str, base: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    // Branch already present (e.g. previous workspace removed but branch
    // survived)? Check it out without creating.
    if branch_exists(repo_dir, branch) {
        return git_in(repo_dir, &["worktree", "add", &path.to_string_lossy(), branch]);
    }
    git_in(repo_dir, &["worktree", "add", "-b", branch, &path.to_string_lossy(), base])
}

/// Removes the worktree at `path` from the repo cloned at `repo_dir`,
/// pruning stale worktree metadata. Falls back to deleting the directory
/// when git refuses (e.g. ignored files like node_modules inside).
pub fn worktree_remove(repo_dir: &Path, path: &Path) -> Result<(), String> {
    if !path.exists() {
        let _ = git_in(repo_dir, &["worktree", "prune"]);
        return Ok(());
    }
    let out = Command::new("git")
        .args(["worktree", "remove", "--force", &path.to_string_lossy()])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        // git refuses to remove worktrees containing ignored files;
        // delete manually and prune metadata instead.
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    let _ = git_in(repo_dir, &["worktree", "prune"]);
    Ok(())
}

/// Deletes a local branch, ignoring "not found".
pub fn branch_delete(repo_dir: &Path, branch: &str) -> Result<(), String> {
    let _ = git_in(repo_dir, &["branch", "-D", branch]);
    Ok(())
}

/// True when the repo has a local branch with this name.
pub fn branch_exists(repo_dir: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .current_dir(repo_dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn fetch(repo_dir: &Path) -> Result<(), String> {
    git_in(repo_dir, &["fetch", "--prune"])
}

// ---------- Workspace pipeline (commit / push / rebase) ----------

/// Stages everything and commits with `message`. Errors when there is
/// nothing to commit.
pub fn commit_all(dir: &Path, message: &str) -> Result<(), String> {
    git_in(dir, &["add", "-A"])?;
    let out = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("nothing to commit") || err.is_empty() {
            return Err("nothing to commit".into());
        }
        return Err(err);
    }
    Ok(())
}

/// Pushes `branch` to origin, creating the upstream on first push.
pub fn push(dir: &Path, branch: &str) -> Result<(), String> {
    git_in(dir, &["push", "-u", "origin", branch])
}

/// Outcome of a rebase attempt.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum RebaseStatus {
    /// Rebased cleanly.
    Clean,
    /// Conflicts left in the worktree (rebase paused; call
    /// rebase_continue / rebase_abort).
    Conflicts(Vec<String>),
}

/// Fetches and rebases onto `origin/<base>`. On conflict the rebase is
/// left PAUSED with the conflicted files listed — resolve them (by hand
/// or via the AI agent), then `git add` + `rebase_continue`, or
/// `rebase_abort` to roll back.
pub fn rebase_onto(dir: &Path, base: &str) -> Result<RebaseStatus, String> {
    git_in(dir, &["fetch", "--prune"])?;
    let target = format!("origin/{base}");
    let out = Command::new("git")
        .args(["rebase", &target])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        return Ok(RebaseStatus::Clean);
    }
    let conflicts = conflicted_files(dir);
    if !conflicts.is_empty() {
        return Ok(RebaseStatus::Conflicts(conflicts));
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // Not a conflict (e.g. behind with local rebase restrictions): roll
    // back so the worktree is never left mid-rebase.
    let _ = git_in(dir, &["rebase", "--abort"]);
    Err(err)
}

/// Files currently in merge conflict (unmerged paths).
pub fn conflicted_files(dir: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(dir)
        .output();
    let Ok(out) = out else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Continues a paused rebase (expects conflicts resolved + staged).
pub fn rebase_continue(dir: &Path) -> Result<(), String> {
    git_in(dir, &["rebase", "--continue"])
}

/// Aborts a paused rebase, restoring the pre-rebase state.
pub fn rebase_abort(dir: &Path) -> Result<(), String> {
    git_in(dir, &["rebase", "--abort"])
}

/// (ahead, behind) for the pipeline badge. "ahead" = commits that are
/// in HEAD but in NEITHER origin/<base> NOR origin/<branch> — i.e. work
/// of this feature that no PR/remote branch carries yet. Comparing
/// against the upstream alone miscounts other people's main commits
/// (branch created from fresh main, remote branch stale) as "ahead".
/// "behind" = commits in origin/<branch> missing locally (pull needed).
pub fn ahead_behind(path: &Path, base: &str) -> (usize, usize) {
    let remote_branch = current_branch(path)
        .map(|b| format!("origin/{b}"))
        .filter(|rb| ref_exists(path, rb));
    let remote_branch = remote_branch.as_deref();

    // ahead: HEAD --not origin/base origin/branch
    let mut nots: Vec<String> = vec![format!("origin/{base}")];
    if let Some(rb) = remote_branch {
        nots.push(rb.to_string());
    }
    let ahead = count_rev_list(path, &["rev-list", "--count", "HEAD", "--not"], &nots);

    // behind: only meaningful when the branch has a remote; commits on
    // the remote branch that we lack locally (someone pushed / we
    // rewrote history).
    let behind = match remote_branch {
        Some(rb) => {
            let out = Command::new("git")
                .args(["rev-list", "--count", &format!("HEAD..{rb}")])
                .current_dir(path)
                .output();
            parse_count(out)
        }
        None => 0,
    };
    (ahead, behind)
}

/// Commits on origin/<branch> that origin/<base> doesn't have — the
/// actual content a PR would carry. 0 = the branch has nothing of its
/// own (no point opening a PR).
pub fn remote_branch_feature_commits(path: &Path, base: &str) -> usize {
    let Some(branch) = current_branch(path) else { return 0 };
    let remote = format!("origin/{branch}");
    if !ref_exists(path, &remote) {
        return 0;
    }
    let target = format!("origin/{base}");
    let out = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &remote,
            "--not",
            &target,
        ])
        .current_dir(path)
        .output();
    parse_count(out)
}

fn ref_exists(path: &Path, r: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{r}^{{}}")])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn count_rev_list(path: &Path, prefix: &[&str], extra: &[String]) -> usize {
    let mut args: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
    args.extend(extra.iter().cloned());
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = Command::new("git").args(&args_ref).current_dir(path).output();
    parse_count(out)
}

fn parse_count(out: std::io::Result<std::process::Output>) -> usize {
    let Ok(out) = out else { return 0 };
    if !out.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// True when the worktree has uncommitted changes.
pub fn is_dirty(path: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
    else {
        return false;
    };
    out.status.success() && !out.stdout.is_empty()
}

/// Name of the current branch, or None.
pub fn current_branch(path: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "HEAD" {
        return None;
    }
    Some(s)
}

/// Lists candidate base branches from a repo's remote (origin), stripping
/// the remote prefix. Falls back to local branches when there is no remote.
pub fn list_branches(path: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["branch", "-r", "--format=%(refname:short)"])
        .current_dir(path)
        .output();
    let list: Vec<String> = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.contains("HEAD"))
            .map(|l| {
                // strip "origin/" prefix when present
                match l.split_once('/') {
                    Some((_remote, rest)) if !rest.is_empty() => rest.to_string(),
                    _ => l,
                }
            })
            .collect(),
        _ => vec![],
    };
    let mut deduped: Vec<String> = Vec::new();
    for b in list {
        if !deduped.contains(&b) {
            deduped.push(b);
        }
    }
    deduped
}

fn git(args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

fn git_in(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

// ---------- Git review (dock Git tab) ----------

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct ChangeEntry {
    pub path: String,
    /// "M" modified, "A" added, "D" deleted, "U" untracked
    pub status: String,
    pub added: u32,
    pub deleted: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct CommitEntry {
    pub sha: String,
    pub message: String,
    pub author: String,
    /// relative time string from git itself
    pub when: String,
}

/// Full content pair for the diff review pane.
#[derive(Serialize, Clone, Debug)]
pub struct FileDiff {
    pub original: String,
    pub modified: String,
}

fn git_out(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Working-tree changes (unstaged + untracked) with per-file +N/-M stats.
pub fn changes(dir: &Path) -> Result<Vec<ChangeEntry>, String> {
    let status = git_out(dir, &["status", "--porcelain"])?;
    let numstat = git_out(dir, &["diff", "--numstat", "HEAD"])?;
    // map path -> (added, deleted); numstat lines: "<added>\t<deleted>\t<path>"
    let mut stats: std::collections::HashMap<String, (u32, u32)> = std::collections::HashMap::new();
    for line in numstat.lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        stats.insert(p.to_string(), (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
    }
    let mut out = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let path = line[3..].trim().to_string();
        let status = match xy.as_bytes()[1] {
            b'M' => "M",
            b'A' => "A",
            b'D' => "D",
            b'?' => "U",
            b => {
                // staged-only statuses (e.g. first column M with clean second)
                if xy.as_bytes()[0] == b'?' {
                    "U"
                } else if b == b' ' {
                    "M"
                } else {
                    "M"
                }
            }
        };
        let (added, deleted) = stats.get(&path).copied().unwrap_or((0, 0));
        out.push(ChangeEntry {
            path,
            status: status.to_string(),
            added,
            deleted,
        });
    }
    Ok(out)
}

/// Recent commits on the current branch.
pub fn commits(dir: &Path, limit: usize) -> Result<Vec<CommitEntry>, String> {
    let fmt = "--pretty=format:%H%x1f%s%x1f%an%x1f%cr";
    let raw = git_out(dir, &["log", &format!("-{limit}"), fmt])?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split('\x1f');
        let (Some(sha), Some(message), Some(author), Some(when)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        out.push(CommitEntry {
            sha: sha[..8.min(sha.len())].to_string(),
            message: message.to_string(),
            author: author.to_string(),
            when: when.to_string(),
        });
    }
    Ok(out)
}

/// Full content of a file at a revision (HEAD by default).
pub fn rev_content(dir: &Path, path: &str, rev: &str) -> Result<String, String> {
    git_out(dir, &["show", &format!("{rev}:{path}")])
}

/// Files touched by a commit (paths + change letter + stats).
pub fn commit_files(dir: &Path, sha: &str) -> Result<Vec<ChangeEntry>, String> {
    // --name-status gives the letter (A/M/D); --numstat gives +/-. Parse both.
    let names = git_out(dir, &["show", "--name-status", "--format=", sha])?;
    let nums = git_out(dir, &["show", "--numstat", "--format=", sha])?;
    let mut stats: std::collections::HashMap<String, (u32, u32)> = std::collections::HashMap::new();
    for line in nums.lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        stats.insert(p.to_string(), (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
    }
    let mut out = Vec::new();
    for line in names.lines() {
        // "M\tpath" or "R100\told\tnew" (renames keep 3 cols; take the last)
        let mut parts = line.split('\t');
        let Some(letter) = parts.next() else { continue };
        let path = parts.last().unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        let status = match letter.chars().next() {
            Some('A') => "A",
            Some('D') => "D",
            Some('R') | Some('C') => "M",
            _ => "M",
        };
        let (added, deleted) = stats.get(&path).copied().unwrap_or((0, 0));
        out.push(ChangeEntry {
            path,
            status: status.to_string(),
            added,
            deleted,
        });
    }
    Ok(out)
}