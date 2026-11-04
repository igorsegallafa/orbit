//! Editor tab: gpui-component's code editor (tree-sitter highlighting) with
//! save, dirty state and a language server: definition, hover, diagnostics,
//! completion, signature help, code actions, references, symbols,
//! formatting and rename. Plus go to line, problem navigation, the
//! breadcrumb and status bar, clangd's missing compile database banner, and
//! the Markdown preview.
pub mod markdown;
mod picker;
mod provider;

use crate::lsp::{self, path_to_uri, uri_to_path, FileLanguage, LspRegistry, LspServer, RepoKey};
use crate::theme::{self, c};
use anyhow::{bail, Context as _, Result};
use gpui::{
    actions, anchored, deferred, div, point, prelude::*, px, Anchor, App, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Global, KeyBinding, KeyDownEvent, MouseButton, SharedString, Subscription, Task, WeakEntity, Window,
};
use gpui_component::input::{
    CodeActionProvider, Editor, EditorState, Escape, GoToDefinition, HoverProvider, InputEvent, Rope, RopeExt,
    ShowCompletions, TabSize,
};
use orbit_core::lsp::CppSetup;
use picker::{PickItem, Picker, PickerKey, PickerKind};
pub use provider::hover_text;
use provider::{sort_for_apply, targets, workspace_edits, DefinitionHit, DocProvider, LspDoc, ProblemHover, Target};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

actions!(
    orbit_editor,
    [
        Save,
        GoToLine,
        NextProblem,
        PreviousProblem,
        FindReferences,
        GoToImplementation,
        GoToTypeDefinition,
        FileOutline,
        WorkspaceSymbols,
        FormatDocument,
        RenameSymbol,
        SignatureHelp,
        RestartLanguageServer,
        GenerateCompileCommands,
        CodeActions,
    ]
);

const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// An editor command for a command palette or keymap editor: the GPUI action
/// name (`cx.build_action(id, None)`), a title, its default keys and the key
/// context they are bound in.
pub struct EditorCommand {
    pub id: &'static str,
    pub title: &'static str,
    pub keys: &'static [&'static str],
    pub context: &'static str,
}

const fn cmd(id: &'static str, title: &'static str, keys: &'static [&'static str], context: &'static str) -> EditorCommand {
    EditorCommand { id, title, keys, context }
}

/// Every editor command, in palette order. Actions in `OrbitEditor` are
/// handled by the editor tab; `Input` ones by gpui-component's editor.
pub fn editor_actions() -> Vec<EditorCommand> {
    vec![
        cmd("orbit_editor::Save", "Save", &["ctrl-s"], "OrbitEditor"),
        cmd("orbit_editor::GoToLine", "Go to Line…", &["ctrl-g"], "OrbitEditor"),
        cmd("orbit_editor::FileOutline", "Go to Symbol in File…", &["ctrl-shift-o"], "OrbitEditor"),
        cmd("orbit_editor::WorkspaceSymbols", "Go to Symbol in Project…", &["ctrl-t"], "OrbitEditor"),
        cmd("input::GoToDefinition", "Go to Definition", &["f12"], "Input"),
        cmd("orbit_editor::FindReferences", "Find References", &["shift-f12"], "OrbitEditor"),
        cmd("orbit_editor::GoToImplementation", "Go to Implementation", &["ctrl-f12"], "OrbitEditor"),
        cmd("orbit_editor::GoToTypeDefinition", "Go to Type Definition", &[], "OrbitEditor"),
        cmd("orbit_editor::NextProblem", "Next Problem", &["f8"], "OrbitEditor"),
        cmd("orbit_editor::PreviousProblem", "Previous Problem", &["shift-f8"], "OrbitEditor"),
        cmd("input::ShowCompletions", "Trigger Suggest", &["ctrl-space"], "Input"),
        cmd("orbit_editor::SignatureHelp", "Parameter Hints", &["ctrl-shift-space"], "OrbitEditor"),
        cmd("orbit_editor::CodeActions", "Quick Fix…", &["ctrl-.", "alt-enter"], "Input"),
        cmd("orbit_editor::RenameSymbol", "Rename Symbol", &["f2"], "OrbitEditor"),
        cmd("orbit_editor::FormatDocument", "Format Document", &["ctrl-shift-i", "shift-alt-f"], "OrbitEditor"),
        cmd("orbit_editor::RestartLanguageServer", "Restart Language Server", &[], "OrbitEditor"),
        cmd("orbit_editor::GenerateCompileCommands", "Generate compile_commands.json (CMake)", &[], "OrbitEditor"),
    ]
}

/// tree-sitter-cpp's highlight query only adds C++ on top of C's (editors load both);
/// gpui-component registers it alone, so includes, strings and most tokens stay plain.
pub fn register_languages() {
    use gpui_component::highlighter::LanguageRegistry;
    let registry = LanguageRegistry::singleton();
    if let (Some(c), Some(mut cpp)) = (registry.language("c"), registry.language("cpp")) {
        cpp.highlights = format!("{}\n{}", cpp.highlights, c.highlights).into();
        registry.register("cpp", &cpp);
    }
}

pub fn bind_keys(cx: &mut App) {
    let editor = Some("OrbitEditor");
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, editor),
        KeyBinding::new("ctrl-g", GoToLine, editor),
        KeyBinding::new("ctrl-shift-o", FileOutline, editor),
        KeyBinding::new("ctrl-t", WorkspaceSymbols, editor),
        KeyBinding::new("shift-f12", FindReferences, editor),
        KeyBinding::new("ctrl-f12", GoToImplementation, editor),
        KeyBinding::new("f8", NextProblem, editor),
        KeyBinding::new("shift-f8", PreviousProblem, editor),
        KeyBinding::new("ctrl-shift-space", SignatureHelp, editor),
        KeyBinding::new("f2", RenameSymbol, editor),
        KeyBinding::new("ctrl-shift-i", FormatDocument, editor),
        KeyBinding::new("shift-alt-f", FormatDocument, editor),
        KeyBinding::new("f12", GoToDefinition, Some("Input")),
        KeyBinding::new("ctrl-space", ShowCompletions, Some("Input")),
        // Replaces the component's code action menu (ctrl-. → ToggleCodeActions):
        // its Enter also reached the text as a newline.
        KeyBinding::new("ctrl-.", CodeActions, Some("Input")),
        KeyBinding::new("alt-enter", CodeActions, Some("Input")),
    ]);
    markdown::register(cx);
}

/// Reads Settings → Languages' diagnostics filter at start (Settings updates it later).
pub fn load_diagnostic_filters(cx: &mut App) {
    let status = cx.background_spawn(async { orbit_core::lsp::status() });
    cx.spawn(async move |cx| {
        if let Ok(status) = status.await {
            cx.update(|cx| set_diagnostic_filters(&status.languages, cx));
        }
    })
    .detach();
}

/// Per language (`ServerDef::language`): errors only, and the codes to hide.
#[derive(Default)]
struct DiagnosticFilters(HashMap<String, (bool, Vec<String>)>);

impl Global for DiagnosticFilters {}

/// Takes Settings → Languages' filters and re-applies them to every open editor.
pub fn set_diagnostic_filters(languages: &[orbit_core::lsp::LanguageStatus], cx: &mut App) {
    let filters = languages.iter().map(|l| (l.id.clone(), (l.errors_only, l.hidden_diagnostics.clone()))).collect();
    cx.set_global(DiagnosticFilters(filters));
    for editor in open_editors(cx) {
        editor.update(cx, |e, cx| e.show_diagnostics(cx));
    }
}

/// The React editor's rule: no severity counts as an error; codes compare as text.
fn diagnostic_shown(filter: &(bool, Vec<String>), d: &lsp_types::Diagnostic) -> bool {
    let (errors_only, hidden) = filter;
    if *errors_only && d.severity.is_some_and(|s| s != lsp_types::DiagnosticSeverity::ERROR) {
        return false;
    }
    match &d.code {
        Some(lsp_types::NumberOrString::Number(n)) => !hidden.contains(&n.to_string()),
        Some(lsp_types::NumberOrString::String(code)) => !hidden.contains(code),
        None => true,
    }
}

pub enum EditorEvent {
    DirtyChanged,
    /// A location (definition, reference, symbol) in another file.
    OpenLocation { path: PathBuf, range: Option<lsp_types::Range> },
    /// Show this file's changes against HEAD.
    OpenDiff,
}

