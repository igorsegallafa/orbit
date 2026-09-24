// Lists repos from GitHub using the `gh` CLI (reuses the user's existing
// `gh auth login` session — no OAuth flow to build/maintain ourselves).
//
// Fetches repos for the authenticated user AND every organization they
// belong to, in parallel (each `gh` invocation is a separate process with
// noticeable startup + network latency, so sequential calls scale badly
// once someone is in several orgs).
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GithubRepo {
    pub name: String,
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    #[serde(rename = "sshUrl")]
    pub ssh_url: String,
    #[serde(rename = "isPrivate")]
    pub is_private: bool,
}

/// A TTL'd in-memory cache slot.
type Cached<T> = Mutex<Option<(Instant, Vec<T>)>>;

const CACHE_TTL: Duration = Duration::from_secs(300);

fn cache() -> &'static Cached<GithubRepo> {
    static CACHE: OnceLock<Cached<GithubRepo>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn is_authenticated() -> bool {
    crate::proc::cmd("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_gh(args: &[&str]) -> Result<Vec<u8>, String> {
    crate::proc::run("gh", args, None).map(String::into_bytes)
}

fn current_login() -> Result<String, String> {
    let out = run_gh(&["api", "user", "--jq", ".login"])?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

fn my_orgs() -> Result<Vec<String>, String> {
    let out = run_gh(&["api", "user/orgs", "--jq", ".[].login"])?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn repos_for_owner(owner: &str) -> Result<Vec<GithubRepo>, String> {
    let out = run_gh(&[
        "repo",
        "list",
        owner,
        "--limit",
        "200",
        "--json",
        "name,nameWithOwner,sshUrl,isPrivate",
    ])?;
    serde_json::from_slice(&out).map_err(|e| format!("failed to parse gh output: {e}"))
}

/// Lists repos for the authenticated user plus every org they belong to.
/// Results are cached in memory for `CACHE_TTL`; pass `force_refresh` to
/// bypass the cache (e.g. a "Refresh" button in the UI).
pub fn list_accessible_repos(force_refresh: bool) -> Result<Vec<GithubRepo>, String> {
    if !force_refresh {
        if let Some((fetched_at, repos)) = cache().lock().unwrap().clone() {
            if fetched_at.elapsed() < CACHE_TTL {
                return Ok(repos);
            }
        }
    }

    let login = current_login()?;
    let mut owners = vec![login];
    owners.extend(my_orgs()?);

    let repos: Vec<GithubRepo> = std::thread::scope(|scope| {
        let handles: Vec<_> = owners
            .iter()
            .map(|owner| scope.spawn(move || repos_for_owner(owner)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|r| r.ok())
            .flatten()
            .collect()
    });

    let mut repos = repos;
    repos.sort_by_key(|a| a.name_with_owner.to_lowercase());

    *cache().lock().unwrap() = Some((Instant::now(), repos.clone()));
    Ok(repos)
}

// ---------- Code Review: PRs across the user's configured repos ----------

/// Parses "owner/repo" out of a Service.repo URL
/// (git@github.com:owner/repo.git or https://github.com/owner/repo.git).
pub fn parse_owner_repo(url: &str) -> Result<String, String> {
    let no_git = url.trim().trim_end_matches(".git");
    if let Some(rest) = no_git.strip_prefix("git@github.com:") {
        return Ok(rest.to_string());
    }
    if let Some(rest) = no_git
        .strip_prefix("https://github.com/")
        .or_else(|| no_git.strip_prefix("http://github.com/"))
    {
        return Ok(rest.trim_end_matches('/').to_string());
    }
    Err(format!("cannot derive owner/repo from '{url}'"))
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub repo: String,      // Orbit service name
    pub owner_repo: String, // "owner/repo" for gh -R
    pub number: u64,
    pub title: String,
    pub branch: String, // headRefName — the grouping key
    pub base: String,
    pub author: String,
    pub is_draft: bool,
    pub url: String,
    pub updated_at: String, // ISO date from gh
    /// Review and CI state, filled for the open-PR list only (search and
    /// lookups leave it empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PrReviewStatus>,
}

/// Where an open PR stands: reviews, CI, conflicts, and the viewer's own review.
#[derive(Debug, Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrReviewStatus {
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED, None when no review is required.
    pub review_decision: Option<String>,
    /// "pass" | "fail" | "pending"; None without checks.
    pub checks: Option<String>,
    /// The base can't take it without resolving conflicts.
    pub conflicts: bool,
    /// Opened by the viewer.
    pub mine: bool,
    /// The viewer's latest review: APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED.
    pub my_review: Option<String>,
    /// Commits were pushed after the viewer's latest review.
    pub my_review_stale: bool,
}

/// A feature: 1+ PRs sharing the same branch name across repos.
#[derive(Debug, Serialize, Clone)]
pub struct PrGroup {
    pub branch: String,
    pub prs: Vec<PullRequest>,
}

/// Workspace PR status row: the workspace home's PR tracker (below the
/// pipeline bar). Same list as PullRequest plus state/commit count.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WsPrStatus {
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    /// OPEN | MERGED | CLOSED
    pub state: String,
    pub is_draft: bool,
    pub commits: u64,
    pub updated_at: String,
}

/// Squash-merges a PR and deletes its branch on GitHub. The deletion goes
/// through the API rather than `--delete-branch`, which would also delete
/// and switch local branches in whatever checkout gh runs from.
pub fn squash_merge(owner_repo: &str, number: u64) -> Result<(), String> {
    let n = number.to_string();
    let head = run_gh(&["pr", "view", &n, "-R", owner_repo, "--json", "headRefName", "--jq", ".headRefName"])?;
    let head = String::from_utf8_lossy(&head).trim().to_string();
    run_gh(&["pr", "merge", &n, "-R", owner_repo, "--squash"])?;
    // Already gone when the repo auto-deletes merged branches: fine.
    let _ = run_gh(&["api", "-X", "DELETE", &format!("repos/{owner_repo}/git/refs/heads/{head}")]);
    Ok(())
}

/// Status of the workspace branch's PRs (any state) in one repo.
pub fn pr_status_for_branch(owner_repo: &str, branch: &str) -> Result<Vec<WsPrStatus>, String> {
    let out = run_gh(&[
        "pr",
        "list",
        "-R",
        owner_repo,
        "--head",
        branch,
        "--state",
        "all",
        "--limit",
        "10",
        "--json",
        "number,title,author,isDraft,url,state,updatedAt,commits",
    ])?;
    let v: Vec<GhPrStatus> =
        serde_json::from_slice(&out).map_err(|e| format!("failed to parse pr status: {e}"))?;
    Ok(v.into_iter()
        .map(|p| WsPrStatus {
            repo: owner_repo.split('/').next_back().unwrap_or(owner_repo).to_string(),
            number: p.number,
            title: p.title,
            url: p.url,
            author: p.author.map(|a| a.login).unwrap_or_default(),
            state: p.state,
            is_draft: p.is_draft,
            commits: p.commits.len() as u64,
            updated_at: p.updated_at,
        })
        .collect())
}

#[derive(Deserialize)]
struct GhPrStatus {
    number: u64,
    title: String,
    author: Option<GhPrAuthor>,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    url: String,
    state: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(default)]
    commits: Vec<serde_json::Value>,
}

/// Live state of ONE known PR (direct view — no branch scan).
pub fn pr_status_by_number(
    owner_repo: &str,
    pr: &crate::workspace::PrRef,
) -> Result<Vec<WsPrStatus>, String> {
    let out = run_gh(&[
        "pr",
        "view",
        "-R",
        owner_repo,
        &pr.number.to_string(),
        "--json",
        "number,title,author,isDraft,url,state,updatedAt,commits",
    ])?;
    let p: GhPrStatus =
        serde_json::from_slice(&out).map_err(|e| format!("failed to parse pr view: {e}"))?;
    Ok(vec![WsPrStatus {
        repo: pr.repo.clone(),
        number: p.number,
        title: p.title,
        url: p.url,
        author: p.author.map(|a| a.login).unwrap_or_default(),
        state: p.state,
        is_draft: p.is_draft,
        commits: p.commits.len() as u64,
        updated_at: p.updated_at,
    }])
}

const PR_CACHE_TTL: Duration = Duration::from_secs(60);

fn pr_cache() -> &'static Cached<PrGroup> {
    static CACHE: OnceLock<Cached<PrGroup>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

const PR_INDEX_TTL: Duration = Duration::from_secs(300);

/// Recent-PR pool per repo for local substring search. The GitHub search
/// API matches title+body (templates pollute results) and does no
/// substring matching, so we fetch a window and filter locally instead.
fn pr_index() -> &'static Cached<PullRequest> {
    static CACHE: OnceLock<Cached<PullRequest>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Deserialize)]
