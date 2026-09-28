// The feature plan (PLAN.md at the workspace root): drafted by an agent from
// the linked card and/or a goal the developer types, optionally after an
// interview, or written by hand from a template. Every agent call streams
// its progress (live.rs) and can be cancelled; the interview is one
// continuing conversation, not a cold agent per round.
use super::live::{run_live, Reply};
use super::{parse_grill_json, Access, GrillRound, PLAN_FILE};
use crate::integrations::{fetch_card, TrackerKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::AppHandle;

/// Drafting explores every repo: a long leash, it's visible and cancellable.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const ROUND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// User override of the draft prompt (placeholders documented in `draft_prompt`).
const PROMPT_PATH: &str = ".config/orbit/plan-prompt.md";

/// What the plan is about: the workspace's linked card (if any) and what the
/// developer typed. At least one of them must say something.
struct Context {
    title: String,
    card_line: String,
    brief: String,
    repos: Vec<String>,
    branch: String,
    ws_dir: PathBuf,
}

impl Context {
    fn load(ws_name: &str, goal: &str) -> Result<Context, String> {
        let ws_dir = crate::workspace::ws_dir(ws_name)?;
        if !ws_dir.exists() {
            return Err(format!("workspace '{ws_name}' not found"));
        }
        let meta = crate::workspace::load_meta(&ws_dir)?;
        let goal = goal.trim();
        // A card that can't be fetched (tracker offline, token expired) still
        // leaves its title; the goal can carry the rest.
        let card = meta.card.as_ref().map(|c| {
            let detail = TrackerKind::parse(&c.kind).and_then(|k| fetch_card(k, &c.id)).ok();
            (c, detail)
        });
        if goal.is_empty() && card.is_none() {
            return Err("describe what the feature should do (no card is linked to this workspace)".into());
        }
        let mut brief = String::new();
        let (title, card_line) = match &card {
            Some((c, detail)) => {
                let title = detail.as_ref().map(|d| d.title.clone()).unwrap_or_else(|| c.title.clone());
                brief.push_str(&format!("Card {}: {title}\n", c.id));
                if let Some(d) = detail.as_ref().filter(|d| !d.description.trim().is_empty()) {
                    brief.push_str(&format!("{}\n", d.description.trim()));
                }
                (title, format!("**Card**: [{}]({}) · **Branch**: {}", c.id, c.url, meta.branch))
            }
            None => {
                let first = goal.lines().next().unwrap_or("Feature plan").trim();
                let title: String = first.chars().take(80).collect();
                (title, format!("**Branch**: {}", meta.branch))
            }
        };
        if !goal.is_empty() {
            brief.push_str(&format!(
                "\nThe developer's own words (they take precedence over the card):\n{goal}\n"
            ));
        }
        Ok(Context { title, card_line, brief, repos: meta.repos, branch: meta.branch, ws_dir })
    }

    fn repos_list(&self) -> String {
        self.repos.iter().map(|r| format!("- `{r}` (folder `{r}/`)")).collect::<Vec<_>>().join("\n")
    }

    fn plan_path(&self) -> PathBuf {
        self.ws_dir.join(PLAN_FILE)
    }
}

/// The plan's shape and the rules that make its tasks runnable one at a
/// time by an agent (the same bar Ralph needs).
fn format_rules(ctx: &Context) -> String {
    format!(
        r#"The plan is markdown with EXACTLY this structure:

```
# {title}

{card_line}

## Context

- (3-6 bullets: what the feature is for, and the decisions that shape it)

## Tasks

- [ ] 1. <imperative title> — <repo>
      <what to change, 1-3 lines: which modules/files, what behavior>
      Accept: <verifiable check>; <verifiable check>
- [ ] 2. …
```

Rules for the tasks:
- Each task fits ONE focused agent session: if it touches many files or can't be described in 2-3 lines, split it.
- Order by dependency (data/schema → backend → API → UI); a task never depends on a later one.
- Each task names exactly one repository from the list, spelled as listed.
- "Accept" lists checks anyone can verify: a command that passes (the repo's typecheck/lint/tests when it has them), an endpoint returning X, a screen showing Y. Never "works correctly".
- Explore the code first: no task for what already exists; reference real paths."#,
        title = ctx.title,
        card_line = ctx.card_line,
    )
}

/// The default draft prompt, or the user's template with its placeholders
/// filled: {card_title} {card_description} {ws_name} {branch} {repos}
/// {plan_path} {goal} {decisions}.
fn draft_prompt(ctx: &Context, ws_name: &str, goal: &str, decisions: &str) -> String {
    let plan_path = ctx.plan_path().to_string_lossy().to_string();
    let custom = crate::config::home_dir().ok().and_then(|h| std::fs::read_to_string(h.join(PROMPT_PATH)).ok());
    if let Some(t) = custom {
        return t
            .replace("{card_title}", &ctx.title)
            .replace("{card_description}", &ctx.brief)
            .replace("{ws_name}", ws_name)
            .replace("{branch}", &ctx.branch)
            .replace("{repos}", &ctx.repos_list())
            .replace("{plan_path}", &plan_path)
            .replace("{goal}", goal)
            .replace("{decisions}", decisions);
    }
    let decisions = if decisions.trim().is_empty() {
        "None: there was no interview. Where something important is unclear, pick the most reasonable option and say so in Context.".to_string()
    } else {
        format!("The developer was interviewed; honor these decisions:\n{decisions}")
    };
    format!(
        r#"You are planning a feature in a workspace of several repositories (one folder per repository in the current directory).

# The feature
{brief}
# Decisions
{decisions}

# Repositories
{repos}

# Your job
Explore the repositories as much as you need, then write the plan to `{plan_path}` (absolute path; write EXACTLY there). Do not modify any code: write only that file.

{rules}

When the file is written, reply with one line: the path."#,
        brief = ctx.brief,
        repos = ctx.repos_list(),
        rules = format_rules(ctx),
    )
}

/// A plan being replaced is kept, not lost: `.orbit/plans/PLAN-<time>.md`.
fn keep_previous(ctx: &Context) {
    let path = ctx.plan_path();
    if !path.exists() {
        return;
    }
    let dir = ctx.ws_dir.join(".orbit").join("plans");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::copy(&path, dir.join(format!("PLAN-{stamp}.md")));
    }
}

/// Drafts PLAN.md with the configured agent; returns its path.
pub fn draft(app: &AppHandle, run: &str, ws_name: &str, goal: &str, decisions: &str) -> Result<String, String> {
    let ctx = Context::load(ws_name, goal)?;
    let ai = crate::config::Config::load()?.ai;
    let prompt = draft_prompt(&ctx, ws_name, goal, decisions);
    keep_previous(&ctx);
    let before = std::fs::metadata(ctx.plan_path()).and_then(|m| m.modified()).ok();
    let reply = run_live(app, run, &ai, &ctx.ws_dir, &prompt, Access::Edit, None, DRAFT_TIMEOUT)?;
    settle_plan_file(&ctx, &reply.text, before)
}

/// Makes sure the agent's plan ended up at the workspace root: moved from a
/// repo folder, or salvaged from its reply when it answered instead of
/// writing (sandboxes that refuse writes).
fn settle_plan_file(ctx: &Context, reply: &str, before: Option<std::time::SystemTime>) -> Result<String, String> {
    let path = ctx.plan_path();
    let fresh = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok().is_some_and(|t| Some(t) != before);
    if !fresh(&path) {
        for repo in &ctx.repos {
            let stray = ctx.ws_dir.join(repo).join(PLAN_FILE);
            if stray.exists() {
                std::fs::rename(&stray, &path).map_err(|e| format!("couldn't move {repo}/PLAN.md to the workspace root: {e}"))?;
                return Ok(path.to_string_lossy().to_string());
            }
        }
        let salvaged = extract_markdown(reply);
        if salvaged.is_empty() {
            return Err("the agent finished without writing PLAN.md (and its reply had no plan in it)".into());
        }
        std::fs::write(&path, salvaged).map_err(|e| format!("couldn't write PLAN.md: {e}"))?;
    }
    Ok(path.to_string_lossy().to_string())
}

/// The largest markdown block of a reply: a ```markdown / ```md fence, else
/// everything from the first "# " heading.
pub(crate) fn extract_markdown(reply: &str) -> String {
    for fence in ["```markdown", "```md"] {
        if let Some(start) = reply.find(fence) {
            let body = &reply[start + fence.len()..];
            if let Some(end) = body.find("\n```") {
                return body[..end].trim().to_string() + "\n";
            }
        }
    }
    if let Some(start) = reply.find("\n# ").map(|i| i + 1).or_else(|| reply.starts_with("# ").then_some(0)) {
        return reply[start..].trim().to_string() + "\n";
    }
    String::new()
}

/// A question the developer answered, with the question itself (the agent
/// needs its own words back when it can't continue the conversation).
#[derive(Deserialize, Debug, Clone)]
pub struct Answer {
    pub question: String,
    pub answer: String,
}

#[derive(Serialize, Debug)]
pub struct InterviewTurn {
    pub round: GrillRound,
    /// Pass back on the next call: the interview continues that conversation.
    pub session: Option<String>,
}

const INTERVIEW_JSON: &str = r#"Reply with ONLY this JSON, no prose before or after:
{"done": false, "questions": [{"id": "q1", "text": "…?", "options": [{"label": "…", "description": "…tradeoff…", "recommended": true}, {"label": "…", "description": "…"}, {"label": "…", "description": "…"}]}]}
or, when everything important is settled:
{"done": true, "summary": "- decision 1\n- decision 2", "questions": []}"#;

fn history(answers: &[Answer]) -> String {
    if answers.is_empty() {
        return "None yet.".into();
    }
    answers.iter().map(|a| format!("- Q: {}\n  A: {}", a.question, a.answer)).collect::<Vec<_>>().join("\n")
}

/// The opening prompt: the whole context, how to interview, and (for agents
/// that can't continue a conversation) everything answered so far.
fn interview_opening(ctx: &Context, answers: &[Answer], finish: bool) -> String {
    format!(
        r#"You are interviewing a developer to plan a feature before anyone writes code. Explore the repositories first (one folder per repository in the current directory) so your questions are informed; never ask what the code can answer.

# The feature
{brief}
# Repositories
{repos}

# Answers so far
{history}

# How to interview
Work a decision tree: each round, ask every question whose prerequisites are already settled (3-6 questions), about behaviors, contracts, edge cases, failure modes and cross-repo impact the developer may have assumed. Never repeat an answered question.
Each question offers exactly 3 concrete options, your recommended one first (recommended=true), each with a short label and a one-line tradeoff.
{finish}
{json}"#,
        brief = ctx.brief,
        repos = ctx.repos_list(),
        history = history(answers),
        finish = finish_note(finish),
        json = INTERVIEW_JSON,
    )
}

fn finish_note(finish: bool) -> &'static str {
    if finish {
        "The developer wants to stop here: ask nothing more, set done=true and summarize every decision so far (5-10 bullets)."
    } else {
        "When nothing important is left open, set done=true with a summary of the decisions (5-10 bullets)."
    }
}

