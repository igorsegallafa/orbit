// File watching: tells the UI when a workspace's files or git state change
// on disk (agents, other editors, git in a terminal), so diffs, the git
// panel, the file tree and editors refresh on their own.
//
// One watcher per workspace, shared by every subscriber (ref-counted by
// watch / unwatch). Events are batched per repo and emitted as
// `fs-changed` once things go quiet for a moment.
//
// Linux (inotify) needs one watch per folder, so it walks the tree honoring
// .gitignore (no watches inside node_modules/target) and adds folders as
// they appear. macOS and Windows watch the root recursively, for free.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;

use crate::events::Events;
use crate::workspace;

/// Quiet time before a batch is emitted, and the longest a batch can wait
/// while changes keep coming (an agent writing many files).
const QUIET: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_millis(1000);
/// Paths listed per event before it collapses to "everything changed".
const MAX_PATHS: usize = 200;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FsChange {
    pub workspace: String,
    pub repo: String,
    /// Relative to the repo root, "/" separated; empty = refresh everything.
    pub paths: Vec<String>,
    /// Git state moved (HEAD or index): commit, checkout, stage, reset.
    pub git: bool,
}

struct Root {
    repo: String,
    dir: PathBuf,
    git_dir: Option<PathBuf>,
    ignore: Gitignore,
}

struct Entry {
    refs: usize,
    // Kept alive here; the batching thread only holds a Weak to it, so
    // dropping the entry stops the watcher and ends the thread.
    _watcher: Arc<Mutex<RecommendedWatcher>>,
}

fn entries() -> &'static Mutex<HashMap<String, Entry>> {
    static E: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    E.get_or_init(Default::default)
}

/// Subscribes to a workspace's `fs-changed` events (ref-counted).
pub fn watch(events: Events, workspace: &str) -> Result<(), String> {
    let mut map = entries().lock().map_err(|e| e.to_string())?;
    if let Some(e) = map.get_mut(workspace) {
        e.refs += 1;
        return Ok(());
    }
    let watcher = start(events, workspace)?;
    map.insert(workspace.to_string(), Entry { refs: 1, _watcher: watcher });
    Ok(())
}

pub fn unwatch(workspace: &str) -> Result<(), String> {
    let mut map = entries().lock().map_err(|e| e.to_string())?;
    if let Some(e) = map.get_mut(workspace) {
        e.refs = e.refs.saturating_sub(1);
        if e.refs == 0 {
            map.remove(workspace);
        }
    }
    Ok(())
}

/// Stops watching a workspace regardless of subscribers (it's being removed).
pub fn forget(workspace: &str) {
    if let Ok(mut map) = entries().lock() {
        map.remove(workspace);
    }
}

fn roots_of(ws: &str) -> Result<Vec<Root>, String> {
    let repos: Vec<String> = match workspace::repo_scope(ws) {
        Some(r) => vec![r.to_string()],
        None => workspace::load_meta(&workspace::ws_dir(ws)?)?.repos,
    };
    let mut out = Vec::new();
    for repo in repos {
        let Ok(path) = workspace::repo_path(ws, &repo) else { continue };
        // Branch-only repos are a link into the base clone: watch the target.
        let Ok(dir) = std::fs::canonicalize(&path) else { continue };
        let git_dir = crate::proc::run("git", &["rev-parse", "--absolute-git-dir"], Some(&dir))
            .ok()
            .map(|s| PathBuf::from(s.trim()))
            .and_then(|p| std::fs::canonicalize(p).ok());
        let mut gi = GitignoreBuilder::new(&dir);
        gi.add(dir.join(".gitignore"));
        let ignore = gi.build().unwrap_or_else(|_| Gitignore::empty());
        out.push(Root { repo, dir, git_dir, ignore });
    }
    Ok(out)
}

/// True for paths no view cares about: git internals and ignored folders.
fn skipped(root: &Root, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(&root.dir) else { return true };
    if rel.components().any(|c| {
        let s = c.as_os_str();
        s == ".git" || s == "node_modules"
    }) {
        return true;
    }
    root.ignore.matched_path_or_any_parents(rel, path.is_dir()).is_ignore()
}