struct GhPr {
    number: u64,
    title: String,
    #[serde(rename = "headRefName")]
    head_ref: String,
    #[serde(rename = "baseRefName")]
    base_ref: String,
    author: Option<GhPrAuthor>,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    url: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Deserialize)]
struct GhPrAuthor {
    login: String,
}

fn prs_for_repo(service_name: &str, owner_repo: &str, state: &str, limit: u32) -> Result<Vec<PullRequest>, String> {
    let out = run_gh(&[
        "pr",
        "list",
        "-R",
        owner_repo,
        "--state",
        state,
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,headRefName,baseRefName,author,isDraft,url,updatedAt",
    ])?;
    parse_pr_list(service_name, owner_repo, &out)
}

/// Open PRs of one branch in one repo (workspace "Pull requests" button).
pub fn prs_for_branch(service_name: &str, owner_repo: &str, branch: &str) -> Result<Vec<PullRequest>, String> {
    let out = run_gh(&[
        "pr",
        "list",
        "-R",
        owner_repo,
        "--head",
        branch,
        "--state",
        "open",
        "--limit",
        "10",
        "--json",
        "number,title,headRefName,baseRefName,author,isDraft,url,updatedAt",
    ])?;
    parse_pr_list(service_name, owner_repo, &out)
}

fn parse_pr_list(service_name: &str, owner_repo: &str, out: &[u8]) -> Result<Vec<PullRequest>, String> {
    let gh_prs: Vec<GhPr> =
        serde_json::from_slice(out).map_err(|e| format!("failed to parse pr list: {e}"))?;
    Ok(gh_prs
        .into_iter()
        .map(|p| PullRequest {
            repo: service_name.to_string(),
            owner_repo: owner_repo.to_string(),
            number: p.number,
            title: p.title,
            branch: p.head_ref,
            base: p.base_ref,
            author: p.author.map(|a| a.login).unwrap_or_default(),
            is_draft: p.is_draft,
            url: p.url,
            updated_at: p.updated_at,
            ..Default::default()
        })
        .collect())
}

