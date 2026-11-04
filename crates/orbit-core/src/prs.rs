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
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PrSpec {
    pub repo: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
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

/// Opens a PR for one repo's branch into `base`. Does not save it.
pub fn create_for_repo(workspace: &str, repo: &str, base: &str, title: &str, body: &str) -> Result<PrRef, String> {
    let dir = worktree_on_branch(workspace, repo)?;
    let owner_repo = remote_of(&dir).ok_or_else(|| format!("{repo}: cannot resolve origin remote"))?;
    let branch = git::current_branch(&dir).ok_or_else(|| format!("{repo}: detached HEAD"))?;
    let base = git::base_branch(&dir, base);
    let out = crate::proc::cmd("gh")
        .args(["pr", "create", "-R", &owner_repo, "--base", &base, "--head", &branch, "--title", title, "--body", body])
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

/// Opens one repo's PR and saves it in the workspace meta. Returns its URL.
pub fn create(workspace: &str, repo: &str, base: &str, title: &str, body: &str) -> Result<String, String> {
    let r = create_for_repo(workspace, repo, base, title, body)?;
    let url = r.url.clone();
    workspace::save_pr_refs(workspace, &[r])?;
    Ok(url)
}

/// Opens PRs per spec, in parallel, and returns every open PR of the
/// workspace (existing and new). Repos with nothing to PR are skipped;
/// other failures are reported together, and only when no PR exists.
pub fn create_all(workspace: &str, base: &str, specs: &[PrSpec]) -> Result<Vec<PullRequest>, String> {
    let created: Vec<Result<PrRef, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = specs
            .iter()
            .map(|spec| scope.spawn(move || create_for_repo(workspace, &spec.repo, base, &spec.title, &spec.body)))
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
    // A repo view has no meta: it finds its PRs by the clone's branch.
    if !refs.is_empty() && workspace::repo_scope(workspace).is_none() {
        workspace::save_pr_refs(workspace, &refs)?;
    }
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

        let spec = |repo: &str| PrSpec { repo: repo.into(), title: "Add x".into(), body: "why".into() };
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
}
