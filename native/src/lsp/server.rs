//! The JSON-RPC client for one language server. orbit-core runs the process
//! (`orbit_core::lsp::start`: server choice, clangd's compilation database,
//! the real folder of a symlinked repo); this side keeps the requests.
//!
//! Repos shared into a workspace are symlinks. Servers resolve links and
//! answer with real paths, and compilation databases list real paths, so the
//! server runs in the real folder and every `file:` URI is mapped between the
//! workspace root (what the UI opens) and the real root on the way in and out
//! (the same fix as `src/lib/lsp/paths.ts`).
use super::paths::{map_uris, path_to_uri};
use anyhow::{anyhow, Result};
use futures::channel::{mpsc, oneshot};
use orbit_core::lsp;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// A server-to-client notification, URIs already mapped to the workspace root.
pub struct Notification {
    pub method: String,
    pub params: Value,
}

type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>;

/// What the reader thread shares with the client.
struct Shared {
    open_root: PathBuf,
    real_root: PathBuf,
    pending: Pending,
    exited: AtomicBool,
    notifications: mpsc::UnboundedSender<Notification>,
}

pub struct LspServer {
    /// orbit-core's id for the process.
    pub id: u32,
    pub language: &'static str,
    pub command: String,
    /// The repo folder as the UI opens it (may be a symlink).
    pub open_root: PathBuf,
    /// `open_root` with links resolved; the server's working directory.
    pub real_root: PathBuf,
    next_id: AtomicI64,
    shared: Arc<Shared>,
}

impl Drop for LspServer {
    fn drop(&mut self) {
        lsp::stop(self.id);
    }
}

impl LspServer {
    /// Starts the server for `language` in `repo` of `workspace`.
    pub fn spawn(workspace: &str, repo: &str, language: &'static str, notifications: mpsc::UnboundedSender<Notification>) -> Result<Arc<Self>> {
        let open_root = orbit_core::workspace::repo_path(workspace, repo).map_err(|e| anyhow!(e))?;
        let real_root = lsp::real_path(&open_root);
        let shared = Arc::new(Shared {
            open_root: open_root.clone(),
            real_root: real_root.clone(),
            pending: Mutex::new(HashMap::new()),
            exited: AtomicBool::new(false),
            notifications,
        });
        let (reader, on_exit) = (shared.clone(), shared.clone());
        let info = lsp::start(
            workspace,
            repo,
            language,
            move |id, raw| on_message(&reader, id, raw),
            move |_, _, _| {
                on_exit.exited.store(true, Ordering::Release);
                // Whoever still waits gets an error instead of hanging.
                for (_, tx) in on_exit.pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("language server exited".into()));
                }
            },
        )
        .map_err(|e| anyhow!(e))?;
        Ok(Arc::new(Self {
            id: info.id,
            language,
            command: info.command,
            open_root,
            real_root,
            next_id: AtomicI64::new(1),
            shared,
        }))
    }

    pub fn has_exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    pub fn log_tail(&self, n: usize) -> Vec<String> {
        let log = lsp::log(self.id);
        log[log.len().saturating_sub(n)..].to_vec()
    }

    fn write(&self, message: &Value) -> Result<()> {
        lsp::send(self.id, &message.to_string()).map_err(|e| anyhow!(e))
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
        self.shared.pending.lock().unwrap().insert(id, tx);
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

    /// LSP shutdown and exit; killed if still running two seconds later.
    pub fn shutdown(&self) {
        lsp::stop(self.id);
    }
}

fn on_message(shared: &Shared, server_id: u32, raw: String) {
    let Ok(msg) = serde_json::from_str::<Value>(&raw) else { return };
    let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
    match (msg.get("id").cloned(), method) {
        // A response to one of our requests.
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(tx) = shared.pending.lock().unwrap().remove(&id) {
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
            let _ = lsp::send(server_id, &json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string());
        }
        (None, Some(method)) => {
            let params = map_uris(msg.get("params").cloned().unwrap_or(Value::Null), &shared.real_root, &shared.open_root);
            let _ = shared.notifications.unbounded_send(Notification { method, params });
        }
        (None, None) => {}
    }
}
