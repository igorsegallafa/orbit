// Real terminal sessions backed by portable-pty. Agents (claude/opencode)
// and shells run inside the workspace folder; output streams to the frontend
// via Tauri events, input flows back through the `pty_write` command.
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

struct Session {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}

static SESSIONS: Mutex<Option<HashMap<u32, Session>>> = Mutex::new(None);

fn sessions() -> &'static Mutex<Option<HashMap<u32, Session>>> {
    &SESSIONS
}

#[derive(Serialize, Clone)]
struct PtyOutput<'a> {
    id: u32,
    data: &'a [u8],
}

fn next_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per invoke field
pub async fn pty_spawn(
    app: AppHandle,
    workspace: String,
    repo: Option<String>,
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cols: u16,
    rows: u16,
    key: Option<String>,
) -> Result<u32, String> {
    let id = next_id();
    let app_for_reader = app.clone();
    let cwd = match repo.filter(|r| !r.is_empty()) {
        Some(r) => crate::workspace::repo_path(&workspace, &r)?,
        None => crate::workspace::scope_dir(&workspace)?,
    }
    .to_string_lossy()
    .to_string();
    if !std::path::Path::new(&cwd).exists() {
        return Err(format!("directory does not exist: {cwd}"));
    }

    std::thread::spawn(move || {
        let pty_system = native_pty_system();
        // Spawn at the real terminal size: TUI apps (opencode, claude) size
        // their canvas from the initial PTY size and don't always reflow on
        // SIGWINCH, so a wrong default leaves a small box in the corner.
        let size = PtySize {
            rows: rows.max(4),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = match pty_system.openpty(size) {
            Ok(p) => p,
            Err(e) => {
                emit_pty_error(&app_for_reader, e.to_string());
                return;
            }
        };

        let mut cmd_builder = match &cmd {
            Some(c) if !c.trim().is_empty() => CommandBuilder::new(c.trim()),
            _ => CommandBuilder::new(default_shell()),
        };
        // Claude sessions report their lifecycle back to Orbit via hooks.
        if let (Some(c), Some(k)) = (&cmd, &key) {
            if is_claude(c) {
                for a in crate::agent_hooks::claude_args(k) {
                    cmd_builder.arg(a);
                }
            }
        }
        // Extra argv for the launched program (e.g. a prompt for an agent).
        if let Some(extra) = &args {
            for a in extra {
                cmd_builder.arg(a);
            }
        }
        cmd_builder.cwd(&cwd);
        cmd_builder.env("TERM", "xterm-256color");
        for k in crate::proc::CLAUDE_SESSION_ENV {
            cmd_builder.env_remove(k);
        }

        let child = match pair.slave.spawn_command(cmd_builder) {
            Ok(c) => c,
            Err(e) => {
                emit_pty_error(&app_for_reader, e.to_string());
                return;
            }
        };
        drop(pair.slave);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                emit_pty_error(&app_for_reader, e.to_string());
                return;
            }
        };

        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                emit_pty_error(&app_for_reader, e.to_string());
                return;
            }
        };
        let master = pair.master;

        {
            let mut guard = sessions().lock().unwrap();
            guard
                .get_or_insert_with(HashMap::new)
                .insert(id, Session { writer, child, master });
        }

        let scrollback = key.as_deref().map(Scrollback::start);
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = app_for_reader.emit(
                        "pty-output",
                        PtyOutput { id, data: &buf[..n] },
                    );
                    if let Some(sb) = &scrollback {
                        if let Ok(mut sb) = sb.lock() {
                            sb.push(&buf[..n]);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(Ok(mut sb)) = scrollback.as_ref().map(|s| s.lock()) {
            sb.flush();
        }
        let _ = app_for_reader.emit("pty-exit", id);
        sessions().lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    });

    Ok(id)
}

const SCROLLBACK_MAX: usize = 256 * 1024;

/// Tail of a session's output, persisted per tab so a restarted Orbit can
/// show what a terminal printed before. Seeded from the previous file so
/// history accumulates across restarts.
struct Scrollback {
    path: Option<std::path::PathBuf>,
    data: Vec<u8>,
    dirty: bool,
}

