// Run configurations (Rider/CLion style): what a repo builds and runs, read
// from `<repo>/.orbit/run.json` (shared, in git) and `.orbit/run.local.json`
// (one person's, gitignored), plus one per executable target CMake reports
// (`cmake_api`) or Cargo bin/example (`cargo`), as Rider lists them. CMake
// configs resolve their build folder from the repo's CMake presets; builds
// stream `run-output` events.
use crate::agent::runner::{self, Line};
use crate::cmake_api::{self, CmakeTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use crate::events::Events;

const BUILD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
const SHARED_FILE: &str = "run.json";
const LOCAL_FILE: &str = "run.local.json";

/// One configuration as written in run.json; every field can be overridden
/// by run.local.json (matched by name).
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunConfig {
    pub name: String,
    /// "cmake" (build a target with a CMake preset), "cargo" (a bin or
    /// example), "command", or "compound" (launches `configs`).
    pub kind: String,
    /// CMake preset; empty or "${auto}" picks one (see `pick_preset`).
    pub preset: String,
    pub target: String,
    /// kind "command": the build command, run in the repo with the shell.
    pub build: String,
    /// What Run starts after a successful build; empty: the target's
    /// artifact. Variables: ${repo}, ${buildDir}, ${preset}, ${exe}, $HOME,
    /// ${VAR} (the environment), a leading ~.
    pub program: String,
    pub args: Vec<String>,
    /// Empty: the artifact's folder for a CMake executable (Rider's
    /// default), else the repo.
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    /// Debug adapter: "auto" (gdb on Linux, lldb-dap elsewhere), "gdb",
    /// "lldb-dap", or a path to one.
    pub debugger: String,
    /// Rider's "Before launch: Build"; on when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_before_launch: Option<bool>,
    /// Rider's "Allow multiple instances": launching it again starts another
    /// instance instead of restarting the running one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub allow_multiple: bool,
    /// kind "compound": the configurations it launches, in order: a name in
    /// this repo, or "<repo>/<name>" in another repo of the workspace.
    pub configs: Vec<String>,
}

impl RunConfig {
    pub fn is_compound(&self) -> bool {
        self.kind == "compound"
    }
}

/// How a compound in `own` lists `name` of `repo`.
pub fn part_ref(own: &str, repo: &str, name: &str) -> String {
    if own == repo { name.to_string() } else { format!("{repo}/{name}") }
}

/// A compound's configurations that can launch, as (repo, name) in order, and
/// what is wrong with the others: an unknown name, itself, or another compound
/// (they don't nest). `configs` are every repo's, as (repo, configuration).
pub fn compound_parts<'a>(compound: &RunConfig, own: &str, configs: impl IntoIterator<Item = (&'a str, &'a RunConfig)> + Clone) -> (Vec<(String, String)>, Vec<String>) {
    let (mut parts, mut problems) = (vec![], vec![]);
    for entry in &compound.configs {
        // "<repo>/<name>" only when that repo is in the workspace: names may hold a '/'.
        let (repo, name) = entry
            .split_once('/')
            .filter(|(r, _)| configs.clone().into_iter().any(|(cr, _)| cr == *r))
            .unwrap_or((own, entry.as_str()));
        match configs.clone().into_iter().find(|(r, c)| *r == repo && c.name == name) {
            _ if repo == own && name == compound.name => problems.push(format!("'{name}' can't include itself")),
            None => problems.push(format!("'{}' lists '{entry}', which is not a configuration", compound.name)),
            Some((_, c)) if c.is_compound() => problems.push(format!("'{entry}' is a compound: compounds don't nest")),
            Some(_) => parts.push((repo.to_string(), name.to_string())),
        }
    }
    if compound.configs.is_empty() {
        problems.push(format!("'{}' lists no configurations", compound.name));
    }
    (parts, problems)
}

/// Repos the `compounds` list as "<repo>/<name>" that aren't in the workspace
/// (`in_ws`) but are configured without worktrees, so their one clone is the
/// right checkout to launch from.
pub fn outside_repos<'a>(compounds: impl IntoIterator<Item = &'a RunConfig>, in_ws: &[String], services: &[crate::config::Service]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for entry in compounds.into_iter().filter(|c| c.is_compound()).flat_map(|c| &c.configs) {
        let Some((repo, _)) = entry.split_once('/') else { continue };
        if clone_service(repo, services).is_some() && !in_ws.iter().any(|r| r == repo) && !out.iter().any(|r| r == repo) {
            out.push(repo.to_string());
        }
    }
    out
}

/// The service of a repo configured without worktrees: its one clone is the checkout every workspace runs it from.
pub fn clone_service<'a>(repo: &str, services: &'a [crate::config::Service]) -> Option<&'a crate::config::Service> {
    services.iter().find(|s| s.name == repo && !s.worktree)
}

