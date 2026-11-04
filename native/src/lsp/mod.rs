//! Language servers for the editor: one process per (repo, language), started
//! on demand when a file of that language is opened. orbit-core picks and runs
//! the server (the same as the Tauri app); this module is the client side.
mod paths;
mod server;

pub use paths::{path_to_uri, uri_to_path};
pub use server::{LspServer, Notification};


use futures::channel::mpsc;
use futures::future::{FutureExt, Shared};
use gpui::{App, AppContext as _, Task};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

/// What a file is, for highlighting and for picking a server.
pub struct FileLanguage {
    /// gpui-component highlighter name.
    pub highlight: &'static str,
    /// Shown in the status bar.
    pub name: &'static str,
    /// Server key (`ServerDef::language`) and LSP `languageId`.
    pub lsp: Option<(&'static str, &'static str)>,
}

pub fn file_language(path: &Path) -> FileLanguage {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // Servers per language come from orbit_core::lsp::LANGUAGES; Python, Go,
    // C# and Lua have no highlighter compiled in, only the server.
    let (highlight, name, lsp) = match ext.as_str() {
        "c" => ("c", "C", Some(("cpp", "c"))),
        "h" | "hh" | "hpp" | "hxx" | "inl" | "ipp" | "tpp" | "cc" | "cpp" | "cxx" | "c++" => ("cpp", "C++", Some(("cpp", "cpp"))),
        "rs" => ("rust", "Rust", Some(("rust", "rust"))),
        "ts" | "mts" | "cts" => ("typescript", "TypeScript", Some(("typescript", "typescript"))),
        "tsx" => ("tsx", "TypeScript", Some(("typescript", "typescriptreact"))),
        "js" | "mjs" | "cjs" => ("javascript", "JavaScript", Some(("typescript", "javascript"))),
        "jsx" => ("javascript", "JavaScript", Some(("typescript", "javascriptreact"))),
        "py" | "pyi" => ("text", "Python", Some(("python", "python"))),
        "go" => ("text", "Go", Some(("go", "go"))),
        "cs" => ("text", "C#", Some(("csharp", "csharp"))),
        "lua" => ("text", "Lua", Some(("lua", "lua"))),
        "json" => ("json", "JSON", None),
        "md" | "markdown" => ("text", "Markdown", None),
        _ => ("text", "Plain Text", None),
    };
    FileLanguage { highlight, name, lsp }
}

/// A server being started; the error says why it isn't running.
pub type ServerFuture = Shared<Task<Result<Arc<LspServer>, String>>>;

/// A workspace repo: what orbit-core starts a server for.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RepoKey {
    pub workspace: String,
    pub repo: String,
}

struct Registry {
    servers: HashMap<(RepoKey, &'static str), ServerFuture>,
    notifications: mpsc::UnboundedSender<Notification>,
}

/// The running servers, keyed by repo and server language. A cheap handle:
/// editors keep a clone to restart their server.
#[derive(Clone)]
pub struct LspRegistry(Rc<RefCell<Registry>>);

impl LspRegistry {
    pub fn new(notifications: mpsc::UnboundedSender<Notification>) -> Self {
        Self(Rc::new(RefCell::new(Registry { servers: HashMap::new(), notifications })))
    }

    /// Stops the server so the next `server` call starts a new one.
    pub fn restart(&self, repo: &RepoKey, language: &'static str) {
        let old = self.0.borrow_mut().servers.remove(&(repo.clone(), language));
        if let Some(server) = old.and_then(|f| f.peek().and_then(|r| r.as_ref().ok()).cloned()) {
            server.shutdown();
        }
    }

    /// The server for `language` in `repo`, started (and initialized in the
    /// background) the first time it's asked for.
    pub fn server(&self, repo: &RepoKey, language: &'static str, cx: &mut App) -> ServerFuture {
        let key = (repo.clone(), language);
        let mut this = self.0.borrow_mut();
        if let Some(existing) = this.servers.get(&key) {
            let dead = existing.peek().is_some_and(|s| s.as_ref().map_or(true, |s| s.has_exited()));
            if !dead {
                return existing.clone();
            }
        }
        let tx = this.notifications.clone();
        let repo = repo.clone();
        let task = cx.background_spawn(async move {
            let started = std::time::Instant::now();
            // Picks the server (config, PATH) and, for clangd, the compilation database.
            let server = LspServer::spawn(&repo.workspace, &repo.repo, language, tx).map_err(|e| {
                eprintln!("lsp: {e:#}");
                e.to_string()
            })?;
            match server.initialize().await {
                Ok(_) => {
                    eprintln!("lsp: {} ready in {} ms (cwd {})", server.command, started.elapsed().as_millis(), server.real_root.display());
                    Ok(server)
                }
                Err(e) => {
                    eprintln!("lsp: {} failed to initialize: {e:#}", server.command);
                    Err(format!("{} failed to initialize", server.command))
                }
            }
        });
        let shared = task.shared();
        this.servers.insert(key, shared.clone());
        shared
    }

    pub fn running(&self) -> Vec<Arc<LspServer>> {
        self.0.borrow().servers.values().filter_map(|f| f.peek().and_then(|r| r.as_ref().ok()).cloned()).collect()
    }

    pub fn shutdown_all(&mut self) {
        for server in self.running() {
            server.shutdown();
        }
        self.0.borrow_mut().servers.clear();
    }
}
