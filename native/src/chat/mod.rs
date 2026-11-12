//! Chat sessions: Orbit renders an agent's conversation itself from
//! `orbit_core::chat` events (user bubbles, streamed replies, thinking rows)
//! with a prompt box at the bottom. Deltas are applied in batches: every
//! event that arrived while the UI thread was busy lands in one update.
use crate::app::OrbitApp;
use crate::commands::{self, Command};
use crate::pages::{self, Host, PageView};
use crate::probe::probe;
use crate::sessions::status::Status;
use crate::theme::{self, c};
use futures::StreamExt as _;
use gpui::{
    div, list, prelude::*, px, App, Context, Entity, FollowMode, ListAlignment, ListState, SharedString, Subscription,
    Task, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::text::{TextView, TextViewState};
use gpui_component::diff::{Diff, DiffFile, DiffMode, DiffState};
use orbit_core::chat::{ChatBackend, ChatEvent, ChatInput, FileDiff, PermissionOption};
use regex::Regex;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;

enum Item {
    User(String),
    /// `md` renders `text` with `path:line` mentions turned into links.
    Assistant { id: String, text: String, md: Entity<TextViewState>, _parsed: Subscription },
    Thinking { id: String, text: String, open: bool },
    /// `size` is the drawn size, at most `MAX_IMAGE_H` tall.
    Image { path: PathBuf, title: String, size: Option<(f32, f32)> },
    /// Errors.
    Note(String),
    /// The session ended, with its error if any; the last one offers Restart.
    Exited(String),
    /// A finished turn's duration, tokens and cost.
    Footer(String),
    /// One tool call, or a run of consecutive read-only ones drawn as one row.
    Tool { calls: Vec<Call>, open: bool, all: bool, diff: Option<(Entity<DiffState>, usize)> },
    /// A permission request; `answer` is the picked option's label once answered.
    Permission { id: String, title: String, detail: String, options: Vec<PermissionOption>, answer: Option<String> },
}

struct Call {
    id: String,
    name: String,
    summary: String,
    output: String,
    /// None while running, then whether it succeeded.
    ok: Option<bool>,
}

const READ_ONLY: [&str; 5] = ["Read", "Grep", "Glob", "LS", "list"];
const MAX_OUTPUT_LINES: usize = 200;
/// The diff view's row height at the chat's mono size, and the most rows shown before it scrolls.
const DIFF_ROW_H: f32 = 21.;
const MAX_DIFF_ROWS: usize = 24;

fn tool_icon(name: &str) -> &'static str {
    match name {
        "Read" => "doc",
        "Grep" | "Glob" | "LS" | "list" => "search",
        "Edit" | "MultiEdit" | "Write" => "pencil",
        "Bash" => "terminal",
        _ => "hammer",
    }
}

fn group_label(calls: &[Call]) -> String {
    let reads = calls.iter().filter(|c| c.name == "Read").count();
    let other = calls.len() - reads;
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (reads, other) {
        (0, n) => format!("Searched {}", plural(n, "time", "times")),
        (r, 0) => format!("Read {}", plural(r, "file", "files")),
        (r, n) => format!("Read {}, {}", plural(r, "file", "files"), plural(n, "search", "searches")),
    }
}

fn file_patch(d: &FileDiff) -> String {
    let path = d.path.display().to_string();
    let lines = |t: &str| if t.is_empty() { String::new() } else { format!("{t}\n") };
    let (old, new) = (lines(&d.old_text), lines(&d.new_text));
    let diff = similar::TextDiff::from_lines(&old, &new);
    diff.unified_diff().context_radius(3).header(&format!("a/{path}"), &format!("b/{path}")).to_string()
}

const MAX_IMAGE_H: f32 = 480.;
const IMAGE_EXT: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Known up front so the list measures the row right before the picture loads.
fn image_size(path: &std::path::Path) -> Option<(f32, f32)> {
    let s = imagesize::size(path).ok()?;
    let (w, h) = (s.width as f32, s.height as f32);
    let scale = (MAX_IMAGE_H / h).min(1.);
    (w > 0. && h > 0.).then_some((w * scale, h * scale))
}

fn is_image(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXT.contains(&e.to_lowercase().as_str()))
}

