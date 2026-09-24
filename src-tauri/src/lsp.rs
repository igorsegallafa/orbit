// Language servers for the editor: one process per (folder, language) —
// clangd, rust-analyzer, typescript-language-server… — started on demand.
// This module only manages the processes and moves JSON-RPC messages between
// their stdio and the webview (`lsp-message` events, `lsp_send`); the
// protocol itself lives in the frontend client (src/lib/lsp).
use crate::config::{Config, LanguageServerSetting};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

pub struct LanguageDef {
    /// Key in the config and in the frontend ("cpp").
    pub id: &'static str,
    pub name: &'static str,
    /// Servers in order of preference: the first one installed wins.
    pub servers: &'static [ServerDef],
    /// Root entries telling a repo uses the language: file names, or "*.ext".
    pub markers: &'static [&'static str],
}

pub struct ServerDef {
    pub bin: &'static str,
    pub args: &'static [&'static str],
    /// How to install it, shown when it's missing.
    pub install: &'static str,
}

pub const LANGUAGES: &[LanguageDef] = &[
    LanguageDef {
        id: "cpp",
        name: "C / C++",
        servers: &[ServerDef {
            bin: "clangd",
            args: &["--background-index", "--header-insertion=never"],
            install: "Install LLVM (winget install LLVM.LLVM, brew install llvm, apt install clangd) or VS's C++ Clang tools",
        }],
        markers: &["CMakeLists.txt", "meson.build", "compile_commands.json", "*.vcxproj", "*.cpp", "*.c", "*.h"],
    },
    LanguageDef {
        id: "rust",
        name: "Rust",
        servers: &[ServerDef { bin: "rust-analyzer", args: &[], install: "rustup component add rust-analyzer" }],
        markers: &["Cargo.toml"],
    },
    LanguageDef {
        id: "typescript",
        name: "TypeScript / JavaScript",
        servers: &[ServerDef {
            bin: "typescript-language-server",
            args: &["--stdio"],
            install: "npm install -g typescript typescript-language-server",
        }],
        markers: &["tsconfig.json", "jsconfig.json", "package.json"],
    },
    LanguageDef {
        id: "python",
        name: "Python",
        servers: &[
            ServerDef { bin: "pyright-langserver", args: &["--stdio"], install: "npm install -g pyright" },
            ServerDef { bin: "pylsp", args: &[], install: "pip install python-lsp-server" },
        ],
        markers: &["pyproject.toml", "setup.py", "requirements.txt", "Pipfile"],
    },
    LanguageDef {
        id: "go",
        name: "Go",
        servers: &[ServerDef { bin: "gopls", args: &[], install: "go install golang.org/x/tools/gopls@latest" }],
        markers: &["go.mod"],
    },
    LanguageDef {
        id: "csharp",
        name: "C#",
        servers: &[ServerDef { bin: "csharp-ls", args: &[], install: "dotnet tool install --global csharp-ls" }],
        markers: &["*.sln", "*.csproj"],
    },
    LanguageDef {
        id: "lua",
        name: "Lua",
        servers: &[ServerDef {
            bin: "lua-language-server",
            args: &[],
            install: "https://luals.github.io/#install (winget install LuaLS.lua-language-server)",
        }],
        markers: &[".luarc.json", "*.lua"],
    },
];

pub fn language(id: &str) -> Option<&'static LanguageDef> {
    LANGUAGES.iter().find(|l| l.id == id)
}

