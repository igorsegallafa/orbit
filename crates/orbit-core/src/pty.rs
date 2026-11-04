//! Terminal sessions on a PTY (`portable-pty`): shells, agents (claude,
//! opencode) and run configurations. Output reaches the UI through a callback,
//! in bursts; a consumer slower than the program holds the program back
//! instead of filling memory. Sessions with a key keep the tail of
//! their output on disk so a restarted app can show it again.
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Terminal size in cells, plus pixels for programs that draw images.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl From<Size> for PtySize {
    fn from(s: Size) -> Self {
        PtySize { rows: s.rows, cols: s.cols, pixel_width: s.pixel_width, pixel_height: s.pixel_height }
    }
}

/// What to run, where.
#[derive(Clone, Debug, Default)]
pub struct Spawn {
    pub cwd: PathBuf,
    /// None (or blank): the user's shell.
    pub cmd: Option<String>,
    pub args: Vec<String>,
    /// Extra variables, set last.
    pub env: Vec<(String, String)>,
    pub size: Size,
    /// The tab's stable id: claude's hook route, and the scrollback file's name.
    pub key: Option<String>,
    /// Keep the output's tail on disk under `key` (see [`scrollback`]).
    pub scrollback: bool,
    /// `TERM_PROGRAM_VERSION` for the child.
    pub program_version: String,
}

pub enum Output {
    /// A burst of output (never empty).
    Data(Vec<u8>),
    /// The program ended; nothing follows.
    Exit,
}

struct Session {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}

static SESSIONS: Mutex<Option<HashMap<u32, Session>>> = Mutex::new(None);

fn with_session<T>(id: u32, f: impl FnOnce(&mut Session) -> T) -> Result<T, String> {
    let mut guard = SESSIONS.lock().unwrap();
    guard.as_mut().and_then(|m| m.get_mut(&id)).map(f).ok_or_else(|| format!("pty session {id} not found"))
}

/// A burst within this interval goes to the UI as one message.
const OUTPUT_INTERVAL: Duration = Duration::from_millis(8);
const OUTPUT_MAX: usize = 256 * 1024;
/// Reads queued between the reader and the burst thread; past this the reader
/// waits, so a consumer slower than the program holds the program back.
const READ_QUEUE: usize = 64;

/// Set by the terminal Orbit was started from; programs would mistake Orbit for it.
const OUTER_TERMINAL_ENV: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "VTE_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "KITTY_INSTALLATION_DIR",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_UNIX_SOCKET",
    "KONSOLE_VERSION",
    "KONSOLE_DBUS_SESSION",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "ITERM_SESSION_ID",
    "WINDOWID",
];
const OUTER_TERMINAL_PREFIXES: &[&str] = &["GHOSTTY_"];

/// A fresh session id. Reserved before `spawn` so a caller can wire the id
/// into its output handler first.
pub fn reserve_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// The folder a session runs in: a run configuration's `cwd`, else the repo's
/// folder in the workspace, else the workspace (or folder scope) itself.
pub fn session_dir(workspace: &str, repo: Option<&str>, cwd: Option<&str>) -> Result<PathBuf, String> {
    let dir = match (cwd.filter(|c| !c.is_empty()), repo.filter(|r| !r.is_empty())) {
        (Some(c), _) => PathBuf::from(c),
        (None, Some(r)) => crate::workspace::repo_path(workspace, r)?,
        (None, None) => crate::workspace::scope_dir(workspace)?,
    };
    if !dir.exists() {
        return Err(format!("directory does not exist: {}", dir.to_string_lossy()));
    }
    Ok(dir)
}

/// Starts `spec` as session `id` (from [`reserve_id`]). `on_output` runs on a
/// background thread with each burst, then once with [`Output::Exit`].
pub fn spawn(id: u32, spec: Spawn, mut on_output: impl FnMut(Output) + Send + 'static) -> Result<(), String> {
    // Spawn at the real terminal size: TUI apps (opencode, claude) size
    // their canvas from the initial PTY size and don't always reflow on
    // SIGWINCH, so a wrong default leaves a small box in the corner.
    let size = Size { rows: spec.size.rows.max(4), cols: spec.size.cols.max(20), ..spec.size };
    let pair = native_pty_system().openpty(size.into()).map_err(|e| e.to_string())?;
    let child = pair.slave.spawn_command(command(&spec)).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
    SESSIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, Session { writer, child, master: pair.master });

    let scrollback = spec.key.as_deref().filter(|_| spec.scrollback).map(Scrollback::start);
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(READ_QUEUE);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut last_send = Instant::now().checked_sub(OUTPUT_INTERVAL).unwrap_or_else(Instant::now);
        while let Ok(first) = rx.recv() {
            let mut out = first;
            let open = drain_burst(&rx, &mut out, last_send + OUTPUT_INTERVAL);
            if let Some(sb) = &scrollback {
                if let Ok(mut sb) = sb.lock() {
                    sb.push(&out);
                }
            }
            on_output(Output::Data(out));
            last_send = Instant::now();
            if !open {
                break;
            }
        }
        if let Some(Ok(mut sb)) = scrollback.as_ref().map(|s| s.lock()) {
            sb.flush();
        }
        on_output(Output::Exit);
        SESSIONS.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    });
    Ok(())
}

