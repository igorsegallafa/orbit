// Codex over `codex app-server`: JSON-RPC on stdio, one thread per chat session.
use super::{Agent, ChatBackend, ChatEvent, ChatInput, FileDiff, PermissionOption, SlashCommand};
use crate::agent::stream;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Codex {
    agent: Arc<Agent>,
    parser: Arc<Mutex<Parser>>,
}

pub fn start(cwd: &Path, resume: Option<String>, key: &str, sink: impl Fn(ChatEvent) + Send + 'static) -> Result<Box<dyn ChatBackend>, String> {
    let (launch_args, launch_env) = crate::agent_hooks::session_launch("codex", key);
    let mut cmd = crate::proc::cmd("codex");
    cmd.arg("app-server").args(launch_args).envs(launch_env).current_dir(cwd);
    let parser = Arc::new(Mutex::new(Parser { cwd: cwd.to_path_buf(), resume, ..Parser::default() }));
    let agent = super::spawn_rpc(cmd, "codex", parser.clone(), sink)?;
    let init = parser.lock().unwrap().request("initialize", json!({ "clientInfo": { "name": "orbit", "title": "Orbit", "version": env!("CARGO_PKG_VERSION") } }));
    agent.write(&init);
    Ok(Box::new(Codex { agent, parser }))
}

impl ChatBackend for Codex {
    fn send(&self, input: ChatInput) {
        let line = {
            let mut p = self.parser.lock().unwrap();
            match input {
                ChatInput::Prompt { text, images } => {
                    let mut input = vec![json!({ "type": "text", "text": text })];
                    input.extend(images.iter().map(|path| json!({ "type": "localImage", "path": path })));
                    let thread = p.thread_id.clone();
                    p.request("turn/start", json!({ "threadId": thread, "input": input }))
                }
                ChatInput::Interrupt if !p.turn_id.is_empty() => {
                    let params = json!({ "threadId": p.thread_id, "turnId": p.turn_id });
                    p.request("turn/interrupt", params)
                }
                ChatInput::AnswerPermission { id, option_id } => match p.answer(&id, &option_id) {
                    Some(reply) => reply,
                    None => return,
                },
                _ => return,
            }
        };
        self.agent.write(&line);
    }

    fn stop(&self) {
        self.agent.stop();
    }
}

#[derive(Default)]
struct Parser {
    cwd: PathBuf,
    resume: Option<String>,
    next_id: u64,
    /// Our requests waiting for a response: id → method.
    pending: HashMap<u64, String>,
    /// Lines the reader must write back.
    outbox: Vec<Value>,
    thread_id: String,
    turn_id: String,
    /// Command items whose output already streamed, so their aggregated output isn't repeated.
    streamed: HashSet<String>,
    turn_tokens: u64,
    /// Approval requests waiting for the user: PermissionAsked id → the server's request.
    approvals: HashMap<String, Value>,
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