/// Where a server binary is: on PATH, or (clangd on Windows) where the LLVM
/// installer or Visual Studio's "C++ Clang tools" put it without touching PATH.
fn locate(bin: &str) -> Option<String> {
    if crate::proc::exists(bin) {
        // rustup puts a rust-analyzer proxy on PATH that only works once the
        // component is installed ("Unknown binary 'rust-analyzer.exe'").
        if bin == "rust-analyzer" && crate::proc::exists("rustup") && crate::proc::run("rustup", &["which", "rust-analyzer"], None).is_err() {
            return None;
        }
        return Some(bin.to_string());
    }
    if cfg!(windows) && bin == "clangd" {
        let mut candidates = vec![PathBuf::from(r"C:\Program Files\LLVM\bin\clangd.exe")];
        for pf in [r"C:\Program Files\Microsoft Visual Studio", r"C:\Program Files (x86)\Microsoft Visual Studio"] {
            // <year>/<edition>/VC/Tools/Llvm/x64/bin/clangd.exe
            for year in std::fs::read_dir(pf).into_iter().flatten().flatten() {
                for edition in std::fs::read_dir(year.path()).into_iter().flatten().flatten() {
                    candidates.push(edition.path().join(r"VC\Tools\Llvm\x64\bin\clangd.exe"));
                }
            }
        }
        return candidates.into_iter().find(|p| p.is_file()).map(|p| p.to_string_lossy().to_string());
    }
    None
}

/// The command a language would run: the configured override, else the
/// first known server that's installed. None when disabled or missing.
pub fn resolve(lang: &LanguageDef, setting: Option<&LanguageServerSetting>) -> Option<(String, Vec<String>)> {
    if setting.is_some_and(|s| s.disabled) {
        return None;
    }
    if let Some(cmd) = setting.and_then(|s| s.command.as_deref()).map(str::trim).filter(|c| !c.is_empty()) {
        let mut words = shell_words::split(cmd).ok()?.into_iter();
        let bin = words.next()?;
        return Some((bin, words.collect()));
    }
    lang.servers.iter().find_map(|s| {
        let bin = locate(s.bin)?;
        Some((bin, s.args.iter().map(|a| a.to_string()).collect()))
    })
}

/// Languages a repo uses, from what sits at its root.
pub fn languages_in(dir: &Path) -> Vec<&'static str> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    LANGUAGES
        .iter()
        .filter(|l| {
            l.markers.iter().any(|m| match m.strip_prefix('*') {
                Some(ext) => names.iter().any(|n| n.ends_with(&ext.to_lowercase())),
                None => names.iter().any(|n| n == &m.to_lowercase()),
            })
        })
        .map(|l| l.id)
        .collect()
}

// ---------- compile_commands.json (clangd) ----------

/// The compilation database clangd should use: at the root, in `build/`, or
/// in one level of the usual CMake binary dirs (build/<preset>, out/build/…,
/// cmake-build-*), newest first.
pub fn find_compile_commands(root: &Path) -> Option<PathBuf> {
    let direct = [root.join("compile_commands.json"), root.join("build").join("compile_commands.json")];
    let mut found: Vec<PathBuf> = direct.into_iter().filter(|p| p.is_file()).collect();
    let mut parents = vec![root.join("build"), root.join("out"), root.join("out").join("build")];
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("cmake-build-") {
            found.extend(Some(e.path().join("compile_commands.json")).filter(|p| p.is_file()));
        }
    }
    for parent in parents.drain(..) {
        for e in std::fs::read_dir(&parent).into_iter().flatten().flatten() {
            let p = e.path().join("compile_commands.json");
            if p.is_file() {
                found.push(p);
            }
        }
    }
    found.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()));
    found.into_iter().next()
}

/// Configure presets CMake offers on this machine (it hides the ones whose
/// condition fails, e.g. other OSes).
fn cmake_presets(root: &Path) -> Vec<String> {
    let has_presets = root.join("CMakePresets.json").is_file() || root.join("CMakeUserPresets.json").is_file();
    if !has_presets || !crate::proc::exists("cmake") {
        return vec![];
    }
    crate::proc::run("cmake", &["--list-presets"], Some(root))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split('"').next().map(str::to_string))
        .collect()
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CppSetup {
    pub compile_commands: Option<String>,
    /// The repo builds with CMake.
    pub cmake: bool,
    pub cmake_installed: bool,
    /// Ninja is what makes CMake write compile_commands.json on Windows
    /// (the Visual Studio generator doesn't).
    pub ninja_installed: bool,
    pub presets: Vec<String>,
}

