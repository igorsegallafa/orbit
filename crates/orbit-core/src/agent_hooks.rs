// Live signals from the Claude Code sessions Orbit launches in its terminals.
// Each session gets a `--settings` file whose HTTP hooks (prompt submitted,
// tool finished, turn finished, needs attention) post to a loopback listener,
// keyed by the terminal tab id; the listener re-emits them as `agent-event`.
// The same file routes the status line through Orbit (`--orbit-statusline`)
// so subscription rate limits get captured without replacing the user's own.
use serde::Serialize;
use serde_json::{json, Value};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use crate::events::Events;

static ADDR: OnceLock<(u16, String)> = OnceLock::new();

const PERMISSION_WAIT: Duration = Duration::from_secs(600);

type Asker = Arc<dyn Fn(String, String, Value) + Send + Sync>;
/// Per session key: who to ask when its agent wants a permission (id, tool name, tool input).
static ASKERS: Mutex<Option<HashMap<String, Asker>>> = Mutex::new(None);
/// Open permission requests: id → (session key, where the answer goes).
static PENDING: Mutex<Option<HashMap<String, Pending>>> = Mutex::new(None);
type Pending = (String, Sender<String>);

type Viewer = Arc<dyn Fn(String, Option<String>) + Send + Sync>;
/// Per session key: where its ShowImage pictures go (path, title) instead of `agent-event`.
static VIEWERS: Mutex<Option<HashMap<String, Viewer>>> = Mutex::new(None);

pub fn on_show_image(key: &str, show: impl Fn(String, Option<String>) + Send + Sync + 'static) {
    VIEWERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key.into(), Arc::new(show));
}

/// Routes the requests of `agent_mcp`'s permission tool for session `key` to `ask`.
pub fn on_permission(key: &str, ask: impl Fn(String, String, Value) + Send + Sync + 'static) {
    ASKERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key.into(), Arc::new(ask));
}

/// Stops routing `key`'s permission requests and pictures; its open requests are denied.
pub fn forget(key: &str) {
    if let Some(a) = ASKERS.lock().unwrap().as_mut() {
        a.remove(key);
    }
    if let Some(v) = VIEWERS.lock().unwrap().as_mut() {
        v.remove(key);
    }
    if let Some(p) = PENDING.lock().unwrap().as_mut() {
        p.retain(|_, (k, _)| k != key);
    }
}

/// Answers an open permission request with the chosen option id (`allow` allows, anything else denies).
pub fn answer_permission(id: &str, option_id: &str) -> bool {
    let tx = PENDING.lock().unwrap().as_mut().and_then(|p| p.remove(id));
    tx.is_some_and(|(_, tx)| tx.send(option_id.into()).is_ok())
}

/// Blocks until the session answers a PermissionRequest body; `deny` when nobody asks or answers.
fn ask_permission(key: &str, body: &Value) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let Some(ask) = ASKERS.lock().unwrap().as_ref().and_then(|a| a.get(key).cloned()) else { return "deny".into() };
    let id = format!("perm-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = channel();
    PENDING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), (key.into(), tx));
    ask(id.clone(), body["tool_name"].as_str().unwrap_or("").into(), body["input"].clone());
    let answer = rx.recv_timeout(PERMISSION_WAIT).unwrap_or_else(|_| "deny".into());
    if let Some(p) = PENDING.lock().unwrap().as_mut() {
        p.remove(&id);
    }
    answer
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub key: String,
    /// Claude hook name: UserPromptSubmit | PostToolUse | Stop | StopFailure | Notification.
    pub event: String,
    pub notification_type: Option<String>,
    pub message: Option<String>,
    /// ShowImage (from `agent_mcp`): the picture to show.
    pub path: Option<String>,
}

/// Binds the loopback listener. Without it sessions still run, just
/// without hook-driven status.
pub fn start(app: Events) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else { return };
    let Ok(port) = listener.local_addr().map(|a| a.port()) else { return };
    let token = random_token();
    if ADDR.set((port, token.clone())).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                if let Some(ev) = read_event(stream, &token) {
                    let _ = app.emit("agent-event", ev);
                }
            });
        }
    });
}

/// Unguessable path segment so other local processes can't forge events.
fn random_token() -> String {
    let s = std::collections::hash_map::RandomState::new();
    let (mut a, mut b) = (s.build_hasher(), s.build_hasher());
    a.write_u64(std::process::id() as u64);
    b.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("{:016x}{:016x}", a.finish(), b.finish())
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 80 && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
}

