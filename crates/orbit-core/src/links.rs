// Directory links between a worktree and its base clone (junctions on
// Windows, symlinks elsewhere), so fresh worktrees reuse node_modules and
// build output instead of reinstalling/rebuilding from scratch.
use crate::config::Service;
use std::path::{Path, PathBuf};

/// Links `link` -> `target` (creating `target` if missing). Returns false
/// when something already occupies `link`.
pub fn link_dir(target: &Path, link: &Path) -> Result<bool, String> {
    if std::fs::symlink_metadata(link).is_ok() {
        return Ok(false);
    }
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        // Junctions need no admin/developer-mode privilege, unlike symlinks.
        // mklink rejects forward slashes, which user-typed paths may carry.
        let win = |p: &Path| p.to_string_lossy().replace('/', "\\");
        let out = crate::proc::cmd("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(win(link))
            .arg(win(target))
            .output()
            .map_err(|e| format!("failed to run mklink: {e}"))?;
        if !out.status.success() {
            let msg = [out.stderr.as_slice(), out.stdout.as_slice()]
                .iter()
                .map(|b| String::from_utf8_lossy(b).trim().to_string())
                .find(|m| !m.is_empty())
                .unwrap_or_default();
            return Err(format!("mklink /J {} failed: {msg}", win(link)));
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())?;
    Ok(true)
}

/// True when `p` is a symlink or junction (never follows it).
pub fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Removes the link itself, leaving its target untouched. Real directories
/// are never removed. Returns true when a link was removed.
pub fn unlink(p: &Path) -> Result<bool, String> {
    if !is_link(p) {
        return Ok(false);
    }
    // A directory junction/symlink is removed as a directory on Windows
    // and as a file on unix; neither call descends into the target.
    #[cfg(windows)]
    std::fs::remove_dir(p).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::remove_file(p).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Worktree-relative paths `link_shared` may create for this repo.
fn shared_paths(svc: &Service, clone_dir: &Path) -> Vec<PathBuf> {
    let mut paths = svc.shared_output_paths();
    if svc.build.is_some() && clone_dir.join("package.json").exists() {
        paths.push(PathBuf::from("node_modules"));
    }
    paths
}

/// Links build output and node_modules of `wt` to the base clone. Returns
/// the paths linked (worktree-relative, `/`-separated for display).
pub fn link_shared(svc: &Service, clone_dir: &Path, wt: &Path) -> Result<Vec<String>, String> {
    let mut linked = Vec::new();
    for rel in shared_paths(svc, clone_dir) {
        if link_dir(&clone_dir.join(&rel), &wt.join(&rel))? {
            linked.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(linked)
}

/// Copies the untracked files the base clone's `.worktreeinclude` lists
/// (gitignore syntax, e.g. `.env`, `.vscode/settings.json`) into a new
/// worktree; each worktree owns its copy. Same file Claude Code and Orca read.
// ponytail: copies whatever the patterns match; a pattern like
// `node_modules` would copy the whole tree (shared paths are linked instead).
pub fn copy_worktreeinclude(clone_dir: &Path, wt: &Path) -> Vec<String> {
    if !clone_dir.join(".worktreeinclude").is_file() {
        return vec![];
    }
    let Ok(out) = crate::proc::run(
        "git",
        &["ls-files", "--others", "--ignored", "--exclude-from=.worktreeinclude"],
        Some(clone_dir),
    ) else {
        return vec![];
    };
    let mut copied = Vec::new();
    for rel in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (src, dst) = (clone_dir.join(rel), wt.join(rel));
        if dst.exists() || !src.is_file() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&src, &dst).is_ok() {
            copied.push(rel.to_string());
        }
    }
    copied
}

/// Removes the links made by `link_shared`. Must run before
/// `git worktree remove`: on Windows git follows junctions and deletes
/// the shared content inside the base clone.
pub fn unlink_shared(svc: &Service, wt: &Path) {
    let mut paths = svc.shared_output_paths();
    paths.push(PathBuf::from("node_modules"));
    for rel in paths {
        let _ = unlink(&wt.join(rel));
    }
}

/// Copies the agent config that is not inherited by walking up from the
/// workspace folder: `.mcp.json` (read from the project root only) and the
/// `.claude/` pieces (skill discovery stops at a repository root).
/// AGENTS.md/CLAUDE.md at the root ARE inherited, so they are not copied.
pub fn copy_entrypoints(root: &Path, ws_dir: &Path) -> Vec<String> {
    let mut copied = Vec::new();
    let mcp = root.join(".mcp.json");
    if mcp.is_file() && std::fs::copy(&mcp, ws_dir.join(".mcp.json")).is_ok() {
        copied.push(".mcp.json".to_string());
    }
    for name in ["skills", "rules", "hooks", "commands", "settings.json"] {
        let src = root.join(".claude").join(name);
        if src.exists() && copy_recursive(&src, &ws_dir.join(".claude").join(name)).is_ok() {
            copied.push(format!(".claude/{name}"));
        }
    }
    copied
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BuildCmd;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orbit-links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unlink_removes_only_the_link_and_keeps_shared_content() {
        let d = tmp("unlink");
        let (clone, wt) = (d.join("clone"), d.join("wt"));
        std::fs::create_dir_all(clone.join("node_modules/pkg")).unwrap();
        std::fs::write(clone.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(clone.join("package.json"), "{}").unwrap();
        std::fs::create_dir_all(&wt).unwrap();

        let mut svc = Service::new("web".into(), "o/web".into());
        svc.build = Some(BuildCmd::All("pnpm build".into()));
        let linked = link_shared(&svc, &clone, &wt).unwrap();
        assert_eq!(linked, ["dist", "node_modules"]);
        assert!(is_link(&wt.join("node_modules")));
        assert!(wt.join("node_modules/pkg/index.js").exists());

        unlink_shared(&svc, &wt);
        assert!(!wt.join("node_modules").exists());
        assert!(clone.join("node_modules/pkg/index.js").exists());
        assert!(clone.join("dist").is_dir());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn links_paths_written_with_forward_slashes() {
        let d = tmp("slashes");
        let fwd = |p: PathBuf| PathBuf::from(p.to_string_lossy().replace('\\', "/"));
        assert!(link_dir(&fwd(d.join("target")), &fwd(d.join("sub").join("link"))).unwrap());
        assert!(is_link(&d.join("sub").join("link")));
        unlink(&d.join("sub").join("link")).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn unlink_never_touches_a_real_directory() {
        let d = tmp("real");
        std::fs::create_dir_all(d.join("dist")).unwrap();
        assert!(!unlink(&d.join("dist")).unwrap());
        assert!(d.join("dist").is_dir());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn repos_without_build_link_nothing() {
        let d = tmp("nobuild");
        let svc = Service::new("x".into(), "o/x".into());
        assert!(link_shared(&svc, &d, &d.join("wt")).unwrap().is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