/// A follow-up turn in the same conversation: only the new answers.
fn interview_followup(new_answers: &[Answer], finish: bool) -> String {
    format!(
        "My answers:\n{}\n\n{}\n{}",
        history(new_answers),
        finish_note(finish),
        INTERVIEW_JSON
    )
}

/// One interview round. `session` continues the conversation (only the
/// answers since the last round are sent); without one (first round, or an
/// agent that can't resume) the full context and history go out.
#[allow(clippy::too_many_arguments)]
pub fn interview(
    app: &AppHandle,
    run: &str,
    ws_name: &str,
    goal: &str,
    answers: &[Answer],
    new_answers: &[Answer],
    session: Option<&str>,
    finish: bool,
) -> Result<InterviewTurn, String> {
    let ctx = Context::load(ws_name, goal)?;
    let ai = crate::config::Config::load()?.ai;
    let prompt = match session {
        Some(_) => interview_followup(new_answers, finish),
        None => interview_opening(&ctx, answers, finish),
    };
    let reply = run_live(app, run, &ai, &ctx.ws_dir, &prompt, Access::ReadOnly, session, ROUND_TIMEOUT)?;
    let session = reply.session.clone();
    match parse_grill_json(&reply.text) {
        Ok(round) => Ok(InterviewTurn { round, session }),
        // Models drift from the format now and then: ask once more, in the
        // same conversation, when there is one.
        Err(first) => {
            let Some(sid) = session.as_deref() else { return Err(first) };
            let again: Reply = run_live(
                app,
                run,
                &ai,
                &ctx.ws_dir,
                &format!("That wasn't valid. {INTERVIEW_JSON}"),
                Access::ReadOnly,
                Some(sid),
                ROUND_TIMEOUT,
            )?;
            let round = parse_grill_json(&again.text)?;
            Ok(InterviewTurn { round, session })
        }
    }
}