pub struct ChatView {
    /// The tab's key, which is also its inbox key.
    key: String,
    /// The sidebar scope the chat was opened in.
    scope: String,
    agent: String,
    cwd: PathBuf,
    /// The last link or image clicked, for the self-test dump.
    opened: Option<String>,
    backend: Option<Box<dyn ChatBackend>>,
    session_id: Option<String>,
    items: Vec<Item>,
    list: ListState,
    prompt: Entity<TextareaState>,
    /// Images pasted or dropped into the prompt, sent with the next message.
    attached: Vec<PathBuf>,
    /// When the running turn started; None between turns.
    turn: Option<Instant>,
    _events: Task<()>,
    _prompt: Subscription,
}

impl ChatView {
    pub fn new(key: String, scope: String, agent: &str, cwd: PathBuf, resume: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Message the agent…"));
        let sub = cx.subscribe_in(&prompt, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { shift: false, .. }) {
                this.send(window, cx);
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let mut view = Self {
            key,
            scope,
            agent: agent.into(),
            cwd,
            opened: None,
            backend: None,
            session_id: None,
            items: vec![],
            list,
            prompt,
            attached: vec![],
            turn: None,
            _events: Task::ready(()),
            _prompt: sub,
        };
        view.start(resume, window, cx);
        view.prompt.update(cx, |p, cx| p.focus(window, cx));
        view
    }

    fn start(&mut self, resume: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let backend = orbit_core::chat::start(&self.agent, &self.cwd, resume, move |e| {
            let _ = tx.unbounded_send(e);
        });
        self._events = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                }
                let applied = this.update_in(cx, |v, window, cx| {
                    batch.into_iter().for_each(|e| v.on_event(e, window, cx));
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        match backend {
            Ok(b) => self.backend = Some(b),
            Err(e) => self.push(Item::Exited(e)),
        }
    }

    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.backend.is_none() {
            self.start(self.session_id.clone(), window, cx);
            cx.notify();
        }
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let (Some(b), Some(_)) = (&self.backend, self.turn) {
            b.send(ChatInput::Interrupt);
            cx.notify();
        }
    }

    /// Claude takes a prompt as soon as it runs and reports its id only after the first one;
    /// the JSON-RPC agents need their handshake (which ends with the id) first.
    fn ready(&self) -> bool {
        self.backend.is_some() && (self.session_id.is_some() || self.agent == "claude")
    }

    fn pending(&self) -> Option<usize> {
        self.items.iter().rposition(|i| matches!(i, Item::Permission { answer: None, .. }))
    }

    fn status(&self) -> Status {
        if self.pending().is_some() {
            Status::Waiting
        } else if self.backend.is_none() {
            Status::Exited
        } else if self.turn.is_some() {
            Status::Busy
        } else {
            Status::Idle
        }
    }

    /// Answers card `ix` with option `n`, or with its first deny option when `n` is None.
    fn answer(&mut self, ix: usize, n: Option<usize>, cx: &mut Context<Self>) {
        let Some(Item::Permission { id, options, answer: answer @ None, .. }) = self.items.get_mut(ix) else { return };
        let Some(option) = n.map_or_else(|| options.iter().find(|o| o.kind.starts_with("reject")), |n| options.get(n)) else { return };
        if let Some(b) = &self.backend {
            b.send(ChatInput::AnswerPermission { id: id.clone(), option_id: option.id.clone() });
        }
        *answer = Some(option.label.clone());
        self.list.remeasure_items(ix..ix + 1);
        if self.pending().is_none() {
            let key = self.key.clone();
            crate::sessions::store(cx).update(cx, |s, cx| s.settle(&key, cx));
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.pending() else {
            if ev.keystroke.key == "escape" && self.turn.is_some() && gpui::Focusable::focus_handle(self.prompt.read(cx), cx).is_focused(window) {
                self.interrupt(cx);
                cx.stop_propagation();
            }
            return;
        };
        let n = match ev.keystroke.key.as_str() {
            "escape" => None,
            k => match k.parse::<usize>() {
                Ok(d @ 1..=9) => Some(d - 1),
                _ => return,
            },
        };
        self.answer(ix, n, cx);
        cx.stop_propagation();
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        let n = self.items.len();
        self.list.splice(n - 1..n - 1, 1);
    }

    /// Appends to the current turn's reply (or thinking) with this id, or starts one.
    fn append(&mut self, thinking: bool, id: String, delta: &str, cx: &mut Context<Self>) {
        let at = self.items.iter().rposition(|i| match i {
            Item::Assistant { id: x, .. } if !thinking => *x == id,
            Item::Thinking { id: x, .. } if thinking => *x == id,
            _ => false,
        });
        let turn_start = self.items.iter().rposition(|i| matches!(i, Item::User(_)));
        match at.filter(|&ix| turn_start.is_none_or(|t| ix > t)) {
            Some(ix) => {
                match &mut self.items[ix] {
                    Item::Assistant { text, md, .. } => {
                        text.push_str(delta);
                        let linked = linkify(text);
                        md.update(cx, |s, cx| s.set_text(&linked, cx));
                    }
                    Item::Thinking { text, .. } => text.push_str(delta),
                    _ => {}
                }
                self.list.remeasure_items(ix..ix + 1);
            }
            None if thinking => self.push(Item::Thinking { id, text: delta.into(), open: false }),
            None => {
                let md = cx.new(|cx| TextViewState::markdown(&linkify(delta), cx).selectable(true));
                // The parse finishes in the background: only then is the reply's height known.
                let ix = self.items.len();
                let parsed = cx.observe(&md, move |this, _, cx| {
                    this.list.remeasure_items(ix..ix + 1);
                    cx.notify();
                });
                self.push(Item::Assistant { id, text: delta.into(), md, _parsed: parsed });
            }
        }
    }

    fn call(&mut self, id: &str) -> Option<(usize, &mut Call)> {
        self.items.iter_mut().enumerate().rev().find_map(|(ix, item)| match item {
            Item::Tool { calls, .. } => calls.iter_mut().find(|c| c.id == id).map(|c| (ix, c)),
            _ => None,
        })
    }

    fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        use crate::editor::markdown::{resolve_link, Link};
        // A stand-in file so relative links resolve against the session folder.
        match resolve_link(url, &self.cwd.join("_"), None) {
            Link::External(url) => {
                if !crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                    cx.open_url(&url);
                }
            }
            Link::Local { path, line } if path.is_file() => crate::run::open_location(path, line.unwrap_or(1), 1, cx),
            Link::Local { path, .. } if path.is_dir() => crate::file_ops::reveal_in_tree(path, window, cx),
            _ => {}
        }
        self.opened = Some(url.into());
    }

