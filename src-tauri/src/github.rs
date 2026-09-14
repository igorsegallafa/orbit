// Lists repos from GitHub using the `gh` CLI (reuses the user's existing
// `gh auth login` session — no OAuth flow to build/maintain ourselves).
//
// Fetches repos for the authenticated user AND every organization they
// belong to, in parallel (each `gh` invocation is a separate process with
// noticeable startup + network latency, so sequential calls scale badly
// once someone is in several orgs).
use serde::{Deserialize, Serialize};
use std::process::Command;
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

const CACHE_TTL: Duration = Duration::from_secs(300);

fn cache() -> &'static Mutex<Option<(Instant, Vec<GithubRepo>)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<GithubRepo>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn is_authenticated() -> bool {
    Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_gh(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out.stdout)
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
    repos.sort_by(|a, b| a.name_with_owner.to_lowercase().cmp(&b.name_with_owner.to_lowercase()));

    *cache().lock().unwrap() = Some((Instant::now(), repos.clone()));
    Ok(repos)
}