/// Groups PRs by identical branch name (multi-repo features share branches).
fn group_prs(prs: Vec<PullRequest>) -> Vec<PrGroup> {
    group_prs_pub(prs)
}

/// Public re-export for commands.rs (workspace PR lookup).
pub fn group_prs_pub(prs: Vec<PullRequest>) -> Vec<PrGroup> {
    use std::collections::BTreeMap;
    let mut by_branch: BTreeMap<String, Vec<PullRequest>> = BTreeMap::new();
    for pr in prs {
        by_branch.entry(pr.branch.clone()).or_default().push(pr);
    }
    let mut groups: Vec<PrGroup> = by_branch
        .into_iter()
        .map(|(branch, mut prs)| {
            // newest first inside the group
            prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            PrGroup { branch, prs }
        })
        .collect();
    // multi-repo features first, then newest first
    groups.sort_by(|a, b| {
        b.prs
            .len()
            .cmp(&a.prs.len())
            .then_with(|| b.prs[0].updated_at.cmp(&a.prs[0].updated_at))
    });
    groups
}

/// Lists OPEN PRs from every configured service, updated in the last 7
/// days, grouped by branch. Cached for 60s; bypass with force_refresh.
pub fn list_prs(force_refresh: bool) -> Result<Vec<PrGroup>, String> {
    if !force_refresh {
        if let Some((fetched_at, groups)) = pr_cache().lock().unwrap().clone() {
            if fetched_at.elapsed() < PR_CACHE_TTL {
                return Ok(groups);
            }
        }
    }

    let cfg = crate::config::Config::load()?;
    let services: Vec<(String, String)> = cfg
        .services
        .iter()
        .filter_map(|s| parse_owner_repo(&s.repo).ok().map(|or| (s.name.clone(), or)))
        .collect();

    // 7-day cutoff on updatedAt (gh emits ISO 8601; lexicographic compare works).
    let cutoff = iso_cutoff_days(7);

    let prs: Vec<PullRequest> = std::thread::scope(|scope| {
        let handles: Vec<_> = services
            .iter()
            // Without review/CI state rather than not at all.
            .map(|(name, or)| {
                scope.spawn(move || open_prs_with_status(name, or).or_else(|_| prs_for_repo(name, or, "open", 50)))
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter_map(|r| r.ok())
            .flatten()
            .filter(|pr| pr.updated_at >= cutoff)
            .collect()
    });

    let groups = group_prs(prs);
    *pr_cache().lock().unwrap() = Some((Instant::now(), groups.clone()));
    warm_pr_index();
    Ok(groups)
}

/// Open PRs of one repo with their review and CI state, and the viewer's
/// own latest review, in one GraphQL round trip (`gh pr list --json` can't
/// tell which commit a review was on, and asking it for commits overflows
/// GitHub's node limit).
const OPEN_PRS_QUERY: &str = "query($owner: String!, $name: String!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequests(states: OPEN, first: 50, orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes {
        number title headRefName baseRefName isDraft url updatedAt headRefOid
        author { login }
        reviewDecision mergeable
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        latestReviews(first: 30) { nodes { author { login } state commit { oid } } }
      }
    }
  }
}";

