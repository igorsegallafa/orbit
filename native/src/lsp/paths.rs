//! `file:` URIs and the mapping between a repo's workspace path and its real path.
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn path_to_uri(path: &Path) -> Option<lsp_types::Uri> {
    url::Url::from_file_path(path).ok()?.as_str().parse().ok()
}

pub fn uri_to_path(uri: &lsp_types::Uri) -> Option<PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

/// `path` moved from under `from` to under `to`, or None when it's not inside `from`.
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) })
}

/// Every `file:` URI inside a JSON-RPC payload, moved from one root to the other.
pub fn map_uris(value: Value, from: &Path, to: &Path) -> Value {
    if from == to {
        return value;
    }
    match value {
        Value::String(s) if s.starts_with("file://") => {
            let moved = url::Url::parse(&s)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .and_then(|p| rebase(&p, from, to))
                .and_then(|p| url::Url::from_file_path(p).ok());
            Value::String(moved.map_or(s, |u| u.to_string()))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(|v| map_uris(v, from, to)).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, map_uris(v, from, to))).collect()),
        other => other,
    }
}

/// `p` with symlinks resolved, or `p` itself when that fails.
pub fn real_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The compilation database clangd should use: at the root, in `build/`, or
/// in one level of the usual CMake binary dirs (build/<preset>, out/build/…,
/// cmake-build-*), newest first. Same rules as `src-tauri/src/lsp.rs`.
pub fn find_compile_commands(root: &Path) -> Option<PathBuf> {
    let direct = [root.join("compile_commands.json"), root.join("build").join("compile_commands.json")];
    let mut found: Vec<PathBuf> = direct.into_iter().filter(|p| p.is_file()).collect();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("cmake-build-") {
            found.extend(Some(e.path().join("compile_commands.json")).filter(|p| p.is_file()));
        }
    }
    for parent in [root.join("build"), root.join("out"), root.join("out").join("build")] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_nested_uris_between_roots() {
        let v = json!({
            "uri": "file:///ws/feat/engine/src/a.cpp",
            "list": [{ "targetUri": "file:///ws/feat/engine/b.h" }, "file:///elsewhere/c.h", 3],
        });
        let out = map_uris(v, Path::new("/ws/feat/engine"), Path::new("/real/engine"));
        assert_eq!(out["uri"], "file:///real/engine/src/a.cpp");
        assert_eq!(out["list"][0]["targetUri"], "file:///real/engine/b.h");
        assert_eq!(out["list"][1], "file:///elsewhere/c.h");
        assert_eq!(out["list"][2], 3);
    }

    #[test]
    fn does_not_rebase_sibling_prefixes() {
        assert_eq!(rebase(Path::new("/a/engine2/x"), Path::new("/a/engine"), Path::new("/b")), None);
        assert_eq!(rebase(Path::new("/a/engine/x"), Path::new("/a/engine"), Path::new("/b")), Some(PathBuf::from("/b/x")));
    }

    #[test]
    fn uris_round_trip_with_spaces() {
        let p = Path::new("/tmp/with space/a.cpp");
        let uri = path_to_uri(p).unwrap();
        assert!(uri.as_str().contains("%20"));
        assert_eq!(uri_to_path(&uri).unwrap(), p);
    }
}
