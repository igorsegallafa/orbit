//! One language server process and the JSON-RPC over its stdio.
//!
//! Repos shared into a workspace are symlinks. Servers resolve links and
//! answer with real paths, and compilation databases list real paths, so the
//! server runs in the real folder and every `file:` URI is mapped between the
//! workspace root (what the UI opens) and the real root on the way in and out
//! (the same fix as `src/lib/lsp/paths.ts`).
use super::paths::{map_uris, path_to_uri};
use anyhow::{anyhow, Context as _, Result};
use futures::channel::{mpsc, oneshot};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

const LOG_LINES: usize = 200;

/// A server-to-client notification, URIs already mapped to the workspace root.
pub struct Notification {
    pub method: String,
    pub params: Value,
}

type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>;

pub struct LspServer {
    pub language: &'static str,
    pub command: String,
    /// The repo folder as the UI opens it (may be a symlink).
    pub open_root: PathBuf,
    /// `open_root` with links resolved; the server's working directory.
    pub real_root: PathBuf,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    pending: Pending,
    log: Arc<Mutex<VecDeque<String>>>,
    exited: Arc<AtomicBool>,
}

impl Drop for LspServer {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

impl LspServer {
    pub fn spawn(
        language: &'static str,
        bin: &str,
        args: &[String],
        open_root: &Path,
        real_root: &Path,
        notifications: mpsc::UnboundedSender<Notification>,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new(bin)
            .args(args)
            .current_dir(real_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start {bin}"))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let log = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_LINES)));
        let exited = Arc::new(AtomicBool::new(false));
        let server = Arc::new(Self {
            language,
            command: std::iter::once(bin).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" "),
            open_root: open_root.to_path_buf(),
            real_root: real_root.to_path_buf(),
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            log: log.clone(),
            exited: exited.clone(),
        });
        std::thread::Builder::new().name(format!("{language}-lsp-stderr")).spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut l = log.lock().unwrap();
                if l.len() == LOG_LINES {
                    l.pop_front();
                }
                l.push_back(line);
            }
        })?;
        let weak = Arc::downgrade(&server);
        std::thread::Builder::new().name(format!("{language}-lsp-stdout")).spawn(move || {
            read_frames(stdout, |raw| on_message(&weak, raw, &notifications));
            exited.store(true, Ordering::Release);
            // Whoever still waits gets an error instead of hanging.
            if let Some(server) = weak.upgrade() {
                for (_, tx) in server.pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("language server exited".into()));
                }
            }
        })?;
        Ok(server)
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub fn log_tail(&self, n: usize) -> Vec<String> {
        let log = self.log.lock().unwrap();
        log.iter().skip(log.len().saturating_sub(n)).cloned().collect()
    }

    fn write(&self, message: &Value) -> Result<()> {
        let body = message.to_string();
        let mut stdin = self.stdin.lock().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        stdin.flush()?;
        Ok(())
    }

    fn to_server(&self, params: Value) -> Value {
        map_uris(params, &self.open_root, &self.real_root)
    }

    pub fn notify(&self, method: &str, params: Value) {
        let params = self.to_server(params);
        let _ = self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Sends a request now; the returned future resolves with its result
    /// (URIs mapped back to the workspace root).
    pub fn request(self: &Arc<Self>, method: &str, params: Value) -> impl std::future::Future<Output = Result<Value>> + Send + 'static {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let params = self.to_server(params);
        let sent = self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let (open_root, real_root) = (self.open_root.clone(), self.real_root.clone());
        let method = method.to_string();
        async move {
            sent?;
            let result = rx.await.map_err(|_| anyhow!("{method}: no response"))?.map_err(|e| anyhow!("{method}: {e}"))?;
            Ok(map_uris(result, &real_root, &open_root))
        }
    }

    /// The `initialize` handshake. Resolves once the server accepts requests.
    pub async fn initialize(self: &Arc<Self>) -> Result<Value> {
        let root = path_to_uri(&self.open_root).map(|u| u.to_string());
        let name = self.open_root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let result = self
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "clientInfo": { "name": "orbit-native" },
                    "rootUri": root,
                    "workspaceFolders": [{ "uri": root, "name": name }],
                    "capabilities": {
                        // The editor counts columns in chars: ask for UTF-32 (clangd supports it).
                        "general": { "positionEncodings": ["utf-32", "utf-16"] },
                        "textDocument": {
                            "synchronization": { "didSave": true, "dynamicRegistration": false },
                            "hover": { "contentFormat": ["markdown", "plaintext"] },
                            "definition": { "linkSupport": true },
                            "publishDiagnostics": { "relatedInformation": true },
                        },
                        "workspace": { "workspaceFolders": true, "configuration": true },
                        "window": { "workDoneProgress": false },
                    },
                }),
            )
            .await?;
        self.notify("initialized", json!({}));
        Ok(result)
    }

    pub fn shutdown(&self) {
        let _ = self.write(&json!({ "jsonrpc": "2.0", "id": 0, "method": "shutdown" }));
        let _ = self.write(&json!({ "jsonrpc": "2.0", "method": "exit" }));
    }
}

fn on_message(server: &Weak<LspServer>, raw: String, notifications: &mpsc::UnboundedSender<Notification>) {
    let Ok(msg) = serde_json::from_str::<Value>(&raw) else { return };
    let Some(server) = server.upgrade() else { return };
    let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
    match (msg.get("id").cloned(), method) {
        // A response to one of our requests.
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(tx) = server.pending.lock().unwrap().remove(&id) {
                let result = match msg.get("error") {
                    Some(err) => Err(err.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
        }
        // A request from the server: answer what's needed to keep it going.
        (Some(id), Some(method)) => {
            let result = match method.as_str() {
                "workspace/configuration" => {
                    let n = msg.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; n])
                }
                _ => Value::Null,
            };
            let _ = server.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
        (None, Some(method)) => {
            let params = map_uris(msg.get("params").cloned().unwrap_or(Value::Null), &server.real_root, &server.open_root);
            let _ = notifications.unbounded_send(Notification { method, params });
        }
        (None, None) => {}
    }
}

/// Reads `Content-Length`-framed messages until the stream closes.
fn read_frames(stdout: impl Read, mut on_message: impl FnMut(String)) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                length = v.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { continue };
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        on_message(String::from_utf8_lossy(&body).into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages() {
        let input = b"Content-Length: 2\r\n\r\n{}Content-Length: 7\r\nContent-Type: x\r\n\r\n{\"a\":1}";
        let mut out = vec![];
        read_frames(&input[..], |m| out.push(m));
        assert_eq!(out, vec!["{}", "{\"a\":1}"]);
    }
}
