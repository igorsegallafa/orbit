// ACP agents (`opencode acp`, `omp acp`): JSON-RPC on stdio, one ACP session per chat session.
use super::{Agent, ChatBackend, ChatEvent, ChatInput, FileDiff, SlashCommand};
use crate::agent::stream;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Acp {
    agent: Arc<Agent>,
    parser: Arc<Mutex<Parser>>,
}

pub fn start(name: &str, cwd: &Path, resume: Option<String>, sink: impl Fn(ChatEvent) + Send + 'static) -> Result<Box<dyn ChatBackend>, String> {
    let key = super::session_key();
    let (launch_args, launch_env) = crate::agent_hooks::session_launch(name, &key);
    let mut cmd = crate::proc::cmd(name);
    cmd.arg("acp").args(launch_args).envs(launch_env).current_dir(cwd);
    let mcp_servers = match (crate::agent_hooks::hook_url(&key), std::env::current_exe()) {
        (Some(url), Ok(exe)) => json!([{ "name": "orbit", "command": exe, "args": ["--orbit-mcp", url], "env": [] }]),
        _ => json!([]),
    };
    let parser = Arc::new(Mutex::new(Parser { cwd: cwd.to_path_buf(), resume, mcp_servers, ..Parser::default() }));
    let agent = super::spawn_rpc(cmd, name, parser.clone(), sink)?;
    let init = parser.lock().unwrap().request(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true }, "terminal": false },
            "clientInfo": { "name": "orbit", "title": "Orbit", "version": env!("CARGO_PKG_VERSION") },
        }),
    );
    agent.write(&init);
    Ok(Box::new(Acp { agent, parser }))
}

impl ChatBackend for Acp {
    fn send(&self, input: ChatInput) {
        let line = {
            let mut p = self.parser.lock().unwrap();
            match input {
                ChatInput::Prompt { text, images } => p.prompt(&text, &images),
                ChatInput::Interrupt => json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": p.session_id } }),
                ChatInput::AnswerPermission { .. } => return,
            }
        };
        self.agent.write(&line);
    }

    fn stop(&self) {
        self.agent.stop();
    }
}

#[derive(Default)]
struct Tool {
    started: bool,
    kind: String,
    title: String,
    input: Value,
    /// Text content so far; ACP resends the whole content list on each update.
    output: String,
}

#[derive(Default)]
struct Parser {
    cwd: PathBuf,
    resume: Option<String>,
    mcp_servers: Value,
    next_id: u64,
    /// Our requests waiting for a response: id → method.
    pending: HashMap<u64, String>,
    /// Lines the reader must write back.
    outbox: Vec<Value>,
    session_id: String,
    tools: HashMap<String, Tool>,
    /// usage_update's cost is the session total; a turn costs the growth since the last turn ended.
    cost: Option<f64>,
    cost_before_turn: f64,
}

impl super::RpcParser for Parser {
    fn line(&mut self, line: &str) -> Vec<ChatEvent> {
        Parser::line(self, line)
    }
    fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }
}

