// Plan generation: runs a local CLI agent (claude/opencode) non-interactively
// in the workspace root with a prompt built from the linked card, streaming
// its output to the frontend via Tauri events. The agent writes PLAN.md.
mod cmd;
pub mod runner;

pub use cmd::{agent_cmd, Access};
use runner::Line;
use crate::config::AiSettings;
use crate::integrations::{fetch_card, CardDetail, TrackerKind};
use crate::workspace::CardRef;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const PLAN_FILE: &str = "PLAN.md";
const PROMPT_PATH: &str = ".config/orbit/plan-prompt.md";
const AGENT_TIMEOUT_SECS: u64 = 300;
/// Headless one-shot answers (commit messages, PR drafts): shorter leash
/// than the plan agent — these are moments in a UI flow, not background
/// jobs.
const DRAFT_TIMEOUT_SECS: u64 = 120;

/// Run id of the (single) plan agent, for cancellation.
const PLAN_RUN: &str = "plan";

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
    let ws_dir = crate::workspace::ws_dir(ws_name)?;
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
    let reply = runner::run_capture(
        agent_cmd(&ai, &prompt, Access::ReadOnly, false),
        &ws_dir,
        Duration::from_secs(AGENT_TIMEOUT_SECS),
    )?;
    parse_grill_json(&reply)
}

/// Extracts the first JSON object from the agent reply and shapes it.
/// Tolerates both the new option objects ({label, description, recommended})
/// and the legacy plain-string options.
pub(crate) fn parse_grill_json(reply: &str) -> Result<GrillRound, String> {
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
    let ws_dir = crate::workspace::ws_dir(ws_name)?;
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
    run_plan_agent(app, &ws_dir, &meta.repos, &prompt, &ai)
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

    let template = crate::config::home_dir()
        .ok()
        .and_then(|h| std::fs::read_to_string(h.join(PROMPT_PATH)).ok())
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
    let ws_dir = crate::workspace::ws_dir(ws_name)?;
    if !ws_dir.exists() {
        return Err(format!("workspace '{ws_name}' not found"));
    }

    // Fetch the card's full description for the prompt.
    let kind = TrackerKind::parse(&card.kind)?;
    let detail: CardDetail = fetch_card(kind, &card.id)?;
    let prompt = build_prompt(&detail, repos, branch, ws_name, &ws_dir);

    run_plan_agent(app, &ws_dir, repos, &prompt, ai)
}

/// Streams the plan agent (cancellable as PLAN_RUN) and makes sure PLAN.md
/// ends up at the workspace root: moved from a repo dir or salvaged from
/// the reply when the agent didn't write it where asked.
fn run_plan_agent(
    app: &AppHandle,
    ws_dir: &Path,
    repos: &[String],
    prompt: &str,
    ai: &AiSettings,
) -> Result<PlanResult, String> {
    let emit = |status: &str, line: String| {
        let _ = app.emit("plan-progress", PlanEvent { status, line });
    };
    let outcome = runner::run_streaming(
        agent_cmd(ai, prompt, Access::Edit, false),
        ws_dir,
        Duration::from_secs(AGENT_TIMEOUT_SECS),
        Some(PLAN_RUN),
        |line| match line {
            Line::Out(l) => emit("line", l),
            Line::Err(l) => emit("line", format!("[stderr] {l}")),
        },
    )?;
    if !outcome.success {
        return Err("agent exited with an error (or was cancelled) — see the log above".into());
    }
    let reply = outcome.stdout;

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
    if runner::cancel(PLAN_RUN) {
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
    runner::run_capture(
        agent_cmd(ai, "Reply with the single word: ok", Access::ReadOnly, false),
        &std::env::temp_dir(),
        Duration::from_secs(DRAFT_TIMEOUT_SECS),
    )
    .map(|_| ())
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
            "claude-haiku-4-5".into(),
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
            let out = crate::proc::cmd("omp")
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
            let out = crate::proc::cmd("opencode")
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
        } else { t.strip_prefix("- [x] ").or_else(|| t.strip_prefix("- [X] ")).map(|rest| PlanTask { text: rest.trim().to_string(), done: true }) }
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
    let ws_dir = crate::workspace::ws_dir(ws_name)?;
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
    let path = crate::workspace::ws_dir(ws_name)?.join(PLAN_FILE);
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(parse_tasks(&raw))
}

