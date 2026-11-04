//! Language servers for the editor: one process per (repo folder, language),
//! started on demand when a file of that language is opened. Server choice and
//! clangd's flags follow `src-tauri/src/lsp.rs`.
mod paths;
mod server;

pub use paths::{path_to_uri, uri_to_path};
pub use server::{LspServer, Notification};

use futures::channel::mpsc;
use futures::future::{FutureExt, Shared};
use gpui::{App, AppContext as _, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a file is, for highlighting and for picking a server.
pub struct FileLanguage {
    /// gpui-component highlighter name.
    pub highlight: &'static str,
    /// Server key (`ServerDef::language`) and LSP `languageId`.
    pub lsp: Option<(&'static str, &'static str)>,
}

pub fn file_language(path: &Path) -> FileLanguage {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (highlight, lsp) = match ext.as_str() {
        "c" => ("c", Some(("cpp", "c"))),
        "h" | "hh" | "hpp" | "hxx" | "inl" | "ipp" | "cc" | "cpp" | "cxx" | "c++" => ("cpp", Some(("cpp", "cpp"))),
        "rs" => ("rust", Some(("rust", "rust"))),
        "ts" | "mts" | "cts" => ("typescript", Some(("typescript", "typescript"))),
        "tsx" => ("tsx", Some(("typescript", "typescriptreact"))),
        "js" | "mjs" | "cjs" => ("javascript", Some(("typescript", "javascript"))),
        "jsx" => ("javascript", Some(("typescript", "javascriptreact"))),
        "json" => ("json", None),
        _ => ("text", None),
    };
    FileLanguage { highlight, lsp }
}

struct ServerDef {
    language: &'static str,
    bin: &'static str,
    args: &'static [&'static str],
}

const SERVERS: &[ServerDef] = &[
    ServerDef { language: "cpp", bin: "clangd", args: &["--background-index", "--header-insertion=never"] },
    ServerDef { language: "rust", bin: "rust-analyzer", args: &[] },
    ServerDef { language: "typescript", bin: "typescript-language-server", args: &["--stdio"] },
];

fn on_path(bin: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// The command for a language in a repo, with clangd pointed at a compilation
/// database it wouldn't find by itself.
fn command_for(language: &str, real_root: &Path) -> Option<(String, Vec<String>)> {
    let def = SERVERS.iter().find(|s| s.language == language)?;
    let bin = on_path(def.bin)?;
    let mut args: Vec<String> = def.args.iter().map(|a| a.to_string()).collect();
    if language == "cpp" {
        // clangd finds <root> and <root>/build itself; deeper preset dirs need a pointer.
        if let Some(dir) = paths::find_compile_commands(real_root).and_then(|p| p.parent().map(Path::to_path_buf)) {
            if dir != real_root && dir != real_root.join("build") {
                args.push(format!("--compile-commands-dir={}", dir.display()));
            }
        }
    }
    Some((bin.to_string_lossy().to_string(), args))
}

pub type ServerFuture = Shared<Task<Option<Arc<LspServer>>>>;

/// The running servers, keyed by the repo folder as the UI opens it.
pub struct LspRegistry {
    servers: HashMap<(PathBuf, &'static str), ServerFuture>,
    notifications: mpsc::UnboundedSender<Notification>,
    /// Why the last start failed, per key (status line / dumps).
    pub errors: HashMap<(PathBuf, &'static str), String>,
}

impl LspRegistry {
    pub fn new(notifications: mpsc::UnboundedSender<Notification>) -> Self {
        Self { servers: HashMap::new(), notifications, errors: HashMap::new() }
    }

    /// The server for `language` in the repo at `open_root`, started (and
    /// initialized in the background) the first time it's asked for.
    pub fn server(&mut self, open_root: &Path, language: &'static str, cx: &mut App) -> Option<ServerFuture> {
        let key = (open_root.to_path_buf(), language);
        if let Some(existing) = self.servers.get(&key) {
            let dead = existing.peek().is_some_and(|s| s.as_ref().is_none_or(|s| s.has_exited()));
            if !dead {
                return Some(existing.clone());
            }
        }
        let real_root = paths::real_path(open_root);
        let Some((bin, args)) = command_for(language, &real_root) else {
            self.errors.insert(key, format!("no {language} language server on PATH"));
            return None;
        };
        let open_root = open_root.to_path_buf();
        let tx = self.notifications.clone();
        let started = std::time::Instant::now();
        let task = cx.background_spawn(async move {
            let server = match LspServer::spawn(language, &bin, &args, &open_root, &real_root, tx) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("lsp: {e:#}");
                    return None;
                }
            };
            match server.initialize().await {
                Ok(_) => {
                    eprintln!("lsp: {} ready in {} ms (cwd {})", server.command, started.elapsed().as_millis(), real_root.display());
                    Some(server)
                }
                Err(e) => {
                    eprintln!("lsp: {} failed to initialize: {e:#}", server.command);
                    None
                }
            }
        });
        let shared = task.shared();
        self.servers.insert(key, shared.clone());
        Some(shared)
    }

    pub fn running(&self) -> Vec<Arc<LspServer>> {
        self.servers.values().filter_map(|f| f.peek().cloned().flatten()).collect()
    }

    pub fn shutdown_all(&mut self) {
        for server in self.running() {
            server.shutdown();
        }
        self.servers.clear();
    }
}
