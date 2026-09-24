// Workspace = one feature worked across multiple repos, each repo getting
// its own git worktree under <root>/workspaces/<name>/<repo>.
// Metadata lives in <root>/workspaces/<name>/.workspace.yaml (same source
// of truth pattern as the generosity-workspace CLI).
use crate::config::Config;
use crate::git;
use crate::links;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    /// Repos whose branch isn't `branch`: a workspace checked out from PRs
    /// with different branches, or a fork's read-only `pr-<n>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repo_branches: BTreeMap<String, String>,
}

impl Workspace {
    /// The branch `repo` belongs on in this workspace.
    pub fn branch_of(&self, repo: &str) -> &str {
        self.repo_branches.get(repo).map(String::as_str).unwrap_or(&self.branch)
    }
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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
    /// The branch this workspace expects the repo on.
    #[serde(default)]
    pub expected_branch: String,
    /// Checked out on another branch than `expected_branch`: a branch-only
    /// repo's clone is shared, so another workspace may have switched it.
    #[serde(default)]
    pub off_branch: bool,
    /// Branch-only repo: the folder is the base clone itself.
    #[serde(default)]
    pub branch_only: bool,
    /// Switch branch-only repos back without asking (repo setting).
    #[serde(default)]
    pub auto_switch: bool,
    /// Another workspace whose branch the repo is on, when `off_branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_by: Option<String>,
    /// A paused rebase / merge / cherry-pick / revert, which blocks a switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
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

// ---------- Repo scope: a repo's base clone addressed like a workspace ----------

/// The repo a scope name addresses ("@repo" → "repo"). A repo scope is a
/// workspace name for a repo's base clone: the dock, editor, search,
/// terminals and git commands, all keyed by (workspace, repo), then work on
/// the clone itself, outside any workspace.
pub fn repo_scope(workspace: &str) -> Option<&str> {
    workspace.strip_prefix('@')
}

/// Folder of `repo` in `workspace`: its worktree, or the base clone for a repo scope.
pub fn repo_path(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    match repo_scope(workspace) {
        Some(r) if r == repo => clone_dir_of(repo),
        Some(r) => Err(format!("'{repo}' is not part of the '{r}' repository view")),
        None => Ok(ws_dir(workspace)?.join(repo)),
    }
}

/// Root folder of a workspace, or the base clone for a repo scope.
pub fn scope_dir(workspace: &str) -> Result<PathBuf, String> {
    match repo_scope(workspace) {
        Some(repo) => clone_dir_of(repo),
        None => ws_dir(workspace),
    }
}

/// Orbit's own files for a workspace (Ralph runs, custom prompts): its
/// `.orbit` folder, or for a repo scope one outside the clone, so nothing
/// lands in the repo's working tree.
pub fn orbit_dir(workspace: &str) -> Result<PathBuf, String> {
    match repo_scope(workspace) {
        Some(repo) => Ok(workspace_root()?.join(".orbit").join("repos").join(repo)),
        None => Ok(ws_dir(workspace)?.join(".orbit")),
    }
}

/// Branch a workspace works on; the clone's checked-out branch for a repo scope.
pub fn scope_branch(workspace: &str) -> Result<String, String> {
    match repo_scope(workspace) {
        Some(repo) => git::current_branch(&clone_dir_of(repo)?)
            .ok_or_else(|| format!("{repo} is on a detached HEAD: switch to a branch first")),
        None => Ok(load_meta(&ws_dir(workspace)?)?.branch),
    }
}

/// Repos of a workspace; just the one for a repo scope.
pub fn scope_repos(workspace: &str) -> Result<Vec<String>, String> {
    match repo_scope(workspace) {
        Some(repo) => Ok(vec![repo.to_string()]),
        None => Ok(load_meta(&ws_dir(workspace)?)?.repos),
    }
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
    if repo_scope(name).is_some() {
        return Err("workspace names can't start with '@' (reserved for repository views)".into());
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

// ---------- Workspace from pull requests (Code Review → Check out) ----------

/// One PR to check out: a feature can span repos, one PR each.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrCheckout {
    pub repo: String,
    pub number: u64,
    pub branch: String,
    pub url: String,
}

/// What checking out a PR will do in its repo, shown before confirming.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrCheckoutPlan {
    pub repo: String,
    /// The repo is configured in Orbit.
    pub known: bool,
    /// Already cloned (otherwise it's cloned first).
    pub cloned: bool,
    /// The branch is on origin; false = a fork's PR (read-only `pr-<n>` branch).
    pub on_origin: bool,
    /// Branch-only repo: checked out in the clone itself, which must be clean.
    pub branch_only: bool,
    pub dirty: bool,
}

pub fn plan_pr_checkout(prs: &[PrCheckout]) -> Result<Vec<PrCheckoutPlan>, String> {
    let cfg = Config::load()?;
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = prs
            .iter()
            .map(|pr| {
                let svc = cfg.services.iter().find(|s| s.name == pr.repo);
                scope.spawn(move || {
                    let dir = svc.and_then(|s| clone_dir(s).ok());
                    let cloned = dir.as_deref().is_some_and(is_cloned);
                    PrCheckoutPlan {
                        repo: pr.repo.clone(),
                        known: svc.is_some(),
                        cloned,
                        // Uncloned repos can't be asked yet; forks are rare.
                        on_origin: !cloned || git::remote_has_branch(dir.as_deref().unwrap(), &pr.branch),
                        branch_only: svc.is_some_and(|s| !s.worktree),
                        dirty: cloned && svc.is_some_and(|s| !s.worktree) && git::is_dirty(dir.as_deref().unwrap()),
                    }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    }))
}

/// Creates a workspace on the branches of `prs` (one per repo): tracking
/// origin's branch, so fixes can be pushed to the PR, or on a read-only
/// `pr-<n>` branch for a fork's PR. Clones missing repos. Per-repo problems
/// are returned without aborting the others; the workspace remembers the PRs.
pub fn create_from_prs(name: &str, base: &str, prs: &[PrCheckout]) -> Result<(Workspace, Vec<String>), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("workspace name is required".into());
    }
    if repo_scope(name).is_some() {
        return Err("workspace names can't start with '@' (reserved for repository views)".into());
    }
    if prs.is_empty() {
        return Err("no pull request to check out".into());
    }
    let ws_dir = ws_dir(name)?;
    if ws_dir.exists() {
        return Err(format!("workspace '{name}' already exists"));
    }
    let cfg = Config::load()?;

