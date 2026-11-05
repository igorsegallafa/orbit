// A Cargo workspace's runnable targets (bins and examples), from `cargo
// metadata`, listed as Run configurations like CMake's executables.
use crate::cmake_api::CmakeTarget;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The workspace root (cargo's messages name files relative to it) and the
/// bins and examples of its members, built in the dev profile.
pub fn parse_metadata(meta: &Value) -> Option<(PathBuf, Vec<CmakeTarget>)> {
    let root = PathBuf::from(meta.get("workspace_root")?.as_str()?);
    let target_dir = PathBuf::from(meta.get("target_directory")?.as_str()?);
    let members: Vec<&str> = meta.get("workspace_members")?.as_array()?.iter().filter_map(Value::as_str).collect();
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect()
    };
    let mut out = Vec::new();
    for p in meta.get("packages")?.as_array()? {
        if !p.get("id").and_then(Value::as_str).is_some_and(|id| members.contains(&id)) {
            continue;
        }
        let package = p.get("name").and_then(Value::as_str).unwrap_or_default();
        let defaults = strings(p.pointer("/features/default"));
        for t in p.get("targets").and_then(Value::as_array).into_iter().flatten() {
            let kinds = strings(t.get("kind"));
            let Some(kind) = ["bin", "example"].into_iter().find(|k| kinds.iter().any(|x| x == k)) else { continue };
            let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
            let dir = if kind == "example" { target_dir.join("debug/examples") } else { target_dir.join("debug") };
            out.push(CmakeTarget {
                name: name.to_string(),
                kind: kind.to_string(),
                artifact: Some(dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)).to_string_lossy().to_string()),
                package: Some(package.to_string()),
                // shortcut: only features named in `default` count as on, not ones they enable.
                features: strings(t.get("required-features")).into_iter().filter(|f| !defaults.contains(f)).collect(),
            });
        }
    }
    out.sort_by(|a, b| (a.kind != "bin", &a.name).cmp(&(b.kind != "bin", &b.name)));
    Some((root, out))
}

/// `cargo metadata` for the workspace `root` is in; None when it has no
/// Cargo.toml or cargo fails.
pub fn targets(root: &Path) -> Option<(PathBuf, Vec<CmakeTarget>)> {
    if !root.join("Cargo.toml").is_file() {
        return None;
    }
    let out = crate::proc::run("cargo", &["metadata", "--format-version", "1", "--no-deps"], Some(root)).ok()?;
    parse_metadata(&serde_json::from_str(&out).ok()?)
}

/// `cargo build` arguments for one target, or the whole workspace (None).
pub fn build_args(target: Option<&CmakeTarget>) -> Vec<String> {
    let mut args: Vec<String> = vec!["build".into()];
    match target {
        Some(t) => {
            if let Some(p) = &t.package {
                args.extend(["-p".into(), p.clone()]);
            }
            args.extend([format!("--{}", t.kind), t.name.clone()]);
            if !t.features.is_empty() {
                args.extend(["--features".into(), t.features.join(",")]);
            }
        }
        None => args.extend(["--workspace".into(), "--all-targets".into()]),
    }
    // One line per message, "file:line:col: error[E…]: text", which the problem matcher reads.
    args.push("--message-format=short".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_bins_then_examples_with_their_build_commands() {
        let meta: Value = serde_json::from_str(include_str!("fixtures/cargo-metadata.json")).unwrap();
        let (root, targets) = parse_metadata(&meta).unwrap();
        assert_eq!(root, PathBuf::from("/src/dataserver"));
        let names: Vec<_> = targets.iter().map(|t| (t.kind.as_str(), t.name.as_str())).collect();
        assert_eq!(names, [("bin", "dataserver"), ("bin", "migrate"), ("bin", "seeds"), ("bin", "truncate"), ("example", "dump")]);
        assert!(targets.iter().all(CmakeTarget::runnable));
        let exe = std::env::consts::EXE_SUFFIX;
        assert_eq!(targets[0].artifact, Some(format!("/src/dataserver/target/debug/dataserver{exe}")));
        assert_eq!(targets[4].artifact, Some(format!("/src/dataserver/target/debug/examples/dump{exe}")));
        // "server" is a default feature; "tools" is not, so the build turns it on.
        assert_eq!(build_args(Some(&targets[0])).join(" "), "build -p dataserver --bin dataserver --message-format=short");
        assert_eq!(build_args(Some(&targets[4])).join(" "), "build -p dataserver --example dump --features tools --message-format=short");
        assert_eq!(build_args(None).join(" "), "build --workspace --all-targets --message-format=short");
    }
}
