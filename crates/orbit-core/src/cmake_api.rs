// The targets a CMake build folder has, through the CMake File API (as Rider
// and CLion list them): a query file under `.cmake/api/v1/query/client-orbit`
// makes every configure write `reply/codemodel-v2`, read here. Replies are
// cached per build folder until a newer reply index appears.
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const QUERY: &str = r#"{ "requests": [ { "kind": "codemodel", "version": 2 } ] }"#;

/// One target of the build, for Run (executables) and Build (all of them).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CmakeTarget {
    pub name: String,
    /// EXECUTABLE, STATIC_LIBRARY, SHARED_LIBRARY, MODULE_LIBRARY,
    /// OBJECT_LIBRARY, INTERFACE_LIBRARY or UTILITY.
    pub kind: String,
    /// The first artifact, relative to the build folder ('/'-separated) when
    /// it is under it.
    pub artifact: Option<String>,
}

impl CmakeTarget {
    pub fn runnable(&self) -> bool {
        self.kind == "EXECUTABLE" && self.artifact.is_some()
    }

    /// The artifact's folder (Rider's default working directory).
    pub fn artifact_dir(&self) -> Option<String> {
        let a = self.artifact.as_deref()?;
        Some(a.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default())
    }
}

fn reply_dir(build_dir: &Path) -> PathBuf {
    build_dir.join(".cmake/api/v1/reply")
}

/// Asks CMake for the code model on its next configure; true if it was missing.
pub fn ensure_query(build_dir: &Path) -> bool {
    let dir = build_dir.join(".cmake/api/v1/query/client-orbit");
    let file = dir.join("query.json");
    if file.is_file() {
        return false;
    }
    let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, QUERY));
    true
}

/// The newest reply index (CMake names them by time, so the name sorts).
fn newest_index(build_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(reply_dir(build_dir))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("index-") && n.ends_with(".json")))
        .max()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// No reply, or one older than the last configure (CMakeCache.txt is written
/// just before the replies).
pub fn stale(build_dir: &Path) -> bool {
    let Some(index) = newest_index(build_dir) else { return true };
    match (mtime(&index), mtime(&build_dir.join("CMakeCache.txt"))) {
        (Some(reply), Some(cache)) => reply < cache,
        _ => false,
    }
}

