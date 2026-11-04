// Run configurations (Rider/CLion style): what a repo builds and runs, read
// from `<repo>/.orbit/run.json` (shared, in git) and `.orbit/run.local.json`
// (one person's overrides). CMake configs resolve their build folder from
// the repo's CMake presets; builds stream `run-output` events.
use crate::agent::runner::{self, Line};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const BUILD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// One configuration as written in run.json; every field can be overridden
/// by run.local.json (matched by name).
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunConfig {
    pub name: String,
    /// "cmake" (build a target with a CMake preset) or "command".
    pub kind: String,
    /// CMake preset; empty or "${auto}" picks one (see `pick_preset`).
    pub preset: String,
    pub target: String,
    /// kind "command": the build command, run in the repo with the shell.
    pub build: String,
    /// What Run starts after a successful build. Variables: ${repo},
    /// ${buildDir}, ${preset}, ${exe}.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
struct RunFile {
    #[serde(default)]
    configurations: Vec<Value>,
}

/// A configuration with its preset picked and variables expanded.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRun {
    pub name: String,
    pub kind: String,
    pub preset: Option<String>,
    pub build_dir: Option<String>,
    /// The build command, for display.
    pub build_label: String,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunSetup {
    /// The repo folder builds run in (links resolved): compiler messages
    /// name files under it.
    pub root: String,
    /// The same folder as the workspace shows it (editor tabs use it).
    pub linked_root: String,
    pub configs: Vec<ResolvedRun>,
    /// CMake presets available on this machine (configure presets).
    pub presets: Vec<String>,
}

// ---------- reading and merging ----------

/// Shared configs with the local file's fields laid over them, by name;
/// local-only names are added.
fn merge(shared: Vec<Value>, local: Vec<Value>) -> Vec<RunConfig> {
    let mut list: Vec<Map<String, Value>> = shared.into_iter().filter_map(|v| v.as_object().cloned()).collect();
    for v in local {
        let Some(over) = v.as_object() else { continue };
        let name = over.get("name").and_then(Value::as_str);
        match list.iter_mut().find(|c| name.is_some() && c.get("name").and_then(Value::as_str) == name) {
            Some(base) => base.extend(over.clone()),
            None => list.push(over.clone()),
        }
    }
    list.into_iter()
        .filter_map(|m| serde_json::from_value::<RunConfig>(Value::Object(m)).ok())
        .filter(|c| !c.name.is_empty())
        .map(|mut c| {
            if c.kind.is_empty() {
                c.kind = if c.build.is_empty() { "cmake".into() } else { "command".into() };
            }
            c
        })
        .collect()
}

fn read_configs(root: &Path) -> Result<Vec<RunConfig>, String> {
    let read = |name: &str| -> Result<Vec<Value>, String> {
        let path = root.join(".orbit").join(name);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<RunFile>(&raw)
                .map(|f| f.configurations)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(vec![]),
        }
    };
    Ok(merge(read("run.json")?, read("run.local.json")?))
}

// ---------- CMake presets ----------

/// Configure presets of CMakePresets.json and CMakeUserPresets.json by name.
fn preset_defs(root: &Path) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for file in ["CMakePresets.json", "CMakeUserPresets.json"] {
        let Ok(raw) = std::fs::read_to_string(root.join(file)) else { continue };
        let Ok(doc) = serde_json::from_str::<Value>(&raw) else { continue };
        for p in doc.get("configurePresets").and_then(Value::as_array).into_iter().flatten() {
            if let Some(name) = p.get("name").and_then(Value::as_str) {
                out.insert(name.to_string(), p.clone());
            }
        }
    }
    out
}