    fn line(&mut self, line: &str) -> Vec<ChatEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        let method = v["method"].as_str().unwrap_or("");
        if !v["id"].is_null() && method.is_empty() {
            let request = v["id"].as_u64().and_then(|id| self.pending.remove(&id)).unwrap_or_default();
            return self.response(&request, &v);
        }
        if !v["id"].is_null() {
            if let Some(asked) = self.approval(&v) {
                return vec![asked];
            }
            // shortcut: other server requests (user input, elicitation) are refused until Orbit answers them.
            self.outbox.push(json!({ "jsonrpc": "2.0", "id": v["id"], "error": { "code": -32601, "message": format!("Orbit does not handle {method}") } }));
            return vec![];
        }
        let p = &v["params"];
        match method {
            "turn/started" => {
                self.turn_id = s(&p["turn"]["id"]);
                vec![]
            }
            "item/agentMessage/delta" => vec![ChatEvent::TextDelta { message_id: s(&p["itemId"]), text: s(&p["delta"]) }],
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => vec![ChatEvent::ThinkingDelta { message_id: s(&p["itemId"]), text: s(&p["delta"]) }],
            "item/commandExecution/outputDelta" => {
                self.streamed.insert(s(&p["itemId"]));
                vec![ChatEvent::ToolOutput { id: s(&p["itemId"]), text: s(&p["delta"]) }]
            }
            "item/started" => tool_started(&p["item"]).into_iter().collect(),
            "item/completed" => self.tool_finished(&p["item"]),
            "thread/tokenUsage/updated" => {
                let last = &p["tokenUsage"]["last"];
                self.turn_tokens += last["inputTokens"].as_u64().unwrap_or(0) + last["outputTokens"].as_u64().unwrap_or(0);
                vec![]
            }
            "turn/completed" => {
                let turn = &p["turn"];
                let error = match turn["error"]["message"].as_str() {
                    Some(m) => Some(m.to_string()),
                    None => (turn["status"] != "completed").then(|| s(&turn["status"])),
                };
                self.turn_id.clear();
                let tokens = std::mem::take(&mut self.turn_tokens);
                vec![ChatEvent::TurnEnded { cost_usd: None, tokens: (tokens > 0).then_some(tokens), error }]
            }
            _ => vec![],
        }
    }

    fn response(&mut self, request: &str, v: &Value) -> Vec<ChatEvent> {
        if let Some(message) = v["error"]["message"].as_str() {
            return match request {
                "thread/start" | "thread/resume" | "turn/start" => vec![ChatEvent::TurnEnded { cost_usd: None, tokens: None, error: Some(message.into()) }],
                _ => vec![],
            };
        }
        let r = &v["result"];
        match request {
            "initialize" => {
                self.outbox.push(json!({ "jsonrpc": "2.0", "method": "initialized" }));
                let cwd = self.cwd.clone();
                let next = match self.resume.clone() {
                    Some(id) => self.request("thread/resume", json!({ "threadId": id, "cwd": cwd })),
                    None => self.request("thread/start", json!({ "cwd": cwd })),
                };
                self.outbox.push(next);
                vec![]
            }
            "thread/start" | "thread/resume" => {
                self.thread_id = r["thread"]["id"].as_str().unwrap_or("").into();
                let skills = self.request("skills/list", json!({ "cwds": [self.cwd] }));
                self.outbox.push(skills);
                vec![ChatEvent::SessionId(self.thread_id.clone())]
            }
            "skills/list" => {
                let skills = r["data"].as_array().into_iter().flatten().flat_map(|e| e["skills"].as_array().into_iter().flatten());
                let commands = skills
                    .filter(|k| k["enabled"].as_bool() != Some(false))
                    .map(|k| SlashCommand { name: k["name"].as_str().unwrap_or("").into(), description: k["description"].as_str().unwrap_or("").into() })
                    .collect();
                vec![ChatEvent::Commands(commands)]
            }
            "turn/start" => {
                self.turn_id = r["turn"]["id"].as_str().unwrap_or("").into();
                vec![]
            }
            _ => vec![],
        }
    }

    fn approval(&mut self, v: &Value) -> Option<ChatEvent> {
        let p = &v["params"];
        let reason = p["reason"].as_str().unwrap_or("");
        let (title, detail) = match v["method"].as_str()? {
            "item/commandExecution/requestApproval" => ("Run command", format!("{}\n{reason}", stream::unwrap_shell(p["command"].as_str().unwrap_or("")))),
            "item/fileChange/requestApproval" => ("Change files", reason.to_string()),
            "item/permissions/requestApproval" => ("Grant permissions", format!("{reason}\n{}", p["permissions"])),
            _ => return None,
        };
        // A command approval may list `cancel` but not `decline`, so offer only what the server lists.
        let all = [("accept", "Allow", "allow_once"), ("acceptForSession", "Allow for this session", "allow_always"), ("decline", "Deny", "reject_once"), ("cancel", "Deny and stop", "reject_once")];
        let offered = |d: &str| p["availableDecisions"].as_array().map_or(d != "cancel", |a| a.iter().any(|x| x == d));
        let options = all.into_iter().filter(|(id, ..)| offered(id)).map(|(id, label, kind)| PermissionOption { id: id.into(), label: label.into(), kind: kind.into() }).collect();
        let id = v["id"].to_string();
        self.approvals.insert(id.clone(), v.clone());
        Some(ChatEvent::PermissionAsked { id, title: title.into(), detail: detail.trim().into(), options })
    }

    fn answer(&mut self, id: &str, option_id: &str) -> Option<Value> {
        let request = self.approvals.remove(id)?;
        let result = if request["method"] == "item/permissions/requestApproval" {
            // Permissions are granted by echoing the requested profile; an empty profile denies.
            let granted = if option_id.starts_with("accept") { request["params"]["permissions"].clone() } else { json!({}) };
            json!({ "permissions": granted, "scope": if option_id == "acceptForSession" { "session" } else { "turn" } })
        } else {
            json!({ "decision": option_id })
        };
        Some(json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }))
    }

    fn tool_finished(&mut self, item: &Value) -> Vec<ChatEvent> {
        let id = item["id"].as_str().unwrap_or("").to_string();
        let ok = item["status"] == "completed";
        let (output, diff) = match item["type"].as_str().unwrap_or("") {
            "commandExecution" => {
                let streamed = self.streamed.remove(&id);
                let ok = ok && item["exitCode"].as_i64().unwrap_or(0) == 0;
                let out = item["aggregatedOutput"].as_str().filter(|_| !streamed).unwrap_or("");
                return output_then_finished(id, out.into(), ok, None);
            }
            "mcpToolCall" => {
                let text = match item["error"]["message"].as_str() {
                    Some(e) => e.to_string(),
                    None => item["result"]["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
                };
                (text, None)
            }
            // shortcut: a patch over several files shows only the first file's diff; split into one row per file if that matters.
            "fileChange" => (String::new(), item["changes"].get(0).and_then(file_diff)),
            _ => return vec![],
        };
        output_then_finished(id, output, ok, diff.filter(|_| ok))
    }
}

