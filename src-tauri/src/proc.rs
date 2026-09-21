// Spawning external CLIs (git, gh, claude, editors) portably.
use std::path::PathBuf;
use std::process::Command;

/// `Command::new(bin)` that also works on Windows for npm/editor shims
/// (`code.cmd`, `opencode.cmd`), which std only finds when given a full
/// path, and never flashes a console window from the GUI app.
pub fn cmd(bin: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(resolve(bin));
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