impl Parser {
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.pending.insert(self.next_id, method.into());
        json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params })
    }

    fn prompt(&mut self, text: &str, images: &[PathBuf]) -> Value {
        let mut prompt = vec![json!({ "type": "text", "text": text })];
        prompt.extend(images.iter().filter_map(|p| super::encode_image(p)).map(|(media, data)| json!({ "type": "image", "mimeType": media, "data": data })));
        let session = self.session_id.clone();
        self.request("session/prompt", json!({ "sessionId": session, "prompt": prompt }))
    }

    fn line(&mut self, line: &str) -> Vec<ChatEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
        let method = v["method"].as_str().unwrap_or("");
        if !v["id"].is_null() && method.is_empty() {
            let request = v["id"].as_u64().and_then(|id| self.pending.remove(&id)).unwrap_or_default();
            return self.response(&request, &v);
        }
        if !v["id"].is_null() {
            // shortcut: agent requests (permissions, fs) are refused until Orbit serves them.
            self.outbox.push(json!({ "jsonrpc": "2.0", "id": v["id"], "error": { "code": -32601, "message": format!("Orbit does not handle {method}") } }));
            return vec![];
        }
        match method {
            "session/update" => self.update(&v["params"]["update"]),
            _ => vec![],
        }
    }

    fn response(&mut self, request: &str, v: &Value) -> Vec<ChatEvent> {
        if let Some(message) = v["error"]["message"].as_str() {
            self.tools.clear();
            return match request {
                "session/new" | "session/load" | "session/prompt" => vec![ChatEvent::TurnEnded { cost_usd: None, tokens: None, error: Some(message.into()) }],
                _ => vec![],
            };
        }
        let r = &v["result"];
        match request {
            "initialize" => {
                let params = json!({ "cwd": self.cwd, "mcpServers": self.mcp_servers });
                let next = match self.resume.clone() {
                    Some(id) => self.request("session/load", json!({ "sessionId": id, "cwd": params["cwd"], "mcpServers": params["mcpServers"] })),
                    None => self.request("session/new", params),
                };
                self.outbox.push(next);
                vec![]
            }
            "session/new" => {
                self.session_id = r["sessionId"].as_str().unwrap_or("").into();
                vec![ChatEvent::SessionId(self.session_id.clone())]
            }
            "session/load" => {
                self.session_id = self.resume.clone().unwrap_or_default();
                // The cost of the history a load replays isn't the next turn's.
                self.cost_before_turn = self.cost.unwrap_or(0.0);
                vec![ChatEvent::SessionId(self.session_id.clone())]
            }
            "session/prompt" => {
                self.tools.clear();
                let usage = &r["usage"];
                let tokens = usage["inputTokens"].as_u64().unwrap_or(0) + usage["outputTokens"].as_u64().unwrap_or(0);
                let cost_usd = self.cost.map(|c| c - self.cost_before_turn);
                self.cost_before_turn = self.cost.unwrap_or(0.0);
                let error = r["stopReason"].as_str().filter(|s| *s != "end_turn").map(String::from);
                vec![ChatEvent::TurnEnded { cost_usd, tokens: (tokens > 0).then_some(tokens), error }]
            }
            _ => vec![],
        }
    }

    fn update(&mut self, u: &Value) -> Vec<ChatEvent> {
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => text(&u["content"]).map(|text| ChatEvent::TextDelta { message_id: s(&u["messageId"]), text }).into_iter().collect(),
            "agent_thought_chunk" => text(&u["content"]).map(|text| ChatEvent::ThinkingDelta { message_id: s(&u["messageId"]), text }).into_iter().collect(),
            "tool_call" | "tool_call_update" => self.tool(u),
            "available_commands_update" => {
                let commands = u["availableCommands"].as_array().into_iter().flatten();
                vec![ChatEvent::Commands(commands.map(|c| SlashCommand { name: s(&c["name"]), description: s(&c["description"]) }).collect())]
            }
            "usage_update" => {
                self.cost = u["cost"]["amount"].as_f64().or(self.cost);
                vec![]
            }
            _ => vec![],
        }
    }

    fn tool(&mut self, u: &Value) -> Vec<ChatEvent> {
        let id = u["toolCallId"].as_str().unwrap_or("").to_string();
        let t = self.tools.entry(id.clone()).or_default();
        if let Some(kind) = u["kind"].as_str() {
            t.kind = kind.into();
        }
        if !t.started {
            if let Some(title) = u["title"].as_str().or(u["name"].as_str()) {
                t.title = title.into();
            }
        }
        if u["rawInput"].as_object().is_some_and(|o| !o.is_empty()) {
            t.input = u["rawInput"].clone();
        }
        let status = u["status"].as_str().unwrap_or("");
        let done = matches!(status, "completed" | "failed");
        let mut events = vec![];
        if !t.started {
            let name = match t.kind.as_str() {
                "read" => "Read",
                "edit" => "Edit",
                "delete" => "Delete",
                "move" => "Move",
                "search" => "Grep",
                "execute" => "Bash",
                "fetch" => "WebFetch",
                _ => t.title.as_str(),
            }
            .to_string();
            let summary = stream::summarize(&name, &t.input);
            // A pending call's input may still be streaming in (opencode sends `{cwd}` before the command).
            if !summary.is_empty() || status != "pending" {
                t.started = true;
                let summary = if summary.is_empty() || summary == name { t.title.clone() } else { summary };
                events.push(ChatEvent::ToolStarted { id: id.clone(), name, summary });
            }
        }
        let content = u["content"].as_array();
        if let Some(content) = content {
            let output: Vec<String> = content.iter().filter(|c| c["type"] == "content").filter_map(|c| text(&c["content"])).collect();
            let output = output.join("\n");
            let new = output.strip_prefix(t.output.as_str()).unwrap_or(&output);
            if !new.is_empty() {
                events.push(ChatEvent::ToolOutput { id: id.clone(), text: new.to_string() });
            }
            t.output = output;
        }
        if done {
            self.tools.remove(&id);
            let diff = content.into_iter().flatten().find(|c| c["type"] == "diff").and_then(|c| {
                Some(FileDiff { path: c["path"].as_str()?.into(), old_text: c["oldText"].as_str().unwrap_or("").into(), new_text: c["newText"].as_str().unwrap_or("").into() })
            });
            events.push(ChatEvent::ToolFinished { id, ok: status == "completed", diff });
        }
        events
    }
}

