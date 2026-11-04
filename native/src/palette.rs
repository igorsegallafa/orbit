//! The search overlay: Search Everywhere (tabs All / Files / Symbols /
//! Actions / Text; `Ctrl+Shift+A` is its Actions tab) and Find in Files.
//! Sources and keys are in `search/`; looks follow `SearchEverywhereModal.tsx`
//! and `FindInFiles.tsx`.
use crate::motion;
use crate::app::OrbitApp;
use crate::commands;
use crate::icons;
use crate::probe::probe;
use crate::search::{self, fuzzy, FindOptions, Item, Row, SeTab, SymbolHit};
use crate::theme::{self, c};
use futures::future::join_all;
use gpui::{
    div, prelude::*, px, relative, svg, uniform_list, AnyElement, AnyWindowHandle, App, Context, Entity, Focusable,
    FontWeight, HighlightStyle, MouseButton, ScrollStrategy, SharedString, StyledText, Subscription, Task,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{
    Editor, EditorState, Input, InputEvent, InputState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
};
use gpui_component::{IconName, IconNamed};
use std::cell::RefCell;
use orbit_core::search::{SearchQuery, SearchResult};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Everywhere(SeTab),
    Find,
}

pub struct Palette {
    pub input: Entity<InputState>,
    pub mode: Mode,
    /// Section (or file) headers and items, as listed.
    pub rows: Vec<Row>,
    /// Index into `rows`; always an item when there is one.
    pub selected: usize,
    /// Why the list is empty, or Find's summary.
    pub status: Option<SharedString>,
    pub scroll: UniformListScrollHandle,
    /// The workspace searched: name and folder.
    ws: Option<(String, PathBuf)>,
    /// Every file of the workspace, listed once per open (None while listing).
    files: Option<Vec<(String, String)>>,
    /// Symbols for a query, and how many servers answered.
    symbols: Option<(String, Vec<SymbolHit>, usize)>,
    /// Text matches for a query (Text tab, Find in Files).
    text: Option<(String, Result<SearchResult, String>)>,
    searching: bool,
    /// Find in Files' file mask.
    pub mask: Option<Entity<InputState>>,
    /// Find in Files' preview of the selected match's file.
    preview: Option<Preview>,
    /// The match the preview is being loaded for (`repo/path:line`).
    preview_wanted: RefCell<Option<String>>,
    window: Option<AnyWindowHandle>,
    _subs: Vec<Subscription>,
    _files_task: Task<()>,
    _symbols_task: Task<()>,
    _text_task: Task<()>,
}

/// `.search-row` (34px as drawn) and `.find-row`.
const ROW_H: f32 = 34.;
const FIND_ROW_H: f32 = 24.;
const LIST_MAX_H: f32 = 380.;

/// A read-only view of a file, scrolled to a match (`FindPreview`).
pub struct Preview {
    /// `repo/path`.
    file: String,
    /// `repo/path:line`.
    key: String,
    state: Entity<EditorState>,
    /// The match line and its hits.
    decos: RangeDecorationCollection,
}
/// Results per section of the All tab.
const ALL_FILES: usize = 8;
const ALL_SYMBOLS: usize = 8;
const ALL_ACTIONS: usize = 5;

impl Palette {
    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    fn is_item(&self, ix: usize) -> bool {
        matches!(self.rows.get(ix), Some(Row::Item(_)))
    }

    /// Moves to the next item `delta` rows away (headers are skipped), clamped.
    pub fn move_selection(&mut self, delta: isize) {
        let items: Vec<usize> = (0..self.rows.len()).filter(|&i| self.is_item(i)).collect();
        let Some(at) = items.iter().position(|&i| i == self.selected).or((!items.is_empty()).then_some(0)) else { return };
        let to = (at as isize + delta).clamp(0, items.len() as isize - 1) as usize;
        self.selected = items[to];
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }

    fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.selected = (0..self.rows.len()).find(|&i| self.is_item(i)).unwrap_or(0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    pub fn selected_item(&self) -> Option<&Item> {
        match self.rows.get(self.selected)? {
            Row::Item(item) => Some(item),
            Row::Header { .. } => None,
        }
    }

    /// For the self-test dump.
    pub fn dump_text(&self, cx: &App) -> String {
        let mode = match self.mode {
            Mode::Everywhere(tab) => format!("search everywhere [{}]", tab.label()),
            Mode::Find => {
                let o = cx.try_global::<FindOptions>().cloned().unwrap_or_default();
                let mask = self.mask.as_ref().map(|m| m.read(cx).value().to_string()).unwrap_or_default();
                format!("find in files [case {} word {} regex {} mask {mask:?} repo {:?}]", o.case_sensitive, o.whole_word, o.regex, o.repo)
            }
        };
        let mut out = format!(
            "--- palette: {mode} query {:?}, {} rows, selected {}, status {:?}\n",
            self.input.read(cx).value(),
            self.rows.len(),
            self.selected,
            self.status
        );
        for (ix, row) in self.rows.iter().enumerate().take(14) {
            let mark = if ix == self.selected { ">" } else { " " };
            let text = match row {
                Row::Header { title, detail, .. } => format!("## {title} {detail}"),
                Row::Item(item) => item_text(item),
            };
            out.push_str(&format!("  {mark} {text}\n"));
        }
        out
    }
}

fn item_text(item: &Item) -> String {
    match item {
        Item::File { repo, path } => format!("file {repo}/{path}"),
        Item::Symbol(h) => format!(
            "symbol {} {} {}:{}",
            h.kind,
            h.name,
            h.path.file_name().unwrap_or_default().to_string_lossy(),
            h.range.start.line + 1
        ),
        Item::Action(cmd) => format!("action {} [{}]", cmd.title, cmd.shortcut().unwrap_or_default()),
        Item::Text { m, .. } => format!("text {}:{} {}", m.path, m.line, m.text.trim()),
        Item::Match { m, hits } => {
            let shown: Vec<&str> = hits.iter().filter_map(|h| m.text.get(h.clone())).collect();
            format!("match {}/{}:{}:{} {shown:?}", m.repo, m.path, m.line, m.col)
        }
    }
}

impl OrbitApp {
    /// Find Action (`Ctrl+Shift+A`): Search Everywhere on its Actions tab.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(SeTab::Actions, window, cx);
    }

