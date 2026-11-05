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
use std::time::{Duration, Instant};

/// A server with no tab of its language open in its repo for this long is
/// stopped (clangd and rust-analyzer hold a gigabyte or more); opening one
/// starts it again.
pub const IDLE_GRACE: Duration = Duration::from_secs(120);

pub fn idle(in_use: bool, last_used: Instant, now: Instant) -> bool {
    !in_use && now.saturating_duration_since(last_used) >= IDLE_GRACE
}

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

pub type ServerKey = (RepoKey, &'static str);

struct Entry {
    server: ServerFuture,
    /// When a tab last used it (see `stop_idle`).
    last_used: Instant,
}

struct Registry {
    servers: HashMap<ServerKey, Entry>,
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
        if let Some(server) = old.and_then(|e| e.server.peek().and_then(|r| r.as_ref().ok()).cloned()) {
            server.shutdown();
        }
    }

    /// The server for `language` in `repo`, started (and initialized in the
    /// background) the first time it's asked for.
    pub fn server(&self, repo: &RepoKey, language: &'static str, cx: &mut App) -> ServerFuture {
        let key = (repo.clone(), language);
        let mut this = self.0.borrow_mut();
        if let Some(existing) = this.servers.get_mut(&key) {
            let dead = existing.server.peek().is_some_and(|s| s.as_ref().map_or(true, |s| s.has_exited()));
            if !dead {
                existing.last_used = Instant::now();
                return existing.server.clone();
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
        this.servers.insert(key, Entry { server: shared.clone(), last_used: Instant::now() });
        shared
    }

    pub fn running(&self) -> Vec<Arc<LspServer>> {
        self.0.borrow().servers.values().filter_map(|e| e.server.peek().and_then(|r| r.as_ref().ok()).cloned()).collect()
    }

    /// Stops the servers no tab has used (`in_use`) for `IDLE_GRACE`; one
    /// still starting is left for the next call.
    pub fn stop_idle(&self, in_use: &[ServerKey], now: Instant) {
        let mut this = self.0.borrow_mut();
        let mut stop = vec![];
        for (key, e) in this.servers.iter_mut() {
            let used = in_use.contains(key);
            if used {
                e.last_used = now;
            } else if idle(used, e.last_used, now) && e.server.peek().is_some() {
                stop.push(key.clone());
            }
        }
        for key in stop {
            if let Some(Ok(server)) = this.servers.remove(&key).and_then(|e| e.server.peek().cloned()) {
                eprintln!("lsp: stopping idle {} ({})", server.command, key.0.repo);
                server.shutdown();
            }
        }
    }

    pub fn shutdown_all(&mut self) {
        for server in self.running() {
            server.shutdown();
        }
        self.0.borrow_mut().servers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_stop_only_after_the_grace_unused() {
        let t0 = Instant::now();
        assert!(!idle(false, t0, t0 + IDLE_GRACE - Duration::from_secs(1)));
        assert!(idle(false, t0, t0 + IDLE_GRACE));
        assert!(!idle(true, t0, t0 + IDLE_GRACE * 10));
        // A clock that reads earlier than last use never stops it.
        assert!(!idle(false, t0 + IDLE_GRACE, t0));
    }
}
