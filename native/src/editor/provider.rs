//! The bridge between gpui-component's editor hooks and a language server:
//! the document as the server knows it, the providers (definition, hover,
//! completion, code actions) and conversions of LSP results.
use crate::lsp::{uri_to_path, LspServer};
use anyhow::Result;
use gpui::{App, AppContext as _, Entity, SharedString, Task, Window};
use gpui_component::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider, Rope, RopeExt,
};
use lsp_types::{CodeAction, CompletionContext, CompletionItem, CompletionResponse, Diagnostic, Range, TextEdit};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::ops::Range as ByteRange;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// Applies a `WorkspaceEdit` (as JSON) on behalf of the editor that owns the document.
pub type EditHook = Rc<dyn Fn(Value, &mut Window, &mut App)>;

/// The document as the language server knows it.
pub struct LspDoc {
    pub uri: lsp_types::Uri,
    pub language_id: &'static str,
    pub server: RefCell<Option<Arc<LspServer>>>,
    pub version: Cell<i32>,
    /// The text the server has, to send changes before any request.
    synced: RefCell<Rope>,
    /// The latest published diagnostics, sent back with code action requests.
    pub diagnostics: RefCell<Vec<Diagnostic>>,
    pub apply_edit: RefCell<Option<EditHook>>,
}

// The server forgets the document with its last editor (a closed tab, a renamed file).
impl Drop for LspDoc {
    fn drop(&mut self) {
        if self.version.get() > 0 {
            if let Some(server) = self.server() {
                server.notify("textDocument/didClose", json!({ "textDocument": { "uri": self.uri.as_str() } }));
            }
        }
    }
}

impl LspDoc {
    pub fn new(uri: lsp_types::Uri, language_id: &'static str) -> Self {
        Self {
            uri,
            language_id,
            server: RefCell::new(None),
            version: Cell::new(0),
            synced: RefCell::new(Rope::new()),
            diagnostics: RefCell::new(vec![]),
            apply_edit: RefCell::new(None),
        }
    }

    pub fn server(&self) -> Option<Arc<LspServer>> {
        self.server.borrow().clone().filter(|s| !s.has_exited())
    }

    /// didOpen with `text` to a newly attached server.
    pub fn open(&self, server: &LspServer, text: &Rope) {
        self.version.set(1);
        *self.synced.borrow_mut() = text.clone();
        server.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": self.uri.as_str(), "languageId": self.language_id, "version": 1, "text": text.to_string() }}),
        );
    }

    /// didChange (full text) if the server's copy is behind. Requests call it
    /// first: the editor asks for completions before it reports the edit.
    pub fn sync(&self, text: &Rope) {
        let Some(server) = self.server() else { return };
        if *self.synced.borrow() == *text {
            return;
        }
        *self.synced.borrow_mut() = text.clone();
        let version = self.version.get() + 1;
        self.version.set(version);
        server.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": self.uri.as_str(), "version": version }, "contentChanges": [{ "text": text.to_string() }] }),
        );
    }

    pub fn text_document(&self) -> Value {
        json!({ "uri": self.uri.as_str() })
    }

    pub fn position_params(&self, text: &Rope, offset: usize) -> Value {
        self.sync(text);
        let pos = text.offset_to_position(offset);
        json!({ "textDocument": self.text_document(), "position": { "line": pos.line, "character": pos.character } })
    }
}

pub struct DocProvider(pub Rc<LspDoc>);

impl DefinitionProvider for DocProvider {
    fn definitions(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Vec<lsp_types::LocationLink>>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(vec![])) };
        let request = server.request("textDocument/definition", self.0.position_params(text, offset));
        cx.background_spawn(async move { Ok(location_links(request.await?)) })
    }
}

impl HoverProvider for DocProvider {
    fn hover(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Option<lsp_types::Hover>>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(None)) };
        let request = server.request("textDocument/hover", self.0.position_params(text, offset));
        cx.background_spawn(async move { Ok(serde_json::from_value(request.await?).ok()) })
    }
}

impl CompletionProvider for DocProvider {
    fn completions(&self, text: &Rope, offset: usize, trigger: CompletionContext, _: &mut Window, cx: &mut App) -> Task<Result<CompletionResponse>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(CompletionResponse::Array(vec![]))) };
        // The component passes what was typed as the "trigger character";
        // only a real one goes to the server as such.
        let triggers = server.trigger_characters("completionProvider");
        let context = match trigger.trigger_character.as_deref().and_then(|t| triggers.iter().find(|c| t.ends_with(c.as_str()))) {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c }),
            None => json!({ "triggerKind": 1 }),
        };
        let mut params = self.0.position_params(text, offset);
        params["context"] = context;
        let prefix = word_before(text, offset);
        let request = server.request("textDocument/completion", params);
        cx.background_spawn(async move {
            let mut items = match serde_json::from_value::<Option<CompletionResponse>>(request.await?).ok().flatten() {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => vec![],
            };
            rank_completions(&mut items, &prefix);
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        let Some(server) = self.0.server() else { return false };
        new_text.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_')
            || server.trigger_characters("completionProvider").iter().any(|t| new_text.ends_with(t.as_str()))
    }
}