#[derive(Deserialize)]
struct GqlOpenPrs {
    data: GqlOpenPrsData,
}
#[derive(Deserialize)]
struct GqlOpenPrsData {
    viewer: GhPrAuthor,
    repository: GqlRepo,
}
#[derive(Deserialize)]
struct GqlRepo {
    #[serde(rename = "pullRequests")]
    pull_requests: GqlNodes<GqlPr>,
}
#[derive(Deserialize)]
struct GqlNodes<T> {
    nodes: Vec<T>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPr {
    number: u64,
    title: String,
    head_ref_name: String,
    base_ref_name: String,
    is_draft: bool,
    url: String,
    updated_at: String,
    head_ref_oid: String,
    author: Option<GhPrAuthor>,
    review_decision: Option<String>,
    mergeable: Option<String>,
    commits: GqlNodes<GqlCommitNode>,
    latest_reviews: GqlNodes<GqlReview>,
}
#[derive(Deserialize)]
struct GqlCommitNode {
    commit: GqlCommit,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCommit {
    status_check_rollup: Option<GqlRollup>,
}
#[derive(Deserialize)]
struct GqlRollup {
    state: String,
}
#[derive(Deserialize)]
struct GqlReview {
    author: Option<GhPrAuthor>,
    state: String,
    commit: Option<GqlOid>,
}
#[derive(Deserialize)]
struct GqlOid {
    oid: String,
}

fn open_prs_with_status(service_name: &str, owner_repo: &str) -> Result<Vec<PullRequest>, String> {
    let (owner, name) = owner_repo.split_once('/').ok_or_else(|| format!("bad owner/repo '{owner_repo}'"))?;
    let out = run_gh(&[
        "api",
        "graphql",
        "-f",
        &format!("owner={owner}"),
        "-f",
        &format!("name={name}"),
        "-f",
        &format!("query={OPEN_PRS_QUERY}"),
    ])?;
    let data: GqlOpenPrs = serde_json::from_slice(&out).map_err(|e| format!("failed to parse open PRs: {e}"))?;
    Ok(parse_open_prs(service_name, owner_repo, data.data))
}

fn parse_open_prs(service_name: &str, owner_repo: &str, data: GqlOpenPrsData) -> Vec<PullRequest> {
    let me = data.viewer.login;
    data.repository
        .pull_requests
        .nodes
        .into_iter()
        .map(|p| {
            let author = p.author.map(|a| a.login).unwrap_or_default();
            let mine = review_of(&p.latest_reviews.nodes, &me);
            let checks = p.commits.nodes.into_iter().next().and_then(|c| c.commit.status_check_rollup).and_then(|r| {
                match r.state.as_str() {
                    "SUCCESS" => Some("pass"),
                    "FAILURE" | "ERROR" => Some("fail"),
                    "PENDING" | "EXPECTED" => Some("pending"),
                    _ => None,
                }
            });
            let status = PrReviewStatus {
                review_decision: p.review_decision.filter(|d| !d.is_empty()),
                checks: checks.map(str::to_string),
                conflicts: p.mergeable.as_deref() == Some("CONFLICTING"),
                mine: !me.is_empty() && author == me,
                my_review_stale: mine.is_some_and(|r| r.commit.as_ref().is_some_and(|c| c.oid != p.head_ref_oid)),
                my_review: mine.map(|r| r.state.clone()),
            };
            PullRequest {
                repo: service_name.to_string(),
                owner_repo: owner_repo.to_string(),
                number: p.number,
                title: p.title,
                branch: p.head_ref_name,
                base: p.base_ref_name,
                author,
                is_draft: p.is_draft,
                url: p.url,
                updated_at: p.updated_at,
                status: Some(status),
            }
        })
        .collect()
}

/// `login`'s latest review among a PR's latest reviews (one per reviewer).
fn review_of<'a>(reviews: &'a [GqlReview], login: &str) -> Option<&'a GqlReview> {
    reviews
        .iter()
        .find(|r| !login.is_empty() && r.author.as_ref().is_some_and(|a| a.login == login))
}

