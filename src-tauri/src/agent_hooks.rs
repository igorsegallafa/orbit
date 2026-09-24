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
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

static ADDR: OnceLock<(u16, String)> = OnceLock::new();

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub key: String,
    /// Claude hook name: UserPromptSubmit | PostToolUse | Stop | StopFailure | Notification.
    pub event: String,
    pub notification_type: Option<String>,
    pub message: Option<String>,
}

/// Binds the loopback listener. Without it sessions still run, just
/// without hook-driven status.
pub fn start(app: AppHandle) {
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
        message: s("message"),
    })
}

/// Minimal HTTP/1.1: one POST per connection, always answered 200 with an
/// empty body (no hook decision), so a bad request never blocks the agent.
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
    let mut out = stream;
    let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    // Close gracefully: send FIN, then wait for the client to hang up. A bare
    // drop can make Windows reset the connection, and the client reading the
    // response then fails with "connection forcibly closed" (10054).
    let _ = out.shutdown(std::net::Shutdown::Write);
    let _ = out.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = std::io::copy(&mut reader.take(64 * 1024), &mut std::io::sink());
    parse_event(route(&path, token)?, &body)
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
        }
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

/// Status line helper (`Orbit --orbit-statusline <file>`): saves the
/// rate-limit windows Claude Code passes on stdin, then renders the user's
/// own status line from the same input so it looks unchanged.
pub fn statusline_helper(file: Option<String>) {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    if let (Some(file), Ok(v)) = (file, serde_json::from_slice::<Value>(&input)) {
        if let Some(limits) = v.get("rate_limits").filter(|l| l.is_object()) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let tmp = format!("{file}.tmp");
            if std::fs::write(&tmp, json!({ "rateLimits": limits, "updatedAt": now }).to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, &file);
            }
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

#[tauri::command]
pub fn claude_rate_limits() -> RateLimits {
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
        assert_eq!(ev, AgentEvent { key: "tm:9".into(), event: "Stop".into(), notification_type: None, message: None });
        assert!(client.join().unwrap().starts_with("HTTP/1.1 200"));
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