    fn on_event(&mut self, event: ChatEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ChatEvent::SessionId(id) => {
                let was_ready = self.ready();
                self.session_id = Some(id);
                if !was_ready {
                    self.prompt.update(cx, |p, cx| p.focus(window, cx));
                }
            }
            ChatEvent::TextDelta { message_id, text } => self.append(false, message_id, &text, cx),
            ChatEvent::ThinkingDelta { message_id, text } => self.append(true, message_id, &text, cx),
            ChatEvent::TurnEnded { cost_usd, tokens, error } => {
                if let Some(e) = error {
                    self.push(Item::Note(e));
                }
                let took = self.turn.take().map(|t| t.elapsed());
                self.push(Item::Footer(footer(took, tokens, cost_usd)));
                let (key, who) = (self.key.clone(), format!("{} chat finished", self.agent));
                let what = crate::sessions::scope_label(&self.scope);
                crate::sessions::store(cx).update(cx, |s, cx| s.notify(key.clone(), true, who, what, cx));
                // Nothing to report when the user is looking at the chat.
                cx.defer(move |cx| {
                    if crate::sessions::focus_of(&key, cx) == (true, true) {
                        crate::sessions::store(cx).update(cx, |s, cx| s.settle(&key, cx));
                    }
                });
            }
            ChatEvent::Image { path, title } => {
                let size = image_size(&path);
                self.push(Item::Image { path, title, size });
            }
            ChatEvent::ToolStarted { id, name, summary } => {
                let call = Call { id, name, summary, output: String::new(), ok: None };
                let read_only = READ_ONLY.contains(&call.name.as_str());
                match self.items.last_mut() {
                    Some(Item::Tool { calls, .. }) if read_only && calls.iter().all(|c| READ_ONLY.contains(&c.name.as_str())) => {
                        calls.push(call);
                        let n = self.items.len();
                        self.list.remeasure_items(n - 1..n);
                    }
                    _ => self.push(Item::Tool { calls: vec![call], open: false, all: false, diff: None }),
                }
            }
            ChatEvent::ToolOutput { id, text } => {
                if let Some((ix, call)) = self.call(&id) {
                    call.output.push_str(&text);
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
            ChatEvent::ToolFinished { id, ok, diff } => {
                if let Some((ix, call)) = self.call(&id) {
                    call.ok = Some(ok);
                    let state = diff.map(|d| {
                        let patch = file_patch(&d);
                        let language = crate::lsp::file_language(&d.path).highlight;
                        let files = DiffFile::parse(&patch).unwrap_or_default().into_iter().map(|f| f.with_language(language)).collect();
                        (crate::diff_view::highlighted_state(files, DiffMode::Unified, cx), patch.lines().count().saturating_sub(2))
                    });
                    if let (Some(state), Some(Item::Tool { diff, .. })) = (state, self.items.get_mut(ix)) {
                        *diff = Some(state);
                    }
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
            ChatEvent::PermissionAsked { id, title, detail, options } => {
                let (key, who) = (self.key.clone(), format!("{} chat needs your attention", self.agent));
                let what = title.clone();
                crate::sessions::store(cx).update(cx, |s, cx| s.notify(key, false, who, what, cx));
                self.push(Item::Permission { id, title, detail, options, answer: None });
            }
            ChatEvent::Exited { error } => {
                self.backend = None;
                self.turn = None;
                for item in &mut self.items {
                    if let Item::Permission { answer: answer @ None, .. } = item {
                        *answer = Some("not answered".into());
                    }
                }
                let key = self.key.clone();
                crate::sessions::store(cx).update(cx, |s, cx| s.settle(&key, cx));
                self.push(Item::Exited(error.map_or("The session ended.".into(), |e| format!("The session ended: {e}"))));
            }
            _ => {}
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).value().trim().to_string();
        if (text.is_empty() && self.attached.is_empty()) || !self.ready() || self.turn.is_some() {
            return;
        }
        let images = std::mem::take(&mut self.attached);
        if let Some(b) = &self.backend {
            b.send(ChatInput::Prompt { text: text.clone(), images: images.clone() });
        }
        self.turn = Some(Instant::now());
        self.push(Item::User(text));
        for path in images {
            let size = image_size(&path);
            self.push(Item::Image { path, title: String::new(), size });
        }
        self.list.set_follow_mode(FollowMode::Tail);
        self.prompt.update(cx, |p, cx| p.set_value("", window, cx));
        cx.notify();
    }

    fn render_item(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = div().w_full().px_4().py_1p5().flex();
        match &self.items[ix] {
            Item::User(text) => row
                .justify_end()
                .child(
                    div()
                        .max_w(px(560.))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(c(theme::ACCENT_SOFT))
                        .text_color(c(theme::TEXT))
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Assistant { md, .. } => {
                let this = cx.weak_entity();
                let stand_in = self.cwd.join("_");
                let view = TextView::new(md)
                    .style(crate::editor::markdown::preview_style())
                    .selectable(true)
                    .inline_code_style(gpui::StyleRefinement::default().bg(c(theme::PANEL_RAISED)))
                    .on_link_click(move |url, _, window, cx| {
                        let _ = this.update(cx, |v, cx| v.open_link(url, window, cx));
                    })
                    .image_source(move |uri| {
                        use crate::editor::markdown::{resolve_image, ImageRef};
                        match resolve_image(uri, &stand_in, None) {
                            ImageRef::Local(path) => path.into(),
                            _ => uri.clone().into(),
                        }
                    });
                row.relative().text_color(c(theme::TEXT)).child(div().w_full().child(view)).child(probe(format!("reply:{ix}"))).into_any_element()
            }
            Item::Thinking { text, open, .. } => {
                let label = if *open { format!("Thinking\n{text}") } else { "Thinking…".into() };
                row.id(("thinking", ix))
                    .relative()
                    .cursor_pointer()
                    .text_color(c(theme::TEXT_FAINT))
                    .italic()
                    .child(label)
                    .child(probe(format!("thinking:{ix}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Item::Thinking { open, .. }) = this.items.get_mut(ix) {
                            *open = !*open;
                        }
                        this.list.remeasure_items(ix..ix + 1);
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Item::Image { path, title, size } => {
                let open = path.clone();
                let mut pic = gpui::img(path.clone()).max_w_full().max_h(px(MAX_IMAGE_H));
                if let Some((w, h)) = size {
                    pic = pic.w(px(*w)).h(px(*h));
                }
                row.relative()
                    .flex_col()
                    .gap_1()
                    .when(!title.is_empty(), |d| d.text_color(c(theme::TEXT_DIM)).child(title.clone()))
                    .child(div().id(("image", ix)).cursor_pointer().child(pic).on_click(cx.listener(move |this, _, _, cx| {
                        if !crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                            cx.open_with_system(&open);
                        }
                        this.opened = Some(open.display().to_string());
                    })))
                    .child(probe(format!("image:{ix}")))
                    .into_any_element()
            }
            Item::Note(text) => row.text_color(c(theme::DANGER)).child(text.clone()).into_any_element(),
            Item::Footer(text) => row.text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(text.clone()).into_any_element(),
            Item::Exited(text) => {
                let last = self.backend.is_none() && !self.items[ix + 1..].iter().any(|i| matches!(i, Item::Exited(_)));
                row.flex_col()
                    .gap_2()
                    .child(div().text_color(c(theme::DANGER)).child(text.clone()))
                    .when(last, |d| {
                        let restart = crate::pages::kit::mini("chat-restart", "Restart", crate::pages::kit::Kind::Secondary)
                            .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx)));
                        d.child(div().flex().child(restart))
                    })
                    .into_any_element()
            }
            Item::Tool { .. } => self.render_tool(ix, row, cx),
            Item::Permission { title, answer: Some(answer), .. } => {
                row.gap_2()
                    .items_center()
                    .text_color(c(theme::TEXT_DIM))
                    .child(crate::pages::kit::icon("bell", 13., c(theme::TEXT_DIM)))
                    .child(format!("{title} — {answer}"))
                    .into_any_element()
            }
            Item::Permission { title, detail, options, answer: None, .. } => {
                use crate::pages::kit::{button, card, Kind};
                let buttons = options.iter().enumerate().map(|(n, o)| {
                    let kind = if o.kind.starts_with("reject") { Kind::DangerOutline } else if n == 0 { Kind::Primary } else { Kind::Secondary };
                    button(format!("perm:{ix}:{}", n + 1), format!("{}  {}", n + 1, o.label), kind)
                        .on_click(cx.listener(move |this, _, _, cx| this.answer(ix, Some(n), cx)))
                });
                row.child(
                    card()
                        .w_full()
                        .p_3()
                        .gap_2()
                        .border_color(c(theme::WARN))
                        .child(div().text_color(c(theme::TEXT)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.clone()))
                        .when(!detail.is_empty(), |d| {
                            d.child(div().font_family(theme::MONO_FONT).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(detail.clone()))
                        })
                        .child(div().flex().flex_wrap().gap_2().children(buttons))
                        .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child("Press a number to choose, Esc to deny")),
                )
                .into_any_element()
            }
        }
    }

    fn render_tool(&self, ix: usize, row: gpui::Div, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Item::Tool { calls, open, all, diff } = &self.items[ix] else { unreachable!() };
        let (open, all, diff) = (*open, *all, diff.as_ref());
        use crate::pages::kit::icon;
        let toggle = move |this: &mut Self, ix: usize, all_lines: bool| {
            if let Some(Item::Tool { open, all, .. }) = this.items.get_mut(ix) {
                if all_lines {
                    *all = true;
                } else {
                    *open = !*open;
                }
            }
            this.list.remeasure_items(ix..ix + 1);
        };
        let first = &calls[0];
        let (name, summary) =
            if calls.len() > 1 { (group_label(calls), calls.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>().join(", ")) } else { (first.name.clone(), first.summary.clone()) };
        let running = calls.iter().any(|c| c.ok.is_none());
        let failed = calls.iter().any(|c| c.ok == Some(false));
        let status = if running {
            crate::workspace_ui::kit::spinner(gpui::ElementId::NamedInteger("tool-spin".into(), ix as u64), 12., c(theme::ACCENT)).into_any_element()
        } else if failed {
            icon("x", 12., c(theme::DANGER)).into_any_element()
        } else {
            icon("check", 12., c(theme::OK)).into_any_element()
        };
        let header = div()
            .id(("tool", ix))
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(icon(tool_icon(&first.name), 13., c(theme::TEXT_DIM)))
            .child(div().flex_none().text_color(c(theme::TEXT)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT_FAINT)).child(summary))
            .child(status)
            .child(probe(format!("tool:{ix}")))
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this, ix, false);
                cx.notify();
            }));
        let mut body = div().w_full().flex().flex_col().gap_1().child(header);
        if open {
            if let Some((state, rows)) = diff {
                let h = (*rows).clamp(1, MAX_DIFF_ROWS) as f32 * DIFF_ROW_H + 8.;
                body = body.child(div().w_full().h(px(h)).rounded(px(4.)).overflow_hidden().border_1().border_color(c(theme::BORDER)).child(Diff::new(state).size_full().header_visible(false)));
            }
            let output: String = calls.iter().map(|c| c.output.as_str()).filter(|o| !o.is_empty()).collect::<Vec<_>>().join("\n");
            if !output.is_empty() && diff.is_none() {
                let total = output.lines().count();
                let shown = if all || total <= MAX_OUTPUT_LINES { output } else { output.lines().take(MAX_OUTPUT_LINES).collect::<Vec<_>>().join("\n") };
                body = body.child(
                    div()
                        .w_full()
                        .p_2()
                        .rounded(px(4.))
                        .bg(c(theme::PANEL_RAISED))
                        .font_family(theme::MONO_FONT)
                        .text_size(px(12.))
                        .text_color(c(theme::TEXT_DIM))
                        .child(shown),
                );
                if !all && total > MAX_OUTPUT_LINES {
                    body = body.child(
                        div()
                            .id(("tool-all", ix))
                            .relative()
                            .cursor_pointer()
                            .text_color(c(theme::ACCENT))
                            .child(format!("Show all {total} lines"))
                            .child(probe(format!("tool-all:{ix}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                toggle(this, ix, true);
                                cx.notify();
                            })),
                    );
                }
            }
        }
        row.child(body).into_any_element()
    }

    fn attach(&mut self, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        self.attached.extend(paths.into_iter().filter(|p| is_image(p) && p.is_file()));
        cx.notify();
    }

    fn attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_wrap().gap_2().pb_2().children(self.attached.iter().enumerate().map(|(i, path)| {
            div()
                .relative()
                .size(px(56.))
                .rounded(px(4.))
                .border_1()
                .border_color(c(theme::BORDER))
                .overflow_hidden()
                .child(gpui::img(path.clone()).size_full().object_fit(gpui::ObjectFit::Cover))
                .child(
                    div()
                        .id(("unattach", i))
                        .absolute()
                        .top_0()
                        .right_0()
                        .p(px(2.))
                        .bg(c(theme::PANEL))
                        .cursor_pointer()
                        .child(crate::pages::kit::icon("x", 10., c(theme::TEXT_DIM)))
                        .child(probe(format!("unattach:{i}")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.attached.len() {
                                this.attached.remove(i);
                            }
                            cx.notify();
                        })),
                )
        }))
    }
}

