// Parses the live event streams agents print when run headless:
// Claude Code `--output-format stream-json`, OpenCode `run --format json` and
// Codex `exec --json`
// (one JSON object per line). What a UI feed needs: text, tool calls, the
// session id (to continue the conversation) and the final result.
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Event {
    Text { text: String },
    /// The model reasoning before it acts (Claude thinking blocks, OpenCode
    /// with --thinking): what fills the long pauses.
    Thinking { text: String },
    Tool { name: String, summary: String },
    /// The run's end: Claude's closing summary, Codex's turn end (no text).
    Result {
        text: String,
        is_error: bool,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        turns: Option<u64>,
        /// Input plus output tokens of the run, when the agent reports them.
        tokens: Option<u64>,
    },
    /// The conversation id to resume with (`--resume` / `--session`).
    Session { id: String },
    /// Claude: the model the CLI resolved and actually runs (its init line),
    /// which can differ from the alias Orbit asked for.
    Model { name: String },
    /// OpenCode: cost and tokens of one step (Claude and Codex report them in their Result).
    Cost { usd: f64, tokens: u64 },
}

/// Events in one stream line (a Claude assistant message can carry several
/// blocks). Non-JSON lines and uninteresting events yield nothing.
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
    if v.get("part").is_some() {
        return opencode(&v);
    }
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    if kind.starts_with("thread.") || kind.starts_with("turn.") || kind.starts_with("item.") {
        return codex(&v, kind);
    }
    if matches!(kind, "session" | "tool_execution_start" | "message_end" | "agent_end") {
        return pi(&v, kind);
    }
    match v.get("type").and_then(Value::as_str) {
        Some("system") => {
            let mut out: Vec<Event> =
                v.get("session_id").and_then(Value::as_str).map(|id| Event::Session { id: id.to_string() }).into_iter().collect();
            if let Some(name) = v.get("model").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                out.push(Event::Model { name: name.to_string() });
            }
            out
        }
        Some("assistant") => v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| blocks.iter().filter_map(block).collect())
            .unwrap_or_default(),
        Some("result") => vec![Event::Result {
            text: v.get("result").and_then(Value::as_str).unwrap_or("").to_string(),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
            duration_ms: v.get("duration_ms").and_then(Value::as_u64),
            turns: v.get("num_turns").and_then(Value::as_u64),
            tokens: usage_tokens(&v["usage"]),
        }],
        _ => vec![],
    }
}

/// OpenCode `run --format json`: `{type, sessionID, part: {...}}` per line.
fn opencode(v: &Value) -> Vec<Event> {
    let part = &v["part"];
    let mut out = Vec::new();
    if let Some(id) = v.get("sessionID").and_then(Value::as_str) {
        out.push(Event::Session { id: id.to_string() });
    }
    match v.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if !text.is_empty() {
                out.push(Event::Text { text: text.to_string() });
            }
        }
        Some("reasoning") => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if !text.is_empty() {
                out.push(Event::Thinking { text: text.to_string() });
            }
        }
        Some("tool_use") => {
            let raw = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let name = tool_name(raw);
            let input = part.pointer("/state/input").unwrap_or(&Value::Null);
            out.push(Event::Tool { summary: summarize(&name, input), name });
        }
        Some("step_finish") => {
            let usd = part.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
            let t = &part["tokens"];
            let tokens = [t["input"].as_u64(), t["output"].as_u64(), t["reasoning"].as_u64()].into_iter().flatten().sum();
            if usd > 0.0 || tokens > 0 {
                out.push(Event::Cost { usd, tokens });
            }
        }
        _ => {}
    }
    out
}