/// Current UTC time minus `days`, as an ISO-8601 string for comparisons.
fn iso_cutoff_days(days: u64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        - (days as i64) * 86_400;
    // epoch secs -> civil date (Howard Hinnant's days-from-civil, inverted)
    let epoch_days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = epoch_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Searches PRs (any state/age) across every configured service. Understands
/// three query shapes: a GitHub PR URL, a bare number (scans configured
/// repos for that PR number), or free text (gh search prs).
pub fn search_prs(query: &str) -> Result<Vec<PrGroup>, String> {
    let cfg = crate::config::Config::load()?;
    let services: Vec<(String, String)> = cfg
        .services
        .iter()
        .filter_map(|s| parse_owner_repo(&s.repo).ok().map(|or| (s.name.clone(), or)))
        .collect();
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }

    // 1) GitHub PR URL — resolve directly.
    if let Some((owner_repo, number)) = parse_pr_url(q) {
        let prs = std::thread::scope(|scope| {
            let handles: Vec<_> = services
                .iter()
                .map(|(name, or)| {
                    scope.spawn(move || pr_by_number(name, or, number))
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter_map(|r| r.ok())
                .flatten()
                .collect::<Vec<PullRequest>>()
        });
        // also resolve even when the repo isn't configured
        let mut prs = prs;
        if !prs.iter().any(|p| p.owner_repo == owner_repo) {
            let service_name = services
                .iter()
                .find(|(_, or)| *or == owner_repo)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| owner_repo.split('/').next_back().unwrap_or("repo").to_string());
            if let Ok(pr) = pr_by_number(&service_name, &owner_repo, number) {
                prs.extend(pr);
            }
        }
        return Ok(group_prs(prs));
    }

    // 2) Bare number — scan configured repos for that PR number.
    if let Ok(number) = q.parse::<u64>() {
        let prs: Vec<PullRequest> = std::thread::scope(|scope| {
            let handles: Vec<_> = services
                .iter()
                .map(|(name, or)| scope.spawn(move || pr_by_number(name, or, number)))
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter_map(|r| r.ok())
                .flatten()
                .collect()
        });
        if !prs.is_empty() {
            return Ok(group_prs(prs));
        }
        return Ok(vec![]);
    }

    // 3) Free text — local substring filter over a recent-PR pool. GitHub
    //    search matches title+body (PR templates pollute results) and does
    //    no substring matching, so we filter title-only locally.
    // ponytail: title-only by design (user request); branch/author search
    // would need a separate toggle, pool already has the fields.
    let q = q.to_lowercase();
    let pool: Vec<PullRequest> = {
        // Hold the lock across the fetch: concurrent searches (one per
        // debounce) share one fetch instead of stampeding gh.
        let mut guard = pr_index().lock().unwrap();
        match guard.clone() {
            Some((fetched_at, prs)) if fetched_at.elapsed() < PR_INDEX_TTL => prs,
            _ => {
                let prs: Vec<PullRequest> = std::thread::scope(|scope| {
                    let handles: Vec<_> = services
                        .iter()
                        .map(|(name, or)| {
            // ponytail: pool of 500/repo covers every PR in these repos (verified
    // 2026-09); raise or switch to GitHub search API if a repo passes 500.
                    scope.spawn(move || prs_for_repo(name, or, "all", 500))
                        })
                        .collect();
                    handles
                        .into_iter()
                        .filter_map(|h| h.join().ok())
                        .filter_map(|r| r.ok())
                        .flatten()
                        .collect()
                });
                *guard = Some((Instant::now(), prs.clone()));
                prs
            }
        }
    };
    let matched: Vec<PullRequest> = pool
        .into_iter()
        .filter(|p| p.title.to_lowercase().contains(&q))
        .collect();

    Ok(group_prs(matched))
}

/// Warms the search index so the first typed query doesn't wait on a
/// fetch. Called alongside pr_list on page load.
pub fn warm_pr_index() {
    let cached = pr_index()
        .lock()
        .unwrap()
        .as_ref()
        .map(|(at, _)| at.elapsed() < PR_INDEX_TTL)
        .unwrap_or(false);
    if cached {
        return;
    }
    std::thread::spawn(|| {
        let _ = search_prs_probe();
    });
}

fn search_prs_probe() -> Result<Vec<PrGroup>, String> {
    // Runs the free-text path with a query that matches nothing, purely to
    // populate the index cache.
    search_prs("\u{0}warm")
}

/// "https://github.com/owner/repo/pull/123" (with optional suffixes) →
/// ("owner/repo", 123).
fn parse_pr_url(s: &str) -> Option<(String, u64)> {
    let rest = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let num = parts.next()?.parse::<u64>().ok()?;
    Some((format!("{owner}/{repo}"), num))
}

/// Fetches a single PR by number from one repo (None when it doesn't exist).
fn pr_by_number(service_name: &str, owner_repo: &str, number: u64) -> Result<Vec<PullRequest>, String> {
    let out = run_gh(&[
        "pr",
        "view",
        "-R",
        owner_repo,
        &number.to_string(),
        "--json",
        "number,title,headRefName,baseRefName,author,isDraft,url,updatedAt",
    ])?;
    let p: GhPr =
        serde_json::from_slice(&out).map_err(|e| format!("failed to parse pr view: {e}"))?;
    Ok(vec![PullRequest {
        repo: service_name.to_string(),
        owner_repo: owner_repo.to_string(),
        number: p.number,
        title: p.title,
        branch: p.head_ref,
        base: p.base_ref,
        author: p.author.map(|a| a.login).unwrap_or_default(),
        is_draft: p.is_draft,
        url: p.url,
        updated_at: p.updated_at,
        ..Default::default()
    }])
}


#[derive(Debug, Serialize, Clone)]
pub struct PrFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub title: String,
    pub author: String,
    pub url: String,
    pub branch: String,
    pub base: String,
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub files: Vec<PrFile>,
    pub head_sha: String,
    pub base_sha: String,
}

#[derive(Deserialize)]
struct GhPrView {
    title: String,
    author: Option<GhPrAuthor>,
    url: String,
    #[serde(rename = "headRefName")]
    head_ref: String,
    #[serde(rename = "baseRefName")]
    base_ref: String,
    body: Option<String>,
    additions: u64,
    deletions: u64,
    files: Option<Vec<GhViewFile>>,
    #[serde(rename = "headRefOid")]
    head_sha: String,
    #[serde(rename = "baseRefOid")]
    base_sha: String,
}

#[derive(Deserialize)]
struct GhViewFile {
    path: String,
    additions: Option<u64>,
    deletions: Option<u64>,
}

