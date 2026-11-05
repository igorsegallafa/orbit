//! The file tree's ties to the app, installed when the root view is created:
//! its context menu, the workspace's file watcher (the tree and unmodified
//! editors follow disk changes), the tree following the active file, and
//! revealing folders from the editor's breadcrumb.
use crate::app::{OrbitApp, TabKind};
use crate::commands::{self, Command};
use crate::file_tree::{PromptKind, TreeRequest};
use crate::overlay::MenuItem;
use crate::toast::ToastKind;
use futures::StreamExt;
use gpui::{App, AppContext, Context, Focusable, Global, Pixels, Point, WeakEntity, Window};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The app, for code that has no handle on it (the breadcrumb).
struct AppLink(WeakEntity<OrbitApp>);

impl Global for AppLink {}

/// `orbit_core::watch`'s `fs-changed` payload.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct FsChange {
    workspace: String,
    repo: String,
    paths: Vec<String>,
    git: bool,
}

#[derive(Default)]
struct Sync {
    watched: Option<String>,
    followed: Option<PathBuf>,
}

pub fn init(cx: &mut App) {
    let has_editor = |a: &OrbitApp, _: &App| a.active_editor().is_some();
    commands::register(
        cx,
        Command::new("files.revealActive", "Select Opened File in Tree").when(has_editor).run(|a, w, cx| {
            if let Some(path) = a.active_editor().map(|e| e.read(cx).path.clone()) {
                reveal(a, &path, w, cx);
            }
        }),
    );
    commands::register(cx, Command::new("files.refresh", "Reload Files from Disk").when(|a, _| a.workspace().is_some()).run(|a, _, cx| {
        a.tree.update(cx, |t, cx| t.refresh(None, cx));
    }));
    cx.observe_new(|app: &mut OrbitApp, window, cx| {
        if let Some(window) = window {
            attach(app, window, cx);
        }
    })
    .detach();
}