// ---------- running servers ----------

struct Running {
    language: String,
    root: PathBuf,
    command: String,
    started: u64,
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    log: Arc<Mutex<VecDeque<String>>>,
}

fn servers() -> &'static Mutex<HashMap<u32, Running>> {
    static S: OnceLock<Mutex<HashMap<u32, Running>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

const LOG_LINES: usize = 400;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LspMessage {
    id: u32,
    /// Raw JSON-RPC message (parsed by the frontend).
    message: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LspExit {
    id: u32,
    code: Option<i32>,
    /// Last stderr lines, to explain a crash.
    log: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StartInfo {
    pub id: u32,
    /// Absolute folder the server works in (the repo's worktree or clone).
    pub root: String,
    pub command: String,
}

/// Reads LSP frames ("Content-Length: N\r\n\r\n<json>") until EOF.
fn read_frames(stdout: impl Read, mut on_message: impl FnMut(String)) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut len = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse::<usize>().ok();
                }
            }
        }
        let Some(len) = len else { continue };
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        on_message(String::from_utf8_lossy(&body).into_owned());
    }
}

fn write_frame(stdin: &Mutex<ChildStdin>, message: &str) -> Result<(), String> {
    let mut w = stdin.lock().unwrap();
    write!(w, "Content-Length: {}\r\n\r\n{message}", message.len()).map_err(|e| format!("language server is gone: {e}"))?;
    w.flush().map_err(|e| e.to_string())
}

pub fn start(app: &AppHandle, workspace: &str, repo: &str, language_id: &str) -> Result<StartInfo, String> {
    let lang = language(language_id).ok_or_else(|| format!("unknown language '{language_id}'"))?;
    let cfg = Config::load()?;
    let (bin, mut args) = resolve(lang, cfg.language_servers.get(lang.id)).ok_or_else(|| {
        let names: Vec<&str> = lang.servers.iter().map(|s| s.bin).collect();
        format!("no language server for {}: install {} ({})", lang.name, names.join(" or "), lang.servers[0].install)
    })?;
    let root = crate::workspace::repo_path(workspace, repo)?;
    if !root.is_dir() {
        return Err(format!("'{repo}' folder not found"));
    }
    if lang.id == "cpp" && !args.iter().any(|a| a.starts_with("--compile-commands-dir")) {
        // clangd finds <root> and <root>/build itself; deeper preset dirs need a pointer.
        if let Some(dir) = find_compile_commands(&root).and_then(|p| p.parent().map(Path::to_path_buf)) {
            if dir != root && dir != root.join("build") {
                args.push(format!("--compile-commands-dir={}", dir.display()));
            }
        }
    }

    // The frontend starts a server only when it has none: an existing one is
    // left from a reload and can't be initialized twice, so it's replaced.
    let stale: Vec<u32> = {
        let map = servers().lock().unwrap();
        map.iter().filter(|(_, r)| r.root == root && r.language == lang.id).map(|(id, _)| *id).collect()
    };
    for id in stale {
        kill(id);
    }

    let mut child = crate::proc::cmd(&bin)
        .args(&args)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start {bin}: {e}"))?;
    let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let log = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_LINES)));
    let id = next_id();
    let command = std::iter::once(bin.as_str()).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");

    {
        let log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut l = log.lock().unwrap();
                if l.len() == LOG_LINES {
                    l.pop_front();
                }
                l.push_back(line);
            }
        });
    }
    {
        let app = app.clone();
        let log = log.clone();
        std::thread::spawn(move || {
            read_frames(stdout, |message| {
                let _ = app.emit("lsp-message", LspMessage { id, message });
            });
            // stdout closed: the server exited (or was stopped).
            let gone = servers().lock().unwrap().remove(&id);
            let code = gone.and_then(|mut r| r.child.wait().ok()).and_then(|s| s.code());
            let log = log.lock().unwrap().iter().rev().take(20).rev().cloned().collect();
            let _ = app.emit("lsp-exit", LspExit { id, code, log });
        });
    }

    servers().lock().unwrap().insert(
        id,
        Running { language: lang.id.to_string(), root: root.clone(), command: command.clone(), started: now(), child, stdin, log },
    );
    Ok(StartInfo { id, root: root.to_string_lossy().to_string(), command })
}

