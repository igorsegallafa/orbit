//! Search Everywhere (double Shift; `Ctrl+Shift+A` opens it on Actions) and
//! Find in Files (`Ctrl+Shift+F`). Both live in the palette overlay
//! (`palette.rs`); this module has their sources, commands and keys.
//! References: `SearchEverywhereModal.tsx`, `FindInFiles.tsx`.
pub mod fuzzy;

use crate::app::OrbitApp;
use crate::commands::{self, Command};
use crate::lsp::uri_to_path;
use gpui::{actions, App, KeyBinding, SharedString};
use orbit_core::search::SearchMatch;
use serde_json::{json, Value};
use std::ops::Range;
use std::path::{Path, PathBuf};

actions!(orbit_search, [NextTab, PrevTab, ToggleCase, ToggleWord, ToggleRegex, PageUp, PageDown, ToggleSearch]);

/// Key context of the search modal.
pub const CONTEXT: &str = "OrbitSearch";

/// Cmd+Shift+F on macOS (Ctrl+Shift+F still works there), Ctrl+Shift+F elsewhere.
const FIND_KEYS: &[&str] = if cfg!(target_os = "macos") { &["cmd-shift-f", "ctrl-shift-f"] } else { &["ctrl-shift-f"] };

pub fn init(cx: &mut App) {
    // `> Input` too: the query field binds Tab and PageUp/Down itself.
    for ctx in [CONTEXT, "OrbitSearch > Input"] {
        cx.bind_keys([
            KeyBinding::new("tab", NextTab, Some(ctx)),
            KeyBinding::new("shift-tab", PrevTab, Some(ctx)),
            KeyBinding::new("alt-c", ToggleCase, Some(ctx)),
            KeyBinding::new("alt-w", ToggleWord, Some(ctx)),
            KeyBinding::new("alt-x", ToggleRegex, Some(ctx)),
            KeyBinding::new("pageup", PageUp, Some(ctx)),
            KeyBinding::new("pagedown", PageDown, Some(ctx)),
            KeyBinding::new("shift shift", ToggleSearch, Some(ctx)),
        ]);
    }
    let has_ws = |a: &OrbitApp, _: &App| a.workspace().is_some();
    for command in [
        Command::new("search.everywhere", "Search Everywhere").keys(["shift shift"]).run(|a, w, cx| a.open_search(SeTab::All, w, cx)),
        Command::new("search.files", "Go to File…").when(has_ws).run(|a, w, cx| a.open_search(SeTab::Files, w, cx)),
        Command::new("search.symbols", "Go to Symbol…").when(has_ws).run(|a, w, cx| a.open_search(SeTab::Symbols, w, cx)),
        Command::new("search.text", "Search Text…").when(has_ws).run(|a, w, cx| a.open_search(SeTab::Text, w, cx)),
        Command::new("search.findInFiles", "Find in Files…").keys(FIND_KEYS.iter().copied()).when(has_ws).run(|a, w, cx| a.open_find(w, cx)),
    ] {
        commands::register(cx, command);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeTab {
    All,
    Files,
    Symbols,
    Actions,
    Text,
}

impl SeTab {
    pub const ALL: [SeTab; 5] = [SeTab::All, SeTab::Files, SeTab::Symbols, SeTab::Actions, SeTab::Text];

    pub fn label(self) -> &'static str {
        match self {
            SeTab::All => "All",
            SeTab::Files => "Files",
            SeTab::Symbols => "Symbols",
            SeTab::Actions => "Actions",
            SeTab::Text => "Text",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            SeTab::All => "Search files, symbols and actions…",
            SeTab::Files => "Search files…",
            SeTab::Symbols => "Search classes, functions, variables…",
            SeTab::Actions => "Search actions…",
            SeTab::Text => "Search text in files…",
        }
    }

    pub fn cycle(self, delta: isize) -> SeTab {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as isize;
        Self::ALL[(i + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Clone, Debug)]
pub struct SymbolHit {
    pub name: String,
    pub kind: &'static str,
    pub container: Option<String>,
    pub path: PathBuf,
    pub range: lsp_types::Range,
}

#[derive(Clone)]
pub enum Item {
    /// Repo-relative path.
    File { repo: String, path: String },
    Symbol(SymbolHit),
    Action(Command),
    /// A Text-tab match; `len` is the query's length in characters.
    Text { m: SearchMatch, len: usize },
    /// A Find in Files match with its hits on the line (byte ranges of `m.text`).
    Match { m: SearchMatch, hits: Vec<Range<usize>> },
}

#[derive(Clone)]
pub enum Row {
    /// A section (Search Everywhere) or a file (Find in Files). `tab`: clicking switches to it.
    Header { title: SharedString, detail: SharedString, tab: Option<SeTab> },
    Item(Item),
}

/// Find in Files options, kept between opens (the Tauri app keeps them in localStorage).
#[derive(Clone, Default)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub mask: String,
    /// None: every repo.
    pub repo: Option<String>,
}

impl gpui::Global for FindOptions {}

/// Parses a `workspace/symbol` answer; hits outside `root` are dropped, as the Tauri app does.
pub fn symbol_hits(value: &Value, root: &Path) -> Vec<SymbolHit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let uri: lsp_types::Uri = s.pointer("/location/uri")?.as_str()?.parse().ok()?;
            let path = uri_to_path(&uri)?;
            if !path.starts_with(root) {
                return None;
            }
            let range = s.pointer("/location/range").and_then(|r| serde_json::from_value(r.clone()).ok()).unwrap_or_default();
            Some(SymbolHit {
                name: s.get("name")?.as_str()?.to_string(),
                kind: crate::editor::symbol_kind(s.get("kind").and_then(Value::as_u64)),
                container: s.get("containerName").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string),
                path,
                range,
            })
        })
        .collect()
}