fn attach(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let link = AppLink(cx.weak_entity());
    cx.set_global(link);
    cx.subscribe_in(&app.tree, window, |app, _, request: &TreeRequest, window, cx| match request {
        TreeRequest::ContextMenu { path, position } => open_menu(app, path, *position, window, cx),
        TreeRequest::Notify { error, title, detail } => {
            let kind = if *error { ToastKind::Error } else { ToastKind::Success };
            app.toast(kind, title.clone(), detail.clone().map(Into::into), cx);
        }
        TreeRequest::Renamed { from, to } => app.file_renamed(from, to, window, cx),
    })
    .detach();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<FsChange>();
    cx.spawn_in(window, async move |this, cx| {
        while let Some(change) = rx.next().await {
            if this.update_in(cx, |app, window, cx| on_fs_change(app, change, window, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    // Workspace switches move the watcher; tab switches move the tree's selection.
    let state = Rc::new(RefCell::new(Sync::default()));
    let entity = cx.entity();
    cx.observe_in(&entity, window, {
        let state = state.clone();
        let tx = tx.clone();
        move |app, _, _, cx| sync(app, &state, &tx, cx)
    })
    .detach();
    sync(app, &state, &tx, cx);
}

/// Runs on every notify of the app: cheap unless something moved. Must not notify the app.
fn sync(app: &mut OrbitApp, state: &Rc<RefCell<Sync>>, tx: &futures::channel::mpsc::UnboundedSender<FsChange>, cx: &mut Context<OrbitApp>) {
    let mut s = state.borrow_mut();
    let ws = app.workspace().map(|w| w.meta.name.clone());
    if ws != s.watched {
        let old = s.watched.take();
        app.tree.update(cx, |t, _| t.set_workspace(ws.clone()));
        let tx = tx.clone();
        let new = ws.clone();
        // Linux watches folder by folder: the walk runs off the UI thread.
        cx.background_spawn(async move {
            if let Some(old) = old {
                let _ = orbit_core::watch::unwatch(&old);
            }
            let Some(name) = new else { return };
            let events = orbit_core::events::Events::new(move |event: &str, payload: serde_json::Value| {
                if event == "fs-changed" {
                    if let Ok(change) = serde_json::from_value::<FsChange>(payload) {
                        let _ = tx.unbounded_send(change);
                    }
                }
            });
            match orbit_core::watch::watch(events, &name) {
                Ok(()) => eprintln!("watching {name}"),
                Err(e) => eprintln!("watching {name}: {e}"),
            }
        })
        .detach();
        s.watched = ws;
        s.followed = None;
    }
    let active = app.active_editor().map(|e| e.read(cx).path.clone());
    if active != s.followed {
        s.followed = active.clone();
        if let Some(path) = active {
            app.tree.update(cx, |t, cx| t.reveal(&path, cx));
        }
    }
}

/// Build output and caches: never worth a refresh (the Tauri app skips them in its file list too).
fn is_noise(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or_default();
    first == "build" || first.starts_with("build-") || first.starts_with("build_") || rel.split('/').any(|p| p == ".cache")
}

fn on_fs_change(app: &mut OrbitApp, change: FsChange, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(ws) = app.workspace().filter(|w| w.meta.name == change.workspace) else { return };
    if change.repo.is_empty() || !ws.meta.repos.contains(&change.repo) {
        return;
    }
    let root = ws.dir.join(&change.repo);
    let everything = change.git || change.paths.is_empty();
    let paths: Vec<PathBuf> = change.paths.iter().filter(|p| !is_noise(p)).map(|p| root.join(p)).collect();
    if !everything && paths.is_empty() {
        return;
    }
    eprintln!("fs change in {}: {} path(s){}", change.repo, paths.len(), if everything { ", everything" } else { "" });
    let dirs: Option<Vec<PathBuf>> = (!everything).then(|| {
        let set: HashSet<PathBuf> = paths.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
        set.into_iter().collect()
    });
    app.tree.update(cx, |t, cx| t.refresh(dirs.as_deref(), cx));
    let changed: HashSet<&PathBuf> = paths.iter().collect();
    let editors: Vec<_> = app
        .tabs
        .iter()
        .filter_map(|t| match &t.kind {
            TabKind::Editor(e) => Some(e.clone()),
            _ => None,
        })
        .filter(|e| {
            let path = &e.read(cx).path;
            if everything { path.starts_with(&root) } else { changed.contains(path) }
        })
        .collect();
    for editor in editors {
        if editor.update(cx, |e, cx| e.reload_from_disk(window, cx)) {
            eprintln!("reloaded {}", editor.read(cx).path.display());
        }
        if change.git {
            editor.update(cx, |e, cx| e.reload_git_bases(cx));
        }
    }
}

/// Shows the tree (and the files panel), opens the folders down to `path` and selects it.
fn reveal(app: &mut OrbitApp, path: &Path, window: &mut Window, cx: &mut Context<OrbitApp>) {
    if app.layout.dock_hidden {
        app.toggle_dock(cx);
    }
    let tree = app.tree.clone();
    tree.update(cx, |t, cx| t.reveal(path, cx));
    window.focus(&tree.focus_handle(cx), cx);
}

/// The breadcrumb's folders.
pub fn reveal_in_tree(path: PathBuf, window: &mut Window, cx: &mut App) {
    let Some(app) = cx.try_global::<AppLink>().and_then(|l| l.0.upgrade()) else { return };
    app.update(cx, |app, cx| reveal(app, &path, window, cx));
}

/// The tree's context menu (`FileTreePanel.tsx`'s items, plus copy relative path and open in terminal).
fn open_menu(app: &mut OrbitApp, path: &Path, position: Point<Pixels>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let tree = app.tree.read(cx);
    let (is_dir, is_root) = (tree.is_dir(path), tree.is_root(path));
    let on_tree = |f: fn(&mut crate::file_tree::FileTree, PathBuf, &mut Window, &mut Context<crate::file_tree::FileTree>)| {
        let path = path.to_path_buf();
        move |a: &mut OrbitApp, w: &mut Window, cx: &mut Context<OrbitApp>| a.tree.update(cx, |t, cx| f(t, path.clone(), w, cx))
    };
    let mut items = vec![];
    if is_dir {
        items.push(MenuItem::new("New File…", on_tree(|t, p, w, cx| t.start_prompt(PromptKind::NewFile(p), w, cx))));
        items.push(MenuItem::new("New Folder…", on_tree(|t, p, w, cx| t.start_prompt(PromptKind::NewFolder(p), w, cx))));
    } else {
        let p = path.to_path_buf();
        items.push(MenuItem::new("Open", move |a, w, cx| {
            a.open_file(p.clone(), false, w, cx);
        }));
    }
    if !is_root {
        items.push(MenuItem::new("Rename…", on_tree(|t, p, w, cx| t.start_prompt(PromptKind::Rename(p), w, cx))));
    }
    items.push(MenuItem::new("Copy Path", on_tree(|t, p, _, cx| t.copy_path(&p, false, cx))));
    if !is_root {
        items.push(MenuItem::new("Copy Relative Path", on_tree(|t, p, _, cx| t.copy_path(&p, true, cx))));
    }
    items.push(MenuItem::new("Reveal in File Manager", on_tree(|t, p, _, cx| t.reveal_in_file_manager(&p, cx))));
    let dir = if is_dir { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    items.push(MenuItem::new("Open in Terminal", move |a, w, cx| crate::sessions::new_shell_in(a, &dir, w, cx)));
    if !is_root {
        items.push(MenuItem::new("Delete", on_tree(|t, p, _, cx| t.delete(p, cx))).danger());
    }
    app.open_menu(position, items, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_output_and_caches_are_noise() {
        assert!(is_noise("build/linux/foo.o"));
        assert!(is_noise("build-debug/x"));
        assert!(is_noise(".cache/clangd/index/a.idx"));
        assert!(is_noise("src/.cache/x"));
        assert!(!is_noise("src/build.rs"));
        assert!(!is_noise("build.rs"));
        assert!(!is_noise("src/game/Game.cpp"));
    }
}
