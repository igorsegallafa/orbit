// Plan generation: runs a local CLI agent (claude/opencode) non-interactively
// in the workspace root with a prompt built from the linked card, streaming
// its output to the frontend via Tauri events. The agent writes PLAN.md.
mod cmd;
pub mod runner;
pub mod stream;
pub mod live;
pub mod plan;

pub use cmd::{agent_cmd, live_cmd, Access};
use crate::config::AiSettings;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

pub const PLAN_FILE: &str = "PLAN.md";
const AGENT_TIMEOUT_SECS: u64 = 300;
/// Headless one-shot answers (commit messages, PR drafts): shorter leash
/// than the plan agent — these are moments in a UI flow, not background
/// jobs.
const DRAFT_TIMEOUT_SECS: u64 = 120;

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
    change_summary_of(dir, &[])
}

/// `change_summary` limited to `paths` (all when empty).
fn change_summary_of(dir: &Path, paths: &[String]) -> Result<String, String> {
    Ok(crate::git::changes(dir)?
        .iter()
        .filter(|c| paths.is_empty() || paths.contains(&c.path))
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
/// `paths`: only those files will be committed, so only they are described
/// (all changes when empty).
pub fn commit_message(ws_name: &str, repo: &str, paths: &[String]) -> Result<CommitMsg, String> {
    let dir = crate::workspace::repo_path(ws_name, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let numstat = change_summary_of(&dir, paths)?;
    if numstat.is_empty() {
        return Err(format!("{repo}: nothing to commit"));
    }
    let branch = crate::git::current_branch(&dir).unwrap_or_default();
    let mut diff_args = vec!["diff", "HEAD", "--no-color", "--no-ext-diff", "--unified=2"];
    if !paths.is_empty() {
        diff_args.push("--");
        diff_args.extend(paths.iter().map(String::as_str));
    }
    let diff = clip(&git_out(&dir, &diff_args), DRAFT_DIFF_CHARS);
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
    // Two attempts in total, whatever went wrong (CLI failure or a reply that
    // drifted from the format): stacking the transient-failure retry on top
    // of the format retry let a slow model keep the modal spinning for 8 min.
    let mut ai = crate::config::Config::load()?.ai;
    ai.model = ai.draft_model();
    let mut last = String::new();
    for _ in 0..2 {
        match answer_once(&ai, &dir, &prompt, Access::Answer) {
            Ok(out) => match parse_title_body(&out) {
                Some(d) => return Ok(d),
                None => {
                    let preview: String = out.trim().chars().take(160).collect();
                    last = format!("agent reply had no TITLE line: {preview}");
                }
            },
            Err(e) => last = e,
        }
    }
    Err(last)
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
fn address_prompt(number: u64, threads: &[&crate::review::ReviewThread], notes: &[String]) -> String {
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
    // Reviews' general comments (no thread to reply to): act on them, no entry in the list.
    let general = if notes.is_empty() {
        String::new()
    } else {
        let items = notes.iter().map(|n| format!("- {}", n.trim().replace('\n', "\n  "))).collect::<Vec<_>>().join("\n");
        format!("\n## General review comments\nNot tied to a line: apply what's valid; they need no entry in your reply list.\n{items}\n")
    };
    format!(
        r#"You are addressing code review feedback on pull request #{number} of the repository in this folder.

For each review thread below decide:
- "fixed": the request is valid. Change the code accordingly: minimal, focused, matching the surrounding style.
- "answered": it is a question, not applicable, or you disagree. Do not change code for it; answer it.

Do not commit or push. Do not touch unrelated code.
{list}{general}
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
/// Applies the selected review conversations (and general review notes) in
/// the repo's worktree with a live, cancellable agent (`run`); returns the
/// reply drafted for each conversation.
#[allow(clippy::too_many_arguments)]
pub fn address_review(
    app: &tauri::AppHandle,
    run: &str,
    workspace: &str,
    repo: &str,
    owner_repo: &str,
    number: u64,
    thread_ids: &[String],
    notes: &[String],
) -> Result<Vec<ThreadReply>, String> {
    let dir = crate::workspace::repo_path(workspace, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    let data = crate::review::review_data(owner_repo, number, false)?;
    let threads: Vec<&crate::review::ReviewThread> = data.threads.iter().filter(|t| thread_ids.contains(&t.id)).collect();
    if threads.is_empty() && notes.is_empty() {
        return Err("none of the selected conversations are open anymore".into());
    }
    let ai = crate::config::Config::load()?.ai;
    // Several fixes in one run: give it more room than a single edit.
    let reply = live::run_live(
        app,
        run,
        &ai,
        &dir,
        &address_prompt(number, &threads, notes),
        Access::Edit,
        None,
        Duration::from_secs(AGENT_TIMEOUT_SECS * 3),
    )?;
    if threads.is_empty() {
        return Ok(vec![]);
    }
    let replies = parse_thread_replies(&reply.text).ok_or_else(|| {
        let preview: String = reply.text.trim().chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
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
        let p = address_prompt(3, &[&t], &[]);
        assert!(p.contains("(id: PRRT_1) — src/a.rs:7 (outdated"));
        assert!(p.contains("pull request #3"));
        assert!(!p.contains("General review comments"));
        let p = address_prompt(3, &[&t], &["Please add tests\nfor the edge case".into()]);
        assert!(p.contains("## General review comments") && p.contains("- Please add tests\n  for the edge case"));
    }
}