/// A compound's environment under one of its configurations': the configuration's own keys win.
pub fn merge_env(compound: &BTreeMap<String, String>, own: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    compound.iter().chain(own).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Where a configuration comes from.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `run.json` (maybe with a local override).
    Project,
    /// Only in `run.local.json`.
    Local,
    /// A CMake executable or Cargo bin nobody saved a configuration for.
    Discovered,
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
    pub origin: Origin,
    pub target: String,
    pub preset: Option<String>,
    pub build_dir: Option<String>,
    /// The build command, for display.
    pub build_label: String,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub debugger: String,
    pub build_before_launch: bool,
    /// As saved (or discovered), for editing.
    pub raw: RunConfig,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunSetup {
    /// The repo folder builds run in (links resolved): compiler messages
    /// name files under it.
    pub root: String,
    /// The same folder as the workspace shows it (editor tabs use it).
    pub linked_root: String,
    /// Saved configurations from run.json, then run.local.json, then the
    /// discovered executables.
    pub configs: Vec<ResolvedRun>,
    /// CMake presets available on this machine (configure presets).
    pub presets: Vec<String>,
    /// The repo's preset (the chosen one, or `${auto}`'s pick) and its build folder.
    pub preset: Option<String>,
    pub build_dir: Option<String>,
    /// Every target of that build folder (executables and libraries).
    pub targets: Vec<CmakeTarget>,
}

// ---------- reading and merging ----------

/// Shared configs with the local file's fields laid over them, by name;
/// local-only names are added.
fn merge(shared: Vec<Value>, local: Vec<Value>) -> Vec<(RunConfig, Origin)> {
    let mut list: Vec<(Map<String, Value>, Origin)> =
        shared.into_iter().filter_map(|v| v.as_object().cloned()).map(|m| (m, Origin::Project)).collect();
    for v in local {
        let Some(over) = v.as_object() else { continue };
        let name = over.get("name").and_then(Value::as_str);
        match list.iter_mut().find(|(c, _)| name.is_some() && c.get("name").and_then(Value::as_str) == name) {
            Some((base, _)) => base.extend(over.clone()),
            None => list.push((over.clone(), Origin::Local)),
        }
    }
    list.into_iter()
        .filter_map(|(m, origin)| Some((serde_json::from_value::<RunConfig>(Value::Object(m)).ok()?, origin)))
        .filter(|(c, _)| !c.name.is_empty())
        .map(|(mut c, origin)| {
            if c.kind.is_empty() {
                c.kind = if c.build.is_empty() { "cmake".into() } else { "command".into() };
            }
            (c, origin)
        })
        .collect()
}

/// Saved configurations, then one per executable target that no saved
/// configuration names or builds.
fn with_discovered(mut saved: Vec<(RunConfig, Origin)>, targets: &[CmakeTarget]) -> Vec<(RunConfig, Origin)> {
    let taken = |t: &CmakeTarget, saved: &[(RunConfig, Origin)]| {
        saved.iter().any(|(c, _)| c.name == t.name || (c.kind == kind_of(t) && c.target == t.name))
    };
    let found: Vec<_> = targets
        .iter()
        .filter(|t| t.runnable() && !taken(t, &saved))
        .map(|t| {
            let preset = if t.package.is_some() { String::new() } else { "${auto}".into() };
            let c = RunConfig { name: t.name.clone(), kind: kind_of(t).into(), preset, target: t.name.clone(), ..Default::default() };
            (c, Origin::Discovered)
        })
        .collect();
    saved.extend(found);
    saved
}

fn kind_of(t: &CmakeTarget) -> &'static str {
    if t.package.is_some() { "cargo" } else { "cmake" }
}

fn read_file(root: &Path, name: &str) -> Result<Vec<Value>, String> {
    let path = root.join(".orbit").join(name);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<RunFile>(&raw).map(|f| f.configurations).map_err(|e| format!("{}: {e}", path.display())),
        Err(_) => Ok(vec![]),
    }
}

fn read_configs(root: &Path) -> Result<Vec<(RunConfig, Origin)>, String> {
    Ok(merge(read_file(root, SHARED_FILE)?, read_file(root, LOCAL_FILE)?))
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

fn auto_preset(root: &Path, presets: &[String], defs: &HashMap<String, Value>) -> Option<String> {
    pick_preset(presets, |p| binary_dir(root, defs, p).is_some_and(|d| d.join("CMakeCache.txt").is_file()))
}

/// The build folder of the chosen (or `${auto}`) preset, else a configured `build/`.
pub fn cmake_build_dir(root: &Path, preset: Option<&str>) -> Option<PathBuf> {
    let defs = preset_defs(root);
    let preset = preset.filter(|p| !p.is_empty()).map(str::to_string).or_else(|| auto_preset(root, &crate::lsp::cmake_presets(root), &defs));
    preset.and_then(|p| binary_dir(root, &defs, &p)).or_else(|| Some(root.join("build")).filter(|d| d.join("CMakeCache.txt").is_file()))
}

// ---------- resolving ----------

fn expand(s: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(s.to_string(), |acc, (k, v)| acc.replace(&format!("${{{k}}}"), v))
}

/// A leading `~`, then `$NAME` and `${NAME}` from the environment (unset
/// ones stay as written).
fn expand_env(s: &str, get: impl Fn(&str) -> Option<String>) -> String {
    let home = || std::env::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    let s = if s == "~" {
        home()
    } else if let Some(rest) = s.strip_prefix("~/") {
        format!("{}/{rest}", home())
    } else {
        s.to_string()
    };
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, len) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).unwrap_or(after.len());
            (&after[..end], end)
        };
        match (!name.is_empty()).then(|| get(name)).flatten() {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..at + 1 + len]),
        }
        rest = &rest[at + 1 + len..];
    }
    out.push_str(rest);
    out
}

fn expand_all(s: &str, vars: &[(&str, &str)]) -> String {
    expand_env(&expand(s, vars), |k| std::env::var(k).ok())
}

