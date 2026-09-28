// Builds the headless command line for the configured agent CLI.
use crate::config::AiSettings;
use std::process::Command;

/// What the agent may do on its own in a headless run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// Text answer from the prompt alone: no tools, no MCP servers, so it's
    /// a single model call (claude; other agents run as ReadOnly).
    Answer,
    /// Answers only; any write is denied.
    ReadOnly,
    /// File edits auto-approved (claude acceptEdits / opencode --auto).
    Edit,
    /// Everything auto-approved, shell included (autonomous loops).
    Full,
}

impl Access {
    fn writes(self) -> bool {
        matches!(self, Access::Edit | Access::Full)
    }
}

/// One `--flag=value` arg: `--allowedTools` is variadic and would swallow the prompt.
const READ_ONLY_TOOLS: &str =
    "--allowedTools=Read,Grep,Glob,Bash(git log:*),Bash(git diff:*),Bash(git show:*),Bash(git status:*),Bash(git ls-files:*)";

/// Argv (after the binary) for one headless agent run. `stream_json` only
/// applies to claude (one JSON event per line); other agents print text.
pub fn agent_args(ai: &AiSettings, prompt: &str, access: Access, stream_json: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match ai.agent.as_str() {
        "opencode" => {
            a.extend(["run".into(), "--model".into(), ai.model.clone()]);
            if access.writes() {
                a.push("--auto".into());
            }
        }
        "omp" => {
            a.push("-p".into());
            if access.writes() {
                a.push("--auto-approve".into());
            }
            a.extend(["--model".into(), ai.model.clone()]);
        }
        _ => {
            a.extend(["-p".into(), "--model".into(), ai.model.clone()]);
            match access {
                Access::Answer => a.extend(["--tools=".into(), "--strict-mcp-config".into()]),
                // Headless claude can't prompt, so unlisted tools are denied and it
                // may answer "I need approval" instead; read-only git is safe.
                Access::ReadOnly => a.push(READ_ONLY_TOOLS.into()),
                Access::Edit => a.extend(["--permission-mode".into(), "acceptEdits".into()]),
                Access::Full => a.push("--dangerously-skip-permissions".into()),
            }
            if stream_json {
                a.extend(["--output-format".into(), "stream-json".into(), "--verbose".into()]);
            }
        }
    }
    a.push(prompt.to_string());
    a
}

/// Argv for a headless run whose progress shows live: one JSON event per
/// line from claude (stream-json) and opencode (--format json); omp prints
/// text. `resume` continues an earlier conversation of the same agent.
pub fn live_args(ai: &AiSettings, prompt: &str, access: Access, resume: Option<&str>) -> Vec<String> {
    let bin = ai.agent_bin();
    let mut a = agent_args(ai, prompt, access, bin == "claude");
    let prompt_arg = a.pop().unwrap_or_default();
    match (bin.as_str(), resume) {
        ("claude", Some(id)) => a.extend(["--resume".into(), id.into()]),
        ("opencode", Some(id)) => a.extend(["--session".into(), id.into()]),
        _ => {}
    }
    if bin == "opencode" {
        a.extend(["--format".into(), "json".into()]);
    }
    a.push(prompt_arg);
    a
}

pub fn live_cmd(ai: &AiSettings, prompt: &str, access: Access, resume: Option<&str>) -> Command {
    let mut c = crate::proc::cmd(&ai.agent_bin());
    c.args(live_args(ai, prompt, access, resume));
    c
}

/// Ready-to-spawn command for the configured agent (cwd/stdio set by the runner).
pub fn agent_cmd(ai: &AiSettings, prompt: &str, access: Access, stream_json: bool) -> Command {
    let mut c = crate::proc::cmd(&ai.agent_bin());
    c.args(agent_args(ai, prompt, access, stream_json));
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(agent: &str) -> AiSettings {
        AiSettings { agent: agent.into(), model: "m".into(), fast_model: None }
    }

    #[test]
    fn answer_runs_claude_without_tools_or_mcp() {
        assert_eq!(
            agent_args(&ai("claude"), "hi", Access::Answer, false),
            ["-p", "--model", "m", "--tools=", "--strict-mcp-config", "hi"]
        );
        assert_eq!(agent_args(&ai("opencode"), "hi", Access::Answer, false), ["run", "--model", "m", "hi"]);
    }

    #[test]
    fn claude_access_levels_map_to_permission_flags() {
        assert_eq!(
            agent_args(&ai("claude"), "hi", Access::ReadOnly, false),
            ["-p", "--model", "m", READ_ONLY_TOOLS, "hi"]
        );
        assert_eq!(
            agent_args(&ai("claude"), "hi", Access::Edit, false),
            ["-p", "--model", "m", "--permission-mode", "acceptEdits", "hi"]
        );
        assert_eq!(
            agent_args(&ai("claude"), "hi", Access::Full, true),
            ["-p", "--model", "m", "--dangerously-skip-permissions", "--output-format", "stream-json", "--verbose", "hi"]
        );
    }

    #[test]
    fn opencode_and_omp_auto_approve_only_when_writing() {
        assert_eq!(agent_args(&ai("opencode"), "hi", Access::ReadOnly, false), ["run", "--model", "m", "hi"]);
        assert_eq!(agent_args(&ai("opencode"), "hi", Access::Edit, false), ["run", "--model", "m", "--auto", "hi"]);
        assert_eq!(agent_args(&ai("omp"), "hi", Access::ReadOnly, false), ["-p", "--model", "m", "hi"]);
        assert_eq!(agent_args(&ai("omp"), "hi", Access::Full, false), ["-p", "--auto-approve", "--model", "m", "hi"]);
    }

    #[test]
    fn live_runs_stream_json_and_resume() {
        assert_eq!(
            live_args(&ai("claude"), "hi", Access::ReadOnly, Some("s1")),
            ["-p", "--model", "m", READ_ONLY_TOOLS, "--output-format", "stream-json", "--verbose", "--resume", "s1", "hi"]
        );
        assert_eq!(
            live_args(&ai("opencode"), "hi", Access::Edit, Some("ses_1")),
            ["run", "--model", "m", "--auto", "--session", "ses_1", "--format", "json", "hi"]
        );
        assert_eq!(live_args(&ai("omp"), "hi", Access::ReadOnly, Some("x")), ["-p", "--model", "m", "hi"]);
    }

    #[test]
    fn unknown_agent_falls_back_to_claude() {
        assert_eq!(ai("whatever").agent_bin(), "claude");
        assert_eq!(agent_args(&ai("whatever"), "x", Access::ReadOnly, false)[0], "-p");
    }
}
