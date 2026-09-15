// Plan generation: runs a local CLI agent (claude/opencode) non-interactively
// in the workspace root with a prompt built from the linked card, streaming
// its output to the frontend via Tauri events. The agent writes PLAN.md.
use crate::config::AiSettings;
use crate::integrations::{fetch_card, CardDetail, TrackerKind};
use crate::workspace::{workspace_root, CardRef};
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

pub const PLAN_FILE: &str = "PLAN.md";
const PROMPT_PATH: &str = ".config/orbit/plan-prompt.md";
const AGENT_TIMEOUT_SECS: u64 = 300;

/// The currently running plan agent (one at a time), kept so the user can
/// cancel it mid-run.
static ACTIVE_CHILD: Mutex<Option<std::sync::Arc<std::sync::Mutex<Child>>>> = Mutex::new(None);

#[derive(Serialize, Clone)]
struct PlanEvent<'a> {
    status: &'a str, // "line" | "done" | "error"
    line: String,
}

#[derive(Serialize)]
pub struct PlanResult {
    pub plan_path: String,
}

/// Renders the plan prompt. Users can override the template at
/// ~/.config/orbit/plan-prompt.md; the compiled default covers the
/// PLAN.md contract (context + `- [ ]` tasks).
fn build_prompt(
    card: &CardDetail,
    repos: &[String],
    branch: &str,
    ws_name: &str,
    ws_dir: &std::path::Path,
) -> String {
    let default = r#"You are planning a feature that spans multiple repositories.

# Card
Title: {card_title}
Description:
{card_description}

# Workspace
Name: {ws_name}
Branch: {branch}
Repositories (one git worktree each, in subdirectories of the current folder):
{repos}

# Task
Explore the repositories as needed and write a markdown plan to the file `{plan_path}` (absolute path — write EXACTLY this path; do not write anywhere else). The plan must follow this exact structure:

```
# {card_title}

**Card**: [{card_id}]({card_url}) · **Branch**: {branch}

## Context

(3-6 bullets summarizing the card and what you found in the repos)

## Plan

- [ ] <task> — <repo> (one line per task, 4-8 tasks, each naming which repository it touches)
```

Rules:
- Write ONLY that file. Do not modify any repository code.
- Tasks must be concrete, ordered, and each must name the repository it applies to.
- Reply with a single line: the path of the file you wrote."#;

    let template = std::env::var_os("HOME")
        .map(|h| {
            let p = std::path::PathBuf::from(h).join(PROMPT_PATH);
            std::fs::read_to_string(p).ok()
        })
        .flatten()
        .unwrap_or_else(|| default.to_string());

    template
        .replace("{card_title}", &card.title)
        .replace("{card_description}", &card.description)
        .replace("{card_id}", &card.id)
        .replace("{card_url}", &card.url)
        .replace("{branch}", branch)
        .replace("{ws_name}", ws_name)
        .replace(
            "{plan_path}",
            &ws_dir.join(PLAN_FILE).to_string_lossy(),
        )
        .replace(
            "{repos}",
            &repos.iter().map(|r| format!("- `{r}`")).collect::<Vec<_>>().join("\n"),
        )
}

