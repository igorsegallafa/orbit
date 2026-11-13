// ACP agents (`opencode acp`, `omp acp`): JSON-RPC on stdio, one ACP session per chat session.
use super::{Agent, ChatBackend, ChatEvent, ChatInput, Choice, FileDiff, PermissionOption, SlashCommand, Turn};
use crate::agent::stream;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Acp {
    agent: Arc<Agent>,
    parser: Arc<Mutex<Parser>>,
}

pub fn start(name: &str, cwd: &Path, resume: Option<String>, key: &str, sink: impl Fn(ChatEvent) + Send + 'static) -> Result<Box<dyn ChatBackend>, String> {
    let (launch_args, launch_env) = crate::agent_hooks::session_launch(name, key, true, cwd);
    let mut cmd = crate::proc::cmd(name);
    cmd.arg("acp").args(launch_args).envs(launch_env).current_dir(cwd);
    let mcp_servers = match (crate::agent_hooks::hook_url(key), std::env::current_exe()) {
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
                ChatInput::AnswerPermission { id, option_id } => match p.answer(&id, &option_id) {
                    Some(reply) => reply,
                    None => return,
                },
                ChatInput::SetModel(value) => match p.model_config.clone() {
                    Some(id) => p.set_config(&id, &value),
                    None => return,
                },
                ChatInput::SetMode(value) => match p.mode_config.clone() {
                    Some(id) => p.set_config(&id, &value),
                    None => {
                        let session = p.session_id.clone();
                        p.request("session/set_mode", json!({ "sessionId": session, "modeId": value }))
                    }
                },
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
    /// Permission requests waiting for the user: PermissionAsked id → the agent's request id.
    approvals: HashMap<String, Value>,
    /// usage_update's cost is the session total; a turn costs the growth since the last turn ended.
    cost: Option<f64>,
    cost_before_turn: f64,
    /// While session/load runs: the turns its replayed updates make up.
    history: Option<Vec<Turn>>,
    /// The config option ids for the model and mode pickers, when the agent offers them that way.
    model_config: Option<String>,
    mode_config: Option<String>,
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

    fn set_config(&mut self, config: &str, value: &str) -> Value {
        let session = self.session_id.clone();
        self.request("session/set_config_option", json!({ "sessionId": session, "configId": config, "value": value }))
    }

    /// The model and mode pickers from a session's `configOptions` (or its older `modes`).
    fn pickers(&mut self, r: &Value) -> Vec<ChatEvent> {
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        let mut out = vec![];
        for o in r["configOptions"].as_array().into_iter().flatten() {
            let choices = o["options"].as_array().into_iter().flatten().map(|c| Choice::new(&s(&c["value"]), &s(&c["name"]), &s(&c["description"]))).collect();
            match o["category"].as_str() {
                Some("model") => {
                    self.model_config = Some(s(&o["id"]));
                    out.extend([ChatEvent::Models(choices), ChatEvent::Model(s(&o["currentValue"]))]);
                }
                Some("mode") => {
                    self.mode_config = Some(s(&o["id"]));
                    out.extend([ChatEvent::Modes(choices), ChatEvent::Mode(s(&o["currentValue"]))]);
                }
                _ => {}
            }
        }
        if self.mode_config.is_none() && r["modes"].is_object() {
            let choices = r["modes"]["availableModes"].as_array().into_iter().flatten().map(|m| Choice::new(&s(&m["id"]), &s(&m["name"]), &s(&m["description"]))).collect();
            out.extend([ChatEvent::Modes(choices), ChatEvent::Mode(s(&r["modes"]["currentModeId"]))]);
        }
        out
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
            return self.agent_request(method, &v);
        }
        match method {
            "session/update" if self.history.is_some() => self.replayed(&v["params"]["update"]),
            "session/update" => self.update(&v["params"]["update"]),
            _ => vec![],
        }
    }

    fn answer(&mut self, id: &str, option_id: &str) -> Option<Value> {
        let request_id = self.approvals.remove(id)?;
        Some(json!({ "jsonrpc": "2.0", "id": request_id, "result": { "outcome": { "outcome": "selected", "optionId": option_id } } }))
    }

    fn agent_request(&mut self, method: &str, v: &Value) -> Vec<ChatEvent> {
        let p = &v["params"];
        let result = match method {
            "session/request_permission" => {
                let call = &p["toolCall"];
                let detail: Vec<String> = call["content"].as_array().into_iter().flatten().filter_map(|c| text(&c["content"])).collect();
                let s = |v: &Value| v.as_str().unwrap_or("").to_string();
                let options = p["options"].as_array().into_iter().flatten().map(|o| PermissionOption { id: s(&o["optionId"]), label: s(&o["name"]), kind: s(&o["kind"]), description: String::new() }).collect();
                let id = v["id"].to_string();
                self.approvals.insert(id.clone(), v["id"].clone());
                return vec![ChatEvent::PermissionAsked { id, title: s(&call["title"]), detail: detail.join("\n"), options }];
            }
            "fs/read_text_file" => inside(&self.cwd, p["path"].as_str().unwrap_or("")).and_then(|path| std::fs::read_to_string(path).map_err(|e| e.to_string())).map(|text| {
                // `line` is 1-based; `limit` caps the line count.
                let skip = p["line"].as_u64().map_or(0, |l| l.saturating_sub(1)) as usize;
                let lines = text.split_inclusive('\n').skip(skip).take(p["limit"].as_u64().map_or(usize::MAX, |l| l as usize));
                json!({ "content": lines.collect::<String>() })
            }),
            "fs/write_text_file" => inside(&self.cwd, p["path"].as_str().unwrap_or("")).and_then(|path| std::fs::write(path, p["content"].as_str().unwrap_or("")).map_err(|e| e.to_string())).map(|_| Value::Null),
            _ => Err(format!("Orbit does not handle {method}")),
        };
        self.outbox.push(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": v["id"], "result": result }),
            Err(message) => json!({ "jsonrpc": "2.0", "id": v["id"], "error": { "code": -32603, "message": message } }),
        });
        vec![]
    }

    fn response(&mut self, request: &str, v: &Value) -> Vec<ChatEvent> {
        if let Some(message) = v["error"]["message"].as_str() {
            self.tools.clear();
            self.history = None;
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
                    Some(id) => {
                        self.history = Some(vec![]);
                        self.request("session/load", json!({ "sessionId": id, "cwd": params["cwd"], "mcpServers": params["mcpServers"] }))
                    }
                    None => self.request("session/new", params),
                };
                self.outbox.push(next);
                vec![]
            }
            "session/new" => {
                self.session_id = r["sessionId"].as_str().unwrap_or("").into();
                let mut out = vec![ChatEvent::SessionId(self.session_id.clone())];
                out.extend(self.pickers(r));
                out
            }
            "session/set_config_option" => self.pickers(r),
            "session/load" => {
                self.session_id = self.resume.clone().unwrap_or_default();
                // The cost of the history a load replays isn't the next turn's.
                self.cost_before_turn = self.cost.unwrap_or(0.0);
                let turns = self.history.take().unwrap_or_default();
                let history = (!turns.is_empty()).then_some(ChatEvent::History(turns));
                let pickers = self.pickers(r);
                history.into_iter().chain(pickers).chain([ChatEvent::SessionId(self.session_id.clone())]).collect()
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
                u["used"].as_u64().map(|used| ChatEvent::Context { used, window: u["size"].as_u64() }).into_iter().collect()
            }
            "config_option_update" => self.pickers(u),
            "current_mode_update" => vec![ChatEvent::Mode(s(&u["currentModeId"]))],
            _ => vec![],
        }
    }

    /// A history update session/load replays: user chunks start a turn, the rest joins it (commands still go live).
    fn replayed(&mut self, u: &Value) -> Vec<ChatEvent> {
        if u["sessionUpdate"] == "user_message_chunk" {
            let turns = self.history.get_or_insert_default();
            // shortcut: image blocks of a replayed prompt are not shown in history.
            let chunk = text(&u["content"]).unwrap_or_default();
            match turns.last_mut() {
                Some(t) if t.events.is_empty() => t.prompt.push_str(&chunk),
                _ => turns.push(Turn { prompt: chunk, ..Turn::default() }),
            }
            return vec![];
        }
        let (live, past): (Vec<_>, Vec<_>) = self.update(u).into_iter().partition(|e| matches!(e, ChatEvent::Commands(_)));
        super::add_to_turn(self.history.get_or_insert_default(), past);
        live
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

/// `path` if it lies inside `root`, symlinks resolved; a file being created needs only an existing ancestor.
fn inside(root: &Path, path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let refused = || Err(format!("{} is outside the session folder", path.display()));
    if !path.is_absolute() || path.components().any(|c| c == std::path::Component::ParentDir) {
        return refused();
    }
    let (Ok(root), Some(existing)) = (root.canonicalize(), path.ancestors().find(|a| a.exists())) else { return refused() };
    let mut real = existing.canonicalize().map_err(|e| e.to_string())?;
    if existing != path {
        real.push(path.strip_prefix(existing).unwrap_or(path));
    }
    if real.starts_with(&root) { Ok(real) } else { refused() }
}

fn text(block: &Value) -> Option<String> {
    (block["type"] == "text").then(|| block["text"].as_str()).flatten().filter(|t| !t.is_empty()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_and_mode_pickers_from_config_options() {
        let mut p = Parser::default();
        let r = json!({ "configOptions": [
            { "id": "mode", "category": "mode", "currentValue": "plan", "options": [{ "value": "default", "name": "Default" }, { "value": "plan", "name": "Plan" }] },
            { "id": "model", "category": "model", "currentValue": "a/x", "options": [{ "value": "a/x", "name": "X" }] }
        ] });
        let events = p.pickers(&r);
        assert!(events.contains(&ChatEvent::Mode("plan".into())) && events.contains(&ChatEvent::Model("a/x".into())));
        assert_eq!((p.mode_config.as_deref(), p.model_config.as_deref()), (Some("mode"), Some("model")));
        assert_eq!(p.set_config("model", "a/y")["params"], json!({ "sessionId": "", "configId": "model", "value": "a/y" }));
        let old = Parser::default().pickers(&json!({ "modes": { "availableModes": [{ "id": "default", "name": "Default" }], "currentModeId": "default" } }));
        assert!(old.contains(&ChatEvent::Mode("default".into())));
    }

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
    fn history_from_session_load() {
        let mut p = Parser { resume: Some("ses_e2f769eb9ffek3R3046uVjw3tH".into()), history: Some(vec![]), ..Parser::default() };
        let commands = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"init","description":"x"}]}}}"#;
        assert!(matches!(&p.line(commands)[..], [ChatEvent::Commands(_)]), "commands stay live during a load");
        let mut events = vec![];
        for line in std::fs::read_to_string(format!("{FIXTURES}opencode-load.jsonl")).unwrap().lines() {
            match line.strip_prefix(">> ") {
                Some(sent) => _ = p.pending.insert(2, serde_json::from_str::<Value>(sent).unwrap()["method"].as_str().unwrap().into()),
                None => events.extend(p.line(line)),
            }
        }
        let [ChatEvent::History(turns), ChatEvent::SessionId(id)] = &events[..] else { panic!("{events:?}") };
        assert_eq!(id, "ses_e2f769eb9ffek3R3046uVjw3tH");
        assert_eq!(turns.len(), 1);
        assert!(turns[0].prompt.starts_with("Read a.txt, then replace hello with bye"), "{}", turns[0].prompt);
        assert_eq!(started(&turns[0].events), [("Read", "/tmp/afx/a.txt"), ("Edit", "/tmp/afx/a.txt"), ("Bash", "ls")]);
        assert_eq!(diff(&turns[0].events).map(|d| d.new_text), Some("bye".into()));
        assert!(joined(&turns[0].events, true).contains("need to read a.txt"));
        assert_eq!(joined(&turns[0].events, false), "done");
        assert!(p.history.is_none());
        let chunk = r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","messageId":"m","content":{"type":"text","text":"hi"}}}}"#;
        assert_eq!(p.line(chunk), [ChatEvent::TextDelta { message_id: "m".into(), text: "hi".into() }]);
    }

    #[test]
    fn unknown_agent_requests_are_refused_not_left_hanging() {
        let mut p = Parser::default();
        assert!(p.line(r#"{"jsonrpc":"2.0","id":0,"method":"terminal/create","params":{}}"#).is_empty());
        assert_eq!(p.outbox[0]["id"], 0);
        assert!(p.outbox[0]["error"]["message"].is_string());
    }

    #[test]
    fn permission_round_trip() {
        let line = std::fs::read_to_string(format!("{FIXTURES}omp-edit.jsonl")).unwrap().lines().find(|l| l.contains("session/request_permission")).unwrap().to_string();
        let mut p = Parser::default();
        let [ChatEvent::PermissionAsked { id, title, detail, options }] = &p.line(&line)[..] else { panic!() };
        assert_eq!((title.as_str(), detail.as_str()), ("ls", "$ ls"));
        assert_eq!(options[1], PermissionOption { id: "allow_always".into(), label: "Always allow".into(), kind: "allow_always".into(), description: String::new() });
        assert!(p.outbox.is_empty());
        let reply = p.answer(id, "allow_once").unwrap();
        assert_eq!(reply.to_string(), r#"{"jsonrpc":"2.0","id":2,"result":{"outcome":{"outcome":"selected","optionId":"allow_once"}}}"#);
        assert_eq!(p.answer(id, "allow_once"), None);
    }

    #[test]
    fn fs_is_served_only_inside_the_session_folder() {
        let root = std::env::temp_dir().join(format!("orbit-acp-fs-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut p = Parser { cwd: root.clone(), ..Parser::default() };
        let mut ask = |method: &str, params: Value| {
            p.line(&json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }).to_string());
            p.outbox.pop().unwrap()
        };
        let a = root.join("sub/a.txt");
        assert_eq!(ask("fs/read_text_file", json!({ "path": a }))["result"]["content"], "one\ntwo\nthree\n");
        assert_eq!(ask("fs/read_text_file", json!({ "path": a, "line": 2, "limit": 1 }))["result"]["content"], "two\n");
        assert!(ask("fs/write_text_file", json!({ "path": root.join("b.txt"), "content": "hi" }))["result"].is_null());
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "hi");
        for outside in [json!("/etc/hostname"), json!(root.join("sub/../../x")), json!("sub/a.txt")] {
            let reply = ask("fs/read_text_file", json!({ "path": outside }));
            assert_eq!((reply["id"].clone(), reply["result"].is_null()), (json!(7), true));
            assert!(reply["error"]["message"].as_str().unwrap().contains("outside"), "{reply}");
        }
        assert!(ask("fs/write_text_file", json!({ "path": "/tmp/orbit-acp-escape.txt", "content": "x" }))["error"].is_object());
        std::fs::remove_dir_all(root).unwrap();
    }
}