/// A preset's build folder: its `binaryDir`, or the first one it inherits,
/// with the usual macros expanded.
fn binary_dir(root: &Path, defs: &HashMap<String, Value>, preset: &str) -> Option<PathBuf> {
    fn find(defs: &HashMap<String, Value>, name: &str, depth: u8) -> Option<String> {
        let p = defs.get(name)?;
        if let Some(dir) = p.get("binaryDir").and_then(Value::as_str) {
            return Some(dir.to_string());
        }
        if depth > 16 {
            return None;
        }
        let parents: Vec<String> = match p.get("inherits") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => vec![],
        };
        parents.iter().find_map(|parent| find(defs, parent, depth + 1))
    }
    let raw = find(defs, preset, 0)?;
    let source = root.to_string_lossy();
    let parent = root.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let mut dir = raw
        .replace("${sourceDir}", &source)
        .replace("${sourceParentDir}", &parent)
        .replace("${presetName}", preset);
    while let Some(start) = dir.find("$env{") {
        let Some(len) = dir[start..].find('}') else { break };
        let var = dir[start + 5..start + len].to_string();
        dir.replace_range(start..start + len + 1, &std::env::var(&var).unwrap_or_default());
    }
    let path = PathBuf::from(dir);
    Some(if path.is_absolute() { path } else { root.join(path) })
}

/// The preset to use when the config doesn't name one: the first already
/// configured (it has a CMakeCache.txt), else the first offered. `cmake
/// --list-presets` lists user presets first and hides other OSes'.
fn pick_preset(presets: &[String], configured: impl Fn(&str) -> bool) -> Option<String> {
    presets.iter().find(|p| configured(p)).or(presets.first()).cloned()
}

// ---------- resolving ----------

fn expand(s: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(s.to_string(), |acc, (k, v)| acc.replace(&format!("${{{k}}}"), v))
}

fn resolve(
    root: &Path,
    c: &RunConfig,
    chosen: Option<&str>,
    presets: &[String],
    defs: &HashMap<String, Value>,
) -> ResolvedRun {
    let cmake = c.kind == "cmake";
    let preset = if !cmake {
        None
    } else if !chosen.unwrap_or("").is_empty() {
        chosen.map(str::to_string)
    } else if !c.preset.is_empty() && c.preset != "${auto}" {
        Some(c.preset.clone())
    } else {
        pick_preset(presets, |p| binary_dir(root, defs, p).is_some_and(|d| d.join("CMakeCache.txt").is_file()))
    };
    let build_dir = preset.as_deref().and_then(|p| binary_dir(root, defs, p));
    let root_s = root.to_string_lossy().to_string();
    let dir_s = build_dir.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let preset_s = preset.clone().unwrap_or_default();
    let vars = [("repo", root_s.as_str()), ("buildDir", dir_s.as_str()), ("preset", preset_s.as_str()), ("exe", EXE)];
    let build_label = if cmake {
        let target = if c.target.is_empty() { String::new() } else { format!(" --target {}", c.target) };
        format!("cmake --build {dir_s}{target}")
    } else {
        expand(&c.build, &vars)
    };
    let program = Some(expand(&c.program, &vars)).filter(|p| !p.trim().is_empty());
    let cwd = if c.cwd.is_empty() { root_s.clone() } else { expand(&c.cwd, &vars) };
    ResolvedRun {
        name: c.name.clone(),
        kind: c.kind.clone(),
        preset,
        build_dir: build_dir.map(|d| d.to_string_lossy().to_string()),
        build_label,
        program,
        args: c.args.iter().map(|a| expand(a, &vars)).collect(),
        cwd: normalize(&cwd),
        env: c.env.iter().map(|(k, v)| (k.clone(), expand(v, &vars))).collect(),
    }
}

/// Folds `..` and `.` (cwd "${repo}/../client-runtime") without touching
/// the disk.
fn normalize(p: &str) -> String {
    let mut out = PathBuf::new();
    for comp in Path::new(p).components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out.to_string_lossy().to_string()
}

