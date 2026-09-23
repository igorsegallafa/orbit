// Workspace file access for the built-in editor: tree listing + read/write.
// Paths are always resolved inside the workspace root to prevent traversal.
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Serialize)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Folder file paths are relative to: the repo's worktree (or base clone for
/// a repo scope), or the workspace root itself when `repo` is empty.
fn base_dir(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    if repo.is_empty() {
        crate::workspace::scope_dir(workspace)
    } else {
        crate::workspace::repo_path(workspace, repo)
    }
}

/// Resolves `rel` inside the repo folder and refuses paths escaping it.
fn resolve(workspace: &str, repo: &str, rel: &str) -> Result<PathBuf, String> {
    let base = normalize(&base_dir(workspace, repo)?)?;
    let joined = if rel.is_empty() {
        base.clone()
    } else {
        base.join(rel)
    };
    let normalized = normalize(&joined)?;
    if !normalized.starts_with(&base) {
        return Err("path escapes the workspace".into());
    }
    Ok(normalized)
}

fn normalize(p: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => return Err("path escapes the workspace".into()),
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn list_files(workspace: String, repo: String, path: String) -> Result<Vec<FileNode>, String> {
    let dir = resolve(&workspace, &repo, &path)?;
    let base = resolve(&workspace, &repo, "")?;
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    let mut nodes: Vec<FileNode> = entries
        .flatten()
        .map(|e| {
            let p = e.path();
            FileNode {
                name: e.file_name().to_string_lossy().to_string(),
                path: p
                    .strip_prefix(&base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string(),
                is_dir: p.is_dir(),
            }
        })
        .collect();
    nodes.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(nodes)
}

#[tauri::command]
pub async fn read_file(workspace: String, repo: String, path: String) -> Result<String, String> {
    let file = resolve(&workspace, &repo, &path)?;
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    // ponytail: 1MB cap keeps accidental binaries from freezing the webview
    if meta.len() > 1_048_576 {
        return Err("file is too large to open (1MB cap)".into());
    }
    std::fs::read_to_string(&file).map_err(|e| format!("failed to read: {e}"))
}

#[tauri::command]
pub async fn write_file(workspace: String, repo: String, path: String, content: String) -> Result<(), String> {
    let file = resolve(&workspace, &repo, &path)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, content).map_err(|e| format!("failed to write: {e}"))
}

// ---------- Tree file operations (context menu / drag & drop) ----------

fn valid_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(format!("invalid name: '{name}'"));
    }
    Ok(())
}

/// Moves `src` (file or folder, repo-relative) into `dest_dir` (repo-relative
/// folder, "" = repo root). Refuses to overwrite and to move a folder into
/// its own subtree.
#[tauri::command]
pub async fn move_file(workspace: String, repo: String, src: String, dest_dir: String) -> Result<(), String> {
    let src_path = resolve(&workspace, &repo, &src)?;
    let dest_base = resolve(&workspace, &repo, &dest_dir)?;
    if !src_path.exists() {
        return Err(format!("'{src}' does not exist"));
    }
    if !dest_base.is_dir() {
        return Err(format!("'{dest_dir}' is not a folder"));
    }
    if dest_base.starts_with(&src_path) {
        return Err("cannot move a folder into itself".into());
    }
    let name = src_path
        .file_name()
        .ok_or("source has no file name")?
        .to_string_lossy()
        .to_string();
    let dest_path = dest_base.join(&name);
    if dest_path.exists() {
        return Err(format!("'{name}' already exists in the destination folder"));
    }
    std::fs::rename(&src_path, &dest_path).map_err(|e| format!("failed to move: {e}"))
}

/// Renames a file/folder in place (same parent directory).
#[tauri::command]
pub async fn rename_node(workspace: String, repo: String, path: String, new_name: String) -> Result<(), String> {
    valid_name(&new_name)?;
    let p = resolve(&workspace, &repo, &path)?;
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    let parent = p.parent().ok_or("no parent directory")?.to_path_buf();
    let dest = parent.join(&new_name);
    if dest.exists() {
        return Err(format!("'{new_name}' already exists"));
    }
    std::fs::rename(&p, &dest).map_err(|e| format!("failed to rename: {e}"))
}

/// Deletes a file/folder by moving it to the OS Trash / Recycle Bin
/// (recoverable). Falls back to an error rather than permanent deletion.
#[tauri::command]
pub async fn delete_node(workspace: String, repo: String, path: String) -> Result<(), String> {
    let p = resolve(&workspace, &repo, &path)?;
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    trash::delete(&p).map_err(|e| format!("could not move to Trash: {e}"))
}