pub fn symbol_request(query: &str) -> Value {
    json!({ "query": query })
}

/// The hits on a matched line, as byte ranges (the backend's semantics:
/// `git grep -F`/`-P`, `-i`, `-w`). Falls back to the reported column.
pub fn hits(text: &str, query: &str, opts: &FindOptions, col: u64) -> Vec<Range<usize>> {
    let mut out = vec![];
    if !query.is_empty() {
        let src = if opts.regex { query.to_string() } else { regex::escape(query) };
        let src = if opts.whole_word { format!(r"\b(?:{src})\b") } else { src };
        if let Ok(re) = regex::RegexBuilder::new(&src).case_insensitive(!opts.case_sensitive).build() {
            out.extend(re.find_iter(text).filter(|m| !m.is_empty()).take(50).map(|m| m.range()));
        }
    }
    if out.is_empty() {
        let start = text.char_indices().nth(col.saturating_sub(1) as usize).map_or(text.len(), |(i, _)| i);
        let len = if opts.regex { 1 } else { query.chars().count() };
        let end = text[start..].char_indices().nth(len).map_or(text.len(), |(i, _)| start + i);
        out.push(start..end);
    }
    out
}

/// A matched line for display: leading indentation dropped, and a long
/// line cut to keep `context` characters before the first hit ("…" marks it).
/// Returns the text and the hits moved into it.
pub fn display_line(text: &str, hits: &[Range<usize>], context: usize) -> (String, Vec<Range<usize>>) {
    let indent = text.len() - text.trim_start().len();
    let first = hits.first().map_or(0, |h| h.start);
    let before = text[..first].chars().count();
    let mut lead = if before > context { text[..first].char_indices().nth(before - context).map_or(first, |(i, _)| i) } else { 0 };
    lead = lead.max(indent.min(first));
    let ellipsis = if lead > indent { "…" } else { "" };
    let shift = |i: usize| (i.max(lead) - lead) + ellipsis.len();
    let moved = hits.iter().filter(|h| h.end > lead).map(|h| shift(h.start)..shift(h.end)).collect();
    (format!("{ellipsis}{}", &text[lead..]), moved)
}

/// The 0-based editor position of a match hit (`col` of a hit range, in characters).
pub fn hit_position(m: &SearchMatch, hit: &Range<usize>) -> (lsp_types::Position, usize) {
    let col = m.text[..hit.start.min(m.text.len())].chars().count() as u32;
    let len = m.text.get(hit.clone()).map_or(0, |s| s.chars().count());
    (lsp_types::Position::new(m.line.saturating_sub(1) as u32, col), len)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn opts(case: bool, word: bool, regex: bool) -> FindOptions {
        FindOptions { case_sensitive: case, whole_word: word, regex, ..Default::default() }
    }

    #[test]
    fn finds_hits_with_the_options() {
        let line = "let foo = Foo(foobar);";
        assert_eq!(hits(line, "foo", &opts(false, false, false), 5), [4..7, 10..13, 14..17]);
        assert_eq!(hits(line, "foo", &opts(true, false, false), 5), [4..7, 14..17]);
        assert_eq!(hits(line, "foo", &opts(false, true, false), 5), [4..7, 10..13]);
        assert_eq!(hits(line, r"fo+b\w+", &opts(false, false, true), 15), [14..20]);
        // Literal queries escape regex characters.
        assert_eq!(hits("a.b axb", "a.b", &opts(false, false, false), 1), [0..3]);
        // A pattern Rust's regex can't parse falls back to the column.
        assert_eq!(hits("x(?<=y)z", "(?<=y)", &opts(false, false, true), 2), [1..2]);
    }

    #[test]
    fn display_drops_indent_and_cuts_long_lines() {
        let (text, h) = display_line("    let x = 1;", &[8..9], 40);
        assert_eq!(text, "let x = 1;");
        assert_eq!(&text[h[0].clone()], "x");
        let long = format!("{}needle", "a".repeat(100));
        let (text, h) = display_line(&long, &[100..106], 10);
        assert!(text.starts_with('…'));
        assert_eq!(&text[h[0].clone()], "needle");
    }

    #[test]
    fn hit_positions_count_characters() {
        let m = SearchMatch { repo: "r".into(), path: "a".into(), line: 3, col: 6, text: "ção = x".into() };
        let h = hits(&m.text, "= x", &opts(false, false, false), m.col);
        assert_eq!(hit_position(&m, &h[0]), (lsp_types::Position::new(2, 4), 3));
    }

    #[test]
    fn tabs_cycle() {
        assert_eq!(SeTab::All.cycle(-1), SeTab::Text);
        assert_eq!(SeTab::Text.cycle(1), SeTab::All);
    }
}
