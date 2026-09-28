// Parses the live event streams agents print when run headless:
// Claude Code `--output-format stream-json` and OpenCode `run --format json`
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
    /// Claude only: its closing summary of the run.
    Result {
        text: String,
        is_error: bool,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        turns: Option<u64>,
    },
    /// The conversation id to resume with (`--resume` / `--session`).
    Session { id: String },
    /// OpenCode: cost of one step (Claude reports it in its Result).
    Cost { usd: f64 },
}

/// Events in one stream line (a Claude assistant message can carry several
/// blocks). Non-JSON lines and uninteresting events yield nothing.
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
    if v.get("part").is_some() {
        return opencode(&v);
    }
    match v.get("type").and_then(Value::as_str) {
        Some("system") => v
            .get("session_id")
            .and_then(Value::as_str)
            .map(|id| vec![Event::Session { id: id.to_string() }])
            .unwrap_or_default(),
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
            if let Some(usd) = part.get("cost").and_then(Value::as_f64).filter(|c| *c > 0.0) {
                out.push(Event::Cost { usd });
            }
        }
        _ => {}
    }
    out
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
                turns: Some(7)
            }]
        );
        let s = r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#;
        assert_eq!(parse_line(s), vec![Event::Session { id: "abc-123".into() }]);
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
        assert_eq!(parse_line(f), vec![Event::Session { id: "ses_1".into() }, Event::Cost { usd: 0.01 }]);
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