/// `K=V;K2=V2` (Rider's environment field; newlines separate too).
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    text.split([';', '\n'])
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim_end_matches('\r').to_string()))
        })
        .collect()
}

/// The target's artifact as a program path, relative to `${buildDir}`.
fn artifact_program(t: &CmakeTarget) -> Option<String> {
    let a = t.artifact.as_deref()?;
    Some(if Path::new(a).is_absolute() { a.to_string() } else { format!("${{buildDir}}/{a}") })
}

fn artifact_cwd(t: &CmakeTarget) -> Option<String> {
    let dir = t.artifact_dir()?;
    Some(if Path::new(&dir).is_absolute() { dir } else if dir.is_empty() { "${buildDir}".into() } else { format!("${{buildDir}}/{dir}") })
}

/// What an empty program and cwd mean for this configuration: the
/// target's artifact and its folder (Cargo: the repo, as `cargo run`), when
/// it builds an executable.
pub fn defaults(c: &RunConfig, targets: &[CmakeTarget]) -> (Option<String>, Option<String>) {
    let t = targets.iter().find(|t| c.kind == kind_of(t) && !c.target.is_empty() && t.name == c.target && t.runnable());
    (t.and_then(artifact_program), t.filter(|t| t.package.is_none()).and_then(artifact_cwd))
}

struct Ctx<'a> {
    root: &'a Path,
    chosen: Option<&'a str>,
    presets: &'a [String],
    defs: &'a HashMap<String, Value>,
    targets: &'a [CmakeTarget],
    /// The Cargo workspace root, when the repo has one.
    cargo_root: Option<&'a Path>,
}

/// A cargo config's target, or a bin of that name when discovery no longer lists it.
fn cargo_target(c: &RunConfig, targets: &[CmakeTarget]) -> CmakeTarget {
    targets
        .iter()
        .find(|t| t.package.is_some() && t.name == c.target)
        .cloned()
        .unwrap_or_else(|| CmakeTarget { name: c.target.clone(), kind: "bin".into(), ..Default::default() })
}

