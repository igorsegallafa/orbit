// A workspace's pull requests: creating them per repo, finding the open
// ones, their live status, CI checks and failed-check logs. Everything goes
// through the `gh` login; the parsing is split out so it can be tested
// without GitHub.
use crate::git;
use crate::github::{self, PrCheck, PullRequest, WsPrStatus};
use crate::workspace::{self, PrRef};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Folder of `repo` in `workspace`, which must exist.
pub fn worktree_of(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let dir = workspace::repo_path(workspace, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found in '{workspace}'"));
    }
    Ok(dir)
}

/// `worktree_of`, for actions that write to the branch (commit, push,
/// pull, rebase, PR): refuses a repo that isn't on the workspace's branch.
pub fn worktree_on_branch(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let dir = worktree_of(workspace, repo)?;
    workspace::ensure_on_branch(workspace, repo)?;
    Ok(dir)
}

fn origin_url(dir: &Path) -> Option<String> {
    let out = crate::proc::cmd("git").args(["remote", "get-url", "origin"]).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `owner/repo` of a checkout's GitHub origin.
pub fn remote_of(dir: &Path) -> Option<String> {
    github::parse_owner_repo(&origin_url(dir)?).ok()
}

/// `owner/repo` of a workspace repo.
pub fn owner_repo(workspace: &str, repo: &str) -> Result<String, String> {
    remote_of(&worktree_of(workspace, repo)?).ok_or_else(|| format!("{repo}: origin is not a GitHub repository"))
}

// ---------- create ----------

/// Per-repo PR content as decided in the PR modal.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct PrSpec {
    pub repo: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Opens it as a draft PR (`gh pr create --draft`).
    #[serde(default)]
    pub draft: bool,
}

/// A readable PR title from a branch: `feat/add-items-skills` → "Add items skills".
pub fn title_from_branch(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch).replace(['-', '_'], " ");
    let words: Vec<&str> = last.split_whitespace().collect();
    let joined = words.join(" ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What opening a repo's PR would do.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlanState {
    /// The pushed branch has commits the base lacks.
    Ready,
    /// Has commits origin lacks: they are pushed before the PR is opened.
    NeedsPush,
    /// Nothing the base doesn't already have.
    NoCommits,
    /// A PR is already open for the branch.
    HasPr,
    /// Can't open one (see `reason`).
    Blocked,
}

/// An open PR found for a repo's branch.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenPr {
    pub number: u64,
    pub url: String,
    pub is_draft: bool,
}

/// One repo of the Create PRs modal, checked before anything is sent.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrPlan {
    pub repo: String,
    pub branch: String,
    pub base: String,
    /// Commits the PR would carry.
    pub commits: usize,
    /// Of those, the ones origin doesn't have yet.
    pub unpushed: usize,
    /// origin has the branch.
    pub pushed: bool,
    pub state: PlanState,
    /// Why it is not Ready, for the row's hint.
    pub reason: String,
    pub pr: Option<OpenPr>,
}

/// State and reason from a branch's facts, once the repo is known to be on
/// its branch with a GitHub origin and no open PR.
pub fn classify(f: &git::BranchFacts, branch: &str, base: &str) -> (PlanState, String) {
    if f.local == 0 && f.remote == 0 {
        return (PlanState::NoCommits, format!("No commits ahead of {base}"));
    }
    if f.integrated {
        return (PlanState::NoCommits, format!("Already in {base}"));
    }
    if f.unpushed > 0 && f.behind > 0 {
        return (PlanState::Blocked, format!("Diverged from origin/{branch}: pull or force-push first"));
    }
    if f.unpushed > 0 {
        let what = if f.pushed { format!("{} new commit{}", f.unpushed, if f.unpushed == 1 { "" } else { "s" }) } else { "the branch".into() };
        return (PlanState::NeedsPush, format!("Pushes {what} to origin first"));
    }
    (PlanState::Ready, String::new())
}

/// (repo, folder, branch) of every repo the scope can open PRs for.
fn targets_of(workspace: &str) -> Result<Vec<(String, PathBuf, String)>, String> {
    Ok(match workspace::repo_scope(workspace) {
        Some(repo) => {
            let dir = workspace::clone_dir_of(repo)?;
            let branch = git::current_branch(&dir).unwrap_or_default();
            vec![(repo.to_string(), dir, branch)]
        }
        None => {
            let ws_dir = workspace::ws_dir(workspace)?;
            let meta = workspace::load_meta(&ws_dir)?;
            meta.repos.iter().map(|r| (r.clone(), ws_dir.join(r), meta.branch_of(r).to_string())).collect()
        }
    })
}

