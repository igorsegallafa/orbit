//! One debug adapter session (`src/lib/dap/client.ts`): DAP requests,
//! responses and events over `orbit_core::dap`'s stdio bridge. Responses are
//! matched on the reader thread; events go down a channel to the UI.
use futures::channel::{mpsc, oneshot};
use futures::FutureExt;
use orbit_core::dap::{self, Adapter};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub enum DapEvent {
    Event { event: String, body: Value },
    /// The adapter process is gone.
    Exit,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct DapClient {
    pub id: u32,
    pub adapter: Adapter,
    seq: Arc<AtomicI64>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    initialized: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

fn fail_all(pending: &Pending, why: &str) {
    for (_, tx) in pending.lock().unwrap().drain() {
        let _ = tx.send(Err(why.to_string()));
    }
}

impl DapClient {
    /// Starts the adapter (`requested`: "auto", "gdb", "lldb-dap" or a path) in `cwd`.
    pub fn start(requested: Option<&str>, cwd: &str) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<DapEvent>), String> {
        let adapter = dap::adapter(requested)?;
        let pending: Pending = Arc::default();
        let seq = Arc::new(AtomicI64::new(1));
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::unbounded();
        let (init_tx, init_rx) = oneshot::channel();
        let on_message = {
            let (pending, seq, tx) = (pending.clone(), seq.clone(), tx.clone());
            let mut init_tx = Some(init_tx);
            move |id: u32, raw: String| {
                let Ok(msg) = serde_json::from_str::<Value>(&raw) else { return };
                match msg["type"].as_str() {
                    Some("response") => {
                        let Some(seq) = msg["request_seq"].as_i64() else { return };
                        let Some(waiter) = pending.lock().unwrap().remove(&seq) else { return };
                        let result = if msg["success"].as_bool() == Some(true) {
                            Ok(msg.get("body").cloned().unwrap_or(json!({})))
                        } else {
                            let why = msg["body"]["error"]["format"]
                                .as_str()
                                .or(msg["message"].as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("{} failed", msg["command"].as_str().unwrap_or("request")));
                            Err(why)
                        };
                        let _ = waiter.send(result);
                    }
                    Some("event") => {
                        let event = msg["event"].as_str().unwrap_or_default().to_string();
                        if event == "initialized" {
                            if let Some(t) = init_tx.take() {
                                let _ = t.send(());
                            }
                        }
                        let body = msg.get("body").cloned().unwrap_or(json!({}));
                        let _ = tx.unbounded_send(DapEvent::Event { event, body });
                    }
                    Some("request") => {
                        // Reverse requests (runInTerminal, startDebugging): not supported.
                        let reply = json!({
                            "seq": seq.fetch_add(1, Ordering::SeqCst), "type": "response", "request_seq": msg["seq"],
                            "command": msg["command"], "success": false, "message": "unsupported",
                        });
                        let _ = dap::send(id, &reply.to_string());
                    }
                    _ => {}
                }
            }
        };
        let on_exit = {
            let (pending, alive) = (pending.clone(), alive.clone());
            move |_: u32, _: Option<i32>| {
                alive.store(false, Ordering::SeqCst);
                fail_all(&pending, "the debugger exited");
                let _ = tx.unbounded_send(DapEvent::Exit);
            }
        };
        let id = dap::start(&adapter, cwd, on_message, on_exit)?;
        let client = Self { id, adapter, seq, pending, alive, initialized: Arc::new(Mutex::new(Some(init_rx))) };
        Ok((Arc::new(client), rx))
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Sends the request now; the future resolves with the response body.
    pub fn request(&self, command: &str, args: Value) -> impl Future<Output = Result<Value, String>> + 'static {
        let (tx, rx) = oneshot::channel();
        if !self.alive() {
            let _ = tx.send(Err("the debugger is not running".to_string()));
        } else {
            let seq = self.seq.fetch_add(1, Ordering::SeqCst);
            self.pending.lock().unwrap().insert(seq, tx);
            let message = json!({ "seq": seq, "type": "request", "command": command, "arguments": args });
            if let Err(e) = dap::send(self.id, &message.to_string()) {
                if let Some(tx) = self.pending.lock().unwrap().remove(&seq) {
                    let _ = tx.send(Err(e));
                }
            }
        }
        rx.map(|r| r.unwrap_or_else(|_| Err("the debugger exited".to_string())))
    }

    /// Resolves once the adapter sent `initialized` (an error if it exits first).
    pub fn initialized(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let rx = self.initialized.lock().unwrap().take();
        async move {
            match rx {
                Some(rx) => rx.await.map_err(|_| "the debugger exited before it was ready".to_string()),
                None => Ok(()),
            }
        }
    }

    /// Kills the adapter (and the program it debugs).
    pub fn kill(&self) {
        dap::stop(self.id);
    }
}