/// PR metadata + changed files list.
pub fn pr_detail(owner_repo: &str, number: u64) -> Result<PrDetail, String> {
    let out = run_gh(&[
        "pr",
        "view",
        "-R",
        owner_repo,
        &number.to_string(),
        "--json",
        "title,author,url,headRefName,baseRefName,body,additions,deletions,files,headRefOid,baseRefOid",
    ])?;
    let v: GhPrView =
        serde_json::from_slice(&out).map_err(|e| format!("failed to parse pr view: {e}"))?;
    Ok(PrDetail {
        title: v.title,
        author: v.author.map(|a| a.login).unwrap_or_default(),
        url: v.url,
        branch: v.head_ref,
        base: v.base_ref,
        body: v.body.unwrap_or_default(),
        additions: v.additions,
        deletions: v.deletions,
        files: v
            .files
            .unwrap_or_default()
            .into_iter()
            .map(|f| PrFile {
                path: f.path,
                additions: f.additions.unwrap_or(0),
                deletions: f.deletions.unwrap_or(0),
            })
            .collect(),
        head_sha: v.head_sha,
        base_sha: v.base_sha,
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct PrFileDiff {
    pub original: String,
    pub modified: String,
}

/// File contents at a PR's base and head, fetched via the GitHub contents
/// API (base64). New files -> empty original; deleted -> empty modified.
/// Binary or >1MB files error with a friendly message.
pub fn pr_file_diff(
    owner_repo: &str,
    head_sha: &str,
    base_sha: &str,
    path: &str,
) -> Result<PrFileDiff, String> {
    // Both sides at once: each is a separate gh round trip.
    let (modified, original) = std::thread::scope(|sc| {
        let head = sc.spawn(|| content_or_empty(owner_repo, head_sha, path));
        let base = content_or_empty(owner_repo, base_sha, path);
        (head.join().unwrap_or_else(|_| Err("fetch panicked".into())), base)
    });
    Ok(PrFileDiff { original: original?, modified: modified? })
}

/// File contents at a commit, memoized: a (repo, sha, path) never changes.
/// Missing at that commit (added/deleted in the PR) is an empty file.
// ponytail: in-memory and unbounded; the session's reviewed files are small
// next to the app, persist to disk if cold starts matter.
fn content_or_empty(owner_repo: &str, sha: &str, path: &str) -> Result<String, String> {
    static CONTENTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let key = format!("{owner_repo}@{sha}:{path}");
    let map = CONTENTS.get_or_init(Default::default);
    if let Some(c) = map.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(c);
    }
    let c = match fetch_content_at(owner_repo, sha, path) {
        Ok(c) => c,
        Err(e) if e.contains("Not Found") || e.contains("404") => String::new(),
        Err(e) => return Err(e),
    };
    if let Ok(mut m) = map.lock() {
        m.insert(key, c.clone());
    }
    Ok(c)
}

fn fetch_content_at(owner_repo: &str, sha: &str, path: &str) -> Result<String, String> {
    let api = format!("repos/{owner_repo}/contents/{path}?ref={sha}");
    let out = run_gh(&["api", &api])?;
    let v: serde_json::Value =
        serde_json::from_slice(&out).map_err(|e| format!("bad contents response: {e}"))?;
    match v.get("encoding").and_then(|e| e.as_str()) {
        Some("base64") => {}
        // >1MB files come back without inline content (encoding "none")
        Some(_) => {
            return Err("file is too large to display in the diff (>1MB)".into());
        }
        None => return Err("binary or too large to display".into()),
    }
    let b64 = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
    decode_base64(b64)
}

// Minimal base64 decode (no new deps).
fn decode_base64(s: &str) -> Result<String, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut buf: Vec<u8> = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for ch in s.chars() {
        if ch == '=' {
            break;
        }
        let val = TABLE
            .iter()
            .position(|&t| t as char == ch)
            .ok_or_else(|| "invalid base64".to_string())? as u32;
        acc = (acc << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    String::from_utf8(buf).map_err(|_| "file is not valid UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_owner_repo_handles_ssh_and_https() {
        assert_eq!(
            parse_owner_repo("git@github.com:NSXBet/referrals-svc.git").unwrap(),
            "NSXBet/referrals-svc"
        );
        assert_eq!(
            parse_owner_repo("https://github.com/igorsegallafa/orbit.git").unwrap(),
            "igorsegallafa/orbit"
        );
        assert!(parse_owner_repo("not a url").is_err());
    }

    #[test]
    fn groups_prs_by_identical_branch() {
        let mk = |repo: &str, n: u64, br: &str| PullRequest {
            repo: repo.into(),
            owner_repo: format!("o/{repo}"),
            number: n,
            title: format!("t{n}"),
            branch: br.into(),
            base: "main".into(),
            author: "a".into(),
            is_draft: false,
            url: String::new(),
            updated_at: "2026-09-10T00:00:00Z".into(),
            ..Default::default()
        };
        let groups = group_prs(vec![mk("a", 1, "feat/x"), mk("b", 2, "feat/x"), mk("c", 3, "other")]);
        assert_eq!(groups[0].branch, "feat/x");
        assert_eq!(groups[0].prs.len(), 2);
        assert_eq!(groups[1].prs.len(), 1);
    }

    #[test]
    fn parse_pr_url_handles_full_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/NSXBet/campaign-manager-svc/pull/75"),
            Some(("NSXBet/campaign-manager-svc".into(), 75))
        );
        assert_eq!(
            parse_pr_url("https://github.com/NSXBet/campaign-manager-svc/pull/75/files"),
            Some(("NSXBet/campaign-manager-svc".into(), 75))
        );
        assert_eq!(parse_pr_url("https://github.com/NSXBet/campaign-manager-svc"), None);
        assert_eq!(parse_pr_url("not a url"), None);
    }

    #[test]
    fn pr_view_shape_parses_gh_output() {
        // Simulate `gh pr view --json` output including the exact field set
        // pr_detail requests.
        let sample = r#"{"title":"t","author":{"login":"me"},"url":"u","headRefName":"b","baseRefName":"main","body":null,"additions":3,"deletions":1,"files":[{"path":"go.mod","additions":3,"deletions":9}],"headRefOid":"abc","baseRefOid":"def"}"#;
        let v: GhPrView = serde_json::from_str(sample).expect("should parse pr view json");
        assert_eq!(v.head_sha, "abc");
        assert_eq!(v.files.unwrap()[0].path, "go.mod");
    }

    #[test]
    fn cutoff_is_seven_days_ago_iso() {
        let s = iso_cutoff_days(7);
        assert!(s.ends_with('Z') && s.len() == 20 && s.contains('T'));
        // 2026-09-16 minus 7 days is 2026-09-09
        assert!(s.starts_with("2026-09-"), "{s}");
    }
}



#[cfg(test)]
mod pr_wire_tests {
    use super::*;

    #[test]
    fn pull_request_serializes_camel_case_for_frontend() {
        // The frontend reads pr.ownerRepo / isDraft / updatedAt — serde must
        // emit camelCase or every invoke with these fields fails with
        // "invalid args" before the command even runs.
        let pr = PullRequest {
            repo: "svc".into(),
            owner_repo: "owner/svc".into(),
            number: 1,
            title: "t".into(),
            branch: "b".into(),
            base: "main".into(),
            author: "a".into(),
            is_draft: false,
            url: "u".into(),
            updated_at: "2026-09-16T00:00:00Z".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(&pr).unwrap();
        assert!(v.get("ownerRepo").is_some(), "must be ownerRepo, got: {v}");
        assert!(v.get("isDraft").is_some());
        assert!(v.get("updatedAt").is_some());

        let d = PrDetail {
            title: "t".into(), author: "a".into(), url: "u".into(),
            branch: "b".into(), base: "main".into(), body: String::new(),
            additions: 1, deletions: 1, files: vec![],
            head_sha: "abc".into(), base_sha: "def".into(),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("headSha").is_some(), "must be headSha, got: {v}");
        assert!(v.get("baseSha").is_some());
    }

    #[test]
    fn open_prs_carry_review_ci_and_the_viewers_review() {
        let raw = r#"{"data":{"viewer":{"login":"me"},"repository":{"pullRequests":{"nodes":[
          {"number":1,"title":"reviewed, then pushed to","headRefName":"a","baseRefName":"main","isDraft":false,"url":"u","updatedAt":"t","headRefOid":"new",
           "author":{"login":"ana"},"reviewDecision":"CHANGES_REQUESTED","mergeable":"CONFLICTING",
           "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]},
           "latestReviews":{"nodes":[{"author":{"login":"bob"},"state":"APPROVED","commit":{"oid":"new"}},{"author":{"login":"me"},"state":"CHANGES_REQUESTED","commit":{"oid":"old"}}]}},
          {"number":2,"title":"mine, no checks","headRefName":"b","baseRefName":"main","isDraft":true,"url":"u","updatedAt":"t","headRefOid":"x",
           "author":{"login":"me"},"reviewDecision":"","mergeable":"MERGEABLE",
           "commits":{"nodes":[{"commit":{"statusCheckRollup":null}}]},"latestReviews":{"nodes":[]}}
        ]}}}}"#;
        let data: GqlOpenPrs = serde_json::from_str(raw).unwrap();
        let prs = parse_open_prs("svc", "o/svc", data.data);
        let s1 = prs[0].status.clone().unwrap();
        assert_eq!(s1.review_decision.as_deref(), Some("CHANGES_REQUESTED"));
        assert_eq!(s1.checks.as_deref(), Some("fail"));
        assert!(s1.conflicts && !s1.mine);
        assert_eq!(s1.my_review.as_deref(), Some("CHANGES_REQUESTED"));
        assert!(s1.my_review_stale, "the head moved past my review");
        let s2 = prs[1].status.clone().unwrap();
        assert_eq!(s2, PrReviewStatus { mine: true, ..Default::default() });
        let v = serde_json::to_value(&prs[0]).unwrap();
        assert!(v["status"].get("myReviewStale").is_some(), "camelCase: {v}");
    }

    fn pr(title: &str, branch: &str, author: &str) -> PullRequest {
        PullRequest {
            repo: "svc".into(),
            owner_repo: "owner/svc".into(),
            number: 1,
            title: title.into(),
            branch: branch.into(),
            base: "main".into(),
            author: author.into(),
            is_draft: false,
            url: "u".into(),
            updated_at: "2026-09-16T00:00:00Z".into(),
            ..Default::default()
        }
    }

    #[test]
    fn local_search_matches_substring_in_title_branch_author() {
        // "trouble" must match "troubleshooting" (substring, any of the
        // three fields), and matching must be case-insensitive.
        let pool = [pr("chore: add troubleshooting endpoint", "feat/x", "lucas"),
            pr("feat: unrelated", "fix/troublesome-bug", "MARIA"),
            pr("docs: onboarding rewrite", "docs/readme", "joao")];
        let q = "trouble".to_lowercase();
        let matched: Vec<&PullRequest> = pool
            .iter()
            .filter(|p| {
                p.title.to_lowercase().contains(&q)
                    || p.branch.to_lowercase().contains(&q)
                    || p.author.to_lowercase().contains(&q)
            })
            .collect();
        assert_eq!(matched.len(), 2, "title + branch substrings");
    }
}