fn plan_repo(repo: &str, dir: &Path, branch: &str, base: &str) -> PrPlan {
    let mut p = PrPlan {
        repo: repo.to_string(),
        branch: branch.to_string(),
        base: base.to_string(),
        commits: 0,
        unpushed: 0,
        pushed: false,
        state: PlanState::Blocked,
        reason: String::new(),
        pr: None,
    };
    if !dir.exists() {
        p.reason = "Worktree not found".into();
        return p;
    }
    p.base = git::base_branch(dir, base);
    match git::current_branch(dir) {
        Some(b) if b == branch => {}
        other => {
            p.reason = format!("On {}, not {branch}", other.map(|b| format!("'{b}'")).unwrap_or_else(|| "a detached HEAD".into()));
            return p;
        }
    }
    let Some(owner_repo) = remote_of(dir) else {
        p.reason = "origin is not a GitHub repository".into();
        return p;
    };
    let facts = git::branch_facts(dir, base);
    p.commits = facts.local.max(facts.remote);
    p.unpushed = facts.unpushed;
    p.pushed = facts.pushed;
    if facts.pushed {
        let open = github::prs_for_branch(repo, &owner_repo, branch).unwrap_or_default();
        if let Some(pr) = open.into_iter().next() {
            p.state = PlanState::HasPr;
            p.reason = format!("PR #{} is already open", pr.number);
            p.pr = Some(OpenPr { number: pr.number, url: pr.url, is_draft: pr.is_draft });
            return p;
        }
    }
    (p.state, p.reason) = classify(&facts, branch, &p.base);
    p
}

/// Checks every repo of the scope in parallel: commits ahead of `base`,
/// pushed or not, PR already open.
pub fn plan(workspace: &str, base: &str) -> Result<Vec<PrPlan>, String> {
    let targets = targets_of(workspace)?;
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> =
            targets.iter().map(|(repo, dir, branch)| scope.spawn(move || plan_repo(repo, dir, branch, base))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    }))
}