impl CodeActionProvider for DocProvider {
    fn id(&self) -> SharedString {
        "lsp".into()
    }

    fn code_actions(&self, state: Entity<EditorState>, range: ByteRange<usize>, _: &mut Window, cx: &mut App) -> Task<Result<Vec<CodeAction>>> {
        let Some(server) = self.0.server() else { return Task::ready(Ok(vec![])) };
        let text = state.read(cx).text();
        self.0.sync(text);
        let range = Range::new(text.offset_to_position(range.start), text.offset_to_position(range.end));
        // Diagnostics on the selected lines: their quick fixes come first.
        let diagnostics: Vec<Diagnostic> = self
            .0
            .diagnostics
            .borrow()
            .iter()
            .filter(|d| d.range.start.line <= range.end.line && d.range.end.line >= range.start.line)
            .cloned()
            .collect();
        let request = server.request(
            "textDocument/codeAction",
            json!({ "textDocument": self.0.text_document(), "range": range, "context": { "diagnostics": diagnostics, "triggerKind": 1 } }),
        );
        cx.background_spawn(async move { Ok(code_actions(request.await?)) })
    }

    fn perform_code_action(&self, _: Entity<EditorState>, action: CodeAction, _: bool, window: &mut Window, cx: &mut App) -> Task<Result<()>> {
        let hook = self.0.apply_edit.borrow().clone();
        // Runs inside the editor state's update: apply after it.
        if let (Some(edit), Some(hook)) = (action.edit.clone(), hook.clone()) {
            let edit = serde_json::to_value(edit).unwrap_or(Value::Null);
            window.defer(cx, move |window, cx| hook(edit, window, cx));
        }
        let (Some(command), Some(server)) = (action.command, self.0.server()) else { return Task::ready(Ok(())) };
        // A command (clangd's tweaks) answers with workspace/applyEdit requests.
        let request = server.request("workspace/executeCommand", json!({ "command": command.command, "arguments": command.arguments }));
        window.spawn(cx, async move |cx| {
            request.await?;
            let edits = server.take_applied_edits();
            if let Some(hook) = hook {
                cx.update(|window, cx| edits.into_iter().for_each(|e| hook(e, window, cx)))?;
            }
            Ok(())
        })
    }
}

/// The identifier being typed before `offset`.
pub fn word_before(text: &Rope, offset: usize) -> String {
    let start = offset - text.chars_at(offset).reversed().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum::<usize>();
    text.slice(start..offset).to_string()
}

/// Keeps the items matching `prefix`, best first, at most 100. Labels are
/// trimmed (clangd pads them for its own bullets).
pub fn rank_completions(items: &mut Vec<CompletionItem>, prefix: &str) {
    for item in items.iter_mut() {
        item.label = item.label.trim().to_string();
    }
    let mut scored: Vec<(i32, CompletionItem)> = std::mem::take(items)
        .into_iter()
        .filter_map(|item| {
            let key = item.filter_text.as_deref().unwrap_or(&item.label).trim();
            // Match quality in buckets (prefix, substring, subsequence); the server's order within each.
            let score = if prefix.is_empty() { 0 } else { fuzzy_score(prefix, key)? / 1000 };
            Some((score, item))
        })
        .collect();
    scored.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.sort_text.as_deref().unwrap_or(&x.label).cmp(y.sort_text.as_deref().unwrap_or(&y.label))));
    scored.truncate(100);
    *items = scored.into_iter().map(|(_, item)| item).collect();
}

/// How well `query` matches `text` (case-insensitive subsequence), higher is
/// better; None when it doesn't. Prefix and contiguous matches score highest.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if t.starts_with(&q) {
        return Some(3000 - t.len() as i32);
    }
    if let Some(at) = t.find(&q) {
        return Some(2000 - at as i32);
    }
    let mut chars = t.chars().enumerate();
    let mut gaps = 0;
    let mut last = 0;
    for qc in q.chars() {
        let (ix, _) = chars.find(|(_, c)| *c == qc)?;
        gaps += ix - last;
        last = ix;
    }
    Some(1000 - gaps as i32)
}

/// `Location | Location[] | LocationLink[] | null` as links.
pub fn location_links(value: Value) -> Vec<lsp_types::LocationLink> {
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

/// A place in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub range: Range,
}

/// Locations or links as file targets (non-file URIs dropped).
pub fn targets(value: Value) -> Vec<Target> {
    location_links(value)
        .into_iter()
        .filter_map(|l| Some(Target { path: uri_to_path(&l.target_uri)?, range: l.target_selection_range }))
        .collect()
}

