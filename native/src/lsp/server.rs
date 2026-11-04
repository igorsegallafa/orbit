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
use std::sync::{Arc, Mutex, OnceLock};

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
    /// `$/progress` texts by token (clangd's indexing).
    progress: Mutex<Vec<(String, String)>>,
    /// `workspace/applyEdit` requests, kept for the editor that ran the
    /// command (clangd's tweaks arrive this way), URIs already mapped.
    applied_edits: Mutex<Vec<Value>>,
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
    /// The server's `ServerCapabilities`, once initialized.
    capabilities: OnceLock<Value>,
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
            progress: Mutex::new(vec![]),
            applied_edits: Mutex::new(vec![]),
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
            capabilities: OnceLock::new(),
        }))
    }

    pub fn has_exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    /// A capability by JSON pointer (`/renameProvider`); None when absent or false.
    pub fn capability(&self, pointer: &str) -> Option<&Value> {
        self.capabilities.get()?.pointer(pointer).filter(|v| !v.is_null() && **v != Value::Bool(false))
    }

    /// The trigger characters of a provider (`completionProvider`, `signatureHelpProvider`).
    pub fn trigger_characters(&self, provider: &str) -> Vec<String> {
        self.capability(&format!("/{provider}/triggerCharacters"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    /// Work in progress, e.g. "indexing: 40/120", or None when idle.
    pub fn progress(&self) -> Option<String> {
        self.shared.progress.lock().unwrap().last().map(|(_, text)| text.clone())
    }

    /// Edits the server asked to apply since the last call.
    pub fn take_applied_edits(&self) -> Vec<Value> {
        std::mem::take(&mut *self.shared.applied_edits.lock().unwrap())
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
                            "declaration": { "linkSupport": true },
                            "typeDefinition": { "linkSupport": true },
                            "implementation": { "linkSupport": true },
                            "references": {},
                            "publishDiagnostics": { "relatedInformation": true },
                            // The editor has no snippet engine: plain insert text only.
                            "completion": {
                                "completionItem": {
                                    "snippetSupport": false,
                                    "documentationFormat": ["markdown", "plaintext"],
                                    "labelDetailsSupport": true,
                                },
                                "contextSupport": true,
                            },
                            "signatureHelp": {
                                "signatureInformation": {
                                    "documentationFormat": ["markdown", "plaintext"],
                                    "parameterInformation": { "labelOffsetSupport": true },
                                    "activeParameterSupport": true,
                                },
                            },
                            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                            "codeAction": {
                                "codeActionLiteralSupport": { "codeActionKind": { "valueSet": [
                                    "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                                    "refactor.rewrite", "source", "source.organizeImports",
                                ]}},
                                "isPreferredSupport": true,
                            },
                            "formatting": {},
                            "rename": { "prepareSupport": false },
                        },
                        "workspace": {
                            "workspaceFolders": true,
                            "configuration": true,
                            "applyEdit": true,
                            "symbol": {},
                            "workspaceEdit": { "documentChanges": true },
                        },
                        "window": { "workDoneProgress": true },
                    },
                }),
            )
            .await?;
        let _ = self.capabilities.set(result.get("capabilities").cloned().unwrap_or(Value::Null));
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
                "workspace/workspaceFolders" => {
                    let uri = path_to_uri(&shared.real_root).map(|u| u.to_string());
                    json!([{ "uri": uri, "name": shared.real_root.file_name().map(|n| n.to_string_lossy().to_string()) }])
                }
                "workspace/applyEdit" => {
                    let edit = msg.pointer("/params/edit").cloned().unwrap_or(Value::Null);
                    shared.applied_edits.lock().unwrap().push(map_uris(edit, &shared.real_root, &shared.open_root));
                    json!({ "applied": true })
                }
                _ => Value::Null,
            };
            let _ = lsp::send(server_id, &json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string());
        }
        (None, Some(method)) if method == "$/progress" => {
            if let Some(params) = msg.get("params") {
                on_progress(shared, params);
            }
        }
        (None, Some(method)) => {
            let params = map_uris(msg.get("params").cloned().unwrap_or(Value::Null), &shared.real_root, &shared.open_root);
            let _ = shared.notifications.unbounded_send(Notification { method, params });
        }
        (None, None) => {}
    }
}

/// `$/progress` (work done): begin/report set the token's text, end clears it.
fn on_progress(shared: &Shared, params: &Value) {
    let token = params.get("token").map(|t| t.to_string()).unwrap_or_default();
    let Some(value) = params.get("value") else { return };
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or_default();
    let mut progress = shared.progress.lock().unwrap();
    if kind == "end" {
        progress.retain(|(t, _)| *t != token);
        return;
    }
    let title = match kind {
        "begin" => value.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
        _ => progress.iter().find(|(t, _)| *t == token).map(|(_, text)| text.split(':').next().unwrap_or_default().to_string()).unwrap_or_default(),
    };
    let detail = value
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.get("percentage").and_then(Value::as_u64).map(|p| format!("{p}%")));
    let text = match detail {
        Some(d) if !d.is_empty() => format!("{title}: {d}"),
        _ => title,
    };
    match progress.iter_mut().find(|(t, _)| *t == token) {
        Some(entry) => entry.1 = text,
        None => progress.push((token, text)),
    }
}