/// "https://github.com/owner/repo/pull/123" → 123
pub fn pr_number_from_url(url: &str) -> Option<u64> {
    url.trim().trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// gh's answers for a repo with nothing to open a PR for (or one already open).
pub fn is_benign_create_error(e: &str) -> bool {
    e.contains("No commits between")
        || e.contains("already exists")
        || e.contains("diff between the head and base")
        || e.contains("nothing to compare")
}

/// `gh pr create` arguments for a spec.
fn create_args<'a>(owner_repo: &'a str, base: &'a str, branch: &'a str, spec: &'a PrSpec) -> Vec<&'a str> {
    let mut args = vec!["pr", "create", "-R", owner_repo, "--base", base, "--head", branch, "--title", &spec.title, "--body", &spec.body];
    if spec.draft {
        args.push("--draft");
    }
    args
}

/// Opens a PR for one repo's branch into `base`, pushing the branch first
/// when origin lacks some of its commits. Does not save it.
pub fn create_for_repo(workspace: &str, base: &str, spec: &PrSpec) -> Result<PrRef, String> {
    let repo = spec.repo.as_str();
    let dir = worktree_on_branch(workspace, repo)?;
    let owner_repo = remote_of(&dir).ok_or_else(|| format!("{repo}: cannot resolve origin remote"))?;
    let branch = git::current_branch(&dir).ok_or_else(|| format!("{repo}: detached HEAD"))?;
    if git::branch_facts(&dir, base).unpushed > 0 {
        git::push(&dir, &branch, false).map_err(|e| format!("{repo}: push failed: {e}"))?;
    }
    let base = git::base_branch(&dir, base);
    let out = crate::proc::cmd("gh")
        .args(create_args(&owner_repo, &base, &branch, spec))
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !out.status.success() {
        return Err(format!("{repo}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let number = pr_number_from_url(&url).ok_or_else(|| format!("{repo}: cannot parse PR number from '{url}'"))?;
    Ok(PrRef { repo: repo.to_string(), number, url })
}

/// Serializes meta writes of PRs created side by side.
static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn save_refs(workspace: &str, refs: &[PrRef]) -> Result<(), String> {
    // A repo view has no meta: it finds its PRs by the clone's branch.
    if refs.is_empty() || workspace::repo_scope(workspace).is_some() {
        return Ok(());
    }
    let _g = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    workspace::save_pr_refs(workspace, refs)
}

/// Opens one repo's PR and saves it in the workspace meta. Safe to run for
/// several repos at once (the modal shows each one's progress).
pub fn create(workspace: &str, base: &str, spec: &PrSpec) -> Result<PrRef, String> {
    let r = create_for_repo(workspace, base, spec)?;
    save_refs(workspace, std::slice::from_ref(&r))?;
    Ok(r)
}

/// Opens PRs per spec, in parallel, and returns every open PR of the
/// workspace (existing and new). Repos with nothing to PR are skipped;
/// other failures are reported together, and only when no PR exists.
pub fn create_all(workspace: &str, base: &str, specs: &[PrSpec]) -> Result<Vec<PullRequest>, String> {
    let created: Vec<Result<PrRef, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = specs
            .iter()
            .map(|spec| scope.spawn(move || create_for_repo(workspace, base, spec)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("thread panicked".into()))).collect()
    });
    let mut errors = Vec::new();
    let mut refs = Vec::new();
    for r in created {
        match r {
            Ok(r) => refs.push(r),
            Err(e) if !is_benign_create_error(&e) => errors.push(e),
            Err(_) => {}
        }
    }
    save_refs(workspace, &refs)?;
    let prs = list(workspace)?;
    if prs.is_empty() && !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(prs)
}

// ---------- read ----------

/// Open PRs of the workspace's branch across its repos, in parallel.
pub fn list(workspace: &str) -> Result<Vec<PullRequest>, String> {
    // (repo, folder, branch) to look up; a repo view uses the clone's branch.
    let targets: Vec<(String, PathBuf, String)> = match workspace::repo_scope(workspace) {
        Some(repo) => {
            let dir = workspace::clone_dir_of(repo)?;
            let branch = git::current_branch(&dir).unwrap_or_default();
            vec![(repo.to_string(), dir, branch)]
        }
        None => {
            let ws_dir = workspace::ws_dir(workspace)?;
            let meta = workspace::load_meta(&ws_dir)?;
            meta.repos.iter().map(|r| (r.clone(), ws_dir.join(r), meta.branch.clone())).collect()
        }
    };
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(repo, dir, branch)| {
                scope.spawn(move || {
                    let Some(owner_repo) = dir.exists().then(|| remote_of(dir)).flatten() else { return vec![] };
                    github::prs_for_branch(repo, &owner_repo, branch).unwrap_or_default()
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    }))
}

/// Live state of the workspace's PRs. The saved refs are the source; with
/// none saved the branch is scanned, and what it finds is saved.
pub fn status(workspace: &str) -> Result<Vec<WsPrStatus>, String> {
    let ws_dir = workspace::ws_dir(workspace)?;
    let meta = workspace::load_meta(&ws_dir)?;
    let scan = meta.pr_refs.is_empty();
    let rows: Vec<WsPrStatus> = std::thread::scope(|scope| {
        let handles: Vec<_> = if scan {
            meta.repos
                .iter()
                .map(|repo| {
                    let (branch, dir) = (meta.branch.clone(), ws_dir.join(repo));
                    scope.spawn(move || {
                        let Some(owner_repo) = dir.exists().then(|| remote_of(&dir)).flatten() else { return vec![] };
                        github::pr_status_for_branch(&owner_repo, &branch).unwrap_or_default()
                    })
                })
                .collect()
        } else {
            meta.pr_refs
                .iter()
                .map(|r| {
                    let (dir, pr) = (ws_dir.join(&r.repo), r.clone());
                    scope.spawn(move || {
                        let Some(owner_repo) = remote_of(&dir) else { return vec![] };
                        github::pr_status_by_number(&owner_repo, &pr).unwrap_or_default()
                    })
                })
                .collect()
        };
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    });
    if scan && !rows.is_empty() {
        let refs: Vec<PrRef> = rows.iter().map(|r| PrRef { repo: r.repo.clone(), number: r.number, url: r.url.clone() }).collect();
        let _ = workspace::save_pr_refs(workspace, &refs);
    }
    Ok(rows)
}

// ---------- checks ----------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WsCheck {
    pub repo: String,
    pub pr_number: u64,
    pub checks: Vec<PrCheck>,
    /// Aggregated bucket for the row indicator: pass | fail | running | none
    pub status: String,
}

/// One indicator for a PR's checks: pass | fail | running | none.
pub fn aggregate(checks: &[PrCheck]) -> String {
    if checks.is_empty() {
        return "none".into();
    }
    if checks.iter().any(|c| matches!(c.bucket.as_str(), "pending" | "queued")) {
        return "running".into();
    }
    if checks.iter().any(|c| c.bucket == "fail") {
        return "fail".into();
    }
    if checks.iter().all(|c| c.bucket == "pass" || c.bucket == "skipping") {
        "pass".into()
    } else {
        "none".into()
    }
}

fn check_row(repo: String, owner_repo: Option<String>, number: u64) -> WsCheck {
    let checks = owner_repo.and_then(|or| github::pr_checks(&or, number).ok()).unwrap_or_default();
    let status = aggregate(&checks);
    WsCheck { repo, pr_number: number, checks, status }
}

/// CI checks of every saved PR of the workspace, in parallel.
pub fn checks(workspace: &str) -> Result<Vec<WsCheck>, String> {
    let ws_dir = workspace::ws_dir(workspace)?;
    let meta = workspace::load_meta(&ws_dir)?;
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = meta
            .pr_refs
            .iter()
            .map(|r| {
                let (dir, pr) = (ws_dir.join(&r.repo), r.clone());
                scope.spawn(move || check_row(pr.repo, remote_of(&dir), pr.number))
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    }))
}

