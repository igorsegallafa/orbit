// AI usage stats per workspace, merged from two local sources:
//  - Claude Code JSONL logs (~/.claude/projects/<cwd-with-dashes>/*.jsonl)
//  - OpenCode's SQLite DB (~/.local/share/opencode/opencode.db)
//
// Folder matching is EXACT (workspace folder + one folder per repo from
// .workspace.yaml) — prefix matching previously leaked sibling workspaces
// with similar names (e.g. "promo" matching "promotion-...").
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone, Debug, Default)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub cost_usd: f64,
    pub sessions: Vec<SessionDetail>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct SessionDetail {
    pub title: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub cost_usd: f64,
    /// Unix seconds of the session's last activity (0 = unknown).
    pub last_active: i64,
}

#[derive(Serialize, Debug, Default)]
pub struct AiUsage {
    pub by_model: Vec<ModelUsage>,
    pub sessions: usize,
}

fn claude_projects_dir() -> Option<PathBuf> {
    Some(crate::config::home_dir().ok()?.join(".claude/projects"))
}

/// Claude Code encodes a project dir by replacing every non-alphanumeric
/// char of the cwd with `-` (`/a/b.c` -> `-a-b-c`, `C:\x` -> `C--x`).
pub(crate) fn dir_to_project_name(p: &std::path::Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Rough ISO-8601 ("2026-09-14T18:59:59.123Z") -> unix seconds.
// ponytail: hand-rolled date math instead of a chrono dependency.
fn iso_to_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        std::str::from_utf8(&b[from..to]).ok()?.parse().ok()
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[derive(Default)]
struct Raw {
    models: Vec<ModelUsage>,
    sessions: std::collections::HashSet<String>,
}

fn add_session(raw: &mut Raw, model: &str, s: SessionDetail) {
    if let Some(m) = raw.models.iter_mut().find(|m| m.model == model) {
        m.input_tokens += s.input_tokens;
        m.output_tokens += s.output_tokens;
        m.cache_tokens += s.cache_tokens;
        m.cost_usd += s.cost_usd;
        m.sessions.push(s);
    } else {
        let mut m = ModelUsage {
            model: model.to_string(),
            input_tokens: s.input_tokens,
            output_tokens: s.output_tokens,
            cache_tokens: s.cache_tokens,
            cost_usd: s.cost_usd,
            sessions: vec![],
        };
        m.sessions.push(s);
        raw.models.push(m);
    }
}

/// Aggregates usage for sessions run in the workspace folder or any of its
/// repo worktrees, merging Claude Code logs and OpenCode's DB.
pub fn workspace_usage(ws_dir: &std::path::Path, repos: &[String]) -> AiUsage {
    let mut raw = claude_usage(ws_dir, repos);
    let oc = opencode_usage(ws_dir, repos);
    for s in oc.sessions {
        raw.sessions.insert(s);
    }
    for mut m in oc.models {
        match raw.models.iter_mut().find(|d| d.model == m.model) {
            Some(d) => {
                d.input_tokens += m.input_tokens;
                d.output_tokens += m.output_tokens;
                d.cache_tokens += m.cache_tokens;
                d.cost_usd += m.cost_usd;
                d.sessions.append(&mut m.sessions);
            }
            None => raw.models.push(m),
        }
    }
    for m in &mut raw.models {
        m.sessions.sort_by_key(|a| std::cmp::Reverse(a.last_active));
    }
    raw.models.sort_by_key(|a| std::cmp::Reverse(a.output_tokens));
    AiUsage {
        by_model: raw.models,
        sessions: raw.sessions.len(),
    }
}

/// OpenCode: SQLite DB with pre-aggregated usage per session.
// ponytail: sqlite3 CLI instead of a Rust sqlite crate; if the DB grows huge
// or the CLI is missing, swap for rusqlite.
fn opencode_usage(ws_dir: &std::path::Path, repos: &[String]) -> Raw {
    let mut raw = Raw::default();
    let Ok(home) = crate::config::home_dir() else {
        return raw;
    };
    let db = home.join(".local/share/opencode/opencode.db");
    if !db.exists() {
        return raw;
    }

    let mut dirs: Vec<String> = vec![ws_dir.to_string_lossy().to_string()];
    for repo in repos {
        dirs.push(ws_dir.join(repo).to_string_lossy().to_string());
    }
    // Trailing "/%" disambiguates prefixes (no sibling-name leaks).
    let filter = dirs
        .iter()
        .map(|d| format!("directory = '{d}' OR directory LIKE '{d}/%'"))
        .collect::<Vec<_>>()
        .join(" OR ");

    let sql = format!(
        "SELECT model, title, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, \
         cost, time_updated, id \
         FROM session WHERE ({filter}) AND model IS NOT NULL"
    );
    let Ok(out) = crate::proc::cmd("sqlite3")
        .arg("-json")
        .arg(&db)
        .arg(&sql)
        .output()
    else {
        return raw;
    };
    if !out.status.success() {
        return raw;
    }
    let rows: Vec<serde_json::Value> = match serde_json::from_slice(&out.stdout) {
        Ok(r) => r,
        Err(_) => return raw,
    };

    let num = |v: &serde_json::Value| -> u64 {
        v.as_i64().unwrap_or(0).max(0) as u64
    };
    for row in rows {
        // OpenCode stores model as a JSON string like
        // {"id":"aihub/glm-5.3-flash","providerID":"aihub"} — extract the id.
        let model = row
            .get("model")
            .and_then(|m| m.as_str())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|j| j.get("id").and_then(|i| i.as_str()).map(String::from))
            .or_else(|| {
                row.get("model")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "opencode".into());
        let title = row
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("opencode session")
            .to_string();
        let inp = row.get("tokens_input").map(&num).unwrap_or(0);
        let outp = row.get("tokens_output").map(&num).unwrap_or(0);
        let cache = row.get("tokens_cache_read").map(&num).unwrap_or(0)
            + row.get("tokens_cache_write").map(&num).unwrap_or(0);
        let cost = row.get("cost").and_then(|c| c.as_f64()).unwrap_or(0.0);
        let last_active = row.get("time_updated").and_then(|t| t.as_i64()).unwrap_or(0) / 1000;

        if let Some(id) = row.get("id").and_then(|i| i.as_str()) {
            raw.sessions.insert(format!("oc:{id}"));
        }
        add_session(
            &mut raw,
            &model,
            SessionDetail {
                title,
                model: model.clone(),
                input_tokens: inp,
                output_tokens: outp,
                cache_tokens: cache,
                cost_usd: cost,
                last_active,
            },
        );
    }
    raw
}

/// Claude Code: JSONL logs, one SessionDetail per .jsonl file.
fn claude_usage(ws_dir: &std::path::Path, repos: &[String]) -> Raw {
    let mut raw = Raw::default();
    let Some(projects) = claude_projects_dir() else {
        return raw;
    };

    let mut valid: Vec<String> = vec![dir_to_project_name(ws_dir)];
    for repo in repos {
        valid.push(dir_to_project_name(&ws_dir.join(repo)));
    }

    for name in &valid {
        let dir = projects.join(name);
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            if f.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            raw.sessions.insert(f.path().to_string_lossy().to_string());
            let Ok(content) = std::fs::read_to_string(f.path()) else {
                continue;
            };

            // Aggregate this jsonl file into one session detail.
            let mut det = SessionDetail {
                title: f
                    .path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                model: String::new(),
                ..Default::default()
            };
            for line in content.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if t == "ai-title" {
                    // Prefer the human title Claude generates for the chat.
                    if let Some(title) = v.get("aiTitle").and_then(|x| x.as_str()) {
                        det.title = title.to_string();
                    }
                    continue;
                }
                if t != "assistant" {
                    continue;
                }
                let Some(msg) = v.get("message") else { continue };
                let model = msg
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown");
                if det.model.is_empty() {
                    det.model = model.to_string();
                }
                let Some(u) = msg.get("usage") else { continue };
                det.input_tokens += u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                det.output_tokens += u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                det.cache_tokens += u
                    .get("cache_read_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
                    + u.get("cache_creation_input_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                if let Some(ts) = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(iso_to_epoch)
                {
                    det.last_active = det.last_active.max(ts);
                }
            }
            if det.output_tokens > 0 || det.input_tokens > 0 {
                let model = if det.model.is_empty() {
                    "claude".into()
                } else {
                    det.model.clone()
                };
                add_session(&mut raw, &model, det);
            }
        }
    }
    raw
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_matches_claude_encoding_on_both_platforms() {
        use std::path::Path;
        assert_eq!(dir_to_project_name(Path::new("/Users/x/my.repo")), "-Users-x-my-repo");
        assert_eq!(dir_to_project_name(Path::new(r"C:\Users\igorc\orbit")), "C--Users-igorc-orbit");
    }
}