/// Runs the configured agent with the plan prompt in the workspace root,
/// streaming each output line to the frontend as `plan-progress` events.
/// Returns the PLAN.md path on success.
pub fn generate_plan(
    app: &AppHandle,
    ws_name: &str,
    card: &CardRef,
    repos: &[String],
    branch: &str,
    ai: &AiSettings,
) -> Result<PlanResult, String> {
    let ws_dir = workspace_root()?.join("workspaces").join(ws_name);
    if !ws_dir.exists() {
        return Err(format!("workspace '{ws_name}' not found"));
    }

    // Fetch the card's full description for the prompt.
    let kind = TrackerKind::parse(&card.kind)?;
    let detail: CardDetail = fetch_card(kind, &card.id)?;
    let prompt = build_prompt(&detail, repos, branch, ws_name, &ws_dir);

    let mut cmd = match ai.agent.as_str() {
        "opencode" => {
            let mut c = Command::new("opencode");
            // Same rationale as claude's acceptEdits: headless runs can't
            // ask for permission, so auto-approve what isn't denied.
            c.args(["run", "--model", &ai.model, "--auto", &prompt]);
            c
        }
        // default: claude — acceptEdits auto-approves file writes inside the
        // session dir; without it, non-interactive runs deny every write
        // ("I don't have permission to write that file") and PLAN.md is
        // never created.
        _ => {
            let mut c = Command::new("claude");
            c.args([
                "-p",
                "--model",
                &ai.model,
                "--permission-mode",
                "acceptEdits",
                &prompt,
            ]);
            c
        }
    };
    cmd.current_dir(&ws_dir).stdin(Stdio::null());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let emit = |status: &str, line: String| {
        let _ = app.emit("plan-progress", PlanEvent { status, line });
    };

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch {}: {e}", ai.agent))?;

    // Take stdio handles BEFORE registering the child for cancellation.
    let mut stderr = child.stderr.take();
    let stdout = child
        .stdout
        .take()
        .ok_or("no stdout from agent")?;

    // Register for cancellation; only one plan may run at a time.
    let child = {
        let mut guard = ACTIVE_CHILD.lock().unwrap();
        if let Some(existing) = guard.take() {
            if let Ok(mut old) = existing.lock() {
                let _ = old.kill();
            }
        }
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        *guard = Some(std::sync::Arc::clone(&child));
        child
    };

    // Stream stderr on its own thread (opencode prints chrome there).
    let app_err = app.clone();
    std::thread::spawn(move || {
        if let Some(err) = stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = app_err.emit(
                    "plan-progress",
                    PlanEvent {
                        status: "line",
                        line: format!("[stderr] {line}"),
                    },
                );
            }
        }
    });

    // Collect stdout while streaming: if the agent failed to write PLAN.md
    // itself (e.g. permission errors in its sandbox), we salvage the
    // markdown from its reply instead of failing the whole run.
    let mut reply = String::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        reply.push_str(&line);
        reply.push('\n');
        emit("line", line);
    }

    // Wait with timeout: poll try_wait (child shared with cancel_plan).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(AGENT_TIMEOUT_SECS);
    let exit_ok = loop {
        let status = {
            let mut c = child.lock().map_err(|_| "agent state poisoned")?;
            match c.try_wait() {
                Ok(Some(status)) => status,
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = c.kill();
                        return Err("agent timed out".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(format!("failed waiting agent: {e}")),
            }
        };
        break status.success();
    };

    // Unregister (no-op when cancelled: cancel_plan already took it).
    *ACTIVE_CHILD.lock().unwrap() = None;

    if !exit_ok {
        return Err("agent exited with an error (or was cancelled) — see the log above".into());
    }

    let plan_path = ws_dir.join(PLAN_FILE);
    if !plan_path.exists() {
        // The agent sometimes writes PLAN.md inside a repo worktree instead
        // of the workspace root — move it to the expected path.
        for repo in repos {
            let stray = ws_dir.join(repo).join(PLAN_FILE);
            if stray.exists() {
                std::fs::rename(&stray, &plan_path)
                    .map_err(|e| format!("failed to move PLAN.md to workspace root: {e}"))?;
                emit(
                    "line",
                    format!("[orbit] moved PLAN.md from {repo}/ to the workspace root"),
                );
                break;
            }
        }
    }
    if !plan_path.exists() {
        // Salvage: agents sometimes reply with the plan instead of writing
        // the file (permission-restricted sandboxes). Extract the markdown
        // block (``` fences or a "# " heading) from the reply.
        let salvaged = extract_markdown(&reply);
        if salvaged.is_empty() {
            return Err(
                "agent finished but did not write PLAN.md (and its reply had no markdown to salvage)".into(),
            );
        }
        std::fs::write(&plan_path, salvaged)
            .map_err(|e| format!("failed to write salvaged PLAN.md: {e}"))?;
        emit("line", "[orbit] agent didn't write PLAN.md — salvaged it from the reply".into());
    }
    emit("done", plan_path.to_string_lossy().to_string());
    Ok(PlanResult {
        plan_path: plan_path.to_string_lossy().to_string(),
    })
}

/// Cancels the running plan agent, if any.
pub fn cancel_plan() -> Result<(), String> {
    let mut guard = ACTIVE_CHILD.lock().unwrap();
    if let Some(child) = guard.take() {
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
        }
        Ok(())
    } else {
        Err("no plan is running".into())
    }
}

