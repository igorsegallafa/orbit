//! The window's root view: workspaces sidebar, file tree, tab strip and the
//! active tab. Tabs keep the React app's order: sessions (terminals) first,
//! then files after a divider, with a VS Code-style preview tab.
use crate::diff_view::DiffTab;
use crate::editor::{self, EditorEvent, EditorTab};
use crate::file_tree::{FileTree, FileTreeEvent};
use crate::lsp::{uri_to_path, LspRegistry, Notification};
use crate::terminal::{self, TerminalView};
use crate::theme::{self, c};
use crate::workspace::{self, Workspace};
use futures::StreamExt;
use gpui::{
    actions, div, font, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    SharedString, Subscription, Window,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

actions!(orbit, [NewTerminal, CloseFileTab, CloseTab, NextTab, Quit]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-t", NewTerminal, None),
        KeyBinding::new("ctrl-w", CloseFileTab, None),
        KeyBinding::new("ctrl-shift-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
}

pub enum TabKind {
    Terminal(Entity<TerminalView>),
    Editor(Entity<EditorTab>),
    Diff(Entity<DiffTab>),
}

pub struct Tab {
    pub kind: TabKind,
    /// Opened with a single click: the next such file replaces it.
    pub preview: bool,
}

impl Tab {
    pub fn is_file(&self) -> bool {
        !matches!(self.kind, TabKind::Terminal(_))
    }

    pub fn title(&self, cx: &App) -> SharedString {
        match &self.kind {
            TabKind::Terminal(t) => t.read(cx).title(),
            TabKind::Editor(e) => e.read(cx).title.clone(),
            TabKind::Diff(d) => d.read(cx).title.clone(),
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
        }
    }
}

pub struct OrbitApp {
    pub workspaces: Vec<Workspace>,
    pub selected: Option<usize>,
    pub tree: Entity<FileTree>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    pub lsp: LspRegistry,
    /// A one-line message in the status bar (errors, refusals).
    pub notice: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl OrbitApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            workspaces: workspace::list_workspaces(),
            selected: None,
            tree,
            tabs: vec![],
            active: None,
            lsp: LspRegistry::new(tx),
            notice: None,
            _subscriptions: vec![sub],
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

    fn cwd(&self) -> PathBuf {
        self.workspace().map(|w| w.dir.clone()).unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
    }

    /// The selected workspace's repo holding `path`: its name and its folder in the workspace.
    fn repo_of(&self, path: &Path) -> Option<(String, PathBuf)> {
        let ws = self.workspace()?;
        ws.meta.repos.iter().map(|r| (r.clone(), ws.dir.join(r))).find(|(_, root)| path.starts_with(root))
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

    /// Adds a tab in its group (sessions before files) and returns its index.
    fn insert_tab(&mut self, tab: Tab) -> usize {
        let ix = if tab.is_file() { self.tabs.len() } else { self.tabs.iter().take_while(|t| !t.is_file()).count() };
        self.tabs.insert(ix, tab);
        if let Some(active) = self.active.as_mut().filter(|a| **a >= ix) {
            *active += 1;
        }
        ix
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(ix);
        if let Some(tab) = self.tabs.get(ix) {
            window.focus(&tab.focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub fn new_terminal(&mut self, input: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let pty = match terminal::Pty::spawn(&self.cwd()) {
            Ok(pty) => pty,
            Err(e) => {
                self.notice = Some(format!("terminal failed: {e}").into());
                return;
            }
        };
        let view = cx.new(|cx| TerminalView::new(pty, input, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        let ix = self.insert_tab(Tab { kind: TabKind::Terminal(view), preview: false });
        self.activate(ix, window, cx);
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
                self.notice = Some(format!("{}: {e:#}", path.display()).into());
                cx.notify();
                return None;
            }
        };
        let repo = self.repo_of(&path);
        let lsp = &mut self.lsp;
        let editor = cx.new(|cx| EditorTab::new(path.clone(), text, repo, lsp, window, cx));
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

    pub fn open_diff(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<DiffTab>> {
        let files = match DiffTab::load(&path) {
            Ok(files) => files,
            Err(e) => {
                self.notice = Some(format!("diff of {}: {e:#}", path.display()).into());
                cx.notify();
                return None;
            }
        };
        let view = cx.new(|cx| DiffTab::new(path, files, true, cx));
        let ix = self.insert_tab(Tab { kind: TabKind::Diff(view.clone()), preview: false });
        self.activate(ix, window, cx);
        Some(view)
    }

    pub fn close_tab(&mut self, ix: usize, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.dirty(cx) && !force {
            self.notice = Some("Unsaved changes: Ctrl+S to save, Ctrl+Shift+W to close without saving".into());
            cx.notify();
            return;
        }
        self.tabs.remove(ix);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) => Some(a.min(self.tabs.len() - 1)),
            None => None,
        };
        if let Some(a) = self.active {
            self.activate(a, window, cx);
        }
        cx.notify();
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspaces")
            .w(px(210.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .flex()
            .flex_col()
            .child(section_header("WORKSPACES"))
            .children(self.workspaces.iter().enumerate().map(|(ix, ws)| {
                let selected = self.selected == Some(ix);
                div()
                    .id(("ws", ix))
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .when(selected, |d| d.bg(theme::accent_soft()))
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .child(div().text_sm().text_color(c(theme::TEXT)).child(ws.meta.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::TEXT_FAINT))
                            .child(format!("{} · {} repos", ws.meta.branch, ws.meta.repos.len())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select_workspace(ix, cx)))
            }))
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tab-strip")
            .h(px(34.))
            .flex_none()
            .flex()
            .items_end()
            .overflow_x_scroll()
            .bg(c(theme::PANEL))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let active = self.active == Some(ix);
                let dirty = tab.dirty(cx);
                // A divider where the files start, after the sessions.
                let group_start = tab.is_file() && ix > 0 && !self.tabs[ix - 1].is_file();
                let icon = match tab.kind {
                    TabKind::Terminal(_) => ">_",
                    TabKind::Editor(_) => "▤",
                    TabKind::Diff(_) => "±",
                };
                div()
                    .id(("tab", ix))
                    .h_full()
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(c(theme::BORDER))
                    .when(group_start, |d| d.border_l_2().border_color(c(theme::BORDER_STRONG)))
                    .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
                    .when(active, |d| d.bg(c(theme::BG)).border_t_2().border_color(c(theme::ACCENT)))
                    .when(tab.preview, |d| d.italic())
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(icon))
                    .child(tab.title(cx))
                    .child(
                        div()
                            .id(("close", ix))
                            .w(px(14.))
                            .flex()
                            .justify_center()
                            .text_color(c(theme::TEXT_FAINT))
                            .hover(|s| s.text_color(c(theme::TEXT)))
                            .child(if dirty { "●" } else { "×" })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(ix, false, window, cx)
                                }),
                            ),
                    )
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        // Double click keeps a preview tab open.
                        if ev.click_count() >= 2 {
                            if let Some(t) = this.tabs.get_mut(ix) {
                                t.preview = false;
                            }
                        }
                        this.activate(ix, window, cx)
                    }))
            }))
            .child(
                div()
                    .id("new-terminal")
                    .h_full()
                    .px_3()
                    .flex()
                    .flex_none()
                    .items_center()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(c(theme::TEXT_DIM))
                    .hover(|s| s.text_color(c(theme::TEXT)))
                    .child("+ terminal")
                    .on_click(cx.listener(|this, _, window, cx| this.new_terminal(None, window, cx))),
            )
    }

    fn render_status(&self) -> impl IntoElement {
        let servers = self.lsp.running();
        div()
            .h(px(22.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .text_xs()
            .bg(c(theme::PANEL))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_color(c(theme::TEXT_FAINT))
            .child(div().flex_1().overflow_hidden().whitespace_nowrap().children(self.notice.clone()))
            .children(servers.iter().map(|s| {
                let name = s.command.split(' ').next().and_then(|b| Path::new(b).file_name()).map(|n| n.to_string_lossy().to_string());
                let repo = s.open_root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                format!("{} · {repo}", name.unwrap_or_default())
            }))
    }
}

fn section_header(label: &'static str) -> impl IntoElement {
    div().px_3().pt_3().pb_2().text_xs().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child(label)
}

impl Render for OrbitApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active_tab().map(|t| &t.kind) {
            Some(TabKind::Terminal(t)) => t.clone().into_any_element(),
            Some(TabKind::Editor(e)) => e.clone().into_any_element(),
            Some(TabKind::Diff(d)) => d.clone().into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::TEXT_FAINT))
                .child("Ctrl+Shift+T opens a terminal · click a file to open it")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .text_color(c(theme::TEXT))
            .font(font(theme::UI_FONT))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(None, window, cx)))
            .on_action(cx.listener(|this, _: &CloseFileTab, window, cx| {
                if let Some(ix) = this.active.filter(|ix| this.tabs[*ix].is_file()) {
                    this.close_tab(ix, false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(ix) = this.active {
                    this.close_tab(ix, true, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if !this.tabs.is_empty() {
                    let next = this.active.map_or(0, |a| (a + 1) % this.tabs.len());
                    this.activate(next, window, cx);
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .w(px(280.))
                            .flex_none()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(c(theme::PANEL))
                            .border_r_1()
                            .border_color(c(theme::BORDER))
                            .child(section_header("FILES"))
                            .child(div().flex_1().min_h_0().child(self.tree.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(self.render_tabs(cx))
                            .child(div().flex_1().min_h_0().child(content)),
                    ),
            )
            .child(self.render_status())
    }
}