fn command(spec: &Spawn) -> CommandBuilder {
    let cmd = spec.cmd.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let mut b = CommandBuilder::new(cmd.map_or_else(default_shell, str::to_string));
    // Claude sessions report their lifecycle back to Orbit via hooks.
    if let (Some(c), Some(k)) = (cmd, &spec.key) {
        if is_claude(c) {
            if crate::config::Config::load().is_ok_and(|cfg| cfg.ai.remote_control) {
                b.arg("--remote-control");
            }
            for a in crate::agent_hooks::claude_args(k) {
                b.arg(a);
            }
        }
    }
    // Extra argv for the launched program (e.g. a prompt for an agent).
    for a in &spec.args {
        b.arg(a);
    }
    b.cwd(&spec.cwd);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if OUTER_TERMINAL_ENV.contains(&k.as_ref()) || OUTER_TERMINAL_PREFIXES.iter().any(|p| k.starts_with(p)) {
            b.env_remove(k.as_ref());
        }
    }
    b.env("TERM", "xterm-256color");
    b.env("COLORTERM", "truecolor");
    b.env("TERM_PROGRAM", "Orbit");
    b.env("TERM_PROGRAM_VERSION", &spec.program_version);
    // Claude Code only detects kitty graphics support in kitty and Ghostty.
    b.env("CLAUDE_CODE_FORCE_TERMINAL_IMAGES", "1");
    for k in crate::proc::CLAUDE_SESSION_ENV {
        b.env_remove(k);
    }
    for (k, v) in crate::proc::host_env() {
        match v {
            Some(v) => b.env(k, v),
            None => b.env_remove(k),
        }
    }
    for (k, v) in &spec.env {
        b.env(k, v);
    }
    b
}

