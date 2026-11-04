//! The window's root view and its state: workspaces, tabs, panes, navigation
//! history, overlays and what is restored on start. Rendering is in
//! `shell.rs`. Tabs keep the React app's order: sessions (terminals) first,
//! then files after a divider, with a VS Code-style preview tab.
use crate::commands::{self, Command};
use crate::diff_view::DiffTab;
use crate::editor::{self, EditorEvent, EditorTab};
use crate::file_tree::{FileTree, FileTreeEvent};
use crate::lsp::{uri_to_path, LspRegistry, Notification, RepoKey};
use crate::nav::NavHistory;
use crate::overlay::{ButtonStyle, ContextMenu, Dialog, DialogButton, MenuItem};
use crate::palette::Palette;
use crate::state::{self, Layout, SavedState, SavedTab};
use crate::terminal::TerminalView;
use crate::toast::{ToastKind, Toasts};
use crate::workspace::{self, Workspace};
use futures::StreamExt;
use gpui::{
    px, App, AnyView, AppContext, Context, Entity, FocusHandle, Focusable, Pixels, Point, SharedString, Subscription,
    Task, Window,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub enum TabKind {
    Terminal(Entity<TerminalView>),
    Editor(Entity<EditorTab>),
    Diff(Entity<DiffTab>),
    Review(Entity<crate::review::ReviewTab>),
    Page(Entity<crate::workspace_ui::PageTab>),
    Tool(Entity<crate::pages::PageTab>),
}

pub struct Tab {
    pub kind: TabKind,
    /// Opened with a single click: the next such file replaces it.
    pub preview: bool,
}

/// Where a history entry leads back to.
#[derive(Clone, Debug, PartialEq)]
pub enum NavTarget {
    Editor(PathBuf),
    Diff(PathBuf),
    /// Review pages reopen from the sidebar; like terminals, closed ones are skipped.
    Review,
    /// Closed terminals are skipped: reopening one would be a new session.
    Terminal,
}

impl Tab {
    /// An editor: the strip's second group, after a divider (`isEditorTab`).
    pub fn is_file(&self) -> bool {
        matches!(self.kind, TabKind::Editor(_))
    }

    /// A page the React app shows with no tab active (its `navPage`):
    /// Dashboard, Code Review, a repo, Settings, Integrations. Off the strip;
    /// the sidebar opens them, and one at a time is kept.
    pub fn is_nav(&self, cx: &App) -> bool {
        let key = self.key(cx);
        key == "page:dashboard"
            || key == "page:settings"
            || key == "page:integrations"
            || key.starts_with("page:repo:")
            || key.starts_with("review:prs:")
            || key == "page:code-review"
    }

    /// An agent session: its × minimizes instead of ending it.
    pub fn is_agent(&self, cx: &App) -> bool {
        matches!(&self.kind, TabKind::Terminal(t) if t.read(cx).is_agent())
    }

    pub fn title(&self, cx: &App) -> SharedString {
        match &self.kind {
            TabKind::Terminal(t) => t.read(cx).title(),
            TabKind::Editor(e) => e.read(cx).title.clone(),
            TabKind::Diff(d) => d.read(cx).title.clone(),
            TabKind::Review(r) => r.read(cx).title(),
            TabKind::Page(p) => p.read(cx).title(),
            TabKind::Tool(p) => p.read(cx).title(cx),
        }
    }

    pub fn dirty(&self, cx: &App) -> bool {
        matches!(&self.kind, TabKind::Editor(e) if e.read(cx).is_dirty())
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.kind {
            TabKind::Terminal(t) => t.focus_handle(cx),
            TabKind::Editor(e) => e.focus_handle(cx),
            TabKind::Diff(d) => d.read(cx).state.focus_handle(cx),
            TabKind::Review(r) => r.focus_handle(cx),
            TabKind::Page(p) => p.focus_handle(cx),
            TabKind::Tool(p) => p.focus_handle(cx),
        }
    }

    /// Stable identity: history entries, menus and dialogs refer to tabs by key.
    pub fn key(&self, cx: &App) -> String {
        match &self.kind {
            TabKind::Terminal(t) => format!("term:{}", t.entity_id()),
            TabKind::Editor(e) => format!("ed:{}", e.read(cx).path.display()),
            TabKind::Diff(d) => format!("diff:{}", d.read(cx).path.display()),
            TabKind::Review(r) => r.read(cx).key(),
            TabKind::Page(p) => p.read(cx).key(),
            TabKind::Tool(p) => p.read(cx).key.clone(),
        }
    }

    fn nav_target(&self, cx: &App) -> NavTarget {
        match &self.kind {
            TabKind::Terminal(_) => NavTarget::Terminal,
            TabKind::Editor(e) => NavTarget::Editor(e.read(cx).path.clone()),
            TabKind::Diff(d) => NavTarget::Diff(d.read(cx).path.clone()),
            TabKind::Review(_) | TabKind::Page(_) | TabKind::Tool(_) => NavTarget::Review,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pane {
    Sidebar,
    Dock,
    Panel,
}

pub struct PaneDrag {
    pub pane: Pane,
    pub start: Point<Pixels>,
    pub size: f32,
    /// The mouse moved since the press (a press alone may be a double click).
    pub moved: bool,
}

/// Debounced writes of `SavedState`.
#[derive(Default)]
struct Persist {
    path: Option<PathBuf>,
    last: String,
    task: Option<Task<()>>,
}

pub struct OrbitApp {
    pub workspaces: Vec<Workspace>,
    pub selected: Option<usize>,
    pub tree: Entity<FileTree>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    pub lsp: LspRegistry,
    /// Focused when no tab is, so app shortcuts always have a dispatch path.
    pub focus: FocusHandle,
    pub layout: Layout,
    pub drag: Option<PaneDrag>,
    pub nav: NavHistory<NavTarget>,
    /// Set while back/forward switches tabs, so the switch isn't recorded.
    applying_nav: bool,
    pub toasts: Toasts,
    pub menu: Option<ContextMenu>,
    pub dialog: Option<Dialog>,
    pub palette: Option<Palette>,
    pub(crate) overlay_focus: FocusHandle,
    /// Views shown at the right of the titlebar, before the files-panel
    /// toggle: where the run widget and the inbox go.
    pub titlebar_items: Vec<AnyView>,
    /// Sidebar: workspaces whose session list is folded (`orbit.ws-expanded`).
    pub ws_collapsed: BTreeSet<String>,
    /// Agent sessions closed from the strip but still running, by tab key.
    pub minimized: HashSet<String>,
    /// The configured repos (`config.services`) and group count, for the sidebar.
    pub services: Vec<String>,
    pub group_count: usize,
    /// Branch, changes and ahead/behind per repo (`useRepoBriefs.ts`).
    pub briefs: HashMap<String, orbit_core::repo::RepoBrief>,
    /// The status bar's AI usage and Claude limits, for `usage.0`'s workspace.
    pub usage: Option<(String, Option<orbit_core::usage::AiUsage>, orbit_core::agent_hooks::RateLimits)>,
    /// Shell chrome state (dock tab, open popovers), see `shell.rs`.
    pub chrome: crate::shell::Chrome,
    persist: Persist,
    _subscriptions: Vec<Subscription>,
}

/// The app's commands and keys (JetBrains-style, as the React app).
pub fn init(cx: &mut App) {
    crate::overlay::bind_keys(cx);
    let has_files = |a: &OrbitApp, _: &App| a.tabs.iter().any(Tab::is_file);
    let active_is_file = |a: &OrbitApp, _: &App| a.active_tab().is_some_and(Tab::is_file);
    let not_in_terminal = |a: &OrbitApp, _: &App| a.active_terminal().is_none();
    for command in [
        Command::new("app.newTerminal", "New Terminal").keys(["ctrl-shift-t"]).run(|a, w, cx| a.new_terminal(None, w, cx)),
        Command::new("app.closeTab", "Close Tab")
            .keys(["ctrl-w"])
            .when(|a, _| a.active.is_some())
            // Shells use Ctrl+W: the key closes file tabs only.
            .key_when(not_in_terminal)
            .run(|a, w, cx| a.close_active(false, w, cx)),
        Command::new("app.closeTabDiscard", "Close Tab Without Saving")
            .keys(["ctrl-shift-w"])
            .when(|a, _| a.active.is_some())
            .run(|a, w, cx| a.close_active(true, w, cx)),
        Command::new("app.keepOpen", "Keep Tab Open").when(|a, _| a.active_tab().is_some_and(|t| t.preview)).run(|a, _, cx| {
            if let Some(ix) = a.active {
                a.keep_open(ix, cx);
            }
        }),
        Command::new("app.closeOtherTabs", "Close Other Tabs").when(|a, _| a.active.is_some()).run(|a, w, cx| {
            if let Some(ix) = a.active {
                a.close_other_tabs(ix, w, cx);
            }
        }),
        Command::new("app.closeAllTabs", "Close All Tabs").when(|a, _| !a.tabs.is_empty()).run(|a, w, cx| a.close_all_tabs(w, cx)),
        Command::new("app.closeOtherFiles", "Close Other Files").when(active_is_file).run(|a, w, cx| {
            if let Some(ix) = a.active {
                a.close_files(FileSet::Others(ix), w, cx);
            }
        }),
        Command::new("app.closeFilesToRight", "Close Files to the Right").when(active_is_file).run(|a, w, cx| {
            if let Some(ix) = a.active {
                a.close_files(FileSet::RightOf(ix), w, cx);
            }
        }),
        Command::new("app.closeSavedFiles", "Close Saved Files").when(has_files).run(|a, w, cx| a.close_files(FileSet::Saved, w, cx)),
        Command::new("app.closeAllFiles", "Close All Files").when(has_files).run(|a, w, cx| a.close_files(FileSet::All, w, cx)),
        Command::new("app.nextTab", "Next Tab").keys(["ctrl-tab"]).when(|a, _| !a.tabs.is_empty()).run(|a, w, cx| a.cycle_tab(1, w, cx)),
        Command::new("app.prevTab", "Previous Tab")
            .keys(["ctrl-shift-tab"])
            .when(|a, _| !a.tabs.is_empty())
            .run(|a, w, cx| a.cycle_tab(-1, w, cx)),
        // Shells move by word with Alt+←/→.
        Command::new("app.back", "Go Back").keys(["alt-left"]).key_when(not_in_terminal).run(|a, w, cx| a.navigate(true, w, cx)),
        Command::new("app.forward", "Go Forward").keys(["alt-right"]).key_when(not_in_terminal).run(|a, w, cx| a.navigate(false, w, cx)),
        Command::new("app.toggleSidebar", "Toggle Sidebar").run(|a, _, cx| a.toggle_sidebar(cx)),
        Command::new("app.toggleDock", "Toggle Files Panel").run(|a, _, cx| a.toggle_dock(cx)),
        Command::new("app.togglePanel", "Toggle Build/Run Panel").run(|a, _, cx| a.toggle_panel(cx)),
        Command::new("app.selectTheme", "Select Theme…").run(|a, w, cx| {
            let items = crate::theme::list()
                .iter()
                .map(|t| {
                    let mark = if crate::theme::current().id == t.id { "✓  " } else { "    " };
                    MenuItem::new(format!("{mark}{}", t.label), move |_: &mut OrbitApp, _: &mut Window, cx: &mut Context<OrbitApp>| {
                        crate::theme::set(t.id, cx);
                    })
                })
                .collect();
            a.open_menu(Point::new(px(420.), px(90.)), items, w, cx)
        }),
        Command::new("app.findAction", "Find Action…").keys(["ctrl-shift-a"]).run(|a, w, cx| a.open_palette(w, cx)),
        Command::new("app.switchWorkspace", "Switch Workspace…").when(|a, _| !a.workspaces.is_empty()).run(|a, w, cx| {
            a.open_workspace_menu(Point::new(px(160.), px(34.)), w, cx)
        }),
        // Sessions (`sessions.rs`); Ctrl+Shift+T above opens a shell session.
        Command::new("sessions.newAgent", "New Agent Session").keys(["ctrl-alt-a"]).action(crate::sessions::NewAgentSession),
        Command::new("sessions.newClaude", "New Claude Session").action(crate::sessions::NewClaudeSession),
        Command::new("sessions.newOpencode", "New OpenCode Session").action(crate::sessions::NewOpencodeSession),
        Command::new("sessions.newOmp", "New omp Session").action(crate::sessions::NewOmpSession),
        Command::new("sessions.newShell", "New Shell Session").action(crate::sessions::NewShellSession),
        Command::new("sessions.resumeLast", "Resume Last Session").keys(["ctrl-alt-r"]).action(crate::sessions::ResumeLastSession),
        Command::new("sessions.nextNeedingInput", "Next Session Needing Input")
            .keys(["ctrl-alt-i"])
            .action(crate::sessions::NextSessionNeedingInput),
        Command::new("sessions.markInboxRead", "Mark Inbox Read").action(crate::sessions::MarkInboxRead),
        Command::new("sessions.clearInbox", "Clear Inbox").action(crate::sessions::ClearInbox),
        Command::new("app.quit", "Quit").keys(["ctrl-q"]).run(|a, _, cx| {
            a.save_now(cx);
            cx.quit()
        }),
    ] {
        commands::register(cx, command);
    }
    // The editor's actions, listed in the palette under their GPUI ids.
    for c in editor::editor_actions() {
        let Ok(action) = cx.build_action(c.id, None) else { continue };
        let mut command = Command::new(c.id, c.title).keys(c.keys.iter().copied()).context(c.context);
        command.run = Some(commands::Run::Action(action.into()));
        commands::register(cx, command);
    }
}

/// Which file tabs a batch close picks.
#[derive(Clone, Copy)]
pub enum FileSet {
    All,
    Saved,
    Others(usize),
    RightOf(usize),
}

impl OrbitApp {
    pub fn new(state_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(FileTree::new);
        let sub = cx.subscribe_in(&tree, window, |this, _, event, window, cx| match event {
            FileTreeEvent::OpenFile { path, preview } => {
                this.open_file(path.clone(), *preview, window, cx);
            }
            FileTreeEvent::Settled => {}
        });
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Notification>();
        cx.spawn(async move |this, cx| {
            while let Some(n) = rx.next().await {
                if this.update(cx, |this, cx| this.on_lsp_notification(n, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        crate::sessions::init(window, cx);
        let store = crate::sessions::store(cx);
        let store_sub = cx.observe(&store, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let config = orbit_core::config::Config::load().ok();
        crate::shell::start_polls(window, cx);
        Self {
            workspaces: workspace::list_workspaces(),
            selected: None,
            tree,
            tabs: vec![],
            active: None,
            lsp: LspRegistry::new(tx),
            focus,
            layout: Layout::default(),
            drag: None,
            nav: NavHistory::default(),
            applying_nav: false,
            toasts: Toasts::default(),
            menu: None,
            dialog: None,
            palette: None,
            overlay_focus: cx.focus_handle(),
            titlebar_items: vec![],
            ws_collapsed: BTreeSet::new(),
            minimized: HashSet::new(),
            services: config.as_ref().map(|c| c.services.iter().map(|s| s.name.clone()).collect()).unwrap_or_default(),
            group_count: config.as_ref().map_or(0, |c| c.groups.len()),
            briefs: HashMap::new(),
            usage: None,
            chrome: Default::default(),
            persist: Persist { path: state_path, ..Default::default() },
            _subscriptions: vec![sub, store_sub],
        }
    }

    pub fn select_workspace(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        let ws = &self.workspaces[ix];
        let roots = ws.meta.repos.iter().map(|r| (r.clone(), ws.dir.join(r))).collect();
        self.tree.update(cx, |tree, cx| tree.set_roots(roots, cx));
        cx.notify();
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.selected.map(|ix| &self.workspaces[ix])
    }

    /// The selected workspace's repo holding `path`, and its folder in the workspace.
    fn repo_of(&self, path: &Path) -> Option<(RepoKey, PathBuf)> {
        let ws = self.workspace()?;
        let (repo, root) = ws.meta.repos.iter().map(|r| (r, ws.dir.join(r))).find(|(_, root)| path.starts_with(root))?;
        Some((RepoKey { workspace: ws.meta.name.clone(), repo: repo.clone() }, root))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|ix| self.tabs.get(ix))
    }

    pub fn active_editor(&self) -> Option<Entity<EditorTab>> {
        match self.active_tab()?.kind {
            TabKind::Editor(ref e) => Some(e.clone()),
            _ => None,
        }
    }

    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        match self.active_tab()?.kind {
            TabKind::Terminal(ref t) => Some(t.clone()),
            _ => None,
        }
    }

    pub fn tab_index(&self, key: &str, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|t| t.key(cx) == key)
    }

    /// Adds a tab in its group (sessions before files) and returns its index.
    pub(crate) fn insert_tab(&mut self, tab: Tab) -> usize {
        let ix = if tab.is_file() { self.tabs.len() } else { self.tabs.iter().take_while(|t| !t.is_file()).count() };
        self.tabs.insert(ix, tab);
        if let Some(active) = self.active.as_mut().filter(|a| **a >= ix) {
            *active += 1;
        }
        ix
    }

    pub fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_cursor(cx);
        self.active = Some(ix);
        let Some(tab) = self.tabs.get(ix) else { return cx.notify() };
        let key = tab.key(cx);
        // Showing a minimized session brings its tab back.
        self.minimized.remove(&key);
        // One nav page at a time, as the React app's single `navPage`.
        if tab.is_nav(cx) {
            let others: Vec<String> = self.tabs.iter().filter(|t| t.is_nav(cx)).map(|t| t.key(cx)).filter(|k| *k != key).collect();
            if !others.is_empty() {
                self.tabs.retain(|t| !others.contains(&t.key(cx)));
                self.active = self.tab_index(&key, cx);
            }
        }
        // The dock, run widget and status bar follow the tab's workspace (`focusWorkspace`).
        if let Some(ws) = self.tab_workspace(&key, cx) {
            if self.selected != Some(ws) {
                self.select_workspace(ws, cx);
            }
        }
        if let Some(tab) = self.active.and_then(|a| self.tabs.get(a)) {
            window.focus(&tab.focus_handle(cx), cx);
            if !self.applying_nav {
                let target = tab.nav_target(cx);
                self.nav.visit(key, target);
            }
        }
        cx.notify();
    }

    /// The workspace (index) a tab works in: its page, its sessions' scope, its files.
    pub fn tab_workspace(&self, key: &str, cx: &App) -> Option<usize> {
        let by_name = |n: &str| self.workspaces.iter().position(|w| w.meta.name == n);
        let tab = self.tabs.iter().find(|t| t.key(cx) == key)?;
        match &tab.kind {
            TabKind::Terminal(t) => by_name(&t.read(cx).meta.workspace),
            TabKind::Editor(e) => self.workspaces.iter().position(|w| e.read(cx).path.starts_with(&w.dir)),
            TabKind::Diff(d) => self.workspaces.iter().position(|w| d.read(cx).path.starts_with(&w.dir)),
            _ if tab.is_nav(cx) => None,
            _ => {
                let rest = key.strip_prefix("page:ws:").or_else(|| key.strip_prefix("review:changes:"));
                let rest = rest.or_else(|| ["page:ralph:", "page:usage:", "page:artifacts:"].iter().find_map(|p| key.strip_prefix(p)));
                rest.and_then(by_name)
            }
        }
    }

    /// The workspace in focus (`focusWorkspace`): the active tab's. Nav pages have none.
    pub fn focus_workspace(&self, cx: &App) -> Option<&Workspace> {
        let key = self.active_tab()?.key(cx);
        self.tab_workspace(&key, cx).map(|ix| &self.workspaces[ix])
    }

    /// Indices of the tabs on the strip, in its order: sessions and views, then files.
    pub fn strip_tabs(&self, cx: &App) -> Vec<usize> {
        let shown: Vec<usize> =
            (0..self.tabs.len()).filter(|&i| !self.tabs[i].is_nav(cx) && !self.minimized.contains(&self.tabs[i].key(cx))).collect();
        let (files, rest): (Vec<usize>, Vec<usize>) = shown.into_iter().partition(|&i| self.tabs[i].is_file());
        rest.into_iter().chain(files).collect()
    }

    /// The tab's × and middle click: agent sessions minimize (they keep
    /// running, the sidebar still lists them); the rest close.
    pub fn dismiss_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.tabs.get(ix) {
            Some(t) if t.is_agent(cx) => self.minimize_tab(ix, window, cx),
            Some(_) => self.close_tab(ix, false, window, cx),
            None => {}
        }
    }

    /// Takes a session off the strip; the strip's neighbour becomes active.
    pub fn minimize_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.tabs.get(ix).map(|t| t.key(cx)) else { return };
        let strip = self.strip_tabs(cx);
        let pos = strip.iter().position(|&i| i == ix);
        self.minimized.insert(key);
        if self.active == Some(ix) {
            let rest: Vec<usize> = strip.into_iter().filter(|&i| i != ix).collect();
            match pos.and_then(|p| rest.get(p.min(rest.len().saturating_sub(1))).copied()) {
                Some(next) => self.activate(next, window, cx),
                None => self.show_dashboard(window, cx),
            }
        }
        cx.notify();
    }

    /// The Dashboard nav page (the React app's default `navPage`).
    pub fn show_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::workspace_ui::open(self, crate::workspace_ui::PageId::Dashboard, window, cx);
    }

    /// The shown editor's cursor goes into its history entry.
    fn remember_cursor(&mut self, cx: &App) {
        if let Some(tab) = self.active_tab() {
            if let TabKind::Editor(e) = &tab.kind {
                let key = tab.key(cx);
                self.nav.set_position(&key, e.read(cx).cursor(cx));
            }
        }
    }

    /// Focus back to the active tab, or the app when there is none.
    pub fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab() {
            Some(tab) => window.focus(&tab.focus_handle(cx), cx),
            None => window.focus(&self.focus, cx),
        }
    }

    /// Next / previous tab on the strip.
    pub fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let strip = self.strip_tabs(cx);
        let n = strip.len() as isize;
        if n > 0 {
            let at = self.active.and_then(|a| strip.iter().position(|&i| i == a));
            let next = at.map_or(0, |p| (p as isize + delta).rem_euclid(n));
            self.activate(strip[next as usize], window, cx);
        }
    }

    pub fn new_terminal(&mut self, input: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        crate::sessions::new_session(self, None, input, window, cx);
    }

    /// Opens (or focuses) a file's editor tab. `preview` reuses the preview
    /// tab's slot; opening a previewed file without `preview` keeps it.
    pub fn open_file(&mut self, path: PathBuf, preview: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<EditorTab>> {
        if let Some(ix) = self.tabs.iter().position(|t| matches!(&t.kind, TabKind::Editor(e) if e.read(cx).path == path)) {
            if !preview {
                self.tabs[ix].preview = false;
            }
            self.activate(ix, window, cx);
            return self.active_editor();
        }
        let started = Instant::now();
        let text = match editor::read_text(&path) {
            Ok(text) => text,
            Err(e) => {
                self.toast(ToastKind::Error, format!("Can't open {}", file_name(&path)), Some(format!("{e:#}").into()), cx);
                return None;
            }
        };
        let editor = self.new_editor(path.clone(), text, window, cx);
        let tab = Tab { kind: TabKind::Editor(editor.clone()), preview };
        let reuse = preview.then(|| self.tabs.iter().position(|t| t.preview && !t.dirty(cx))).flatten();
        let ix = match reuse {
            Some(ix) => {
                self.tabs[ix] = tab;
                ix
            }
            None => self.insert_tab(tab),
        };
        eprintln!("opened {} in {} ms", path.display(), started.elapsed().as_millis());
        self.activate(ix, window, cx);
        Some(editor)
    }

    /// An editor for `path` with the app's handling of its events, not yet in a tab.
    fn new_editor(&mut self, path: PathBuf, text: String, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorTab> {
        let repo = self.repo_of(&path);
        let lsp = &mut self.lsp;
        let editor = cx.new(|cx| EditorTab::new(path, text, repo, lsp, window, cx));
        let sub = cx.subscribe_in(&editor, window, |this, editor, event, window, cx| match event {
            EditorEvent::DirtyChanged => {
                // An edited preview tab stays open.
                if editor.read(cx).is_dirty() {
                    if let Some(t) = this.tabs.iter_mut().find(|t| matches!(&t.kind, TabKind::Editor(e) if e == editor)) {
                        t.preview = false;
                    }
                }
                cx.notify();
            }
            EditorEvent::OpenLocation { path, range } => {
                let (path, range) = (path.clone(), *range);
                if let Some(target) = this.open_file(path, false, window, cx) {
                    if let Some(range) = range {
                        target.update(cx, |t, cx| t.reveal(range, window, cx));
                    }
                }
            }
            EditorEvent::OpenDiff => {
                let path = editor.read(cx).path.clone();
                this.open_diff(path, window, cx);
            }
        });
        self._subscriptions.push(sub);
        editor
    }

    /// After a rename in the tree: editors of the file, or of files under the
    /// folder, reopen at the new path in the same tab, keeping unsaved text and cursor.
    pub fn file_renamed(&mut self, from: &Path, to: &Path, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.tabs.len() {
            let TabKind::Editor(old) = &self.tabs[ix].kind else { continue };
            let old = old.clone();
            let Ok(rest) = old.read(cx).path.strip_prefix(from).map(Path::to_path_buf) else { continue };
            let path = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            let Ok(text) = editor::read_text(&path) else { continue };
            let (dirty, buffer, cursor) = {
                let e = old.read(cx);
                let state = e.state.read(cx);
                (e.is_dirty(), state.text().to_string(), state.cursor_position())
            };
            let editor = self.new_editor(path, text, window, cx);
            editor.update(cx, |e, cx| {
                e.state.update(cx, |s, cx| {
                    // As an edit, so the tab is dirty against the file on disk.
                    if dirty {
                        s.replace_all(buffer, window, cx);
                    }
                    s.set_cursor_position(cursor, window, cx);
                });
            });
            self.tabs[ix].kind = TabKind::Editor(editor);
            if self.active == Some(ix) {
                self.activate(ix, window, cx);
            }
        }
        cx.notify();
    }

    pub fn open_diff(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<DiffTab>> {
        if let Some(ix) = self.tabs.iter().position(|t| matches!(&t.kind, TabKind::Diff(d) if d.read(cx).path == path)) {
            self.activate(ix, window, cx);
            return match &self.tabs[ix].kind {
                TabKind::Diff(d) => Some(d.clone()),
                _ => None,
            };
        }
        let files = match DiffTab::load(&path) {
            Ok(files) => files,
            Err(e) => {
                self.toast(ToastKind::Error, format!("No diff for {}", file_name(&path)), Some(format!("{e:#}").into()), cx);
                return None;
            }
        };
        let view = cx.new(|cx| DiffTab::new(path, files, true, cx));
        let ix = self.insert_tab(Tab { kind: TabKind::Diff(view.clone()), preview: false });
        self.activate(ix, window, cx);
        Some(view)
    }

    /// Closes a tab. One with unsaved changes asks first, unless `force`.
    pub fn close_tab(&mut self, ix: usize, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.dirty(cx) && !force {
            return self.confirm_unsaved(ix, window, cx);
        }
        if let TabKind::Terminal(t) = &tab.kind {
            crate::sessions::closed(t, cx);
        }
        let key = tab.key(cx);
        self.minimized.remove(&key);
        let strip_pos = self.strip_tabs(cx).iter().position(|&i| i == ix);
        let was_active = self.active == Some(ix);
        self.tabs.remove(ix);
        if !was_active {
            self.active = self.active.map(|a| if a > ix { a - 1 } else { a });
            return cx.notify();
        }
        // The strip's neighbour, else the open nav page, else the Dashboard.
        let strip = self.strip_tabs(cx);
        let next = strip_pos
            .and_then(|p| strip.get(p.min(strip.len().saturating_sub(1))).copied())
            .or_else(|| strip.last().copied())
            .or_else(|| self.tabs.iter().position(|t| t.is_nav(cx)));
        self.active = None;
        match next {
            Some(a) => self.activate(a, window, cx),
            None => {
                window.focus(&self.focus, cx);
                self.show_dashboard(window, cx);
            }
        }
        cx.notify();
    }

    pub fn close_active(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active {
            self.close_tab(ix, force, window, cx);
        }
    }

    /// "Save changes to X?": Save, Don't Save or Cancel, as the React app.
    fn confirm_unsaved(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &self.tabs[ix];
        let (name, key) = (tab.title(cx), tab.key(cx));
        let discard_key = key.clone();
        let dialog = Dialog {
            title: format!("Save changes to {name}?").into(),
            message: "Your changes will be lost if you don't save them.".into(),
            buttons: vec![
                DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) },
                DialogButton {
                    label: "Don't Save".into(),
                    style: ButtonStyle::Secondary,
                    run: Rc::new(move |a, w, cx| {
                        if let Some(ix) = a.tab_index(&discard_key, cx) {
                            a.close_tab(ix, true, w, cx);
                        }
                    }),
                },
                DialogButton {
                    label: "Save".into(),
                    style: ButtonStyle::Primary,
                    run: Rc::new(move |a, w, cx| {
                        let Some(ix) = a.tab_index(&key, cx) else { return };
                        if let TabKind::Editor(e) = &a.tabs[ix].kind {
                            let e = e.clone();
                            e.update(cx, |e, cx| e.save(&editor::Save, w, cx));
                            if e.read(cx).is_dirty() {
                                let err = e.read(cx).last_error.clone().unwrap_or_else(|| "save failed".into());
                                return a.toast(ToastKind::Error, "Not closed: the file wasn't saved", Some(err.into()), cx);
                            }
                        }
                        a.close_tab(ix, true, w, cx);
                    }),
                },
            ],
            on_cancel: Rc::new(|_, _, _| {}),
        };
        self.open_dialog(dialog, window, cx);
    }

    /// Removes the tabs `which` picks, except ones with unsaved changes;
    /// returns how many of those stayed. The active tab, if closed, becomes the last.
    fn remove_tabs(&mut self, which: impl Fn(usize, &Tab) -> bool, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let active_key = self.active_tab().map(|t| t.key(cx));
        let mut kept_dirty = 0;
        // Agent sessions survive batch closes: they're minimized, not ended.
        let mut minimize = vec![];
        let remove: Vec<bool> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, t)| {
                let picked = which(ix, t);
                if picked && t.is_agent(cx) {
                    minimize.push(t.key(cx));
                    return false;
                }
                let dirty = picked && t.dirty(cx);
                kept_dirty += dirty as usize;
                picked && !dirty
            })
            .collect();
        let active_minimized = active_key.as_ref().is_some_and(|k| minimize.contains(k));
        self.minimized.extend(minimize);
        for (t, _) in self.tabs.iter().zip(&remove).filter(|(_, r)| **r) {
            if let TabKind::Terminal(v) = &t.kind {
                crate::sessions::closed(v, cx);
            }
        }
        let mut flags = remove.into_iter();
        self.tabs.retain(|_| !flags.next().unwrap_or(false));
        let still = active_key.and_then(|k| self.tab_index(&k, cx)).filter(|_| !active_minimized);
        self.active = still.or_else(|| self.strip_tabs(cx).last().copied());
        match self.active {
            Some(a) if still.is_none() => self.activate(a, window, cx),
            Some(_) => {}
            None => window.focus(&self.focus, cx),
        }
        cx.notify();
        kept_dirty
    }

    fn report_kept(&mut self, kept: usize, cx: &mut Context<Self>) {
        if kept > 0 {
            let s = if kept > 1 { "s" } else { "" };
            self.toast(ToastKind::Info, format!("{kept} file{s} with unsaved changes kept open"), None, cx);
        }
    }

    pub fn close_files(&mut self, set: FileSet, window: &mut Window, cx: &mut Context<Self>) {
        // Positions among the file tabs, as the React menu counts them.
        let files: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].is_file()).collect();
        let at = |ix: usize| files.iter().position(|&f| f == ix);
        let pick: Vec<bool> = (0..self.tabs.len())
            .map(|i| match (set, at(i)) {
                (_, None) => false,
                (FileSet::All, _) => true,
                (FileSet::Saved, _) => !self.tabs[i].dirty(cx),
                (FileSet::Others(keep), Some(_)) => i != keep,
                (FileSet::RightOf(from), Some(p)) => at(from).is_some_and(|f| p > f),
            })
            .collect();
        let kept = self.remove_tabs(|i, _| pick[i], window, cx);
        self.report_kept(kept, cx);
    }

    pub fn close_other_tabs(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.tabs.get(ix).map(|t| t.key(cx)) else { return };
        let kept = self.remove_tabs(|i, _| i != ix, window, cx);
        if let Some(ix) = self.tab_index(&key, cx) {
            self.activate(ix, window, cx);
        }
        self.report_kept(kept, cx);
    }

    pub fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kept = self.remove_tabs(|_, _| true, window, cx);
        self.report_kept(kept, cx);
    }

    pub fn keep_open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(t) = self.tabs.get_mut(ix) {
            t.preview = false;
            cx.notify();
        }
    }

    /// The tab's context menu, with the React app's items.
    pub fn open_tab_menu(&mut self, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let key = tab.key(cx);
        let files: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].is_file()).collect();
        let at = files.iter().position(|&f| f == ix);
        let with = |f: fn(&mut OrbitApp, usize, &mut Window, &mut Context<OrbitApp>)| {
            let key = key.clone();
            move |a: &mut OrbitApp, w: &mut Window, cx: &mut Context<OrbitApp>| {
                if let Some(ix) = a.tab_index(&key, cx) {
                    f(a, ix, w, cx)
                }
            }
        };
        let mut items = vec![MenuItem::new("Close", with(|a, ix, w, cx| a.close_tab(ix, false, w, cx)))];
        if !files.is_empty() {
            if tab.preview {
                items.push(MenuItem::new("Keep Open", with(|a, ix, _, cx| a.keep_open(ix, cx))));
            }
            if at.is_some() && files.len() > 1 {
                items.push(MenuItem::new("Close Other Files", with(|a, ix, w, cx| a.close_files(FileSet::Others(ix), w, cx))));
            }
            if at.is_some_and(|p| p + 1 < files.len()) {
                items.push(MenuItem::new("Close Files to the Right", with(|a, ix, w, cx| a.close_files(FileSet::RightOf(ix), w, cx))));
            }
            items.push(MenuItem::new("Close Saved Files", |a, w, cx| a.close_files(FileSet::Saved, w, cx)));
            items.push(MenuItem::new("Close All Files", |a, w, cx| a.close_files(FileSet::All, w, cx)));
        }
        items.push(MenuItem::new("Close Other Tabs", with(|a, ix, w, cx| a.close_other_tabs(ix, w, cx))));
        items.push(MenuItem::new("Close All Tabs", |a, w, cx| a.close_all_tabs(w, cx)).danger());
        self.open_menu(position, items, window, cx);
    }

    pub fn open_workspace_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(ix, ws)| {
                let label = if self.selected == Some(ix) { format!("✓  {}", ws.meta.name) } else { format!("    {}", ws.meta.name) };
                MenuItem::new(label, move |a: &mut OrbitApp, _: &mut Window, cx: &mut Context<OrbitApp>| a.select_workspace(ix, cx))
            })
            .collect();
        self.open_menu(position, items, window, cx);
    }

    /// Back (or forward) in the history. Closed files reopen; closed terminals are skipped.
    pub fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            return;
        }
        self.remember_cursor(cx);
        let open: Vec<String> = self.tabs.iter().map(|t| t.key(cx)).collect();
        let Some(entry) = self.nav.step(back, |e| open.contains(&e.key) || !matches!(e.target, NavTarget::Terminal | NavTarget::Review)) else {
            cx.notify();
            return;
        };
        self.applying_nav = true;
        let editor = match (open.iter().position(|k| *k == entry.key), &entry.target) {
            (Some(ix), _) => {
                self.activate(ix, window, cx);
                self.active_editor()
            }
            (None, NavTarget::Editor(path)) => self.open_file(path.clone(), false, window, cx),
            (None, NavTarget::Diff(path)) => {
                self.open_diff(path.clone(), window, cx);
                None
            }
            (None, NavTarget::Terminal | NavTarget::Review) => None,
        };
        self.applying_nav = false;
        if let (Some(editor), Some((line, col))) = (editor, entry.position) {
            let p = lsp_types::Position::new(line - 1, col - 1);
            editor.update(cx, |e, cx| e.reveal(lsp_types::Range::new(p, p), window, cx));
        }
        cx.notify();
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.layout.sidebar_hidden = !self.layout.sidebar_hidden;
        cx.notify();
    }

    pub fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        self.layout.dock_hidden = !self.layout.dock_hidden;
        cx.notify();
    }

    pub fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.layout.panel_open = !self.layout.panel_open;
        cx.notify();
    }

    pub fn start_drag(&mut self, pane: Pane, start: Point<Pixels>, cx: &mut Context<Self>) {
        let size = match pane {
            Pane::Sidebar => self.layout.sidebar_width,
            Pane::Dock => self.layout.dock_width,
            Pane::Panel => self.layout.panel_height,
        };
        self.drag = Some(PaneDrag { pane, start, size, moved: false });
        cx.notify();
    }

    pub fn drag_to(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.as_mut() else { return };
        d.moved = true;
        let d = &*d;
        let dx = f32::from(position.x - d.start.x);
        let dy = f32::from(position.y - d.start.y);
        match d.pane {
            Pane::Sidebar => self.layout.sidebar_width = Layout::snap_sidebar(d.size + dx),
            // The dock is on the right and the panel at the bottom: they grow the other way.
            Pane::Dock => self.layout.dock_width = state::DOCK.clamp(d.size - dx),
            Pane::Panel => {
                let max = (f32::from(window.viewport_size().height) - 200.).max(state::PANEL.min);
                self.layout.panel_height = state::PANEL.clamp(d.size - dy).min(max);
            }
        }
        cx.notify();
    }

    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    /// Double click on a resizer: back to the default size.
    pub fn reset_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        match pane {
            Pane::Sidebar => self.layout.sidebar_width = state::SIDEBAR.default,
            Pane::Dock => self.layout.dock_width = state::DOCK.default,
            Pane::Panel => self.layout.panel_height = state::PANEL.default,
        }
        cx.notify();
    }

    pub fn snapshot(&self, cx: &App) -> SavedState {
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::Editor(e) => {
                    let e = e.read(cx);
                    Some(SavedTab::Editor { path: e.path.clone(), preview: t.preview, cursor: Some(e.cursor(cx)) })
                }
                TabKind::Diff(d) => Some(SavedTab::Diff { path: d.read(cx).path.clone() }),
                // Sessions come back with the Sessions stage (scrollback restore).
                TabKind::Terminal(_) | TabKind::Review(_) | TabKind::Page(_) | TabKind::Tool(_) => None,
            })
            .collect();
        let saved = |t: &Tab| matches!(t.kind, TabKind::Editor(_) | TabKind::Diff(_));
        let active = self.active.and_then(|a| {
            let ix = self.tabs[..a].iter().filter(|t| saved(t)).count();
            saved(&self.tabs[a]).then_some(ix)
        });
        SavedState {
            workspace: self.workspace().map(|w| w.meta.name.clone()),
            tabs,
            active,
            layout: self.layout.clone(),
            theme: Some(crate::theme::pref()),
            ws_collapsed: self.ws_collapsed.iter().cloned().collect(),
        }
    }

    /// Writes the state a moment after it last changed.
    pub(crate) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.persist.path.clone() else { return };
        let Ok(json) = serde_json::to_string_pretty(&self.snapshot(cx)) else { return };
        if json == self.persist.last {
            return;
        }
        self.persist.last = json.clone();
        self.persist.task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            if let Err(e) = state::save(&path, &json) {
                eprintln!("saving {}: {e}", path.display());
            }
        }));
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.persist.path.clone() else { return };
        self.persist.task = None;
        if let Ok(json) = serde_json::to_string_pretty(&self.snapshot(cx)) {
            if let Err(e) = state::save(&path, &json) {
                eprintln!("saving {}: {e}", path.display());
            }
            self.persist.last = json;
        }
    }

    pub fn state_path(&self) -> Option<&Path> {
        self.persist.path.as_deref()
    }

    /// Reopens what was open last time. Files that are gone are skipped.
    pub fn restore(&mut self, saved: SavedState, window: &mut Window, cx: &mut Context<Self>) {
        self.layout = saved.layout;
        self.ws_collapsed = saved.ws_collapsed.into_iter().collect();
        // Nothing open (after the sessions come back too): the Dashboard, as the React app starts.
        cx.defer_in(window, |a, w, cx| {
            if a.active.is_none() {
                a.show_dashboard(w, cx);
            }
        });
        if let Some(ix) = saved.workspace.and_then(|name| self.workspaces.iter().position(|w| w.meta.name == name)) {
            self.select_workspace(ix, cx);
        }
        self.applying_nav = true;
        let mut opened = vec![];
        let mut missing = 0;
        for tab in &saved.tabs {
            let ok = match tab {
                SavedTab::Editor { path, preview, cursor } if path.is_file() => {
                    let editor = self.open_file(path.clone(), *preview, window, cx);
                    if let (Some(e), Some((line, col))) = (&editor, cursor) {
                        let p = lsp_types::Position::new(line.saturating_sub(1), col.saturating_sub(1));
                        e.update(cx, |e, cx| e.reveal(lsp_types::Range::new(p, p), window, cx));
                    }
                    editor.is_some()
                }
                SavedTab::Diff { path } if path.is_file() => self.open_diff(path.clone(), window, cx).is_some(),
                _ => false,
            };
            opened.push(ok.then(|| self.active.and_then(|a| self.tabs.get(a)).map(|t| t.key(cx))).flatten());
            missing += !ok as usize;
        }
        self.applying_nav = false;
        let active = saved.active.and_then(|a| opened.get(a).cloned().flatten()).and_then(|k| self.tab_index(&k, cx));
        if let Some(ix) = active.or(self.tabs.len().checked_sub(1)) {
            self.activate(ix, window, cx);
        }
        if missing > 0 {
            let s = if missing > 1 { "s" } else { "" };
            self.toast(ToastKind::Info, format!("{missing} tab{s} from last time could not be reopened"), None, cx);
        }
        // What was just restored is what's on disk.
        self.persist.last = serde_json::to_string_pretty(&self.snapshot(cx)).unwrap_or_default();
    }

    fn on_lsp_notification(&mut self, n: Notification, cx: &mut Context<Self>) {
        if n.method != "textDocument/publishDiagnostics" {
            return;
        }
        let Ok(params) = serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(n.params) else { return };
        let Some(path) = uri_to_path(&params.uri) else { return };
        for tab in &self.tabs {
            if let TabKind::Editor(e) = &tab.kind {
                if e.read(cx).path == path {
                    e.update(cx, |e, cx| e.set_diagnostics(params.diagnostics.clone(), cx));
                }
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string())
}

/// Loads the saved state from `path` (when there is one).
pub fn load_state(path: Option<&Path>) -> SavedState {
    path.map(state::load).unwrap_or_default()
}