/// The repo folder builds run in: links resolved, so CMake sees the same
/// source path its cache was made with.
fn repo_root(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let root = crate::workspace::repo_path(workspace, repo)?;
    if !root.is_dir() {
        return Err(format!("'{repo}' folder not found"));
    }
    Ok(crate::lsp::real_path(&root))
}

pub fn setup(workspace: &str, repo: &str, preset: Option<&str>) -> Result<RunSetup, String> {
    let root = repo_root(workspace, repo)?;
    let linked_root = crate::workspace::repo_path(workspace, repo)?.to_string_lossy().to_string();
    let configs = read_configs(&root)?;
    let presets = if configs.iter().any(|c| c.kind == "cmake") { crate::lsp::cmake_presets(&root) } else { vec![] };
    let defs = preset_defs(&root);
    Ok(RunSetup {
        root: root.to_string_lossy().to_string(),
        linked_root,
        configs: configs.iter().map(|c| resolve(&root, c, preset, &presets, &defs)).collect(),
        presets,
    })
}

// ---------- building ----------

#[derive(Serialize, Clone)]
struct OutputLine<'a> {
    key: &'a str,
    /// "out", "err", or "info" (Orbit's own lines: the command, timing).
    stream: &'a str,
    line: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildExit {
    pub success: bool,
    pub cancelled: bool,
}

/// `cmake <args>`, inside the Visual Studio environment on Windows.
fn cmake(args: &[String]) -> Command {
    #[cfg(windows)]
    if let Some(vcvars) = crate::lsp::find_vcvars() {
        let script = std::env::temp_dir().join(format!("orbit-run-{}.cmd", std::process::id()));
        if std::fs::write(&script, crate::lsp::vcvars_script(&vcvars.to_string_lossy(), args)).is_ok() {
            let mut c = crate::proc::cmd("cmd");
            c.args(["/d", "/c"]).arg(script);
            return c;
        }
    }
    let mut c = crate::proc::cmd("cmake");
    c.args(args);
    c
}

fn shell(script: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = crate::proc::cmd("cmd");
        c.raw_arg(format!("/d /s /c \"{script}\""));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = crate::proc::cmd("sh");
        c.args(["-c", script]);
        c
    }
}

/// Builds configuration `name` (configuring the preset first when its
/// build folder isn't set up), streaming `run-output {key, stream, line}`.
/// Cancel with `cancel(key)`.
pub fn build(app: &AppHandle, workspace: &str, repo: &str, name: &str, preset: Option<&str>, key: &str) -> Result<BuildExit, String> {
    let root = repo_root(workspace, repo)?;
    let config = read_configs(&root)?
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| format!("no run configuration '{name}' in {repo}/.orbit/run.json"))?;
    let presets = if config.kind == "cmake" { crate::lsp::cmake_presets(&root) } else { vec![] };
    let r = resolve(&root, &config, preset, &presets, &preset_defs(&root));
    let emit = |stream: &str, line: String| {
        let _ = app.emit("run-output", OutputLine { key, stream, line });
    };
    let started = std::time::Instant::now();
    let id = format!("run:{key}");
    let mut steps: Vec<(String, Command)> = Vec::new();
    if config.kind == "cmake" {
        let preset = r.preset.clone().ok_or("no CMake preset available for this platform")?;
        let dir = r.build_dir.clone().ok_or_else(|| format!("preset '{preset}' has no binaryDir"))?;
        if !Path::new(&dir).join("CMakeCache.txt").is_file() {
            let args = vec!["--preset".to_string(), preset.clone()];
            steps.push((format!("cmake --preset {preset}"), cmake(&args)));
        }
        let mut args = vec!["--build".to_string(), dir];
        if !config.target.is_empty() {
            args.extend(["--target".to_string(), config.target.clone()]);
        }
        steps.push((r.build_label.clone(), cmake(&args)));
    } else if !r.build_label.trim().is_empty() {
        steps.push((r.build_label.clone(), shell(&r.build_label)));
    }

    for (label, cmd) in steps {
        emit("info", format!("$ {label}"));
        let outcome = runner::run_streaming(cmd, &root, BUILD_TIMEOUT, Some(&id), |l| match l {
            Line::Out(s) => emit("out", s),
            Line::Err(s) => emit("err", s),
        })?;
        if outcome.cancelled {
            emit("info", "Build cancelled".into());
            return Ok(BuildExit { success: false, cancelled: true });
        }
        if !outcome.success {
            emit("info", format!("Build failed after {:.1}s", started.elapsed().as_secs_f32()));
            return Ok(BuildExit { success: false, cancelled: false });
        }
    }
    emit("info", format!("Build finished in {:.1}s", started.elapsed().as_secs_f32()));
    Ok(BuildExit { success: true, cancelled: false })
}