/// False once the reader has ended.
fn drain_burst(rx: &mpsc::Receiver<Vec<u8>>, out: &mut Vec<u8>, until: Instant) -> bool {
    while out.len() < OUTPUT_MAX {
        let next = match until.checked_duration_since(Instant::now()) {
            Some(left) if !left.is_zero() => rx.recv_timeout(left),
            _ => rx.try_recv().map_err(|e| match e {
                mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
            }),
        };
        match next {
            Ok(more) => out.extend_from_slice(&more),
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
}

pub fn write(id: u32, data: &[u8]) -> Result<(), String> {
    // The session lock is not held while writing: a program that stops
    // reading its input must not block every other session.
    let writer = with_session(id, |s| s.writer.clone())?;
    let mut w = writer.lock().unwrap();
    w.write_all(data).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

pub fn resize(id: u32, size: Size) -> Result<(), String> {
    with_session(id, |s| s.master.resize(size.into()).map_err(|e| e.to_string()))?
}

pub fn kill(id: u32) -> Result<(), String> {
    let removed = SESSIONS.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
    let mut s = removed.ok_or_else(|| format!("pty session {id} not found"))?;
    let _ = s.child.kill();
    Ok(())
}

const SCROLLBACK_MAX: usize = 256 * 1024;

/// Tail of a session's output, persisted per tab so a restarted Orbit can
/// show what a terminal printed before. Seeded from the previous file so
/// history accumulates across restarts.
struct Scrollback {
    path: Option<PathBuf>,
    data: VecDeque<u8>,
    dirty: bool,
}

fn scrollback_path(key: &str) -> Option<PathBuf> {
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
    fn start(key: &str) -> Arc<Mutex<Self>> {
        let path = scrollback_path(key);
        let data = path.as_ref().and_then(|p| std::fs::read(p).ok()).unwrap_or_default().into();
        let sb = Arc::new(Mutex::new(Scrollback { path, data, dirty: false }));
        let weak = Arc::downgrade(&sb);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            let Some(sb) = weak.upgrade() else { break };
            if let Ok(mut guard) = sb.lock() {
                guard.flush();
            };
        });
        sb
    }

    fn push(&mut self, bytes: &[u8]) {
        let bytes = &bytes[bytes.len().saturating_sub(SCROLLBACK_MAX)..];
        let cut = (self.data.len() + bytes.len()).saturating_sub(SCROLLBACK_MAX);
        self.data.drain(..cut);
        self.data.extend(bytes);
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
        let _ = std::fs::write(path, self.data.make_contiguous());
        self.dirty = false;
    }
}

/// Saved output of a tab's previous session (empty when none).
pub fn scrollback(key: &str) -> Vec<u8> {
    scrollback_path(key).and_then(|p| std::fs::read(p).ok()).unwrap_or_default()
}

/// A tab closed for good: drop what was kept to restore it.
pub fn forget(key: &str) {
    if let Some(p) = scrollback_path(key) {
        let _ = std::fs::remove_file(p);
    }
    if let Ok(dir) = crate::config::config_dir() {
        let _ = std::fs::remove_file(dir.join("agent-settings").join(format!("{}.json", key.replace(':', "_"))));
    }
}

/// True when Claude Code has a transcript for this session id in the
/// workspace folder, i.e. `--resume <id>` will find it.
pub fn claude_session_exists(workspace: &str, session_id: &str) -> bool {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    let (Ok(ws), Ok(home)) = (crate::workspace::scope_dir(workspace), crate::config::home_dir()) else { return false };
    home.join(".claude/projects")
        .join(crate::usage::dir_to_project_name(&ws))
        .join(format!("{session_id}.jsonl"))
        .exists()
}

/// The opencode session a terminal tab started: the latest one in the tab's
/// folder created since `since_ms` (the tab's launch), so it can be resumed
/// with `opencode --session <id>` after the tab is closed. Blocks on opencode.
pub fn opencode_session_id(workspace: &str, repo: Option<&str>, since_ms: u64) -> Result<Option<String>, String> {
    let cwd = match repo.filter(|r| !r.is_empty()) {
        Some(r) => crate::workspace::repo_path(workspace, r)?,
        None => crate::workspace::scope_dir(workspace)?,
    };
    let out = crate::proc::cmd("opencode")
        .args(["session", "list", "--format", "json", "-n", "50"])
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .output()
        .map_err(|e| format!("failed to run opencode: {e}"))?;
    let list: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(pick_opencode_session(&list, &cwd.to_string_lossy(), since_ms))
}

/// Latest-updated session in `dir` created at or after `since_ms` (with a
/// little slack for the tab's clock read vs opencode's).
fn pick_opencode_session(list: &[serde_json::Value], dir: &str, since_ms: u64) -> Option<String> {
    let norm = |p: &str| p.trim_end_matches(['/', '\\']).to_string();
    let dir = norm(dir);
    list.iter()
        .filter(|s| s.get("directory").and_then(|d| d.as_str()).map(norm).as_deref() == Some(dir.as_str()))
        .filter(|s| s.get("created").and_then(|c| c.as_u64()).unwrap_or(0) + 5_000 >= since_ms)
        .max_by_key(|s| s.get("updated").and_then(|u| u.as_u64()).unwrap_or(0))
        .and_then(|s| s.get("id").and_then(|i| i.as_str()).map(String::from))
}

pub fn is_claude(cmd: &str) -> bool {
    let name = Path::new(cmd.trim()).file_stem().map(|s| s.to_string_lossy().to_lowercase());
    name.as_deref() == Some("claude")
}

pub fn default_shell() -> String {
    if cfg!(windows) {
        return "powershell.exe".into();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_the_tabs_session_in_its_folder() {
        let list = vec![
            json!({"id": "old", "directory": "/w/a", "created": 1_000, "updated": 9_000}),
            json!({"id": "other-dir", "directory": "/w/b", "created": 20_000, "updated": 30_000}),
            json!({"id": "mine", "directory": "/w/a/", "created": 20_000, "updated": 25_000}),
        ];
        assert_eq!(pick_opencode_session(&list, "/w/a", 19_000).as_deref(), Some("mine"));
        assert_eq!(pick_opencode_session(&list, "/w/c", 0), None);
    }

    #[test]
    fn scrollback_keeps_the_tail_and_rejects_unsafe_keys() {
        let mut sb = Scrollback { path: None, data: VecDeque::new(), dirty: false };
        sb.push(&vec![b'a'; SCROLLBACK_MAX]);
        sb.push(b"END");
        assert_eq!(sb.data.len(), SCROLLBACK_MAX);
        assert!(sb.data.make_contiguous().ends_with(b"END"));
        sb.push(&vec![b'b'; SCROLLBACK_MAX + 10]);
        assert_eq!(sb.data.len(), SCROLLBACK_MAX);
        assert!(scrollback_path("../../etc").is_none());
        assert!(scrollback_path("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_program_and_reports_its_output_then_exit() {
        let (tx, rx) = mpsc::channel();
        let id = reserve_id();
        let spec = Spawn {
            cwd: std::env::temp_dir(),
            cmd: Some("/bin/sh".into()),
            args: vec!["-c".into(), "read l; echo got:$l; printf '%s' \"$TERM_PROGRAM\"".into()],
            size: Size { cols: 80, rows: 24, ..Default::default() },
            ..Default::default()
        };
        spawn(id, spec, move |o| {
            let _ = tx.send(match o {
                Output::Data(d) => Some(d),
                Output::Exit => None,
            });
        })
        .unwrap();
        write(id, b"hi\r").unwrap();
        let mut out = Vec::new();
        while let Ok(Some(d)) = rx.recv_timeout(Duration::from_secs(10)) {
            out.extend(d);
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("got:hi"), "{text}");
        assert!(text.ends_with("Orbit"), "{text}");
        assert!(write(id, b"x").is_err(), "the session is gone after exit");
    }
}