impl Drop for ChatView {
    fn drop(&mut self) {
        if let Some(b) = &self.backend {
            b.stop();
        }
    }
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::pages::kit::{button, Kind};
        let ready = self.ready();
        let action = if self.turn.is_some() {
            button("chat-stop", "Stop", Kind::DangerOutline).on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
        } else {
            button("chat-send", "Send", Kind::Primary).when(!ready, |b| b.opacity(0.5)).on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };
        let this = cx.weak_entity();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .capture_key_down(cx.listener(Self::on_key))
            .child(list(self.list.clone(), cx.processor(|this, ix, _, cx| this.render_item(ix, cx))).flex_1().py_2())
            .child(
                div()
                    .relative()
                    .p_3()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| this.attach(paths.paths().to_vec(), cx)))
                    .when(!self.attached.is_empty(), |d| d.child(self.attachments(cx)))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .child(div().flex_1().relative().child(Textarea::new(&self.prompt).disabled(!ready).on_paste(move |item, _, cx| {
                                paste_images(&this, item, cx)
                            })).child(probe("chat-prompt")))
                            .child(action),
                    ),
            )
    }
}

impl PageView for ChatView {
    fn title(&self) -> SharedString {
        match self.status() {
            Status::Waiting => format!("{} (chat) · waiting", self.agent).into(),
            Status::Busy => format!("{} (chat) · working", self.agent).into(),
            _ => format!("{} (chat)", self.agent).into(),
        }
    }