fn resolve(x: &Ctx, c: &RunConfig, origin: Origin) -> ResolvedRun {
    let root = x.root;
    let cmake = c.kind == "cmake";
    let preset = if !cmake {
        None
    } else if !x.chosen.unwrap_or("").is_empty() {
        x.chosen.map(str::to_string)
    } else if !c.preset.is_empty() && c.preset != "${auto}" {
        Some(c.preset.clone())
    } else {
        auto_preset(root, x.presets, x.defs)
    };
    // Cargo's "build folder" is where its messages' relative paths start.
    let build_dir = if c.kind == "cargo" { x.cargo_root.map(Path::to_path_buf) } else { preset.as_deref().and_then(|p| binary_dir(root, x.defs, p)) };
    let root_s = root.to_string_lossy().to_string();
    let dir_s = build_dir.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let preset_s = preset.clone().unwrap_or_default();
    let vars = [("repo", root_s.as_str()), ("buildDir", dir_s.as_str()), ("preset", preset_s.as_str()), ("exe", EXE)];
    let build_label = if cmake {
        let target = if c.target.is_empty() { String::new() } else { format!(" --target {}", c.target) };
        format!("cmake --build {dir_s}{target}")
    } else if c.kind == "cargo" {
        format!("cargo {}", crate::cargo::build_args(Some(&cargo_target(c, x.targets))).join(" "))
    } else {
        expand(&c.build, &vars)
    };
    let (default_program, default_cwd) = defaults(c, x.targets);
    let program = Some(c.program.clone()).filter(|p| !p.trim().is_empty()).or(default_program);
    let program = program.map(|p| expand_all(&p, &vars)).filter(|p| !p.trim().is_empty());
    let cwd = Some(c.cwd.clone()).filter(|p| !p.trim().is_empty()).or(default_cwd).unwrap_or_else(|| "${repo}".into());
    ResolvedRun {
        name: c.name.clone(),
        kind: c.kind.clone(),
        origin,
        target: c.target.clone(),
        preset,
        build_dir: build_dir.map(|d| d.to_string_lossy().to_string()),
        build_label,
        program,
        args: c.args.iter().map(|a| expand(a, &vars)).collect(),
        cwd: normalize(&expand_all(&cwd, &vars)),
        env: c.env.iter().map(|(k, v)| (k.clone(), expand_all(v, &vars))).collect(),
        debugger: c.debugger.clone(),
        build_before_launch: c.build_before_launch.unwrap_or(true),
        raw: c.clone(),
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

/// The repo's folder in the workspace or, for a repo the workspace doesn't hold, its clone when it runs from one.
fn repo_dir(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let dir = crate::workspace::repo_path(workspace, repo)?;
    if dir.is_dir() {
        return Ok(dir);
    }
    let services = crate::config::Config::load().map(|c| c.services).unwrap_or_default();
    clone_service(repo, &services)
        .and_then(|svc| crate::workspace::clone_dir(svc).ok())
        .filter(|d| d.is_dir())
        .ok_or_else(|| format!("'{repo}' folder not found"))
}

/// The repo folder builds run in: links resolved, so CMake sees the same
/// source path its cache was made with.
fn repo_root(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    Ok(crate::lsp::real_path(&repo_dir(workspace, repo)?))
}

/// Everything a repo's configurations resolve against.
struct Loaded {
    configs: Vec<(RunConfig, Origin)>,
    presets: Vec<String>,
    defs: HashMap<String, Value>,
    preset: Option<String>,
    build_dir: Option<PathBuf>,
    targets: Vec<CmakeTarget>,
    cargo_root: Option<PathBuf>,
}

fn load(root: &Path, chosen: Option<&str>) -> Result<Loaded, String> {
    let saved = read_configs(root)?;
    let cmake_repo = saved.iter().any(|(c, _)| c.kind == "cmake") || root.join("CMakeLists.txt").is_file();
    let presets = if cmake_repo { crate::lsp::cmake_presets(root) } else { vec![] };
    let defs = preset_defs(root);
    let preset = chosen.filter(|p| !p.is_empty()).map(str::to_string).or_else(|| auto_preset(root, &presets, &defs));
    let build_dir = preset.as_deref().and_then(|p| binary_dir(root, &defs, p));
    // A CMake repo with nothing saved and never configured: configure it, as
    // Rider loads a CMake project on open, so its targets can be listed.
    if let (Some(p), Some(dir)) = (preset.as_deref(), build_dir.as_deref()) {
        if saved.is_empty() && !dir.join("CMakeCache.txt").is_file() {
            cmake_api::ensure_query(dir);
            let mut c = cmake(&["--preset".to_string(), p.to_string()]);
            c.current_dir(root).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            let _ = c.status();
        }
    }
    let mut targets = build_dir.as_deref().map(|dir| cmake_api::targets(dir, || reconfigure(root, dir))).unwrap_or_default();
    let (cargo_root, cargo_targets) = crate::cargo::targets(root).unzip();
    targets.extend(cargo_targets.into_iter().flatten());
    let build_dir = build_dir.or_else(|| cargo_root.clone());
    Ok(Loaded { configs: with_discovered(saved, &targets), presets, defs, preset, build_dir, targets, cargo_root })
}

/// Configures an already configured build folder again, so CMake answers
/// the File API query.
fn reconfigure(root: &Path, dir: &Path) -> bool {
    let mut c = cmake(&[dir.to_string_lossy().to_string()]);
    c.current_dir(root).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.status().is_ok_and(|s| s.success())
}

pub fn setup(workspace: &str, repo: &str, preset: Option<&str>) -> Result<RunSetup, String> {
    let root = repo_root(workspace, repo)?;
    let linked_root = repo_dir(workspace, repo)?.to_string_lossy().to_string();
    let l = load(&root, preset)?;
    let x = Ctx { root: &root, chosen: preset, presets: &l.presets, defs: &l.defs, targets: &l.targets, cargo_root: l.cargo_root.as_deref() };
    Ok(RunSetup {
        root: root.to_string_lossy().to_string(),
        linked_root,
        configs: l.configs.iter().map(|(c, o)| resolve(&x, c, *o)).collect(),
        presets: l.presets.clone(),
        preset: l.preset.clone(),
        build_dir: l.build_dir.as_ref().map(|d| d.to_string_lossy().to_string()),
        targets: l.targets.clone(),
    })
}

// ---------- saving ----------

/// The file's line ending, final newline and indent, kept when it is rewritten.
struct Style {
    crlf: bool,
    final_newline: bool,
    indent: String,
}

fn style_of(raw: &str) -> Style {
    let indent = raw
        .lines()
        .find_map(|l| {
            let n = l.len() - l.trim_start_matches([' ', '\t']).len();
            (n > 0).then(|| l[..n].to_string())
        })
        .unwrap_or_else(|| "  ".into());
    Style { crlf: raw.contains("\r\n"), final_newline: raw.is_empty() || raw.ends_with('\n'), indent }
}

fn write_json(doc: &Value, style: &Style) -> Result<String, String> {
    use serde::Serialize as _;
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(style.indent.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    doc.serialize(&mut ser).map_err(|e| e.to_string())?;
    let mut out = String::from_utf8(buf).map_err(|e| e.to_string())?;
    if style.final_newline {
        out.push('\n');
    }
    Ok(if style.crlf { out.replace('\n', "\r\n") } else { out })
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// `existing` with the configuration's fields: set ones replaced or
/// appended, empty ones (and defaults) dropped, unknown ones kept, in order.
fn update_entry(existing: &Map<String, Value>, config: &RunConfig) -> Map<String, Value> {
    let Ok(Value::Object(fields)) = serde_json::to_value(config) else { return existing.clone() };
    let keep = |k: &str, v: &Value| !is_empty(v) && !(k == "buildBeforeLaunch" && v == &Value::Bool(true));
    let mut out = Map::new();
    for (k, v) in existing {
        match fields.get(k) {
            Some(new) if keep(k, new) => {
                out.insert(k.clone(), new.clone());
            }
            Some(_) => {}
            // Fields serde leaves out at their default: dropped from the file too.
            None if k == "buildBeforeLaunch" || k == "allowMultiple" => {}
            None => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    for (k, v) in fields {
        if !out.contains_key(&k) && !existing.contains_key(&k) && keep(&k, &v) {
            out.insert(k, v);
        }
    }
    out
}

/// Reads `.orbit/<file>`, lets `f` change its configurations, and writes it
/// back in the same style when something changed. A missing file starts
/// empty and is only created when `f` adds to it.
fn edit_file(root: &Path, file: &str, f: impl FnOnce(&mut Vec<Value>)) -> Result<(), String> {
    let path = root.join(".orbit").join(file);
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: Value = if raw.trim().is_empty() {
        serde_json::json!({ "configurations": [] })
    } else {
        serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?
    };
    let obj = doc.as_object_mut().ok_or_else(|| format!("{}: not a JSON object", path.display()))?;
    let list = obj.entry("configurations").or_insert_with(|| Value::Array(vec![]));
    let Value::Array(list) = list else { return Err(format!("{}: configurations is not a list", path.display())) };
    let before = list.clone();
    f(list);
    if *list == before {
        return Ok(());
    }
    // A new file takes its sibling's style (aether-engine's run.json is CRLF).
    let sibling = if file == SHARED_FILE { LOCAL_FILE } else { SHARED_FILE };
    let model = if raw.is_empty() { std::fs::read_to_string(root.join(".orbit").join(sibling)).unwrap_or_default() } else { raw.clone() };
    let text = write_json(&doc, &style_of(&model))?;
    std::fs::create_dir_all(path.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn named(v: &Value, names: &[&str]) -> bool {
    v.get("name").and_then(Value::as_str).is_some_and(|n| names.contains(&n))
}

/// Saves a configuration to run.json (`shared`, Rider's "Store as project
/// file") or run.local.json, replacing the entry named `previous` (or its
/// own name) and keeping its other fields; the other file loses that name.
pub fn save_config(root: &Path, config: &RunConfig, shared: bool, previous: Option<&str>) -> Result<(), String> {
    let names: Vec<&str> = [Some(config.name.as_str()), previous].into_iter().flatten().collect();
    let (to, other) = if shared { (SHARED_FILE, LOCAL_FILE) } else { (LOCAL_FILE, SHARED_FILE) };
    edit_file(root, to, |list| {
        let at = names.iter().find_map(|n| list.iter().position(|v| named(v, &[n])));
        match at {
            Some(ix) => {
                let base = list[ix].as_object().cloned().unwrap_or_default();
                list[ix] = Value::Object(update_entry(&base, config));
                let mut seen = false;
                list.retain(|v| {
                    let dup = named(v, &names);
                    let keep = !dup || !seen;
                    seen |= dup;
                    keep
                });
            }
            None => list.push(Value::Object(update_entry(&Map::new(), config))),
        }
    })?;
    if root.join(".orbit").join(other).is_file() {
        edit_file(root, other, |list| list.retain(|v| !named(v, &names)))?;
    }
    Ok(())
}

/// Removes a saved configuration from both files.
pub fn remove_config(root: &Path, name: &str) -> Result<(), String> {
    for file in [SHARED_FILE, LOCAL_FILE] {
        if root.join(".orbit").join(file).is_file() {
            edit_file(root, file, |list| list.retain(|v| !named(v, &[name])))?;
        }
    }
    Ok(())
}

/// The repo folder of a workspace repo, as `save_config` takes it.
pub fn root_of(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    repo_root(workspace, repo)
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

/// What a build makes.
#[derive(Clone, Copy, Debug)]
pub enum BuildWhat<'a> {
    /// A configuration's target (or its build command).
    Config(&'a str),
    /// Every target of the repo's build folder (`cmake --build <dir>`).
    All,
}

/// Builds configuration `name`, or everything, configuring the preset first
/// when its build folder isn't set up, streaming `run-output {key, stream,
/// line}`. Cancel with `cancel(key)`.
pub fn build(app: &Events, workspace: &str, repo: &str, what: BuildWhat, preset: Option<&str>, key: &str) -> Result<BuildExit, String> {
    let root = repo_root(workspace, repo)?;
    let l = load(&root, preset)?;
    let x = Ctx { root: &root, chosen: preset, presets: &l.presets, defs: &l.defs, targets: &l.targets, cargo_root: l.cargo_root.as_deref() };
    // Build All is the repo's preset with no target: a configuration's shape.
    let (kind, target, r) = match what {
        BuildWhat::Config(name) => {
            let (config, origin) = l
                .configs
                .iter()
                .find(|(c, _)| c.name == name)
                .ok_or_else(|| format!("no run configuration '{name}' in {repo}"))?;
            (config.kind.clone(), config.target.clone(), resolve(&x, config, *origin))
        }
        BuildWhat::All if l.presets.is_empty() && l.cargo_root.is_some() => {
            let all = RunConfig { name: "all".into(), kind: "cargo".into(), ..Default::default() };
            ("cargo".to_string(), String::new(), resolve(&x, &all, Origin::Discovered))
        }
        BuildWhat::All => {
            if l.presets.is_empty() {
                return Err(format!("{repo} has no CMake presets to build"));
            }
            let all = RunConfig { name: "all".into(), kind: "cmake".into(), preset: l.preset.clone().unwrap_or_default(), ..Default::default() };
            ("cmake".to_string(), String::new(), resolve(&x, &all, Origin::Discovered))
        }
    };
    let emit = |stream: &str, line: String| {
        let _ = app.emit("run-output", OutputLine { key, stream, line });
    };
    let started = std::time::Instant::now();
    let id = format!("run:{key}");
    let mut steps: Vec<(String, Command)> = Vec::new();
    if kind == "cmake" {
        let preset = r.preset.clone().ok_or("no CMake preset available for this platform")?;
        let dir = r.build_dir.clone().ok_or_else(|| format!("preset '{preset}' has no binaryDir"))?;
        if !Path::new(&dir).join("CMakeCache.txt").is_file() {
            // Before the first configure, so its reply lists the targets.
            cmake_api::ensure_query(Path::new(&dir));
            let args = vec!["--preset".to_string(), preset.clone()];
            steps.push((format!("cmake --preset {preset}"), cmake(&args)));
        }
        let mut args = vec!["--build".to_string(), dir];
        if !target.is_empty() {
            args.extend(["--target".to_string(), target.clone()]);
        }
        steps.push((r.build_label.clone(), cmake(&args)));
    } else if kind == "cargo" {
        let args = match what {
            BuildWhat::All => crate::cargo::build_args(None),
            BuildWhat::Config(_) => crate::cargo::build_args(Some(&cargo_target(&r.raw, &l.targets))),
        };
        let mut c = crate::proc::cmd("cargo");
        c.args(&args);
        steps.push((format!("cargo {}", args.join(" ")), c));
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
    fn a_repo_outside_the_workspace_builds_from_its_clone() {
        let _g = crate::workspace::tests::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("orbit-run-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("ORBIT_WORKSPACE_ROOT", root.join("ws-root"));
        std::env::set_var("ORBIT_CONFIG_DIR", root.join("cfg"));
        let svc = |name: &str, worktree: bool| crate::config::Service { worktree, ..crate::config::Service::new(name.into(), String::new()) };
        let config = crate::config::Config { services: vec![svc("dataserver", false), svc("web", true)], ..Default::default() };
        config.save().unwrap();
        let repos = root.join("ws-root/repos");
        for repo in ["dataserver", "web"] {
            std::fs::create_dir_all(repos.join(repo)).unwrap();
        }
        std::fs::create_dir_all(root.join("ws-root/workspaces/feat-x/game")).unwrap();

        assert_eq!(repo_dir("feat-x", "game").unwrap(), root.join("ws-root/workspaces/feat-x/game"));
        assert_eq!(repo_dir("feat-x", "dataserver").unwrap(), repos.join("dataserver"));
        assert_eq!(repo_dir("feat-x", "web").unwrap_err(), "'web' folder not found");
        assert_eq!(repo_dir("feat-x", "unknown").unwrap_err(), "'unknown' folder not found");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_configurations_env_overrides_its_compounds() {
        let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let compound = env(&[("LOG", "debug"), ("PORT", "80")]);
        assert_eq!(merge_env(&compound, &env(&[("PORT", "8080"), ("X", "1")])), env(&[("LOG", "debug"), ("PORT", "8080"), ("X", "1")]));
        assert_eq!(merge_env(&compound, &BTreeMap::new()), compound);
        assert_eq!(merge_env(&BTreeMap::new(), &compound), compound);
    }

    fn names(list: &[(RunConfig, Origin)]) -> Vec<(&str, Origin)> {
        list.iter().map(|(c, o)| (c.name.as_str(), *o)).collect()
    }

    fn target(name: &str, kind: &str, artifact: Option<&str>) -> CmakeTarget {
        CmakeTarget { name: name.into(), kind: kind.into(), artifact: artifact.map(str::to_string), ..Default::default() }
    }

    #[test]
    fn local_overrides_shared_by_name_and_adds_its_own() {
        let shared = vec![json!({"name": "game", "target": "game", "preset": "${auto}"}), json!({"name": "tools", "build": "make tools"})];
        let local = vec![json!({"name": "game", "preset": "linux-gcc-relwithdebinfo"}), json!({"name": "mine", "build": "true"})];
        let list = merge(shared, local);
        assert_eq!(names(&list), [("game", Origin::Project), ("tools", Origin::Project), ("mine", Origin::Local)]);
        assert_eq!(list[0].0.preset, "linux-gcc-relwithdebinfo");
        assert_eq!(list[0].0.target, "game");
        assert_eq!(list[0].0.kind, "cmake");
        assert_eq!(list[1].0.kind, "command");
    }

    #[test]
    fn discovered_targets_follow_saved_configs() {
        let saved = merge(
            vec![json!({"name": "game", "target": "game", "cwd": "${repo}/../client-runtime"})],
            vec![json!({"name": "server (debug)", "target": "gameserver"}), json!({"name": "shared_test", "build": "make"})],
        );
        let targets = [
            target("game", "EXECUTABLE", Some("src/game/game")),
            target("gameserver", "EXECUTABLE", Some("src/gameserver/gameserver")),
            target("shared", "STATIC_LIBRARY", Some("src/shared/libshared.a")),
            target("shared_test", "EXECUTABLE", Some("test/shared/shared_test")),
            target("modelio_test", "EXECUTABLE", Some("test/modelio/modelio_test")),
        ];
        let list = with_discovered(saved, &targets);
        // Saved ones first; game and gameserver are taken by target, shared_test by name; libraries don't run.
        assert_eq!(
            names(&list),
            [("game", Origin::Project), ("server (debug)", Origin::Local), ("shared_test", Origin::Local), ("modelio_test", Origin::Discovered)]
        );

        let root = Path::new("/src/engine");
        let defs: HashMap<String, Value> =
            [("p".to_string(), json!({"name": "p", "binaryDir": "${sourceDir}/build/${presetName}"}))].into_iter().collect();
        let presets = ["p".to_string()];
        let x = Ctx { root, chosen: None, presets: &presets, defs: &defs, targets: &targets, cargo_root: None };
        let r: Vec<_> = list.iter().map(|(c, o)| resolve(&x, c, *o)).collect();
        // A saved config without a program gets the target's artifact and keeps its own cwd.
        assert_eq!(r[0].program.as_deref(), Some("/src/engine/build/p/src/game/game"));
        assert_eq!(r[0].cwd, "/src/client-runtime");
        // Discovered: Rider's default cwd, the artifact's folder.
        assert_eq!(r[3].program.as_deref(), Some("/src/engine/build/p/test/modelio/modelio_test"));
        assert_eq!(r[3].cwd, "/src/engine/build/p/test/modelio");
        assert_eq!(r[3].build_label, "cmake --build /src/engine/build/p --target modelio_test");
        // A command config doesn't take a target's defaults: no program, the repo as cwd.
        assert_eq!((r[2].program.as_deref(), r[2].cwd.as_str()), (None, "/src/engine"));
        assert!(r.iter().all(|r| r.build_before_launch));
    }

    #[test]
    fn cargo_bins_become_cargo_configs() {
        let bin = CmakeTarget {
            name: "dataserver".into(),
            kind: "bin".into(),
            artifact: Some("/src/ds/target/debug/dataserver".into()),
            package: Some("dataserver".into()),
            features: vec![],
        };
        let saved = merge(vec![json!({"name": "seeds", "kind": "cargo", "target": "seeds"})], vec![]);
        let list = with_discovered(saved, std::slice::from_ref(&bin));
        assert_eq!(names(&list), [("seeds", Origin::Project), ("dataserver", Origin::Discovered)]);
        let root = Path::new("/src/ds");
        let x = Ctx { root, chosen: None, presets: &[], defs: &HashMap::new(), targets: std::slice::from_ref(&bin), cargo_root: Some(root) };
        let r = resolve(&x, &list[1].0, Origin::Discovered);
        assert_eq!((r.kind.as_str(), r.preset.as_deref()), ("cargo", None));
        assert_eq!(r.program.as_deref(), Some("/src/ds/target/debug/dataserver"));
        assert_eq!(r.cwd, "/src/ds");
        assert_eq!(r.build_dir.as_deref(), Some("/src/ds"));
        assert_eq!(r.build_label, "cargo build -p dataserver --bin dataserver --message-format=short");
        // A saved target discovery no longer lists still builds by name.
        assert_eq!(resolve(&x, &list[0].0, Origin::Project).build_label, "cargo build --bin seeds --message-format=short");
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
        let presets = ["p".to_string()];
        let x = Ctx { root, chosen: None, presets: &presets, defs: &defs, targets: &[], cargo_root: None };
        let r = resolve(&x, &c, Origin::Project);
        assert_eq!(r.preset.as_deref(), Some("p"));
        assert_eq!(r.program, Some(format!("/src/engine/build/p/src/game/game{EXE}")));
        assert_eq!(r.cwd, "/src/client-runtime");
        assert_eq!(r.build_label, "cmake --build /src/engine/build/p --target game");
        let chosen = resolve(&Ctx { chosen: Some("other"), ..x }, &c, Origin::Project);
        assert_eq!(chosen.preset.as_deref(), Some("other"));
    }

    #[test]
    fn expands_the_environment_and_home() {
        let get = |k: &str| match k {
            "HOME" => Some("/home/me".to_string()),
            "XDG" => Some("/x".to_string()),
            _ => None,
        };
        assert_eq!(expand_env("$HOME/.cache/nv", get), "/home/me/.cache/nv");
        assert_eq!(expand_env("${XDG}/a:$UNSET:${NOPE}:$", get), "/x/a:$UNSET:${NOPE}:$");
        let home = std::env::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(expand_env("~/cache", get), format!("{home}/cache"));
        assert_eq!(expand_env("a~b", get), "a~b");
    }

    #[test]
    fn parses_env_pairs() {
        assert_eq!(
            parse_env("__GL_SHADER_DISK_CACHE_PATH=/home/igors/.cache/nvidia-aether-dev; A=1;B=x=y;;=bad;C="),
            [
                ("__GL_SHADER_DISK_CACHE_PATH".to_string(), "/home/igors/.cache/nvidia-aether-dev".to_string()),
                ("A".into(), "1".into()),
                ("B".into(), "x=y".into()),
                ("C".into(), "".into()),
            ]
        );
        assert!(parse_env("no pairs here").is_empty());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".orbit")).unwrap();
        dir
    }

    #[test]
    fn saving_keeps_crlf_fields_and_order() {
        let root = scratch("crlf");
        let shared = "{\r\n  \"configurations\": [\r\n    {\r\n      \"name\": \"game\",\r\n      \"kind\": \"cmake\",\r\n      \"preset\": \"${auto}\",\r\n      \"target\": \"game\",\r\n      \"program\": \"${buildDir}/src/game/game${exe}\",\r\n      \"cwd\": \"${repo}/../client-runtime\",\r\n      \"note\": \"kept\"\r\n    }\r\n  ]\r\n}\r\n";
        std::fs::write(root.join(".orbit/run.json"), shared).unwrap();
        let (mut game, _) = read_configs(&root).unwrap().remove(0);
        game.env.insert("__GL_SHADER_DISK_CACHE_PATH".into(), "$HOME/.cache/nvidia-aether-dev".into());
        game.program.clear();
        save_config(&root, &game, true, None).unwrap();
        let text = std::fs::read_to_string(root.join(".orbit/run.json")).unwrap();
        assert!(!text.replace("\r\n", "").contains('\n'), "only CRLF line ends: {text:?}");
        assert!(text.ends_with("}\r\n"));
        let doc: Value = serde_json::from_str(&text).unwrap();
        let entry = doc["configurations"][0].as_object().unwrap();
        assert_eq!(entry.keys().collect::<Vec<_>>(), ["name", "kind", "preset", "target", "cwd", "note", "env"]);
        assert_eq!(entry["note"], "kept");
        assert!(!root.join(".orbit/run.local.json").exists());

        // Unchecking "Store as project file" moves it to the local file (new: CRLF like run.json).
        game.name = "game local".into();
        game.build_before_launch = Some(false);
        save_config(&root, &game, false, Some("game")).unwrap();
        let shared: Value = serde_json::from_str(&std::fs::read_to_string(root.join(".orbit/run.json")).unwrap()).unwrap();
        assert_eq!(shared["configurations"], json!([]));
        let local = std::fs::read_to_string(root.join(".orbit/run.local.json")).unwrap();
        assert!(local.contains("\r\n") && !local.replace("\r\n", "").contains('\n'));
        let list = read_configs(&root).unwrap();
        assert_eq!(names(&list), [("game local", Origin::Local)]);
        assert_eq!(list[0].0.build_before_launch, Some(false));
        assert_eq!(list[0].0.env["__GL_SHADER_DISK_CACHE_PATH"], "$HOME/.cache/nvidia-aether-dev");

        remove_config(&root, "game local").unwrap();
        assert!(read_configs(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compounds_list_known_configurations_in_order() {
        let c = |name: &str, kind: &str, configs: &[&str]| RunConfig {
            name: name.into(),
            kind: kind.into(),
            configs: configs.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let all = [c("gameserver", "cmake", &[]), c("game", "cmake", &[]), c("both", "compound", &["game", "gameserver"]), c("other", "compound", &[])];
        let ds = [c("server", "cargo", &[]), c("a/b", "cargo", &[])];
        let every = all.iter().map(|c| ("aether", c)).chain(ds.iter().map(|c| ("dataserver", c)));
        let pair = |r: &str, n: &str| (r.to_string(), n.to_string());
        assert_eq!(compound_parts(&all[2], "aether", every.clone()), (vec![pair("aether", "game"), pair("aether", "gameserver")], vec![]));

        let bad = c("bad", "compound", &["game", "nope", "bad", "both"]);
        let (parts, problems) = compound_parts(&bad, "aether", every.clone());
        assert_eq!(parts, [pair("aether", "game")]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("'nope'") && problems[1].contains("itself") && problems[2].contains("don't nest"));
        assert_eq!(compound_parts(&all[3], "aether", every.clone()).1.len(), 1);

        // Another repo's configurations, by "<repo>/<name>"; a '/' in a name of this repo still works.
        let mixed = c("stack", "compound", &["dataserver/server", "game", "dataserver/nope"]);
        let (parts, problems) = compound_parts(&mixed, "aether", every.clone());
        assert_eq!(parts, [pair("dataserver", "server"), pair("aether", "game")]);
        assert!(problems[0].contains("'dataserver/nope'"));
        assert_eq!(compound_parts(&c("x", "compound", &["a/b"]), "dataserver", every.clone()).0, [pair("dataserver", "a/b")]);
        assert_eq!(part_ref("aether", "dataserver", "server"), "dataserver/server");
        assert_eq!(part_ref("aether", "aether", "game"), "game");

        // Repos outside the workspace load only when configured without worktrees.
        let svc = |name: &str, worktree: bool| crate::config::Service { worktree, ..crate::config::Service::new(name.into(), String::new()) };
        let services = [svc("dataserver", false), svc("web", true)];
        let stacks = [mixed.clone(), c("w", "compound", &["web/x", "unknown/y", "dataserver/z"]), c("dataserver/fake", "cargo", &[])];
        assert_eq!(outside_repos(&stacks, &["aether".into()], &services), ["dataserver"]);
        assert!(outside_repos(&stacks, &["aether".into(), "dataserver".into()], &services).is_empty());

        let parsed: RunConfig = serde_json::from_value(json!({ "name": "both", "kind": "compound", "configs": ["game", "gameserver"] })).unwrap();
        assert_eq!(parsed, all[2]);
    }

    #[test]
    fn allow_multiple_is_saved_only_when_on() {
        let off: RunConfig = serde_json::from_value(json!({ "name": "game" })).unwrap();
        assert!(!off.allow_multiple);
        assert!(serde_json::to_value(&off).unwrap().get("allowMultiple").is_none());

        let root = scratch("multi");
        let mut game = RunConfig { name: "game".into(), kind: "command".into(), allow_multiple: true, ..Default::default() };
        save_config(&root, &game, true, None).unwrap();
        let text = std::fs::read_to_string(root.join(".orbit/run.json")).unwrap();
        assert!(text.contains("\"allowMultiple\": true"), "{text}");
        assert!(read_configs(&root).unwrap()[0].0.allow_multiple);

        game.allow_multiple = false;
        save_config(&root, &game, true, Some("game")).unwrap();
        let text = std::fs::read_to_string(root.join(".orbit/run.json")).unwrap();
        assert!(!text.contains("allowMultiple"), "{text}");
        assert!(!read_configs(&root).unwrap()[0].0.allow_multiple);
        let _ = std::fs::remove_dir_all(&root);
    }
}