/// Codex `exec --json`: `thread.*`, `turn.*` and `item.*` events.
fn codex(v: &Value, kind: &str) -> Vec<Event> {
    let item = &v["item"];
    let s = |val: &Value| val.as_str().unwrap_or("").trim().to_string();
    let event = match (kind, item["type"].as_str().unwrap_or("")) {
        ("thread.started", _) => Some(Event::Session { id: s(&v["thread_id"]) }),
        ("item.started", "command_execution") => {
            Some(Event::Tool { name: "Bash".into(), summary: summarize("Bash", &serde_json::json!({ "command": unwrap_shell(item["command"].as_str().unwrap_or("")) })) })
        }
        ("item.completed", "agent_message") => Some(Event::Text { text: s(&item["text"]) }),
        ("item.completed", "reasoning") => Some(Event::Thinking { text: s(&item["text"]) }),
        ("item.completed", "file_change") => {
            let paths: Vec<&str> = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
            Some(Event::Tool { name: "Edit".into(), summary: paths.join(", ") })
        }
        ("item.started", "mcp_tool_call") => Some(Event::Tool { name: s(&item["tool"]), summary: s(&item["server"]) }),
        ("item.started", "web_search") => Some(Event::Tool { name: "WebSearch".into(), summary: s(&item["query"]) }),
        ("turn.completed", _) => Some(Event::Result { text: String::new(), is_error: false, cost_usd: None, duration_ms: None, turns: None, tokens: usage_tokens(&v["usage"]) }),
        ("turn.failed", _) => Some(Event::Result { text: s(&v["error"]["message"]), is_error: true, cost_usd: None, duration_ms: None, turns: None, tokens: None }),
        _ => None,
    };
    event.into_iter().filter(|e| !matches!(e, Event::Text { text } | Event::Thinking { text } if text.is_empty())).collect()
}

/// omp (pi's `--mode json`): `session`, `tool_execution_*`, `message_*`,
/// `agent_end`. Messages are read whole at `message_end`.
fn pi(v: &Value, kind: &str) -> Vec<Event> {
    let msg = &v["message"];
    match kind {
        "session" => v["id"].as_str().map(|id| Event::Session { id: id.to_string() }).into_iter().collect(),
        "tool_execution_start" => {
            let name = tool_name(v["toolName"].as_str().unwrap_or("tool"));
            vec![Event::Tool { summary: summarize(&name, &v["args"]), name }]
        }
        "message_end" if msg["role"] == "assistant" => {
            let mut out: Vec<Event> = msg["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let text = |k: &str| b[k].as_str().map(str::trim).filter(|t| !t.is_empty()).map(String::from);
                    match b["type"].as_str()? {
                        "text" => text("text").map(|text| Event::Text { text }),
                        "thinking" => text("thinking").map(|text| Event::Thinking { text }),
                        _ => None,
                    }
                })
                .collect();
            let u = &msg["usage"];
            let usd = u["cost"]["total"].as_f64().unwrap_or(0.0);
            let tokens = ["input", "output", "cacheRead", "cacheWrite"].iter().filter_map(|k| u[*k].as_u64()).sum();
            if usd > 0.0 || tokens > 0 {
                out.push(Event::Cost { usd, tokens });
            }
            out
        }
        "agent_end" => {
            // The last assistant message's error, if the run ended on one.
            let error = v["messages"].as_array().and_then(|m| m.iter().rev().find(|m| m["role"] == "assistant")).and_then(|m| m["errorMessage"].as_str()).unwrap_or("");
            vec![Event::Result { text: error.to_string(), is_error: !error.is_empty(), cost_usd: None, duration_ms: None, turns: None, tokens: None }]
        }
        _ => vec![],
    }
}

/// Input (cached and cache writes included) plus output tokens of a Claude or
/// Codex `usage` object.
pub(crate) fn usage_tokens(usage: &Value) -> Option<u64> {
    let keys = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"];
    let n: u64 = keys.iter().filter_map(|k| usage[*k].as_u64()).sum();
    (n > 0).then_some(n)
}

/// "/usr/bin/bash -lc 'cargo test'" → "cargo test": Codex wraps every command in a shell.
fn unwrap_shell(cmd: &str) -> &str {
    let shell = cmd.split_whitespace().next().is_some_and(|b| b.ends_with("sh"));
    match cmd.split_once(" -lc ").or_else(|| cmd.split_once(" -c ")) {
        Some((_, inner)) if shell => inner.trim().trim_matches(['\'', '"']),
        _ => cmd,
    }
}

