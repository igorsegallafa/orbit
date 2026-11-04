//! What the app restores on start: the active scope (a workspace, or none),
//! each scope's tabs and last active tab, and the pane sizes. Kept in `<orbit config dir>/native-state.json`,
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

impl SavedTab {
    /// The tab's key in the app (`Tab::key`).
    pub fn key(&self) -> String {
        match self {
            SavedTab::Editor { path, .. } => format!("ed:{}", path.display()),
            SavedTab::Diff { path } => format!("diff:{}", path.display()),
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            SavedTab::Editor { path, .. } | SavedTab::Diff { path } => path,
        }
    }
}

/// One scope's tabs: a workspace's, or the global scope's (`workspace: None`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedScope {
    pub workspace: Option<String>,
    pub tabs: Vec<SavedTab>,
    /// The scope's last active tab, by key (sessions: `session:<id>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    /// The active scope's workspace (None: the global scope).
    pub workspace: Option<String>,
    pub scopes: Vec<SavedScope>,
    /// Before scopes: one global tab list and an index into it. Read only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<SavedTab>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    pub layout: Layout,
    /// Theme id from `theme::list()`, or "system" (`orbit.theme`).
    pub theme: Option<String>,
    /// Desktop notifications (Settings → Notifications); None is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
    /// Terminals inhibit GNOME's shortcuts (Settings → Keyboard); None is the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_shortcuts: Option<bool>,
    /// The toast about GNOME's permission prompt was shown.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shortcuts_explained: bool,
    /// Workspaces whose sessions are collapsed in the sidebar (`orbit.ws-expanded` false).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ws_collapsed: Vec<String>,
}

impl SavedState {
    /// Moves an old global tab list into scopes, by `scope_of` each tab's
    /// path. The old active tab's scope becomes the active one.
    pub fn migrate(mut self, scope_of: impl Fn(&Path) -> Option<String>) -> Self {
        let tabs = std::mem::take(&mut self.tabs);
        let active = self.active.take().and_then(|a| tabs.get(a)).map(|t| (scope_of(t.path()), t.key()));
        if tabs.is_empty() || !self.scopes.is_empty() {
            return self;
        }
        for tab in tabs {
            let ws = scope_of(tab.path());
            match self.scopes.iter_mut().find(|s| s.workspace == ws) {
                Some(s) => s.tabs.push(tab),
                None => self.scopes.push(SavedScope { workspace: ws, tabs: vec![tab], active: None }),
            }
        }
        if let Some((ws, key)) = active {
            if let Some(s) = self.scopes.iter_mut().find(|s| s.workspace == ws) {
                s.active = Some(key);
            }
            self.workspace = ws;
        }
        self
    }
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
            scopes: vec![
                SavedScope {
                    workspace: Some("ws".into()),
                    tabs: vec![
                        SavedTab::Editor { path: "/a/b.rs".into(), preview: true, cursor: Some((3, 4)) },
                        SavedTab::Diff { path: "/a/b.rs".into() },
                    ],
                    active: Some("session:tm:1".into()),
                },
                SavedScope { workspace: None, tabs: vec![SavedTab::Diff { path: "/c.rs".into() }], active: None },
            ],
            tabs: vec![],
            active: None,
            layout: Layout { sidebar_width: 300., ..Default::default() },
            theme: Some("nord".into()),
            notifications: Some(false),
            terminal_shortcuts: Some(false),
            shortcuts_explained: true,
            ws_collapsed: vec!["ws".into()],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<SavedState>(&json).unwrap(), s);
    }

    #[test]
    fn migrates_the_global_tab_list() {
        let old = r#"{"workspace":"a","tabs":[
            {"kind":"editor","path":"/ws/a/r/x.rs"},
            {"kind":"editor","path":"/home/u/notes.md"},
            {"kind":"diff","path":"/ws/b/r/y.rs"},
            {"kind":"editor","path":"/ws/a/r/z.rs"}],"active":2}"#;
        let s: SavedState = serde_json::from_str(old).unwrap();
        let scope_of = |p: &Path| ["a", "b"].iter().find(|w| p.starts_with(format!("/ws/{w}"))).map(|w| w.to_string());
        let s = s.migrate(scope_of);
        assert!(s.tabs.is_empty() && s.active.is_none());
        let scopes: Vec<_> = s.scopes.iter().map(|s| (s.workspace.clone(), s.tabs.len())).collect();
        assert_eq!(scopes, vec![(Some("a".into()), 2), (None, 1), (Some("b".into()), 1)]);
        // The old active tab picks the active scope.
        assert_eq!(s.workspace.as_deref(), Some("b"));
        assert_eq!(s.scopes[2].active.as_deref(), Some("diff:/ws/b/r/y.rs"));
        // Already migrated: unchanged, and the old fields aren't written.
        assert_eq!(s.clone().migrate(scope_of), s);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("\"active\":2") && json.matches("\"tabs\"").count() == 3, "{json}");
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