/// Folders of `dir` to watch one by one (inotify), skipping ignored ones.
fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
    ignore::WalkBuilder::new(dir)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != "node_modules")
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_dir()))
        .map(|e| e.into_path())
        .collect()
}

const PER_DIR: bool = cfg!(target_os = "linux");

fn add_tree(w: &mut RecommendedWatcher, dir: &Path) {
    if PER_DIR {
        for d in walk_dirs(dir) {
            let _ = w.watch(&d, RecursiveMode::NonRecursive);
        }
    } else {
        let _ = w.watch(dir, RecursiveMode::Recursive);
    }
}

fn start(events: Events, ws: &str) -> Result<Arc<Mutex<RecommendedWatcher>>, String> {
    let roots = roots_of(ws)?;
    let (tx, rx) = channel::<notify::Event>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let _ = tx.send(ev);
        }
    })
    .map_err(|e| format!("couldn't start file watcher: {e}"))?;
    let watcher = Arc::new(Mutex::new(watcher));
    {
        let mut w = watcher.lock().map_err(|e| e.to_string())?;
        for r in &roots {
            add_tree(&mut w, &r.dir);
            if let Some(g) = &r.git_dir {
                // HEAD and index live here; logs/HEAD grows on every commit.
                let _ = w.watch(g, RecursiveMode::NonRecursive);
                let _ = w.watch(&g.join("logs"), RecursiveMode::NonRecursive);
            }
        }
    }
    // REVIEW.md sits outside the repos: watch its folder (not recursively).
    let notes = crate::notes::notes_path(ws).ok();
    if let Some(dir) = notes.as_ref().and_then(|n| n.parent()) {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(mut w) = watcher.lock() {
            let _ = w.watch(dir, RecursiveMode::NonRecursive);
        }
    }
    let weak = Arc::downgrade(&watcher);
    let emit = move |c: FsChange| {
        let _ = events.emit("fs-changed", c);
    };
    let ws = ws.to_string();
    std::thread::spawn(move || batch_loop(emit, ws, roots, notes, rx, weak));
    Ok(watcher)
}

#[derive(Default)]
struct Pending {
    paths: HashSet<String>,
    overflow: bool,
    git: bool,
}

