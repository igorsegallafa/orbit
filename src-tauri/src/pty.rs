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
pub async fn pty_spawn(
    app: AppHandle,
    cwd: String,
    cmd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<u32, String> {
    let id = next_id();
    let app_for_reader = app.clone();
    let cwd = expand_home(&cwd);
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

        let mut cmd_builder = CommandBuilder::new(match &cmd {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => default_shell(),
        });
        cmd_builder.cwd(&cwd);
        cmd_builder.env("TERM", "xterm-256color");

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

        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = app_for_reader.emit(
                        "pty-output",
                        PtyOutput { id, data: &buf[..n] },
                    );
                }
                Err(_) => break,
            }
        }
        let _ = app_for_reader.emit("pty-exit", id);
        sessions().lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    });

    Ok(id)
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("$HOME") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest.trim_start_matches('/'));
        }
    }
    p.to_string()
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