/// `(Command | CodeAction)[]` as code actions (a bare command becomes an action that runs it).
pub fn code_actions(value: Value) -> Vec<CodeAction> {
    let items = serde_json::from_value::<Option<Vec<lsp_types::CodeActionOrCommand>>>(value).ok().flatten().unwrap_or_default();
    items
        .into_iter()
        .map(|item| match item {
            lsp_types::CodeActionOrCommand::CodeAction(a) => a,
            lsp_types::CodeActionOrCommand::Command(c) => CodeAction { title: c.title.clone(), command: Some(c), ..Default::default() },
        })
        .filter(|a| a.disabled.is_none())
        .collect()
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

/// A `WorkspaceEdit`'s text edits by file (`changes` or `documentChanges`;
/// file create/rename/delete operations are not supported and skipped).
pub fn workspace_edits(edit: &Value) -> Vec<(PathBuf, Vec<TextEdit>)> {
    let mut out: Vec<(PathBuf, Vec<TextEdit>)> = vec![];
    let mut add = |uri: &str, edits: &Value| {
        let (Ok(uri), Ok(edits)) = (uri.parse::<lsp_types::Uri>(), serde_json::from_value::<Vec<TextEdit>>(edits.clone())) else { return };
        let Some(path) = uri_to_path(&uri) else { return };
        match out.iter_mut().find(|(p, _)| *p == path) {
            Some((_, list)) => list.extend(edits),
            None => out.push((path, edits)),
        }
    };
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if let (Some(uri), Some(edits)) = (change.pointer("/textDocument/uri").and_then(Value::as_str), change.get("edits")) {
                add(uri, edits);
            }
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            add(uri, edits);
        }
    }
    out
}

/// Edits sorted last first, so applying them in order keeps earlier positions valid.
pub fn sort_for_apply(mut edits: Vec<TextEdit>) -> Vec<TextEdit> {
    edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
    edits
}

/// `text` with the edits applied (for files that aren't open).
pub fn apply_to_text(text: &str, edits: Vec<TextEdit>) -> String {
    let mut rope = Rope::from(text);
    for edit in sort_for_apply(edits) {
        let start = rope.position_to_offset(&edit.range.start);
        let end = rope.position_to_offset(&edit.range.end);
        rope.replace(start..end, &edit.new_text);
    }
    rope.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn item(label: &str, sort: &str) -> CompletionItem {
        CompletionItem { label: label.into(), sort_text: Some(sort.into()), ..Default::default() }
    }

    #[test]
    fn ranks_prefix_matches_first_and_drops_the_rest() {
        let mut items = vec![item(" GetInitialized()", "1"), item(" Initialize()", "2"), item(" Update()", "0"), item(" isInited", "3")];
        rank_completions(&mut items, "Init");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Initialize()", "GetInitialized()", "isInited"]);
    }

    #[test]
    fn empty_prefix_keeps_the_server_order() {
        let mut items = vec![item("b", "2"), item("a", "1")];
        rank_completions(&mut items, "");
        assert_eq!(items[0].label, "a");
    }

    #[test]
    fn fuzzy_scores() {
        assert!(fuzzy_score("gm", "GameManager").unwrap() < fuzzy_score("game", "GameManager").unwrap());
        assert!(fuzzy_score("mgr", "GameManager").is_some());
        assert!(fuzzy_score("xyz", "GameManager").is_none());
    }

    #[test]
    fn word_before_the_cursor() {
        let rope = Rope::from("foo.bar_Ba");
        assert_eq!(word_before(&rope, rope.len()), "bar_Ba");
        assert_eq!(word_before(&rope, 4), "");
    }

    #[test]
    fn workspace_edits_from_changes_and_document_changes() {
        let e = |l: u32| json!({ "range": { "start": { "line": l, "character": 0 }, "end": { "line": l, "character": 3 } }, "newText": "bar" });
        let changes = json!({ "changes": { "file:///a/x.cpp": [e(0), e(2)], "file:///a/y.h": [e(1)] } });
        let mut got = workspace_edits(&changes);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, PathBuf::from("/a/x.cpp"));
        assert_eq!(got[0].1.len(), 2);
        let doc = json!({ "documentChanges": [{ "textDocument": { "uri": "file:///a/x.cpp", "version": 3 }, "edits": [e(0)] }, { "kind": "create", "uri": "file:///a/new.h" }] });
        assert_eq!(workspace_edits(&doc).len(), 1);
    }

    #[test]
    fn applies_edits_last_first() {
        let edit = |line, from, to, text: &str| TextEdit::new(Range::new(Position::new(line, from), Position::new(line, to)), text.into());
        let text = "int foo = 1;\nfoo += foo;\n";
        let out = apply_to_text(text, vec![edit(0, 4, 7, "count"), edit(1, 0, 3, "count"), edit(1, 7, 10, "count")]);
        assert_eq!(out, "int count = 1;\ncount += count;\n");
    }

    #[test]
    fn code_actions_accept_commands() {
        let v = json!([{ "title": "Fix", "kind": "quickfix", "edit": { "changes": {} } }, { "title": "Tweak", "command": "clangd.applyTweak", "arguments": [] }]);
        let actions = code_actions(v);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[1].command.as_ref().unwrap().command, "clangd.applyTweak");
    }
}