/// Every open editor, so a rename or a server restart reaches them all.
#[derive(Default)]
struct OpenEditors(Vec<WeakEntity<EditorTab>>);

impl Global for OpenEditors {}

fn open_editors(cx: &mut App) -> Vec<Entity<EditorTab>> {
    let list = cx.default_global::<OpenEditors>();
    list.0.retain(|e| e.upgrade().is_some());
    list.0.iter().filter_map(WeakEntity::upgrade).collect()
}

#[derive(Clone, PartialEq)]
enum LspState {
    /// No server for this language.
    Off,
    Starting,
    /// Not installed, disabled, or failed to start: why.
    Missing(SharedString),
    Ready(SharedString),
}

struct Signature {
    label: String,
    /// Byte range of the active parameter in `label`.
    active: Option<std::ops::Range<usize>>,
    /// The line it was asked on; moving off it hides the popup.
    line: u32,
}

enum CmakeRun {
    Idle,
    Running,
    Failed(String),
    Done,
}

/// clangd without compile_commands.json: what's needed to generate it.
struct CppBanner {
    setup: CppSetup,
    preset: usize,
    run: CmakeRun,
    dismissed: bool,
}

pub struct EditorTab {
    pub path: PathBuf,
    /// "repo/file.ext", or the file name outside a repo.
    pub title: SharedString,
    /// The path shown above the editor.
    pub display_path: SharedString,
    pub state: Entity<EditorState>,
    language: FileLanguage,
    repo: Option<(RepoKey, PathBuf)>,
    registry: LspRegistry,
    saved: Rope,
    dirty: bool,
    crlf: bool,
    tab_size: TabSize,
    doc: Option<Rc<LspDoc>>,
    provider: Option<Rc<DocProvider>>,
    /// Hover (problems, then the server's answer); `problems` is what `show_diagnostics` shows.
    hover: Rc<ProblemHover>,
    /// The last definition the server gave (a Ctrl-hover asks for it), for `on_ctrl_click`.
    definition: Rc<RefCell<Option<DefinitionHit>>>,
    lsp_state: LspState,
    /// Last progress text shown, to repaint only when it changes.
    shown_progress: Option<String>,
    /// The language server's and the last build's together (`set_build_diagnostics`).
    pub diagnostics: Vec<lsp_types::Diagnostic>,
    lsp_diagnostics: Vec<lsp_types::Diagnostic>,
    pub build_diagnostics: Vec<lsp_types::Diagnostic>,
    pub last_error: Option<String>,
    /// A transient line in the status bar (problem text, format and rename results).
    pub message: Option<SharedString>,
    picker: Option<Picker>,
    picker_focus: FocusHandle,
    /// Where a rename started (byte offset).
    rename_at: usize,
    signature: Option<Signature>,
    /// The code actions the picker lists, in its item order.
    code_actions: Vec<lsp_types::CodeAction>,
    cpp: Option<CppBanner>,
    /// `.md` files: the preview and which side is shown.
    markdown: Option<markdown::Preview>,
    _subscriptions: Vec<Subscription>,
    _attach: Task<()>,
    _poll: Task<()>,
    _picker_task: Task<()>,
    _signature_task: Task<()>,
    _cpp_task: Task<()>,
}

impl EventEmitter<EditorEvent> for EditorTab {}

impl Focusable for EditorTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.markdown {
            Some(md) if md.mode == markdown::Mode::Preview => md.focus_handle(cx),
            _ => self.state.read(cx).focus_handle(cx),
        }
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

/// Tabs, or the most common indent step among space-indented lines (default 4).
fn detect_indent(text: &str) -> TabSize {
    let mut tabs = 0;
    let mut steps: HashMap<usize, usize> = HashMap::new();
    let mut prev = 0;
    for line in text.lines().take(2000).filter(|l| !l.trim().is_empty()) {
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = line.len() - line.trim_start_matches(' ').len();
        if n > prev && (2..=8).contains(&(n - prev)) {
            *steps.entry(n - prev).or_default() += 1;
        }
        prev = n;
    }
    let spaced: usize = steps.values().sum();
    if tabs > spaced {
        return TabSize { tab_size: 4, hard_tabs: true };
    }
    let size = steps.into_iter().max_by_key(|&(step, n)| (n, std::cmp::Reverse(step))).map_or(4, |(s, _)| s);
    TabSize { tab_size: size, hard_tabs: false }
}

const SYMBOL_KINDS: [&str; 26] = [
    "", "file", "module", "namespace", "package", "class", "method", "property", "field", "constructor", "enum", "interface",
    "function", "variable", "constant", "string", "number", "boolean", "array", "object", "key", "null", "enum member",
    "struct", "event", "operator",
];

pub(crate) fn symbol_kind(kind: Option<u64>) -> &'static str {
    kind.and_then(|k| SYMBOL_KINDS.get(k as usize).copied()).unwrap_or("type param")
}