/// OpenCode names tools in lowercase ("read", "bash"): Claude's casing, so
/// both agents read the same in a feed.
fn tool_name(raw: &str) -> String {
    let mut c = raw.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => raw.to_string(),
    }
}

fn block(b: &Value) -> Option<Event> {
    match b.get("type")?.as_str()? {
        "text" => {
            let text = b.get("text")?.as_str()?.trim();
            (!text.is_empty()).then(|| Event::Text { text: text.to_string() })
        }
        "thinking" => {
            let text = b.get("thinking")?.as_str()?.trim();
            (!text.is_empty()).then(|| Event::Thinking { text: text.to_string() })
        }
        "tool_use" => {
            let name = b.get("name")?.as_str()?.to_string();
            let summary = summarize(&name, b.get("input").unwrap_or(&Value::Null));
            Some(Event::Tool { name, summary })
        }
        _ => None,
    }
}

/// One line describing what a tool call touches ("src/x.rs", "cargo test").
pub fn summarize(name: &str, input: &Value) -> String {
    let s = |keys: &[&str]| keys.iter().find_map(|k| input.get(*k).and_then(Value::as_str)).unwrap_or("");
    let raw = match name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "Patch" => s(&["file_path", "filePath", "notebook_path", "path"]),
        "Bash" => s(&["command"]),
        "Grep" | "Glob" => s(&["pattern"]),
        "List" => s(&["path"]),
        "Task" | "Agent" => s(&["description"]),
        "WebFetch" | "Webfetch" => s(&["url"]),
        "WebSearch" | "Websearch" => s(&["query"]),
        "TodoWrite" | "Todowrite" => "update todo list",
        _ => input
            .as_object()
            .and_then(|o| o.values().find_map(Value::as_str))
            .unwrap_or(""),
    };
    let one_line = raw.lines().next().unwrap_or("").trim();
    if one_line.chars().count() > 140 {
        one_line.chars().take(140).collect::<String>() + "…"
    } else {
        one_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_blocks_and_result() {
        let l = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the PRD"},{"type":"tool_use","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"a"}}]}}"#;
        assert_eq!(
            parse_line(l),
            vec![
                Event::Text { text: "Reading the PRD".into() },
                Event::Tool { name: "Edit".into(), summary: "src/lib.rs".into() },
            ]
        );
        let r = r#"{"type":"result","subtype":"success","is_error":false,"result":"done <promise>COMPLETE</promise>","total_cost_usd":0.42,"duration_ms":1000,"num_turns":7}"#;
        assert_eq!(
            parse_line(r),
            vec![Event::Result {
                text: "done <promise>COMPLETE</promise>".into(),
                is_error: false,
                cost_usd: Some(0.42),
                duration_ms: Some(1000),
                turns: Some(7),
                tokens: None
            }]
        );
        let s = r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#;
        assert_eq!(parse_line(s), vec![Event::Session { id: "abc-123".into() }]);
        let m = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"claude-opus-5-5"}"#;
        assert_eq!(parse_line(m), vec![Event::Session { id: "abc-123".into() }, Event::Model { name: "claude-opus-5-5".into() }]);
    }

    #[test]
    fn parses_opencode_events() {
        let t = r#"{"type":"tool_use","sessionID":"ses_1","part":{"type":"tool","tool":"read","state":{"input":{"filePath":"/w/src/a.ts"}}}}"#;
        assert_eq!(
            parse_line(t),
            vec![Event::Session { id: "ses_1".into() }, Event::Tool { name: "Read".into(), summary: "/w/src/a.ts".into() }]
        );
        let x = r#"{"type":"text","sessionID":"ses_1","part":{"type":"text","text":" done. "}}"#;
        assert_eq!(parse_line(x), vec![Event::Session { id: "ses_1".into() }, Event::Text { text: "done.".into() }]);
        let f = r#"{"type":"step_finish","sessionID":"ses_1","part":{"type":"step-finish","cost":0.01}}"#;
        assert_eq!(parse_line(f), vec![Event::Session { id: "ses_1".into() }, Event::Cost { usd: 0.01, tokens: 0 }]);
        let t = r#"{"type":"step_finish","part":{"type":"step-finish","cost":0,"tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":0,"write":0}}}}"#;
        assert_eq!(parse_line(t), vec![Event::Cost { usd: 0.0, tokens: 125 }]);
    }

    #[test]
    fn parses_codex_events() {
        let p = |l: &str| parse_line(l);
        assert_eq!(p(r#"{"type":"thread.started","thread_id":"t-1"}"#), vec![Event::Session { id: "t-1".into() }]);
        assert!(p(r#"{"type":"turn.started"}"#).is_empty());
        let cmd = r#"{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/usr/bin/bash -lc 'cargo test'","status":"in_progress"}}"#;
        assert_eq!(p(cmd), vec![Event::Tool { name: "Bash".into(), summary: "cargo test".into() }]);
        assert!(p(&cmd.replace("item.started", "item.completed")).is_empty(), "a command shows once");
        assert_eq!(
            p(r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hello <task-done/>"}}"#),
            vec![Event::Text { text: "hello <task-done/>".into() }]
        );
        assert_eq!(
            p(r#"{"type":"item.completed","item":{"id":"i","type":"file_change","changes":[{"path":"/w/a.rs","kind":"update"}],"status":"completed"}}"#),
            vec![Event::Tool { name: "Edit".into(), summary: "/w/a.rs".into() }]
        );
        assert!(matches!(&p(r#"{"type":"turn.completed","usage":{"input_tokens":28209,"cached_input_tokens":23040,"output_tokens":43}}"#)[..], [Event::Result { is_error: false, tokens: Some(28252), .. }]));
        assert!(matches!(&p(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#)[..], [Event::Result { is_error: true, text, .. }] if text == "usage limit reached"));
        assert_eq!(unwrap_shell("ls -c foo"), "ls -c foo");
    }

    #[test]
    fn parses_pi_events() {
        let p = |l: &str| parse_line(l);
        assert_eq!(p(r#"{"type":"session","version":3,"id":"s-1","cwd":"/w"}"#), vec![Event::Session { id: "s-1".into() }]);
        assert_eq!(
            p(r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"cargo test"}}"#),
            vec![Event::Tool { name: "Bash".into(), summary: "cargo test".into() }]
        );
        let end = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Done <task-done/>"},{"type":"toolCall","id":"c2","name":"read","arguments":{}}],"usage":{"input":100,"output":20,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.01}}}}"#;
        assert_eq!(
            p(end),
            vec![Event::Thinking { text: "hm".into() }, Event::Text { text: "Done <task-done/>".into() }, Event::Cost { usd: 0.01, tokens: 120 }]
        );
        assert!(p(r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#).is_empty());
        assert!(matches!(&p(r#"{"type":"agent_end","messages":[]}"#)[..], [Event::Result { is_error: false, .. }]));
        assert!(matches!(&p(r#"{"type":"agent_end","messages":[{"role":"assistant","errorMessage":"429 rate limit"}]}"#)[..], [Event::Result { is_error: true, .. }]));
    }

    #[test]
    fn ignores_noise() {
        assert!(parse_line("plain text").is_empty());
        assert!(parse_line(r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#).is_empty());
    }

    #[test]
    fn summaries_are_single_short_lines() {
        let v = serde_json::json!({"command": "cargo test\necho done"});
        assert_eq!(summarize("Bash", &v), "cargo test");
        let long = serde_json::json!({"command": "x".repeat(300)});
        assert_eq!(summarize("Bash", &long).chars().count(), 141);
        assert_eq!(summarize("Mystery", &serde_json::json!({"a": "b"})), "b");
    }
}