pub fn cancel(key: &str) -> bool {
    runner::cancel(&format!("run:{key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_overrides_shared_by_name_and_adds_its_own() {
        let shared = vec![json!({"name": "game", "target": "game", "preset": "${auto}"}), json!({"name": "tools", "build": "make tools"})];
        let local = vec![json!({"name": "game", "preset": "linux-gcc-relwithdebinfo"}), json!({"name": "mine", "build": "true"})];
        let list = merge(shared, local);
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["game", "tools", "mine"]);
        assert_eq!(list[0].preset, "linux-gcc-relwithdebinfo");
        assert_eq!(list[0].target, "game");
        assert_eq!(list[0].kind, "cmake");
        assert_eq!(list[1].kind, "command");
    }

    #[test]
    fn binary_dir_follows_inherits_and_macros() {
        let root = Path::new("/src/engine");
        let defs: HashMap<String, Value> = [
            ("base", json!({"name": "base", "binaryDir": "${sourceDir}/build/${presetName}"})),
            ("mid", json!({"name": "mid", "inherits": ["other", "base"]})),
            ("leaf", json!({"name": "leaf", "inherits": "mid"})),
            ("own", json!({"name": "own", "binaryDir": "out/x"})),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(binary_dir(root, &defs, "leaf"), Some(PathBuf::from("/src/engine/build/leaf")));
        assert_eq!(binary_dir(root, &defs, "own"), Some(PathBuf::from("/src/engine/out/x")));
        assert_eq!(binary_dir(root, &defs, "missing"), None);
    }

    #[test]
    fn picks_a_configured_preset_first() {
        let presets = vec!["user-gcc".to_string(), "linux-x64-debug".to_string()];
        assert_eq!(pick_preset(&presets, |p| p == "linux-x64-debug"), Some("linux-x64-debug".into()));
        assert_eq!(pick_preset(&presets, |_| false), Some("user-gcc".into()));
        assert_eq!(pick_preset(&[], |_| true), None);
    }

    #[test]
    fn resolves_program_and_cwd() {
        let root = Path::new("/src/engine");
        let defs: HashMap<String, Value> =
            [("p".to_string(), json!({"name": "p", "binaryDir": "${sourceDir}/build/${presetName}"}))].into_iter().collect();
        let c = RunConfig {
            name: "game".into(),
            kind: "cmake".into(),
            target: "game".into(),
            program: "${buildDir}/src/game/game${exe}".into(),
            cwd: "${repo}/../client-runtime".into(),
            ..Default::default()
        };
        let r = resolve(root, &c, None, &["p".to_string()], &defs);
        assert_eq!(r.preset.as_deref(), Some("p"));
        assert_eq!(r.program, Some(format!("/src/engine/build/p/src/game/game{EXE}")));
        assert_eq!(r.cwd, "/src/client-runtime");
        assert_eq!(r.build_label, "cmake --build /src/engine/build/p --target game");
        let chosen = resolve(root, &c, Some("other"), &["p".to_string()], &defs);
        assert_eq!(chosen.preset.as_deref(), Some("other"));
    }
}