fn batch_loop(
    emit: impl Fn(FsChange),
    ws: String,
    roots: Vec<Root>,
    notes: Option<PathBuf>,
    rx: Receiver<notify::Event>,
    watcher: Weak<Mutex<RecommendedWatcher>>,
) {
    let mut pending: HashMap<usize, Pending> = HashMap::new();
    let mut first: Option<Instant> = None;
    loop {
        let msg = if first.is_some() { rx.recv_timeout(QUIET) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match msg {
            Ok(ev) => {
                if matches!(ev.kind, EventKind::Access(_)) {
                    continue;
                }
                let new_dir = matches!(ev.kind, EventKind::Create(notify::event::CreateKind::Folder))
                    || (matches!(ev.kind, EventKind::Create(_)) && ev.paths.iter().any(|p| p.is_dir()));
                for path in &ev.paths {
                    if notes.as_deref() == Some(path.as_path()) {
                        pending.entry(NOTES).or_default().paths.insert(crate::notes::FILE.to_string());
                        continue;
                    }
                    if classify(&roots, path, &mut pending) && new_dir && PER_DIR {
                        if let Some(w) = watcher.upgrade() {
                            if let Ok(mut w) = w.lock() {
                                add_tree(&mut w, path);
                            }
                        }
                    }
                }
                if !pending.is_empty() && first.is_none() {
                    first = Some(Instant::now());
                }
                if first.is_some_and(|t| t.elapsed() >= MAX_WAIT) {
                    flush(&emit, &ws, &roots, &mut pending);
                    first = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush(&emit, &ws, &roots, &mut pending);
                first = None;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Files `path` under its root; returns true when it's a watched work-tree path.
fn classify(roots: &[Root], path: &Path, pending: &mut HashMap<usize, Pending>) -> bool {
    for (i, r) in roots.iter().enumerate() {
        if let Some(g) = &r.git_dir {
            if path.starts_with(g) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "HEAD" || name == "index" {
                    pending.entry(i).or_default().git = true;
                }
                return false;
            }
        }
        if path.starts_with(&r.dir) {
            if skipped(r, path) {
                return false;
            }
            let p = pending.entry(i).or_default();
            if p.paths.len() >= MAX_PATHS {
                p.overflow = true;
            } else if let Ok(rel) = path.strip_prefix(&r.dir) {
                p.paths.insert(rel.to_string_lossy().replace('\\', "/"));
            }
            return true;
        }
    }
    false
}

/// Pending slot for REVIEW.md, reported with an empty repo.
const NOTES: usize = usize::MAX;

fn flush(emit: &impl Fn(FsChange), ws: &str, roots: &[Root], pending: &mut HashMap<usize, Pending>) {
    for (i, p) in pending.drain() {
        if i == NOTES {
            emit(FsChange { workspace: ws.to_string(), repo: String::new(), paths: vec![crate::notes::FILE.to_string()], git: false });
            continue;
        }
        let mut paths: Vec<String> = if p.overflow { vec![] } else { p.paths.into_iter().collect() };
        paths.sort();
        emit(FsChange { workspace: ws.to_string(), repo: roots[i].repo.clone(), paths, git: p.git });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(dir: &Path) -> Root {
        let mut gi = GitignoreBuilder::new(dir);
        gi.add_line(None, "target/").unwrap();
        gi.add_line(None, "*.log").unwrap();
        Root { repo: "api".into(), dir: dir.to_path_buf(), git_dir: Some(dir.join(".gitdir")), ignore: gi.build().unwrap() }
    }

    #[test]
    fn classifies_work_tree_and_git_paths() {
        let base = PathBuf::from("/tmp/orbit-watch-test/ws/api");
        let roots = vec![root(&base)];
        let mut pending = HashMap::new();
        assert!(classify(&roots, &base.join("src/main.rs"), &mut pending));
        assert!(!classify(&roots, &base.join("target/debug/x"), &mut pending));
        assert!(!classify(&roots, &base.join("app.log"), &mut pending));
        assert!(!classify(&roots, &base.join("node_modules/a/b.js"), &mut pending));
        assert!(!classify(&roots, &base.join(".gitdir/index"), &mut pending));
        assert!(!classify(&roots, &base.join(".gitdir/index.lock"), &mut pending));
        let p = &pending[&0];
        assert!(p.git);
        assert_eq!(p.paths.iter().collect::<Vec<_>>(), vec!["src/main.rs"]);
    }

    /// End to end on the real file system: a new folder gets watched and
    /// writes inside it and in ignored folders are reported (or not).
    #[test]
    fn reports_real_changes_in_batches() {
        let dir = std::env::temp_dir().join(format!("orbit-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut gi = GitignoreBuilder::new(&dir);
        gi.add(dir.join(".gitignore"));
        let roots = vec![Root { repo: "api".into(), dir: dir.clone(), git_dir: None, ignore: gi.build().unwrap() }];

        let (tx, rx) = channel::<notify::Event>();
        let watcher = Arc::new(Mutex::new(
            notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
                if let Ok(e) = r {
                    let _ = tx.send(e);
                }
            })
            .unwrap(),
        ));
        add_tree(&mut watcher.lock().unwrap(), &dir);
        let (out_tx, out_rx) = channel::<FsChange>();
        let weak = Arc::downgrade(&watcher);
        std::thread::spawn(move || batch_loop(move |c| { let _ = out_tx.send(c); }, "ws".into(), roots, None, rx, weak));

        std::fs::write(dir.join("src/a.rs"), "x").unwrap();
        std::fs::write(dir.join("target/junk"), "x").unwrap();
        let c = out_rx.recv_timeout(Duration::from_secs(3)).expect("first batch");
        assert_eq!(c.paths, vec!["src/a.rs"]);

        // A folder created after start is picked up (inotify needs a new watch).
        std::fs::create_dir_all(dir.join("src/new")).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let _ = out_rx.try_iter().count();
        std::fs::write(dir.join("src/new/b.rs"), "x").unwrap();
        let c = out_rx.recv_timeout(Duration::from_secs(3)).expect("second batch");
        assert!(c.paths.contains(&"src/new/b.rs".to_string()), "{:?}", c.paths);

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