fn output_then_finished(id: String, text: String, ok: bool, diff: Option<FileDiff>) -> Vec<ChatEvent> {
    let output = (!text.is_empty()).then(|| ChatEvent::ToolOutput { id: id.clone(), text });
    output.into_iter().chain([ChatEvent::ToolFinished { id, ok, diff }]).collect()
}

fn tool_started(item: &Value) -> Option<ChatEvent> {
    let id = item["id"].as_str().unwrap_or("").to_string();
    let (name, summary) = match item["type"].as_str()? {
        "commandExecution" => {
            let command = stream::unwrap_shell(item["command"].as_str().unwrap_or(""));
            ("Bash".to_string(), stream::summarize("Bash", &json!({ "command": command })))
        }
        "mcpToolCall" => {
            let tool = item["tool"].as_str().unwrap_or("").to_string();
            let summary = stream::summarize(&tool, &item["arguments"]);
            (tool, summary)
        }
        "fileChange" => {
            let paths: Vec<&str> = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
            ("Edit".to_string(), paths.join(", "))
        }
        _ => return None,
    };
    Some(ChatEvent::ToolStarted { id, name, summary })
}

/// The old and new text of a file change's unified diff hunks (context lines in both).
fn file_diff(change: &Value) -> Option<FileDiff> {
    let diff = change["diff"].as_str()?;
    let (mut old_text, mut new_text) = (vec![], vec![]);
    if change["kind"]["type"] == "add" && !diff.starts_with("@@") {
        new_text = diff.lines().collect();
    } else {
        for line in diff.lines() {
            match line.split_at_checked(1) {
                Some(("-", rest)) if !line.starts_with("---") => old_text.push(rest),
                Some(("+", rest)) if !line.starts_with("+++") => new_text.push(rest),
                Some((" ", rest)) => {
                    old_text.push(rest);
                    new_text.push(rest);
                }
                _ => {}
            }
        }
    }
    Some(FileDiff { path: change["path"].as_str()?.into(), old_text: old_text.join("\n"), new_text: new_text.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/chat/codex/");

    /// Replays a recording; `>> ` lines are the requests the recorder sent, registered as pending.
    fn replay(name: &str) -> (Parser, Vec<ChatEvent>) {
        let mut p = Parser { cwd: "/tmp/cfx".into(), ..Parser::default() };
        let mut events = vec![];
        for line in std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap().lines() {
            match line.strip_prefix(">> ") {
                Some(req) => {
                    let v: Value = serde_json::from_str(req).unwrap();
                    p.pending.insert(v["id"].as_u64().unwrap(), v["method"].as_str().unwrap().into());
                }
                None => events.extend(p.line(line)),
            }
        }
        (p, events)
    }

    #[test]
    fn edit_turn_fixture() {
        let (p, events) = replay("edit.jsonl");
        assert!(matches!(&events[0], ChatEvent::SessionId(id) if id == &p.thread_id && !id.is_empty()));
        assert!(matches!(&events[1], ChatEvent::Commands(c) if c.len() == 2 && c[0].name == "imagegen" && !c[0].description.is_empty()));
        let started: Vec<_> = events.iter().filter_map(|e| if let ChatEvent::ToolStarted { name, summary, .. } = e { Some((name.as_str(), summary.as_str())) } else { None }).collect();
        assert_eq!(started, [("Bash", "cat a.txt"), ("Edit", "/tmp/cfx/a.txt, /tmp/cfx/b.txt")]);
        assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolOutput { text, .. } if text == "hello\n")));
        let diff = events.iter().find_map(|e| if let ChatEvent::ToolFinished { ok: true, diff: Some(d), .. } = e { Some(d.clone()) } else { None });
        assert_eq!(diff, Some(FileDiff { path: "/tmp/cfx/a.txt".into(), old_text: "hello".into(), new_text: "bye".into() }));
        let text: String = events.iter().filter_map(|e| if let ChatEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None }).collect();
        assert!(text.ends_with("done"), "{text}");
        assert!(matches!(events.last(), Some(ChatEvent::TurnEnded { cost_usd: None, tokens: Some(t), error: None }) if *t > 0));
        assert!(p.turn_id.is_empty());
    }

    #[test]
    fn interrupted_and_failed_turns_carry_an_error() {
        let (_, events) = replay("interrupt.jsonl");
        assert!(matches!(events.last(), Some(ChatEvent::TurnEnded { error: Some(e), .. }) if e == "interrupted"));
        let (_, events) = replay("model-error.jsonl");
        assert!(matches!(events.last(), Some(ChatEvent::TurnEnded { error: Some(e), .. }) if e.contains("not supported")));
    }

    #[test]
    fn handshake_starts_or_resumes_the_thread() {
        let mut p = Parser { cwd: "/w".into(), resume: Some("t1".into()), ..Parser::default() };
        let init = p.request("initialize", json!({}));
        assert!(p.line(&json!({ "id": init["id"], "result": {} }).to_string()).is_empty());
        assert_eq!(p.outbox[0], json!({ "jsonrpc": "2.0", "method": "initialized" }));
        assert_eq!(p.outbox[1]["method"], "thread/resume");
        assert_eq!(p.outbox[1]["params"], json!({ "threadId": "t1", "cwd": "/w" }));
        let id = p.outbox[1]["id"].clone();
        assert_eq!(p.line(&json!({ "id": id, "result": { "thread": { "id": "t1" } } }).to_string()), [ChatEvent::SessionId("t1".into())]);
        assert_eq!(p.outbox[2]["method"], "skills/list");
    }

    #[test]
    fn server_requests_are_refused_not_left_hanging() {
        let mut p = Parser::default();
        assert!(p.line(r#"{"id":7,"method":"item/tool/requestUserInput","params":{}}"#).is_empty());
        assert_eq!(p.outbox[0]["id"], 7);
        assert!(p.outbox[0]["error"]["message"].is_string());
    }

    #[test]
    fn recorded_command_approval_is_asked_and_answered() {
        let (mut p, events) = replay("approval.jsonl");
        let [ChatEvent::PermissionAsked { id, title, detail, options }] = &events[..] else { panic!("{events:?}") };
        assert_eq!(title, "Run command");
        assert!(detail.starts_with("touch b.txt\nMay I create b.txt"), "{detail}");
        let ids: Vec<_> = options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["accept", "cancel"]);
        let reply = p.answer(id, "accept").unwrap();
        assert_eq!(reply.to_string(), r#"{"jsonrpc":"2.0","id":0,"result":{"decision":"accept"}}"#);
        assert!(p.answer(id, "accept").is_none(), "answered twice");
    }

    #[test]
    fn file_and_permission_approvals() {
        let mut p = Parser::default();
        let asked = p.line(r#"{"id":"f","method":"item/fileChange/requestApproval","params":{"itemId":"i","threadId":"t","turnId":"u","startedAtMs":0,"reason":"write a.txt"}}"#);
        let [ChatEvent::PermissionAsked { id, options, .. }] = &asked[..] else { panic!("{asked:?}") };
        assert_eq!(options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["accept", "acceptForSession", "decline"]);
        assert_eq!(p.answer(id, "decline").unwrap().to_string(), r#"{"jsonrpc":"2.0","id":"f","result":{"decision":"decline"}}"#);
        let line = r#"{"id":9,"method":"item/permissions/requestApproval","params":{"itemId":"i","threadId":"t","turnId":"u","startedAtMs":0,"cwd":"/w","permissions":{"network":{"enabled":true}}}}"#;
        p.line(line);
        assert_eq!(p.answer("9", "acceptForSession").unwrap()["result"], json!({ "permissions": { "network": { "enabled": true } }, "scope": "session" }));
        p.line(line);
        assert_eq!(p.answer("9", "decline").unwrap()["result"], json!({ "permissions": {}, "scope": "turn" }));
        assert!(p.outbox.is_empty());
    }

    #[test]
    fn streamed_command_output_is_not_repeated() {
        let mut p = Parser::default();
        p.line(r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"c1","delta":"a"}}"#);
        let done = p.line(r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","status":"completed","exitCode":1,"aggregatedOutput":"a"}}}"#);
        assert_eq!(done, [ChatEvent::ToolFinished { id: "c1".into(), ok: false, diff: None }]);
    }

    #[test]
    fn methods_orbit_uses_exist_in_the_schema() {
        let schema = std::fs::read_to_string(format!("{FIXTURES}codex_app_server_protocol.v2.schemas.json")).unwrap();
        for m in ["initialize", "thread/start", "thread/resume", "skills/list", "turn/start", "turn/interrupt", "turn/started", "turn/completed", "item/started", "item/completed", "item/agentMessage/delta", "item/reasoning/textDelta", "item/reasoning/summaryTextDelta", "item/commandExecution/outputDelta", "thread/tokenUsage/updated", "localImage"] {
            assert!(schema.contains(&format!("\"{m}\"")), "{m} is not in codex's schema");
        }
    }
}