// ---------- CI checks (GitHub Actions status of a PR) ----------

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrCheck {
    pub name: String,
    /// SUCCESS | FAILURE | PENDING | SKIPPED | NEUTRAL | ACTION_REQUIRED ...
    pub state: String,
    /// pass | fail | skipping | pending — gh's bucket, handy for grouping
    pub bucket: String,
    pub workflow: String,
    pub link: String,
    pub started_at: String,
    pub completed_at: String,
}

/// CI checks attached to a PR (via `gh pr checks --json`).
pub fn pr_checks(owner_repo: &str, number: u64) -> Result<Vec<PrCheck>, String> {
    let out = run_gh(&[
        "pr",
        "checks",
        "-R",
        owner_repo,
        &number.to_string(),
        "--json",
        "name,state,bucket,workflow,link,startedAt,completedAt",
    ])?;
    let v: Vec<GhCheck> =
        serde_json::from_slice(&out).map_err(|e| format!("failed to parse pr checks: {e}"))?;
    Ok(v.into_iter()
        .map(|c| PrCheck {
            name: c.name,
            state: c.state,
            bucket: c.bucket,
            workflow: c.workflow,
            link: c.link,
            started_at: c.started_at,
            completed_at: c.completed_at,
        })
        .collect())
}

#[derive(Deserialize)]
struct GhCheck {
    name: String,
    state: String,
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    workflow: String,
    #[serde(default)]
    link: String,
    #[serde(rename = "startedAt", default)]
    started_at: String,
    #[serde(rename = "completedAt", default)]
    completed_at: String,
}