    fn sidebar(&self) -> Option<(String, Status)> {
        Some((self.scope.clone(), self.status()))
    }

    fn dump(&self, out: &mut String, _: &App) {
        let _ = writeln!(out, "  chat {} session {:?} ready {} status {} opened {:?}", self.agent, self.session_id, self.ready(), self.status().label(), self.opened);
        for item in &self.items {
            let _ = match item {
                Item::User(t) => writeln!(out, "    you: {t:?}"),
                Item::Assistant { text, .. } => writeln!(out, "    agent: {text:?}"),
                Item::Thinking { text, open, .. } => writeln!(out, "    thinking ({}): {text:?}", if *open { "open" } else { "collapsed" }),
                Item::Image { path, title, size } => writeln!(out, "    image: {} {title:?} {size:?}", path.display()),
                Item::Note(t) => writeln!(out, "    note: {t:?}"),
                Item::Exited(t) => writeln!(out, "    exited: {t:?}"),
                Item::Footer(t) => writeln!(out, "    footer: {t:?}"),
                Item::Permission { title, options, answer, .. } => {
                    let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                    writeln!(out, "    permission {title:?} {labels:?} answer {answer:?}")
                }
                Item::Tool { calls, open, all, diff } => {
                    let state = |c: &Call| match c.ok {
                        None => "running",
                        Some(true) => "ok",
                        Some(false) => "failed",
                    };
                    let list: Vec<String> = calls.iter().map(|c| format!("{} {:?} {} ({} lines)", c.name, c.summary, state(c), c.output.lines().count())).collect();
                    writeln!(out, "    tool{}{}{}: {}", if *open { " open" } else { "" }, if *all { " all" } else { "" }, if diff.is_some() { " diff" } else { "" }, list.join("; "))
                }
            };
        }
        for path in &self.attached {
            let _ = writeln!(out, "    attached: {}", path.display());
        }
    }
}

