// Claude Code over stream-json: one long-lived `claude -p` process, user
// messages in as NDJSON on stdin, partial-message stream events out.
use super::{ChatBackend, ChatEvent, ChatInput, FileDiff, SlashCommand};
use crate::agent::{runner, stream};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

struct Claude {
    stdin: Mutex<ChildStdin>,
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

pub fn start(cwd: &Path, resume: Option<String>, sink: impl Fn(ChatEvent) + Send + 'static) -> Result<Box<dyn ChatBackend>, String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let key = format!("chat:{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let (launch_args, launch_env) = crate::agent_hooks::session_launch("claude", &key);
    let mut cmd = crate::proc::cmd("claude");
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    if let Some(id) = &resume {
        cmd.args(["--resume", id]);
    }
    cmd.args(launch_args).envs(launch_env).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Own process group, so stop() takes the agent's shells and builds down with it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| format!("could not start claude: {e}"))?;
    let stdin = child.stdin.take().ok_or("claude has no stdin")?;
    let stdout = child.stdout.take().ok_or("claude has no stdout")?;
    let stderr = child.stderr.take().ok_or("claude has no stderr")?;
    let child = Arc::new(Mutex::new(child));
    let stopped = Arc::new(AtomicBool::new(false));

    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let tail_w = tail.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut t = tail_w.lock().unwrap();
            t.push(line);
            if t.len() > 20 {
                t.remove(0);
            }
        }
    });
    let (child_r, stopped_r) = (child.clone(), stopped.clone());
    std::thread::spawn(move || {
        let mut parser = Parser::default();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            parser.line(&line).into_iter().for_each(&sink);
        }
        let status = child_r.lock().unwrap().wait();
        let error = match status {
            _ if stopped_r.load(Ordering::Relaxed) => None,
            Ok(s) if s.success() => None,
            Ok(s) => Some(Some(tail.lock().unwrap().join("\n")).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("claude exited with {s}"))),
            Err(e) => Some(e.to_string()),
        };
        sink(ChatEvent::Exited { error });
    });
    Ok(Box::new(Claude { stdin: Mutex::new(stdin), child, stopped }))
}

impl ChatBackend for Claude {
    fn send(&self, input: ChatInput) {
        let line = match input {
            ChatInput::Prompt { text, images } => user_message(&text, &images),
            // SIGINT ends `claude -p` itself; the control request stops only the turn.
            ChatInput::Interrupt => json!({ "type": "control_request", "request_id": "interrupt", "request": { "subtype": "interrupt" } }),
            ChatInput::AnswerPermission { .. } => return,
        };
        // A failed write means the process is gone; the reader thread reports Exited.
        let _ = writeln!(self.stdin.lock().unwrap(), "{line}");
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        runner::kill_tree(&mut self.child.lock().unwrap());
    }
}

fn user_message(text: &str, images: &[PathBuf]) -> Value {
    let mut content = vec![json!({ "type": "text", "text": text })];
    for path in images {
        let media = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/png",
        };
        if let Ok(bytes) = std::fs::read(path) {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } }));
        }
    }
    json!({ "type": "user", "message": { "role": "user", "content": content } })
}

/// Text and thinking come from stream events; tool calls from whole assistant messages, which carry the full input.
#[derive(Default)]
struct Parser {
    message_id: String,
    /// Tool calls waiting for their result: id → (name, input).
    tools: HashMap<String, (String, Value)>,
}

impl Parser {
    fn line(&mut self, line: &str) -> Vec<ChatEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        match v["type"].as_str().unwrap_or("") {
            "system" if v["subtype"] == "init" => {
                let commands = v["slash_commands"].as_array().into_iter().flatten().filter_map(Value::as_str);
                vec![
                    ChatEvent::SessionId(s(&v["session_id"])),
                    ChatEvent::Commands(commands.map(|name| SlashCommand { name: name.into(), description: String::new() }).collect()),
                ]
            }
            "stream_event" => {
                let e = &v["event"];
                match e["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        self.message_id = s(&e["message"]["id"]);
                        vec![]
                    }
                    "content_block_delta" => {
                        let d = &e["delta"];
                        let message_id = self.message_id.clone();
                        match d["type"].as_str().unwrap_or("") {
                            "text_delta" => vec![ChatEvent::TextDelta { message_id, text: s(&d["text"]) }],
                            "thinking_delta" if !s(&d["thinking"]).is_empty() => vec![ChatEvent::ThinkingDelta { message_id, text: s(&d["thinking"]) }],
                            _ => vec![],
                        }
                    }
                    _ => vec![],
                }
            }
            "assistant" => blocks(&v)
                .filter(|b| b["type"] == "tool_use")
                .map(|b| {
                    let (id, name) = (s(&b["id"]), s(&b["name"]));
                    let summary = stream::summarize(&name, &b["input"]);
                    self.tools.insert(id.clone(), (name.clone(), b["input"].clone()));
                    ChatEvent::ToolStarted { id, name, summary }
                })
                .collect(),
            "user" => blocks(&v)
                .filter(|b| b["type"] == "tool_result")
                .flat_map(|b| {
                    let id = s(&b["tool_use_id"]);
                    let ok = !b["is_error"].as_bool().unwrap_or(false);
                    let diff = self.tools.remove(&id).and_then(|(name, input)| ok.then(|| diff(&name, &input)).flatten());
                    let text = result_text(&b["content"]);
                    let output = (!text.is_empty()).then(|| ChatEvent::ToolOutput { id: id.clone(), text });
                    output.into_iter().chain([ChatEvent::ToolFinished { id, ok, diff }])
                })
                .collect(),
            "result" => {
                let is_error = v["is_error"].as_bool().unwrap_or(false);
                let error = is_error.then(|| Some(s(&v["result"])).filter(|r| !r.is_empty()).unwrap_or_else(|| s(&v["subtype"])));
                vec![ChatEvent::TurnEnded { cost_usd: v["total_cost_usd"].as_f64(), tokens: stream::usage_tokens(&v["usage"]), error }]
            }
            _ => vec![],
        }
    }
}

