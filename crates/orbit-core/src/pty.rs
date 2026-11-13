//! Terminal sessions on a PTY (`portable-pty`): shells, agents (claude,
//! opencode) and run configurations. Output reaches the UI through a callback,
//! in bursts; a consumer slower than the program holds the program back
//! instead of filling memory. Sessions with a key keep the tail of
//! their output on disk so a restarted app can show it again.
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
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
    /// Start nothing: the terminal waits for a program another process starts
    /// on it ([`tty_path`]; the debugger's `set inferior-tty`). It ends once
    /// [`release_tty`] was called and that program has closed it.
    pub tty_only: bool,
}

pub enum Output {
    /// A burst of output (never empty).
    Data(Vec<u8>),
    /// The program ended; nothing follows.
    Exit,
}

struct Session {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    /// Held open for a `tty_only` session until [`release_tty`].
    slave: Option<Box<dyn portable_pty::SlavePty + Send>>,
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
    // No trailing slash (`dir:~` joins ""): claude names its transcript folder after the exact path.
    Ok(dir.components().collect())
}

/// Starts `spec` as session `id` (from [`reserve_id`]). `on_output` runs on a
/// background thread with each burst, then once with [`Output::Exit`].
pub fn spawn(id: u32, spec: Spawn, mut on_output: impl FnMut(Output) + Send + 'static) -> Result<(), String> {
    // Spawn at the real terminal size: TUI apps (opencode, claude) size
    // their canvas from the initial PTY size and don't always reflow on
    // SIGWINCH, so a wrong default leaves a small box in the corner.
    let size = Size { rows: spec.size.rows.max(4), cols: spec.size.cols.max(20), ..spec.size };
    let pair = native_pty_system().openpty(size.into()).map_err(|e| e.to_string())?;
    let (child, slave) = if spec.tty_only {
        (None, Some(pair.slave))
    } else {
        let child = pair.slave.spawn_command(command(&spec)).map_err(|e| e.to_string())?;
        drop(pair.slave);
        (Some(child), None)
    };
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
    SESSIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, Session { writer, child, slave, master: pair.master });

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
    // Agent sessions get Orbit's MCP server and reply style; claude also reports its lifecycle via hooks.
    let mut launch_env = vec![];
    if let (Some(c), Some(k)) = (cmd, &spec.key) {
        if is_claude(c) && crate::config::Config::load().is_ok_and(|cfg| cfg.ai.remote_control) {
            b.arg("--remote-control");
        }
        let (args, env) = crate::agent_hooks::session_launch(c, k, false, &spec.cwd);
        b.args(args);
        launch_env = env;
    }
    // Extra argv for the launched program (e.g. a prompt for an agent).
    for a in &spec.args {
        b.arg(a);
    }
    b.cwd(&spec.cwd);
    b.env_clear();
    // The spec's own variables win: an OPENCODE_CONFIG_CONTENT set by a caller stays.
    launch_env.retain(|(k, _)| !spec.env.iter().any(|(sk, _)| sk == k));
    launch_env.extend(spec.env.iter().cloned());
    for (k, v) in child_env(&launch_env, &spec.program_version) {
        b.env(k, v);
    }
    b
}

/// The environment of a program Orbit starts (a session, Run, the program
/// under the debugger): Orbit's own, without what belongs to its launcher
/// (`proc::host_env`: an AppImage's mount), to the terminal Orbit was started
/// from, or to a Claude session; then the terminal's `TERM` and friends; then
/// `extra`, which wins.
pub fn child_env(extra: &[(String, String)], program_version: &str) -> Vec<(OsString, OsString)> {
    merge_env(std::env::vars_os(), crate::proc::host_env(), extra, program_version)
}

fn merge_env(
    inherited: impl Iterator<Item = (OsString, OsString)>,
    host: Vec<(OsString, Option<OsString>)>,
    extra: &[(String, String)],
    program_version: &str,
) -> Vec<(OsString, OsString)> {
    let mut env: BTreeMap<OsString, OsString> = inherited
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            !OUTER_TERMINAL_ENV.contains(&k.as_ref())
                && !OUTER_TERMINAL_PREFIXES.iter().any(|p| k.starts_with(p))
                && !crate::proc::CLAUDE_SESSION_ENV.contains(&k.as_ref())
        })
        .collect();
    for (k, v) in host {
        match v {
            Some(v) => env.insert(k, v),
            None => env.remove(&k),
        };
    }
    let terminal = [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "Orbit"),
        ("TERM_PROGRAM_VERSION", program_version),
        // Claude Code only detects kitty graphics support in kitty and Ghostty.
        ("CLAUDE_CODE_FORCE_TERMINAL_IMAGES", "1"),
    ];
    for (k, v) in terminal.into_iter().chain(extra.iter().map(|(k, v)| (k.as_str(), v.as_str()))) {
        env.insert(k.into(), v.into());
    }
    env.into_iter().collect()
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
    let s = removed.ok_or_else(|| format!("pty session {id} not found"))?;
    if let Some(mut child) = s.child {
        let _ = child.kill();
    }
    Ok(())
}

