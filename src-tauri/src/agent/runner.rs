// Runs agent processes headless: streaming lines with a real deadline and
// a registry of cancellable runs keyed by id.
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type Shared = Arc<Mutex<Child>>;

fn runs() -> &'static Mutex<HashMap<String, Shared>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Shared>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kills the run registered under `id`. Returns false when none is running.
pub fn cancel(id: &str) -> bool {
    match runs().lock().unwrap().remove(id) {
        Some(child) => {
            kill_tree(&mut child.lock().unwrap());
            true
        }
        None => false,
    }
}

#[cfg(test)]
pub fn is_running(id: &str) -> bool {
    runs().lock().unwrap().contains_key(id)
}

/// Kills the process and its children (agents spawn shells, builds, tests).
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = crate::proc::cmd("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .output();
    }
    // run_streaming puts the child in its own process group (pgid = pid).
    #[cfg(unix)]
    {
        let _ = crate::proc::cmd("kill")
            .args(["-9", &format!("-{}", child.id())])
            .output();
    }
    let _ = child.kill();
}

pub enum Line {
    Out(String),
    Err(String),
}

pub struct Outcome {
    pub success: bool,
    /// true when stopped through `cancel`.
    pub cancelled: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Spawns `cmd` in `dir`, calling `on_line` for every stdout/stderr line
/// as it arrives. Kills the process tree after `timeout`. When `id` is
/// given the run is cancellable via `cancel(id)`; a previous run with the
/// same id is killed first.
pub fn run_streaming(
    mut cmd: Command,
    dir: &Path,
    timeout: Duration,
    id: Option<&str>,
    mut on_line: impl FnMut(Line),
) -> Result<Outcome, String> {
    // PWD too: opencode takes its project directory from it, and the one
    // inherited from wherever Orbit was launched would win over current_dir.
    cmd.current_dir(dir)
        .env("PWD", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let program = cmd.get_program().to_string_lossy().to_string();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch {program}: {e}"))?;

    let (tx, rx) = mpsc::channel::<Line>();
    let out = child.stdout.take().ok_or("no stdout from process")?;
    let err = child.stderr.take().ok_or("no stderr from process")?;
    let tx_err = tx.clone();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(Line::Out(l)).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            if tx_err.send(Line::Err(l)).is_err() {
                break;
            }
        }
    });

    let child: Shared = Arc::new(Mutex::new(child));
    if let Some(id) = id {
        if let Some(old) = runs().lock().unwrap().insert(id.to_string(), child.clone()) {
            kill_tree(&mut old.lock().unwrap());
        }
    }

    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut timed_out = false;
    // Lines keep flowing until both pipes close (process exit or kill).
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                match &line {
                    Line::Out(l) => {
                        stdout.push_str(l);
                        stdout.push('\n');
                    }
                    Line::Err(l) => {
                        stderr.push_str(l);
                        stderr.push('\n');
                    }
                }
                on_line(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !timed_out && Instant::now() > deadline {
            timed_out = true;
            kill_tree(&mut child.lock().unwrap());
        }
    }
    let status = child.lock().unwrap().wait().map_err(|e| format!("wait failed: {e}"))?;
    let cancelled = match id {
        // cancel() removes the entry; still present means we finished normally.
        Some(id) => {
            let mut map = runs().lock().unwrap();
            let ours = map.get(id).is_some_and(|c| Arc::ptr_eq(c, &child));
            if ours {
                map.remove(id);
            }
            !ours
        }
        None => false,
    };
    if timed_out {
        return Err(format!("{program} timed out after {}s", timeout.as_secs()));
    }
    Ok(Outcome {
        success: status.success() && !cancelled,
        cancelled,
        stdout,
        stderr,
    })
}

/// Runs to completion and returns trimmed stdout; stderr becomes the error.
pub fn run_capture(cmd: Command, dir: &Path, timeout: Duration) -> Result<String, String> {
    let o = run_streaming(cmd, dir, timeout, None, |_| {})?;
    if !o.success {
        let err = o.stderr.trim();
        return Err(if err.is_empty() { "process exited with an error".into() } else { err.to_string() });
    }
    Ok(o.stdout.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        if cfg!(windows) {
            let mut c = crate::proc::cmd("cmd");
            c.args(["/C", script]);
            c
        } else {
            let mut c = crate::proc::cmd("sh");
            c.args(["-c", script]);
            c
        }
    }

    #[test]
    fn streams_lines_and_captures_output() {
        let mut seen = Vec::new();
        let o = run_streaming(shell("echo one&& echo two"), &std::env::temp_dir(), Duration::from_secs(10), None, |l| {
            if let Line::Out(s) = l {
                seen.push(s.trim().to_string())
            }
        })
        .unwrap();
        assert!(o.success);
        assert_eq!(seen, ["one", "two"]);
    }

    #[test]
    fn times_out_and_kills_a_hung_process() {
        let script = if cfg!(windows) { "ping -n 30 127.0.0.1 >NUL" } else { "sleep 30" };
        let start = Instant::now();
        let r = run_streaming(shell(script), &std::env::temp_dir(), Duration::from_secs(1), None, |_| {});
        assert!(r.is_err());
        assert!(start.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn cancel_stops_a_registered_run() {
        let script = if cfg!(windows) { "ping -n 30 127.0.0.1 >NUL" } else { "sleep 30" };
        let id = format!("test-cancel-{}", std::process::id());
        let id2 = id.clone();
        let h = std::thread::spawn(move || {
            run_streaming(shell(script), &std::env::temp_dir(), Duration::from_secs(60), Some(&id2), |_| {})
        });
        while !is_running(&id) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(cancel(&id));
        let o = h.join().unwrap().unwrap();
        assert!(o.cancelled && !o.success);
    }
}