    type Outcome = Result<(String, String, Option<String>), String>; // (repo, local branch, note)
    let results: Vec<Outcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = prs
            .iter()
            .map(|pr| {
                let svc = cfg.services.iter().find(|s| s.name == pr.repo);
                let wt = ws_dir.join(&pr.repo);
                scope.spawn(move || -> Outcome {
                    let svc = svc.ok_or_else(|| format!("{}: not a repository in Orbit", pr.repo))?;
                    let clone = clone_dir(svc)?;
                    if !is_cloned(&clone) {
                        git::clone(&svc.repo, &clone)?;
                    }
                    git::fetch(&clone)?;
                    let on_origin = git::remote_has_branch(&clone, &pr.branch);
                    let target = if svc.worktree { Some(wt.as_path()) } else { None };
                    if !svc.worktree && git::is_dirty(&clone) {
                        return Err(format!("{} has uncommitted changes in {}", svc.name, clone.display()));
                    }
                    let (local, note) = if on_origin {
                        let kept = git::checkout_tracking(&clone, &pr.branch, target)?;
                        (pr.branch.clone(), kept.map(|k| format!("{}: {k}", svc.name)))
                    } else {
                        let local = git::checkout_pr_head(&clone, pr.number, target)?;
                        let note = format!("{}: #{} comes from a fork, checked out as {local} (no branch to push to)", svc.name, pr.number);
                        (local, Some(note))
                    };
                    if svc.worktree {
                        links::link_shared(svc, &clone, &wt)?;
                        links::copy_worktreeinclude(&clone, &wt);
                    } else {
                        links::link_dir(&clone, &wt)?;
                    }
                    Ok((svc.name.clone(), local, note))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("checkout thread panicked".into()))).collect()
    });

