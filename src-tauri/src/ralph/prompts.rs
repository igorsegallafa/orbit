// Prompts for Ralph: the per-iteration instructions (adapted from the
// machina-workspace templates/ralph/CLAUDE.md), the PRD interview and the
// PRD writer (adapted from the prd + ralph skills).

/// Placeholders: {prd_path} {progress_path} {branch} {repo}.
pub const ITERATION: &str = r#"# Ralph Agent Instructions

You are an autonomous coding agent. You work inside the git worktree of the repository `{repo}`, already on branch `{branch}`.

## Your Task

1. Read the PRD at `{prd_path}`.
2. Read the progress log at `{progress_path}` (check the `Codebase Patterns` section first).
3. Stay on the current branch `{branch}`. Do **not** create, switch or rename branches. The PRD's `branchName` is documentation only.
4. Pick the **highest priority** user story where `passes: false` (lowest `priority` number).
5. Implement that single user story.
6. Run this repo's quality checks (typecheck, lint, test). Prefer an existing project skill or script for them before improvising commands.
7. Update AGENTS.md/CLAUDE.md files if you discover reusable patterns (see below).
8. If checks pass, commit ALL changes with message: `feat: [Story ID] - [Story Title]`. Do not add `Co-Authored-By` or any AI attribution trailer.
9. Update the PRD to set `passes: true` for the completed story.
10. Append your progress to `{progress_path}`.

## Progress Report Format

APPEND to the progress log (never replace, always append):
```
## [Date/Time] - [Story ID]
- What was implemented
- Files changed
- **Learnings for future iterations:**
  - Patterns discovered (e.g., "this codebase uses X for Y")
  - Gotchas encountered (e.g., "don't forget to update Z when changing W")
  - Useful context (e.g., "the evaluation panel is in component X")
---
```

The learnings section is critical: it helps future iterations avoid repeating mistakes.

## Consolidate Patterns

If you discover a **reusable pattern** future iterations should know, add it to the `## Codebase Patterns` section at the TOP of the progress log (create it if missing). Only general, reusable patterns, not story-specific details.

## Update AGENTS.md / CLAUDE.md Files

Before committing, check whether edited directories have an AGENTS.md or CLAUDE.md (use whichever this repo already has; don't introduce a new convention) and add genuinely reusable knowledge: module conventions, gotchas, dependencies between files, testing approaches. Do NOT add story-specific details or temporary notes.

## Quality Requirements

- ALL commits must pass this project's quality checks.
- Do NOT commit broken code.
- Keep changes focused and minimal. Follow existing code patterns.

## Browser Testing (If Available)

For stories that change UI, verify them in the browser with the `claude-in-chrome` skill when it's available, and note what you verified in the progress log. If it isn't available, note that manual browser verification is still needed.

## Stop Condition

After completing a user story, check whether ALL stories have `passes: true`. If so, reply with:
<promise>COMPLETE</promise>

Otherwise end your response normally (another iteration picks up the next story).

## Important

- Work on ONE story per iteration.
- Read the Codebase Patterns section in the progress log before starting.
- Only work inside this worktree."#;

pub fn render_iteration(template: &str, prd: &str, progress: &str, branch: &str, repo: &str, extra: &str) -> String {
    let mut out = template
        .replace("{prd_path}", prd)
        .replace("{progress_path}", progress)
        .replace("{branch}", branch)
        .replace("{repo}", repo);
    if !extra.trim().is_empty() {
        out.push_str("\n\n## Additional Instructions For This Run\n\n");
        out.push_str(extra.trim());
    }
    out
}

pub fn interview(brief: &str, repo: &str, history: &str, cap_note: &str) -> String {
    format!(
        r#"You are preparing a Product Requirements Document for an autonomous coding loop (Ralph) that will implement it one small user story at a time in the repository `{repo}` (the current folder).

# Feature brief
{brief}

# Answers so far
{history}
{cap_note}

# Your job
Ask only the CRITICAL clarifying questions where the brief is ambiguous: problem/goal, core functionality, scope boundaries (what it must NOT do), success criteria. Explore the repository first when a question can be answered from the code; never ask what the code already tells you. Never repeat an answered question. Ask 3-5 questions per round.

Each question MUST offer exactly 3 concrete options, your RECOMMENDED one first (recommended=true), each with a short label and a description carrying the tradeoff.

When everything important is settled, set done=true and write a summary of the decisions (5-10 bullets).

Reply with ONLY this JSON, no prose before or after:
{{"done": false, "questions": [{{"id": "q1", "text": "…?", "options": [{{"label": "…", "description": "…", "recommended": true}}, {{"label": "…", "description": "…"}}, {{"label": "…", "description": "…"}}]}}]}}
or, when finished:
{{"done": true, "summary": "- decision 1\n- decision 2", "questions": []}}"#
    )
}

pub fn write_prd(brief: &str, decisions: &str, md_path: &str, json_path: &str, branch: &str, project: &str) -> String {
    format!(
        r#"Write a Product Requirements Document and its Ralph task file for the feature below. Explore this repository as needed so stories name real modules and follow existing patterns. Do NOT implement anything.

# Feature brief
{brief}

# Confirmed decisions
{decisions}

# 1. Markdown PRD → `{md_path}`
Sections: Introduction/Overview; Goals; User Stories (each `### US-001: Title`, `**Description:** As a [user], I want [feature] so that [benefit].`, and an `**Acceptance Criteria:**` checklist); Functional Requirements (`FR-1: The system must…`); Non-Goals; Design Considerations (optional); Technical Considerations (optional); Success Metrics; Open Questions. Write for a junior developer or agent reading it cold.

# 2. Ralph task file → `{json_path}`
Exactly this JSON shape:
{{"project": "{project}", "branchName": "{branch}", "description": "<feature description + global rules every story must follow>", "userStories": [{{"id": "US-001", "title": "…", "description": "As a …, I want … so that …", "acceptanceCriteria": ["…", "Typecheck passes"], "priority": 1, "passes": false, "notes": ""}}]}}

Rules:
- Each story must be completable in ONE iteration of a fresh agent with no memory (one context window). If you can't describe the change in 2-3 sentences, split it.
- Order by dependency: schema/data → backend logic → UI → aggregate views. No story may depend on a later one. `priority` follows that order.
- Acceptance criteria must be verifiable ("Filter dropdown has options All/Active/Done"), never vague ("works correctly").
- Every story ends with "Typecheck passes"; testable logic also gets "Tests pass"; UI stories also get "Verify in browser using the claude-in-chrome skill".
- IDs sequential US-001, US-002…; every story starts with "passes": false and "notes": "".

Write both files (absolute paths above, exactly), then reply with one line: the number of stories."#
    )
}