/// Extracts the largest markdown block from an agent reply: prefers a
/// ```markdown fenced block, else everything from the first "# " heading.
fn extract_markdown(reply: &str) -> String {
    if let Some(start) = reply.find("```markdown") {
        let rest = &reply[start + "```markdown".len()..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    if let Some(start) = reply.find("```md") {
        let rest = &reply[start + "```md".len()..];
        if let Some(end) = rest.find("```") {
            return rest[..end].trim().to_string();
        }
    }
    if let Some(pos) = reply.find("\n# ") {
        return reply[pos + 1..].trim().to_string();
    }
    String::new()
}

/// Quick connectivity check for the configured agent.
pub fn test_agent(ai: &AiSettings) -> Result<(), String> {
    let out = match ai.agent.as_str() {
        "opencode" => Command::new("opencode")
            .args(["run", "--model", &ai.model, "Reply with the single word: ok"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
        _ => Command::new("claude")
            .args([
                "-p",
                "--model",
                &ai.model,
                "Reply with the single word: ok",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    };
    let out = out.map_err(|e| format!("failed to launch {}: {e}", ai.agent))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Models known to work with each agent, used as select defaults.
pub fn default_models(agent: &str) -> Vec<String> {
    match agent {
        "opencode" => vec![
            "aihub/aihub/best".into(),
            "aihub/aihub/cheap".into(),
            "aihub/aihub/balanced".into(),
        ],
        _ => vec![
            "claude-sonnet-5".into(),
            "claude-opus-5".into(),
            "claude-haiku-4.5".into(),
        ],
    }
}

/// Lists available models for opencode via its CLI (claude has a fixed set).
pub fn list_models(agent: &str) -> Vec<String> {
    match agent {
        "opencode" => {
            let out = Command::new("opencode")
                .arg("models")
                .stdin(Stdio::null())
                .output();
            match out {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && l.contains('/'))
                    .take(100)
                    .collect(),
                _ => default_models(agent),
            }
        }
        _ => default_models(agent),
    }
}

// ---------- Plan task tracking ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PlanTask {
    pub text: String,
    pub done: bool,
}

/// Parses `- [ ]` / `- [x]` lines from a PLAN.md body.
pub fn parse_tasks(raw: &str) -> Vec<PlanTask> {
    raw.lines().filter_map(|l| {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("- [ ] ") {
            Some(PlanTask { text: rest.trim().to_string(), done: false })
        } else if let Some(rest) = t.strip_prefix("- [x] ").or_else(|| t.strip_prefix("- [X] ")) {
            Some(PlanTask { text: rest.trim().to_string(), done: true })
        } else {
            None
        }
    }).collect()
}

/// Flips the nth `- [ ]` checkbox in a PLAN.md body, returning the new content.
pub fn set_task_in_content(raw: &str, index: usize, done: bool) -> Result<String, String> {
    let mut out: Vec<String> = Vec::new();
    let mut count = 0usize;
    let mut found = false;
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let is_task = trimmed.starts_with("- [ ] ")
            || trimmed.starts_with("- [x] ")
            || trimmed.starts_with("- [X] ");
        if is_task {
            if count == index {
                found = true;
                let indent = &line[..line.len() - trimmed.len()];
                let after = trimmed
                    .strip_prefix("- [ ] ")
                    .or_else(|| trimmed.strip_prefix("- [x] "))
                    .or_else(|| trimmed.strip_prefix("- [X] "))
                    .unwrap_or("");
                let marker = if done { "- [x] " } else { "- [ ] " };
                out.push(format!("{indent}{marker}{}", after.trim_start()));
            } else {
                out.push(line.to_string());
            }
            count += 1;
        } else {
            out.push(line.to_string());
        }
    }
    if !found {
        return Err("task index out of range".into());
    }
    let mut content = out.join("\n");
    if raw.ends_with('\n') {
        content.push('\n');
    }
    Ok(content)
}

/// Tasks from the workspace's PLAN.md (empty when no plan exists).
pub fn plan_tasks(ws_name: &str) -> Result<Vec<PlanTask>, String> {
    let path = workspace_root()?.join("workspaces").join(ws_name).join(PLAN_FILE);
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(parse_tasks(&raw))
}

/// Marks the nth task in the workspace's PLAN.md as done/undone.
pub fn set_plan_task(ws_name: &str, index: usize, done: bool) -> Result<(), String> {
    let path = workspace_root()?.join("workspaces").join(ws_name).join(PLAN_FILE);
    if !path.exists() {
        return Err("no PLAN.md in this workspace".into());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let content = set_task_in_content(&raw, index, done)?;
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}