fn scrollback_path(key: &str) -> Option<std::path::PathBuf> {
    let safe = key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'));
    if !safe || key.is_empty() {
        return None;
    }
    Some(crate::config::config_dir().ok()?.join("scrollback").join(format!("{}.log", key.replace(':', "_"))))
}

impl Scrollback {
    /// Loads the previous tail and flushes every 2s while the session lives:
    /// the app can be killed, and a quiet terminal gets no more output to
    /// trigger a write.
    fn start(key: &str) -> std::sync::Arc<Mutex<Self>> {
        let path = scrollback_path(key);
        let data = path.as_ref().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let sb = std::sync::Arc::new(Mutex::new(Scrollback { path, data, dirty: false }));
        let weak = std::sync::Arc::downgrade(&sb);
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let Some(sb) = weak.upgrade() else { break };
            if let Ok(mut guard) = sb.lock() {
                guard.flush();
            };
        });
        sb
    }

    fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > SCROLLBACK_MAX {
            let cut = self.data.len() - SCROLLBACK_MAX;
            self.data.drain(..cut);
        }
        self.dirty = true;
    }

    fn flush(&mut self) {
        let Some(path) = &self.path else { return };
        if !self.dirty {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, &self.data);
        self.dirty = false;
    }
}

/// Saved output of a tab's previous session (empty when none).
#[tauri::command]
pub fn pty_scrollback(key: String) -> Vec<u8> {
    scrollback_path(&key).and_then(|p| std::fs::read(p).ok()).unwrap_or_default()
}

/// A tab closed for good: drop what was kept to restore it.
#[tauri::command]
pub fn pty_forget(key: String) {
    if let Some(p) = scrollback_path(&key) {
        let _ = std::fs::remove_file(p);
    }
    if let Ok(dir) = crate::config::config_dir() {
        let _ = std::fs::remove_file(dir.join("agent-settings").join(format!("{}.json", key.replace(':', "_"))));
    }
}

/// True when Claude Code has a transcript for this session id in the
/// workspace folder, i.e. `--resume <id>` will find it.
#[tauri::command]
pub fn claude_session_exists(workspace: String, session_id: String) -> bool {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    let (Ok(ws), Ok(home)) = (crate::workspace::scope_dir(&workspace), crate::config::home_dir()) else { return false };
    home.join(".claude/projects")
        .join(crate::usage::dir_to_project_name(&ws))
        .join(format!("{session_id}.jsonl"))
        .exists()
}

fn is_claude(cmd: &str) -> bool {
    let name = std::path::Path::new(cmd.trim()).file_stem().map(|s| s.to_string_lossy().to_lowercase());
    name.as_deref() == Some("claude")
}

fn default_shell() -> String {
    if cfg!(windows) {
        return "powershell.exe".into();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

fn emit_pty_error(app: &AppHandle, msg: String) {
    use serde_json::json;
    let _ = app.emit("pty-error", json!({ "error": msg }));
}

#[tauri::command]
pub fn pty_write(id: u32, data: String) -> Result<(), String> {
    let mut guard = sessions().lock().unwrap();
    let Some(sess) = guard.as_mut().and_then(|m| m.get_mut(&id)) else {
        return Err(format!("pty session {id} not found"));
    };
    sess.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    sess.writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn pty_resize(id: u32, cols: u16, rows: u16) -> Result<(), String> {
    let guard = sessions().lock().unwrap();
    let Some(sess) = guard.as_ref().and_then(|m| m.get(&id)) else {
        return Err(format!("pty session {id} not found"));
    };
    sess.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_keeps_the_tail_and_rejects_unsafe_keys() {
        let mut sb = Scrollback { path: None, data: vec![], dirty: false };
        sb.push(&vec![b'a'; SCROLLBACK_MAX]);
        sb.push(b"END");
        assert_eq!(sb.data.len(), SCROLLBACK_MAX);
        assert!(sb.data.ends_with(b"END"));
        assert!(scrollback_path("../../etc").is_none());
        assert!(scrollback_path("").is_none());
    }
}

#[tauri::command]
pub fn pty_kill(id: u32) -> Result<(), String> {
    let removed = sessions()
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|m| m.remove(&id))
        .map(|mut s| {
            let _ = s.child.kill();
            s
        })
        .is_some();
    if removed {
        Ok(())
    } else {
        Err(format!("pty session {id} not found"))
    }
}