/// Takes a pasted image (saved to a temp file) or copied image files; text pastes go through.
fn paste_images(view: &gpui::WeakEntity<ChatView>, item: &gpui::ClipboardItem, cx: &mut App) -> bool {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let mut paths = vec![];
    for entry in item.entries() {
        match entry {
            gpui::ClipboardEntry::Image(image) => {
                let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let file = std::env::temp_dir().join(format!("orbit-paste-{}-{n}.{}", std::process::id(), image.format().extension()));
                if std::fs::write(&file, image.bytes()).is_ok() {
                    paths.push(file);
                }
            }
            gpui::ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().filter(|p| is_image(p)).cloned()),
            gpui::ClipboardEntry::String(_) => {}
        }
    }
    if paths.is_empty() {
        return false;
    }
    let _ = view.update(cx, |v, cx| v.attach(paths, cx));
    true
}

/// `src/a.rs:42` (bare or as inline code, outside fenced blocks) becomes a link to that line.
fn linkify(text: &str) -> String {
    static LOC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(^|\s)(/?(?:[\w.-]+/)*[\w.-]+\.[A-Za-z]\w*):(\d+)(:\d+)?\b").expect("valid regex"));
    static WHOLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(/?(?:[\w.-]+/)*[\w.-]+\.[A-Za-z]\w*):(\d+)(:\d+)?$").expect("valid regex"));
    let mut out = String::with_capacity(text.len() + 32);
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if fenced || fence {
            fenced ^= fence;
            out.push_str(line);
            continue;
        }
        // Odd pieces are inline code spans; the last one is unclosed when the count is even.
        let pieces: Vec<&str> = line.split('`').collect();
        for (i, piece) in pieces.iter().enumerate() {
            let closed = i + 1 < pieces.len();
            match (i % 2 == 1, closed.then(|| WHOLE.captures(piece)).flatten()) {
                (true, Some(m)) => {
                    let _ = write!(out, "[`{piece}`]({}#L{})", &m[1], &m[2]);
                }
                (true, None) => {
                    let _ = write!(out, "`{piece}{}", if closed { "`" } else { "" });
                }
                (false, _) => out.push_str(&LOC.replace_all(piece, "$1[$2:$3$4]($2#L$3)")),
            }
        }
    }
    out
}