/// Marks the nth task in the workspace's PLAN.md as done/undone.
pub fn set_plan_task(ws_name: &str, index: usize, done: bool) -> Result<(), String> {
    let path = crate::workspace::ws_dir(ws_name)?.join(PLAN_FILE);
    if !path.exists() {
        return Err("no PLAN.md in this workspace".into());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let content = set_task_in_content(&raw, index, done)?;
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}
// ---------- Workspace pipeline: commit messages & conflict resolution ----------

/// Runs the configured agent headless with `prompt` in `dir`, no write
/// permissions needed (read-only answer, may explore the repo).
fn agent_answer(dir: &Path, prompt: &str) -> Result<String, String> {
    let ai = crate::config::Config::load()?.ai;
    with_retry(|| answer_once(&ai, dir, prompt, Access::ReadOnly))
}

/// One-shot draft whose prompt already carries all the context: the fast
/// model, no tools, no MCP servers, so it's a single model call instead of
/// an agent exploring the repo turn by turn.
fn draft_answer(dir: &Path, prompt: &str) -> Result<String, String> {
    let mut ai = crate::config::Config::load()?.ai;
    ai.model = ai.draft_model();
    with_retry(|| answer_once(&ai, dir, prompt, Access::Answer))
}

/// One retry: agent CLIs fail transiently (auth refresh, network blip).
fn with_retry(run: impl Fn() -> Result<String, String>) -> Result<String, String> {
    run().or_else(|first| run().map_err(|second| format!("{first} (retried: {second})")))
}

fn answer_once(ai: &AiSettings, dir: &Path, prompt: &str, access: Access) -> Result<String, String> {
    // Hard deadline: agent CLIs can hang and the UI spinner would run
    // forever. 2 min covers real repo exploration (15-60s typical).
    runner::run_capture(agent_cmd(ai, prompt, access, false), dir, Duration::from_secs(DRAFT_TIMEOUT_SECS))
}

/// Output of a read-only git command, "" when it fails (no upstream yet,
/// shallow clone): drafts still work from whatever context is left.
fn git_out(dir: &Path, args: &[&str]) -> String {
    crate::proc::run("git", args, Some(dir)).unwrap_or_default().trim().to_string()
}

/// `text` cut to `max` chars, marking the cut so the model knows the rest exists.
fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}\n… (truncated)", &text[..at]),
        None => text.to_string(),
    }
}

/// Diff bodies sent with drafts: enough for the model to see what changed,
/// bounded so a huge refactor doesn't blow up the prompt (and the latency).
const DRAFT_DIFF_CHARS: usize = 24_000;

