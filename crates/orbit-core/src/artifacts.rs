// GitHub Actions artifacts of a branch: the builds CI already produced
// (executables, bundles), so a feature can be tried without compiling it.
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// Workflows looked at per branch (their latest run each).
const MAX_WORKFLOWS: usize = 8;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    pub run_id: u64,
    pub workflow: String,
    /// When the run started (ISO).
    pub created_at: String,
    pub head_sha: String,
}

fn gh_json(args: &[&str]) -> Result<Value, String> {
    let out = crate::proc::run("gh", args, None)?;
    serde_json::from_str(&out).map_err(|e| format!("unexpected GitHub reply: {e}"))
}

/// Query-string safe branch name (slashes are fine in a query value).
fn encode(v: &str) -> String {
    v.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => c.to_string(),
            _ => c.to_string().bytes().map(|b| format!("%{b:02X}")).collect(),
        })
        .collect()
}

/// The latest completed run of each workflow, newest first (the API lists
/// runs newest first already).
fn latest_runs(runs: &[Value]) -> Vec<&Value> {
    let mut seen = std::collections::HashSet::new();
    runs.iter()
        .filter(|r| r.get("status").and_then(Value::as_str) == Some("completed"))
        .filter(|r| seen.insert(r.get("workflow_id").and_then(Value::as_u64).unwrap_or(0)))
        .take(MAX_WORKFLOWS)
        .collect()
}

fn artifacts_of(owner_repo: &str, run: &Value) -> Vec<Artifact> {
    let run_id = run.get("id").and_then(Value::as_u64).unwrap_or(0);
    let s = |k: &str| run.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let Ok(v) = gh_json(&["api", &format!("repos/{owner_repo}/actions/runs/{run_id}/artifacts")]) else {
        return vec![];
    };
    v.get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|a| !a.get("expired").and_then(Value::as_bool).unwrap_or(false))
        .map(|a| Artifact {
            id: a.get("id").and_then(Value::as_u64).unwrap_or(0),
            name: a.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            size_bytes: a.get("size_in_bytes").and_then(Value::as_u64).unwrap_or(0),
            run_id,
            workflow: s("name"),
            created_at: s("created_at"),
            head_sha: s("head_sha"),
        })
        .collect()
}

/// Unexpired artifacts from the latest run of each workflow on `branch`.
pub fn branch_artifacts(owner_repo: &str, branch: &str) -> Vec<Artifact> {
    let path = format!("repos/{owner_repo}/actions/runs?branch={}&per_page=50", encode(branch));
    // No Actions on the repo (404) or no access: simply nothing to offer.
    let Ok(v) = gh_json(&["api", &path]) else { return vec![] };
    let runs = v.get("workflow_runs").and_then(Value::as_array).cloned().unwrap_or_default();
    let latest = latest_runs(&runs);
    std::thread::scope(|scope| {
        let handles: Vec<_> = latest.iter().map(|r| scope.spawn(|| artifacts_of(owner_repo, r))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).flatten().collect()
    })
}

/// A name usable as one folder name on every OS.
fn folder_name(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    clean.trim_matches(['-', '.']).to_string()
}

/// `~/Downloads/Orbit`; `ORBIT_DOWNLOADS_DIR` overrides it (tests).
fn downloads_root() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("ORBIT_DOWNLOADS_DIR") {
        return Ok(PathBuf::from(d));
    }
    Ok(crate::config::home_dir()?.join("Downloads").join("Orbit"))
}

/// Downloads (and extracts) one artifact into ~/Downloads/Orbit/<folder>/.
/// Returns the folder.
pub fn download(owner_repo: &str, run_id: u64, name: &str, folder: &str) -> Result<PathBuf, String> {
    let dest = downloads_root()?.join(folder_name(folder)).join(folder_name(name));
    // A fresh copy: gh refuses to overwrite files from an older download.
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("couldn't clear {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(&dest).map_err(|e| format!("couldn't create {}: {e}", dest.display()))?;
    let dest_s = dest.to_string_lossy().to_string();
    crate::proc::run("gh", &["run", "download", &run_id.to_string(), "-R", owner_repo, "-n", name, "-D", &dest_s], None)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_the_latest_completed_run_per_workflow() {
        let runs = vec![
            json!({"id": 3, "workflow_id": 1, "status": "in_progress"}),
            json!({"id": 2, "workflow_id": 1, "status": "completed"}),
            json!({"id": 1, "workflow_id": 1, "status": "completed"}),
            json!({"id": 9, "workflow_id": 7, "status": "completed"}),
        ];
        let ids: Vec<u64> = latest_runs(&runs).iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, [2, 9]);
    }

    #[test]
    fn names_are_folder_and_query_safe() {
        assert_eq!(folder_name("feat/sc-74911: preview"), "feat-sc-74911--preview");
        assert_eq!(encode("feat/sc 1#2"), "feat/sc%201%232");
    }
}
