// Guards an iteration against hanging after its work is done: what the
// agent is doing (from its stream), the stop rules for the runner, and
// the check of what a killed or cut-short iteration actually left behind.
use super::supervisor::looks_like_limit;
use super::RunConfig;
use crate::agent::runner::Watch;
use crate::agent::stream::{parse_line, summarize, Event};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// The live iteration's activity, for "last event Xm ago" in the UI.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// Unix seconds of the agent's last stream line.
    pub last_event_at: Option<u64>,
    /// The tool call still waiting for its result ("Bash: cargo test").
    pub tool: Option<String>,
    /// Background commands started and not killed.
    pub background: Vec<String>,
}

/// Follows tool calls (Claude's stream-json, Codex's items) and Claude's background shells.
#[derive(Default)]
pub struct Tracker {
    /// tool_use id → label, oldest first.
    pending: Vec<(String, String)>,
    /// tool_use id → command of a `run_in_background` Bash call.
    bg_calls: HashMap<String, String>,
    /// shell id → command.
    shells: Vec<(String, String)>,
}

impl Tracker {
    /// Reads one stdout line; true when the visible activity changed.
    pub fn line(&mut self, line: &str, relativize: impl Fn(&str) -> String) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return false };
        let blocks = v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
        let before = (self.tool(), self.background());
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("tool");
                    let input = &b["input"];
                    let summary = relativize(&summarize(name, input));
                    if name == "Bash" && input["run_in_background"].as_bool() == Some(true) {
                        self.bg_calls.insert(id.clone(), summary.clone());
                    }
                    if matches!(name, "KillShell" | "KillBash") {
                        let shell = input["shell_id"].as_str().or(input["bash_id"].as_str()).unwrap_or("");
                        self.shells.retain(|(s, _)| s != shell);
                    }
                    self.pending.push((id, format!("{name}: {summary}")));
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    let id = b["tool_use_id"].as_str().unwrap_or("");
                    self.pending.retain(|(p, _)| p != id);
                    if let Some(cmd) = self.bg_calls.remove(id) {
                        let shell = shell_id(&result_text(&b["content"])).unwrap_or_else(|| id.to_string());
                        self.shells.push((shell, cmd));
                    }
                }
            }
            Some("result") | Some("turn.completed") | Some("turn.failed") => self.pending.clear(),
            // Codex: a tool runs from its item.started to its item.completed.
            Some("item.started") => {
                for e in summarize_codex(line) {
                    self.pending.push((v["item"]["id"].as_str().unwrap_or("").to_string(), relativize(&e)));
                }
            }
            Some("item.completed") => {
                let id = v["item"]["id"].as_str().unwrap_or("");
                self.pending.retain(|(p, _)| p != id);
            }
            _ => {}
        }
        before != (self.tool(), self.background())
    }

    pub fn tool(&self) -> Option<String> {
        self.pending.last().map(|(_, l)| l.clone())
    }

    pub fn background(&self) -> Vec<String> {
        self.shells.iter().map(|(_, c)| c.clone()).collect()
    }
}