/// "12.3s · 1,234 tokens · $0.0120", leaving out what the agent did not report.
fn footer(took: Option<std::time::Duration>, tokens: Option<u64>, cost: Option<f64>) -> String {
    let mut parts = vec![];
    if let Some(d) = took {
        let s = d.as_secs_f64();
        parts.push(if s < 60. { format!("{s:.1}s") } else { format!("{}m {:02}s", s as u64 / 60, s as u64 % 60) });
    }
    if let Some(t) = tokens {
        let digits = t.to_string();
        let grouped: Vec<&str> = digits.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
        parts.push(format!("{} tokens", grouped.join(",")));
    }
    if let Some(c) = cost {
        parts.push(format!("${c:.4}"));
    }
    parts.join(" · ")
}

/// Opens a new chat tab on the selected workspace's folder (or Orbit's own).
fn open(app: &mut OrbitApp, agent: &'static str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cwd = app.workspace().map(|w| w.dir.clone()).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    let key = format!("chat:{n}");
    let scope = crate::sessions::scope(app);
    pages::open(app, key.clone(), move |_: Host, window, cx| ChatView::new(key, scope, agent, cwd, None, window, cx), window, cx);
}

pub fn init(cx: &mut App) {
    // shortcut: a palette entry for testing, until the New Session menu lists chat sessions.
    commands::register(cx, Command::new("chat.newClaude", "New Claude Chat").run(|a, w, cx| open(a, "claude", w, cx)));
}