/// One line per changed file, untracked ones included (a numstat diff
/// against HEAD leaves new files out entirely).
// ponytail: stats only, no patch body, keeps the prompt small; send the
// diff when messages come out too generic.
fn change_summary(dir: &Path) -> Result<String, String> {
    Ok(crate::git::changes(dir)?
        .iter()
        .map(|c| format!("{} +{} -{} {}", c.status, c.added, c.deleted, c.path))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[derive(Serialize, Clone, Debug)]
pub struct CommitMsg {
    pub repo: String,
    pub message: String,
}

/// AI-generated commit message for one dirty repo of a workspace. The
/// agent sees the numstat diff (bounded) and answers with the message
/// only — no JSON to parse, the message IS the reply.
pub fn commit_message(ws_name: &str, repo: &str) -> Result<CommitMsg, String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let numstat = change_summary(&dir)?;
    if numstat.is_empty() {
        return Err(format!("{repo}: nothing to commit"));
    }
    let branch = crate::git::current_branch(&dir).unwrap_or_default();
    let diff = clip(&git_out(&dir, &["diff", "HEAD", "--no-color", "--no-ext-diff", "--unified=2"]), DRAFT_DIFF_CHARS);
    let prompt = format!(
        r#"You are writing a git commit message for the repository `{repo}` (branch `{branch}`). Everything you need is below: answer directly, do not read files or run commands.

Changed files (status, +added -removed, path; U = new file):
{numstat}

Diff of tracked files (new files are only listed above):
{diff}

Write ONE single-line commit message in conventional-commit style: up to 72 chars, starting with a type (feat/fix/chore/refactor/docs/test) followed by a lowercase summary. Describe WHAT changed, not that files changed.

STRICT RULES:
- Plain text only. NO markdown, no backticks, no bold, no bullet points, no code blocks.
- ONE line. No body, no trailing newline junk.
- No scope prefix, no AI attribution, no quotes around the message.

Reply with ONLY the message itself."#
    );
    let message = draft_answer(&dir, &prompt)?;
    // Strip markdown artifacts the models still slip in (backticks, bold,
    // quotes) and collapse to the first non-empty line.
    let message = clean_commit_message(&message);
    if message.is_empty() {
        return Err(format!("{repo}: agent returned an empty message"));
    }
    Ok(CommitMsg {
        repo: repo.to_string(),
        message,
    })
}

/// Collapses the agent reply to a single clean line: drops markdown
/// artifacts (code fences, bold, bullets, quotes) and takes the first
/// non-empty line.
fn clean_commit_message(raw: &str) -> String {
    // First line that isn't empty or a lone code fence.
    let line = raw
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && *l != "```")
        .unwrap_or("");
    let line = line.trim_start_matches("```").trim();
    let line = line
        .trim_start_matches(['#', '-', '*', '>', '"'])
        .trim_end_matches(['*', '"', '`'])
        .trim();
    line.trim_start_matches(['-', '*']).trim().to_string()
}

/// Lets the configured agent resolve rebase conflicts inside the worktree.
/// The agent edits files freely (acceptEdits / --auto) and stages them;
/// `git rebase --continue` stays with us so a failed agent never lands
/// half of a resolution. Returns the agent's summary.
pub fn resolve_conflicts(ws_name: &str, repo: &str) -> Result<String, String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let conflicts = crate::git::conflicted_files(&dir);
    if conflicts.is_empty() {
        return Err(format!("{repo}: no conflicts to resolve"));
    }
    let list = conflicts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n");
    let prompt = format!(
        r#"A git operation (rebase, merge, cherry-pick or revert) is paused in this worktree with the following conflicts:

{list}

Resolve every conflict keeping the intent of both sides (the branch checked out here and the changes being applied), then `git add` each resolved file. Do NOT continue the operation (no `--continue`) or `git commit` — staging is enough. Do not touch anything else.

Reply with a short summary of how you resolved each file."#
    );

    let summary = run_edit_agent(&dir, &prompt)?;
    // Sanity check: agent must have left no unmerged paths behind.
    let left = crate::git::conflicted_files(&dir);
    if !left.is_empty() {
        return Err(format!(
            "{}: agent finished but {} file(s) still conflicted",
            repo,
            left.len()
        ));
    }
    Ok(summary)
}

#[cfg(test)]
mod commit_msg_tests {
    use super::clean_commit_message;

    #[test]
    fn strips_markdown_artifacts() {
        assert_eq!(
            clean_commit_message("  \nfeat: add retry to client  "),
            "feat: add retry to client"
        );
        assert_eq!(clean_commit_message("```feat: add retry```"), "feat: add retry");
        assert_eq!(clean_commit_message("**feat: add retry**"), "feat: add retry");
        assert_eq!(clean_commit_message("\"feat: add retry\""), "feat: add retry");
        assert_eq!(clean_commit_message("- **feat: add retry**"), "feat: add retry");
        assert_eq!(clean_commit_message("```\nfeat: add retry\n```"), "feat: add retry");
        assert_eq!(clean_commit_message("> feat: add retry"), "feat: add retry");
    }
}