/// `/hook/<token>/<key>` → key.
fn route<'a>(path: &'a str, token: &str) -> Option<&'a str> {
    let (t, key) = path.strip_prefix("/hook/")?.split_once('/')?;
    (t == token && valid_key(key)).then_some(key)
}

fn parse_event(key: &str, body: &[u8]) -> Option<AgentEvent> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(AgentEvent {
        key: key.to_string(),
        event: s("hook_event_name")?,
        notification_type: s("notification_type"),
        // UserPromptSubmit carries the prompt instead.
        message: s("message").or_else(|| s("prompt")),
        path: s("path"),
    })
}

/// Minimal HTTP/1.1: one POST per connection, always answered 200 with an
/// empty body (no hook decision), so a bad request never blocks the agent.
/// A PermissionRequest is the exception: held open until answered, the
/// answer is the body, and it is not re-emitted as an event.
fn read_event(stream: TcpStream, token: &str) -> Option<AgentEvent> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len.min(1 << 20)];
    reader.read_exact(&mut body).ok()?;
    let key = route(&path, token);
    let request = serde_json::from_slice::<Value>(&body).ok().filter(|v| v["hook_event_name"] == "PermissionRequest");
    let answer = match (key, &request) {
        (Some(key), Some(v)) => ask_permission(key, v),
        _ => String::new(),
    };
    let mut out = stream;
    let _ = write!(out, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
    // Close gracefully: send FIN, then wait for the client to hang up. A bare
    // drop can make Windows reset the connection, and the client reading the
    // response then fails with "connection forcibly closed" (10054).
    let _ = out.shutdown(std::net::Shutdown::Write);
    let _ = out.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = std::io::copy(&mut reader.take(64 * 1024), &mut std::io::sink());
    if request.is_some() {
        return None;
    }
    let ev = parse_event(key?, &body)?;
    let viewer = VIEWERS.lock().unwrap().as_ref().and_then(|v| v.get(&ev.key).cloned());
    match (ev.event.as_str(), viewer, ev.path.clone()) {
        ("ShowImage", Some(show), Some(path)) => {
            show(path, ev.message);
            None
        }
        _ => Some(ev),
    }
}

/// Settings JSON for one session: hooks posting here plus the status line
/// passthrough. `None` when the listener isn't up.
fn session_settings(key: &str, statusline: Option<String>) -> Option<Value> {
    let (port, token) = ADDR.get()?;
    let hook = json!([{ "hooks": [{ "type": "http", "url": format!("http://127.0.0.1:{port}/hook/{token}/{key}"), "timeout": 5 }] }]);
    let mut v = json!({
        "hooks": {
            "UserPromptSubmit": hook,
            "PostToolUse": hook,
            "Stop": hook,
            "StopFailure": hook,
            "Notification": hook,
        },
        // Orbit's own MCP tool only shows a picture; never ask for it.
        "permissions": { "allow": ["mcp__orbit__show_image"] }
    });
    if let Some(cmd) = statusline {
        v["statusLine"] = json!({ "type": "command", "command": cmd });
    }
    Some(v)
}

/// Writes the session's settings file and returns the `--settings <file>`
/// argv to prepend to a claude launch (empty when unavailable).
pub fn claude_args(key: &str) -> Vec<String> {
    if !valid_key(key) {
        return vec![];
    }
    let Some(settings) = session_settings(key, statusline_command()) else { return vec![] };
    let Ok(dir) = crate::config::config_dir().map(|d| d.join("agent-settings")) else { return vec![] };
    let file = dir.join(format!("{}.json", key.replace(':', "_")));
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, settings.to_string()).is_err() {
        return vec![];
    }
    vec!["--settings".into(), file.to_string_lossy().into_owned()]
}

/// The session's hook route; `agent_mcp` posts its pictures there.
pub(crate) fn hook_url(key: &str) -> Option<String> {
    let (port, token) = ADDR.get()?;
    valid_key(key).then(|| format!("http://127.0.0.1:{port}/hook/{token}/{key}"))
}

/// Appended to agent sessions' system prompt; `ai.reply_style` replaces it, empty turns it off.
pub const REPLY_STYLE: &str = "\
Orbit shows your replies in a terminal pane. Write them for a fast read:
- Lead with the answer or result in one or two sentences. No preamble, no recap of the request.
- Prefer short bullets or a small table to paragraphs; one idea per bullet.
- Don't narrate what you are about to do or repeat output the user already saw.
- Write links as full URLs (they are clickable) and files as path:line.
- If something was skipped or is risky, end with it in one line.
- To show the user an image (a screenshot, chart, rendered UI), call Orbit's show_image tool with its path.";