/// "Bash: cargo test" for each tool a Codex line starts.
fn summarize_codex(line: &str) -> Vec<String> {
    parse_line(line)
        .into_iter()
        .filter_map(|e| match e {
            Event::Tool { name, summary } => Some(format!("{name}: {summary}")),
            _ => None,
        })
        .collect()
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// "Command running in background with ID: abc123" → "abc123".
fn shell_id(text: &str) -> Option<String> {
    let rest = &text[text.find("ID: ")? + 4..];
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

/// The runner's stop rules for one iteration (0 turns a rule off).
pub fn watch(cfg: &RunConfig) -> Watch {
    let secs = |s: f64| (s > 0.0).then(|| Duration::from_secs_f64(s));
    Watch { idle: secs(cfg.idle_timeout_min * 60.0), exit_grace: secs(cfg.exit_grace_sec as f64) }
}

/// What an iteration left behind, checked when it didn't end cleanly.
#[derive(Debug, PartialEq)]
pub struct Check {
    /// A `feat: [<id>]` commit since the iteration started.
    pub committed: bool,
    /// Nothing uncommitted outside Orbit's own files.
    pub clean: bool,
    /// The agent's last message doesn't report a failure.
    pub message_ok: bool,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.committed && self.clean && self.message_ok
    }
}

/// `ignore`: repo-relative folders that don't count as uncommitted work.
pub fn check(dir: &Path, head_before: &Option<String>, id: &str, ignore: &[&str], last_message: &str, is_error: bool) -> Check {
    Check {
        committed: story_committed(dir, head_before, id),
        clean: tree_clean(dir, ignore),
        message_ok: !reports_failure(last_message, is_error),
    }
}

/// The story's commit is in and nothing else is left uncommitted: the
/// iteration's work is done, whatever the agent prints or whether it exits.
pub fn work_done(dir: &Path, head_before: &Option<String>, id: &str, ignore: &[&str]) -> bool {
    story_committed(dir, head_before, id) && tree_clean(dir, ignore)
}

fn story_committed(dir: &Path, from: &Option<String>, id: &str) -> bool {
    let range = from.as_ref().map(|f| format!("{f}..HEAD")).unwrap_or_else(|| "HEAD".into());
    let prefix = format!("feat: [{id}]");
    crate::proc::run("git", &["log", "--format=%s", &range], Some(dir))
        .is_ok_and(|out| out.lines().any(|l| l.trim_start().starts_with(&prefix)))
}

fn tree_clean(dir: &Path, ignore: &[&str]) -> bool {
    let mut args: Vec<String> = ["status", "--porcelain", "--", "."].map(String::from).to_vec();
    args.extend(ignore.iter().map(|p| format!(":(exclude){p}")));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::proc::run("git", &args, Some(dir)).is_ok_and(|out| out.trim().is_empty())
}

fn reports_failure(text: &str, is_error: bool) -> bool {
    is_error || text.contains("<task-blocked>") || looks_like_limit(text)
}

/// Commits on HEAD that no remote branch has.
pub fn unpushed(dir: &Path) -> u32 {
    crate::proc::run("git", &["rev-list", "--count", "HEAD", "--not", "--remotes"], Some(dir))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Fixed rules in every iteration prompt: the hang they prevent is an agent
/// waiting on its own background work, which keeps `claude -p` alive.
pub const BACKGROUND_RULES: &str = "## Background Processes

- Wait on the PID you started (`wait $pid`, `kill -0 $pid`), never on `pgrep -f <pattern>`: the pattern matches your own shell and the wait never ends.
- Bound every wait or polling loop with `timeout` (e.g. `timeout 600 sh -c 'until …; do sleep 5; done'`).
- Stop or kill every background task you started before you finish.";

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn tracks_the_running_tool_and_background_shells() {
        let mut t = Tracker::default();
        let rel = |s: &str| id(s);
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert!(t.line(call, rel));
        assert_eq!(t.tool().as_deref(), Some("Bash: cargo test"));
        assert!(t.line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#, rel));
        assert_eq!(t.tool(), None);

        let bg = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"npm run dev","run_in_background":true}}]}}"#;
        t.line(bg, rel);
        t.line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"Command running in background with ID: b7x"}]}]}}"#, rel);
        assert_eq!(t.background(), ["npm run dev"]);
        t.line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"KillShell","input":{"shell_id":"b7x"}}]}}"#, rel);
        assert!(t.background().is_empty());
        assert!(!t.line("not json", rel));

        let mut t = Tracker::default();
        let codex = r#"{"type":"item.started","item":{"id":"item_3","type":"command_execution","command":"/bin/bash -lc 'cargo test'"}}"#;
        assert!(t.line(codex, rel));
        assert_eq!(t.tool().as_deref(), Some("Bash: cargo test"));
        assert!(t.line(&codex.replace("item.started", "item.completed"), rel));
        assert_eq!(t.tool(), None);
    }

    #[test]
    fn failure_reports() {
        assert!(reports_failure("x", true));
        assert!(reports_failure("<task-blocked>no db</task-blocked>", false));
        assert!(reports_failure("You've hit your usage limit", false));
        assert!(!reports_failure("Implemented T1.\n<task-done/>", false));
    }

    #[test]
    fn watch_turns_off_at_zero_and_allows_fractions() {
        let mut c = super::super::default_config();
        c.idle_timeout_min = 0.5;
        c.exit_grace_sec = 0;
        let w = watch(&c);
        assert_eq!(w.idle, Some(Duration::from_secs(30)));
        assert_eq!(w.exit_grace, None);
    }
}

