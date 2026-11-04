//! Editor tab: gpui-component's code editor (tree-sitter highlighting) with
//! save, dirty state and a language server for definition, hover and
//! diagnostics.
use crate::lsp::{self, path_to_uri, uri_to_path, LspRegistry, LspServer};
use crate::theme::{self, c};
use anyhow::{bail, Context as _, Result};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding,
    SharedString, Subscription, Task, Window,
};
use gpui_component::input::{
    DefinitionProvider, Editor, EditorState, GoToDefinition, HoverProvider, InputEvent, Rope, RopeExt, TabSize,
};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

actions!(orbit_editor, [Save]);

const MAX_BYTES: u64 = 8 * 1024 * 1024;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, Some("OrbitEditor")),
        KeyBinding::new("f12", GoToDefinition, Some("Input")),
    ]);
}

pub enum EditorEvent {
    DirtyChanged,
    /// Go to definition landed in another file.
    OpenLocation { path: PathBuf, range: Option<lsp_types::Range> },
    /// Show this file's changes against HEAD.
    OpenDiff,
}

/// The document as the language server knows it.
pub struct LspDoc {
    uri: lsp_types::Uri,
    language_id: &'static str,
    server: RefCell<Option<Arc<LspServer>>>,
    version: Cell<i32>,
}

impl LspDoc {
    fn server(&self) -> Option<Arc<LspServer>> {
        self.server.borrow().clone()
    }

    fn position_params(&self, text: &Rope, offset: usize) -> Value {
        let pos = text.offset_to_position(offset);
        json!({ "textDocument": { "uri": self.uri.as_str() }, "position": { "line": pos.line, "character": pos.character } })
    }
}

struct DocProvider(Rc<LspDoc>);

impl DefinitionProvider for DocProvider {
    fn definitions(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Vec<lsp_types::LocationLink>>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(vec![])) };
        let request = server.request("textDocument/definition", self.0.position_params(text, offset));
        cx.background_spawn(async move {
            let value = request.await?;
            Ok(location_links(value))
        })
    }
}

impl HoverProvider for DocProvider {
    fn hover(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Option<lsp_types::Hover>>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(None)) };
        let request = server.request("textDocument/hover", self.0.position_params(text, offset));
        cx.background_spawn(async move { Ok(serde_json::from_value(request.await?).ok()) })
    }
}

/// `Location | Location[] | LocationLink[] | null` as links.
fn location_links(value: Value) -> Vec<lsp_types::LocationLink> {
    use lsp_types::{GotoDefinitionResponse as R, LocationLink};
    let link = |l: lsp_types::Location| LocationLink {
        origin_selection_range: None,
        target_uri: l.uri,
        target_range: l.range,
        target_selection_range: l.range,
    };
    match serde_json::from_value::<Option<R>>(value).ok().flatten() {
        Some(R::Scalar(l)) => vec![link(l)],
        Some(R::Array(ls)) => ls.into_iter().map(link).collect(),
        Some(R::Link(links)) => links,
        None => vec![],
    }
}

pub fn hover_text(hover: &lsp_types::Hover) -> String {
    use lsp_types::{HoverContents, MarkedString};
    let marked = |m: &MarkedString| match m {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString(l) => l.value.clone(),
    };
    match &hover.contents {
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(ms) => ms.iter().map(marked).collect::<Vec<_>>().join("\n"),
        HoverContents::Markup(m) => m.value.clone(),
    }
}

pub struct EditorTab {
    pub path: PathBuf,
    /// "repo/file.ext", or the file name outside a repo.
    pub title: SharedString,
    /// The path shown above the editor.
    pub display_path: SharedString,
    pub state: Entity<EditorState>,
    saved: Rope,
    dirty: bool,
    doc: Option<Rc<LspDoc>>,
    provider: Option<Rc<DocProvider>>,
    lsp_status: SharedString,
    pub diagnostics: Vec<lsp_types::Diagnostic>,
    pub last_error: Option<String>,
    _subscriptions: Vec<Subscription>,
    _attach: Task<()>,
}

impl EventEmitter<EditorEvent> for EditorTab {}

impl Focusable for EditorTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

pub fn read_text(path: &Path) -> Result<String> {
    if std::fs::metadata(path)?.len() > MAX_BYTES {
        bail!("file larger than 8 MB");
    }
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if bytes.contains(&0) {
        bail!("binary file");
    }
    String::from_utf8(bytes).context("not UTF-8")
}