    fn new_palette(&mut self, mode: Mode, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Palette {
        self.menu = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_query_changed(cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        window.prevent_default();
        Palette {
            input,
            mode,
            rows: vec![],
            selected: 0,
            status: None,
            scroll: UniformListScrollHandle::new(),
            ws: self.workspace().map(|w| (w.meta.name.clone(), w.dir.clone())),
            files: None,
            symbols: None,
            text: None,
            searching: false,
            mask: None,
            preview: None,
            preview_wanted: RefCell::new(None),
            window: Some(window.window_handle()),
            _subs: vec![sub],
            _files_task: Task::ready(()),
            _symbols_task: Task::ready(()),
            _text_task: Task::ready(()),
        }
    }

    pub fn open_search(&mut self, tab: SeTab, window: &mut Window, cx: &mut Context<Self>) {
        let mut p = self.new_palette(Mode::Everywhere(tab), tab.placeholder(), window, cx);
        // The workspace's files, once per open, listed in the background.
        if let Some((name, _)) = p.ws.clone() {
            let list = cx.background_spawn(async move { orbit_core::files::list_workspace_files(&name) });
            p._files_task = cx.spawn(async move |this, cx| {
                let files = list.await.unwrap_or_default().into_iter().map(|f| (f.repo, f.path)).collect();
                let _ = this.update(cx, |this, cx| {
                    if let Some(p) = this.palette.as_mut() {
                        p.files = Some(files);
                        this.refresh_everywhere(cx);
                    }
                });
            });
        } else {
            p.files = Some(vec![]);
        }
        self.palette = Some(p);
        self.on_query_changed(cx);
    }

    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspace().map(|w| w.meta.name.clone()) else { return };
        let mut p = self.new_palette(Mode::Find, &format!("Search in {name}"), window, cx);
        let opts = cx.default_global::<FindOptions>().clone();
        let mask = cx.new(|cx| InputState::new(window, cx).placeholder("File mask, e.g. *.ts, !*.test.ts").default_value(opts.mask));
        p._subs.push(cx.subscribe_in(&mask, window, |this, mask, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.global_mut::<FindOptions>().mask = mask.read(cx).value().to_string();
                this.schedule_text(0, cx);
            }
        }));
        p.mask = Some(mask);
        self.palette = Some(p);
        self.on_query_changed(cx);
    }

    fn on_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.palette else { return };
        match p.mode {
            Mode::Everywhere(tab) => {
                self.refresh_everywhere(cx);
                if matches!(tab, SeTab::All | SeTab::Symbols) {
                    self.schedule_symbols(cx);
                }
                if tab == SeTab::Text {
                    self.schedule_text(250, cx);
                }
            }
            Mode::Find => self.schedule_text(220, cx),
        }
    }

    fn set_search_tab(&mut self, tab: SeTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else { return };
        if !matches!(p.mode, Mode::Everywhere(_)) {
            return;
        }
        p.mode = Mode::Everywhere(tab);
        p.input.update(cx, |i, cx| i.set_placeholder(tab.placeholder(), window, cx));
        self.on_query_changed(cx);
    }

    /// Rebuilds Search Everywhere's rows from what is known now.
    fn refresh_everywhere(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.palette else { return };
        let Mode::Everywhere(tab) = p.mode else { return };
        let q = p.query(cx);
        let lower = q.to_lowercase();
        let all = tab == SeTab::All;
        let mut sections: Vec<(SeTab, Vec<Item>)> = vec![];
        if all || tab == SeTab::Files {
            let files = p.files.as_deref().unwrap_or_default();
            let ranked = fuzzy::rank_paths(files, |(repo, path)| format!("{repo}/{path}"), &q, if all { ALL_FILES } else { 60 });
            sections.push((SeTab::Files, ranked.into_iter().map(|(repo, path)| Item::File { repo, path }).collect()));
        }
        if let Some((_, hits, _)) = p.symbols.as_ref().filter(|(sq, _, _)| (all || tab == SeTab::Symbols) && q.chars().count() >= 2 && *sq == q) {
            let n = if all { ALL_SYMBOLS } else { 100 };
            sections.push((SeTab::Symbols, hits.iter().take(n).cloned().map(Item::Symbol).collect()));
        }
        if all || tab == SeTab::Actions {
            let mut cmds: Vec<(u32, commands::Command)> = commands::all(cx)
                .into_iter()
                .filter(|c| c.run.is_some() && c.enabled(self, cx))
                .map(|c| (if lower.is_empty() { 1 } else { fuzzy::fuzzy_score(&c.title.to_lowercase(), &lower) }, c))
                .filter(|(s, _)| *s > 0)
                .collect();
            cmds.sort_by_key(|a| std::cmp::Reverse(a.0));
            let n = if all { if lower.is_empty() { 0 } else { ALL_ACTIONS } } else { 100 };
            sections.push((SeTab::Actions, cmds.into_iter().take(n).map(|(_, c)| Item::Action(c)).collect()));
        }
        if let Some((_, Ok(r))) = p.text.as_ref().filter(|(tq, _)| tab == SeTab::Text && q.chars().count() >= 2 && *tq == q) {
            let len = q.chars().count();
            sections.push((SeTab::Text, r.matches.iter().take(200).cloned().map(|m| Item::Text { m, len }).collect()));
        }
        let mut rows = vec![];
        for (section, items) in sections.into_iter().filter(|(_, items)| !items.is_empty()) {
            if all {
                rows.push(Row::Header { title: section.label().into(), detail: "more ›".into(), tab: Some(section) });
            }
            rows.extend(items.into_iter().map(Row::Item));
        }
        let status = if !rows.is_empty() {
            match &p.text {
                Some((_, Ok(r))) if tab == SeTab::Text && r.truncated => Some("Showing the first matches: refine the search"),
                _ => None,
            }
            .map(String::from)
        } else {
            Some(match tab {
                SeTab::All | SeTab::Files if p.files.is_none() => "Indexing files…".into(),
                SeTab::Files | SeTab::Symbols | SeTab::Text if p.ws.is_none() => "No workspace selected".into(),
                SeTab::Symbols | SeTab::Text if q.chars().count() < 2 => "Type at least 2 characters".into(),
                SeTab::Symbols if p.symbols.as_ref().is_some_and(|s| s.2 == 0) => {
                    "No language server running in this workspace: open a source file to start one".into()
                }
                SeTab::Text if p.searching => "Searching…".into(),
                SeTab::Text if matches!(&p.text, Some((_, Err(_)))) => match &p.text {
                    Some((_, Err(e))) => e.clone(),
                    _ => unreachable!(),
                },
                _ if !q.is_empty() => format!("Nothing matches “{q}”"),
                _ => "Nothing to show".into(),
            })
        };
        if let Some(p) = self.palette.as_mut() {
            p.set_rows(rows);
            p.status = status.map(Into::into);
        }
        cx.notify();
    }

    /// `workspace/symbol` on every running server of the workspace, after a pause in typing.
    fn schedule_symbols(&mut self, cx: &mut Context<Self>) {
        let servers = self.lsp.running();
        let Some(p) = self.palette.as_mut() else { return };
        let q = p.query(cx);
        let Some((_, root)) = p.ws.clone() else { return };
        if q.chars().count() < 2 {
            p._symbols_task = Task::ready(());
            return;
        }
        let servers: Vec<_> = servers.into_iter().filter(|s| s.open_root.starts_with(&root) && !s.has_exited()).collect();
        p._symbols_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(180)).await;
            let n = servers.len();
            let answers = join_all(servers.iter().map(|s| s.request("workspace/symbol", search::symbol_request(&q)))).await;
            let hits: Vec<SymbolHit> = answers.into_iter().flatten().flat_map(|v| search::symbol_hits(&v, &root)).collect();
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.palette.as_mut() {
                    p.symbols = Some((q, hits, n));
                    this.refresh_everywhere(cx);
                }
            });
        });
    }

    /// orbit-core's search (git grep) after `delay_ms` without typing.
    fn schedule_text(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        let opts = cx.default_global::<FindOptions>().clone();
        let Some(p) = self.palette.as_mut() else { return };
        let q = if p.mode == Mode::Find { p.input.read(cx).value().to_string() } else { p.query(cx) };
        let Some((ws, _)) = p.ws.clone() else { return };
        let find = p.mode == Mode::Find;
        if q.is_empty() || (!find && q.chars().count() < 2) {
            p._text_task = Task::ready(());
            p.searching = false;
            p.text = None;
            if find {
                self.refresh_find(cx);
            }
            return;
        }
        p.searching = true;
        let query = if find {
            SearchQuery {
                query: q.clone(),
                case_sensitive: opts.case_sensitive,
                whole_word: opts.whole_word,
                regex: opts.regex,
                mask: opts.mask.clone(),
                repos: opts.repo.iter().cloned().collect(),
            }
        } else {
            SearchQuery { query: q.clone(), ..Default::default() }
        };
        if find {
            self.refresh_find(cx);
        } else {
            self.refresh_everywhere(cx);
        }
        let Some(p) = self.palette.as_mut() else { return };
        p._text_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
            let result = cx.background_spawn(async move { orbit_core::search::search(&ws, &query) }).await;
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.palette.as_mut() else { return };
                p.searching = false;
                p.text = Some((q, result));
                if find {
                    this.refresh_find(cx);
                } else {
                    this.refresh_everywhere(cx);
                }
            });
        });
    }

    /// Find in Files' rows: matches grouped under their file.
    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let opts = cx.default_global::<FindOptions>().clone();
        let Some(p) = self.palette.as_mut() else { return };
        let q = p.input.read(cx).value().to_string();
        let mut rows = vec![];
        let mut files = 0;
        let status = match &p.text {
            _ if q.is_empty() => None,
            Some((_, Err(e))) => Some(e.clone()),
            None => Some("Searching…".into()),
            Some((_, Ok(r))) => {
                let mut last: Option<(&str, &str)> = None;
                for m in &r.matches {
                    if last != Some((&m.repo, &m.path)) {
                        last = Some((&m.repo, &m.path));
                        files += 1;
                    }
                    rows.push(Row::Item(Item::Match { hits: search::hits(&m.text, &q, &opts, m.col), m: m.clone() }));
                }
                let n = r.matches.len();
                Some(match n {
                    0 => "No matches".into(),
                    _ => format!(
                        "{}{} match{} in {files} file{}",
                        n,
                        if r.truncated { "+" } else { "" },
                        if n == 1 { "" } else { "es" },
                        if files == 1 { "" } else { "s" }
                    ),
                })
            }
        };
        let searching = p.searching;
        p.set_rows(rows);
        p.status = status.map(|s| if searching && !q.is_empty() && p.text.is_some() { format!("{s} · searching…") } else { s }.into());
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.restore_focus(window, cx);
            cx.notify();
        }
    }

    pub fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.palette.as_ref().map(|p| p.selected) {
            self.pick_palette_row(ix, window, cx);
        }
    }

    fn pick_palette_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_ref() else { return };
        let root = p.ws.as_ref().map(|(_, dir)| dir.clone());
        let item = match p.rows.get(ix) {
            Some(Row::Item(item)) => item.clone(),
            Some(Row::Header { tab: Some(tab), .. }) => return self.set_search_tab(*tab, window, cx),
            _ => return,
        };
        // Closed first: an action goes to the view that had the focus.
        self.close_palette(window, cx);
        let in_ws = |repo: &str, path: &str| root.as_ref().map(|r| r.join(repo).join(path));
        match item {
            Item::Action(cmd) => {
                commands::run(&cmd.id, self, window, cx);
            }
            Item::File { repo, path } => {
                if let Some(path) = in_ws(&repo, &path) {
                    self.open_file(path, false, window, cx);
                }
            }
            Item::Symbol(h) => self.open_at(h.path, h.range.start, 0, window, cx),
            Item::Text { m, len } => {
                if let Some(path) = in_ws(&m.repo, &m.path) {
                    let start = lsp_types::Position::new(m.line.saturating_sub(1) as u32, m.col.saturating_sub(1) as u32);
                    self.open_at(path, start, len, window, cx);
                }
            }
            Item::Match { m, hits } => {
                if let (Some(path), Some(hit)) = (in_ws(&m.repo, &m.path), hits.first()) {
                    let (start, len) = search::hit_position(&m, hit);
                    self.open_at(path, start, len, window, cx);
                }
            }
        }
    }

    /// Opens a file with `len` characters selected from `start` (0-based).
    pub fn open_at(&mut self, path: PathBuf, start: lsp_types::Position, len: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.open_file(path, false, window, cx) else { return };
        editor.update(cx, |e, cx| {
            e.reveal(lsp_types::Range::new(start, start), window, cx);
            if len > 0 {
                e.state.update(cx, |s, cx| {
                    use gpui_component::input::RopeExt;
                    let text = s.text();
                    let from = text.position_to_offset(&start);
                    let to = text.position_to_offset(&lsp_types::Position::new(start.line, start.character + len as u32));
                    s.set_selected_range(from..to, cx);
                    center_line(s, start.line, cx);
                });
            }
        });
    }

    fn on_search_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        match self.palette.as_ref().map(|p| p.mode) {
            Some(Mode::Everywhere(tab)) => self.set_search_tab(tab.cycle(delta), window, cx),
            // Find: Tab moves between the query and the mask.
            Some(Mode::Find) => {
                let Some(p) = &self.palette else { return };
                let target = if p.input.focus_handle(cx).is_focused(window) { p.mask.clone() } else { None };
                match target {
                    Some(mask) => window.focus(&mask.focus_handle(cx), cx),
                    None => window.focus(&p.input.focus_handle(cx), cx),
                }
            }
            None => {}
        }
    }

    fn toggle_find_option(&mut self, f: fn(&mut FindOptions), cx: &mut Context<Self>) {
        if self.palette.as_ref().is_some_and(|p| p.mode == Mode::Find) {
            f(cx.default_global::<FindOptions>());
            self.schedule_text(0, cx);
        }
    }

    fn set_find_repo(&mut self, repo: Option<String>, cx: &mut Context<Self>) {
        cx.default_global::<FindOptions>().repo = repo;
        self.schedule_text(0, cx);
    }
}