/// Starts a hand-written plan: the template with the card/goal filled in.
/// Never overwrites an existing PLAN.md.
pub fn blank(ws_name: &str, goal: &str) -> Result<String, String> {
    let ws_dir = crate::workspace::ws_dir(ws_name)?;
    let path = ws_dir.join(PLAN_FILE);
    if path.exists() {
        return Ok(path.to_string_lossy().to_string());
    }
    let meta = crate::workspace::load_meta(&ws_dir)?;
    let goal = goal.trim();
    let title = meta
        .card
        .as_ref()
        .map(|c| c.title.clone())
        .or_else(|| goal.lines().next().map(|l| l.chars().take(80).collect()))
        .filter(|t: &String| !t.trim().is_empty())
        .unwrap_or_else(|| ws_name.to_string());
    let card_line = match &meta.card {
        Some(c) => format!("**Card**: [{}]({}) · **Branch**: {}", c.id, c.url, meta.branch),
        None => format!("**Branch**: {}", meta.branch),
    };
    let repo = meta.repos.first().cloned().unwrap_or_else(|| "repo".into());
    let context = if goal.is_empty() { "- What the feature is for".to_string() } else { goal.lines().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n") };
    let body = format!(
        "# {title}\n\n{card_line}\n\n## Context\n\n{context}\n\n## Tasks\n\n- [ ] 1. First step — {repo}\n      What to change.\n      Accept: a check anyone can run\n"
    );
    std::fs::write(&path, body).map_err(|e| format!("couldn't create PLAN.md: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Opening prompt for planning in an interactive agent session (terminal):
/// the agent explores, talks it through with the developer, then writes.
pub fn session_prompt(ws_name: &str, goal: &str) -> Result<String, String> {
    let ctx = Context::load(ws_name, goal)?;
    Ok(format!(
        r#"Let's plan a feature together before writing any code.

# The feature
{brief}
# Repositories
{repos}

First explore the repositories to understand what exists. Then interview me: ask a few focused questions at a time about behaviors, edge cases and cross-repo impact (offer options and your recommendation), and wait for my answers. When the important decisions are settled, write the plan to `{plan_path}` and show it to me. Do not modify any code.

{rules}"#,
        brief = ctx.brief,
        repos = ctx.repos_list(),
        plan_path = ctx.plan_path().display(),
        rules = format_rules(&ctx),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvages_a_plan_from_a_reply() {
        assert_eq!(extract_markdown("ok\n```markdown\n# T\n- [ ] a\n```\nbye"), "# T\n- [ ] a\n");
        assert_eq!(extract_markdown("Here:\n# T\n\n## Tasks\n"), "# T\n\n## Tasks\n");
        assert_eq!(extract_markdown("no plan here"), "");
    }

    #[test]
    fn history_carries_the_questions() {
        let a = [Answer { question: "Iframe or API?".into(), answer: "Iframe".into() }];
        assert_eq!(history(&a), "- Q: Iframe or API?\n  A: Iframe");
        assert!(interview_followup(&a, true).contains("ask nothing more"));
    }
}