// ---------- Workspace pipeline: PR title/description drafting ----------

#[derive(Serialize, Clone, Debug)]
pub struct PrDraft {
    pub title: String,
    pub body: String,
}

/// Where repos keep their PR template; the first one found wins.
const PR_TEMPLATE_FILES: &[&str] = &[
    ".github/pull_request_template.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    "pull_request_template.md",
    "PULL_REQUEST_TEMPLATE.md",
    "docs/pull_request_template.md",
    "docs/PULL_REQUEST_TEMPLATE.md",
];

/// The repo's PR template: a known file, else the first .md in a
/// .github/pull_request_template/ directory (multiple-templates layout).
fn pr_template(dir: &Path) -> Option<String> {
    let read = |p: &Path| std::fs::read_to_string(p).ok().filter(|t| !t.trim().is_empty());
    if let Some(t) = PR_TEMPLATE_FILES.iter().find_map(|f| read(&dir.join(f))) {
        return Some(t);
    }
    ["PULL_REQUEST_TEMPLATE", "pull_request_template"].iter().find_map(|d| {
        let mut mds: Vec<_> = std::fs::read_dir(dir.join(".github").join(d))
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")))
            .collect();
        mds.sort();
        mds.iter().find_map(|p| read(p))
    })
}

/// AI-drafted PR title + description for one repo. Orbit gathers the
/// context itself (the repo's pull_request_template, commits, diff stat and
/// a bounded diff vs origin/<base>) so the model answers in one call — what/
/// why/how, filling the template when there is one. Reply format is plain
/// "TITLE:" + "---" + body (no JSON escaping issues with multiline markdown).
pub fn pr_draft(ws_name: &str, repo: &str) -> Result<PrDraft, String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let branch = crate::git::current_branch(&dir).unwrap_or_default();
    // A repo view has no workspace base: PRs target the repo's default branch.
    let base = match crate::workspace::repo_scope(ws_name) {
        Some(_) => crate::git::default_branch(&dir).unwrap_or_else(|| "main".into()),
        None => crate::workspace::load_meta(&crate::workspace::ws_dir(ws_name)?)?.base,
    };
    let commits = git_out(&dir, &["log", "--oneline", "--no-decorate", &format!("origin/{base}..HEAD")]);
    let range = format!("origin/{base}...HEAD");
    let stat = git_out(&dir, &["diff", "--stat", &range]);
    let diff = clip(&git_out(&dir, &["diff", "--no-color", "--no-ext-diff", "--unified=2", &range]), DRAFT_DIFF_CHARS);
    let template = match pr_template(&dir) {
        Some(t) => format!("The repository's PR template — use its exact structure and fill it COMPLETELY with concrete facts from the changes (never leave placeholders):\n{t}"),
        None => "The repository has no PR template: write a short objective description of what was done, why and how, with bullet points where they help.".to_string(),
    };
    let prompt = format!(
        r#"You are preparing a pull request for the repository `{repo}` (branch `{branch}` into `{base}`). Everything you need is below: answer directly, do not read files or run commands.

Commits:
{commits}

Diff stat:
{stat}

Diff:
{diff}

{template}

Write:
- title: ONE line, up to 72 chars, objective, no markdown.
- description: as described above. No AI attribution, no generic filler.

Reply EXACTLY in this format, plain text, no code fences:
TITLE: <the title>
---
<the description>"#,
    );
    // Models occasionally drift from the format; one more try usually lands.
    let out = draft_answer(&dir, &prompt)?;
    if let Some(d) = parse_title_body(&out) {
        return Ok(d);
    }
    let out = draft_answer(&dir, &prompt)?;
    parse_title_body(&out).ok_or_else(|| {
        let preview: String = out.trim().chars().take(160).collect();
        format!("agent reply had no TITLE line: {preview}")
    })
}