    let mut ready: Vec<(String, String)> = Vec::new();
    let mut notes = Vec::new();
    for (pr, r) in prs.iter().zip(results) {
        match r {
            Ok((repo, local, note)) => {
                ready.push((repo, local));
                notes.extend(note);
            }
            Err(e) => notes.push(if e.starts_with(&pr.repo) { e } else { format!("{}: {e}", pr.repo) }),
        }
    }
    if ready.is_empty() {
        let _ = std::fs::remove_dir_all(&ws_dir);
        return Err(notes.join("\n"));
    }
    let branch = ready[0].1.clone();
    let repos = ready.iter().map(|(r, _)| r.clone()).collect();
    let mut ws = finish_create(name, &branch, base, repos, None)?;
    ws.repo_branches = ready.iter().filter(|(_, b)| *b != branch).cloned().collect();
    if !ws.repo_branches.is_empty() {
        save_meta(&ws_dir, &ws)?;
    }
    let refs: Vec<PrRef> = prs
        .iter()
        .filter(|p| ready.iter().any(|(r, _)| r == &p.repo))
        .map(|p| PrRef { repo: p.repo.clone(), number: p.number, url: p.url.clone() })
        .collect();
    save_pr_refs(name, &refs)?;
    Ok((load_meta(&ws_dir)?, notes))
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
        repo_branches: BTreeMap::new(),
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
    let cfg = Config::load().ok();
    // Read only when some repo is off its branch: who holds it.
    let mut others: Option<Vec<Workspace>> = None;

    let statuses: Vec<RepoStatus> = ws
        .repos
        .iter()
        .map(|repo| {
            let wt = ws_dir.join(repo);
            let expected_branch = ws.branch_of(repo).to_string();
            if !wt.exists() {
                return RepoStatus { repo: repo.clone(), expected_branch, ..Default::default() };
            }
            let (ahead, behind) = git::ahead_behind(&wt, &ws.base);
            let pr_commits = git::remote_branch_feature_commits(&wt, &ws.base);
            let branch = git::current_branch(&wt);
            let operation = git::operation(&wt);
            // A paused rebase detaches HEAD; that's not being off-branch.
            let off_branch = operation.is_none() && branch.as_deref() != Some(expected_branch.as_str());
            let branch_only = links::is_link(&wt);
            let held_by = match (&branch, off_branch && branch_only) {
                (Some(b), true) => others
                    .get_or_insert_with(|| list().unwrap_or_default())
                    .iter()
                    .find(|o| o.name != ws.name && o.repos.contains(repo) && o.branch_of(repo) == b)
                    .map(|o| o.name.clone()),
                _ => None,
            };
            RepoStatus {
                repo: repo.clone(),
                dirty: git::is_dirty(&wt),
                ahead,
                behind,
                pr_commits,
                integrated: ahead > 0 && git::is_integrated(&wt, "HEAD", &ws.base),
                off_branch,
                branch_only,
                auto_switch: branch_only
                    && cfg.as_ref().is_some_and(|c| c.services.iter().any(|s| &s.name == repo && s.auto_switch)),
                held_by,
                operation,
                branch,
                expected_branch,
            }
        })
        .collect();
    Ok(statuses)
}