/// Added in chat mode, where Orbit renders the reply as markdown.
pub const CHAT_STYLE: &str = "\
Orbit renders your replies as markdown in a chat view. Structure them when it helps the reader:
- Comparisons, options or results per item: a markdown table.
- A multi-part answer: short `###` headings. Steps to follow: a numbered list; a checklist: `- [ ]` items.
- Code, commands and output: fenced blocks with a language. A warning or caveat: a `> **Note:**` quote.
- A short answer stays plain text: no structure for its own sake.";

fn reply_style(chat: bool) -> Option<String> {
    let custom = crate::config::Config::load().ok().and_then(|c| c.ai.reply_style);
    let style = Some(custom.unwrap_or_else(|| REPLY_STYLE.into())).filter(|s| !s.trim().is_empty())?;
    Some(if chat { format!("{style}\n{CHAT_STYLE}") } else { style })
}

/// The repos in `cwd` (or `cwd` itself) that are links to a base clone.
fn link_note(cwd: &Path) -> Option<String> {
    let entries = std::fs::read_dir(cwd).into_iter().flatten().flatten().map(|e| e.path());
    let links: Vec<String> = std::iter::once(cwd.to_path_buf())
        .chain(entries)
        .filter(|p| crate::links::is_link(p) && p.is_dir())
        .map(|p| format!("- `{}` -> `{}`", p.display(), crate::lsp::real_path(&p).display()))
        .collect();
    (!links.is_empty()).then(|| {
        format!(
            "These repo folders are links to a base clone shared with other workspaces:\n{}\nRun builds and CMake configures from the real path on the right (e.g. `cd -P`), never through the link: CMake records the path it was configured from, and a different one makes the next build recompile everything.",
            links.join("\n")
        )
    })
}

