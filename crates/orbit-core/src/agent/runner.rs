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
    signal_group(child.id(), "-KILL");
    let _ = child.kill();
}

#[cfg(unix)]
fn signal_group(pgid: u32, sig: &str) -> bool {
    crate::proc::cmd("kill")
        .args([sig, "--", &format!("-{pgid}")])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// SIGTERM to the whole process group, SIGKILL to whatever is left after
/// `grace`. Windows has no group signal: `taskkill /T /F` at once.
pub fn terminate_tree(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    {
        let pgid = child.id();
        if signal_group(pgid, "-TERM") {
            let end = Instant::now() + grace;
            // `kill -0` succeeds while any member of the group is alive.
            while Instant::now() < end && signal_group(pgid, "-0") {
                let _ = child.try_wait();
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = grace;
    kill_tree(child);
}

/// Extra stop rules for unattended runs (`run_watched`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Watch {
    /// Kill the tree when no output line arrives for this long.
    pub idle: Option<Duration>,
    /// Kill the tree this long after the run reported it finished
    /// (`on_line`) or was found done (`done`).
    pub exit_grace: Option<Duration>,
}

/// Why a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Exited,
    Idle,
    /// Reported finished but stayed alive past the exit grace.
    ExitGrace,
    Timeout,
}

const TERM_GRACE: Duration = Duration::from_secs(5);
/// How long a watched leader's children may hold the pipes after it exits.
const ORPHAN_GRACE: Duration = Duration::from_secs(2);
/// How often `run_watched` asks `done`.
const DONE_POLL: Duration = Duration::from_secs(3);

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
    pub ended: Ended,
    /// Process id, also the process group id on unix.
    pub pid: u32,
}

/// Spawns `cmd` in `dir`, calling `on_line` for every stdout/stderr line
/// as it arrives. Kills the process tree after `timeout`. When `id` is
/// given the run is cancellable via `cancel(id)`; a previous run with the
/// same id is killed first. Stdin is the caller's to set.
pub fn run_streaming(
    cmd: Command,
    dir: &Path,
    timeout: Duration,
    id: Option<&str>,
    mut on_line: impl FnMut(Line),
) -> Result<Outcome, String> {
    let program = cmd.get_program().to_string_lossy().to_string();
    let o = run_watched(
        cmd,
        dir,
        timeout,
        id,
        Watch::default(),
        |l| {
            on_line(l);
            false
        },
        || false,
    )?;
    if o.ended == Ended::Timeout {
        return Err(format!("{program} timed out after {}s", timeout.as_secs()));
    }
    Ok(o)
}

/// `run_streaming` with `watch`'s stop rules: `on_line` returns true once
/// the run reported it is finished, `done` (asked every few seconds) once
/// its work is found finished, whatever it prints; a timeout is an `Ended`,
/// not an error.
pub fn run_watched(
    mut cmd: Command,
    dir: &Path,
    timeout: Duration,
    id: Option<&str>,
    watch: Watch,
    mut on_line: impl FnMut(Line) -> bool,
    mut done: impl FnMut() -> bool,
) -> Result<Outcome, String> {
    // PWD too: opencode takes its project directory from it, and the one
    // inherited from wherever Orbit was launched would win over current_dir.
    cmd.current_dir(dir)
        .env("PWD", dir)
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
    let pid = child.id();

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

    let watched = watch.idle.is_some() || watch.exit_grace.is_some();
    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut ended = Ended::Exited;
    let mut last_line = Instant::now();
    let mut finished_at: Option<Instant> = None;
    // From `done`: cleared when it stops holding (the agent went back to work).
    let mut found_done_at: Option<Instant> = None;
    let mut polled_at = Instant::now();
    let mut leader_gone: Option<Instant> = None;
    let mut orphans_killed = false;
    // Lines keep flowing until both pipes close (process exit or kill).
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                last_line = Instant::now();
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
                if on_line(line) && finished_at.is_none() {
                    finished_at = Some(Instant::now());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if ended != Ended::Exited {
            continue;
        }
        let now = Instant::now();
        if watch.exit_grace.is_some() && finished_at.is_none() && now - polled_at >= DONE_POLL {
            polled_at = now;
            found_done_at = if done() { found_done_at.or(Some(now)) } else { None };
        }
        let stop = if now > deadline {
            Some(Ended::Timeout)
        } else if watch.idle.is_some_and(|d| now - last_line > d) {
            Some(Ended::Idle)
        } else if finished_at.or(found_done_at).zip(watch.exit_grace).is_some_and(|(t, d)| now - t > d) {
            Some(Ended::ExitGrace)
        } else {
            None
        };
        if let Some(why) = stop {
            ended = why;
            if why == Ended::Timeout {
                kill_tree(&mut child.lock().unwrap());
            } else {
                terminate_tree(&mut child.lock().unwrap(), TERM_GRACE);
            }
            continue;
        }
        // The leader exited but its children still hold the pipes open.
        if watched
            && !orphans_killed
            && child.lock().unwrap().try_wait().ok().flatten().is_some()
            && now - *leader_gone.get_or_insert(now) > ORPHAN_GRACE
        {
            orphans_killed = true;
            terminate_tree(&mut child.lock().unwrap(), TERM_GRACE);
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
    Ok(Outcome {
        success: status.success() && !cancelled && ended == Ended::Exited,
        cancelled,
        stdout,
        stderr,
        ended,
        pid,
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

    #[cfg(unix)]
    fn group_alive(pgid: u32) -> bool {
        signal_group(pgid, "-0")
    }

    #[cfg(unix)]
    #[test]
    fn idle_watch_kills_the_whole_group() {
        let watch = Watch { idle: Some(Duration::from_millis(600)), exit_grace: None };
        let start = Instant::now();
        let o = run_watched(shell("echo hi; sleep 30 & sleep 30"), &std::env::temp_dir(), Duration::from_secs(60), None, watch, |_| false, || false).unwrap();
        assert_eq!(o.ended, Ended::Idle);
        assert!(!o.success);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(!group_alive(o.pid));
    }

    #[cfg(unix)]
    #[test]
    fn exit_grace_kills_a_finished_run_that_stays_alive() {
        let watch = Watch { idle: Some(Duration::from_secs(60)), exit_grace: Some(Duration::from_millis(500)) };
        let o = run_watched(shell("echo done; sleep 30"), &std::env::temp_dir(), Duration::from_secs(60), None, watch, |l| {
            matches!(l, Line::Out(s) if s == "done")
        }, || false)
        .unwrap();
        assert_eq!(o.ended, Ended::ExitGrace);
        assert!(!group_alive(o.pid));
    }

    #[cfg(unix)]
    #[test]
    fn exit_grace_also_starts_when_the_work_is_found_done() {
        let watch = Watch { idle: Some(Duration::from_secs(60)), exit_grace: Some(Duration::from_millis(500)) };
        let marker = std::env::temp_dir().join(format!("orbit-done-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("touch '{}'; sleep 30", marker.display());
        let start = Instant::now();
        let o = run_watched(shell(&script), &std::env::temp_dir(), Duration::from_secs(60), None, watch, |_| false, || marker.exists()).unwrap();
        assert_eq!(o.ended, Ended::ExitGrace);
        assert!(start.elapsed() < Duration::from_secs(15), "{:?}", start.elapsed());
        assert!(!group_alive(o.pid));
        let _ = std::fs::remove_file(&marker);
    }

    #[cfg(unix)]
    #[test]
    fn watched_run_reaps_children_left_holding_the_pipes() {
        let watch = Watch { idle: Some(Duration::from_secs(60)), exit_grace: None };
        let start = Instant::now();
        let o = run_watched(shell("sleep 30 & echo bye"), &std::env::temp_dir(), Duration::from_secs(60), None, watch, |_| false, || false).unwrap();
        assert_eq!(o.ended, Ended::Exited);
        assert!(o.success);
        assert!(start.elapsed() < Duration::from_secs(15));
        assert!(!group_alive(o.pid));
    }
}
