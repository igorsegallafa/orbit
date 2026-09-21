// Parses Claude Code `--output-format stream-json` lines into the few
// things the Ralph feed shows: assistant text, tool calls and the result.
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Event {
    Text { text: String },
    Tool { name: String, summary: String },
    Result {
        text: String,
        is_error: bool,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        turns: Option<u64>,
    },
}

/// Events in one stream-json line (an assistant message can carry several
/// blocks). Non-JSON lines and uninteresting events yield nothing.
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
    match v.get("type").and_then(Value::as_str) {
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

fn block(b: &Value) -> Option<Event> {
    match b.get("type")?.as_str()? {
        "text" => {
            let text = b.get("text")?.as_str()?.trim();
            (!text.is_empty()).then(|| Event::Text { text: text.to_string() })
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
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let raw = match name {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let p = s("file_path");
            if p.is_empty() { s("notebook_path") } else { p }
        }
        "Bash" => s("command"),
        "Grep" | "Glob" => s("pattern"),
        "Task" | "Agent" => s("description"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "TodoWrite" => "update todo list",
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
    }

    #[test]
    fn ignores_noise() {
        assert!(parse_line("plain text").is_empty());
        assert!(parse_line(r#"{"type":"system","subtype":"init"}"#).is_empty());
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