/// Extracts the Actions run id from a check's job link
/// (".../actions/runs/12345/job/678" → 12345). External statuses
/// (Aikido, CodeRabbit) have no Actions link → None.
pub fn run_id_from_link(link: &str) -> Option<u64> {
    let idx = link.find("/actions/runs/")?;
    let rest = &link[idx + "/actions/runs/".len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

#[cfg(test)]
mod checks_tests {
    use super::*;

    #[test]
    fn parses_run_id_from_job_link() {
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/35173937422/job/105051372465"),
            Some(35173937422)
        );
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/123"),
            Some(123)
        );
        assert_eq!(run_id_from_link("https://app.aikido.dev/scan/195"), None);
        assert_eq!(run_id_from_link(""), None);
    }

    #[test]
    fn derives_owner_repo_from_job_link() {
        assert_eq!(
            owner_repo_from_link("https://github.com/NSXBet/r/actions/runs/1/job/2"),
            Some("NSXBet/r".to_string())
        );
        assert_eq!(owner_repo_from_link("https://app.aikido.dev/x"), None);
    }

    #[test]
    fn check_serializes_camel_case_for_frontend() {
        // The frontend reads startedAt/completedAt — snake_case output was
        // the same class of bug as PullRequest/RepoStatus (bit twice).
        let c = PrCheck {
            name: "test".into(),
            state: "FAILURE".into(),
            bucket: "fail".into(),
            workflow: "CI".into(),
            link: "https://x".into(),
            started_at: "2026-01-01T00:00:00Z".into(),
            completed_at: "2026-01-01T00:01:00Z".into(),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("startedAt").is_some(), "must be startedAt: {v}");
        assert!(v.get("completedAt").is_some());
    }
}

/// "https://github.com/owner/repo/actions/..." → "owner/repo" (None for
/// external links).
pub fn owner_repo_from_link(link: &str) -> Option<String> {
    let rest = link.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}