impl EditorTab {
    /// `repo`: the repo the file belongs to (workspace and repo name, and the
    /// folder as the workspace has it), which scopes the language server.
    pub fn new(
        path: PathBuf,
        text: String,
        repo: Option<(RepoKey, PathBuf)>,
        registry: &mut LspRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = lsp::file_language(&path);
        let crlf = text.contains("\r\n");
        let tab_size = detect_indent(&text);
        let markdown = markdown::is_markdown(&path).then(|| markdown::Preview::new(&text, cx));
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language.highlight)
                .line_number(true)
                .tab_size(tab_size)
                .default_value(text)
        });
        let saved = state.read(cx).text().clone();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let (title, display_path) = match &repo {
            Some((RepoKey { repo: name, .. }, root)) => {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
                (format!("{name}/{file_name}"), format!("{name}/{rel}"))
            }
            None => (file_name, path.to_string_lossy().to_string()),
        };
        let weak = cx.weak_entity();
        cx.default_global::<OpenEditors>().0.push(weak);

        let mut doc = None;
        let mut provider = None;
        let definition: Rc<RefCell<Option<DefinitionHit>>> = Default::default();
        if let (Some((_, language_id)), Some(_), Some(uri)) = (language.lsp, &repo, path_to_uri(&path)) {
            let d = Rc::new(LspDoc::new(uri, language_id));
            let p = Rc::new(DocProvider(d.clone(), definition.clone()));
            let this = cx.weak_entity();
            *d.apply_edit.borrow_mut() = Some(Rc::new({
                let this = this.clone();
                move |edit, window, cx| {
                    let _ = this.update(cx, |this, cx| this.apply_workspace_edit(edit, window, cx));
                }
            }));
            let own_path = path.clone();
            state.update(cx, |s, _| {
                let lsp = s.lsp_mut();
                lsp.definition_provider = Some(p.clone());
                lsp.completion_provider = Some(p.clone());
                lsp.code_action_providers = vec![p.clone()];
                lsp.completion_menu.max_width = px(520.);
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
            doc = Some(d);
            provider = Some(p);
        }
        let hover = Rc::new(ProblemHover { doc: doc.clone(), problems: Default::default() });
        state.update(cx, |s, _| s.lsp_mut().hover_provider = Some(hover.clone()));

        let subscriptions = vec![
            cx.subscribe(&state, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_change(cx);
                }
            }),
            // The component shows a problem under the pointer as a popover over
            // the line above, and the hover popover lands on top of it. Monaco
            // puts the message in the hover instead (`ProblemHover`), so its
            // popover is closed as soon as it opens, before it is drawn.
            cx.observe(&state, |_, state, cx| {
                if state.read(cx).diagnostic_popover().is_some() {
                    state.update(cx, |s, cx| s.clear_diagnostic_popover(cx));
                }
            }),
        ];
        let mut this = Self {
            path,
            title: title.into(),
            display_path: display_path.into(),
            state,
            language,
            repo,
            registry: registry.clone(),
            saved,
            dirty: false,
            crlf,
            tab_size,
            doc,
            provider,
            hover,
            definition,
            lsp_state: LspState::Off,
            shown_progress: None,
            diagnostics: vec![],
            lsp_diagnostics: vec![],
            build_diagnostics: vec![],
            last_error: None,
            message: None,
            picker: None,
            picker_focus: cx.focus_handle(),
            rename_at: 0,
            signature: None,
            code_actions: vec![],
            cpp: None,
            markdown,
            _subscriptions: subscriptions,
            _attach: Task::ready(()),
            _poll: Task::ready(()),
            _picker_task: Task::ready(()),
            _signature_task: Task::ready(()),
            _cpp_task: Task::ready(()),
        };
        this.connect(cx);
        this
    }

    fn server_key(&self) -> Option<(&RepoKey, &'static str)> {
        Some((&self.repo.as_ref()?.0, self.language.lsp?.0))
    }

    /// Asks the registry for this file's server and attaches when it's up.
    fn connect(&mut self, cx: &mut Context<Self>) {
        let (Some(doc), Some((key, language))) = (&self.doc, self.server_key()) else { return };
        *doc.server.borrow_mut() = None;
        let server = self.registry.server(key, language, cx);
        self.lsp_state = LspState::Starting;
        self._attach = cx.spawn(async move |this, cx| {
            let server = server.await;
            let _ = this.update(cx, |this, cx| this.attach(server, cx));
        });
        cx.notify();
    }

    fn attach(&mut self, server: Result<Arc<LspServer>, String>, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let server = match server {
            Ok(server) => server,
            Err(e) => {
                self.lsp_state = LspState::Missing(e.into());
                cx.notify();
                return;
            }
        };
        // didOpen carries the text as it is now, edits made while starting included.
        doc.open(&server, self.state.read(cx).text());
        let name = Path::new(&server.command.split(' ').next().unwrap_or_default())
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.lsp_state = LspState::Ready(name.into());
        *doc.server.borrow_mut() = Some(server.clone());
        // Repaint on progress (indexing) and when the server goes away.
        self._poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(700)).await;
            let progress = server.progress();
            let exited = server.has_exited();
            let alive = this.update(cx, |this, cx| {
                if this.shown_progress != progress || exited {
                    this.shown_progress = progress;
                    cx.notify();
                }
            });
            if alive.is_err() || exited {
                break;
            }
        });
        if self.language.lsp.is_some_and(|(l, _)| l == "cpp") {
            self.check_cpp_setup(cx);
        }
        cx.notify();
    }

    pub fn lsp_attached(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.server().is_some())
    }

    fn server(&self) -> Option<Arc<LspServer>> {
        self.doc.as_ref()?.server()
    }

    /// The server's name, why it isn't running, or "<language>: starting".
    pub fn lsp_status(&self) -> SharedString {
        match &self.lsp_state {
            LspState::Off => "".into(),
            LspState::Starting => format!("{}: starting", self.language.lsp.map_or("", |l| l.0)).into(),
            LspState::Missing(e) => e.clone(),
            LspState::Ready(name) => name.clone(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn on_change(&mut self, cx: &mut Context<Self>) {
        let text = self.state.read(cx).text().clone();
        let dirty = text != self.saved;
        if let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d, d.server()?))) {
            doc.sync(&text);
            // Parameter hints open on `(`/`,` and follow the call while shown.
            let cursor = self.state.read(cx).cursor();
            let before = text.chars_at(cursor).reversed().next().map(String::from).unwrap_or_default();
            if self.signature.is_some() || server.trigger_characters("signatureHelpProvider").contains(&before) {
                self.request_signature(cx);
            }
        }
        if dirty != self.dirty {
            self.dirty = dirty;
            cx.emit(EditorEvent::DirtyChanged);
        }
        self.message = None;
        self.schedule_markdown(cx);
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

    /// Takes the file from disk when it changed there and has no unsaved
    /// edits (an agent, git, another editor); the cursor stays on its line.
    pub fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.dirty {
            return false;
        }
        let Ok(text) = read_text(&self.path) else { return false };
        let saved = self.saved.to_string();
        if text == saved || text.replace("\r\n", "\n") == saved {
            return false;
        }
        let cursor = self.state.read(cx).cursor_position();
        self.state.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            let line = cursor.line.min(s.text().lines_len().saturating_sub(1) as u32);
            let at = s.text().position_to_offset(&lsp_types::Position::new(line, cursor.character));
            s.set_selected_range(at..at, cx);
        });
        self.saved = self.state.read(cx).text().clone();
        if let Some(doc) = &self.doc {
            doc.sync(&self.saved);
        }
        self.message = Some("Reloaded from disk".into());
        self.schedule_markdown(cx);
        cx.notify();
        true
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<lsp_types::Diagnostic>, cx: &mut Context<Self>) {
        if let Some(doc) = &self.doc {
            *doc.diagnostics.borrow_mut() = diagnostics.clone();
        }
        self.lsp_diagnostics = diagnostics;
        self.show_diagnostics(cx);
    }

    /// Problems from the last build (`run::apply_problems`), shown with the server's.
    pub fn set_build_diagnostics(&mut self, diagnostics: Vec<lsp_types::Diagnostic>, cx: &mut Context<Self>) {
        self.build_diagnostics = diagnostics;
        self.show_diagnostics(cx);
    }

    fn show_diagnostics(&mut self, cx: &mut Context<Self>) {
        let filter = self.language.lsp.and_then(|(language, _)| cx.try_global::<DiagnosticFilters>()?.0.get(language).cloned());
        let lsp = self.lsp_diagnostics.iter().filter(|d| filter.as_ref().is_none_or(|f| diagnostic_shown(f, d)));
        let diagnostics: Vec<_> = lsp.chain(&self.build_diagnostics).cloned().collect();
        self.diagnostics = diagnostics.clone();
        self.diagnostics.sort_by_key(|d| (d.range.start.line, d.range.start.character));
        *self.hover.problems.borrow_mut() = self.diagnostics.clone();
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
        if self.markdown_mode() == Some(markdown::Mode::Preview) {
            self.set_markdown_mode(markdown::Mode::Editor, window, cx);
        }
        self.state.update(cx, |s, cx| s.set_cursor_position(range.start, window, cx));
    }

    /// 1-based line and column of the cursor.
    pub fn cursor(&self, cx: &App) -> (u32, u32) {
        let p = self.state.read(cx).cursor_position();
        (p.line + 1, p.character + 1)
    }

    /// Hover text at the cursor, asked of the server directly (self-test).
    pub fn hover_at_cursor(&self, window: &mut Window, cx: &mut App) -> Task<Option<String>> {
        let provider = self.hover.clone();
        let state = self.state.read(cx);
        let (text, offset) = (state.text().clone(), state.cursor());
        let task = provider.hover(&text, offset, window, cx);
        cx.background_spawn(async move { task.await.ok().flatten().map(|h| hover_text(&h)) })
    }

    fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.focus(window, cx));
    }

    fn rel_path(&self, path: &Path) -> String {
        match &self.repo {
            Some((key, root)) => path.strip_prefix(root).map_or_else(|_| path.display().to_string(), |r| format!("{}/{}", key.repo, r.display())),
            None => path.display().to_string(),
        }
    }

    /// Same file: move the cursor. Another: ask the app to open it.
    fn open_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if target.path == self.path {
            self.reveal(target.range, window, cx);
        } else {
            cx.emit(EditorEvent::OpenLocation { path: target.path, range: Some(target.range) });
        }
    }

    /// Ctrl/Cmd+click on a symbol whose definition the Ctrl-hover found. The
    /// component follows it too, but only after any open popover (hover,
    /// problem) has taken the press as a click outside and cleared the found
    /// definition, so a click on a hovered symbol or a line with a problem did
    /// nothing. This runs first, in the capture phase.
    fn on_ctrl_click(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || !event.modifiers.secondary(cx) || event.modifiers.alt || event.modifiers.shift {
            return;
        }
        let link = {
            let hit = self.definition.borrow();
            let state = self.state.read(cx);
            hit.as_ref()
                .filter(|h| h.text == *state.text())
                .filter(|h| state.range_to_bounds(&h.symbol).is_some_and(|b| b.contains(&event.position)))
                .and_then(|h| h.links.first().cloned())
        };
        let Some(link) = link else { return };
        window.prevent_default();
        cx.stop_propagation();
        self.state.update(cx, |s, cx| s.clear_hover_state(cx));
        let Some(path) = uri_to_path(&link.target_uri) else { return };
        self.open_target(Target { path, range: link.target_selection_range }, window, cx);
    }

    // ---------- edits ----------

    fn apply_text_edits(&mut self, edits: Vec<lsp_types::TextEdit>, keep_cursor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let edits = sort_for_apply(edits);
        self.state.update(cx, |s, cx| {
            let pos = s.cursor_position();
            s.apply_lsp_edits(&edits, window, cx);
            if keep_cursor {
                s.set_cursor_position(pos, window, cx);
            }
        });
    }

    /// A `WorkspaceEdit`: this file in the buffer, other open files in their
    /// editors, the rest on disk. Returns (edits, files).
    fn apply_workspace_edit(&mut self, edit: Value, window: &mut Window, cx: &mut Context<Self>) -> (usize, usize) {
        let me = cx.entity_id();
        let changes = workspace_edits(&edit);
        let (mut edits, files) = (0, changes.len());
        let mut errors = vec![];
        for (path, list) in changes {
            edits += list.len();
            if path == self.path {
                self.apply_text_edits(list, true, window, cx);
                continue;
            }
            let other = open_editors(cx).into_iter().find(|e| e.entity_id() != me && e.read(cx).path == path);
            match other {
                Some(other) => other.update(cx, |o, cx| o.apply_text_edits(list, false, window, cx)),
                None => {
                    let written = read_text(&path).and_then(|text| Ok(std::fs::write(&path, provider::apply_to_text(&text, list))?));
                    if let Err(e) = written {
                        errors.push(format!("{}: {e:#}", path.display()));
                    }
                }
            }
        }
        if !errors.is_empty() {
            self.last_error = Some(errors.join("; "));
        }
        cx.notify();
        (edits, files)
    }

    // ---------- pickers ----------

    fn open_picker(&mut self, kind: PickerKind, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = Some(Picker::new(kind, query));
        self._picker_task = Task::ready(());
        window.focus(&self.picker_focus, cx);
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self._picker_task = Task::ready(());
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn set_picker_status(&mut self, status: Option<String>) {
        if let Some(p) = &mut self.picker {
            p.status = status.map(Into::into);
        }
    }

    fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerKind::GoToLine, String::new(), window, cx);
        self.update_line_hint(cx);
    }

    fn update_line_hint(&mut self, cx: &App) {
        let lines = self.state.read(cx).text().lines_len();
        let (line, col) = self.cursor(cx);
        self.set_picker_status(Some(format!("Current line: {line}, column {col}. Type a line between 1 and {lines}, Enter to go")));
    }

    fn file_outline(&mut self, _: &FileOutline, window: &mut Window, cx: &mut Context<Self>) {
        let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d.clone(), d.server()?))) else {
            self.message = Some("No language server for symbols".into());
            return cx.notify();
        };
        self.open_picker(PickerKind::Outline, String::new(), window, cx);
        self.set_picker_status(Some("Loading symbols…".into()));
        let request = server.request("textDocument/documentSymbol", json!({ "textDocument": doc.text_document() }));
        let path = self.path.clone();
        self._picker_task = cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                let items = match result {
                    Ok(v) => outline_items(&v, &path),
                    Err(e) => {
                        this.set_picker_status(Some(format!("{e:#}")));
                        return cx.notify();
                    }
                };
                if let Some(p) = &mut this.picker {
                    p.status = items.is_empty().then(|| "No symbols".into());
                    p.set_items(items);
                }
                cx.notify();
            });
        });
    }

    fn workspace_symbols(&mut self, _: &WorkspaceSymbols, window: &mut Window, cx: &mut Context<Self>) {
        if self.server().is_none() {
            self.message = Some("No language server for project symbols".into());
            return cx.notify();
        }
        // Start from the word under the cursor, like the Tauri app's selection seed.
        let query = {
            let s = self.state.read(cx);
            let sel = s.selected_text().to_string();
            if sel.is_empty() || sel.contains('\n') { String::new() } else { sel }
        };
        self.open_picker(PickerKind::WorkspaceSymbols, query, window, cx);
        self.search_workspace_symbols(cx);
    }

    /// Asks the server after a short pause in typing.
    fn search_workspace_symbols(&mut self, cx: &mut Context<Self>) {
        let (Some(server), Some(p)) = (self.server(), &mut self.picker) else { return };
        let query = p.query.trim().to_string();
        if query.is_empty() {
            p.set_items(vec![]);
            p.status = Some("Type a symbol name".into());
            return;
        }
        p.status = Some("Searching…".into());
        self._picker_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let result = server.request("workspace/symbol", json!({ "query": query })).await;
            let _ = this.update(cx, |this, cx| {
                let items = match result {
                    Ok(v) => v.as_array().cloned().unwrap_or_default().iter().take(200).filter_map(|s| this.symbol_item(s)).collect(),
                    Err(e) => {
                        this.set_picker_status(Some(format!("{e:#}")));
                        return cx.notify();
                    }
                };
                if let Some(p) = &mut this.picker {
                    p.status = Vec::is_empty(&items).then(|| "No symbols found".into());
                    p.set_items(items);
                }
                cx.notify();
            });
        });
    }

    fn symbol_item(&self, s: &Value) -> Option<PickItem> {
        let uri: lsp_types::Uri = s.pointer("/location/uri")?.as_str()?.parse().ok()?;
        let path = uri_to_path(&uri)?;
        let range: lsp_types::Range = s.pointer("/location/range").and_then(|r| serde_json::from_value(r.clone()).ok()).unwrap_or_default();
        let container = s.get("containerName").and_then(Value::as_str).filter(|c| !c.is_empty());
        let kind = symbol_kind(s.get("kind").and_then(Value::as_u64));
        let detail = match container {
            Some(c) => format!("{kind} · {c} · {}:{}", self.rel_path(&path), range.start.line + 1),
            None => format!("{kind} · {}:{}", self.rel_path(&path), range.start.line + 1),
        };
        Some(PickItem { label: s.get("name")?.as_str()?.to_string().into(), detail: detail.into(), depth: 0, target: Some(Target { path, range }) })
    }

    fn find_references(&mut self, _: &FindReferences, window: &mut Window, cx: &mut Context<Self>) {
        self.goto_locations("textDocument/references", "References", window, cx);
    }

    fn go_to_implementation(&mut self, _: &GoToImplementation, window: &mut Window, cx: &mut Context<Self>) {
        self.goto_locations("textDocument/implementation", "Implementations", window, cx);
    }

    fn go_to_type_definition(&mut self, _: &GoToTypeDefinition, window: &mut Window, cx: &mut Context<Self>) {
        self.goto_locations("textDocument/typeDefinition", "Type definitions", window, cx);
    }

    /// One result opens directly (except references); several open a list.
    fn goto_locations(&mut self, method: &'static str, title: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d.clone(), d.server()?))) else {
            self.message = Some("No language server".into());
            return cx.notify();
        };
        let state = self.state.read(cx);
        let (text, offset) = (state.text().clone(), state.cursor());
        let word = text.word_at(offset);
        let mut params = doc.position_params(&text, offset);
        if method == "textDocument/references" {
            params["context"] = json!({ "includeDeclaration": true });
        }
        let request = server.request(method, params);
        let kind = PickerKind::Locations { title: format!("{title} of {word}").into() };
        self.open_picker(kind, String::new(), window, cx);
        self.set_picker_status(Some("Searching…".into()));
        let own = (self.path.clone(), text);
        let root = self.repo.clone();
        self._picker_task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let found = match result {
                Ok(v) => targets(v),
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_picker_status(Some(format!("{e:#}")));
                        cx.notify();
                    });
                    return;
                }
            };
            if found.len() == 1 && method != "textDocument/references" {
                let target = found.into_iter().next().unwrap();
                let _ = this.update_in(cx, |this, window, cx| {
                    this.close_picker(window, cx);
                    this.open_target(target, window, cx);
                });
                return;
            }
            let items = cx.background_spawn(async move { location_items(found, &own, root.as_ref()) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = &mut this.picker {
                    p.status = Some(match items.len() {
                        0 => "No results".to_string(),
                        1 => "1 result".to_string(),
                        n => format!("{n} results"),
                    }.into());
                    p.set_items(items);
                }
                cx.notify();
            });
        });
    }

    fn rename_symbol(&mut self, _: &RenameSymbol, window: &mut Window, cx: &mut Context<Self>) {
        if self.server().is_none() {
            self.message = Some("No language server to rename with".into());
            return cx.notify();
        }
        let state = self.state.read(cx);
        let offset = state.cursor();
        let word = state.text().word_at(offset);
        if word.is_empty() {
            self.message = Some("Put the cursor on a symbol to rename it".into());
            return cx.notify();
        }
        self.rename_at = offset;
        self.open_picker(PickerKind::Rename, word.clone(), window, cx);
        self.set_picker_status(Some(format!("Rename '{word}' everywhere it's used; Enter to apply")));
    }

    fn do_rename(&mut self, new_name: String, cx: &mut Context<Self>, window: &mut Window) {
        let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d.clone(), d.server()?))) else { return };
        let text = self.state.read(cx).text().clone();
        let mut params = doc.position_params(&text, self.rename_at.min(text.len()));
        params["newName"] = json!(new_name);
        let request = server.request("textDocument/rename", params);
        self._picker_task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.message = Some(match result {
                    Ok(Value::Null) => "Nothing to rename here".into(),
                    Ok(edit) => {
                        let (edits, files) = this.apply_workspace_edit(edit, window, cx);
                        format!("Renamed {edits} occurrences in {files} files").into()
                    }
                    Err(e) => format!("Rename failed: {e:#}").into(),
                });
                cx.notify();
            });
        });
    }

    fn show_code_actions(&mut self, _: &CodeActions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.provider.clone().filter(|_| self.server().is_some()) else {
            self.message = Some("No language server for quick fixes".into());
            return cx.notify();
        };
        let range = self.state.read(cx).selected_range();
        let task = provider.code_actions(self.state.clone(), range, window, cx);
        self.open_picker(PickerKind::CodeActions, String::new(), window, cx);
        self.set_picker_status(Some("Loading…".into()));
        self._picker_task = cx.spawn(async move |this, cx| {
            let actions = task.await;
            let _ = this.update(cx, |this, cx| {
                let actions = match actions {
                    Ok(a) => a,
                    Err(e) => {
                        this.set_picker_status(Some(format!("{e:#}")));
                        return cx.notify();
                    }
                };
                let items = actions
                    .iter()
                    .map(|a| PickItem {
                        label: a.title.clone().into(),
                        detail: a.kind.as_ref().map(|k| k.as_str().to_string()).unwrap_or_default().into(),
                        depth: 0,
                        target: None,
                    })
                    .collect();
                this.code_actions = actions;
                if let Some(p) = &mut this.picker {
                    p.status = this.code_actions.is_empty().then(|| "No quick fixes here".into());
                    p.set_items(items);
                }
                cx.notify();
            });
        });
    }

    fn on_picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.picker else { return };
        let outcome = p.on_key(event, cx);
        if !matches!(outcome, PickerKey::Ignored) {
            cx.stop_propagation();
        }
        match outcome {
            PickerKey::Edited => match p.kind {
                PickerKind::WorkspaceSymbols => self.search_workspace_symbols(cx),
                PickerKind::GoToLine => self.update_line_hint(cx),
                _ => {}
            },
            PickerKey::Accept => self.accept_picker(None, window, cx),
            PickerKey::Close => self.close_picker(window, cx),
            PickerKey::Handled | PickerKey::Ignored => {}
        }
        cx.notify();
    }

    fn accept_picker(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.picker.take() else { return };
        if let Some(row) = row {
            p.selected = row;
        }
        self.close_picker(window, cx);
        match p.kind {
            PickerKind::GoToLine => {
                let mut parts = p.query.trim().split([':', ',']).map(|s| s.trim().parse::<u32>().ok());
                if let Some(Some(line)) = parts.next() {
                    let col = parts.next().flatten().unwrap_or(1);
                    let line = line.clamp(1, self.state.read(cx).text().lines_len() as u32);
                    let pos = lsp_types::Position::new(line - 1, col.saturating_sub(1));
                    self.reveal(lsp_types::Range::new(pos, pos), window, cx);
                }
            }
            PickerKind::Rename => {
                let name = p.query.trim().to_string();
                if !name.is_empty() {
                    self.do_rename(name, cx, window);
                }
            }
            PickerKind::CodeActions => {
                let action = p.selected_index().and_then(|ix| self.code_actions.get(ix).cloned());
                if let (Some(action), Some(provider)) = (action, self.provider.clone()) {
                    let task = provider.perform_code_action(self.state.clone(), action, true, window, cx);
                    cx.spawn(async move |_, _| {
                        let _ = task.await;
                    })
                    .detach();
                }
            }
            _ => {
                if let Some(target) = p.selected_item().and_then(|i| i.target.clone()) {
                    self.open_target(target, window, cx);
                }
            }
        }
    }

    // ---------- problems, format, signature help ----------

    fn next_problem(&mut self, _: &NextProblem, window: &mut Window, cx: &mut Context<Self>) {
        self.jump_to_problem(true, window, cx);
    }

    fn previous_problem(&mut self, _: &PreviousProblem, window: &mut Window, cx: &mut Context<Self>) {
        self.jump_to_problem(false, window, cx);
    }

    /// The next (or previous) diagnostic from the cursor, wrapping around.
    fn jump_to_problem(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.diagnostics.is_empty() {
            self.message = Some("No problems in this file".into());
            return cx.notify();
        }
        let cur = self.state.read(cx).cursor_position();
        let at = |d: &lsp_types::Diagnostic| (d.range.start.line, d.range.start.character);
        let here = (cur.line, cur.character);
        let d = if forward {
            self.diagnostics.iter().find(|d| at(d) > here).or(self.diagnostics.first())
        } else {
            self.diagnostics.iter().rev().find(|d| at(d) < here).or(self.diagnostics.last())
        };
        let Some(d) = d.cloned() else { return };
        let ix = self.diagnostics.iter().position(|x| *x == d).unwrap_or(0) + 1;
        let icon = if d.severity == Some(lsp_types::DiagnosticSeverity::ERROR) { "⨯" } else { "⚠" };
        self.message = Some(format!("{icon} {ix}/{} {}", self.diagnostics.len(), d.message.lines().next().unwrap_or_default()).into());
        self.reveal(d.range, window, cx);
        // The message in the hover popover, as hovering the squiggle shows it.
        let here: Vec<_> = self.diagnostics.iter().filter(|x| x.range == d.range).cloned().collect();
        if let Some(hover) = provider::with_problems(&here, None) {
            self.state.update(cx, |s, cx| {
                let text = s.text();
                let range = text.position_to_offset(&d.range.start)..text.position_to_offset(&d.range.end);
                s.present_hover(range, hover, cx);
            });
        }
        cx.notify();
    }

    fn format_document(&mut self, _: &FormatDocument, window: &mut Window, cx: &mut Context<Self>) {
        let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d.clone(), d.server()?))) else {
            self.message = Some("No language server to format with".into());
            return cx.notify();
        };
        if server.capability("/documentFormattingProvider").is_none() {
            self.message = Some("This language server doesn't format".into());
            return cx.notify();
        }
        let options = json!({ "tabSize": self.tab_size.tab_size, "insertSpaces": !self.tab_size.hard_tabs });
        let request = server.request("textDocument/formatting", json!({ "textDocument": doc.text_document(), "options": options }));
        let version = doc.version.get();
        self._picker_task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.message = Some(match result.map(serde_json::from_value::<Option<Vec<lsp_types::TextEdit>>>) {
                    // Edits for an older text would land in the wrong places.
                    _ if this.doc.as_ref().is_some_and(|d| d.version.get() != version) => "Format skipped: the file changed meanwhile".into(),
                    Ok(Ok(Some(edits))) if !edits.is_empty() => {
                        let n = edits.len();
                        this.apply_text_edits(edits, true, window, cx);
                        format!("Formatted ({n} edits)").into()
                    }
                    Ok(Ok(_)) => "Already formatted".into(),
                    Ok(Err(e)) => format!("Format failed: {e}").into(),
                    Err(e) => format!("Format failed: {e:#}").into(),
                });
                cx.notify();
            });
        });
    }

    fn signature_help(&mut self, _: &SignatureHelp, _: &mut Window, cx: &mut Context<Self>) {
        self.request_signature(cx);
    }

    fn request_signature(&mut self, cx: &mut Context<Self>) {
        let Some((doc, server)) = self.doc.as_ref().and_then(|d| Some((d.clone(), d.server()?))) else { return };
        let state = self.state.read(cx);
        let (text, offset) = (state.text().clone(), state.cursor());
        let line = text.offset_to_position(offset).line;
        let request = server.request("textDocument/signatureHelp", doc.position_params(&text, offset));
        self._signature_task = cx.spawn(async move |this, cx| {
            let help = request.await.ok().and_then(|v| serde_json::from_value::<Option<lsp_types::SignatureHelp>>(v).ok().flatten());
            let _ = this.update(cx, |this, cx| {
                this.signature = help.and_then(|h| signature(&h, line));
                cx.notify();
            });
        });
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.clear_hover_state(cx));
        if self.signature.take().is_some() || self.message.take().is_some() {
            cx.notify();
        }
    }

    // ---------- server restart, clangd setup ----------

    fn restart_server(&mut self, _: &RestartLanguageServer, _: &mut Window, cx: &mut Context<Self>) {
        self.restart(cx);
    }

    /// Restarts this file's server and reconnects every editor that used it.
    fn restart(&mut self, cx: &mut Context<Self>) {
        let Some((key, language)) = self.server_key().map(|(k, l)| (k.clone(), l)) else { return };
        self.registry.restart(&key, language);
        let me = cx.entity_id();
        let others: Vec<_> = open_editors(cx).into_iter().filter(|e| e.entity_id() != me).collect();
        self.connect(cx);
        cx.defer(move |cx| {
            for e in others {
                e.update(cx, |e, cx| {
                    if e.server_key().is_some_and(|(k, l)| *k == key && l == language) {
                        e.connect(cx);
                    }
                });
            }
        });
    }

    fn check_cpp_setup(&mut self, cx: &mut Context<Self>) {
        let Some((key, _)) = &self.repo else { return };
        let (ws, repo) = (key.workspace.clone(), key.repo.clone());
        let task = cx.background_spawn(async move { orbit_core::lsp::cpp_setup(&ws, &repo) });
        self._cpp_task = cx.spawn(async move |this, cx| {
            let setup = task.await;
            let _ = this.update(cx, |this, cx| {
                this.cpp = match setup {
                    Ok(setup) if setup.compile_commands.is_none() => {
                        let dismissed = this.cpp.as_ref().is_some_and(|b| b.dismissed);
                        Some(CppBanner { setup, preset: 0, run: CmakeRun::Idle, dismissed })
                    }
                    _ => None,
                };
                cx.notify();
            });
        });
    }

    /// Runs CMake to write compile_commands.json, then restarts clangd.
    fn generate_compile_commands(&mut self, _: &GenerateCompileCommands, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(banner), Some((_, root))) = (&mut self.cpp, &self.repo) else {
            self.message = Some("compile_commands.json is already there".into());
            return cx.notify();
        };
        if matches!(banner.run, CmakeRun::Running) || !banner.setup.cmake {
            return;
        }
        let setup = &banner.setup;
        let args: Vec<String> = match setup.presets.get(banner.preset) {
            Some(p) => vec!["--preset".into(), p.clone(), "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON".into()],
            None => {
                let mut a: Vec<String> = vec!["-S".into(), ".".into(), "-B".into(), "build".into()];
                // Visual Studio's Ninja is only on PATH inside its developer environment.
                if setup.ninja_installed || setup.vcvars.is_some() {
                    a.extend(["-G".into(), "Ninja".into()]);
                }
                a.push("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON".into());
                a
            }
        };
        let vcvars = setup.vcvars.clone();
        let dir = orbit_core::lsp::real_path(root);
        banner.run = CmakeRun::Running;
        let task = cx.background_spawn(async move {
            match vcvars {
                Some(vcvars) => {
                    let script = orbit_core::lsp::write_cmake_script(&vcvars, &args)?;
                    orbit_core::proc::run("cmd", &["/d", "/c", &script], Some(&dir))
                }
                None => orbit_core::proc::run("cmake", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(&dir)),
            }
        });
        self._cpp_task = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(b) = &mut this.cpp {
                    b.run = match result {
                        Ok(_) => CmakeRun::Done,
                        Err(e) => CmakeRun::Failed(e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("cmake failed").to_string()),
                    };
                }
                if this.cpp.as_ref().is_some_and(|b| matches!(b.run, CmakeRun::Done)) {
                    this.message = Some("compile_commands.json generated; clangd restarted".into());
                    this.restart(cx);
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    // ---------- self-test views of the state ----------

    /// The open picker as text: title, query, status and the first rows.
    pub fn picker_text(&self) -> Option<String> {
        let p = self.picker.as_ref()?;
        let mut out = format!("{} [{}] {}\n", p.title(), p.query, p.status.as_deref().unwrap_or(""));
        for (pos, &ix) in p.shown.iter().enumerate().take(12) {
            let item = &p.items[ix];
            let mark = if pos == p.selected { ">" } else { " " };
            out.push_str(&format!("  {mark}{}{}  {}\n", "  ".repeat(item.depth), item.label, item.detail));
        }
        Some(out)
    }

    pub fn signature_text(&self) -> Option<String> {
        let s = self.signature.as_ref()?;
        Some(match &s.active {
            Some(r) => format!("{}[{}]{}", &s.label[..r.start], &s.label[r.clone()], &s.label[r.end..]),
            None => s.label.clone(),
        })
    }

    pub fn cpp_banner_text(&self) -> Option<String> {
        let b = self.cpp.as_ref().filter(|b| !b.dismissed)?;
        let run = match &b.run {
            CmakeRun::Idle => "idle".to_string(),
            CmakeRun::Running => "running".to_string(),
            CmakeRun::Failed(e) => format!("failed: {e}"),
            CmakeRun::Done => "done".to_string(),
        };
        Some(format!("no compile_commands.json · cmake {} · presets {:?} · run {run}", b.setup.cmake, b.setup.presets))
    }

    /// The status bar as text.
    pub fn status_text(&self, cx: &App) -> String {
        let (errors, warnings) = self.problem_counts();
        let (line, col) = self.cursor(cx);
        format!(
            "⨯ {errors} ⚠ {warnings}{} | Ln {line}, Col {col} | {} | {} | {} | {}",
            if self.dirty { " | Unsaved" } else { "" },
            if self.crlf { "CRLF" } else { "LF" },
            self.indent_label(),
            self.language.name,
            self.chip_text(),
        )
    }

    fn problem_counts(&self) -> (usize, usize) {
        let errors = self.diagnostics.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count();
        let warnings = self.diagnostics.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)).count();
        (errors, warnings)
    }

    fn indent_label(&self) -> String {
        if self.tab_size.hard_tabs { format!("Tab: {}", self.tab_size.tab_size) } else { format!("Spaces: {}", self.tab_size.tab_size) }
    }

    fn chip_text(&self) -> String {
        match &self.lsp_state {
            LspState::Off => String::new(),
            LspState::Starting => "Starting language server".into(),
            LspState::Missing(_) => "No language server".into(),
            LspState::Ready(name) => match self.doc.as_ref().and_then(|d| d.server.borrow().clone()) {
                Some(s) if s.has_exited() => format!("{name} stopped · Restart"),
                Some(s) => s.progress().map_or_else(|| name.to_string(), |p| format!("{name} · {p}")),
                None => name.to_string(),
            },
        }
    }

    // ---------- rendering ----------

    fn render_breadcrumb(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let parts: Vec<String> = self.display_path.split('/').map(str::to_string).collect();
        let last = parts.len().saturating_sub(1);
        // Crumb i's folder: the repo root, then the folders under it.
        let folders: Vec<Option<PathBuf>> = match &self.repo {
            Some((_, root)) => (0..last).map(|i| Some(parts[1..=i].iter().fold(root.clone(), |p, part| p.join(part)))).collect(),
            None => vec![],
        };
        // `.breadcrumb`, `button.crumb`, `.crumb-sep`.
        let crumb = |id: gpui::ElementId| {
            div()
                .id(id)
                .relative()
                .h(px(22.))
                .px(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .text_size(px(13.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.))
            .text_color(c(theme::TEXT_DIM))
            .children(parts.into_iter().enumerate().map(move |(i, part)| {
                let wrap = div().flex().items_center().when(i > 0, |d| d.child(div().px(px(3.)).text_color(c(theme::TEXT_FAINT)).child("›")));
                if i == last {
                    wrap.child(
                        crumb("crumb-file".into())
                            .text_color(c(theme::TEXT))
                            .child(part)
                            .on_click(cx.listener(|this, _, window, cx| this.file_outline(&FileOutline, window, cx))),
                    )
                } else if let Some(dir) = folders.get(i).cloned().flatten() {
                    // A folder: shown in the file tree.
                    wrap.child(
                        crumb(("crumb-dir", i).into())
                            .child(part)
                            .child(crate::probe::probe(format!("crumb:{i}")))
                            .on_click(move |_, window, cx| crate::file_ops::reveal_in_tree(dir.clone(), window, cx)),
                    )
                } else {
                    wrap.child(div().px(px(4.)).text_size(px(13.)).child(part))
                }
            }))
    }

    fn render_cpp_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let b = self.cpp.as_ref().filter(|b| !b.dismissed)?;
        let repo = self.repo.as_ref().map(|(k, _)| k.repo.clone()).unwrap_or_default();
        let button = |id: &'static str, label: String, primary: bool| {
            let (bg, hover, fg) = if primary {
                (c(theme::ACCENT), c(theme::ACCENT_HOVER), c(theme::ON_ACCENT))
            } else {
                (c(theme::BG), c(theme::PANEL_RAISED), c(theme::TEXT))
            };
            div()
                .id(id)
                .flex_none()
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .bg(bg)
                .text_color(fg)
                .hover(move |s| s.bg(hover))
                .child(label)
        };
        let action: gpui::AnyElement = match &b.run {
            CmakeRun::Running => div().text_color(c(theme::TEXT_FAINT)).child("Running CMake…").into_any_element(),
            CmakeRun::Done => button("cpp-restart", "Restart clangd".into(), true)
                .on_click(cx.listener(|this, _, _, cx| this.restart(cx)))
                .into_any_element(),
            run if b.setup.cmake && (b.setup.cmake_installed || b.setup.vcvars.is_some()) => {
                let preset = b.setup.presets.get(b.preset).cloned();
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .when_some(preset.clone(), |d, p| {
                        d.child(
                            button("cpp-preset", format!("{p} ▾"), false)
                                .border_1()
                                .border_color(c(theme::BORDER_STRONG))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(b) = &mut this.cpp {
                                        b.preset = (b.preset + 1) % b.setup.presets.len().max(1);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        button("cpp-generate", "Generate with CMake".into(), true)
                            .on_click(cx.listener(|this, _, window, cx| this.generate_compile_commands(&GenerateCompileCommands, window, cx))),
                    )
                    .when_some(if let CmakeRun::Failed(e) = run { Some(e.clone()) } else { None }, |d, e| {
                        d.child(div().text_color(c(theme::DANGER)).child(e))
                    })
                    .into_any_element()
            }
            _ => div()
                .text_color(c(theme::TEXT_FAINT))
                .child(if b.setup.cmake { "Install CMake to generate it." } else { "Generate it with your build system (CMake, or bear -- make)." })
                .into_any_element(),
        };
        Some(
            div()
                .flex_none()
                .px(px(14.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.))
                .bg(theme::hsla(theme::WARN).opacity(0.07))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .text_color(c(theme::TEXT_DIM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(gpui::relative(1.45))
                        .child(
                            gpui::StyledText::new(format!(
                                "clangd has no compile_commands.json for {repo}, so it guesses include paths and flags: some navigation and errors will be off."
                            ))
                            .with_highlights([(0..36, gpui::HighlightStyle { color: Some(theme::hsla(theme::TEXT)), font_weight: Some(gpui::FontWeight::SEMIBOLD), ..Default::default() })]),
                        ),
                )
                .child(action)
                .child(
                    div()
                        .id("cpp-dismiss")
                        .flex_none()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(15.))
                        .text_color(c(theme::TEXT_FAINT))
                        .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                        .child("×")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(b) = &mut this.cpp {
                                b.dismissed = true;
                            }
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (errors, warnings) = self.problem_counts();
        let state = self.state.read(cx);
        let selected = state.selected_text().chars().count();
        let (line, col) = self.cursor(cx);
        let item = || div().h(px(20.)).px(px(6.)).flex().items_center().gap(px(4.)).rounded(px(4.)).flex_none();
        let button = |id: &'static str| {
            item().id(id).cursor_pointer().text_size(px(13.)).hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
        };
        let exited = self.chip_text().ends_with("Restart");
        let missing = matches!(self.lsp_state, LspState::Missing(_));
        let ready = matches!(self.lsp_state, LspState::Ready(_)) && !exited;
        let progress = ready && self.chip_text().contains(" · ");
        div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .text_size(px(11.5))
            .text_color(c(theme::TEXT_DIM))
            .whitespace_nowrap()
            .overflow_hidden()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .when(self.doc.is_some() || !self.diagnostics.is_empty(), |d| {
                d.child(
                    button("status-problems")
                        .child(div().text_color(c(if errors > 0 { theme::DANGER } else { theme::TEXT_DIM })).child("⨯"))
                        .child(errors.to_string())
                        .child(div().ml(px(6.)).text_color(c(if errors + warnings > 0 { theme::WARN } else { theme::TEXT_DIM })).child("⚠"))
                        .child(warnings.to_string())
                        .on_click(cx.listener(|this, _, window, cx| this.jump_to_problem(true, window, cx))),
                )
            })
            .when(self.dirty, |d| d.child(item().text_color(c(theme::WARN)).child("Unsaved")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(6.))
                    .overflow_hidden()
                    .children(self.last_error.clone().map(|e| div().text_color(c(theme::DANGER)).child(e)))
                    .children(self.message.clone()),
            )
            .when(self.markdown_mode() != Some(markdown::Mode::Preview), |d| d.child(
                button("status-cursor")
                    .child(if selected > 0 { format!("Ln {line}, Col {col} ({selected} selected)") } else { format!("Ln {line}, Col {col}") })
                    .on_click(cx.listener(|this, _, window, cx| this.go_to_line(&GoToLine, window, cx))),
            ))
            .child(item().child(if self.crlf { "CRLF" } else { "LF" }))
            .child(item().child(self.indent_label()))
            .child(item().child(self.language.name))
            .when(!matches!(self.lsp_state, LspState::Off), |d| {
                let tip = match &self.lsp_state {
                    LspState::Missing(e) => e.clone(),
                    _ => "".into(),
                };
                d.child(
                    item()
                        .id("status-lsp")
                        .gap(px(6.))
                        .max_w(px(320.))
                        .rounded(px(11.))
                        .cursor_pointer()
                        .when(missing, |d| d.text_color(c(theme::TEXT_FAINT)).border_1().border_dashed().border_color(c(theme::BORDER_STRONG)))
                        .when(exited, |d| d.text_color(c(theme::DANGER)))
                        .when(ready && !progress, |d| d.child(div().size(px(6.)).flex_none().rounded_full().bg(c(theme::OK))))
                        .when(progress || matches!(self.lsp_state, LspState::Starting), |d| {
                            d.child(crate::icons::ui_icon("spinner", 10., c(theme::TEXT_DIM)))
                        })
                        .child(div().overflow_hidden().child(self.chip_text()))
                        .when(!tip.is_empty(), |d| d.tooltip(move |w, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(w, cx)))
                        .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                )
            })
    }

    fn render_signature(&self, cx: &App) -> Option<impl IntoElement> {
        let sig = self.signature.as_ref()?;
        let state = self.state.read(cx);
        let cursor = state.cursor();
        if state.text().offset_to_position(cursor).line != sig.line {
            return None;
        }
        let bounds = state.range_to_bounds(&(cursor..cursor))?;
        let (before, active, after) = match &sig.active {
            Some(r) => (&sig.label[..r.start], &sig.label[r.clone()], &sig.label[r.end..]),
            None => (sig.label.as_str(), "", ""),
        };
        Some(deferred(
            anchored()
                .position(point(bounds.left(), bounds.top()))
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    div()
                        .mb_1()
                        .px_2()
                        .py_1()
                        .max_w(px(640.))
                        .flex()
                        .flex_wrap()
                        .text_sm()
                        .font_family(theme::MONO_FONT)
                        .bg(c(theme::PANEL_RAISED))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded_md()
                        .shadow_md()
                        .text_color(c(theme::TEXT_DIM))
                        .child(before.to_string())
                        .child(div().text_color(c(theme::ACCENT)).font_weight(gpui::FontWeight::BOLD).child(active.to_string()))
                        .child(after.to_string()),
                ),
        ))
    }
}

/// Signature help as one label with the active parameter's byte range.
fn signature(help: &lsp_types::SignatureHelp, line: u32) -> Option<Signature> {
    let sig = help.signatures.get(help.active_signature.unwrap_or(0) as usize).or(help.signatures.first())?;
    let active_param = sig.active_parameter.or(help.active_parameter).unwrap_or(0) as usize;
    let active = sig.parameters.as_ref().and_then(|ps| ps.get(active_param)).and_then(|p| match &p.label {
        lsp_types::ParameterLabel::Simple(s) => sig.label.find(s.as_str()).map(|at| at..at + s.len()),
        lsp_types::ParameterLabel::LabelOffsets([a, b]) => {
            // Offsets count UTF-16 units; map them to byte positions.
            let byte = |n: u32| {
                let mut units = 0;
                sig.label.char_indices().find(|(_, ch)| {
                    let hit = units >= n;
                    units += ch.len_utf16() as u32;
                    hit
                }).map_or(sig.label.len(), |(i, _)| i)
            };
            Some(byte(*a)..byte(*b))
        }
    });
    Some(Signature { label: sig.label.clone(), active, line })
}

/// documentSymbol results (hierarchical or flat) as indented rows.
fn outline_items(value: &Value, path: &Path) -> Vec<PickItem> {
    fn walk(symbols: &[Value], depth: usize, path: &Path, out: &mut Vec<PickItem>) {
        for s in symbols {
            let range = s.get("selectionRange").or_else(|| s.pointer("/location/range")).and_then(|r| serde_json::from_value::<lsp_types::Range>(r.clone()).ok());
            let Some(name) = s.get("name").and_then(Value::as_str) else { continue };
            let kind = symbol_kind(s.get("kind").and_then(Value::as_u64));
            let detail = s.get("detail").or_else(|| s.get("containerName")).and_then(Value::as_str).filter(|d| !d.is_empty());
            out.push(PickItem {
                label: name.to_string().into(),
                detail: detail.map_or_else(|| kind.to_string(), |d| format!("{kind} · {d}")).into(),
                depth,
                target: range.map(|range| Target { path: path.to_path_buf(), range }),
            });
            if let Some(children) = s.get("children").and_then(Value::as_array) {
                walk(children, depth + 1, path, out);
            }
        }
    }
    let mut out = vec![];
    walk(value.as_array().map(Vec::as_slice).unwrap_or_default(), 0, path, &mut out);
    out
}

/// Locations as rows: `repo/path:line` and the line's text (read from the
/// open buffer for this file, from disk for others).
fn location_items(targets: Vec<Target>, own: &(PathBuf, Rope), repo: Option<&(RepoKey, PathBuf)>) -> Vec<PickItem> {
    let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
    targets
        .into_iter()
        .map(|t| {
            let line = t.range.start.line as usize;
            let text = if t.path == own.0 {
                own.1.slice_line(line).to_string()
            } else {
                let lines = files
                    .entry(t.path.clone())
                    .or_insert_with(|| std::fs::read_to_string(&t.path).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default());
                lines.get(line).cloned().unwrap_or_default()
            };
            let rel = match repo {
                Some((key, root)) => t.path.strip_prefix(root).map_or_else(|_| t.path.display().to_string(), |r| format!("{}/{}", key.repo, r.display())),
                None => t.path.display().to_string(),
            };
            PickItem {
                label: format!("{rel}:{}", line + 1).into(),
                detail: text.trim().chars().take(160).collect::<String>().into(),
                depth: 0,
                target: Some(t),
            }
        })
        .collect()
}

impl Render for EditorTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = self.picker.as_ref().map(|p| {
            p.render(
                &self.picker_focus,
                |this: &mut Self, ev, window, cx| this.on_picker_key(ev, window, cx),
                |this: &mut Self, row, window, cx| this.accept_picker(Some(row), window, cx),
                cx,
            )
        });
        let preview = self.render_markdown(cx);
        let previewing = preview.is_some();
        div()
            .key_context("OrbitEditor")
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::file_outline))
            .on_action(cx.listener(Self::workspace_symbols))
            .on_action(cx.listener(Self::find_references))
            .on_action(cx.listener(Self::go_to_implementation))
            .on_action(cx.listener(Self::go_to_type_definition))
            .on_action(cx.listener(Self::next_problem))
            .on_action(cx.listener(Self::previous_problem))
            .on_action(cx.listener(Self::format_document))
            .on_action(cx.listener(Self::rename_symbol))
            .on_action(cx.listener(Self::signature_help))
            .on_action(cx.listener(Self::restart_server))
            .on_action(cx.listener(Self::generate_compile_commands))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::show_code_actions))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(
                div()
                    .min_h(px(30.))
                    .flex_none()
                    .px(px(10.))
                    .py(px(3.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_FAINT))
                    .bg(c(theme::PANEL))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(self.render_breadcrumb(cx))
                    .child(
                        div()
                            .id("open-diff")
                            .cursor_pointer()
                            .px(px(6.))
                            .rounded(px(4.))
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                            .child("Diff")
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(EditorEvent::OpenDiff))),
                    )
                    .children(self.render_markdown_toggle(cx)),
            )
            .children(self.render_cpp_banner(cx))
            .children(preview)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .when(previewing, |d| d.hidden())
                    // Breakpoint strip clicks (after the editor moved the caret).
                    .on_mouse_down(MouseButton::Left, cx.listener(crate::debug::gutter::on_mouse_down))
                    .capture_any_mouse_down(cx.listener(Self::on_ctrl_click))
                    .child(Editor::new(&self.state).size_full().bordered(false))
                    .children(picker),
            )
            .children(self.render_signature(cx))
            .child(self.render_status(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_diagnostics_as_the_settings_say() {
        let d = |severity: Option<u64>, code: Value| -> lsp_types::Diagnostic {
            serde_json::from_value(json!({ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                "severity": severity, "code": code, "message": "m" }))
            .unwrap()
        };
        let errors_only = (true, vec![]);
        assert!(diagnostic_shown(&errors_only, &d(Some(1), Value::Null)));
        assert!(diagnostic_shown(&errors_only, &d(None, Value::Null)));
        assert!(!diagnostic_shown(&errors_only, &d(Some(2), Value::Null)));
        let hidden = (false, vec!["unused-includes".to_string(), "6133".to_string()]);
        assert!(!diagnostic_shown(&hidden, &d(Some(2), json!("unused-includes"))));
        assert!(!diagnostic_shown(&hidden, &d(Some(1), json!(6133))));
        assert!(diagnostic_shown(&hidden, &d(Some(2), json!("other"))));
    }

    #[test]
    fn detects_indentation() {
        assert_eq!(detect_indent("a\n  b\n    c\n  d\n").tab_size, 2);
        assert!(!detect_indent("a\n  b\n").hard_tabs);
        assert!(detect_indent("a\n\tb\n\t\tc\n").hard_tabs);
        assert_eq!(detect_indent("x").tab_size, 4);
    }

    #[test]
    fn signature_marks_the_active_parameter() {
        let help: lsp_types::SignatureHelp = serde_json::from_value(json!({
            "signatures": [{ "label": "void f(int a, float b)", "parameters": [{ "label": [7, 12] }, { "label": "float b" }] }],
            "activeSignature": 0, "activeParameter": 1,
        }))
        .unwrap();
        let s = signature(&help, 3).unwrap();
        assert_eq!(&s.label[s.active.clone().unwrap()], "float b");
        let help0 = lsp_types::SignatureHelp { active_parameter: Some(0), ..help };
        let s0 = signature(&help0, 3).unwrap();
        assert_eq!(&s0.label[s0.active.unwrap()], "int a");
    }

    #[test]
    fn outline_flattens_children() {
        let r = json!({ "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 4 } });
        let v = json!([{ "name": "Game", "kind": 3, "range": r, "selectionRange": r, "children": [
            { "name": "Initialize", "kind": 12, "detail": "void ()", "range": r, "selectionRange": r }
        ]}]);
        let items = outline_items(&v, Path::new("/x/Game.h"));
        assert_eq!(items.len(), 2);
        assert_eq!((items[1].label.as_ref(), items[1].depth, items[1].detail.as_ref()), ("Initialize", 1, "function · void ()"));
    }
}

#[cfg(test)]
mod highlight_tests {
    #[test]
    fn cpp_highlights_strings_and_numbers() {
        use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
        super::register_languages();
        let code = "#include \"a.h\"\nint main() { foo(\"x\"); return 42; }\n";
        let mut h = SyntaxHighlighter::new("cpp");
        h.update(None, &gpui_component::Rope::from(code), None);
        let theme = HighlightTheme::default_dark();
        let styles = h.styles(&(0..code.len()), &*theme);
        let colored = |text: &str| styles.iter().any(|(r, s)| &code[r.clone()] == text && s.color.is_some());
        for token in ["#include", "\"a.h\"", "\"x\"", "42", "return"] {
            assert!(colored(token), "{token} has no colour");
        }
    }
}