#[cfg(test)]
mod tests {
    use super::{file_patch, footer, group_label, linkify, Call};
    use orbit_core::chat::FileDiff;

    #[test]
    fn edit_patch_parses_as_one_file() {
        let d = FileDiff { path: "src/a.rs".into(), old_text: "a\nb".into(), new_text: "a\nc".into() };
        let files = gpui_component::diff::DiffFile::parse(&file_patch(&d)).unwrap();
        assert_eq!((files.len(), files[0].additions(), files[0].deletions()), (1, 1, 1));
        let new = FileDiff { path: "n.rs".into(), old_text: String::new(), new_text: "x".into() };
        let new = &gpui_component::diff::DiffFile::parse(&file_patch(&new)).unwrap()[0];
        assert_eq!((new.additions(), new.deletions()), (1, 0));
    }

    #[test]
    fn footer_leaves_out_unknowns() {
        let d = std::time::Duration::from_millis(12_340);
        assert_eq!(footer(Some(d), Some(1_234_567), Some(0.012)), "12.3s · 1,234,567 tokens · $0.0120");
        assert_eq!(footer(Some(std::time::Duration::from_secs(125)), Some(42), None), "2m 05s · 42 tokens");
        assert_eq!(footer(None, None, None), "");
    }

    #[test]
    fn read_only_runs_are_labelled_by_kind() {
        let call = |name: &str| Call { id: String::new(), name: name.into(), summary: String::new(), output: String::new(), ok: None };
        assert_eq!(group_label(&[call("Read"), call("Read")]), "Read 2 files");
        assert_eq!(group_label(&[call("Read"), call("Grep")]), "Read 1 file, 1 search");
        assert_eq!(group_label(&[call("Glob"), call("Grep")]), "Searched 2 times");
    }

    #[test]
    fn links_file_lines_outside_code() {
        assert_eq!(linkify("see src/a.rs:42 now"), "see [src/a.rs:42](src/a.rs#L42) now");
        assert_eq!(linkify("in `src/a.rs:4:2`."), "in [`src/a.rs:4:2`](src/a.rs#L4).");
        assert_eq!(linkify("`let x = a.rs:1` and `b`"), "`let x = a.rs:1` and `b`");
        assert_eq!(linkify("```\nsrc/a.rs:1\n```\nb.rs:2"), "```\nsrc/a.rs:1\n```\n[b.rs:2](b.rs#L2)");
        assert_eq!(linkify("http://host.com:80 and [x](a.rs:3) and `a.rs:5"), "http://host.com:80 and [x](a.rs:3) and `a.rs:5");
    }
}
