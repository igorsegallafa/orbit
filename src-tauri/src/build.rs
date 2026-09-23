// Per-repo builds (config `build` command) with live output, a commit-keyed
// skip cache and a saved log on failure.
use crate::agent::runner::{self, Line};
use crate::config::Config;
use crate::workspace::{workspace_root, ws_dir};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

// ponytail: generous fixed ceiling; make it per-repo config if a build
// legitimately runs longer.
const BUILD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);

#[derive(Serialize, Clone)]
struct BuildLine<'a> {
    key: &'a str,
    line: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub key: String,
    /// "ok" | "skipped" | "failed" | "cancelled"
    pub status: String,
    pub log_path: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    commit: String,
    built_at: u64,
}

/// `<workspace>/<repo>` for a workspace build, `<repo>` for the base clone.
pub fn build_key(workspace: Option<&str>, repo: &str) -> String {
    match workspace {
        Some(ws) => format!("{ws}/{repo}"),
        None => repo.to_string(),
    }
}

fn cache_dir() -> Result<PathBuf, String> {
    Ok(workspace_root()?.join(".orbit-cache"))
}

fn read_cache(path: &Path) -> HashMap<String, CacheEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Up to date = clean tree whose HEAD is the commit last built.
fn is_up_to_date(cache: &HashMap<String, CacheEntry>, key: &str, dir: &Path) -> bool {
    let Some(entry) = cache.get(key) else { return false };
    if crate::git::is_dirty(dir) {
        return false;
    }
    head(dir).is_some_and(|h| h == entry.commit)
}

fn head(dir: &Path) -> Option<String> {
    crate::proc::run("git", &["rev-parse", "HEAD"], Some(dir))
        .ok()
        .map(|s| s.trim().to_string())
}

/// The platform shell running `script`, like Node's `shell: true`.
fn shell(script: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = crate::proc::cmd("cmd");
        // raw_arg: cmd.exe parses its own command line; std's quoting would
        // mangle `&&` chains and nested quotes.
        c.raw_arg(format!("/d /s /c \"{script}\""));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = crate::proc::cmd("sh");
        c.args(["-c", script]);
        c
    }
}

fn log_file_name(key: &str) -> String {
    key.chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect::<String>()
        + ".log"
}

/// Builds `repo` in a workspace (or its base clone when `workspace` is
/// None), streaming `build-output` events. Skips when nothing changed
/// since the last successful build unless `force`.
pub fn build(app: &AppHandle, workspace: Option<&str>, repo: &str, force: bool) -> Result<BuildResult, String> {
    let cfg = Config::load()?;
    let svc = cfg
        .services
        .iter()
        .find(|s| s.name == repo)
        .ok_or_else(|| format!("unknown repository: {repo}"))?;
    let script = svc
        .build
        .as_ref()
        .and_then(|b| b.for_current_os())
        .ok_or_else(|| format!("{repo} has no build command for this platform"))?
        .to_string();
    let dir = match workspace {
        Some(ws) => ws_dir(ws)?.join(repo),
        None => crate::workspace::clone_dir(svc)?,
    };
    if !dir.join(".git").exists() {
        return Err(format!("{repo} is not set up at {}", dir.display()));
    }
    // A branch-only repo builds in its base clone: share its cache entry.
    let key = if workspace.is_some() && crate::links::is_link(&dir) {
        build_key(None, repo)
    } else {
        build_key(workspace, repo)
    };

    let cache_path = cache_dir()?.join("build-state.json");
    let mut cache = read_cache(&cache_path);
    if !force && is_up_to_date(&cache, &key, &dir) {
        return Ok(BuildResult { key, status: "skipped".into(), log_path: None });
    }

    let emit = |line: String| {
        let _ = app.emit("build-output", BuildLine { key: &key, line });
    };
    emit(format!("$ {script}"));
    let run_id = format!("build:{key}");
    let outcome = runner::run_streaming(shell(&script), &dir, BUILD_TIMEOUT, Some(&run_id), |l| match l {
        Line::Out(s) | Line::Err(s) => emit(s),
    });

    let (status, transcript) = match outcome {
        Ok(o) if o.success => ("ok", String::new()),
        Ok(o) if o.cancelled => ("cancelled", String::new()),
        Ok(o) => ("failed", format!("{}{}", o.stdout, o.stderr)),
        Err(e) => ("failed", e),
    };

    let mut log_path = None;
    if status == "ok" {
        if let Some(commit) = head(&dir) {
            let built_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            cache.insert(key.clone(), CacheEntry { commit, built_at });
            std::fs::create_dir_all(cache_dir()?).map_err(|e| e.to_string())?;
            let raw = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
            std::fs::write(&cache_path, raw).map_err(|e| e.to_string())?;
        }
    } else if status == "failed" {
        let logs = cache_dir()?.join("logs");
        std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        let p = logs.join(log_file_name(&key));
        std::fs::write(&p, transcript).map_err(|e| e.to_string())?;
        log_path = Some(p.to_string_lossy().to_string());
    }
    Ok(BuildResult { key, status: status.into(), log_path })
}

pub fn cancel(workspace: Option<&str>, repo: &str) -> bool {
    runner::cancel(&format!("build:{}", build_key(workspace, repo)))
        || runner::cancel(&format!("build:{}", build_key(None, repo)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_skips_only_a_clean_tree_at_the_built_commit() {
        let d = std::env::temp_dir().join(format!("orbit-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let git = |args: &[&str]| {
            let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@t"];
            full.extend_from_slice(args);
            crate::proc::run("git", &full, Some(&d)).unwrap();
        };
        git(&["init"]);
        std::fs::write(d.join("a"), "1").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "x"]);

        let mut cache = HashMap::new();
        assert!(!is_up_to_date(&cache, "k", &d));
        cache.insert("k".into(), CacheEntry { commit: head(&d).unwrap(), built_at: 0 });
        assert!(is_up_to_date(&cache, "k", &d));
        std::fs::write(d.join("a"), "2").unwrap();
        assert!(!is_up_to_date(&cache, "k", &d), "dirty tree rebuilds");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn shell_runs_chained_commands() {
        let out = runner::run_capture(shell("echo one && echo two"), &std::env::temp_dir(), Duration::from_secs(10)).unwrap();
        let lines: Vec<_> = out.lines().map(str::trim).collect();
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn log_names_are_filesystem_safe() {
        assert_eq!(log_file_name("feat/x:y"), "feat_x_y.log");
    }
}