impl OrbitApp {
    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.palette.as_ref()?;
        let find = p.mode == Mode::Find;
        if find {
            self.request_preview(p, cx);
        }
        let list = uniform_list(
            "search-list",
            p.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(p) = this.palette.as_ref() else { return vec![] };
                range.filter_map(|ix| p.rows.get(ix).map(|row| render_row(row, ix, ix == p.selected, find, cx))).collect()
            }),
        )
        .track_scroll(&p.scroll);
        let popup = div()
            .id("palette")
            .key_context(search::CONTEXT)
            .on_action(cx.listener(|this, _: &search::NextTab, w, cx| this.on_search_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &search::PrevTab, w, cx| this.on_search_tab(-1, w, cx)))
            .on_action(cx.listener(|this, _: &search::ToggleCase, _, cx| this.toggle_find_option(|o| o.case_sensitive ^= true, cx)))
            .on_action(cx.listener(|this, _: &search::ToggleWord, _, cx| this.toggle_find_option(|o| o.whole_word ^= true, cx)))
            .on_action(cx.listener(|this, _: &search::ToggleRegex, _, cx| this.toggle_find_option(|o| o.regex ^= true, cx)))
            .on_action(cx.listener(|this, _: &search::PageUp, _, cx| {
                if let Some(p) = this.palette.as_mut() {
                    p.move_selection(-10);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &search::PageDown, _, cx| {
                if let Some(p) = this.palette.as_mut() {
                    p.move_selection(10);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &search::ToggleSearch, w, cx| this.close_palette(w, cx)))
            .occlude()
            .w(px(if find { 760. } else { 640. }))
            .max_w(relative(0.9))
            .bg(c(theme::PANEL))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .rounded(px(12.))
            .shadow(theme::shadow(if find { 24. } else { 20. }, 60., if find { 0.55 } else { 0.6 }))
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let scrim = div()
            .id("palette-scrim")
            .key_context(crate::overlay::CONTEXT)
            .absolute()
            .inset_0()
            .bg(theme::scrim())
            .flex()
            .flex_col()
            .items_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)));
        Some(if find {
            let idle = p.input.read(cx).value().is_empty();
            // `.find-overlay`: the top of a centered full-size popup, so it only grows downward.
            scrim
                .child(div().flex_1())
                .child(motion::popover_in(
                    div()
                        .w_full()
                        .h(relative(0.7))
                        .max_h(px(600.))
                        .flex()
                        .justify_center()
                        .items_start()
                        .child(self.render_find(p, popup.when(!idle, |d| d.h_full()), list, idle, cx)),
                    "search-modal-in",
                    motion::SEARCH_IN,
                ))
                .child(div().flex_1())
        } else {
            let body = div().w_full().flex().flex_col().items_center().child(self.render_everywhere(p, popup, list, cx));
            scrim.justify_center().child(motion::popover_in(body, "search-modal-in", motion::SEARCH_IN))
        })
        .map(|s| motion::overlay_in(s, "palette-scrim-in"))
    }

    fn render_everywhere(
        &self,
        p: &Palette,
        popup: gpui::Stateful<gpui::Div>,
        list: gpui::UniformList,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Mode::Everywhere(tab) = p.mode else { return popup };
        // `.search-list`: max-height 380 with 6px padding.
        let height = (p.rows.len() as f32 * ROW_H).min(LIST_MAX_H - 12.);
        let body = div()
            .p(px(6.))
            .when(p.rows.is_empty(), |d| {
                d.child(
                    div()
                        .p(px(40.))
                        .flex()
                        .justify_center()
                        .text_size(px(13.))
                        .text_color(c(theme::TEXT_FAINT))
                        .gap(px(8.))
                        .when(matches!(p.status.as_deref(), Some("Indexing files…" | "Searching…")), |d| {
                            d.child(crate::workspace_ui::kit::spinner("se-spin", 14., c(theme::ACCENT)))
                        })
                        .child(p.status.clone().unwrap_or_else(|| "Nothing to show".into())),
                )
            })
            .when(!p.rows.is_empty(), |d| d.child(list.h(px(height))))
            .when(!p.rows.is_empty() && p.status.is_some(), |d| {
                d.child(div().px(px(10.)).py(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).children(p.status.clone()))
            });
        popup.child(self.render_search_tabs(tab, cx)).child(render_search_input(p)).child(body)
    }

    fn render_search_tabs(&self, active: SeTab, cx: &mut Context<Self>) -> impl IntoElement {
        // `.search-tabs` / `button.search-tab`.
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(10.))
            .pt(px(8.))
            .children(SeTab::ALL.into_iter().map(|tab| {
                let on = tab == active;
                div()
                    .id(("search-tab", tab as usize))
                    .relative()
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme::accent_soft()).text_color(c(theme::TEXT)))
                    .when(!on, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT))))
                    .child(tab.label())
                    .child(probe(format!("search-tab:{}", tab.label())))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| this.set_search_tab(tab, window, cx)))
            }))
            .child(div().flex_1())
            .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child("Tab to switch"))
    }

    fn render_find(
        &self,
        p: &Palette,
        popup: gpui::Stateful<gpui::Div>,
        list: gpui::UniformList,
        idle: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let opts = cx.try_global::<FindOptions>().cloned().unwrap_or_default();
        let repos = self.workspace().map(|w| w.meta.repos.clone()).unwrap_or_default();
        let ws = p.ws.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();
        // `.find-toggle`.
        let toggle = |id: &'static str, label: &'static str, on: bool, f: fn(&mut FindOptions)| {
            div()
                .id(id)
                .relative()
                .h(px(22.))
                .min_w(px(24.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(theme::MONO_FONT)
                .cursor_pointer()
                .when(on, |d| d.bg(theme::accent_soft()).text_color(c(theme::ACCENT)).border_color(theme::hsla(theme::ACCENT).opacity(0.4)))
                .when(!on, |d| {
                    d.border_color(gpui::transparent_black())
                        .text_color(c(theme::TEXT_FAINT))
                        .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                })
                .child(label)
                .child(probe(id))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_find_option(f, cx)))
        };
        // `.find-chip`.
        let chip = |label: String, repo: Option<String>, on: bool| {
            div()
                .id(SharedString::from(format!("find-repo:{label}")))
                .relative()
                .h(px(24.))
                .px(px(9.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(theme::accent_soft()).text_color(c(theme::TEXT)).border_color(theme::hsla(theme::ACCENT).opacity(0.45)))
                .when(!on, |d| {
                    d.border_color(c(theme::BORDER)).text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                })
                .child(label.clone())
                .child(probe(format!("find-repo:{label}")))
                .on_click(cx.listener(move |this, _, _, cx| this.set_find_repo(repo.clone(), cx)))
        };
        let mut chips = vec![];
        if repos.len() > 1 {
            chips.push(chip("All repos".into(), None, opts.repo.is_none()));
            for r in repos {
                let on = opts.repo.as_deref() == Some(r.as_str());
                chips.push(chip(r.clone(), Some(r), on));
            }
        }
        let has_matches = p.rows.iter().any(|r| matches!(r, Row::Item(_)));
        let empty = |text: SharedString| {
            div().p(px(14.)).flex().justify_center().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(text)
        };
        let results = div().flex().flex_col().border_t_1().border_color(c(theme::BORDER)).py(px(4.)).map(|d| {
            if idle {
                d.child(empty(format!("Type to search every repo of {ws}. Enter opens the match.").into()))
            } else if !has_matches {
                let text = match &p.status {
                    Some(s) if s.as_ref() == "No matches" => format!("No matches in {}", opts.repo.clone().unwrap_or_else(|| "the workspace".into())).into(),
                    Some(s) => s.clone(),
                    None => "Searching…".into(),
                };
                let spin = p.searching.then(|| crate::workspace_ui::kit::spinner("find-empty-spin", 14., c(theme::ACCENT)));
                d.flex_none().h(relative(0.4)).justify_center().child(div().flex().items_center().justify_center().children(spin).child(empty(text)))
            } else {
                d.flex_none().h(relative(0.4)).child(list.size_full())
            }
        });
        let current = match p.selected_item() {
            Some(Item::Match { m, .. }) => Some(format!("{}/{}:{}", m.repo, m.path, m.line)),
            _ => None,
        };
        let summary = if idle || !has_matches { None } else { p.status.clone() };
        popup
            .child(
                // `.find-head`.
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(14.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("Find in Files"))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.5))
                            .text_color(c(theme::TEXT_FAINT))
                            .when(p.searching && summary.is_some(), |d| d.child(crate::workspace_ui::kit::spinner("find-spin", 11., c(theme::ACCENT))))
                            .children(summary.map(|s| div().min_w_0().truncate().child(s))),
                    ),
            )
            .child(
                // `.find-inputrow`.
                div()
                    .mx(px(14.))
                    .h(px(36.))
                    .pl(px(10.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c(theme::BORDER_STRONG))
                    .bg(c(theme::BG))
                    .child(lucide(IconName::Search, 14., c(theme::TEXT_FAINT)))
                    .child(div().flex_1().min_w_0().child(Input::new(&p.input).appearance(false).text_size(px(13.))))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(px(2.))
                            .child(toggle("find-toggle:case", "Aa", opts.case_sensitive, |o| o.case_sensitive ^= true))
                            .child(toggle("find-toggle:word", "W", opts.whole_word, |o| o.whole_word ^= true))
                            .child(toggle("find-toggle:regex", ".*", opts.regex, |o| o.regex ^= true)),
                    ),
            )
            .child(
                // `.find-scope`.
                div()
                    .mx(px(14.))
                    .my(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(div().flex().flex_wrap().gap(px(4.)).children(chips))
                    .children(p.mask.as_ref().map(|m| {
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(c(theme::BORDER_STRONG))
                            .bg(c(theme::BG))
                            .font_family(theme::MONO_FONT)
                            .child(Input::new(m).appearance(false).text_size(px(12.)))
                    })),
            )
            .child(results)
            .map(|d| match p.preview.as_ref().filter(|_| has_matches) {
                Some(pv) => d.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(c(theme::BORDER))
                        .bg(c(theme::BG))
                        .text_size(px(12.))
                        .child(Editor::new(&pv.state).size_full().bordered(false).readonly(true)),
                ),
                None if !idle => d.child(div().flex_1().border_t_1().border_color(c(theme::BORDER)).bg(c(theme::BG))),
                None => d,
            })
            .child(
                // `.find-foot`.
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .text_size(px(11.))
                    .text_color(c(theme::TEXT_FAINT))
                    .child("↑↓ navigate · Enter open · Alt+C / W / X options")
                    .children(current.map(|t| div().min_w_0().truncate().font_family(theme::MONO_FONT).child(t))),
            )
    }

    /// Loads the selected match's file into the preview, off the render.
    fn request_preview(&self, p: &Palette, cx: &mut Context<Self>) {
        let (Some(Item::Match { m, .. }), Some((_, root)), Some(window)) = (p.selected_item(), p.ws.as_ref(), p.window) else { return };
        let key = format!("{}/{}:{}", m.repo, m.path, m.line);
        if p.preview.as_ref().is_some_and(|pv| pv.key == key) || p.preview_wanted.borrow().as_deref() == Some(key.as_str()) {
            return;
        }
        *p.preview_wanted.borrow_mut() = Some(key.clone());
        let file = format!("{}/{}", m.repo, m.path);
        let path = root.join(&m.repo).join(&m.path);
        let reuse = p.preview.as_ref().filter(|pv| pv.file == file).map(|pv| pv.state.clone());
        let line = m.line.saturating_sub(1) as u32;
        // Every hit in this file: (start, length in chars, the selected match's).
        let hits: Vec<(lsp_types::Position, usize, bool)> = p
            .rows
            .iter()
            .enumerate()
            .filter_map(|(ix, r)| match r {
                Row::Item(Item::Match { m: o, hits }) if o.repo == m.repo && o.path == m.path => Some((ix == p.selected, o, hits)),
                _ => None,
            })
            .flat_map(|(on, o, hits)| hits.iter().map(move |h| {
                let (pos, len) = search::hit_position(o, h);
                (pos, len, on)
            }))
            .collect();
        cx.spawn(async move |this, cx| {
            let language = crate::lsp::file_language(&path).highlight;
            let text = match reuse {
                Some(_) => None,
                None => Some(cx.background_spawn(async move { std::fs::read_to_string(&path) }).await),
            };
            let _ = cx.update_window(window, |_, window, cx| {
                let state = match (reuse, text) {
                    (Some(s), _) => s,
                    (None, Some(Ok(t))) => cx.new(|cx| EditorState::new(window, cx).language(language).line_number(true).default_value(t)),
                    _ => return,
                };
                let decos = state.update(cx, |s, cx| {
                    use gpui_component::input::RopeExt;
                    let text = s.text();
                    let at = |pos: lsp_types::Position| text.position_to_offset(&pos);
                    let line_start = at(lsp_types::Position::new(line, 0));
                    let line_end = at(lsp_types::Position::new(line + 1, 0));
                    let mut decos = vec![RangeDecoration::new(line_start..line_end)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(theme::hsla(theme::ACCENT).opacity(0.1))];
                    for (pos, len, on) in &hits {
                        let from = at(*pos);
                        let to = at(lsp_types::Position::new(pos.line, pos.character + *len as u32));
                        decos.push(
                            RangeDecoration::new(from..to)
                                .with_style(RangeDecorationStyle::Fill)
                                .with_color(theme::hsla(theme::WARN).opacity(if *on { 0.45 } else { 0.22 })),
                        );
                    }
                    let decos = s.create_range_decorations_collection(decos, cx);
                    center_line(s, line, cx);
                    decos
                });
                let _ = this.update(cx, |app, cx| {
                    let Some(p) = app.palette.as_mut() else { return decos.dispose(cx) };
                    if p.preview_wanted.borrow().as_deref() != Some(key.as_str()) {
                        return decos.dispose(cx);
                    }
                    if let Some(old) = p.preview.take() {
                        old.decos.dispose(cx);
                    }
                    p.preview = Some(Preview { file, key, state, decos });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn select_palette_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.as_mut() {
            p.selected = ix;
            cx.notify();
        }
    }
}

/// Scrolls the preview so `line` (0-based) sits in the middle (`revealLineInCenter`).
fn center_line(s: &mut EditorState, line: u32, cx: &mut Context<EditorState>) {
    let lh = s.line_height().unwrap_or(px(20.));
    let y = (lh * line as f32 - px(100.)).max(px(0.));
    s.set_scroll_offset(gpui::point(px(0.), -y), cx);
}

/// A Lucide icon from gpui-kit-assets, in an explicit colour.
fn lucide(name: IconName, size: f32, color: impl Into<gpui::Hsla>) -> gpui::Svg {
    svg().path(name.path()).size(px(size)).flex_none().text_color(color)
}

fn render_search_input(p: &Palette) -> impl IntoElement {
    // `.search-input-row`: icon, 15px input, the workspace chip.
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(12.))
        .border_b_1()
        .border_color(c(theme::BORDER))
        .child(lucide(IconName::Search, 15., c(theme::TEXT_FAINT)))
        .child(div().flex_1().min_w_0().h(px(28.)).flex().items_center().child(Input::new(&p.input).appearance(false).text_size(px(15.))))
        .children(p.ws.as_ref().map(|(name, _)| {
            div()
                .flex_none()
                .px(px(10.))
                .py(px(3.))
                .rounded_full()
                .bg(c(theme::PANEL_RAISED))
                .text_size(px(11.))
                .text_color(c(theme::TEXT_FAINT))
                .child(name.trim_start_matches('@').to_string())
        }))
}

fn render_row(row: &Row, ix: usize, selected: bool, find: bool, cx: &mut Context<OrbitApp>) -> AnyElement {
    let base = div()
        .id(("palette-row", ix))
        .relative()
        .w_full()
        .h(px(if find { FIND_ROW_H } else { ROW_H }))
        .flex()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden();
    let faint = |s: String| div().flex_none().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(s);
    let path = |s: String| div().min_w_0().truncate().text_size(px(11.)).font_family(theme::MONO_FONT).text_color(c(theme::TEXT_FAINT)).child(s);
    let name = |s: SharedString| div().flex_none().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT)).child(s);
    let kind = |s: &str| {
        div()
            .flex_none()
            .min_w(px(58.))
            .text_size(px(10.5))
            .font_family(theme::MONO_FONT)
            .text_color(c(theme::TEXT_FAINT))
            .child(s.to_string())
    };
    match row {
        Row::Header { title, detail, tab } => {
            // `button.search-section`: "more ›" shows on hover.
            let tab = *tab;
            let group = SharedString::from(format!("search-section-{ix}"));
            base.group(group.clone())
                .items_end()
                .px(px(10.))
                .pb(px(4.))
                .rounded(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(theme::TEXT_FAINT))
                .child(title.to_uppercase())
                .child(div().flex_1())
                .child(div().font_weight(FontWeight::NORMAL).invisible().group_hover(group, |s| s.visible()).child(detail.clone()))
                .when(tab.is_some(), |d| d.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.pick_palette_row(ix, window, cx))))
                .into_any_element()
        }
        Row::Item(Item::Match { m, hits }) => {
            // `.find-row`: click selects, double click opens.
            let (text, hits) = search::display_line(&m.text, hits, 40);
            let mark = HighlightStyle {
                background_color: Some(theme::hsla(theme::WARN).opacity(0.28)),
                color: Some(theme::hsla(theme::WARN)),
                ..Default::default()
            };
            base.px(px(14.))
                .gap(px(12.))
                .cursor_pointer()
                .font_family(theme::MONO_FONT)
                .text_size(px(12.))
                .when(selected, |d| d.bg(theme::accent_soft()).text_color(c(theme::TEXT)))
                .when(!selected, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_RAISED))))
                .child(div().flex_1().min_w_0().overflow_hidden().child(StyledText::new(text).with_highlights(hits.into_iter().map(|h| (h, mark)))))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .max_w(relative(0.4))
                        .overflow_hidden()
                        .font_family(theme::UI_FONT)
                        .text_size(px(11.5))
                        .text_color(c(theme::TEXT_FAINT))
                        .child(format!("{} ", m.path.rsplit('/').next().unwrap_or(&m.path)))
                        .child(div().opacity(0.8).child(m.line.to_string())),
                )
                .child(probe(format!("palette:{}:{}", m.path, m.line)))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.pick_palette_row(ix, window, cx)
                    } else {
                        this.select_palette_row(ix, cx)
                    }
                }))
                .into_any_element()
        }
        Row::Item(item) => {
            // `.search-row`.
            let el = base
                .px(px(10.))
                .gap(px(9.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(selected, |d| d.bg(theme::accent_soft()))
                .hover(|s| s.bg(theme::accent_soft()))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_palette_row(ix, window, cx)));
            match item {
                Item::File { repo, path: rel } => {
                    let file = rel.rsplit('/').next().unwrap_or(rel).to_string();
                    el.child(icons::file_icon(&file, 14.))
                        .child(name(file.clone().into()))
                        .child(path(format!("{repo}/{rel}")))
                        .child(probe(format!("palette:{file}")))
                }
                Item::Symbol(h) => el
                    .child(kind(h.kind))
                    .child(name(h.name.clone().into()))
                    .children(h.container.clone().map(faint))
                    .child(path(format!("{}:{}", h.path.file_name().unwrap_or_default().to_string_lossy(), h.range.start.line + 1)))
                    .child(probe(format!("palette:{}", h.name))),
                Item::Action(cmd) => el
                    .child(kind("action"))
                    .child(name(cmd.title.clone()))
                    .child(div().flex_1())
                    .children(cmd.shortcut().map(|k| path(k.to_string())))
                    .child(probe(format!("palette:{}", cmd.title))),
                Item::Text { m, .. } => el
                    .child(div().flex_1().min_w_0().overflow_hidden().font_family(theme::MONO_FONT).text_size(px(12.)).child(m.text.trim().to_string()))
                    .child(path(format!("{}:{}", m.path.rsplit('/').next().unwrap_or(&m.path), m.line))),
                Item::Match { .. } => el,
            }
            .into_any_element()
        }
    }
}
