// Ralph's task file (scripts/ralph/prd.json) and progress log, in the same
// format as the machina-workspace ralph.sh so existing runs keep working.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Story {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub passes: bool,
    #[serde(default)]
    pub notes: String,
    /// Fields written by other tools survive a round trip.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Prd {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub branch_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub user_stories: Vec<Story>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Prd {
    /// The story Ralph works on next: lowest priority number not passing.
    pub fn next_story(&self) -> Option<&Story> {
        self.user_stories
            .iter()
            .filter(|s| !s.passes)
            .min_by_key(|s| s.priority)
    }

    pub fn passed(&self) -> usize {
        self.user_stories.iter().filter(|s| s.passes).count()
    }

    pub fn all_passed(&self) -> bool {
        !self.user_stories.is_empty() && self.user_stories.iter().all(|s| s.passes)
    }

    /// Ids that pass in `self` but did not in `before`.
    pub fn newly_passed(&self, before: &Prd) -> Vec<String> {
        self.user_stories
            .iter()
            .filter(|s| s.passes)
            .filter(|s| !before.user_stories.iter().any(|b| b.id == s.id && b.passes))
            .map(|s| s.id.clone())
            .collect()
    }
}

pub fn ralph_dir(repo_dir: &Path) -> PathBuf {
    repo_dir.join("scripts").join("ralph")
}

pub fn prd_path(repo_dir: &Path) -> PathBuf {
    ralph_dir(repo_dir).join("prd.json")
}

pub fn progress_path(repo_dir: &Path) -> PathBuf {
    ralph_dir(repo_dir).join("progress.txt")
}

pub fn read(repo_dir: &Path) -> Result<Option<Prd>, String> {
    let p = prd_path(repo_dir);
    if !p.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| format!("invalid {}: {e}", p.display()))
}

pub fn write(repo_dir: &Path, prd: &Prd) -> Result<(), String> {
    std::fs::create_dir_all(ralph_dir(repo_dir)).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(prd).map_err(|e| e.to_string())?;
    std::fs::write(prd_path(repo_dir), raw + "\n").map_err(|e| e.to_string())
}

/// Creates progress.txt with its header when missing.
pub fn ensure_progress(repo_dir: &Path, now: &str) -> Result<(), String> {
    let p = progress_path(repo_dir);
    if p.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(ralph_dir(repo_dir)).map_err(|e| e.to_string())?;
    std::fs::write(&p, format!("# Ralph Progress Log\nStarted: {now}\n---\n")).map_err(|e| e.to_string())
}

pub fn read_progress(repo_dir: &Path) -> String {
    std::fs::read_to_string(progress_path(repo_dir)).unwrap_or_default()
}

/// Moves a previous feature's prd.json/progress.txt into
/// scripts/ralph/archive/<date>-<branch>/ before a different feature's PRD
/// replaces them (same rule as ralph.sh).
pub fn archive_if_other_branch(repo_dir: &Path, new_branch: &str, date: &str) -> Result<Option<PathBuf>, String> {
    let Some(old) = read(repo_dir)? else { return Ok(None) };
    if old.branch_name.is_empty() || old.branch_name == new_branch {
        return Ok(None);
    }
    let slug = old
        .branch_name
        .trim_start_matches("ralph/")
        .trim_start_matches("feat/")
        .replace('/', "-");
    let dest = ralph_dir(repo_dir).join("archive").join(format!("{date}-{slug}"));
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    std::fs::copy(prd_path(repo_dir), dest.join("prd.json")).map_err(|e| e.to_string())?;
    if progress_path(repo_dir).exists() {
        std::fs::rename(progress_path(repo_dir), dest.join("progress.txt")).map_err(|e| e.to_string())?;
    }
    Ok(Some(dest))
}

/// Normalizes an agent-written PRD: sequential ids when missing, priority
/// defaults to document order, the workspace branch as branchName.
pub fn normalize(prd: &mut Prd, branch: &str) {
    prd.branch_name = branch.to_string();
    for (i, s) in prd.user_stories.iter_mut().enumerate() {
        if s.id.trim().is_empty() {
            s.id = format!("US-{:03}", i + 1);
        }
        if s.priority == 0 {
            s.priority = i as i64 + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{
      "project": "MyApp",
      "branchName": "feat/task-priority",
      "description": "Global rules",
      "userStories": [
        {"id": "US-002", "title": "B", "priority": 2, "passes": false, "notes": "", "acceptanceCriteria": ["Typecheck passes"], "description": "b"},
        {"id": "US-001", "title": "A", "priority": 1, "passes": true, "notes": "done", "acceptanceCriteria": [], "description": "a", "owner": "x"}
      ]
    }"#;

    #[test]
    fn parses_ralph_format_and_keeps_unknown_fields() {
        let prd: Prd = serde_json::from_str(EXAMPLE).unwrap();
        assert_eq!(prd.next_story().unwrap().id, "US-002");
        assert_eq!(prd.passed(), 1);
        assert!(!prd.all_passed());
        let out = serde_json::to_value(&prd).unwrap();
        assert_eq!(out["userStories"][1]["owner"], "x");
        assert_eq!(out["branchName"], "feat/task-priority");
    }

    #[test]
    fn newly_passed_diffs_by_id() {
        let before: Prd = serde_json::from_str(EXAMPLE).unwrap();
        let mut after = before.clone();
        after.user_stories[0].passes = true;
        assert_eq!(after.newly_passed(&before), ["US-002"]);
        assert!(after.all_passed());
    }

    #[test]
    fn archives_a_previous_features_run() {
        let d = std::env::temp_dir().join(format!("orbit-prd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let prd: Prd = serde_json::from_str(EXAMPLE).unwrap();
        write(&d, &prd).unwrap();
        ensure_progress(&d, "now").unwrap();
        assert!(archive_if_other_branch(&d, "feat/task-priority", "2026-01-01").unwrap().is_none());
        let dest = archive_if_other_branch(&d, "feat/other", "2026-01-01").unwrap().unwrap();
        assert!(dest.ends_with("2026-01-01-task-priority"));
        assert!(dest.join("prd.json").exists() && dest.join("progress.txt").exists());
        assert!(!progress_path(&d).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