/// Creates an empty file or a folder inside `dir` (repo-relative, "" = root).
#[tauri::command]
pub async fn create_node(workspace: String, repo: String, dir: String, name: String, is_dir: bool) -> Result<(), String> {
    valid_name(&name)?;
    let base = resolve(&workspace, &repo, &dir)?;
    if !base.is_dir() {
        return Err(format!("'{dir}' is not a folder"));
    }
    let target = base.join(&name);
    if target.exists() {
        return Err(format!("'{name}' already exists"));
    }
    if is_dir {
        std::fs::create_dir_all(&target).map_err(|e| format!("failed to create folder: {e}"))
    } else {
        std::fs::File::create(&target)
            .map(|_| ())
            .map_err(|e| format!("failed to create file: {e}"))
    }
}

/// Reveals a file/folder in the OS file manager (selected in its parent).
#[tauri::command]
pub async fn reveal_node(workspace: String, repo: String, path: String) -> Result<(), String> {
    let p = resolve(&workspace, &repo, &path)?;
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    tauri_plugin_opener::reveal_item_in_dir(&p).map_err(|e| format!("failed to reveal: {e}"))
}

/// Absolute path of a node (for "Copy Path").
#[tauri::command]
pub async fn node_abs_path(workspace: String, repo: String, path: String) -> Result<String, String> {
    let p = resolve(&workspace, &repo, &path)?;
    Ok(p.to_string_lossy().to_string())
}

/// Copies files picked from anywhere on disk into a folder of the repo
/// ("Add Files…" in the tree context menu). Returns how many were copied.
#[tauri::command]
pub async fn import_files(
    workspace: String,
    repo: String,
    dest_dir: String,
    sources: Vec<String>,
) -> Result<usize, String> {
    let base = resolve(&workspace, &repo, &dest_dir)?;
    if !base.is_dir() {
        return Err(format!("'{dest_dir}' is not a folder"));
    }
    let mut copied = 0;
    for src in &sources {
        let p = std::path::Path::new(src);
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name() else { continue };
        let mut dest = base.join(name);
        // Don't clobber: suffix (1), (2)... like Finder does
        let mut i = 1;
        while dest.exists() {
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            dest = base.join(format!("{stem} ({i}){ext}"));
            i += 1;
        }
        std::fs::copy(p, &dest).map_err(|e| format!("failed to copy '{}': {e}", p.display()))?;
        copied += 1;
    }
    Ok(copied)
}

// ---------- Search Everywhere (double-shift) ----------

#[derive(Serialize, Clone)]
pub struct FileSearchEntry {
    pub repo: String,
    pub path: String,
}

// Directories that pollute search results (deps, build output, VCS internals).
const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".build", ".venv",
    "vendor", "__pycache__", ".next", ".cache", ".turbo", "coverage",
];

// ponytail: flat cap instead of streaming; 20k files covers every repo we
// manage and keeps the JSON payload under a few MB.
const MAX_FILES: usize = 20_000;

/// Lists every file across all repos of a workspace (skipping junk dirs).
/// The frontend fuzzy-filters this list locally for instant-as-you-type.
#[tauri::command]
pub async fn list_workspace_files(workspace: String) -> Result<Vec<FileSearchEntry>, String> {
    if !crate::workspace::scope_dir(&workspace)?.exists() {
        return Err(format!("workspace '{workspace}' not found"));
    }
    let repos = crate::workspace::scope_repos(&workspace)?;
    let mut out: Vec<FileSearchEntry> = Vec::new();
    for repo in repos {
        let repo_dir = crate::workspace::repo_path(&workspace, &repo)?;
        if repo_dir.exists() {
            collect_files(&repo_dir, "", &repo, &mut out, 0);
        }
        if out.len() >= MAX_FILES {
            break;
        }
    }
    Ok(out)
}

fn collect_files(dir: &Path, rel: &str, repo: &str, out: &mut Vec<FileSearchEntry>, depth: usize) {
    if depth > 12 || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let rel_child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            collect_files(&e.path(), &rel_child, repo, out, depth + 1);
        } else {
            out.push(FileSearchEntry {
                repo: repo.to_string(),
                path: if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") },
            });
        }
    }
}
/// Frontend render crashes (ErrorBoundary) land here so the full stack —
/// including app frames the on-screen card truncates — is diagnosable.
#[tauri::command]
pub async fn log_render_crash(message: String) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/orbit-render-crash.log")
        .map_err(|e| e.to_string())?;
    writeln!(f, "=== {} ===", chrono_or_now()).ok();
    writeln!(f, "{message}").ok();
    writeln!(f).ok();
    Ok(())
}

fn chrono_or_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}s", d.as_secs()))
        .unwrap_or_default()
}