/// Argv and environment giving an agent session Orbit's MCP server and reply style (and claude its hooks).
// shortcut: omp only reads MCP servers from files (no flag or env), so it gets the style alone.
pub fn session_launch(cmd: &str, key: &str, chat: bool, cwd: &Path) -> (Vec<String>, Vec<(String, String)>) {
    let name = std::path::Path::new(cmd.trim()).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let style = [reply_style(chat), link_note(cwd)].into_iter().flatten().reduce(|a, b| format!("{a}\n\n{b}"));
    let server = hook_url(key).zip(std::env::current_exe().ok()).map(|(url, exe)| (exe.to_string_lossy().into_owned(), url));
    let mut args: Vec<String> = vec![];
    let mut env = vec![];
    match name.as_str() {
        "claude" => {
            args = claude_args(key);
            if let Some((exe, url)) = &server {
                // `=`: --mcp-config is variadic and would swallow a prompt after it.
                args.push(format!("--mcp-config={}", json!({ "mcpServers": { "orbit": { "command": exe, "args": ["--orbit-mcp", url] } } })));
            }
            args.extend(style.map(|s| format!("--append-system-prompt={s}")));
        }
        "codex" => {
            // -c values are TOML; a JSON string or array of strings is valid TOML.
            if let Some((exe, url)) = &server {
                args.extend(["-c".into(), format!("mcp_servers.orbit.command={}", json!(exe))]);
                args.extend(["-c".into(), format!("mcp_servers.orbit.args={}", json!(["--orbit-mcp", url]))]);
                args.extend(["-c".into(), r#"mcp_servers.orbit.default_tools_approval_mode="approve""#.into()]);
            }
            if let Some(s) = style {
                args.extend(["-c".into(), format!("developer_instructions={}", json!(s))]);
            }
        }
        "opencode" => {
            let mut config = json!({});
            if let Some((exe, url)) = &server {
                config["mcp"] = json!({ "orbit": { "type": "local", "command": [exe, "--orbit-mcp", url] } });
            }
            let file = crate::config::config_dir().map(|d| d.join(if chat { "reply-style-chat.md" } else { "reply-style.md" }));
            if let (Some(s), Ok(file)) = (style, file) {
                if std::fs::write(&file, s).is_ok() {
                    config["instructions"] = json!([file]);
                }
            }
            env.push(("OPENCODE_CONFIG_CONTENT".into(), config.to_string()));
        }
        "omp" => args.extend(style.map(|s| format!("--append-system-prompt={s}"))),
        _ => {}
    }
    (args, env)
}

// ---------- status line passthrough + rate limits ----------

fn rate_limits_file() -> Result<PathBuf, String> {
    Ok(crate::config::config_dir()?.join("claude-rate-limits.json"))
}

/// Forward slashes work for both Git Bash and PowerShell; quotes only when
/// needed since PowerShell can't run a quoted path without `&`.
// ponytail: a path with spaces only works under Git Bash; switch to a
// launcher script if Orbit ever installs under such a path on Windows.
fn shell_arg(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

fn statusline_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let file = rate_limits_file().ok()?;
    Some(format!("{} --orbit-statusline {}", shell_arg(&exe), shell_arg(&file)))
}

fn write_rate_limits(file: &std::path::Path, limits: &Value) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, json!({ "rateLimits": limits, "updatedAt": now }).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

/// Saves the windows of a chat's `rate_limit_event` (`unifiedWindows`, utilization 0–1) in the
/// status line's format, so the usage bar shows them too.
pub fn save_rate_limits(windows: &Value) {
    let Ok(file) = rate_limits_file() else { return };
    let mut limits = serde_json::Map::new();
    for (name, w) in windows.as_object().into_iter().flatten() {
        if let (Some(u), Some(at)) = (w["utilization"].as_f64(), w["resetsAt"].as_u64()) {
            limits.insert(name.clone(), json!({ "used_percentage": u * 100., "resets_at": at }));
        }
    }
    if !limits.is_empty() {
        write_rate_limits(&file, &Value::Object(limits));
    }
}

/// Status line helper (`Orbit --orbit-statusline <file>`): saves the
/// rate-limit windows Claude Code passes on stdin, then renders the user's
/// own status line from the same input so it looks unchanged.
pub fn statusline_helper(file: Option<String>) {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    if let (Some(file), Ok(v)) = (file, serde_json::from_slice::<Value>(&input)) {
        if let Some(limits) = v.get("rate_limits").filter(|l| l.is_object()) {
            write_rate_limits(std::path::Path::new(&file), limits);
        }
    }
    if let Some(cmd) = user_statusline() {
        if let Some(out) = run_shell(&cmd, &input) {
            let _ = std::io::stdout().write_all(&out);
        }
    }
}

fn user_statusline() -> Option<String> {
    let raw = std::fs::read_to_string(crate::config::home_dir().ok()?.join(".claude/settings.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let cmd = v.get("statusLine")?.get("command")?.as_str()?.trim().to_string();
    (!cmd.is_empty() && !cmd.contains("--orbit-statusline")).then_some(cmd)
}

/// Same shell Claude Code uses: Git Bash on Windows when present, else PowerShell.
fn run_shell(cmd: &str, input: &[u8]) -> Option<Vec<u8>> {
    use std::process::Stdio;
    let spawn = |prog: &str, args: &[&str]| {
        crate::proc::cmd(prog).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
    };
    let child = if cfg!(windows) {
        let bash = std::env::var("CLAUDE_CODE_GIT_BASH_PATH").unwrap_or_else(|_| "bash".into());
        spawn(&bash, &["-c", cmd]).or_else(|_| spawn("powershell", &["-NoProfile", "-Command", cmd]))
    } else {
        spawn("sh", &["-c", cmd])
    };
    let mut child = child.ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().ok().map(|o| o.stdout)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    pub used_percentage: f64,
    pub resets_at: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub updated_at: Option<u64>,
}

/// Last captured windows; ones past their reset are dropped, like Claude does.
fn parse_rate_limits(raw: &str, now: u64) -> RateLimits {
    let Ok(v) = serde_json::from_str::<Value>(raw) else { return RateLimits::default() };
    let window = |k: &str| {
        let w = v.get("rateLimits")?.get(k)?;
        let resets_at = w.get("resets_at")?.as_u64()?;
        let used_percentage = w.get("used_percentage")?.as_f64()?;
        (resets_at > now).then_some(RateWindow { used_percentage, resets_at })
    };
    RateLimits { five_hour: window("five_hour"), seven_day: window("seven_day"), updated_at: v.get("updatedAt").and_then(Value::as_u64) }
}

/// Claude's rate-limit windows, as the status line helper last saw them.
pub fn rate_limits() -> RateLimits {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    rate_limits_file()
        .ok()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|raw| parse_rate_limits(&raw, now))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_with_the_token_and_a_safe_key() {
        assert_eq!(route("/hook/abc/tm:1-x_y", "abc"), Some("tm:1-x_y"));
        assert_eq!(route("/hook/nope/tm:1", "abc"), None);
        assert_eq!(route("/hook/abc/../etc", "abc"), None);
        assert_eq!(route("/other/abc/tm:1", "abc"), None);
    }

    #[test]
    fn parses_hook_payloads() {
        let ev = parse_event("k", br#"{"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}"#).unwrap();
        assert_eq!(ev.event, "Notification");
        assert_eq!(ev.notification_type.as_deref(), Some("permission_prompt"));
        assert!(parse_event("k", b"not json").is_none());
    }

    #[test]
    fn serves_a_real_hook_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body = r#"{"hook_event_name":"Stop"}"#;
            write!(s, "POST /hook/tok/tm:9 HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            resp
        });
        let (stream, _) = listener.accept().unwrap();
        let ev = read_event(stream, "tok").unwrap();
        assert_eq!(ev, AgentEvent { key: "tm:9".into(), event: "Stop".into(), notification_type: None, message: None, path: None });
        assert!(client.join().unwrap().starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn a_held_permission_request_gets_the_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/hook/tok/tm:perm", listener.local_addr().unwrap().port());
        let (tx, rx) = channel();
        on_permission("tm:perm", move |id, tool, input| tx.send((id, tool, input)).unwrap());
        let client = std::thread::spawn(move || crate::agent_mcp::permission(&json!({"tool_name": "Bash", "input": {"command": "ls"}}), Some(&url)));
        let (stream, _) = listener.accept().unwrap();
        let server = std::thread::spawn(move || read_event(stream, "tok"));
        let (id, tool, input) = rx.recv().unwrap();
        assert_eq!((tool.as_str(), &input), ("Bash", &json!({"command": "ls"})));
        assert!(answer_permission(&id, "allow"));
        assert!(server.join().unwrap().is_none());
        let reply: Value = serde_json::from_str(&client.join().unwrap()).unwrap();
        assert_eq!(reply, json!({"behavior": "allow", "updatedInput": {"command": "ls"}}));
        assert!(!answer_permission(&id, "allow"));
        forget("tm:perm");
    }

    #[test]
    fn a_routed_session_gets_its_pictures_instead_of_an_event() {
        let post = |key: &str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://127.0.0.1:{}/hook/tok/{key}", listener.local_addr().unwrap().port());
            let body = json!({"hook_event_name": "ShowImage", "path": "/a.png", "message": "A"}).to_string();
            let client = std::thread::spawn(move || crate::agent_mcp::post(&url, &body, Duration::from_secs(5)));
            let (stream, _) = listener.accept().unwrap();
            let ev = read_event(stream, "tok");
            client.join().unwrap().unwrap();
            ev
        };
        let (tx, rx) = channel();
        on_show_image("tm:img", move |path, title| tx.send((path, title)).unwrap());
        assert!(post("tm:img").is_none());
        assert_eq!(rx.recv().unwrap(), ("/a.png".into(), Some("A".into())));
        forget("tm:img");
        assert_eq!(post("tm:img").unwrap().path.as_deref(), Some("/a.png"));
    }

    #[test]
    fn wires_the_reply_style_into_each_agent() {
        let has = |a: &[String], p: &str| a.iter().any(|x| x.starts_with(p));
        assert!(has(&session_launch("omp", "tm:1", false, Path::new(".")).0, "--append-system-prompt="));
        assert!(has(&session_launch("/usr/bin/codex", "tm:1", false, Path::new(".")).0, "developer_instructions="));
        assert_eq!(session_launch("opencode", "tm:1", false, Path::new(".")).1[0].0, "OPENCODE_CONFIG_CONTENT");
        assert_eq!(session_launch("bash", "tm:1", false, Path::new(".")), (vec![], vec![]));
        let chat = session_launch("claude", "tm:1", true, Path::new(".")).0;
        assert!(chat.iter().any(|a| a.starts_with("--append-system-prompt=") && a.contains("markdown table")));
    }

    #[cfg(unix)]
    #[test]
    fn notes_repos_that_link_to_a_base_clone() {
        let d = std::env::temp_dir().join(format!("orbit-link-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("clone")).unwrap();
        std::fs::create_dir_all(d.join("ws/own")).unwrap();
        std::os::unix::fs::symlink(d.join("clone"), d.join("ws/engine")).unwrap();
        let note = link_note(&d.join("ws")).unwrap();
        assert!(note.contains("ws/engine` -> `") && note.contains("clone`") && !note.contains("own"));
        assert!(link_note(&d.join("ws/engine")).is_some());
        assert!(link_note(&d.join("ws/own")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_only_windows_that_have_not_reset() {
        let raw = r#"{"rateLimits":{"five_hour":{"used_percentage":23.5,"resets_at":2000},"seven_day":{"used_percentage":41.2,"resets_at":500}},"updatedAt":900}"#;
        let r = parse_rate_limits(raw, 1000);
        assert_eq!(r.five_hour.unwrap().used_percentage, 23.5);
        assert!(r.seven_day.is_none());
        assert_eq!(r.updated_at, Some(900));
    }
}
