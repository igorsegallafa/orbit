// Thin wrappers around the `git` binary via shell-out (same approach as the
// generosity-workspace CLI: worktree support in pure Rust libs is weak).
use std::path::Path;

pub fn remove_clone(dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(dest).map_err(|e| e.to_string())
}

/// Clones into `dest` (missing or empty folder). A GitHub SSH URL that
/// fails for lack of an SSH key is retried over HTTPS, which the `gh`
/// login covers.
pub fn clone(repo_url: &str, dest: &Path) -> Result<(), String> {
    let occupied = std::fs::read_dir(dest).map(|mut d| d.next().is_some()).unwrap_or(false);
    if occupied {
        return Err(format!("{} already exists and is not empty", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let dest_s = dest.to_string_lossy();
    let result = match git(&["clone", repo_url, &dest_s]) {
        Err(e) if e.contains("Permission denied (publickey)") => match github_https(repo_url) {
            Some(https) => git(&["clone", &https, &dest_s]),
            None => Err(e),
        },
        r => r,
    };
    // git's "Cloning into '...'..." progress line buries the actual cause.
    result.map_err(|e| {
        e.lines()
            .filter(|l| !l.starts_with("Cloning into"))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// `git@github.com:owner/repo(.git)` / `ssh://git@github.com/owner/repo` ->
/// `https://github.com/owner/repo.git`.
pub fn github_https(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    Some(format!("https://github.com/{path}.git"))
}

/// Adds a worktree at `path` on branch `branch`, based on `base`
/// of the repo cloned at `repo_dir`. Idempotent: if the worktree path
/// already exists it is reused; if the branch already exists (leftover
/// from a removed workspace), it is reused instead of failing on `-b`.
pub fn worktree_add(repo_dir: &Path, path: &Path, branch: &str, base: &str) -> Result<(), String> {
    worktree_add_from(repo_dir, path, branch, &start_point(repo_dir, base))
}

/// `worktree_add` starting a new branch at an explicit ref (e.g. another
/// local branch) instead of the up-to-date base.
pub fn worktree_add_from(repo_dir: &Path, path: &Path, branch: &str, start: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    // Branch already present (e.g. previous workspace removed but branch
    // survived)? Check it out without creating.
    if branch_exists(repo_dir, branch) {
        return git_in(repo_dir, &["worktree", "add", &path.to_string_lossy(), branch]);
    }
    git_in(repo_dir, &["worktree", "add", "-b", branch, &path.to_string_lossy(), start])
}

/// `origin/<base>` when it exists: the local base branch is not moved by
/// `fetch`, so branching from it could start the feature from stale code.
fn start_point(repo_dir: &Path, base: &str) -> String {
    let remote = format!("origin/{base}");
    if ref_exists(repo_dir, &format!("refs/remotes/{remote}")) {
        remote
    } else {
        base.to_string()
    }
}

/// Default branch of origin (`main`), from origin/HEAD.
pub fn default_branch(repo_dir: &Path) -> Option<String> {
    let r = crate::proc::run("git", &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], Some(repo_dir)).ok()?;
    Some(r.trim().trim_start_matches("origin/").to_string()).filter(|b| !b.is_empty())
}

/// True when origin has a branch named `branch`.
pub fn remote_has_branch(repo_dir: &Path, branch: &str) -> bool {
    crate::proc::run("git", &["ls-remote", "--exit-code", "--heads", "origin", branch], Some(repo_dir))
        .is_ok_and(|o| !o.trim().is_empty())
}

/// Sets up `branch` tracking origin/<branch>: as a worktree at `path`, or
/// (when `path` is None) checked out in the clone itself. A leftover local
/// branch is reset to the remote.
pub fn checkout_remote_branch(repo_dir: &Path, branch: &str, path: Option<&Path>) -> Result<(), String> {
    git_in(repo_dir, &["fetch", "origin", branch])?;
    let remote = format!("origin/{branch}");
    let local = branch_exists(repo_dir, branch);
    match path {
        Some(p) => {
            let p = p.to_string_lossy();
            if local {
                git_in(repo_dir, &["worktree", "add", &p, branch])?;
                git_in(Path::new(p.as_ref()), &["reset", "--hard", &remote])
            } else {
                git_in(repo_dir, &["worktree", "add", &p, "-b", branch, &remote])
            }
        }
        None if local => {
            git_in(repo_dir, &["checkout", branch])?;
            git_in(repo_dir, &["reset", "--hard", &remote])
        }
        None => git_in(repo_dir, &["checkout", "-b", branch, &remote]),
    }
}

// ---------- Checking out someone's pull request ----------

/// Checks out origin's `branch`, tracking it (push and pull go to the PR):
/// as a worktree at `path`, or in the clone itself when `path` is None. A
/// leftover local branch is brought to origin's tip unless it holds commits
/// origin lacks; those are kept, and reported, rather than reset away.
pub fn checkout_tracking(repo_dir: &Path, branch: &str, path: Option<&Path>) -> Result<Option<String>, String> {
    git_in(repo_dir, &["fetch", "origin", branch])?;
    let remote = format!("origin/{branch}");
    let mut note = None;
    if branch_exists(repo_dir, branch) {
        let local_only: usize = git_out(repo_dir, &["rev-list", "--count", branch, "--not", &remote])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let checked_out_here = current_branch(repo_dir).as_deref() == Some(branch);
        if local_only > 0 {
            note = Some(format!("kept {local_only} local commit(s) on {branch} that origin doesn't have"));
        } else if !checked_out_here {
            git_in(repo_dir, &["branch", "--force", branch, &remote])?;
        }
        match path {
            Some(p) => git_in(repo_dir, &["worktree", "add", &p.to_string_lossy(), branch])?,
            None => git_in(repo_dir, &["checkout", branch])?,
        }
        if local_only == 0 && checked_out_here {
            git_in(repo_dir, &["merge", "--ff-only", &remote])?;
        }
    } else {
        match path {
            Some(p) => git_in(repo_dir, &["worktree", "add", "--track", "-b", branch, &p.to_string_lossy(), &remote])?,
            None => git_in(repo_dir, &["checkout", "--track", "-b", branch, &remote])?,
        }
    }
    let _ = git_in(repo_dir, &["branch", "--set-upstream-to", &remote, branch]);
    Ok(note)
}

/// A PR from a fork (its branch isn't on origin): GitHub keeps its head at
/// `pull/<n>/head`, fetched into a local `pr-<n>` branch. It has no upstream:
/// there's nowhere to push. Returns the local branch name.
pub fn checkout_pr_head(repo_dir: &Path, number: u64, path: Option<&Path>) -> Result<String, String> {
    let local = format!("pr-{number}");
    git_in(repo_dir, &["fetch", "origin", &format!("+pull/{number}/head:{local}")])?;
    match path {
        Some(p) => git_in(repo_dir, &["worktree", "add", &p.to_string_lossy(), &local])?,
        None => git_in(repo_dir, &["checkout", &local])?,
    }
    Ok(local)
}

/// Brings a checked-out branch up to its upstream (fast-forward only).
pub fn pull_ff(dir: &Path) -> Result<(), String> {
    git_in(dir, &["pull", "--ff-only"])
}

/// Checks out `branch` in the clone itself (branch-only repos), creating it
/// from origin/<base> when missing.
pub fn checkout_branch_in_place(repo_dir: &Path, branch: &str, base: &str) -> Result<(), String> {
    if branch_exists(repo_dir, branch) {
        git_in(repo_dir, &["checkout", branch])
    } else {
        let start = start_point(repo_dir, base);
        git_in(repo_dir, &["checkout", "-b", branch, &start])
    }
}

/// Switches the clone back to `branch`.
pub fn checkout(repo_dir: &Path, branch: &str) -> Result<(), String> {
    git_in(repo_dir, &["checkout", branch])
}

/// The rebase / merge / cherry-pick / revert paused in the repo whose git
/// dir is `git_dir`, if any.
pub fn operation_in_progress(git_dir: &Path) -> Option<String> {
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

/// `operation_in_progress` for the checkout at `dir` (worktrees have their
/// own git dir).
pub fn operation(dir: &Path) -> Option<String> {
    let git_dir = git_out(dir, &["rev-parse", "--absolute-git-dir"]).ok()?;
    operation_in_progress(Path::new(git_dir.trim()))
}

/// Stash message for work shelved when Orbit switches a checkout away from
/// `branch`; it's re-applied when Orbit switches back.
fn autostash_message(branch: &str) -> String {
    format!("orbit autostash: {branch}")
}

/// Shelves uncommitted work (untracked files included) left on `branch`.
pub fn autostash(dir: &Path, branch: &str) -> Result<(), String> {
    git_in(dir, &["stash", "push", "--include-untracked", "-m", &autostash_message(branch)])
}

/// Re-applies the work `autostash` shelved on `branch`. Ok(false) when there
/// is none. On conflict git keeps the stash entry, so nothing is lost.
pub fn autostash_restore(dir: &Path, branch: &str) -> Result<bool, String> {
    let msg = autostash_message(branch);
    // %gs reads "On <branch>: <message>".
    let list = git_out(dir, &["stash", "list", "--format=%gs"]).unwrap_or_default();
    let Some(index) = list.lines().position(|l| l.ends_with(&format!(": {msg}"))) else {
        return Ok(false);
    };
    git_in(dir, &["stash", "pop", &format!("stash@{{{index}}}")]).map(|_| true)
}

/// Removes the worktree at `path` from the repo cloned at `repo_dir`,
/// pruning stale worktree metadata. Falls back to deleting the directory
/// when git refuses (e.g. ignored files like node_modules inside).
pub fn worktree_remove(repo_dir: &Path, path: &Path) -> Result<(), String> {
    if !path.exists() {
        let _ = git_in(repo_dir, &["worktree", "prune"]);
        return Ok(());
    }
    let out = crate::proc::cmd("git")
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
/// Commits on `branch` that no remote ref has: work that deleting the
/// branch would lose.
pub fn unpushed_commits(repo_dir: &Path, branch: &str) -> usize {
    git_out(repo_dir, &["rev-list", "--count", branch, "--not", "--remotes"])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn branch_delete(repo_dir: &Path, branch: &str) -> Result<(), String> {
    let _ = git_in(repo_dir, &["branch", "-D", branch]);
    Ok(())
}

/// True when the repo has a local branch with this name.
pub fn branch_exists(repo_dir: &Path, branch: &str) -> bool {
    crate::proc::cmd("git")
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
    let out = crate::proc::cmd("git")
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

/// Merges `branch` into the checked-out branch of `dir`; on conflict the
/// merge is aborted so the worktree is left as it was.
pub fn merge(dir: &Path, branch: &str) -> Result<(), String> {
    let out = crate::proc::cmd("git")
        .args(["merge", "--no-ff", "--no-edit", branch])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let _ = git_in(dir, &["merge", "--abort"]);
    let msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Err(if msg.contains("CONFLICT") { format!("merge conflict: {}", msg.lines().find(|l| l.contains("CONFLICT")).unwrap_or(&msg)) } else { msg })
}

/// Files, added and removed lines and commits of `dir` since it forked
/// from `base`, uncommitted and untracked work included.
pub fn diff_since_base(dir: &Path, base: &str) -> (usize, usize, usize, usize) {
    diff_since(dir, &start_point(dir, base))
}

/// Same as `diff_since_base`, measured from where HEAD forked off `start`.
pub fn diff_since(dir: &Path, start: &str) -> (usize, usize, usize, usize) {
    let Ok(mb) = git_out(dir, &["merge-base", "HEAD", start]) else { return (0, 0, 0, 0) };
    let mb = mb.trim();
    let (mut files, mut ins, mut del) = (0, 0, 0);
    for line in git_out(dir, &["diff", "--numstat", mb]).unwrap_or_default().lines() {
        let mut parts = line.split_whitespace();
        let (a, d) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"));
        files += 1;
        ins += a.parse::<usize>().unwrap_or(0);
        del += d.parse::<usize>().unwrap_or(0);
    }
    files += git_out(dir, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).count();
    let commits = git_out(dir, &["rev-list", "--count", &format!("{mb}..HEAD")]).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    (files, ins, del, commits)
}

/// Pushes `branch` to origin, creating the upstream on first push.
/// `force` pushes with `--force-with-lease` (a branch rewritten by a
/// rebase): it still refuses if origin moved since the last fetch, so
/// nobody else's commits get overwritten.
pub fn push(dir: &Path, branch: &str, force: bool) -> Result<(), String> {
    let mut args = vec!["push", "-u"];
    if force {
        args.push("--force-with-lease");
    }
    args.extend(["origin", branch]);
    git_in(dir, &args)
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
    let out = crate::proc::cmd("git")
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
    let out = crate::proc::cmd("git")
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
            let out = crate::proc::cmd("git")
                .args(["rev-list", "--count", &format!("HEAD..{rb}")])
                .current_dir(path)
                .output();
            parse_count(out)
        }
        None => 0,
    };
    (ahead, behind)
}

/// True when merging `rev` into origin/<base> would change nothing: all of
/// its work already landed there, even squash-merged or rebased (the commit
/// ids differ, the content doesn't). Conflicts or errors count as "not
/// integrated". Needs git >= 2.38 (`merge-tree --write-tree`).
pub fn is_integrated(dir: &Path, rev: &str, base: &str) -> bool {
    let target = format!("origin/{base}");
    let Ok(merged) = git_out(dir, &["merge-tree", "--write-tree", &target, rev]) else { return false };
    let Ok(base_tree) = git_out(dir, &["rev-parse", &format!("{target}^{{tree}}")]) else { return false };
    merged.lines().next().map(str::trim) == Some(base_tree.trim())
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
    let out = crate::proc::cmd("git")
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
    crate::proc::cmd("git")
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
    let out = crate::proc::cmd("git").args(&args_ref).current_dir(path).output();
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
    let Ok(out) = crate::proc::cmd("git")
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
    let out = crate::proc::cmd("git")
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
    let out = crate::proc::cmd("git")
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
    crate::proc::run("git", args, None).map(|_| ())
}

fn git_in(dir: &Path, args: &[&str]) -> Result<(), String> {
    crate::proc::run("git", args, Some(dir)).map(|_| ())
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
    crate::proc::run("git", args, Some(dir))
}

/// Working-tree changes (unstaged + untracked) with per-file +N/-M stats.
pub fn changes(dir: &Path) -> Result<Vec<ChangeEntry>, String> {
    // -uall: list the files inside untracked folders; without it git
    // reports just "?? folder/", which is not a file that can be diffed.
    let status = git_out(dir, &["status", "--porcelain", "-uall"])?;
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
    Ok(parse_porcelain(&status)
        .into_iter()
        .map(|(path, status)| {
            let (added, deleted) = stats.get(&path).copied().unwrap_or((0, 0));
            ChangeEntry { path, status: status.to_string(), added, deleted }
        })
        .collect())
}

/// `git status --porcelain` lines -> (path, "M"|"A"|"D"|"U"). Renames keep
/// the new path; quoted paths (spaces, unicode) are unquoted.
fn parse_porcelain(status: &str) -> Vec<(String, &'static str)> {
    status
        .lines()
        .filter(|l| l.len() >= 4)
        .map(|line| {
            let xy = line.as_bytes();
            let raw = &line[3..];
            let path = raw.rsplit(" -> ").next().unwrap_or(raw).trim();
            let path = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')).unwrap_or(path);
            let status = match (xy[0], xy[1]) {
                (b'?', _) | (_, b'?') => "U",
                (_, b'D') | (b'D', b' ') => "D",
                (b'A', _) | (_, b'A') => "A",
                _ => "M",
            };
            (path.replace("\\\"", "\""), status)
        })
        .collect()
}

#[cfg(test)]
mod porcelain_tests {
    use super::parse_porcelain;

    #[test]
    fn parses_untracked_renamed_quoted_and_deleted_entries() {
        let out = parse_porcelain(
            " M src/a.rs\n?? scripts/ralph/prd.json\nR  old.txt -> new.txt\n?? \"with space.md\"\n D gone.rs\nA  added.rs\n",
        );
        assert_eq!(
            out,
            vec![
                ("src/a.rs".to_string(), "M"),
                ("scripts/ralph/prd.json".to_string(), "U"),
                ("new.txt".to_string(), "M"),
                ("with space.md".to_string(), "U"),
                ("gone.rs".to_string(), "D"),
                ("added.rs".to_string(), "A"),
            ]
        );
    }
}

/// Recent commits on the current branch.
pub fn commits(dir: &Path, limit: usize) -> Result<Vec<CommitEntry>, String> {
    commits_in(dir, &[], limit)
}

/// Commits `revs` selects (e.g. ["main..HEAD"]; empty = the current branch),
/// newest first.
pub fn commits_in(dir: &Path, revs: &[&str], limit: usize) -> Result<Vec<CommitEntry>, String> {
    let fmt = "--pretty=format:%H%x1f%s%x1f%an%x1f%cr";
    let n = format!("-{limit}");
    let mut args = vec!["log", &n, fmt];
    args.extend_from_slice(revs);
    let raw = git_out(dir, &args)?;
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

/// The (before, after) revisions to diff for `sha`: its parent and itself,
/// or for a range "base...head" the fork point and head.
pub fn diff_sides(dir: &Path, sha: &str) -> (String, String) {
    match sha.split_once("...") {
        Some((base, head)) => {
            let mb = git_out(dir, &["merge-base", base, head]).map(|s| s.trim().to_string()).unwrap_or_else(|_| base.to_string());
            (mb, head.to_string())
        }
        None => (format!("{sha}^"), sha.to_string()),
    }
}

/// Full content of a file at a revision (HEAD by default).
pub fn rev_content(dir: &Path, path: &str, rev: &str) -> Result<String, String> {
    git_out(dir, &["show", &format!("{rev}:{path}")])
}

/// Files touched by a commit (paths + change letter + stats).
/// `sha` may also be a range "base...head": the files the head side changed
/// since it forked from base (what a PR of it would show).
pub fn commit_files(dir: &Path, sha: &str) -> Result<Vec<ChangeEntry>, String> {
    // --name-status gives the letter (A/M/D); --numstat gives +/-. Parse both.
    let (names, nums) = if sha.contains("...") {
        (git_out(dir, &["diff", "--name-status", sha])?, git_out(dir, &["diff", "--numstat", sha])?)
    } else {
        (git_out(dir, &["show", "--name-status", "--format=", sha])?, git_out(dir, &["show", "--numstat", "--format=", sha])?)
    };
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
        let path = parts.next_back().unwrap_or("").to_string();
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
#[cfg(test)]
mod clone_url_tests {
    use super::github_https;

    #[test]
    fn github_https_converts_ssh_urls() {
        assert_eq!(github_https("git@github.com:org/repo.git").as_deref(), Some("https://github.com/org/repo.git"));
        assert_eq!(github_https("ssh://git@github.com/org/repo").as_deref(), Some("https://github.com/org/repo.git"));
        assert_eq!(github_https("https://gitlab.com/org/repo.git"), None);
    }
}
