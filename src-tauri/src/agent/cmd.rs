// Builds the headless command line for the configured agent CLI.
use crate::config::AiSettings;
use std::process::Command;

/// What the agent may do on its own in a headless run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// Answers only; any write is denied.
    ReadOnly,
    /// File edits auto-approved (claude acceptEdits / opencode --auto).
    Edit,
    /// Everything auto-approved, shell included (autonomous loops).
    Full,
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
            if access != Access::ReadOnly {
                a.push("--auto".into());
            }
        }
        "omp" => {
            a.push("-p".into());
            if access != Access::ReadOnly {
                a.push("--auto-approve".into());
            }
            a.extend(["--model".into(), ai.model.clone()]);
        }
        _ => {
            a.extend(["-p".into(), "--model".into(), ai.model.clone()]);
            match access {
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
        AiSettings { agent: agent.into(), model: "m".into() }
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
    fn unknown_agent_falls_back_to_claude() {
        assert_eq!(ai("whatever").agent_bin(), "claude");
        assert_eq!(agent_args(&ai("whatever"), "x", Access::ReadOnly, false)[0], "-p");
    }
}