/// A PR addressed on GitHub alone (no local worktree).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemotePr {
    pub repo: String,
    pub owner_repo: String,
    pub number: u64,
}

/// CI checks of PRs straight from GitHub, in parallel.
pub fn group_checks(prs: Vec<RemotePr>) -> Vec<WsCheck> {
    std::thread::scope(|scope| {
        let handles: Vec<_> =
            prs.into_iter().map(|pr| scope.spawn(move || check_row(pr.repo, Some(pr.owner_repo), pr.number))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

/// (owner/repo, run id, job id) of a check's Actions job link.
fn job_of(link: &str) -> Result<(String, u64, u64), String> {
    let run_id = github::run_id_from_link(link).ok_or("check has no GitHub Actions run")?;
    let owner_repo = github::owner_repo_from_link(link).ok_or("cannot derive repository from link")?;
    let job_id = link.rsplit('/').next().and_then(|s| s.parse::<u64>().ok()).ok_or("cannot parse job id from link")?;
    Ok((owner_repo, run_id, job_id))
}

// ---------- push / merge ----------

/// Pushes a workspace repo's branch to origin; `force` uses `--force-with-lease`.
pub fn push(workspace: &str, repo: &str, force: bool) -> Result<(), String> {
    git::push_current(&worktree_on_branch(workspace, repo)?, repo, force)
}

/// Squash-merges one of the workspace's PRs on GitHub (its remote branch is
/// deleted; the worktree stays until the workspace is removed).
pub fn merge(workspace: &str, repo: &str, number: u64) -> Result<(), String> {
    github::squash_merge(&owner_repo(workspace, repo)?, number).map_err(|e| format!("{repo} #{number}: {e}"))
}

/// Squash-merges a PR addressed on GitHub alone (never checked out).
pub fn merge_remote(owner_repo: &str, number: u64) -> Result<(), String> {
    github::squash_merge(owner_repo, number).map_err(|e| format!("{owner_repo} #{number}: {e}"))
}

/// Re-runs the failed jobs of the workflow run behind `link`.
pub fn rerun(link: &str) -> Result<(), String> {
    let run_id = github::run_id_from_link(link).ok_or("check has no GitHub Actions run to re-run")?;
    let owner_repo = github::owner_repo_from_link(link).ok_or("cannot derive repository from link")?;
    crate::proc::run("gh", &["api", "-X", "POST", &format!("repos/{owner_repo}/actions/runs/{run_id}/rerun-failed-jobs")], None)
        .map(|_| ())
}

/// Lines of a failed log kept for an AI prompt (full logs blow its budget).
pub const LOG_TAIL: usize = 200;

/// The log's last `LOG_TAIL` lines, after the failed step's name when known.
pub fn bound_log(log: &str, failed_step: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let tail = lines[lines.len().saturating_sub(LOG_TAIL)..].join("\n");
    if failed_step.is_empty() {
        tail
    } else {
        format!("failed step: {failed_step}\n{tail}")
    }
}

/// Bounded failure log of the job behind `link`.
pub fn check_logs(link: &str) -> Result<String, String> {
    let (owner_repo, run_id, job_id) = job_of(link)?;
    let failed_step = crate::proc::run(
        "gh",
        &["api", &format!("repos/{owner_repo}/actions/jobs/{job_id}"), "--jq", ".steps[] | select(.conclusion == \"failure\") | .name"],
        None,
    )
    .unwrap_or_default();
    let log = crate::proc::run("gh", &["run", "view", &run_id.to_string(), "-R", &owner_repo, "--log-failed"], None)?;
    Ok(bound_log(&log, failed_step.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::workspace::tests::ENV_LOCK;

    fn check(bucket: &str) -> PrCheck {
        PrCheck {
            name: "ci".into(),
            state: String::new(),
            bucket: bucket.into(),
            workflow: String::new(),
            link: String::new(),
            started_at: String::new(),
            completed_at: String::new(),
        }
    }

    #[test]
    fn aggregates_checks_into_one_indicator() {
        assert_eq!(aggregate(&[]), "none");
        assert_eq!(aggregate(&[check("pass"), check("pending")]), "running");
        assert_eq!(aggregate(&[check("pass"), check("fail")]), "fail");
        assert_eq!(aggregate(&[check("pass"), check("skipping")]), "pass");
        assert_eq!(aggregate(&[check("pass"), check("cancel")]), "none");
    }

    #[test]
    fn parses_pr_numbers_and_benign_errors() {
        assert_eq!(pr_number_from_url("https://github.com/o/r/pull/123\n"), Some(123));
        assert_eq!(pr_number_from_url("https://github.com/o/r/pull/9/"), Some(9));
        assert_eq!(pr_number_from_url("nope"), None);
        assert!(is_benign_create_error("web: No commits between main and feat/x"));
        assert!(is_benign_create_error("a pull request for branch \"x\" already exists"));
        assert!(!is_benign_create_error("HTTP 401: Bad credentials"));
    }

    #[test]
    fn bounds_logs_to_their_tail_after_the_failed_step() {
        let log: String = (1..=250).map(|n| format!("line {n}\n")).collect();
        let out = bound_log(&log, "Run tests");
        assert!(out.starts_with("failed step: Run tests\nline 51\n"));
        assert!(out.ends_with("line 250"));
        assert_eq!(bound_log("a\nb", ""), "a\nb");
        assert_eq!(job_of("https://github.com/o/r/actions/runs/5/job/6").unwrap(), ("o/r".into(), 5, 6));
        assert!(job_of("https://app.aikido.dev/x").is_err());
    }

    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"];
        full.extend_from_slice(args);
        crate::proc::run("git", &full, Some(dir)).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    }

    /// A stub `gh` first on PATH: `pr create` answers per repo, `pr list` and
    /// `run view` print canned output, everything is logged to `gh.log`.
    fn stub_gh(bin: &Path) {
        std::fs::create_dir_all(bin).unwrap();
        let script = r#"#!/bin/sh
echo "$@" >> "$(dirname "$0")/gh.log"
case "$1 $2" in
  "pr create")
    case "$*" in
      *"-R o/empty"*) echo "pull request create failed: No commits between main and feat/x" >&2; exit 1 ;;
      *"-R o/broken"*) echo "HTTP 401: Bad credentials" >&2; exit 1 ;;
      *) echo "https://github.com/o/web/pull/42" ;;
    esac ;;
  "pr list")
    case "$*" in
      *"-R o/web"*) echo '[{"number":42,"title":"Add x","headRefName":"feat/x","baseRefName":"main","author":{"login":"me"},"isDraft":false,"url":"https://github.com/o/web/pull/42","updatedAt":"2026-01-01T00:00:00Z"}]' ;;
      *) echo '[]' ;;
    esac ;;
  "api repos/o/web/actions/jobs/6") echo "Run tests" ;;
  "run view") seq 1 300 | sed 's/^/log /' ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#;
        let gh = bin.join("gh");
        std::fs::write(&gh, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A workspace whose repos have GitHub-looking origins (never contacted:
    /// `gh` is a stub and nothing is pushed).
    #[cfg(unix)]
    #[test]
    fn creates_prs_lists_them_and_reads_check_logs_with_a_stub_gh() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-prs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        Config::default().save().unwrap();
        let ws = root.join("ws-root/workspaces/feat-x");
        for (repo, owner) in [("web", "o/web"), ("empty", "o/empty"), ("broken", "o/broken")] {
            let dir = ws.join(repo);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init"]);
            git(&dir, &["commit", "--allow-empty", "-m", "init"]);
            git(&dir, &["checkout", "-b", "feat/x"]);
            git(&dir, &["remote", "add", "origin", &format!("https://github.com/{owner}.git")]);
        }
        std::fs::write(ws.join(".workspace.yaml"), "name: feat-x\nbranch: feat/x\nbase: main\nrepos: [web, empty, broken]\n").unwrap();
        let bin = root.join("bin");
        stub_gh(&bin);
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::set_var("PATH", std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&path))).unwrap());

        let spec = |repo: &str| PrSpec { repo: repo.into(), title: "Add x".into(), body: "why".into(), draft: false };
        let result = create_all("feat-x", "main", &[spec("web"), spec("empty"), spec("broken")]);
        let checks_link = "https://github.com/o/web/actions/runs/5/job/6";
        let logs = check_logs(checks_link);
        let meta = workspace::load_meta(&ws);
        std::env::set_var("PATH", path);

        let prs = result.unwrap();
        assert_eq!(prs.iter().map(|p| (p.repo.as_str(), p.number)).collect::<Vec<_>>(), [("web", 42)]);
        let meta = meta.unwrap();
        assert_eq!(meta.pr_refs.iter().map(|r| (r.repo.as_str(), r.number)).collect::<Vec<_>>(), [("web", 42)]);
        let log = std::fs::read_to_string(bin.join("gh.log")).unwrap();
        assert!(log.contains("pr create -R o/web --base main --head feat/x --title Add x --body why"), "{log}");
        let logs = logs.unwrap();
        assert!(logs.starts_with("failed step: Run tests\nlog 101\n"), "{logs}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn derives_readable_titles_from_branches() {
        assert_eq!(title_from_branch("feat/add-items-skills"), "Add items skills");
        assert_eq!(title_from_branch("fix_login--flow"), "Fix login flow");
        assert_eq!(title_from_branch("users/me/ralph/x"), "X");
        assert_eq!(title_from_branch(""), "");
    }

    #[test]
    fn classifies_branches_and_passes_the_draft_flag() {
        use git::BranchFacts;
        let f = |local, remote, pushed, unpushed, behind, integrated| BranchFacts { local, remote, pushed, unpushed, behind, integrated };
        let state = |facts: BranchFacts| classify(&facts, "feat/x", "main");
        assert_eq!(state(f(0, 0, true, 0, 0, false)), (PlanState::NoCommits, "No commits ahead of main".into()));
        assert_eq!(state(f(2, 2, true, 0, 0, true)).0, PlanState::NoCommits);
        assert_eq!(state(f(2, 2, true, 0, 0, false)), (PlanState::Ready, String::new()));
        assert_eq!(state(f(0, 3, true, 0, 3, false)).0, PlanState::Ready);
        assert_eq!(state(f(3, 0, false, 3, 0, false)), (PlanState::NeedsPush, "Pushes the branch to origin first".into()));
        assert_eq!(state(f(3, 2, true, 1, 0, false)), (PlanState::NeedsPush, "Pushes 1 new commit to origin first".into()));
        assert_eq!(state(f(3, 3, true, 1, 1, false)).0, PlanState::Blocked);

        let spec = PrSpec { repo: "web".into(), title: "T".into(), body: "B".into(), draft: true };
        assert_eq!(create_args("o/web", "main", "feat/x", &spec).last(), Some(&"--draft"));
        assert!(!create_args("o/web", "main", "feat/x", &PrSpec { draft: false, ..spec }).contains(&"--draft"));
    }

    /// Repos pushing to local bare remotes (origin's fetch URL is GitHub's,
    /// never contacted) and a stub `gh`: the plan of each, then a draft PR
    /// that pushes its branch first.
    #[cfg(unix)]
    #[test]
    fn plans_repos_and_creates_a_draft_pr_after_pushing() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-prs-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        Config::default().save().unwrap();
        let ws = root.join("ws-root/workspaces/feat-x");
        let repos = ["ready", "local", "empty", "open", "off"];
        for repo in repos {
            let (dir, bare) = (ws.join(repo), root.join(format!("bare/{repo}.git")));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(&bare).unwrap();
            git(&bare, &["init", "--bare"]);
            git(&dir, &["init"]);
            git(&dir, &["commit", "--allow-empty", "-m", "init"]);
            git(&dir, &["remote", "add", "origin", &format!("https://github.com/o/{repo}.git")]);
            git(&dir, &["remote", "set-url", "--push", "origin", bare.to_str().unwrap()]);
            git(&dir, &["push", "origin", "main"]);
            git(&dir, &["checkout", "-b", if repo == "off" { "other" } else { "feat/x" }]);
            if repo != "empty" {
                std::fs::write(dir.join("work.txt"), repo).unwrap();
                git(&dir, &["add", "work.txt"]);
                git(&dir, &["commit", "-m", "work"]);
            }
            if matches!(repo, "ready" | "open") {
                git(&dir, &["push", "-u", "origin", "feat/x"]);
            }
        }
        std::fs::write(ws.join(".workspace.yaml"), "name: feat-x\nbranch: feat/x\nbase: main\nrepos: [ready, local, empty, open, off]\n").unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
