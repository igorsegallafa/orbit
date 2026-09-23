// Workspace = one feature worked across multiple repos, each repo getting
// its own git worktree under <root>/workspaces/<name>/<repo>.
// Metadata lives in <root>/workspaces/<name>/.workspace.yaml (same source
// of truth pattern as the generosity-workspace CLI).
use crate::config::Config;
use crate::git;
use crate::links;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Workspace {
    pub name: String,
    pub branch: String,
    pub base: String,
    pub repos: Vec<String>,
    /// Tracker card this workspace was created from (enables Plan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<CardRef>,
    /// PRs opened from this workspace, persisted once created — the
    /// workspace knows its own PRs; no re-searching GitHub every load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pr_refs: Vec<PrRef>,
    /// Race variant: the workspace this one competes for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    /// Agent racing in this variant, e.g. "claude · claude-opus-5".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrRef {
    pub repo: String,
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CardRef {
    pub kind: String,   // "shortcut" | "linear"
    pub id: String,     // "sc-123" | "ENG-123"
    pub title: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub repo: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub ahead: usize,
    pub behind: usize,
    /// Commits on origin/<branch> that origin/<base> lacks — the content
    /// a PR would actually carry. "Pushed, no PR, with content" is the
    /// resume point of an interrupted PR-creation flow.
    #[serde(default)]
    pub pr_commits: usize,
    /// The branch's work is already in origin/<base> (e.g. its PR was
    /// squash-merged), so its `ahead` commits need no push.
    #[serde(default)]
    pub integrated: bool,
}

/// Default home of the clones and workspaces folders (each can be moved
/// in Settings) and of Orbit's build cache.
pub fn workspace_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("ORBIT_WORKSPACE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    Ok(crate::config::home_dir()?.join("Documents").join("orbit-workspace"))
}

/// Folder new base clones go to.
pub fn repos_dir() -> Result<PathBuf, String> {
    match Config::load()?.repos_dir_override() {
        Some(p) => Ok(p),
        None => Ok(workspace_root()?.join("repos")),
    }
}

pub fn workspaces_dir() -> Result<PathBuf, String> {
    match Config::load()?.workspaces_dir_override() {
        Some(p) => Ok(p),
        None => Ok(workspace_root()?.join("workspaces")),
    }
}

/// Where a repo's base clone lives: its custom path, else the clones folder.
pub fn clone_dir(svc: &crate::config::Service) -> Result<PathBuf, String> {
    match svc.custom_path() {
        Some(p) => Ok(p),
        None => Ok(repos_dir()?.join(&svc.name)),
    }
}

/// `clone_dir` by repo name (repos dropped from the config fall back to
/// the clones folder).
pub fn clone_dir_of(name: &str) -> Result<PathBuf, String> {
    match Config::load()?.services.iter().find(|s| s.name == name) {
        Some(svc) => clone_dir(svc),
        None => Ok(repos_dir()?.join(name)),
    }
}

pub fn is_cloned(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Folder of the workspace `name` (holds the meta file and one worktree per repo).
pub fn ws_dir(name: &str) -> Result<PathBuf, String> {
    Ok(workspaces_dir()?.join(name))
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
/// Idempotent: existing worktrees are reused. Branch-only repos
/// (`worktree: false`) get the branch checked out in the base clone and a
/// junction in the workspace folder instead.
pub fn create(
    name: &str,
    branch: &str,
    base: &str,
    repos: &[String],
    card: Option<CardRef>,
) -> Result<Workspace, String> {
    create_at(name, branch, base, repos, card, None)
}

/// `create`, with worktree branches started at `start` (a local ref) instead
/// of the up-to-date base.
fn create_at(
    name: &str,
    branch: &str,
    base: &str,
    repos: &[String],
    card: Option<CardRef>,
    start: Option<&str>,
) -> Result<Workspace, String> {
    if name.trim().is_empty() {
        return Err("workspace name is required".into());
    }
    if repos.is_empty() {
        return Err("select at least one repository".into());
    }

    let cfg = Config::load()?;
    let ws_dir = ws_dir(name)?;

    let svcs = repos
        .iter()
        .map(|repo| {
            cfg.services
                .iter()
                .find(|s| &s.name == repo)
                .ok_or_else(|| format!("unknown repository: {repo}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    for svc in svcs {
        let clone_dir = clone_dir(svc)?;
        if !is_cloned(&clone_dir) {
            git::clone(&svc.repo, &clone_dir)?;
        }
        git::fetch(&clone_dir)?;
        let wt = ws_dir.join(&svc.name);
        if svc.worktree {
            match start {
                Some(s) => git::worktree_add_from(&clone_dir, &wt, branch, s)?,
                None => git::worktree_add(&clone_dir, &wt, branch, base)?,
            }
            links::link_shared(svc, &clone_dir, &wt)?;
            links::copy_worktreeinclude(&clone_dir, &wt);
        } else {
            if git::is_dirty(&clone_dir) {
                return Err(format!(
                    "{} has uncommitted changes in {}; commit or stash them before switching its branch",
                    svc.name,
                    clone_dir.display()
                ));
            }
            git::checkout_branch_in_place(&clone_dir, branch, base)?;
            links::link_dir(&clone_dir, &wt)?;
        }
    }

    finish_create(name, branch, base, repos.to_vec(), card)
}

enum CheckoutResult {
    Ready(String),
    Skipped,
    Failed(String),
}

/// Creates a workspace from a branch that already exists on origin, in
/// every cloned repo that has it. Repos without the branch are skipped;
/// per-repo failures are returned without aborting the rest.
pub fn create_from_branch(name: &str, branch: &str) -> Result<(Workspace, Vec<String>), String> {
    if name.trim().is_empty() || branch.trim().is_empty() {
        return Err("workspace name and branch are required".into());
    }
    let ws_dir = ws_dir(name)?;
    if ws_dir.exists() {
        return Err(format!("workspace '{name}' already exists"));
    }
    let cfg = Config::load()?;
    let cloned: Vec<(&crate::config::Service, PathBuf)> = cfg
        .services
        .iter()
        .filter_map(|s| clone_dir(s).ok().map(|d| (s, d)))
        .filter(|(_, d)| is_cloned(d))
        .collect();
    if cloned.is_empty() {
        return Err("no cloned repositories; clone them in Settings first".into());
    }

    let results: Vec<CheckoutResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = cloned
            .iter()
            .map(|(svc, clone_dir)| {
                let wt = ws_dir.join(&svc.name);
                scope.spawn(move || {
                    if !git::remote_has_branch(clone_dir, branch) {
                        return CheckoutResult::Skipped;
                    }
                    let r = if svc.worktree {
                        git::checkout_remote_branch(clone_dir, branch, Some(&wt))
                            .and_then(|_| links::link_shared(svc, clone_dir, &wt).map(|_| ()))
                            .map(|_| {
                                links::copy_worktreeinclude(clone_dir, &wt);
                            })
                    } else if git::is_dirty(clone_dir) {
                        Err(format!("{} has uncommitted changes in {}", svc.name, clone_dir.display()))
                    } else {
                        git::checkout_remote_branch(clone_dir, branch, None)
                            .and_then(|_| links::link_dir(clone_dir, &wt).map(|_| ()))
                    };
                    match r {
                        Ok(()) => CheckoutResult::Ready(svc.name.clone()),
                        Err(e) => CheckoutResult::Failed(format!("{}: {e}", svc.name)),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(CheckoutResult::Skipped))
            .collect()
    });

    let mut ready = Vec::new();
    let mut failures = Vec::new();
    for r in results {
        match r {
            CheckoutResult::Ready(n) => ready.push(n),
            CheckoutResult::Failed(e) => failures.push(e),
            CheckoutResult::Skipped => {}
        }
    }
    if ready.is_empty() {
        let _ = std::fs::remove_dir_all(&ws_dir);
        return Err(if failures.is_empty() {
            format!("no repository has a '{branch}' branch on origin")
        } else {
            failures.join("; ")
        });
    }
    let base = git::default_branch(&clone_dir_of(&ready[0])?).unwrap_or_else(|| "main".into());
    let ws = finish_create(name, branch, &base, ready, None)?;
    Ok((ws, failures))
}

/// Writes the meta file and copies the agent entrypoints into the folder.
fn finish_create(
    name: &str,
    branch: &str,
    base: &str,
    repos: Vec<String>,
    card: Option<CardRef>,
) -> Result<Workspace, String> {
    let ws_dir = ws_dir(name)?;
    std::fs::create_dir_all(&ws_dir).map_err(|e| e.to_string())?;
    let ws = Workspace {
        name: name.to_string(),
        branch: branch.to_string(),
        base: base.to_string(),
        repos,
        card,
        pr_refs: Vec::new(),
        variant_of: None,
        agent: None,
    };
    let raw = serde_yaml::to_string(&ws).map_err(|e| e.to_string())?;
    std::fs::write(meta_path(&ws_dir), raw).map_err(|e| e.to_string())?;
    links::copy_entrypoints(&workspace_root()?, &ws_dir);
    Ok(ws)
}

fn save_meta(ws_dir: &Path, ws: &Workspace) -> Result<(), String> {
    let raw = serde_yaml::to_string(ws).map_err(|e| e.to_string())?;
    std::fs::write(meta_path(ws_dir), raw).map_err(|e| e.to_string())
}

// ---------- agent races ----------
// A race runs the same task with several agents, each in its own variant
// workspace (same repos and base as the parent, branch `<branch>-<slug>`).
// The winner's work is merged into the parent; the variants go away.

fn slug(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    out.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

pub fn create_variant(parent: &str, label: &str, agent: &str) -> Result<Workspace, String> {
    let p = load_meta(&ws_dir(parent)?)?;
    if p.variant_of.is_some() {
        return Err("a variant can't start its own race".into());
    }
    let s = slug(label);
    if s.is_empty() {
        return Err("variant label is required".into());
    }
    let name = format!("{parent}--{s}");
    if ws_dir(&name)?.exists() {
        return Err(format!("workspace '{name}' already exists"));
    }
    // Branch-only repos switch the base clone itself: variants would fight over it.
    let cfg = Config::load()?;
    if let Some(r) = p.repos.iter().find(|r| cfg.services.iter().any(|s| &s.name == *r && !s.worktree)) {
        return Err(format!("{r} is set to branch-only (no worktree), so it can't be raced"));
    }
    // Variants build on the parent's current work, so the winner merges cleanly.
    create_at(&name, &format!("{}-{s}", p.branch), &p.base, &p.repos, p.card.clone(), Some(&p.branch))?;
    let dir = ws_dir(&name)?;
    let mut ws = load_meta(&dir)?;
    ws.variant_of = Some(parent.to_string());
    ws.agent = Some(agent.to_string());
    save_meta(&dir, &ws)?;
    Ok(ws)
}

#[derive(Debug, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VariantStats {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub commits: usize,
}

/// What a workspace changed since its base, summed over its repos.
pub fn variant_stats(name: &str) -> Result<VariantStats, String> {
    let dir = ws_dir(name)?;
    let ws = load_meta(&dir)?;
    // A variant is measured against its parent's branch (what it started from).
    let parent_branch = ws.variant_of.as_deref().and_then(|p| ws_dir(p).ok()).and_then(|d| load_meta(&d).ok()).map(|p| p.branch);
    let mut st = VariantStats::default();
    for repo in &ws.repos {
        let wt = dir.join(repo);
        let (f, i, d, c) = match &parent_branch {
            Some(b) => git::diff_since(&wt, b),
            None => git::diff_since_base(&wt, &ws.base),
        };
        st.files += f;
        st.insertions += i;
        st.deletions += d;
        st.commits += c;
    }
    Ok(st)
}

/// Merges the winning variant into its parent (committing any work the
/// agent left uncommitted) and removes every variant of that race.
// ponytail: repos merge one by one; a conflict in a later repo leaves the
// earlier ones merged (reported). Pre-check with `git merge-tree` if it bites.
pub fn adopt_variant(variant: &str) -> Result<Vec<String>, String> {
    let vdir = ws_dir(variant)?;
    let v = load_meta(&vdir)?;
    let parent = v.variant_of.clone().ok_or_else(|| format!("'{variant}' is not a race variant"))?;
    let pdir = ws_dir(&parent)?;
    let p = load_meta(&pdir)?;
    for repo in &p.repos {
        if git::is_dirty(&pdir.join(repo)) {
            return Err(format!("{repo} in {parent} has uncommitted changes; commit or stash them first"));
        }
    }
    let label = v.agent.clone().unwrap_or_else(|| variant.to_string());
    for repo in &v.repos {
        let wt = vdir.join(repo);
        if git::is_dirty(&wt) {
            git::commit_all(&wt, &format!("{label}: race result"))?;
        }
    }
    let mut merged = Vec::new();
    for repo in p.repos.iter().filter(|r| v.repos.contains(r)) {
        if let Err(e) = git::merge(&pdir.join(repo), &v.branch) {
            let done = if merged.is_empty() { String::new() } else { format!(" (already merged: {})", merged.join(", ")) };
            return Err(format!("{repo}: {e}{done}"));
        }
        merged.push(repo.clone());
    }
    discard_race(&parent)
}

/// Removes every variant of `parent`, branches included.
pub fn discard_race(parent: &str) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for ws in list()?.into_iter().filter(|w| w.variant_of.as_deref() == Some(parent)) {
        if let Err(e) = remove_opts(&ws.name, true, true) {
            notes.push(format!("{}: {e}", ws.name));
        }
    }
    Ok(notes)
}

/// Persists newly created PRs into the workspace meta. Idempotent: a PR
/// already recorded (same repo+number) is kept; entries are never
/// removed — merged/closed state is live data, presence is history.
pub fn save_pr_refs(name: &str, new_refs: &[PrRef]) -> Result<(), String> {
    let ws_dir = ws_dir(name)?;
    let mut ws = load_meta(&ws_dir)?;
    let before = ws.pr_refs.len();
    for r in new_refs {
        if !ws.pr_refs.iter().any(|p| p.repo == r.repo && p.number == r.number) {
            ws.pr_refs.push(r.clone());
        }
    }
    if ws.pr_refs.len() != before {
        let raw = serde_yaml::to_string(&ws).map_err(|e| e.to_string())?;
        std::fs::write(meta_path(&ws_dir), raw).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Removes a workspace: deletes each worktree and its branch, then the
/// workspace directory. Refuses when any worktree is dirty unless forced.
/// Branches holding commits no remote has are kept (never silently lose
/// work); returns one note per kept branch.
pub fn remove(name: &str, force: bool) -> Result<Vec<String>, String> {
    remove_opts(name, force, false)
}

/// `discard`: delete branches even with unpushed commits (race losers,
/// merged winners).
pub fn remove_opts(name: &str, force: bool, discard: bool) -> Result<Vec<String>, String> {
    let ws_dir = ws_dir(name)?;
    let ws = load_meta(&ws_dir)?;

    for repo in &ws.repos {
        let wt = ws_dir.join(repo);
        if wt.exists() && !force && git::is_dirty(&wt) {
            return Err(format!(
                "'{repo}' has uncommitted changes. Commit them or use force remove."
            ));
        }
    }
    let cfg = Config::load()?;
    let mut kept = Vec::new();
    for repo in &ws.repos {
        let clone_dir = clone_dir_of(repo)?;
        let wt = ws_dir.join(repo);
        if links::is_link(&wt) {
            // Branch-only: the folder is a junction into the base clone.
            links::unlink(&wt)?;
            if git::current_branch(&clone_dir).as_deref() == Some(ws.branch.as_str()) {
                if let Some(default) = git::default_branch(&clone_dir) {
                    let _ = git::checkout(&clone_dir, &default);
                }
            }
            continue;
        }
        // Before `git worktree remove`: on Windows git follows junctions
        // and would delete the shared content inside the base clone.
        match cfg.services.iter().find(|s| &s.name == repo) {
            Some(svc) => links::unlink_shared(svc, &wt),
            None => {
                let _ = links::unlink(&wt.join("node_modules"));
            }
        }
        if clone_dir.exists() {
            let _ = git::worktree_remove(&clone_dir, &wt);
            // A squash-merged branch has commits no remote holds, but its
            // work is safe in the base: nothing to keep.
            let lose = if discard || git::is_integrated(&clone_dir, &ws.branch, &ws.base) {
                0
            } else {
                git::unpushed_commits(&clone_dir, &ws.branch)
            };
            match lose {
                0 => {
                    let _ = git::branch_delete(&clone_dir, &ws.branch);
                }
                n => kept.push(format!("{repo}: kept {} ({n} unpushed commit{})", ws.branch, if n == 1 { "" } else { "s" })),
            }
        }
    }
    remove_dir_retrying(&ws_dir)?;
    Ok(kept)
}

/// Windows refuses to delete a folder a process still runs in; sessions
/// closed just before removal can take a moment to exit.
fn remove_dir_retrying(dir: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Err(format!("couldn't delete {}: {last}", dir.display()))
}

/// Collects per-repo status for a workspace.
pub fn status(name: &str) -> Result<Vec<RepoStatus>, String> {
    let ws_dir = ws_dir(name)?;
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
                    pr_commits: 0,
                    integrated: false,
                };
            }
            let (ahead, behind) = git::ahead_behind(&wt, &ws.base);
            let pr_commits = git::remote_branch_feature_commits(&wt, &ws.base);
            RepoStatus {
                repo: repo.clone(),
                branch: git::current_branch(&wt),
                dirty: git::is_dirty(&wt),
                ahead,
                behind,
                pr_commits,
                integrated: ahead > 0 && git::is_integrated(&wt, "HEAD", &ws.base),
            }
        })
        .collect();
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that point ORBIT_* env vars at a temp root must not overlap.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"];
        full.extend_from_slice(args);
        crate::proc::run("git", &full, Some(dir)).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    }

    /// Bare origin with `main` (package.json) and an extra `feat/existing`.
    fn origin(root: &Path) -> PathBuf {
        let bare = root.join("origin.git");
        let seed = root.join("seed");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(&seed).unwrap();
        git(&bare, &["init", "--bare"]);
        git(&seed, &["init"]);
        std::fs::write(seed.join("package.json"), "{}").unwrap();
        std::fs::write(seed.join(".gitignore"), "node_modules
dist
.env
").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-m", "init"]);
        git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&seed, &["push", "origin", "main"]);
        git(&seed, &["push", "origin", "main:feat/existing"]);
        git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        bare
    }

    #[test]
    fn squash_merged_branch_counts_as_integrated() {
        let root = std::env::temp_dir().join(format!("orbit-squash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let bare = origin(&root);
        let wt = root.join("wt");
        git(&root, &["clone", &bare.to_string_lossy(), "wt"]);
        git(&wt, &["checkout", "-b", "feat/x"]);
        std::fs::write(wt.join("a.txt"), "a").unwrap();
        git(&wt, &["add", "a.txt"]);
        git(&wt, &["commit", "-m", "a"]);
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        git(&wt, &["add", "b.txt"]);
        git(&wt, &["commit", "-m", "b"]);
        assert!(!git::is_integrated(&wt, "HEAD", "main"), "unmerged work");

        // Squash-merge on the "server": main gets one new commit with the same content.
        let srv = root.join("srv");
        git(&root, &["clone", &bare.to_string_lossy(), "srv"]);
        git(&srv, &["fetch", &wt.to_string_lossy(), "feat/x"]);
        git(&srv, &["merge", "--squash", "FETCH_HEAD"]);
        git(&srv, &["commit", "-m", "feat x (#1)"]);
        git(&srv, &["push", "origin", "main"]);
        git(&wt, &["fetch"]);
        assert!(git::is_integrated(&wt, "HEAD", "main"), "squash-merged work is in main");

        // New work after the merge is not.
        std::fs::write(wt.join("c.txt"), "c").unwrap();
        git(&wt, &["add", "c.txt"]);
        git(&wt, &["commit", "-m", "c"]);
        assert!(!git::is_integrated(&wt, "HEAD", "main"), "post-merge commit still needs a push");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_lifecycle_links_shared_dirs_and_never_deletes_them() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-ws-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));

        let bare = origin(&root);
        let mut svc = crate::config::Service::new("web".into(), bare.to_string_lossy().into());
        svc.build = Some(crate::config::BuildCmd::All("echo build".into()));
        Config { services: vec![svc], ..Default::default() }.save().unwrap();

        // First create clones; then give the base clone an install to share.
        create("one", "feat/one", "main", &["web".into()], None).unwrap();
        let clone = repos_dir().unwrap().join("web");
        std::fs::create_dir_all(clone.join("node_modules/pkg")).unwrap();
        std::fs::write(clone.join("node_modules/pkg/index.js"), "x").unwrap();
        let wt = ws_dir("one").unwrap().join("web");
        assert!(links::is_link(&wt.join("node_modules")));
        assert!(wt.join("node_modules/pkg/index.js").exists(), "install visible through the link");
        assert!(links::is_link(&wt.join("dist")));

        // Local-only commit: removing must keep the branch.
        std::fs::write(wt.join("local.txt"), "x").unwrap();
        git(&wt, &["add", "local.txt"]);
        git(&wt, &["commit", "-m", "local"]);
        let kept = remove("one", true).unwrap();
        assert!(!ws_dir("one").unwrap().exists());
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(git::branch_exists(&clone, "feat/one"), "unpushed branch must survive");

        // .worktreeinclude copies untracked files into new worktrees.
        std::fs::write(clone.join(".env"), "SECRET=1").unwrap();
        std::fs::write(clone.join(".worktreeinclude"), ".env\n").unwrap();
        create("two", "feat/two", "main", &["web".into()], None).unwrap();
        assert_eq!(std::fs::read_to_string(ws_dir("two").unwrap().join("web/.env")).unwrap(), "SECRET=1");
        assert!(remove("two", true).unwrap().is_empty(), "nothing unpushed on a fresh branch");

        // Race: two variants, the winner's (uncommitted) work lands in the parent.
        create("race", "feat/race", "main", &["web".into()], None).unwrap();
        let race_wt = ws_dir("race").unwrap().join("web");
        std::fs::write(race_wt.join("parent.txt"), "p").unwrap();
        git(&race_wt, &["add", "parent.txt"]);
        git(&race_wt, &["commit", "-m", "parent work"]);
        let a = create_variant("race", "Claude", "claude").unwrap();
        assert!(ws_dir(&a.name).unwrap().join("web/parent.txt").exists(), "variants start from the parent's work");
        let b = create_variant("race", "opencode", "opencode").unwrap();
        assert_eq!(a.name, "race--claude");
        assert_eq!(a.branch, "feat/race-claude");
        assert_eq!(list().unwrap().iter().filter(|w| w.variant_of.as_deref() == Some("race")).count(), 2);
        std::fs::write(ws_dir(&a.name).unwrap().join("web/win.txt"), "a\nb\n").unwrap();
        let st = variant_stats(&a.name).unwrap();
        assert_eq!(st, VariantStats { files: 1, insertions: 0, deletions: 0, commits: 0 }, "only the variant's own work counts");
        assert!(create_variant(&a.name, "x", "x").is_err(), "no nested races");
        let notes = adopt_variant(&a.name).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert!(ws_dir("race").unwrap().join("web/win.txt").exists(), "winner merged into parent");
        assert!(!ws_dir(&a.name).unwrap().exists() && !ws_dir(&b.name).unwrap().exists());
        assert!(!git::branch_exists(&clone, "feat/race-opencode"), "loser branch discarded");
        remove("race", true).unwrap();
        assert!(
            clone.join("node_modules/pkg/index.js").exists(),
            "removing a workspace must not delete the shared install"
        );

        let (ws, failures) = create_from_branch("existing", "feat/existing").unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(ws.repos, ["web"]);
        assert_eq!(ws.base, "main");
        let wt = ws_dir("existing").unwrap().join("web");
        assert_eq!(git::current_branch(&wt).as_deref(), Some("feat/existing"));
        assert!(links::is_link(&wt.join("node_modules")));
        assert!(create_from_branch("nope", "feat/missing").is_err());
        remove("existing", true).unwrap();

        std::env::remove_var("ORBIT_WORKSPACE_ROOT");
        std::env::remove_var("ORBIT_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_rejects_unknown_repo_and_empty_selection() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // No config on a fresh environment: any repo selection is unknown.
        let err = create("ws", "feat/ws", "main", &["ghost-repo".into()], None)
            .expect_err("should reject unknown repo");
        assert!(err.contains("unknown repository"));

        let err = create("ws", "feat/ws", "main", &[], None).expect_err("should reject empty selection");
        assert!(err.contains("at least one"));
    }
}
#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn repo_status_serializes_camel_case_for_frontend() {
        // pr_commits → prCommits: the frontend reads prCommits; a missing
        // rename makes the field undefined and every status-driven button
        // silently breaks (bit us twice — also PullRequest).
        let st = RepoStatus {
            repo: "svc".into(),
            branch: Some("feat/x".into()),
            dirty: true,
            ahead: 2,
            behind: 0,
            pr_commits: 1,
            integrated: false,
        };
        let v = serde_json::to_value(&st).unwrap();
        assert!(v.get("prCommits").is_some(), "must serialize prCommits: {v}");
        // round-trip must also accept camelCase on the way back (if ever)
        let back: RepoStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back.pr_commits, 1);
    }
}