fn blocks(v: &Value) -> impl Iterator<Item = &Value> {
    v["message"]["content"].as_array().into_iter().flatten()
}

/// A tool_result's content: a string or a list of text blocks.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The edit a file tool made, rebuilt from its input.
fn diff(name: &str, input: &Value) -> Option<FileDiff> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let (old_text, new_text) = match name {
        "Edit" => (s(&input["old_string"]), s(&input["new_string"])),
        "Write" => (String::new(), s(&input["content"])),
        "MultiEdit" => {
            let edits = input["edits"].as_array()?;
            let join = |k: &str| edits.iter().map(|e| s(&e[k])).collect::<Vec<_>>().join("\n");
            (join("old_string"), join("new_string"))
        }
        _ => return None,
    };
    Some(FileDiff { path: input["file_path"].as_str()?.into(), old_text, new_text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay(name: &str) -> Vec<ChatEvent> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chat/claude").join(name);
        let mut p = Parser::default();
        std::fs::read_to_string(path).unwrap().lines().flat_map(|l| p.line(l)).collect()
    }

    #[test]
    fn edit_turn_fixture() {
        let events = replay("edit-turn.jsonl");
        assert_eq!(events[0], ChatEvent::SessionId("0de0bdb5-2a20-45d7-a8b6-d76317e12f61".into()));
        assert!(matches!(&events[1], ChatEvent::Commands(c) if c.iter().any(|c| c.name == "compact")));
        let tools: Vec<_> = events.iter().filter(|e| matches!(e, ChatEvent::ToolStarted { .. })).collect();
        assert!(matches!(tools[..], [ChatEvent::ToolStarted { name: r, summary: s, .. }, ChatEvent::ToolStarted { name: e, .. }] if r == "Read" && s == "/tmp/rfx/a.txt" && e == "Edit"));
        let diff = events.iter().find_map(|e| match e {
            ChatEvent::ToolFinished { ok: true, diff: Some(d), .. } => Some(d.clone()),
            _ => None,
        });
        assert_eq!(diff, Some(FileDiff { path: "/tmp/rfx/a.txt".into(), old_text: "hello".into(), new_text: "bye".into() }));
        assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolOutput { text, .. } if text.contains("hello"))));
        let text: String = events.iter().filter_map(|e| if let ChatEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(text, "done");
        assert!(matches!(events.last(), Some(ChatEvent::TurnEnded { cost_usd: Some(c), tokens: Some(t), error: None }) if *c > 0.0 && *t > 0));
    }

    #[test]
    fn interrupted_turn_ends_with_an_error() {
        let events = replay("interrupt.jsonl");
        let ends: Vec<_> = events.iter().filter(|e| matches!(e, ChatEvent::TurnEnded { .. })).collect();
        // The interrupted turn, then the follow-up prompt's turn on the same process.
        assert!(matches!(ends[..], [ChatEvent::TurnEnded { error: Some(_), .. }, ChatEvent::TurnEnded { error: None, .. }]));
    }

    #[test]
    fn thinking_and_multi_edit() {
        let mut p = Parser::default();
        p.line(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}"#);
        assert_eq!(
            p.line(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#),
            [ChatEvent::ThinkingDelta { message_id: "m1".into(), text: "hm".into() }]
        );
        let input = json!({"file_path": "a.rs", "edits": [{"old_string": "a", "new_string": "b"}, {"old_string": "c", "new_string": "d"}]});
        assert_eq!(diff("MultiEdit", &input), Some(FileDiff { path: "a.rs".into(), old_text: "a\nc".into(), new_text: "b\nd".into() }));
        assert_eq!(diff("Write", &json!({"file_path": "b.rs", "content": "x"})).map(|d| d.old_text), Some(String::new()));
        assert_eq!(diff("Read", &json!({"file_path": "b.rs"})), None);
    }

    #[test]
    fn prompt_line_carries_images_as_base64() {
        let dir = std::env::temp_dir().join(format!("orbit-chat-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("a.jpg");
        std::fs::write(&img, b"abc").unwrap();
        let v = user_message("hi", &[img, dir.join("missing.png")]);
        assert_eq!(v["message"]["content"][0], json!({"type": "text", "text": "hi"}));
        assert_eq!(v["message"]["content"][1]["source"], json!({"type": "base64", "media_type": "image/jpeg", "data": "YWJj"}));
        assert_eq!(v["message"]["content"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
