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

// ---------- Interactive grill (UI stepper) ----------

#[derive(Serialize, Clone, Debug)]
pub struct GrillOption {
    pub label: String,
    #[serde(default)]
    pub description: String, // the tradeoff
    #[serde(default)]
    pub recommended: bool, // agent's pick, first among equals
}

#[derive(Serialize, Clone, Debug)]
pub struct GrillQuestion {
    pub id: String, // q1, q2, ... stable within the round
    pub text: String,
    #[serde(default)]
    pub options: Vec<GrillOption>, // 3 concrete options, recommended first
}

#[derive(Serialize, Clone, Debug)]
pub struct GrillRound {
    pub done: bool,      // true = interview finished, summary field set
    pub questions: Vec<GrillQuestion>,
    #[serde(default)]
    pub summary: String, // final decision summary when done
}

/// One round of the interview: the agent sees the card + the answers so far
/// and must reply with ONLY a JSON object. Headless claude call.
pub fn grill_round(
    ws_name: &str,
    card: &CardRef,
    answers: &[(String, String)], // (questionId, answer) pairs, oldest first
    rounds_done: usize,           // completed rounds so far (for the cap)
    max_rounds: Option<usize>,    // user cap; None = interview until settled
) -> Result<GrillRound, String> {
    let ws_dir = workspace_root()?.join("workspaces").join(ws_name);
    if !ws_dir.exists() {
        return Err(format!("workspace '{ws_name}' not found"));
    }
    let meta = crate::workspace::load_meta(&ws_dir)?;
    let kind = TrackerKind::parse(&card.kind)?;
    let detail = fetch_card(kind, &card.id)?;

    let history = if answers.is_empty() {
        "None yet — this is round 1.".to_string()
    } else {
        answers
            .iter()
            .map(|(q, a)| format!("- Q: {q}\n  A: {a}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

let cap_note = match max_rounds {
        // The upcoming round IS the last allowed one — force a wrap-up.
        Some(max) if rounds_done + 1 >= max => format!(
            "\\n# HARD LIMIT\\nThis is the LAST and FINAL round (the developer capped the interview at {} round(s)). Ask at most 2 final clarification questions. When you reply, you MUST set done=true with a summary of everything decided so far.",
            max
        ),
        Some(max) => format!(
            "\\n# Round budget\\nRound {} of {}. Wrap up early if the essentials are settled.",
            rounds_done + 1, max
        ),
        None => String::new(),
    };

    let prompt = format!(
        r#"You are interviewing a developer to sharpen a plan (grill-me style).

# Feature
Title: {title}
Description:
{description}

# Repositories
{repos}

# Answers so far
{history}
{cap_note}

# Your job
Interview the developer grilling-style, working a DECISION TREE: every decision branches into the decisions that hang off it. Each round, ask the whole FRONTIER — every question whose prerequisites are already settled by the answers above (3-6 questions). Never ask something already answered, and never a question that hinges on an answer you haven't heard.

Focus on behaviors, contracts, edge cases, failure modes and cross-repo impact the developer may have silently assumed.

Each question MUST offer exactly 3 concrete options, your RECOMMENDED one first (recommended=true), each with a short label plus a description carrying the tradeoff. Facts are YOUR job: if a question needs a fact from the code, don't ask — assume the repo investigation and phrase the options accordingly.

If EVERYTHING important has been settled (the frontier is empty), set done=true instead and write a summary of the decisions (5-10 bullets).

Reply with ONLY this JSON, no prose before or after:
{{"done": false, "questions": [{{"id": "q1", "text": "…?", "options": [{{"label": "SQLite", "description": "…tradeoff…", "recommended": true}}, {{"label": "Postgres", "description": "…"}}, {{"label": "Files", "description": "…"}}]}}]}}
or, when finished:
{{"done": true, "summary": "- decision 1\n- decision 2", "questions": []}}"#,
        title = detail.title,
        description = detail.description,
        repos = meta
            .repos
            .iter()
            .map(|r| format!("- `{r}`"))
            .collect::<Vec<_>>()
            .join("\n"),
        history = history,
        cap_note = cap_note,
    );

    let ai = crate::config::Config::load()?.ai;
    let out = match ai.agent.as_str() {
        "opencode" => Command::new("opencode")
            .args(["run", "--model", &ai.model, &prompt])
            .current_dir(&ws_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("failed to launch opencode: {e}"))?,
        "omp" => Command::new("omp")
            .args(["-p", "--auto-approve", "--model", &ai.model, &prompt])
            .current_dir(&ws_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("failed to launch omp: {e}"))?,
        _ => Command::new("claude")
            .args(["-p", "--model", &ai.model, &prompt])
            .current_dir(&ws_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("failed to launch claude: {e}"))?,
    };
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let reply = String::from_utf8_lossy(&out.stdout);
    parse_grill_json(&reply)
}

/// Extracts the first JSON object from the agent reply and shapes it.
/// Tolerates both the new option objects ({label, description, recommended})
/// and the legacy plain-string options.
fn parse_grill_json(reply: &str) -> Result<GrillRound, String> {
    let start = reply.find('{').ok_or("agent replied without JSON")?;
    let end = reply.rfind('}').ok_or("agent reply has no closing brace")?;
    let slice = &reply[start..=end];
    let v: serde_json::Value = serde_json::from_str(slice)
        .map_err(|e| format!("invalid interview JSON: {e}"))?;
    let done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let mut questions = Vec::new();
    if let Some(arr) = v.get("questions").and_then(|q| q.as_array()) {
        for (i, q) in arr.iter().enumerate() {
            let text = q.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            if text.is_empty() {
                continue;
            }
            let id = q
                .get("id")
                .and_then(|i| i.as_str())
                .map(String::from)
                .unwrap_or_else(|| format!("q{}", i + 1));
            let options: Vec<GrillOption> = q
                .get("options")
                .and_then(|o| o.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| {
                            if let Some(obj) = s.as_object() {
                                Some(GrillOption {
                                    label: obj
                                        .get("label")
                                        .and_then(|l| l.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    description: obj
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    recommended: obj
                                        .get("recommended")
                                        .and_then(|r| r.as_bool())
                                        .unwrap_or(false),
                                })
                            } else {
                                s.as_str().map(|plain| GrillOption {
                                    label: plain.to_string(),
                                    description: String::new(),
                                    recommended: false,
                                })
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            questions.push(GrillQuestion { id, text, options });
        }
    }
    if !done && questions.is_empty() {
        return Err("agent finished the round with no questions and no summary".into());
    }
    Ok(GrillRound {
        done,
        questions,
        summary,
    })
}

/// Generates PLAN.md incorporating the interview decisions (headless, same
/// contract as generate_plan but with the summary injected as context).
pub fn generate_plan_with_decisions(
    app: &AppHandle,
    ws_name: &str,
    card: &CardRef,
    decisions: &str,
) -> Result<PlanResult, String> {
    let ws_dir = workspace_root()?.join("workspaces").join(ws_name);
    if !ws_dir.exists() {
        return Err(format!("workspace '{ws_name}' not found"));
    }
    let meta = crate::workspace::load_meta(&ws_dir)?;
    let kind = TrackerKind::parse(&card.kind)?;
    let detail = fetch_card(kind, &card.id)?;

    let prompt = format!(
        r#"Write the implementation plan for the feature below. The developer was interviewed; their confirmed decisions follow. Honor them.

# Feature
Title: {title}
Description:
{description}

# Repositories
{repos}

# Confirmed decisions from the interview
{decisions}

# Task
Explore the repositories as needed and write a markdown plan to `{plan_path}` (absolute path — write EXACTLY this path). Follow this structure:

```
# {title}

**Card**: [{id}]({url}) · **Branch**: {branch}

## Context

(3-6 bullets: card summary + the interview decisions)

## Plan

- [ ] <task> — <repo> (4-8 concrete, ordered tasks, each naming its repository)
```

Write ONLY that file. Do not modify repository code. Reply with the path you wrote."#,
        title = detail.title,
        description = detail.description,
        repos = meta
            .repos
            .iter()
            .map(|r| format!("- `{r}`"))
            .collect::<Vec<_>>()
            .join("\n"),
        decisions = decisions,
        plan_path = ws_dir.join(PLAN_FILE).display(),
        id = detail.id,
        url = detail.url,
        branch = meta.branch,
    );

    let ai = crate::config::Config::load()?.ai;
    let mut cmd = match ai.agent.as_str() {
        "opencode" => {
            let mut c = Command::new("opencode");
            // Same rationale as generate_plan: headless runs can't ask for
            // permission, so auto-approve what isn't denied.
            c.args(["run", "--model", &ai.model, "--auto", &prompt]);
            c
        }
        "omp" => {
            let mut c = Command::new("omp");
            // Headless + writes PLAN.md: print mode + auto-approve.
            c.args(["-p", "--auto-approve", "--model", &ai.model, &prompt]);
            c
        }
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
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch {}: {e}", ai.agent))?;
    let mut stderr = child.stderr.take();
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
    let stdout = child.stdout.take().ok_or("no stdout from agent")?;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let _ = app.emit("plan-progress", PlanEvent { status: "line", line });
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(AGENT_TIMEOUT_SECS);
    let exit_ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    return Err("agent timed out".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(format!("failed waiting agent: {e}")),
        }
    };
    if !exit_ok {
        return Err("agent exited with an error — see the log above".into());
    }

    let plan_path = ws_dir.join(PLAN_FILE);
    let mut reply = String::new();
    let _ = &mut reply;
    if !plan_path.exists() {
        return Err("agent finished but did not write PLAN.md".into());
    }
    let _ = app.emit(
        "plan-progress",
        PlanEvent {
            status: "done",
            line: plan_path.to_string_lossy().to_string(),
        },
    );
    Ok(PlanResult {
        plan_path: plan_path.to_string_lossy().to_string(),
    })
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
        "omp" => {
            let mut c = Command::new("omp");
            c.args(["-p", "--auto-approve", "--model", &ai.model, &prompt]);
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
        "omp" => Command::new("omp")
            .args(["-p", "--model", &ai.model, "Reply with the single word: ok"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
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
        "omp" => vec![
            "aihub/glm-5.3".into(),
            "aihub/cheap".into(),
            "aihub/balanced".into(),
        ],
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
        "omp" => {
            // `omp models` renders a table with box-drawing chars; rows are
            // "│ model │ ctx │ max-out │ thinking │ images │" — the model id
            // is the first cell.
            let out = Command::new("omp")
                .arg("models")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            match out {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|l| l.contains('│') && l.contains('/'))
                    .filter_map(|l| {
                        l.split('│')
                            .map(|c| c.trim())
                            .find(|c| c.contains('/') && !c.contains(' '))
                            .map(String::from)
                    })
                    .take(100)
                    .collect(),
                _ => default_models(agent),
            }
        }
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
/// Renders the interactive "grill" interview prompt (Orca-adjacent
/// /grill-me style): the agent interviews the user in rounds about the
/// card, then writes PLAN.md. Runs in an interactive terminal session.
pub fn build_grill_prompt(
    card: &CardDetail,
    repos: &[String],
    branch: &str,
    ws_name: &str,
    ws_dir: &std::path::Path,
) -> String {
    let plan_abs = ws_dir.join(PLAN_FILE);
    format!(
        r#"We are planning the feature below. You will interview me (grill me) before writing the plan.

# Card
Title: {title}
Description:
{description}

# Workspace
Name: {ws_name}
Branch: {branch}
Repositories (git worktrees in subdirectories of the current folder):
{repos}

# How to run this session
1. Ask questions in ROUNDS: each round, ask every question you can ask with what you already know (never a question that hinges on an answer you haven't heard). Wait for my answers before the next round.
2. One question at a time per message is fine; keep rounds short (3-6 questions).
3. Focus on behaviors, contracts, edge cases, failure modes, and cross-repo impact I may have silently assumed.
4. If I say "I don't know", propose 2-3 options with trade-offs instead of rephrasing.
5. Stop when the frontier is empty: nothing left silently assumed. Then confirm a short summary of decisions.
6. AFTER I confirm, write the plan to `{plan_path}` (absolute path, write EXACTLY this path) following this structure:

# {title}

**Card**: [{id}]({url}) · **Branch**: {branch}

## Context

(3-6 bullets: card summary + decisions from the interview)

## Plan

- [ ] <task> — <repo> (4-8 concrete, ordered tasks, each naming its repository)

Do not modify repository code. Only write the plan file after I confirm the decisions."#,
        title = card.title,
        description = card.description,
        ws_name = ws_name,
        branch = branch,
        repos = repos.iter().map(|r| format!("- `{r}`")).collect::<Vec<_>>().join("\n"),
        plan_path = plan_abs.display(),
        id = card.id,
        url = card.url,
    )
}

/// Fetches card detail and renders the grill prompt (for the interactive
/// interview session opened by the Plan modal).
pub fn grill_prompt(ws_name: &str, card: &CardRef) -> Result<String, String> {
    let ws_dir = workspace_root()?.join("workspaces").join(ws_name);
    if !ws_dir.exists() {
        return Err(format!("workspace '{ws_name}' not found"));
    }
    let meta = crate::workspace::load_meta(&ws_dir)?;
    let kind = TrackerKind::parse(&card.kind)?;
    let detail = fetch_card(kind, &card.id)?;
    Ok(build_grill_prompt(
        &detail,
        &meta.repos,
        &meta.branch,
        ws_name,
        &ws_dir,
    ))
}

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