/// Scratch repos, stub agents and run state for the iteration tests.
#[cfg(all(test, unix))]
pub mod fixture {
    use super::super::{runs, RunConfig, RunState};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    pub fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-ralph-guard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    pub fn git(dir: &Path, args: &[&str]) -> String {
        crate::proc::run("git", args, Some(dir)).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    /// A repo at `work` with one commit, pushed to a local bare `remote`.
    pub fn repo(work: &Path, remote: &Path) {
        std::fs::create_dir_all(work).unwrap();
        std::fs::create_dir_all(remote).unwrap();
        git(remote, &["init", "-q", "--bare"]);
        git(work, &["init", "-q", "-b", "feat/x"]);
        git(work, &["config", "user.email", "t@t"]);
        git(work, &["config", "user.name", "t"]);
        git(work, &["config", "commit.gpgsign", "false"]);
        std::fs::write(work.join("README"), "x\n").unwrap();
        git(work, &["add", "."]);
        git(work, &["commit", "-qm", "init"]);
        git(work, &["remote", "add", "origin", &remote.to_string_lossy()]);
        git(work, &["push", "-qu", "origin", "HEAD"]);
    }

    /// A shell script standing in for the agent; it records its pid (the
    /// process group id) in `<root>/pid`.
    pub fn stub(root: &Path, body: &str) -> PathBuf {
        let p = root.join("agent.sh");
        std::fs::write(&p, format!("echo $$ > '{}'\n{body}\n", root.join("pid").display())).unwrap();
        p
    }

    pub fn pgid(root: &Path) -> String {
        std::fs::read_to_string(root.join("pid")).unwrap().trim().to_string()
    }

    /// No process left in the group.
    pub fn group_empty(pgid: &str) -> bool {
        let o = crate::proc::cmd("pgrep").args(["-g", pgid]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().is_empty()
    }

    pub fn events(dir: &Path) -> Vec<serde_json::Value> {
        super::super::read_jsonl(&dir.join("events.jsonl"))
    }

    pub fn start_run(key: &str, dir: &Path, config: &RunConfig) {
        runs().lock().unwrap().insert(
            key.to_string(),
            RunState {
                id: key.to_string(),
                repo: "x".into(),
                running: true,
                started_at: 0,
                finished_at: None,
                reason: None,
                iteration: 0,
                current_story: None,
                passed: 0,
                total: 0,
                commits: 0,
                cost_usd: 0.0,
                tokens: 0,
                limit_until: None,
                model: None,
                config: config.clone(),
                activity: Default::default(),
                unpushed: 0,
                events: VecDeque::new(),
                stop: false,
                pause: false,
                dir: dir.to_path_buf(),
            },
        );
    }

    /// Short limits so the tests run in seconds.
    pub fn quick() -> RunConfig {
        RunConfig { idle_timeout_min: 0.05, exit_grace_sec: 1, push: true, ..Default::default() }
    }

    /// The agent reports the task done (text, then its result line).
    pub const RESULT_DONE: &str = r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Implemented it."}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Implemented it.\n<task-done/>","num_turns":3}'"#;
}
