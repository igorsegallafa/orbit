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

fn workspace_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    Ok(home.join("Documents/orbit-workspace"))
}

/// Resolves `workspace/repo/rel` and refuses paths escaping the workspace.
fn resolve(workspace: &str, repo: &str, rel: &str) -> Result<PathBuf, String> {
    let base = workspace_root()?.join("workspaces").join(workspace).join(repo);
    let joined = if rel.is_empty() {
        base.clone()
    } else {
        base.join(rel)
    };
    let normalized = normalize(&joined)?;
    if !normalized.starts_with(&workspace_root()?) {
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
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    let mut nodes: Vec<FileNode> = entries
        .flatten()
        .map(|e| {
            let p = e.path();
            FileNode {
                name: e.file_name().to_string_lossy().to_string(),
                path: p
                    .strip_prefix(workspace_root().unwrap_or_default().join("workspaces").join(&workspace).join(&repo))
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
    let ws_dir = crate::workspace::workspace_root()?
        .join("workspaces")
        .join(&workspace);
    if !ws_dir.exists() {
        return Err(format!("workspace '{workspace}' not found"));
    }
    let repos = crate::workspace::load_meta(&ws_dir)?.repos;
    let mut out: Vec<FileSearchEntry> = Vec::new();
    for repo in repos {
        let repo_dir = ws_dir.join(&repo);
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