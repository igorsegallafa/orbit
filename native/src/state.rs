//! What the app restores on start: the open workspace, its tabs, the active
//! tab and the pane sizes. Kept in `<orbit config dir>/native-state.json`,
//! next to orbit-core's `config.yaml` (the Tauri app keeps the same things in
//! its webview's localStorage).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Pane sizes in px, with the React app's defaults and limits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub sidebar_width: f32,
    pub sidebar_hidden: bool,
    pub dock_width: f32,
    pub dock_hidden: bool,
    pub panel_height: f32,
    pub panel_open: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_width: SIDEBAR.default,
            sidebar_hidden: false,
            dock_width: DOCK.default,
            dock_hidden: false,
            panel_height: PANEL.default,
            panel_open: false,
        }
    }
}

pub struct Limits {
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

/// `SidebarResizer.tsx`: 64–400, snapping to the icon-only 64 below 110.
pub const SIDEBAR: Limits = Limits { default: 220., min: 64., max: 400. };
pub const SIDEBAR_SNAP: f32 = 110.;
/// `App.tsx` dock (the files panel on the right).
pub const DOCK: Limits = Limits { default: 240., min: 180., max: 460. };
/// `BottomPanel.tsx`: at least 120, and 200 px left for the rest.
pub const PANEL: Limits = Limits { default: 240., min: 120., max: 2000. };

impl Limits {
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_finite() {
            v.clamp(self.min, self.max)
        } else {
            self.default
        }
    }
}

impl Layout {
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_width <= SIDEBAR.min + 1.
    }

    /// A dragged sidebar width: below the snap threshold it collapses to icons.
    pub fn snap_sidebar(w: f32) -> f32 {
        if w < SIDEBAR_SNAP {
            SIDEBAR.min
        } else {
            SIDEBAR.clamp(w)
        }
    }

    /// Values out of range (an edited file) fall back into range.
    pub fn sanitized(mut self) -> Self {
        self.sidebar_width = SIDEBAR.clamp(self.sidebar_width);
        self.dock_width = DOCK.clamp(self.dock_width);
        self.panel_height = PANEL.clamp(self.panel_height);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SavedTab {
    Editor {
        path: PathBuf,
        #[serde(default)]
        preview: bool,
        /// 1-based line and column.
        #[serde(default)]
        cursor: Option<(u32, u32)>,
    },
    Diff {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub workspace: Option<String>,
    pub tabs: Vec<SavedTab>,
    /// Index into `tabs`.
    pub active: Option<usize>,
    pub layout: Layout,
    /// Theme id from `theme::list()`, or "system" (`orbit.theme`).
    pub theme: Option<String>,
    /// Workspaces whose sessions are collapsed in the sidebar (`orbit.ws-expanded` false).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ws_collapsed: Vec<String>,
}

pub fn default_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("native-state.json"))
}

/// A missing or unreadable file is a fresh start.
pub fn load(path: &Path) -> SavedState {
    let Ok(text) = std::fs::read_to_string(path) else { return SavedState::default() };
    match serde_json::from_str::<SavedState>(&text) {
        Ok(mut s) => {
            s.layout = s.layout.sanitized();
            s
        }
        Err(e) => {
            eprintln!("ignoring {}: {e}", path.display());
            SavedState::default()
        }
    }
}

/// Written through a temporary file, so a crash never leaves half a file.
pub fn save(path: &Path, json: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = SavedState {
            workspace: Some("ws".into()),
            tabs: vec![
                SavedTab::Editor { path: "/a/b.rs".into(), preview: true, cursor: Some((3, 4)) },
                SavedTab::Diff { path: "/a/b.rs".into() },
            ],
            active: Some(1),
            layout: Layout { sidebar_width: 300., ..Default::default() },
            theme: Some("nord".into()),
            ws_collapsed: vec!["ws".into()],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<SavedState>(&json).unwrap(), s);
    }

    #[test]
    fn partial_and_bad_values() {
        let s: SavedState = serde_json::from_str(r#"{"layout":{"dock_width":9999}}"#).unwrap();
        assert_eq!(s.layout.clone().sanitized().dock_width, DOCK.max);
        assert_eq!(s.layout.sidebar_width, SIDEBAR.default);
        let dir = std::env::temp_dir().join(format!("orbit-state-{}", std::process::id()));
        let path = dir.join("s.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), SavedState::default());
        save(&path, r#"{"workspace":"x"}"#).unwrap();
        assert_eq!(load(&path).workspace.as_deref(), Some("x"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidebar_snaps() {
        assert_eq!(Layout::snap_sidebar(100.), SIDEBAR.min);
        assert_eq!(Layout::snap_sidebar(150.), 150.);
        assert_eq!(Layout::snap_sidebar(900.), SIDEBAR.max);
    }
}