/// The codemodel's targets from a reply index. `read` loads a reply file by
/// name. Targets CMake made itself and those of dependencies built inside
/// the build folder or a FetchContent `_deps` folder are left out.
pub fn parse_reply(index: &Value, read: impl Fn(&str) -> Option<Value>) -> Option<Vec<CmakeTarget>> {
    let ours = index.pointer("/reply/client-orbit/query.json/responses").and_then(Value::as_array);
    let codemodel_file = ours
        .into_iter()
        .flatten()
        .chain(index.get("objects").and_then(Value::as_array).into_iter().flatten())
        .find(|o| o.get("kind").and_then(Value::as_str) == Some("codemodel"))?
        .get("jsonFile")?
        .as_str()?
        .to_string();
    let model = read(&codemodel_file)?;
    let source = model.pointer("/paths/source").and_then(Value::as_str).unwrap_or_default();
    let build = model.pointer("/paths/build").and_then(Value::as_str).unwrap_or_default();
    // The build folder relative to the source folder ("build/linux-gcc"), when inside it.
    let build_in_source = Path::new(build).strip_prefix(source).ok().map(Path::to_path_buf);
    let config = model.get("configurations")?.as_array()?.first()?;
    let mut out = Vec::new();
    for t in config.get("targets").and_then(Value::as_array).into_iter().flatten() {
        let Some(target) = t.get("jsonFile").and_then(Value::as_str).and_then(&read) else { continue };
        if target.get("isGeneratorProvided").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let src = target.pointer("/paths/source").and_then(Value::as_str).unwrap_or_default();
        let src = Path::new(src);
        let dependency = src.components().any(|c| c.as_os_str() == "_deps");
        if src.is_absolute() || dependency || build_in_source.as_ref().is_some_and(|b| src.starts_with(b)) {
            continue;
        }
        let (Some(name), Some(kind)) = (target.get("name").and_then(Value::as_str), target.get("type").and_then(Value::as_str)) else {
            continue;
        };
        let artifact = target
            .pointer("/artifacts/0/path")
            .and_then(Value::as_str)
            .map(|a| a.replace('\\', "/"))
            .map(|a| match Path::new(&a).strip_prefix(build) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => a,
            });
        out.push(CmakeTarget { name: name.to_string(), kind: kind.to_string(), artifact });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

fn read_reply(build_dir: &Path, index: &Path) -> Option<Vec<CmakeTarget>> {
    let dir = reply_dir(build_dir);
    let load = |p: &Path| std::fs::read_to_string(p).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    parse_reply(&load(index)?, |name| load(&dir.join(name)))
}

struct Cached {
    index: Option<PathBuf>,
    /// CMakeCache.txt's time when a configure was last tried for a reply.
    tried: Option<SystemTime>,
    targets: Vec<CmakeTarget>,
}

static CACHE: Mutex<Option<HashMap<PathBuf, Cached>>> = Mutex::new(None);

/// The build folder's targets. A folder that was configured but has no
/// current reply is configured again through `configure` (once per
/// CMakeCache.txt), which writes one; an unconfigured one has none yet.
pub fn targets(build_dir: &Path, configure: impl FnOnce() -> bool) -> Vec<CmakeTarget> {
    let cache_file = build_dir.join("CMakeCache.txt");
    if !cache_file.is_file() {
        return vec![];
    }
    ensure_query(build_dir);
    let cache_time = mtime(&cache_file);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let mut index = newest_index(build_dir);
    if let Some(c) = map.get(build_dir).filter(|c| index.is_some() && c.index == index) {
        return c.targets.clone();
    }
    let tried = map.get(build_dir).and_then(|c| c.tried);
    let read = |index: &Option<PathBuf>| index.as_ref().and_then(|i| read_reply(build_dir, i));
    let mut found = if stale(build_dir) { None } else { read(&index) };
    if found.is_none() && tried != cache_time {
        drop(guard);
        let _ = configure();
        guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        index = newest_index(build_dir);
        found = read(&index);
    }
    let targets = found.unwrap_or_default();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(build_dir.to_path_buf(), Cached { index, tried: cache_time, targets: targets.clone() });
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<Value> {
        let raw = match name {
            "index.json" => include_str!("fixtures/cmake-reply/index.json"),
            "codemodel-v2-1.json" => include_str!("fixtures/cmake-reply/codemodel-v2-1.json"),
            "target-hello.json" => include_str!("fixtures/cmake-reply/target-hello.json"),
            "target-tool.json" => include_str!("fixtures/cmake-reply/target-tool.json"),
            "target-core.json" => include_str!("fixtures/cmake-reply/target-core.json"),
            "target-dep.json" => include_str!("fixtures/cmake-reply/target-dep.json"),
            "target-all.json" => include_str!("fixtures/cmake-reply/target-all.json"),
            _ => return None,
        };
        serde_json::from_str(raw).ok()
    }

    #[test]
    fn reads_targets_from_a_reply() {
        let index = fixture("index.json").unwrap();
        let targets = parse_reply(&index, fixture).unwrap();
        let names: Vec<_> = targets.iter().map(|t| (t.name.as_str(), t.kind.as_str(), t.artifact.as_deref())).collect();
        assert_eq!(
            names,
            [
                ("core", "STATIC_LIBRARY", Some("src/core/libcore.a")),
                ("hello", "EXECUTABLE", Some("src/hello/hello")),
                ("tool", "EXECUTABLE", Some("bin/tool")),
            ]
        );
        assert!(targets[1].runnable() && !targets[0].runnable());
        assert_eq!(targets[1].artifact_dir().as_deref(), Some("src/hello"));
    }

    #[test]
    fn query_and_staleness() {
        let dir = std::env::temp_dir().join(format!("orbit-cmake-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(targets(&dir, || panic!("not configured: nothing to run")).is_empty());
        assert!(ensure_query(&dir));
        assert!(!ensure_query(&dir));
        assert!(dir.join(".cmake/api/v1/query/client-orbit/query.json").is_file());
        assert!(stale(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
