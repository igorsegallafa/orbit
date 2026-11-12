//! Chat sessions: Orbit drives an agent through its machine protocol and gets
//! back one event model, so the chat UI never branches on which agent runs.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod claude;
mod codex;

#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    /// The agent's own session id, used to resume the conversation.
    SessionId(String),
    TextDelta {
        message_id: String,
        text: String,
    },
    ThinkingDelta {
        message_id: String,
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        summary: String,
    },
    ToolOutput {
        id: String,
        text: String,
    },
    ToolFinished {
        id: String,
        ok: bool,
        diff: Option<FileDiff>,
    },
    PermissionAsked {
        id: String,
        title: String,
        detail: String,
        options: Vec<PermissionOption>,
    },
    Commands(Vec<SlashCommand>),
    TurnEnded {
        cost_usd: Option<f64>,
        tokens: Option<u64>,
        error: Option<String>,
    },
    Image {
        path: PathBuf,
        title: String,
    },
    Exited {
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatInput {
    Prompt { text: String, images: Vec<PathBuf> },
    Interrupt,
    AnswerPermission { id: String, option_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    /// The agent's kind for the option, e.g. `allow_once` or `reject_once`.
    pub kind: String,
}

pub trait ChatBackend: Send {
    fn send(&self, input: ChatInput);
    fn stop(&self);
}

/// Events reach `sink` on the backend's own thread.
pub fn start(
    agent: &str,
    cwd: &Path,
    resume: Option<String>,
    sink: impl Fn(ChatEvent) + Send + 'static,
) -> Result<Box<dyn ChatBackend>, String> {
    match agent {
        "claude" => claude::start(cwd, resume, sink),
        "codex" => codex::start(cwd, resume, sink),
        _ => Err(format!("chat mode is not available for {agent} yet")),
    }
}

fn session_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("chat:{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

struct Agent {
    stdin: Mutex<ChildStdin>,
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

impl Agent {
    /// Spawns `cmd` in its own process group, so stop() takes its shells and builds down with it.
    /// Each stdout line goes to `on_line`; at EOF `on_exit` gets the error (stderr's tail), None after stop().
    fn spawn(
        mut cmd: Command,
        name: &str,
        mut on_line: impl FnMut(&str) + Send + 'static,
        on_exit: impl FnOnce(Option<String>) + Send + 'static,
    ) -> Result<Agent, String> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn().map_err(|e| format!("could not start {name}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
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
        let (child_r, stopped_r, name) = (child.clone(), stopped.clone(), name.to_string());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                on_line(&line);
            }
            let status = child_r.lock().unwrap().wait();
            on_exit(match status {
                _ if stopped_r.load(Ordering::Relaxed) => None,
                Ok(s) if s.success() => None,
                Ok(s) => Some(Some(tail.lock().unwrap().join("\n")).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("{name} exited with {s}"))),
                Err(e) => Some(e.to_string()),
            });
        });
        Ok(Agent { stdin: Mutex::new(stdin), child, stopped })
    }

    /// A failed write means the process is gone; the reader thread reports Exited.
    fn write(&self, line: &serde_json::Value) {
        use std::io::Write;
        let _ = writeln!(self.stdin.lock().unwrap(), "{line}");
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        crate::agent::runner::kill_tree(&mut self.child.lock().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Stub(Box<dyn Fn(ChatEvent) + Send>);

    impl ChatBackend for Stub {
        fn send(&self, input: ChatInput) {
            if !matches!(input, ChatInput::Prompt { .. }) {
                return;
            }
            for event in every_event() {
                (self.0)(event);
            }
        }
        fn stop(&self) {
            (self.0)(ChatEvent::Exited { error: None });
        }
    }

    fn every_event() -> Vec<ChatEvent> {
        let s = |v: &str| v.to_string();
        vec![
            ChatEvent::SessionId(s("abc")),
            ChatEvent::TextDelta {
                message_id: s("m1"),
                text: s("hi"),
            },
            ChatEvent::ThinkingDelta {
                message_id: s("m1"),
                text: s("hmm"),
            },
            ChatEvent::ToolStarted {
                id: s("t1"),
                name: s("Edit"),
                summary: s("a.rs"),
            },
            ChatEvent::ToolOutput {
                id: s("t1"),
                text: s("ok"),
            },
            ChatEvent::ToolFinished {
                id: s("t1"),
                ok: true,
                diff: Some(FileDiff {
                    path: "a.rs".into(),
                    old_text: s("a"),
                    new_text: s("b"),
                }),
            },
            ChatEvent::PermissionAsked {
                id: s("p1"),
                title: s("Run ls"),
                detail: s("ls -la"),
                options: vec![PermissionOption {
                    id: s("allow"),
                    label: s("Allow"),
                    kind: s("allow_once"),
                }],
            },
            ChatEvent::Commands(vec![SlashCommand {
                name: s("review"),
                description: s("Review"),
            }]),
            ChatEvent::TurnEnded {
                cost_usd: Some(0.01),
                tokens: Some(42),
                error: None,
            },
            ChatEvent::Image {
                path: "shot.png".into(),
                title: s("shot"),
            },
        ]
    }

    #[test]
    fn stub_backend_emits_every_event_in_order() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let backend: Box<dyn ChatBackend> =
            Box::new(Stub(Box::new(move |e| sink.lock().unwrap().push(e))));
        backend.send(ChatInput::Interrupt);
        backend.send(ChatInput::Prompt {
            text: "go".into(),
            images: vec![],
        });
        backend.stop();
        let mut want = every_event();
        want.push(ChatEvent::Exited { error: None });
        assert_eq!(*got.lock().unwrap(), want);
    }

    #[test]
    fn unknown_agent_is_an_error() {
        assert!(start("nope", Path::new("."), None, |_| {}).is_err());
    }
}