/// The terminal device of a `tty_only` session, for a program to run on.
#[cfg(unix)]
pub fn tty_path(id: u32) -> Option<PathBuf> {
    with_session(id, |s| s.master.tty_name()).ok().flatten()
}

/// A `tty_only` session stops holding its terminal open: it ends once the
/// program on it has closed it too.
pub fn release_tty(id: u32) {
    let _ = with_session(id, |s| s.slave.take());
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

/// The files `forget` would have removed for sessions that are gone (Orbit
/// killed, or ended elsewhere), with their sizes; `live` keys keep theirs.
pub fn leftovers(live: &[String]) -> Vec<(PathBuf, u64)> {
    crate::config::config_dir().map(|dir| leftovers_in(&dir, live)).unwrap_or_default()
}

fn leftovers_in(config: &Path, live: &[String]) -> Vec<(PathBuf, u64)> {
    let live: Vec<String> = live.iter().map(|k| k.replace(':', "_")).collect();
    let mut out = vec![];
    for (dir, ext) in [("scrollback", "log"), ("agent-settings", "json")] {
        for entry in std::fs::read_dir(config.join(dir)).into_iter().flatten().flatten() {
            let path = entry.path();
            // Only session files (`tm_<ms>-<id>.<ext>`), never links or folders.
            let ours = path.extension().is_some_and(|e| e == ext)
                && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("tm_") && !live.iter().any(|k| k == s));
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if ours && meta.is_file() => out.push((path, meta.len())),
                _ => {}
            }
        }
    }
    out
}

/// True when Claude Code has a transcript for this session id for the folder
/// the session runs in (its repo or cwd, not always the workspace root), i.e.
/// `--resume <id>` will find it.
pub fn claude_session_exists(dir: &Path, session_id: &str) -> bool {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    let Ok(home) = crate::config::home_dir() else { return false };
    home.join(".claude/projects")
        .join(crate::usage::dir_to_project_name(dir))
        .join(format!("{session_id}.jsonl"))
        .exists()
}

/// True for oh-my-pi's CLI (`omp`).
pub fn is_omp(cmd: &str) -> bool {
    let name = Path::new(cmd.trim()).file_stem().map(|s| s.to_string_lossy().to_lowercase());
    name.as_deref() == Some("omp")
}

/// omp's session folder for `dir`: `~/.omp/agent/sessions/-Documents-x` for
/// `~/Documents/x`, `--private-tmp--` outside the home folder.
fn omp_sessions_dir(home: &Path, dir: &Path) -> PathBuf {
    let flat = |p: &Path| p.to_string_lossy().trim_matches('/').replace(['/', '\\'], "-");
    let name = match dir.strip_prefix(home) {
        Ok(rel) => format!("-{}", flat(rel)),
        Err(_) => format!("--{}--", flat(dir)),
    };
    home.join(".omp/agent/sessions").join(name)
}

