// Spawning external CLIs (git, gh, claude, editors) portably.
use std::path::PathBuf;
use std::process::Command;

/// `Command::new(bin)` that also works on Windows for npm/editor shims
/// (`code.cmd`, `opencode.cmd`), which std only finds when given a full
/// path, and never flashes a console window from the GUI app.
/// Markers of the Claude Code session Orbit itself may have been started
/// from (e.g. `tauri dev` in a Claude terminal). Inherited, they make every
/// agent Orbit launches think it's a nested child: no transcripts, so no
/// `--resume`. User config vars (CLAUDE_CODE_GIT_BASH_PATH, ...) stay.
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
];

pub fn cmd(bin: &str) -> Command {
    let mut c = Command::new(resolve(bin));
    for k in CLAUDE_SESSION_ENV {
        c.env_remove(k);
    }
    if bin == "git" {
        git_env(&mut c);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Git run from a GUI must never wait on a terminal prompt, and should
/// authenticate to GitHub with the user's `gh` login even when
/// `gh auth setup-git` was never run (an extra helper; existing ones still
/// apply). Env-based config (git >= 2.31) keeps every call site unchanged.
fn git_env(c: &mut Command) {
    c.env("GIT_TERMINAL_PROMPT", "0");
    if exists("gh") {
        c.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "credential.https://github.com.helper")
            .env("GIT_CONFIG_VALUE_0", "!gh auth git-credential");
    }
}

/// Runs `bin args` (in `dir` when given) and returns stdout; a non-zero
/// exit becomes an error carrying stderr.
pub fn run(bin: &str, args: &[&str], dir: Option<&std::path::Path>) -> Result<String, String> {
    let mut c = cmd(bin);
    c.args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    let out = c.output().map_err(|e| format!("failed to run {bin}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Absolute path of `bin` found on PATH (honouring PATHEXT on Windows),
/// or `bin` unchanged so the spawn error names what's missing.
pub fn resolve(bin: &str) -> PathBuf {
    find_in(bin, std::env::var_os("PATH"), &exts()).unwrap_or_else(|| PathBuf::from(bin))
}

/// True when `bin` is on PATH.
pub fn exists(bin: &str) -> bool {
    find_in(bin, std::env::var_os("PATH"), &exts()).is_some()
}

/// Windows: a GUI app inherits PATH from whatever launched it, which can
/// predate a git/gh install (a terminal opened before it, the installer, the
/// relaunch after an update), so the tools look missing. Rebuilds PATH from
/// the registry like a fresh login (machine, then user), keeping entries
/// only this process has. Call once at startup, before spawning threads.
#[cfg(windows)]
pub fn refresh_path() {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let read = |root, key: &str| RegKey::predef(root).open_subkey(key).ok()?.get_value::<String, _>("Path").ok();
    let registry = [
        read(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        read(HKEY_CURRENT_USER, "Environment"),
    ];
    let mut dirs: Vec<PathBuf> = registry
        .iter()
        .flatten()
        .flat_map(|p| std::env::split_paths(&expand_env(p)).collect::<Vec<_>>())
        .collect();
    if let Some(current) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&current) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs.retain(|d| !d.as_os_str().is_empty());
    if let Ok(joined) = std::env::join_paths(dirs) {
        std::env::set_var("PATH", joined);
    }
}

/// Expands `%NAME%` references (REG_EXPAND_SZ values like
/// `%SystemRoot%\system32`); unknown names stay as written.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&format!("%{name}%")),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn exts() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    }
}

fn find_in(bin: &str, path: Option<std::ffi::OsString>, exts: &[String]) -> Option<PathBuf> {
    if bin.contains('/') || bin.contains('\\') {
        return None;
    }
    std::env::split_paths(&path?).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{bin}{ext}")))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_known_env_vars_and_keeps_the_rest() {
        std::env::set_var("ORBIT_TEST_ROOT", r"C:\Win");
        assert_eq!(expand_env(r"%ORBIT_TEST_ROOT%\system32;C:\x"), r"C:\Win\system32;C:\x");
        assert_eq!(expand_env("%ORBIT_NOPE%\\bin"), "%ORBIT_NOPE%\\bin");
        assert_eq!(expand_env("50% off"), "50% off");
    }

    #[test]
    fn finds_shim_by_extension_and_skips_extensionless_file() {
        let dir = std::env::temp_dir().join(format!("orbit-proc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tool"), "").unwrap();
        std::fs::write(dir.join("tool.cmd"), "").unwrap();
        let path = Some(dir.clone().into_os_string());
        let exts = vec![".exe".to_string(), ".cmd".to_string()];
        assert_eq!(find_in("tool", path.clone(), &exts), Some(dir.join("tool.cmd")));
        assert_eq!(find_in("missing", path, &exts), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