/// "TITLE: ...", "---", body -> PrDraft. Tolerates code fences, markdown
/// decoration on the label ("**Title:**", "## Title:") and any case; the body
/// keeps its internal markdown, minus agent attribution lines.
pub fn parse_title_body(raw: &str) -> Option<PrDraft> {
    let lines: Vec<&str> = raw.lines().collect();
    let (at, title) = lines.iter().enumerate().find_map(|(i, l)| {
        let bare = l.trim().trim_start_matches(['#', '*', '_', ' ', '>']);
        let head = bare.get(..6)?;
        if !head.eq_ignore_ascii_case("title:") {
            return None;
        }
        let t = bare[6..].trim().trim_matches(['*', '_', '`', '"']).trim();
        (!t.is_empty()).then(|| (i, t.to_string()))
    })?;
    let rest = &lines[at + 1..];
    let start = rest.iter().position(|l| l.trim() == "---").map_or(0, |p| p + 1);
    let body = rest[start..]
        .iter()
        .filter(|l| !l.contains("Generated with [Claude Code]") && !l.starts_with("Co-Authored-By:"))
        .copied()
        .collect::<Vec<_>>()
        .join("
");
    let body = body.trim().trim_end_matches("```").trim().to_string();
    Some(PrDraft { title, body })
}

#[cfg(test)]
mod pr_draft_tests {
    use super::parse_title_body;

    #[test]
    fn parses_title_body_format() {
        let d = parse_title_body("TITLE: feat: add optin modal\n---\n## What\n- adds modal").unwrap();
        assert_eq!(d.title, "feat: add optin modal");
        assert_eq!(d.body, "## What\n- adds modal");
    }

    #[test]
    fn parses_without_body_and_with_fences() {
        let d = parse_title_body("```\nTITLE: fix thing\n---\n").unwrap();
        assert_eq!(d.title, "fix thing");
        assert_eq!(d.body, "");
        assert!(parse_title_body("no title here").is_none());
    }

    #[test]
    fn tolerates_markdown_label_preamble_and_attribution() {
        let d = parse_title_body(
            "Here is the PR:

**Title:** Add deploy lock

---
## Summary
- lock

Generated with [Claude Code](https://claude.com/claude-code)",
        )
        .unwrap();
        assert_eq!(d.title, "Add deploy lock");
        assert_eq!(d.body, "## Summary
- lock");
        assert_eq!(parse_title_body("## title: fix x
body").unwrap().body, "body");
    }
}

// ---------- CI check investigation ----------

#[derive(Serialize, Clone, Debug)]
pub struct CheckAnalysis {
    /// What broke, 2-4 sentences, plain text.
    pub problem: String,
    /// The proposed fix (may reference files; empty when only infra).
    pub fix: String,
    /// true when the fix is code in this worktree (Apply makes sense).
    pub actionable: bool,
}

/// AI analysis of a failed CI check: reads the failed-job log (bounded)
/// and the PR's numstat, then explains the problem and proposes a fix.
pub fn investigate_check(
    ws_name: &str,
    repo: &str,
    check_name: &str,
    failed_log: &str,
) -> Result<CheckAnalysis, String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let numstat = change_summary(&dir).unwrap_or_default();
    let prompt = format!(
        r#"A GitHub Actions check failed on a pull request in this repository.

Failed check: {check_name}

Failed job log (tail):
{failed_log}

Local uncommitted changes (numstat, may be empty):
{numstat}

Analyze what broke. Consider whether the failure is caused by the PR's changes, by a flaky/external dependency, or by something infrastructural.

Reply EXACTLY in this format, plain text, no code fences:
PROBLEM: <2-4 sentences, objective, what broke and the evidence from the log>
FIX: <the concrete correction, referencing files; empty string if nothing to fix in code>
ACTIONABLE: <yes|no>  — yes when the fix is code in this repository"#,
        check_name = check_name,
        failed_log = failed_log,
        numstat = if numstat.is_empty() { "(none)".to_string() } else { numstat },
    );
    let reply = agent_answer(&dir, &prompt)?;
    parse_check_analysis(&reply)
        .ok_or_else(|| "agent reply was not in PROBLEM:/FIX:/ACTIONABLE: format".into())
}

pub fn parse_check_analysis(raw: &str) -> Option<CheckAnalysis> {
    let get = |key: &str| -> String {
        match raw.find(key) {
            Some(idx) => {
                let after = &raw[idx + key.len()..];
                let line_end = after.find('\n').unwrap_or(after.len());
                after[..line_end].trim().to_string()
            }
            None => String::new(),
        }
    };
    let problem = get("PROBLEM:");
    let fix = get("FIX:");
    let actionable = get("ACTIONABLE:")
        .to_lowercase()
        .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
        .starts_with('y');
    if problem.is_empty() {
        return None;
    }
    Some(CheckAnalysis {
        problem,
        fix,
        actionable,
    })
}

#[cfg(test)]
mod check_analysis_tests {
    use super::parse_check_analysis;

    #[test]
    fn parses_analysis_fields() {
        let a = parse_check_analysis(
            "PROBLEM: Build fails: missing dep.\nFIX: run go mod tidy.\nACTIONABLE: yes",
        )
        .unwrap();
        assert_eq!(a.problem, "Build fails: missing dep.");
        assert_eq!(a.fix, "run go mod tidy.");
        assert!(a.actionable);
    }

    #[test]
    fn handles_multiword_and_missing_fix() {
        let a = parse_check_analysis(
            "PROBLEM: infra flaked (npm registry 502).\nFIX: \nACTIONABLE: no",
        )
        .unwrap();
        assert!(!a.actionable);
        assert_eq!(a.fix, "");
        assert!(parse_check_analysis("garbage").is_none());
    }
}

/// Applies an AI-proposed CI fix in the worktree: agent with write
/// permissions (acceptEdits/--auto), instructed to implement exactly the
/// given fix. Staging/commit stay with the normal app flow.
pub fn apply_check_fix(ws_name: &str, repo: &str, fix: &str) -> Result<(), String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let prompt = format!(
        r#"Apply this fix to the repository code, exactly as described (no extra changes):

{fix}

Edit the files needed to implement it. Do NOT commit or stage — editing is enough. Reply with a one-line summary of what you changed."#,
        fix = fix,
    );

    run_edit_agent(&dir, &prompt).map(|_| ())
}

/// Headless agent allowed to edit files in `dir`; returns its reply.
fn run_edit_agent(dir: &Path, prompt: &str) -> Result<String, String> {
    let ai = crate::config::Config::load()?.ai;
    runner::run_capture(
        agent_cmd(&ai, prompt, Access::Edit, false),
        dir,
        Duration::from_secs(AGENT_TIMEOUT_SECS),
    )
}

// ---------- Addressing code review feedback ----------

/// What the agent did about one review thread, plus the reply to post.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThreadReply {
    pub id: String,
    /// "fixed" (code changed) | "answered" (question / disagreement, no change).
    pub status: String,
    pub reply: String,
}

/// Several threads at once is deliberate: one pass keeps the fixes
/// coherent instead of the agent swinging back and forth per comment.
fn address_prompt(number: u64, threads: &[&crate::review::ReviewThread]) -> String {
    let mut list = String::new();
    for (i, t) in threads.iter().enumerate() {
        let loc = match (t.line, t.original_line) {
            (Some(l), _) => format!("{}:{l}", t.path),
            (None, Some(l)) => format!("{}:{l} (outdated: the code may have moved since)", t.path),
            _ => t.path.clone(),
        };
        list.push_str(&format!("\n### Thread {} (id: {}) — {loc}\n", i + 1, t.id));
        for c in &t.comments {
            list.push_str(&format!("@{}: {}\n", c.author, c.body.trim()));
        }
    }
    format!(
        r#"You are addressing code review feedback on pull request #{number} of the repository in this folder.

For each review thread below decide:
- "fixed": the request is valid. Change the code accordingly: minimal, focused, matching the surrounding style.
- "answered": it is a question, not applicable, or you disagree. Do not change code for it; answer it.

Do not commit or push. Do not touch unrelated code.
{list}
When you are done, reply with ONLY a JSON array (no prose, no code fences), one entry per thread:
[{{"id": "<thread id>", "status": "fixed" | "answered", "reply": "<reply to post on the thread>"}}]

Reply rules: one or two short sentences, objective and clear. Write each reply in the language of its own thread (threads may differ). No greetings, no thanks, no mention of AI. For "fixed", say what changed (e.g. "Done, moved the check into validate()."). For "answered", give the reason plainly."#
    )
}

/// The JSON array the agent ends with; tolerant of prose or fences around it.
pub fn parse_thread_replies(raw: &str) -> Option<Vec<ThreadReply>> {
    let mut starts: Vec<usize> = raw.match_indices('[').map(|(i, _)| i).collect();
    starts.reverse();
    let end = raw.rfind(']')?;
    starts
        .into_iter()
        .filter(|&s| s < end)
        .find_map(|s| serde_json::from_str::<Vec<ThreadReply>>(&raw[s..=end]).ok())
        .filter(|v| !v.is_empty())
}

/// Runs the agent in the repo's worktree on the selected unresolved threads
/// of a PR; it edits files (no commit) and drafts one reply per thread.
pub fn address_review(workspace: &str, repo: &str, owner_repo: &str, number: u64, thread_ids: &[String]) -> Result<Vec<ThreadReply>, String> {
    let dir = crate::workspace::repo_path(workspace, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let data = crate::review::review_data(owner_repo, number, false)?;
    let threads: Vec<&crate::review::ReviewThread> = data.threads.iter().filter(|t| thread_ids.contains(&t.id)).collect();
    if threads.is_empty() {
        return Err("none of the selected conversations are open anymore".into());
    }
    let ai = crate::config::Config::load()?.ai;
    // Several fixes in one run: give it more room than a single edit.
    let out = runner::run_capture(
        agent_cmd(&ai, &address_prompt(number, &threads), Access::Edit, false),
        &dir,
        Duration::from_secs(AGENT_TIMEOUT_SECS * 3),
    )?;
    let replies = parse_thread_replies(&out).ok_or_else(|| {
        let preview: String = out.trim().chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
        format!("the agent finished without the reply list: …{preview}")
    })?;
    Ok(replies.into_iter().filter(|r| thread_ids.contains(&r.id)).collect())
}

#[cfg(test)]
mod address_tests {
    use super::*;

    #[test]
    fn parses_the_trailing_reply_list() {
        let raw = "I updated [the parser].\n```json\n[{\"id\":\"T1\",\"status\":\"fixed\",\"reply\":\"Done, renamed it.\"}]\n```";
        let r = parse_thread_replies(raw).unwrap();
        assert_eq!(r, vec![ThreadReply { id: "T1".into(), status: "fixed".into(), reply: "Done, renamed it.".into() }]);
        assert!(parse_thread_replies("no list here").is_none());
        assert!(parse_thread_replies("[]").is_none());
    }

    #[test]
    fn prompt_lists_every_thread_with_id_and_location() {
        let t = crate::review::ReviewThread {
            id: "PRRT_1".into(),
            path: "src/a.rs".into(),
            side: "RIGHT".into(),
            line: None,
            start_line: None,
            original_line: Some(7),
            is_resolved: false,
            is_outdated: true,
            comments: vec![],
        };
        let p = address_prompt(3, &[&t]);
        assert!(p.contains("(id: PRRT_1) — src/a.rs:7 (outdated"));
        assert!(p.contains("pull request #3"));
    }
}
