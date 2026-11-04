// Debug adapters (Debug Adapter Protocol): gdb's built-in DAP mode on
// Linux, lldb-dap elsewhere. Like lsp.rs, this only runs the process and
// moves messages between its stdio and the UI (`start`'s callbacks, `send`);
// the protocol lives in the frontend (src/lib/dap).
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

struct Session {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

fn sessions() -> &'static Mutex<HashMap<u32, Session>> {
    static S: OnceLock<Mutex<HashMap<u32, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    /// "gdb" or "lldb-dap": the frontend shapes `launch` for it.
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
}

/// The debugger to use: `requested` ("gdb", "lldb-dap", or a path to
/// either), else gdb on Linux and lldb-dap elsewhere, whichever exists.
pub fn adapter(requested: Option<&str>) -> Result<Adapter, String> {
    let make = |kind: &str, command: String| Adapter {
        kind: kind.to_string(),
        args: if kind == "gdb" { vec!["-q".into(), "-i".into(), "dap".into()] } else { vec![] },
        command,
    };
    if let Some(r) = requested.filter(|r| !r.is_empty() && *r != "auto") {
        let name = PathBuf::from(r).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let kind = if name.starts_with("gdb") { "gdb" } else { "lldb-dap" };
        return Ok(make(kind, r.to_string()));
    }
    let order: &[&str] = if cfg!(target_os = "linux") { &["gdb", "lldb-dap"] } else { &["lldb-dap", "gdb"] };
    order
        .iter()
        .find(|b| crate::proc::exists(b))
        .map(|b| make(b, b.to_string()))
        .ok_or_else(|| {
            "no debugger found: install gdb 14 or newer (it speaks DAP), or LLVM's lldb-dap (winget install LLVM.LLVM, brew install llvm)"
                .to_string()
        })
}

/// Starts the adapter in `cwd`. `on_message` gets the session id and each raw
/// DAP message; `on_exit` the exit code once the adapter is gone.
pub fn start(
    adapter: &Adapter,
    cwd: &str,
    mut on_message: impl FnMut(u32, String) + Send + 'static,
    on_exit: impl FnOnce(u32, Option<i32>) + Send + 'static,
) -> Result<u32, String> {
    let mut child = crate::proc::cmd(&adapter.command)
        .args(&adapter.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", adapter.command))?;
    let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let id = next_id();
    sessions().lock().unwrap().insert(id, Session { child, stdin });
    std::thread::spawn(move || {
        crate::lsp::read_frames(stdout, |message| on_message(id, message));
        let gone = sessions().lock().unwrap().remove(&id);
        let code = gone.and_then(|mut s| s.child.wait().ok()).and_then(|s| s.code());
        on_exit(id, code);
    });
    Ok(id)
}

pub fn send(id: u32, message: &str) -> Result<(), String> {
    let stdin = sessions().lock().unwrap().get(&id).map(|s| s.stdin.clone()).ok_or("the debugger is not running")?;
    crate::lsp::write_frame(&stdin, message)
}

/// Kills the adapter (and with it the program it debugs).
pub fn stop(id: u32) {
    if let Some(mut s) = sessions().lock().unwrap().remove(&id) {
        let _ = s.child.kill();
        // Reaped here: the reader thread no longer finds the session to wait on.
        std::thread::spawn(move || {
            let _ = s.child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_adapter_kind_follows_the_binary_name() {
        assert_eq!(adapter(Some("/opt/gdb-15/bin/gdb")).unwrap().kind, "gdb");
        assert_eq!(adapter(Some("/opt/gdb-15/bin/gdb")).unwrap().args, ["-q", "-i", "dap"]);
        let lldb = adapter(Some(r"C:\LLVM\bin\lldb-dap.exe")).unwrap();
        assert_eq!(lldb.kind, "lldb-dap");
        assert!(lldb.args.is_empty());
    }
}