impl EditorTab {
    /// `repo`: the repo the file belongs to (its name and the folder as the
    /// workspace has it), which scopes the language server.
    pub fn new(
        path: PathBuf,
        text: String,
        repo: Option<(String, PathBuf)>,
        registry: &mut LspRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = lsp::file_language(&path);
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language.highlight)
                .line_number(true)
                .tab_size(TabSize { tab_size: 4, hard_tabs: false })
                .default_value(text)
        });
        let saved = state.read(cx).text().clone();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let (title, display_path) = match &repo {
            Some((name, root)) => {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
                (format!("{name}/{file_name}"), format!("{name}/{rel}"))
            }
            None => (file_name, path.to_string_lossy().to_string()),
        };

        let mut doc = None;
        let mut provider = None;
        let mut lsp_status: SharedString = "".into();
        let mut attach = Task::ready(());
        if let (Some((server_language, language_id)), Some((_, root)), Some(uri)) = (language.lsp, &repo, path_to_uri(&path)) {
            match registry.server(root, server_language, cx) {
                Some(server) => {
                    let d = Rc::new(LspDoc { uri, language_id, server: RefCell::new(None), version: Cell::new(0) });
                    let p = Rc::new(DocProvider(d.clone()));
                    let this = cx.weak_entity();
                    let own_path = path.clone();
                    state.update(cx, |s, _| {
                        let lsp = s.lsp_mut();
                        lsp.definition_provider = Some(p.clone());
                        lsp.hover_provider = Some(p.clone());
                        // Definitions in other files open (or focus) their own tab.
                        lsp.show_document = Some(Rc::new(move |params, _, cx| {
                            let Some(target) = uri_to_path(&params.uri) else { return false };
                            if target == own_path {
                                return false;
                            }
                            let range = params.selection;
                            this.update(cx, |_, cx| cx.emit(EditorEvent::OpenLocation { path: target, range })).is_ok()
                        }));
                    });
                    lsp_status = format!("{server_language}: starting").into();
                    attach = cx.spawn(async move |this, cx| {
                        let server = server.await;
                        let _ = this.update(cx, |this, cx| this.attach(server, cx));
                    });
                    doc = Some(d);
                    provider = Some(p);
                }
                None => {
                    lsp_status = registry
                        .errors
                        .get(&(root.clone(), server_language))
                        .cloned()
                        .unwrap_or_default()
                        .into();
                }
            }
        }

        let subscriptions = vec![cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_change(cx);
            }
        })];
        Self {
            path,
            title: title.into(),
            display_path: display_path.into(),
            state,
            saved,
            dirty: false,
            doc,
            provider,
            lsp_status,
            diagnostics: vec![],
            last_error: None,
            _subscriptions: subscriptions,
            _attach: attach,
        }
    }

    fn attach(&mut self, server: Option<Arc<LspServer>>, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(server) = server else {
            self.lsp_status = "language server failed to start".into();
            cx.notify();
            return;
        };
        // didOpen carries the text as it is now, edits made while starting included.
        doc.version.set(1);
        server.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": doc.uri.as_str(), "languageId": doc.language_id, "version": 1,
                "text": self.state.read(cx).text().to_string(),
            }}),
        );
        let name = Path::new(&server.command.split(' ').next().unwrap_or_default())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.lsp_status = name.into();
        *doc.server.borrow_mut() = Some(server);
        cx.notify();
    }

    pub fn lsp_attached(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.server.borrow().is_some())
    }

    pub fn lsp_status(&self) -> &SharedString {
        &self.lsp_status
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn on_change(&mut self, cx: &mut Context<Self>) {
        let text = self.state.read(cx).text();
        let dirty = *text != self.saved;
        if let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d, d.server()?))) {
            let version = doc.version.get() + 1;
            doc.version.set(version);
            server.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": doc.uri.as_str(), "version": version },
                    "contentChanges": [{ "text": text.to_string() }],
                }),
            );
        }
        if dirty != self.dirty {
            self.dirty = dirty;
            cx.emit(EditorEvent::DirtyChanged);
        }
        cx.notify();
    }

    pub fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.state.read(cx).text().clone();
        match std::fs::write(&self.path, text.to_string()) {
            Ok(()) => {
                self.saved = text;
                self.last_error = None;
                if let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d, d.server()?))) {
                    server.notify("textDocument/didSave", json!({ "textDocument": { "uri": doc.uri.as_str() } }));
                }
                if self.dirty {
                    self.dirty = false;
                    cx.emit(EditorEvent::DirtyChanged);
                }
            }
            Err(e) => self.last_error = Some(format!("save failed: {e}")),
        }
        cx.notify();
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<lsp_types::Diagnostic>, cx: &mut Context<Self>) {
        self.diagnostics = diagnostics.clone();
        self.state.update(cx, |s, cx| {
            let text = s.text().clone();
            if let Some(set) = s.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics);
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Moves the cursor to `range` (0-based) and focuses the editor.
    pub fn reveal(&mut self, range: lsp_types::Range, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.set_cursor_position(range.start, window, cx));
    }

    /// 1-based line and column of the cursor.
    pub fn cursor(&self, cx: &App) -> (u32, u32) {
        let p = self.state.read(cx).cursor_position();
        (p.line + 1, p.character + 1)
    }

    /// Hover text at the cursor, asked of the server directly (self-test).
    pub fn hover_at_cursor(&self, window: &mut Window, cx: &mut App) -> Task<Option<String>> {
        let Some(provider) = self.provider.clone() else { return Task::ready(None) };
        let state = self.state.read(cx);
        let (text, offset) = (state.text().clone(), state.cursor());
        let task = provider.hover(&text, offset, window, cx);
        cx.background_spawn(async move { task.await.ok().flatten().map(|h| hover_text(&h)) })
    }
}

impl Render for EditorTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.diagnostics.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count();
        let warnings = self.diagnostics.len() - errors;
        div()
            .key_context("OrbitEditor")
            .on_action(cx.listener(Self::save))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(c(theme::TEXT_DIM))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(self.display_path.clone()))
                    .when_some(self.last_error.clone(), |d, e| d.child(div().text_color(c(theme::DANGER)).child(e)))
                    .when(errors > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("{errors} errors"))))
                    .when(warnings > 0, |d| d.child(div().text_color(c(theme::WARN)).child(format!("{warnings} warnings"))))
                    .child(div().text_color(c(theme::TEXT_FAINT)).child(self.lsp_status.clone()))
                    .child(
                        div()
                            .id("open-diff")
                            .cursor_pointer()
                            .px_2()
                            .rounded_sm()
                            .hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)))
                            .child("Diff")
                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(EditorEvent::OpenDiff))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(Editor::new(&self.state).size_full().bordered(false)))
    }
}