echo "$@" >> "$(dirname "$0")/gh.log"
case "$1 $2 $4" in
  "pr list o/open") echo '[{"number":7,"title":"x","headRefName":"feat/x","baseRefName":"main","author":{"login":"me"},"isDraft":true,"url":"https://github.com/o/open/pull/7","updatedAt":"2026-01-01T00:00:00Z"}]' ;;
  "pr list "*) echo '[]' ;;
  "pr create "*) echo "https://github.com/$4/pull/9" ;;
  *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::set_var("PATH", std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&path))).unwrap());

        let plans = plan("feat-x", "main");
        let spec = PrSpec { repo: "local".into(), title: "Add x".into(), body: "why".into(), draft: true };
        let created = create("feat-x", "main", &spec);
        let after = plan("feat-x", "main");
        let meta = workspace::load_meta(&ws);
        std::env::set_var("PATH", path);

        let plans = plans.unwrap();
        let row = |plans: &[PrPlan], repo: &str| plans.iter().find(|p| p.repo == repo).cloned().unwrap();
        let ready = row(&plans, "ready");
        assert_eq!((ready.state, ready.commits, ready.pushed, ready.base.as_str()), (PlanState::Ready, 1, true, "main"));
        let local = row(&plans, "local");
        assert_eq!((local.state, local.commits, local.unpushed, local.pushed), (PlanState::NeedsPush, 1, 1, false));
        assert_eq!(row(&plans, "empty").state, PlanState::NoCommits);
        let open = row(&plans, "open");
        assert_eq!((open.state, open.pr), (PlanState::HasPr, Some(OpenPr { number: 7, url: "https://github.com/o/open/pull/7".into(), is_draft: true })));
        let off = row(&plans, "off");
        assert_eq!((off.state, off.reason.as_str()), (PlanState::Blocked, "On 'other', not feat/x"));

        assert_eq!(created.unwrap().number, 9);
        assert_eq!(row(&after.unwrap(), "local").state, PlanState::Ready);
        assert!(crate::proc::run("git", &["rev-parse", "--verify", "refs/heads/feat/x"], Some(&root.join("bare/local.git"))).is_ok());
        let log = std::fs::read_to_string(bin.join("gh.log")).unwrap();
        assert!(log.contains("pr create -R o/local --base main --head feat/x --title Add x --body why --draft"), "{log}");
        assert_eq!(meta.unwrap().pr_refs.iter().map(|r| (r.repo.as_str(), r.number)).collect::<Vec<_>>(), [("local", 9)]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