pub fn send(id: u32, message: &str) -> Result<(), String> {
    let stdin = servers().lock().unwrap().get(&id).map(|r| r.stdin.clone()).ok_or("language server is not running")?;
    write_frame(&stdin, message)
}

fn kill(id: u32) {
    let gone = servers().lock().unwrap().remove(&id);
    if let Some(mut r) = gone {
        let _ = r.child.kill();
        let _ = r.child.wait();
    }
}

/// Asks the server to shut down (LSP shutdown + exit), killing it if it
/// hasn't gone after a grace period.
pub fn stop(id: u32) {
    let stdin = servers().lock().unwrap().get(&id).map(|r| r.stdin.clone());
    if let Some(stdin) = stdin {
        let _ = write_frame(&stdin, r#"{"jsonrpc":"2.0","id":"orbit-shutdown","method":"shutdown"}"#);
        let _ = write_frame(&stdin, r#"{"jsonrpc":"2.0","method":"exit"}"#);
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        kill(id);
    });
}

/// Kills every server (Orbit is quitting).
pub fn shutdown_all() {
    let ids: Vec<u32> = servers().lock().unwrap().keys().copied().collect();
    for id in ids {
        kill(id);
    }
}

// ---------- status (Settings → Languages) ----------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LanguageStatus {
    pub id: String,
    pub name: String,
    /// Servers Orbit knows for it, with install hints.
    pub servers: Vec<KnownServer>,
    /// What would run now; None = missing or disabled.
    pub command: Option<String>,
    pub custom_command: Option<String>,
    pub disabled: bool,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KnownServer {
    pub bin: String,
    pub install: String,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunningServer {
    pub id: u32,
    pub language: String,
    pub root: String,
    pub command: String,
    pub started: u64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LspStatus {
    pub languages: Vec<LanguageStatus>,
    pub running: Vec<RunningServer>,
}

pub fn status() -> Result<LspStatus, String> {
    let cfg = Config::load()?;
    let languages = LANGUAGES
        .iter()
        .map(|l| {
            let setting = cfg.language_servers.get(l.id);
            LanguageStatus {
                id: l.id.into(),
                name: l.name.into(),
                servers: l.servers.iter().map(|s| KnownServer { bin: s.bin.into(), install: s.install.into() }).collect(),
                command: resolve(l, setting).map(|(bin, args)| std::iter::once(bin).chain(args).collect::<Vec<_>>().join(" ")),
                custom_command: setting.and_then(|s| s.command.clone()).filter(|c| !c.trim().is_empty()),
                disabled: setting.is_some_and(|s| s.disabled),
            }
        })
        .collect();
    let mut running: Vec<RunningServer> = servers()
        .lock()
        .unwrap()
        .iter()
        .map(|(id, r)| RunningServer {
            id: *id,
            language: r.language.clone(),
            root: r.root.to_string_lossy().to_string(),
            command: r.command.clone(),
            started: r.started,
        })
        .collect();
    running.sort_by_key(|r| r.id);
    Ok(LspStatus { languages, running })
}

// ---------- Tauri commands ----------

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("background task failed: {e}"))?
}

/// Starts the server for `language` in the repo's folder (worktree, or the
/// clone for a "@repo" scope).
#[tauri::command]
pub async fn lsp_start(app: AppHandle, workspace: String, repo: String, language: String) -> Result<StartInfo, String> {
    blocking(move || start(&app, &workspace, &repo, &language)).await
}

#[tauri::command]
pub fn lsp_send(id: u32, message: String) -> Result<(), String> {
    send(id, &message)
}

#[tauri::command]
pub fn lsp_stop(id: u32) {
    stop(id)
}

#[tauri::command]
pub async fn lsp_status() -> Result<LspStatus, String> {
    blocking(status).await
}

/// Recent stderr of a running server (why it misbehaves).
#[tauri::command]
pub fn lsp_log(id: u32) -> Vec<String> {
    servers().lock().unwrap().get(&id).map(|r| r.log.lock().unwrap().iter().cloned().collect()).unwrap_or_default()
}

#[tauri::command]
pub async fn lsp_configure(language: String, command: Option<String>, disabled: bool) -> Result<(), String> {
    blocking(move || {
        self::language(&language).ok_or_else(|| format!("unknown language '{language}'"))?;
        let mut cfg = Config::load()?;
        let setting = LanguageServerSetting { command: command.filter(|c| !c.trim().is_empty()), disabled };
        if setting == LanguageServerSetting::default() {
            cfg.language_servers.remove(&language);
        } else {
            cfg.language_servers.insert(language, setting);
        }
        cfg.save()
    })
    .await
}

/// A file a server pointed at (a definition in a header outside the repo,
/// say), read for the peek view. Capped like the editor's own reads.
#[tauri::command]
pub async fn lsp_read_file(path: String) -> Result<String, String> {
    blocking(move || {
        let meta = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?;
        if meta.len() > 5 * 1_048_576 {
            return Err("file is too large to show".into());
        }
        std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
    })
    .await
}

/// What clangd needs for a repo: its compilation database, or how to make one.
#[tauri::command]
pub async fn lsp_cpp_setup(workspace: String, repo: String) -> Result<CppSetup, String> {
    blocking(move || {
        let root = crate::workspace::repo_path(&workspace, &repo)?;
        Ok(CppSetup {
            compile_commands: find_compile_commands(&root).map(|p| p.to_string_lossy().to_string()),
            cmake: root.join("CMakeLists.txt").is_file(),
            cmake_installed: crate::proc::exists("cmake"),
            ninja_installed: crate::proc::exists("ninja"),
            presets: cmake_presets(&root),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_framed_messages() {
        let raw = "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}Content-Type: x\r\nContent-Length: 2\r\n\r\n{}";
        let mut got = Vec::new();
        read_frames(raw.as_bytes(), |m| got.push(m));
        assert_eq!(got, ["{\"jsonrpc\":\"2.0\"}", "{}"]);
    }

    #[test]
    fn overrides_win_and_disabled_means_none() {
        let cpp = language("cpp").unwrap();
        let custom = LanguageServerSetting { command: Some("\"C:/tools/clangd.exe\" --log=error".into()), disabled: false };
        assert_eq!(resolve(cpp, Some(&custom)), Some(("C:/tools/clangd.exe".into(), vec!["--log=error".into()])));
        let off = LanguageServerSetting { command: Some("clangd".into()), disabled: true };
        assert_eq!(resolve(cpp, Some(&off)), None);
    }

    #[test]
    fn detects_languages_and_compile_commands() {
        let root = std::env::temp_dir().join(format!("orbit-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("build/windows-x64-debug")).unwrap();
        std::fs::write(root.join("CMakeLists.txt"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        assert_eq!(languages_in(&root), ["cpp", "rust"]);

        assert_eq!(find_compile_commands(&root), None);
        let db = root.join("build/windows-x64-debug/compile_commands.json");
        std::fs::write(&db, "[]").unwrap();
        assert_eq!(find_compile_commands(&root), Some(db));
        let _ = std::fs::remove_dir_all(&root);
    }
}
