//! What Build/Run/Debug remembers across restarts, in
//! `<config dir>/native-run.json`: the chosen configuration per workspace
//! and the breakpoints (the React app keeps both in localStorage).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// A run configuration of a repo in a workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunTarget {
    pub workspace: String,
    pub repo: String,
    pub name: String,
    /// Chosen in the widget; None: the configuration's own or automatic.
    pub preset: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub selection: HashMap<String, RunTarget>,
    /// Breakpoint lines (1-based) by the file's workspace path.
    pub breakpoints: BTreeMap<PathBuf, Vec<u32>>,
}

/// A self-test keeps the user's file untouched unless ORBIT_CONFIG_DIR points elsewhere.
fn path() -> Option<PathBuf> {
    let self_test = crate::terminal::CLIPBOARD_DISABLED.load(Ordering::Relaxed);
    if self_test && std::env::var_os("ORBIT_CONFIG_DIR").is_none() {
        return None;
    }
    orbit_core::config::config_dir().ok().map(|d| d.join("native-run.json"))
}

pub fn load() -> Saved {
    path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// Read, change, write: both stores keep their part in the one file.
pub fn update(f: impl FnOnce(&mut Saved)) {
    let Some(path) = path() else { return };
    let mut saved = load();
    f(&mut saved);
    if let Ok(json) = serde_json::to_string_pretty(&saved) {
        if let Err(e) = crate::state::save(&path, &json) {
            eprintln!("saving {}: {e}", path.display());
        }
    }
}