fn text(block: &Value) -> Option<String> {
    (block["type"] == "text").then(|| block["text"].as_str()).flatten().filter(|t| !t.is_empty()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/chat/acp/");

    /// Replays a recording; `>> ` lines are what the recorder sent, its requests registered as pending.
    fn replay(name: &str) -> (Parser, Vec<ChatEvent>) {
        let mut p = Parser::default();
        let mut events = vec![];
        for line in std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap().lines() {
            match line.strip_prefix(">> ") {
                Some(sent) => {
                    let v: Value = serde_json::from_str(sent).unwrap();
                    if let (Some(id), Some(method)) = (v["id"].as_u64(), v["method"].as_str()) {
                        p.pending.insert(id, method.into());
                    }
                }
                None => events.extend(p.line(line)),
            }
        }
        (p, events)
    }

    fn started(events: &[ChatEvent]) -> Vec<(&str, &str)> {
        events.iter().filter_map(|e| if let ChatEvent::ToolStarted { name, summary, .. } = e { Some((name.as_str(), summary.as_str())) } else { None }).collect()
    }

    fn diff(events: &[ChatEvent]) -> Option<FileDiff> {
        events.iter().find_map(|e| if let ChatEvent::ToolFinished { ok: true, diff: Some(d), .. } = e { Some(d.clone()) } else { None })
    }

    fn joined(events: &[ChatEvent], thinking: bool) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                ChatEvent::TextDelta { text, .. } if !thinking => Some(text.as_str()),
                ChatEvent::ThinkingDelta { text, .. } if thinking => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn opencode_edit_turn() {
        let (p, events) = replay("opencode-edit.jsonl");
        assert_eq!(events[0], ChatEvent::SessionId("ses_e2f769eb9ffek3R3046uVjw3tH".into()));
        assert!(matches!(&events[1], ChatEvent::Commands(c) if c.len() == 2 && c[1].name == "review" && !c[1].description.is_empty()));
        assert_eq!(started(&events), [("Read", "/tmp/afx/a.txt"), ("Edit", "/tmp/afx/a.txt"), ("Bash", "ls")]);
        assert_eq!(diff(&events), Some(FileDiff { path: "/tmp/afx/a.txt".into(), old_text: "hello".into(), new_text: "bye".into() }));
        // The command's output came twice (in progress, then completed) but shows once.
        let ls: Vec<_> = events.iter().filter(|e| matches!(e, ChatEvent::ToolOutput { text, .. } if text == "a.txt\n")).collect();
        assert_eq!(ls.len(), 1);
        assert!(joined(&events, true).contains("need to read a.txt"));
        assert!(!joined(&events, false).is_empty());
        assert_eq!(events.iter().filter(|e| matches!(e, ChatEvent::ToolFinished { ok: true, .. })).count(), 3);
        assert_eq!(events.last(), Some(&ChatEvent::TurnEnded { cost_usd: Some(0.0), tokens: Some(115), error: None }));
        assert!(p.tools.is_empty());
    }

    #[test]
    fn omp_edit_turn() {
        let (_, events) = replay("omp-edit.jsonl");
        assert!(matches!(&events[0], ChatEvent::SessionId(id) if id.starts_with("01a1d089")));
        assert!(events.iter().any(|e| matches!(e, ChatEvent::Commands(c) if c.iter().any(|c| c.name == "compact"))));
        assert_eq!(started(&events), [("Read", "a.txt"), ("Edit", "Replacing hello with bye in a.txt"), ("Bash", "ls")]);
        assert_eq!(diff(&events), Some(FileDiff { path: "/tmp/afx2/a.txt".into(), old_text: "hello\n".into(), new_text: "bye\n".into() }));
        assert!(joined(&events, false).to_lowercase().contains("done"));
        let Some(ChatEvent::TurnEnded { cost_usd: Some(cost), tokens: Some(599), error: None }) = events.last() else { panic!("{:?}", events.last()) };
        assert!((cost - 0.00081743).abs() < 1e-9);
    }

    #[test]
    fn omp_cancelled_turn_carries_an_error() {
        let (_, events) = replay("omp-cancel.jsonl");
        assert!(!joined(&events, true).is_empty());
        assert_eq!(events.last(), Some(&ChatEvent::TurnEnded { cost_usd: None, tokens: None, error: Some("cancelled".into()) }));
    }

    #[test]
    fn handshake_creates_or_loads_the_session() {
        let mcp = json!([{ "name": "orbit", "command": "/o", "args": ["--orbit-mcp", "http://x"], "env": [] }]);
        let mut p = Parser { cwd: "/w".into(), mcp_servers: mcp.clone(), ..Parser::default() };
        let init = p.request("initialize", json!({}));
        assert!(p.line(&json!({ "id": init["id"], "result": {} }).to_string()).is_empty());
        assert_eq!(p.outbox[0]["method"], "session/new");
        assert_eq!(p.outbox[0]["params"], json!({ "cwd": "/w", "mcpServers": mcp }));

        let mut p = Parser { cwd: "/w".into(), resume: Some("s1".into()), mcp_servers: json!([]), ..Parser::default() };
        let init = p.request("initialize", json!({}));
        p.line(&json!({ "id": init["id"], "result": {} }).to_string());
        assert_eq!(p.outbox[0]["method"], "session/load");
        assert_eq!(p.outbox[0]["params"], json!({ "sessionId": "s1", "cwd": "/w", "mcpServers": [] }));
        assert_eq!(p.line(&json!({ "id": p.outbox[0]["id"], "result": null }).to_string()), [ChatEvent::SessionId("s1".into())]);
        assert_eq!(p.prompt("hi", &[])["params"], json!({ "sessionId": "s1", "prompt": [{ "type": "text", "text": "hi" }] }));
    }

    #[test]
    fn agent_requests_are_refused_not_left_hanging() {
        let mut p = Parser::default();
        assert!(p.line(r#"{"jsonrpc":"2.0","id":0,"method":"fs/read_text_file","params":{"path":"/x"}}"#).is_empty());
        assert_eq!(p.outbox[0]["id"], 0);
        assert!(p.outbox[0]["error"]["message"].is_string());
    }
}