/// The omp sessions `count` omp tabs in `dir` were last in, to `omp --resume`
/// them: omp can't take an id at launch, and `/resume` inside a tab switches
/// to an older session, so they are the `count` sessions written to last
/// (since `since_ms`, the first tab's launch). Oldest-created first, to pair
/// with the tabs by launch order. Files are `<ISO time>_<id>.jsonl`.
pub fn omp_recent_sessions(dir: &Path, since_ms: u64, count: usize) -> Vec<String> {
    let Ok(home) = crate::config::home_dir() else { return vec![] };
    let Ok(entries) = std::fs::read_dir(omp_sessions_dir(&home, dir)) else { return vec![] };
    let since = std::time::UNIX_EPOCH + Duration::from_millis(since_ms.saturating_sub(5_000));
    let mut found: Vec<(std::time::SystemTime, i64, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (time, id) = name.strip_suffix(".jsonl")?.split_once('_')?;
            let created = crate::usage::iso_to_epoch(time)?;
            let modified = e.metadata().and_then(|m| m.modified()).ok().filter(|m| *m >= since)?;
            Some((modified, created, id.to_string()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.truncate(count);
    found.sort_by_key(|f| f.1);
    found.into_iter().map(|f| f.2).collect()
}

/// The opencode session a terminal tab started: the latest one in the tab's
/// folder created since `since_ms` (the tab's launch), so it can be resumed
/// with `opencode --session <id>` after the tab is closed. Blocks on opencode.
pub fn opencode_session_id(cwd: &Path, since_ms: u64) -> Result<Option<String>, String> {
    let out = crate::proc::cmd("opencode")
        .args(["session", "list", "--format", "json", "-n", "50"])
        .current_dir(cwd)
        .env("PWD", cwd)
        .output()
        .map_err(|e| format!("failed to run opencode: {e}"))?;
    let list: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(pick_opencode_session(&list, &cwd.to_string_lossy(), since_ms))
}

/// The conversation a terminal tab of `agent` started in `dir`: the newest one
/// created since `since_ms`, the tab's launch. Blocks.
pub fn agent_session_id(agent: &str, dir: &Path, since_ms: u64) -> Option<String> {
    let home = crate::config::home_dir().ok()?;
    match agent {
        "codex" => newest_codex_session(&home.join(".codex/sessions"), dir, since_ms),
        "opencode" => opencode_session_id(dir, since_ms).ok().flatten(),
        "omp" => newest_omp_session(&omp_sessions_dir(&home, dir), since_ms),
        _ => None,
    }
}

/// omp files are `<ISO time>_<id>.jsonl`; 5 s of slack for the tab's clock read.
fn newest_omp_session(folder: &Path, since_ms: u64) -> Option<String> {
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (time, id) = name.strip_suffix(".jsonl")?.split_once('_')?;
            let created = crate::usage::iso_to_epoch(time)? * 1000;
            (created + 5_000 >= since_ms as i64).then(|| (created, id.to_string()))
        })
        .max()
        .map(|f| f.1)
}

/// codex rollouts are `<root>/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, opening
/// with a `session_meta` line that has the id, cwd and creation time.
fn newest_codex_session(root: &Path, dir: &Path, since_ms: u64) -> Option<String> {
    fn entries(p: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
        std::fs::read_dir(p).into_iter().flatten().flatten()
    }
    let since = std::time::UNIX_EPOCH + Duration::from_millis(since_ms.saturating_sub(5_000));
    let fresh = |e: &std::fs::DirEntry| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since);
    let norm = |p: &str| p.trim_end_matches(['/', '\\']).to_string();
    let dir = norm(&dir.to_string_lossy());
    // A day folder gets newer when a rollout is created in it: older ones are skipped unread.
    let days = entries(root).flat_map(|y| entries(&y.path())).flat_map(|m| entries(&m.path())).filter(fresh);
    days.flat_map(|d| entries(&d.path()))
        .filter(fresh)
        .filter_map(|f| {
            let mut line = String::new();
            std::io::BufRead::read_line(&mut std::io::BufReader::new(std::fs::File::open(f.path()).ok()?), &mut line).ok()?;
            let v: serde_json::Value = serde_json::from_str(&line).ok()?;
            let meta = &v["payload"];
            if v["type"] != "session_meta" || meta["cwd"].as_str().map(norm).as_deref() != Some(dir.as_str()) {
                return None;
            }
            let created = crate::usage::iso_to_epoch(meta["timestamp"].as_str()?)? * 1000;
            (created + 5_000 >= since_ms as i64).then(|| (created, meta["id"].as_str().map(String::from)))
        })
        .max()
        .and_then(|f| f.1)
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

    #[test]
    fn session_dir_has_no_trailing_slash() {
        let tmp = std::env::temp_dir();
        let with_slash = format!("{}/", tmp.to_string_lossy().trim_end_matches('/'));
        assert_eq!(session_dir("x", None, Some(&with_slash)).unwrap().to_string_lossy(), tmp.to_string_lossy().trim_end_matches('/'));
    }
    use serde_json::json;

    #[test]
    fn omp_session_folder_names() {
        let home = Path::new("/Users/me");
        let dir = |p: &str| omp_sessions_dir(home, Path::new(p)).file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(dir("/Users/me/Documents/orbit-workspace/workspaces/a-b"), "-Documents-orbit-workspace-workspaces-a-b");
        assert_eq!(dir("/private/tmp"), "--private-tmp--");
    }

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
    fn finds_the_newest_codex_and_omp_session_since_launch() {
        let root = std::env::temp_dir().join(format!("orbit-agent-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join("codex/2026/11/12");
        std::fs::create_dir_all(&day).unwrap();
        let rollout = |id: &str, cwd: &str, at: &str| {
            let meta = json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd, "timestamp": at}});
            std::fs::write(day.join(format!("rollout-{at}-{id}.jsonl")), format!("{meta}\n{{}}\n")).unwrap();
        };
        rollout("old", "/w/a", "2026-11-12T10:00:00.000Z");
        rollout("mine", "/w/a", "2026-11-12T12:00:00.000Z");
        rollout("other-dir", "/w/b", "2026-11-12T12:30:00.000Z");
        let launch = (crate::usage::iso_to_epoch("2026-11-12T11:59:58Z").unwrap() * 1000) as u64;
        let codex = root.join("codex");
        assert_eq!(newest_codex_session(&codex, Path::new("/w/a/"), launch).as_deref(), Some("mine"));
        assert_eq!(newest_codex_session(&codex, Path::new("/w/c"), launch), None);
        assert_eq!(newest_codex_session(&codex, Path::new("/w/a"), launch + 3_600_000), None);

        let omp = root.join("omp");
        std::fs::create_dir_all(&omp).unwrap();
        for name in ["2026-11-12T10-00-00-000Z_old", "2026-11-12T12-00-01-500Z_mine", "notes"] {
            std::fs::write(omp.join(format!("{name}.jsonl")), "{}").unwrap();
        }
        assert_eq!(newest_omp_session(&omp, launch).as_deref(), Some("mine"));
        assert_eq!(newest_omp_session(&omp, launch + 3_600_000), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn leftovers_are_dead_session_files_only() {
        let root = std::env::temp_dir().join(format!("orbit-leftovers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, name, body) in [
            ("scrollback", "tm_1-dead.log", "12345"),
            ("scrollback", "tm_2-live.log", "x"),
            ("scrollback", "notes.log", "x"),
            ("scrollback", "tm_3-dead.txt", "x"),
            ("agent-settings", "tm_1-dead.json", "{}"),
            ("agent-settings", "tm_2-live.json", "{}"),
            ("other", "tm_4-dead.log", "x"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(name), body).unwrap();
        }
        std::fs::create_dir_all(root.join("scrollback/tm_5-dir.log")).unwrap();
        let mut found: Vec<_> = leftovers_in(&root, &["tm:2-live".into()])
            .into_iter()
            .map(|(p, n)| (p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), n))
            .collect();
        found.sort();
        assert_eq!(found, [("agent-settings/tm_1-dead.json".to_string(), 2), ("scrollback/tm_1-dead.log".to_string(), 5)]);
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn child_env_inherits_scrubs_and_lets_the_config_win() {
        let vars = [
            ("HOME", "/home/u"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("PATH", "/tmp/.mount_X/usr/bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_X/usr/lib"),
            ("APPIMAGE", "/a/Orbit.AppImage"),
            ("APPDIR", "/tmp/.mount_X"),
            ("CLAUDECODE", "1"),
            ("KITTY_WINDOW_ID", "3"),
            ("GHOSTTY_RESOURCES_DIR", "/g"),
            ("TERM", "dumb"),
            ("MODE", "inherited"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let host = vec![
            ("PATH".into(), Some("/usr/bin".into())),
            ("LD_LIBRARY_PATH".into(), None),
            ("APPIMAGE".into(), None),
            ("APPDIR".into(), None),
        ];
        let extra = [("MODE".to_string(), "config".to_string()), ("CACHE".to_string(), "/c".to_string())];
        let env = merge_env(vars.into_iter(), host, &extra, "1.2.3");
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string_lossy().into_owned());
        assert_eq!(get("HOME").as_deref(), Some("/home/u"));
        assert_eq!(get("XDG_RUNTIME_DIR").as_deref(), Some("/run/user/1000"));
        assert_eq!(get("WAYLAND_DISPLAY").as_deref(), Some("wayland-0"));
        assert_eq!(get("PATH").as_deref(), Some("/usr/bin"));
        assert_eq!(get("MODE").as_deref(), Some("config"));
        assert_eq!(get("CACHE").as_deref(), Some("/c"));
        assert_eq!(get("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get("TERM_PROGRAM_VERSION").as_deref(), Some("1.2.3"));
        for gone in ["LD_LIBRARY_PATH", "APPIMAGE", "APPDIR", "CLAUDECODE", "KITTY_WINDOW_ID", "GHOSTTY_RESOURCES_DIR"] {
            assert_eq!(get(gone), None, "{gone}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_tty_only_session_shows_what_another_process_writes_to_its_tty() {
        let (tx, rx) = mpsc::channel();
        let id = reserve_id();
        let spec = Spawn { tty_only: true, size: Size { cols: 80, rows: 24, ..Default::default() }, ..Default::default() };
        spawn(id, spec, move |o| {
            let _ = tx.send(match o {
                Output::Data(d) => Some(d),
                Output::Exit => None,
            });
        })
        .unwrap();
        let tty = tty_path(id).expect("a tty path");
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'from-the-debuggee' > \"$0\"", &tty.to_string_lossy()])
            .status()
            .unwrap();
        assert!(status.success());
        release_tty(id);
        let mut out = Vec::new();
        while let Ok(Some(d)) = rx.recv_timeout(Duration::from_secs(10)) {
            out.extend(d);
        }
        assert!(String::from_utf8_lossy(&out).contains("from-the-debuggee"));
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