/// Result of putting a repo back on its workspace branch.
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct BranchSync {
    /// The branch the repo is on now.
    pub branch: String,
    /// Nothing done: uncommitted work is in the way and `stash` wasn't set.
    pub dirty: bool,
    /// Uncommitted work was shelved under the branch it was left on.
    pub stashed: bool,
    /// Work shelved when this branch was last left came back.
    pub restored: bool,
    /// Something to tell the user (e.g. shelved work that didn't re-apply).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Switches `repo` back to the workspace's branch. Uncommitted work blocks
/// it unless `stash`: then it is shelved under the branch it was on, and
/// whatever Orbit shelved when last leaving the target branch comes back.
pub fn sync_branch(workspace: &str, repo: &str, stash: bool) -> Result<BranchSync, String> {
    let ws = load_meta(&ws_dir(workspace)?)?;
    if !ws.repos.iter().any(|r| r == repo) {
        return Err(format!("'{repo}' is not part of '{workspace}'"));
    }
    let expected = ws.branch_of(repo).to_string();
    let dir = repo_path(workspace, repo)?;
    if !dir.exists() {
        return Err(format!("{repo} is missing from '{workspace}'"));
    }
    if let Some(op) = git::operation(&dir) {
        return Err(format!("{repo} has a {op} in progress; finish or abort it before switching branches"));
    }
    let current = git::current_branch(&dir);
    let mut out = BranchSync { branch: expected.clone(), ..Default::default() };
    if current.as_deref() == Some(expected.as_str()) {
        return Ok(out);
    }
    if git::is_dirty(&dir) {
        if !stash {
            out.branch = current.unwrap_or_default();
            out.dirty = true;
            return Ok(out);
        }
        let left = current.clone().unwrap_or_else(|| "detached HEAD".into());
        git::autostash(&dir, &left)?;
        out.stashed = true;
    }
    // Recreates the branch from the base if it was deleted meanwhile.
    git::checkout_branch_in_place(&dir, &expected, &ws.base)?;
    match git::autostash_restore(&dir, &expected) {
        Ok(restored) => out.restored = restored,
        Err(e) => {
            out.note = Some(format!("{repo}: the work shelved on {expected} didn't re-apply cleanly ({e}); it's still in the stash"))
        }
    }
    Ok(out)
}

/// Errors when `repo` isn't on its workspace branch, so commit, push,
/// rebase and PR never act on another workspace's branch. Repository views
/// (`@repo`) have no workspace branch and pass.
pub fn ensure_on_branch(workspace: &str, repo: &str) -> Result<(), String> {
    if repo_scope(workspace).is_some() {
        return Ok(());
    }
    let ws = load_meta(&ws_dir(workspace)?)?;
    let expected = ws.branch_of(repo);
    match git::current_branch(&repo_path(workspace, repo)?) {
        Some(b) if b == expected => Ok(()),
        current => Err(format!(
            "{repo} is on {}, not this workspace's branch '{expected}'. Switch it back from the workspace page first.",
            current.map(|b| format!("'{b}'")).unwrap_or_else(|| "a detached HEAD".into())
        )),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Tests that point ORBIT_* env vars at a temp root must not overlap.
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn checks_out_prs_from_origin_and_forks() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-pr-checkout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")] {
            std::env::set_var(k, v);
        }
        let bare = origin(&root);
        // A fork's PR: its commit is only reachable from GitHub's pull ref.
        let seed = root.join("seed");
        git(&seed, &["checkout", "-b", "fork-work"]);
        std::fs::write(seed.join("fork.txt"), "from a fork").unwrap();
        git(&seed, &["add", "fork.txt"]);
        git(&seed, &["commit", "-m", "fork change"]);
        git(&seed, &["push", "origin", "fork-work:refs/pull/7/head"]);
        let svc = crate::config::Service::new("web".into(), bare.to_string_lossy().into());
        Config { services: vec![svc], ..Default::default() }.save().unwrap();

        let same_repo = PrCheckout { repo: "web".into(), number: 3, branch: "feat/existing".into(), url: "u3".into() };
        let fork = PrCheckout { repo: "web".into(), number: 7, branch: "fork-work".into(), url: "u7".into() };

        // Not cloned yet: the plan can't tell a fork, the checkout clones.
        assert!(!plan_pr_checkout(std::slice::from_ref(&same_repo)).unwrap()[0].cloned);
        let (ws, notes) = create_from_prs("review-3", "main", std::slice::from_ref(&same_repo)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!((ws.branch.as_str(), ws.pr_refs.len()), ("feat/existing", 1));
        let wt = ws_dir("review-3").unwrap().join("web");
        assert_eq!(git::current_branch(&wt).as_deref(), Some("feat/existing"));
        let upstream = crate::proc::run("git", &["rev-parse", "--abbrev-ref", "@{u}"], Some(&wt)).unwrap();
        assert_eq!(upstream.trim(), "origin/feat/existing", "pushes go to the PR");

        let plan = plan_pr_checkout(&[same_repo.clone(), fork.clone()]).unwrap();
        assert_eq!(plan.iter().map(|p| p.on_origin).collect::<Vec<_>>(), [true, false]);

        let (ws7, notes7) = create_from_prs("review-7", "main", std::slice::from_ref(&fork)).unwrap();
        assert_eq!(ws7.branch, "pr-7");
        assert!(ws_dir("review-7").unwrap().join("web/fork.txt").exists());
        assert!(notes7[0].contains("fork"), "{notes7:?}");

        // A local commit on the PR branch survives removing and checking out again.
        std::fs::write(wt.join("mine.txt"), "x").unwrap();
        git(&wt, &["add", "mine.txt"]);
        git(&wt, &["commit", "-m", "local fix"]);
        remove("review-3", false).unwrap();
        let (_, again) = create_from_prs("review-3b", "main", &[same_repo]).unwrap();
        assert!(again.iter().any(|n| n.contains("kept 1 local commit")), "{again:?}");
        assert!(ws_dir("review-3b").unwrap().join("web/mine.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn branch_only_repo_is_flagged_off_branch_and_syncs_back() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-branch-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")] {
            std::env::set_var(k, v);
        }
        let bare = origin(&root);
        let mut svc = crate::config::Service::new("web".into(), bare.to_string_lossy().into());
        svc.worktree = false;
        Config { services: vec![svc], ..Default::default() }.save().unwrap();

        // Both workspaces share the clone; the last one created holds it.
        create("a", "feat/a", "main", &["web".into()], None).unwrap();
        create("b", "feat/b", "main", &["web".into()], None).unwrap();
        let st = &status("a").unwrap()[0];
        assert!(st.off_branch && st.branch_only, "{st:?}");
        assert_eq!((st.branch.as_deref(), st.expected_branch.as_str()), (Some("feat/b"), "feat/a"));
        assert_eq!(st.held_by.as_deref(), Some("b"));
        assert!(!status("b").unwrap()[0].off_branch);
        assert!(ensure_on_branch("a", "web").unwrap_err().contains("feat/b"));
        assert!(ensure_on_branch("b", "web").is_ok());

        // b's uncommitted work blocks the switch unless it's shelved.
        let clone = clone_dir_of("web").unwrap();
        std::fs::write(clone.join("wip.txt"), "b's work").unwrap();
        let r = sync_branch("a", "web", false).unwrap();
        assert!(r.dirty && !r.stashed, "{r:?}");
        assert_eq!(git::current_branch(&clone).as_deref(), Some("feat/b"));
        let r = sync_branch("a", "web", true).unwrap();
        assert!(r.stashed && !r.restored, "{r:?}");
        assert_eq!(git::current_branch(&clone).as_deref(), Some("feat/a"));
        assert!(!clone.join("wip.txt").exists());
        assert!(!status("a").unwrap()[0].off_branch);

        // Back on b, its shelved work returns.
        let r = sync_branch("b", "web", false).unwrap();
        assert!(!r.dirty && r.restored, "{r:?}");
        assert_eq!(std::fs::read_to_string(clone.join("wip.txt")).unwrap(), "b's work");
        let _ = std::fs::remove_dir_all(&root);
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
            off_branch: true,
            ..Default::default()
        };
        let v = serde_json::to_value(&st).unwrap();
        assert!(v.get("prCommits").is_some(), "must serialize prCommits: {v}");
        assert!(v.get("offBranch").is_some(), "must serialize offBranch: {v}");
        // round-trip must also accept camelCase on the way back (if ever)
        let back: RepoStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back.pr_commits, 1);
    }
}
