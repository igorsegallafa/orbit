//! Orbit's workspaces, as orbit-core lists them.
use orbit_core::workspace;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Workspace {
    pub meta: workspace::Workspace,
    pub dir: PathBuf,
}

/// Every workspace with a readable `.workspace.yaml`, by name.
pub fn list_workspaces() -> Vec<Workspace> {
    let list = workspace::list().unwrap_or_else(|e| {
        eprintln!("workspaces: {e}");
        vec![]
    });
    list.into_iter().filter_map(|meta| Some(Workspace { dir: workspace::ws_dir(&meta.name).ok()?, meta })).collect()
}
