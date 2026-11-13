//! Chat sessions: Orbit renders an agent's conversation itself from
//! `orbit_core::chat` events. Deltas are applied in batches: every event that
//! arrived while the UI thread was busy lands in one update.
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
use gpui_component::input::{Enter, Escape, IndentInline, InputEvent, MoveDown, MoveUp, Textarea, TextareaState};
use gpui_component::text::{TextView, TextViewState};
use gpui_component::diff::{Diff, DiffFile, DiffMode, DiffState};
use orbit_core::chat::{ChatBackend, ChatEvent, ChatInput, FileDiff, PermissionOption, SlashCommand};
use regex::Regex;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

enum Item {
    /// As typed: attachments appear as their `[Image #1]` / `[Pasted text #1 · N lines]` references.
    User { text: String },
    /// `md` renders `text` with `path:line` mentions turned into links.
    Assistant { id: String, text: String, md: Entity<TextViewState>, _parsed: Subscription },
    Thinking { id: String, text: String, open: bool },
    /// `size` is the drawn size, at most `MAX_IMAGE_H` tall; `user` for the user's own attachments.
    Image { path: PathBuf, title: String, size: Option<(f32, f32)>, user: bool },
    Note(String),
    /// The session ended, with its error if any; the last one offers Restart.
    Exited(String),
    Footer(String),
    /// One edit, or a run of consecutive other tool calls drawn as one row.
    Tool { calls: Vec<Call>, open: bool, all: bool, diff: Option<(Entity<DiffState>, usize)> },
    /// A permission request; `answer` is the picked option's label once answered.
    Permission { id: String, title: String, detail: String, options: Vec<PermissionOption>, answer: Option<String> },
}

struct Queued {
    /// As typed (shown); `full` has the pastes expanded (sent).
    text: String,
    full: String,
    attached: Vec<(String, PathBuf)>,
}

struct Call {
    id: String,
    name: String,
    summary: String,
    output: String,
    /// None while running, then whether it succeeded.
    ok: Option<bool>,
}

/// Tools drawn alone, for their diff; every other run of consecutive calls shares one row.
const EDITS: [&str; 3] = ["Edit", "MultiEdit", "Write"];
const MAX_OUTPUT_LINES: usize = 200;
/// The diff view's row height at the chat's mono size, and the most rows shown before it scrolls.
const DIFF_ROW_H: f32 = 21.;
const MAX_DIFF_ROWS: usize = 24;
const SLASH_ROWS: usize = 8;
/// The conversation column's width: replies and prompts stay close on wide windows.
const COLUMN_W: f32 = 860.;
/// A paste longer than this becomes an attachment chip instead of prompt text.
const PASTE_LINES: usize = 12;
const PASTE_CHARS: usize = 2000;
/// The mode and model picked last per agent, applied to that agent's next chats in this run.
static PICKED: std::sync::Mutex<Vec<(String, &'static str, String)>> = std::sync::Mutex::new(vec![]);

fn remembered(agent: &str, kind: &str) -> Option<String> {
    PICKED.lock().unwrap().iter().find(|(a, k, _)| a == agent && *k == kind).map(|(_, _, v)| v.clone())
}

fn remember(agent: &str, kind: &'static str, value: &str) {
    let mut picked = PICKED.lock().unwrap();
    picked.retain(|(a, k, _)| !(a == agent && *k == kind));
    picked.push((agent.into(), kind, value.into()));
}

pub fn brand(agent: &str) -> (&'static str, u32) {
    match agent {
        "claude" => ("Claude Code", 0xD97757),
        "codex" => ("Codex", 0x10A37F),
        "opencode" => ("OpenCode", 0x5B8DEF),
        "omp" => ("OMP", 0xA78BFA),
        _ => ("Agent", 0x8B949E),
    }
}

fn tool_color(name: &str) -> u32 {
    match name {
        "Read" | "Grep" | "Glob" | "LS" | "list" => theme::INFO,
        "Edit" | "MultiEdit" | "Write" => theme::PURPLE,
        "Bash" => theme::WARN,
        _ => theme::ACCENT,
    }
}

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
    let count = |f: &dyn Fn(&str) -> bool| calls.iter().filter(|c| f(&c.name)).count();
    let bash = count(&|n| n == "Bash");
    let reads = count(&|n| n == "Read");
    let searches = count(&|n| matches!(n, "Grep" | "Glob" | "LS" | "list"));
    let kinds = [(bash, "ran", "command", "commands"), (reads, "read", "file", "files"), (searches, "searched", "time", "times"), (calls.len() - bash - reads - searches, "used", "tool", "tools")];
    let label = kinds.iter().filter(|k| k.0 > 0).map(|&(n, verb, one, many)| format!("{verb} {n} {}", if n == 1 { one } else { many })).collect::<Vec<_>>().join(", ");
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |f| f.to_uppercase().chain(chars).collect())
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
    attached: Vec<(String, PathBuf)>,
    /// When the running turn started; None between turns.
    turn: Option<Instant>,
    commands: Vec<SlashCommand>,
    /// The highlighted `/` row, and whether Esc closed the list until the next edit.
    slash: usize,
    slash_closed: bool,
    /// When each item was added, so only new ones animate in.
    born: Vec<Instant>,
    /// Long pastes waiting to go with the next message, by their reference in the prompt.
    pasted: Vec<(String, String)>,
    /// How many images and pastes the draft has numbered.
    refs: (usize, usize),
    models: Vec<orbit_core::chat::Choice>,
    /// The running model's id, and the option the user picked.
    model: Option<String>,
    picked: Option<String>,
    model_menu: bool,
    /// Tokens in context, and the window size once the agent reports it.
    context: Option<(u64, Option<u64>)>,
    lightbox: Option<usize>,
    /// Messages sent while a turn ran, each going out when the one before ends.
    queue: Vec<Queued>,
    /// The sent message Up recalled into the prompt, counted back from the latest.
    recalled: Option<usize>,
    /// A multiSelect question's ticked options, by permission id.
    ticked: HashMap<String, Vec<usize>>,
    /// The question card being answered in its own words ("Other…"): its index and input.
    other: Option<(usize, Entity<TextareaState>, Subscription)>,
    modes: Vec<orbit_core::chat::Choice>,
    mode: Option<String>,
    spent: (u64, f64),
    limits: Vec<orbit_core::chat::Limit>,
    /// Set once the agent reported limits itself; the saved ones no longer apply.
    live_limits: bool,
    _ticker: Task<()>,
    _events: Task<()>,
    _prompt: Subscription,
}

impl ChatView {
    pub fn new(key: String, scope: String, agent: &str, cwd: PathBuf, resume: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Message the agent…"));
        let sub = cx.subscribe_in(&prompt, window, |this, _, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                InputEvent::Change => (this.slash, this.slash_closed) = (0, false),
                _ => {}
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
            commands: vec![],
            slash: 0,
            slash_closed: false,
            born: vec![],
            pasted: vec![],
            refs: (0, 0),
            models: vec![],
            model: None,
            picked: None,
            model_menu: false,
            context: None,
            lightbox: None,
            queue: vec![],
            recalled: None,
            ticked: HashMap::new(),
            other: None,
            modes: vec![],
            mode: None,
            spent: (0, 0.),
            limits: vec![],
            live_limits: false,
            _ticker: Task::ready(()),
            _events: Task::ready(()),
            _prompt: sub,
        };
        view.start(resume, window, cx);
        view.refresh_limits(cx);
        view.prompt.update(cx, |p, cx| p.focus(window, cx));
        view
    }

    /// Claude's last saved limits until the chat's own stream reports them.
    fn refresh_limits(&mut self, cx: &mut Context<Self>) {
        if self.agent != "claude" || self.live_limits {
            return;
        }
        cx.spawn(async move |this, cx| {
            let limits = cx.background_executor().spawn(async { claude_limits(orbit_core::agent_hooks::rate_limits()) }).await;
            let _ = this.update(cx, |v, cx| {
                if v.live_limits {
                    return;
                }
                v.limits = limits;
                cx.notify();
            });
        })
        .detach();
    }

    fn set_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        remember(&self.agent, "mode", &mode);
        if let Some(b) = &self.backend {
            b.send(ChatInput::SetMode(mode.clone()));
        }
        self.mode = Some(mode);
        cx.notify();
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

    pub fn busy(&self) -> bool {
        self.turn.is_some()
    }

    pub fn stop(&self) {
        if let Some(b) = &self.backend {
            b.stop();
        }
    }

    pub fn stopped(&self) -> bool {
        self.backend.is_none()
    }

    pub fn status(&self) -> Status {
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
    fn answer(&mut self, ix: usize, n: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Item::Permission { id, options, answer: None, .. }) = self.items.get(ix) else { return };
        let Some(option) = n.map_or_else(|| options.iter().find(|o| o.kind.starts_with("reject")), |n| options.get(n)) else { return };
        let (id, kind) = (id.clone(), option.kind.clone());
        // A click moves focus off the chat; give it back so the next card's number keys still work.
        if kind != "other" {
            self.prompt.update(cx, |p, cx| p.focus(window, cx));
        }
        let (option_id, label) = match kind.as_str() {
            "pick_many" => {
                let ticked = self.ticked.entry(id).or_default();
                let n = n.unwrap_or_default();
                match ticked.iter().position(|&t| t == n) {
                    Some(at) => _ = ticked.remove(at),
                    None => ticked.push(n),
                }
                self.list.remeasure_items(ix..ix + 1);
                return cx.notify();
            }
            "submit" => {
                let mut ticked = self.ticked.get(&id).cloned().unwrap_or_default();
                ticked.sort_unstable();
                let mut labels: Vec<String> = ticked.iter().filter_map(|&t| options.get(t)).map(|o| o.label.clone()).collect();
                let typed = self.other.as_ref().filter(|o| o.0 == ix).map(|o| o.1.read(cx).value().trim().to_string());
                labels.extend(typed.filter(|t| !t.is_empty()));
                if labels.is_empty() {
                    return;
                }
                let joined = labels.join(", ");
                (format!("pick:{joined}"), joined)
            }
            "other" => {
                let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 4).submit_on_enter(true).placeholder("Your answer…"));
                let submit = options.iter().position(|o| o.kind == "submit");
                let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| {
                    let text = input.read(cx).value().trim().to_string();
                    if !matches!(ev, InputEvent::PressEnter { shift: false, .. }) {
                        return;
                    }
                    match submit {
                        Some(n) => this.answer(ix, Some(n), window, cx),
                        None if !text.is_empty() => this.settle_card(ix, format!("pick:{text}"), text, cx),
                        None => {}
                    }
                });
                input.update(cx, |i, cx| i.focus(window, cx));
                self.other = Some((ix, input, sub));
                self.list.remeasure_items(ix..ix + 1);
                return cx.notify();
            }
            _ => (option.id.clone(), option.label.clone()),
        };
        self.settle_card(ix, option_id, label, cx);
    }

    /// Options in number order: a question's rows (its Submit and Skip stay unnumbered), else every option.
    fn numbered(options: &[PermissionOption]) -> Vec<usize> {
        let question = options.iter().any(|o| o.kind == "other");
        (0..options.len()).filter(|&n| !question || matches!(options[n].kind.as_str(), "allow_once" | "pick_many" | "other")).collect()
    }

    /// Sends card `ix`'s answer and marks it answered.
    fn settle_card(&mut self, ix: usize, option_id: String, label: String, cx: &mut Context<Self>) {
        let Some(Item::Permission { id, answer: answer @ None, .. }) = self.items.get_mut(ix) else { return };
        // Skipping one question denies the whole AskUserQuestion: its sibling cards go with it.
        let siblings = (option_id == "deny").then(|| id.split_once('#').map(|(base, _)| format!("{base}#"))).flatten();
        if let Some(b) = &self.backend {
            b.send(ChatInput::AnswerPermission { id: id.clone(), option_id });
        }
        self.ticked.remove(id.as_str());
        *answer = Some(label);
        if let Some(prefix) = siblings {
            for item in &mut self.items {
                if let Item::Permission { id, answer: answer @ None, .. } = item {
                    if id.starts_with(&prefix) {
                        *answer = Some("Skip".into());
                    }
                }
            }
            self.list.remeasure_items(0..self.items.len());
        }
        if self.other.as_ref().is_some_and(|o| o.0 == ix) {
            self.other = None;
        }
        self.list.remeasure_items(ix..ix + 1);
        if self.pending().is_none() {
            let key = self.key.clone();
            crate::sessions::store(cx).update(cx, |s, cx| s.settle(&key, cx));
        }
        cx.notify();
    }

    fn slash_matches(&self, cx: &App) -> Vec<usize> {
        if self.slash_closed {
            return vec![];
        }
        filter_commands(&self.commands, &self.prompt.read(cx).value())
    }

    fn complete(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| {
            let value = p.value();
            let text = format!("{}/{name} ", &value[..slash_start(&value).unwrap_or(0)]);
            let (line, last) = (text.matches('\n').count() as u32, text.rsplit('\n').next().unwrap_or_default());
            let end = last.chars().count() as u32;
            p.set_value(text, window, cx);
            p.set_cursor_position(lsp_types::Position::new(line, end), window, cx);
        });
        cx.notify();
    }

    /// The prompt's own key bindings run before key listeners, so these come in as its actions.
    fn on_slash_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.slash_matches(cx);
        if matches.is_empty() {
            if let "up" | "down" = key {
                self.on_history_key(key == "up", window, cx);
            }
            return;
        }
        match key {
            "up" => self.slash = self.slash.checked_sub(1).unwrap_or(matches.len() - 1),
            "down" => self.slash = (self.slash + 1) % matches.len(),
            "tab" | "enter" => {
                let name = self.commands[matches[self.slash.min(matches.len() - 1)]].name.clone();
                self.complete(&name, window, cx);
            }
            _ => self.slash_closed = true,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Up/Down through sent messages, like a shell, while the prompt is empty or holds an untouched recall.
    /// shortcut: a recalled `[Pasted text #N]` goes out as the bare reference; keep `full` on `Item::User` if that bites.
    fn on_history_key(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sent: Vec<String> = self.queue.iter().rev().map(|q| q.text.clone())
            .chain(self.items.iter().rev().filter_map(|i| match i {
                Item::User { text } if !text.is_empty() => Some(text.clone()),
                _ => None,
            }))
            .collect();
        let value = self.prompt.read(cx).value().to_string();
        let at = self.recalled.filter(|&n| sent.get(n).is_some_and(|t| *t == value));
        if (!value.is_empty() && at.is_none()) || sent.is_empty() {
            return;
        }
        self.recalled = match (at, up) {
            (None, false) => return,
            (None, true) => Some(0),
            (Some(n), true) => Some((n + 1).min(sent.len() - 1)),
            (Some(n), false) => n.checked_sub(1),
        };
        let text = self.recalled.map_or_else(String::new, |n| sent[n].clone());
        self.prompt.update(cx, |p, cx| {
            let (line, last) = (text.matches('\n').count() as u32, text.rsplit('\n').next().unwrap_or_default());
            let end = last.chars().count() as u32;
            p.set_value(text, window, cx);
            p.set_cursor_position(lsp_types::Position::new(line, end), window, cx);
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn on_key(&mut self, ev: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.lightbox.is_some() {
            match ev.keystroke.key.as_str() {
                "escape" => self.lightbox = None,
                "left" => self.step_lightbox(-1, cx),
                "right" => self.step_lightbox(1, cx),
                _ => return,
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let Some(ix) = self.pending() else {
            if ev.keystroke.key == "escape" && self.turn.is_some() && gpui::Focusable::focus_handle(self.prompt.read(cx), cx).is_focused(window) {
                self.interrupt(cx);
                cx.stop_propagation();
            }
            return;
        };
        let n = match ev.keystroke.key.as_str() {
            "escape" => None,
            _ if self.other.is_some() => return,
            k => match (k.parse::<usize>(), self.items.get(ix)) {
                (Ok(d @ 1..=9), Some(Item::Permission { options, .. })) => match Self::numbered(options).get(d - 1) {
                    Some(&n) => Some(n),
                    None => return,
                },
                _ => return,
            },
        };
        self.answer(ix, n, window, cx);
        cx.stop_propagation();
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        self.born.push(Instant::now());
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
        let turn_start = self.items.iter().rposition(|i| matches!(i, Item::User { .. }));
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
            Link::Local { path, line } => {
                if let Some(found) = find_under(&self.cwd, path.strip_prefix(&self.cwd).unwrap_or(&path)) {
                    crate::run::open_location(found, line.unwrap_or(1), 1, cx);
                }
            }
            _ => {}
        }
        self.opened = Some(url.into());
    }

    fn on_event(&mut self, event: ChatEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ChatEvent::SessionId(id) => {
                let was_ready = self.ready();
                // A session tab in chat mode keeps the id, to resume it in the other mode or after a restart.
                let key = self.key.clone();
                crate::sessions::store(cx).update(cx, |s, cx| s.set_agent_session_id(&key, &id, cx));
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
                self.spent.0 += tokens.unwrap_or(0);
                self.spent.1 += cost_usd.unwrap_or(0.);
                self.push(Item::Footer(footer(took, tokens, cost_usd)));
                self.refresh_limits(cx);
                // A queued message goes out next; "finished" waits for the last one.
                if self.backend.is_some() && !self.queue.is_empty() {
                    let next = self.queue.remove(0);
                    self.dispatch(next, cx);
                    return;
                }
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
                self.push(Item::Image { path, title, size, user: false });
            }
            ChatEvent::ToolStarted { id, name, summary } => {
                let call = Call { id, name, summary, output: String::new(), ok: None };
                let groups = !EDITS.contains(&call.name.as_str());
                match self.items.last_mut() {
                    Some(Item::Tool { calls, .. }) if groups && calls.iter().all(|c| !EDITS.contains(&c.name.as_str())) => {
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
            // Claude's per-turn init lists names only: keep the described list from its handshake.
            ChatEvent::Commands(commands) if commands.iter().all(|c| c.description.is_empty()) && !self.commands.is_empty() => {}
            ChatEvent::Commands(commands) => self.commands = commands,
            ChatEvent::Models(models) => {
                let picked = remembered(&self.agent, "model").filter(|v| models.iter().any(|m| &m.value == v));
                self.models = models;
                if let (Some(v), None) = (picked, &self.picked) {
                    self.pick_model(v, cx);
                }
            }
            ChatEvent::Modes(modes) => self.modes = modes,
            ChatEvent::Model(model) => self.model = Some(model).filter(|m| !m.is_empty()),
            // A mode picked earlier in Orbit wins over the one the agent starts in.
            ChatEvent::Mode(mode) => match remembered(&self.agent, "mode").filter(|m| *m != mode && self.modes.iter().any(|c| &c.value == m)) {
                Some(m) if self.mode.is_none() => self.set_mode(m, cx),
                _ => self.mode = Some(mode),
            },
            ChatEvent::Limits(limits) => {
                self.live_limits = true;
                self.limits = limits;
            }
            ChatEvent::Context { used, window } => {
                let window = window.or(self.context.and_then(|c| c.1));
                self.context = Some((used, window));
            }
            // The agent's store has the whole conversation, so a restart's history replaces what is shown.
            ChatEvent::History(turns) => {
                self.items.clear();
                self.born.clear();
                self.lightbox = None;
                self.list.reset(0);
                for turn in turns {
                    self.push(Item::User { text: turn.prompt });
                    for path in turn.images {
                        let size = image_size(&path);
                        self.push(Item::Image { path, title: String::new(), size, user: true });
                    }
                    turn.events.into_iter().for_each(|e| self.on_event(e, window, cx));
                }
                // History is already there: nothing animates in.
                let past = Instant::now() - std::time::Duration::from_secs(1);
                self.born.iter_mut().for_each(|b| *b = past);
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
        }
    }

    /// Sends `text` as if typed; while a turn runs it waits in the composer.
    pub fn submit(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| p.set_value(text, window, cx));
        self.send(window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).value().trim().to_string();
        if (text.is_empty() && self.attached.is_empty() && self.pasted.is_empty()) || !self.ready() {
            return;
        }
        let attached = std::mem::take(&mut self.attached);
        let full = expand_pastes(&text, &std::mem::take(&mut self.pasted));
        self.refs = (0, 0);
        self.prompt.update(cx, |p, cx| p.set_value("", window, cx));
        let queued = Queued { text, full, attached };
        if self.turn.is_some() {
            self.queue.push(queued);
            return cx.notify();
        }
        self.dispatch(queued, cx);
    }

    fn dispatch(&mut self, Queued { text, full, attached }: Queued, cx: &mut Context<Self>) {
        let images: Vec<PathBuf> = attached.iter().map(|(_, p)| p.clone()).collect();
        if let Some(b) = &self.backend {
            b.send(ChatInput::Prompt { text: full, images: images.clone() });
        }
        self.turn = Some(Instant::now());
        self._ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let Ok(true) = this.update(cx, |v, cx| {
                cx.notify();
                v.turn.is_some()
            }) else { break };
        });
        self.push(Item::User { text });
        for (label, path) in attached {
            let size = image_size(&path);
            self.push(Item::Image { path, title: label, size, user: true });
        }
        self.list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    /// Whether item `ix` opens the agent's reply to a prompt (it gets the agent's name above it).
    fn starts_reply(&self, ix: usize) -> bool {
        let before = self.items[..ix].iter().rev().find(|i| !matches!(i, Item::Image { user: true, .. }));
        !matches!(self.items[ix], Item::User { .. } | Item::Footer(_) | Item::Exited(_) | Item::Image { user: true, .. })
            && matches!(before, None | Some(Item::User { .. }))
    }

    fn render_item(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut inner = div().w_full().flex().flex_col();
        if matches!(self.items[ix], Item::User { .. }) && ix > 0 {
            inner = inner.pt_5();
        }
        if self.starts_reply(ix) {
            let (name, color) = brand(&self.agent);
            inner = inner.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_3()
                    .pb_1()
                    .child(div().size(px(18.)).rounded_full().flex().items_center().justify_center().bg(crate::pages::kit::alpha(color, 0.18)).child(crate::pages::kit::icon("spark", 11., c(color))))
                    .child(div().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(color)).child(name)),
            );
        }
        let inner = inner.child(self.render_inner(ix, cx));
        let column = div().w_full().max_w(px(COLUMN_W)).px_6();
        let fresh = self.born.get(ix).is_some_and(|b| b.elapsed() < std::time::Duration::from_millis(400));
        let column = if fresh {
            crate::motion::rise_in(column.child(inner), ("born", ix), std::time::Duration::from_millis(180), 6., std::time::Duration::ZERO, crate::motion::standard())
        } else {
            column.child(inner).into_any_element()
        };
        div().w_full().flex().justify_center().child(column).into_any_element()
    }

    fn render_inner(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = div().w_full().py_1().flex();
        match &self.items[ix] {
            Item::User { text } => row
                .justify_end()
                .child(
                    div()
                        .max_w(px(620.))
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1()
                        .group("user-msg")
                        .when(!text.is_empty(), |d| {
                            let copy = text.clone();
                            d.child(
                                div()
                                    .px_3p5()
                                    .py_2()
                                    .rounded(px(14.))
                                    .rounded_br(px(4.))
                                    .bg(theme::accent_soft())
                                    .border_1()
                                    .border_color(crate::pages::kit::alpha(theme::ACCENT, 0.35))
                                    .text_color(c(theme::TEXT))
                                    .child(text.clone()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("user-copy:{ix}")))
                                    .p_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .invisible()
                                    .group_hover("user-msg", |s| s.visible())
                                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Copy").build(window, cx))
                                    .child(crate::pages::kit::icon("copy", 13., c(theme::TEXT_DIM)))
                                    .child(probe(format!("user-copy:{ix}")))
                                    .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
                            )
                        }),
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
                let live = self.turn.is_some() && ix + 1 == self.items.len();
                let chevron = if *open { "chevron" } else { "chevron-right" };
                let head = div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(crate::pages::kit::icon(chevron, 11., c(theme::TEXT_FAINT)))
                    .child(if live { "Thinking…" } else { "Thought" });
                let head = if live { crate::motion::blink(head, ("think-blink", ix), std::time::Duration::from_millis(1400), 0.4) } else { head.into_any_element() };
                row.id(("thinking", ix))
                    .relative()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(c(theme::TEXT_FAINT))
                    .child(head)
                    .when(*open, |d| d.child(div().ml(px(5.)).pl_3().border_l_2().border_color(c(theme::BORDER_STRONG)).italic().child(text.clone())))
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
            Item::Image { path, title, size, user: true } => {
                let (w, h) = size.map_or((160., 120.), |(w, h)| (w.min(160. * w / h.max(1.)).min(220.), h.min(160.)));
                row.justify_end()
                    .child(
                        div()
                            .id(("image", ix))
                            .relative()
                            .rounded(px(8.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(c(theme::BORDER_STRONG))
                            .cursor_pointer()
                            .child(gpui::img(path.clone()).w(px(w)).h(px(h)).object_fit(gpui::ObjectFit::Cover))
                            .child(probe(format!("image:{ix}")))
                            .tooltip({
                                let t = if title.is_empty() { file_name(path) } else { title.clone() };
                                move |window, cx| gpui_component::tooltip::Tooltip::new(t.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| this.open_lightbox(ix, window, cx))),
                    )
                    .into_any_element()
            }
            Item::Image { path, title, size, .. } => {
                let mut pic = gpui::img(path.clone()).max_w_full().max_h(px(MAX_IMAGE_H));
                if let Some((w, h)) = size {
                    pic = pic.w(px(*w)).h(px(*h));
                }
                let caption = if title.is_empty() { file_name(path) } else { title.clone() };
                row.child(
                    div()
                        .min_w(px(320.))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .bg(c(theme::PANEL))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_3()
                                .pr_1p5()
                                .py_1()
                                .border_b_1()
                                .border_color(c(theme::BORDER))
                                .child(crate::pages::kit::icon("image", 13., c(theme::ACCENT)))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(c(theme::TEXT)).child(caption))
                                .children(self.image_actions(ix, path, true, cx)),
                        )
                        .child(
                            div()
                                .id(("image", ix))
                                .relative()
                                .p_3()
                                .flex()
                                .justify_center()
                                .bg(c(theme::BG))
                                .cursor_pointer()
                                .child(pic)
                                .child(probe(format!("image:{ix}")))
                                .on_click(cx.listener(move |this, _, window, cx| this.open_lightbox(ix, window, cx))),
                        ),
                )
                .into_any_element()
            }
            Item::Note(text) => row
                .gap_2()
                .items_center()
                .px_3()
                .py_2()
                .rounded(px(6.))
                .bg(theme::danger_soft())
                .text_color(c(theme::DANGER))
                .child(crate::pages::kit::icon("x", 12., c(theme::DANGER)))
                .child(text.clone())
                .into_any_element(),
            Item::Footer(text) => row
                .items_center()
                .gap_3()
                .pt_1()
                .text_size(px(11.))
                .text_color(c(theme::TEXT_FAINT))
                .child(div().flex_1().h(px(1.)).bg(c(theme::BORDER)))
                .child(text.clone())
                .into_any_element(),
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
                let denied = answer.starts_with("Deny") || answer.starts_with("Reject") || answer == "Skip" || answer == "not answered";
                let tone = if denied { theme::DANGER } else { theme::OK };
                row.gap_2()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .child(crate::pages::kit::icon("bell", 12., c(theme::TEXT_FAINT)))
                    .child(title.clone())
                    .child(div().px_1p5().rounded(px(4.)).bg(crate::pages::kit::alpha(tone, 0.15)).text_color(c(tone)).child(answer.clone()))
                    .into_any_element()
            }
            Item::Permission { id, title, detail, options, answer: None, .. } => {
                use crate::pages::kit::{alpha, button, card, Kind};
                let ticked = self.ticked.get(id).cloned().unwrap_or_default();
                let typing = self.other.as_ref().filter(|o| o.0 == ix).map(|o| o.1.clone());
                // A question lists its options one per row, like Claude's own; a permission keeps a row of buttons.
                let question = options.iter().any(|o| o.kind == "other");
                let order = Self::numbered(options);
                let number = |n: usize| order.iter().position(|&o| o == n).map_or(String::new(), |p| (p + 1).to_string());
                let numbered = |n: usize, o: &PermissionOption| if question { number(n) } else { format!("{}  {}", number(n), o.label) };
                let choice = |n: usize, o: &PermissionOption, cx: &mut Context<Self>| {
                    let on = ticked.contains(&n) || (typing.is_some() && o.kind == "other");
                    let body = match (&typing, o.kind.as_str()) {
                        (Some(input), "other") => div().flex_1().min_w_0().child(Textarea::new(input).appearance(false)).into_any_element(),
                        _ => div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_color(c(theme::TEXT)).font_weight(gpui::FontWeight::MEDIUM).child(o.label.clone()))
                            .when(!o.description.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(o.description.clone())))
                            .into_any_element(),
                    };
                    let name = format!("perm:{ix}:{}", n + 1);
                    div()
                        .id(SharedString::from(name.clone()))
                        .relative()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .px_3()
                        .py_2()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(c(if on { theme::ACCENT } else { theme::BORDER }))
                        .bg(if on { alpha(theme::ACCENT, 0.12) } else { c(theme::PANEL_RAISED) })
                        .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                        .cursor_pointer()
                        .child(div().w(px(12.)).text_color(c(theme::TEXT_FAINT)).child(numbered(n, o)))
                        .when(o.kind == "pick_many", |d| {
                            let tick = div().size(px(14.)).flex_none().flex().items_center().justify_center().rounded(px(3.)).border_1();
                            d.child(if on { tick.bg(c(theme::ACCENT)).border_color(c(theme::ACCENT)).child(crate::pages::kit::icon("check", 11., c(theme::ON_ACCENT))) } else { tick.border_color(c(theme::TEXT_FAINT)) })
                        })
                        .child(body)
                        .child(probe(name))
                        .on_click(cx.listener(move |this, _, window, cx| this.answer(ix, Some(n), window, cx)))
                        .into_any_element()
                };
                let mut rows = vec![];
                let mut buttons = vec![];
                for (n, o) in options.iter().enumerate() {
                    if question && matches!(o.kind.as_str(), "allow_once" | "pick_many" | "other") {
                        rows.push(choice(n, o, cx));
                        continue;
                    }
                    let kind = match o.kind.as_str() {
                        k if k.starts_with("reject") => Kind::DangerOutline,
                        _ if n == 0 || o.kind == "submit" => Kind::Primary,
                        _ => Kind::Secondary,
                    };
                    let label = if question { o.label.clone() } else { numbered(n, o) };
                    buttons.push(button(format!("perm:{ix}:{}", n + 1), label, kind).on_click(cx.listener(move |this, _, window, cx| this.answer(ix, Some(n), window, cx))));
                }
                let hint = match (typing.is_some(), question) {
                    (true, _) => "Enter to send, Esc to skip",
                    (false, true) => "Press a number to choose, Esc to skip",
                    (false, false) => "Press a number to choose, Esc to deny",
                };
                row.child(
                    card()
                        .w_full()
                        .p_3()
                        .gap_2()
                        .border_color(c(theme::WARN))
                        .border_l_4()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(crate::pages::kit::icon("bell", 13., c(theme::WARN)))
                                .child(div().text_color(c(theme::TEXT)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.clone())),
                        )
                        .when(!detail.is_empty(), |d| {
                            d.child(div().p_2().rounded(px(4.)).bg(c(theme::BG)).font_family(theme::MONO_FONT).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(detail.clone()))
                        })
                        .when(!rows.is_empty(), |d| d.child(div().flex().flex_col().gap_1p5().children(rows)))
                        .child(div().flex().flex_wrap().gap_2().children(buttons))
                        .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(hint)),
                )
                .into_any_element()
            }
        }
    }

    fn image_actions(&self, ix: usize, path: &std::path::Path, expand: bool, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let action = |id: &str, icon: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(format!("{id}:{ix}")))
                .relative()
                .p_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
                .child(crate::pages::kit::icon(icon, 13., c(theme::TEXT_DIM)))
                .child(probe(format!("{id}:{ix}")))
        };
        let probing = || crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed);
        let (open, dir, copy) = (path.to_path_buf(), path.parent().map(|p| p.to_path_buf()).unwrap_or_default(), path.display().to_string());
        let mut out = vec![];
        if expand {
            out.push(action("img-expand", "maximize", "Expand").on_click(cx.listener(move |this, _, window, cx| this.open_lightbox(ix, window, cx))).into_any_element());
        }
        out.push(
            action("img-open", "external", "Open in the default app")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !probing() {
                        cx.open_with_system(&open);
                    }
                    this.opened = Some(open.display().to_string());
                }))
                .into_any_element(),
        );
        out.push(
            action("img-folder", "folder", "Show in folder")
                .on_click(move |_, _, cx| {
                    if !probing() {
                        cx.open_with_system(&dir);
                    }
                })
                .into_any_element(),
        );
        out.push(action("img-copy", "copy", "Copy path").on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))).into_any_element());
        out
    }

    fn images(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| matches!(self.items[i], Item::Image { .. })).collect()
    }

    fn open_lightbox(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.lightbox = Some(ix);
        // Its keys (Esc, arrows) arrive through the prompt's actions.
        self.prompt.update(cx, |p, cx| p.focus(window, cx));
        cx.notify();
    }

    /// Steps the lightbox to the previous (-1) or next (+1) image of the chat.
    fn step_lightbox(&mut self, by: isize, cx: &mut Context<Self>) {
        let images = self.images();
        if let Some(at) = self.lightbox.and_then(|ix| images.iter().position(|&i| i == ix)) {
            let n = images.len() as isize;
            self.lightbox = Some(images[((at as isize + by).rem_euclid(n)) as usize]);
            cx.notify();
        }
    }

    fn render_lightbox(&self, ix: usize, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let Item::Image { path, title, .. } = self.items.get(ix)? else { return None };
        let images = self.images();
        let at = images.iter().position(|&i| i == ix).unwrap_or(0);
        let caption = if title.is_empty() { file_name(path) } else { title.clone() };
        // Never past its own size: a small picture stays sharp.
        let mut pic = gpui::img(path.clone()).max_w_full().max_h_full().object_fit(gpui::ObjectFit::Contain);
        if let Ok(s) = imagesize::size(path) {
            pic = pic.w(px(s.width as f32)).h(px(s.height as f32));
        }
        let close = cx.listener(|this, _, _, cx| {
            this.lightbox = None;
            cx.notify();
        });
        let bar = div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1p5()
            .rounded(px(8.))
            .bg(c(theme::PANEL))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().max_w(px(480.)).truncate().text_color(c(theme::TEXT)).child(caption))
            .when(images.len() > 1, |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child(format!("{} / {}", at + 1, images.len()))))
            .children(self.image_actions(ix, path, false, cx))
            .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(if images.len() > 1 { "← → · Esc" } else { "Esc" }));
        let el = div()
            .id("lightbox")
            .absolute()
            .inset_0()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .bg(gpui::rgba(0x000000c8))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, close)
            .child(div().flex_1().min_h_0().w_full().flex().items_center().justify_center().overflow_hidden().child(pic))
            .child(bar)
            .child(probe("lightbox"));
        Some(crate::motion::overlay_in(el, ("lightbox-in", ix)))
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
        let tint = tool_color(&first.name);
        let header = div()
            .id(("tool", ix))
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .px_2p5()
            .py_1p5()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::PANEL_HOVER)))
            .child(div().size(px(20.)).flex_none().rounded(px(5.)).flex().items_center().justify_center().bg(crate::pages::kit::alpha(tint, 0.16)).child(icon(tool_icon(&first.name), 12., c(tint))))
            .child(div().flex_none().text_color(c(theme::TEXT)).font_weight(gpui::FontWeight::MEDIUM).child(name))
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(summary))
            .child(status)
            .child(icon(if open { "chevron" } else { "chevron-right" }, 11., c(theme::TEXT_FAINT)))
            .child(probe(format!("tool:{ix}")))
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this, ix, false);
                cx.notify();
            }));
        let border = if failed { theme::DANGER } else { theme::BORDER };
        let mut body = div().w_full().flex().flex_col().rounded(px(8.)).border_1().border_color(c(border)).bg(c(theme::PANEL)).overflow_hidden().text_size(px(12.5)).child(header);
        if open {
            let mut inside = div().w_full().flex().flex_col().gap_1().p_2().border_t_1().border_color(c(theme::BORDER));
            if let Some((state, rows)) = diff {
                let h = (*rows).clamp(1, MAX_DIFF_ROWS) as f32 * DIFF_ROW_H + 8.;
                inside = inside.child(div().w_full().h(px(h)).rounded(px(4.)).overflow_hidden().border_1().border_color(c(theme::BORDER)).child(Diff::new(state).size_full().header_visible(false)));
            }
            let output: String = if calls.len() > 1 {
                calls.iter().map(|c| format!("▸ {} {}\n{}", c.name, c.summary, c.output.trim_end())).collect::<Vec<_>>().join("\n\n")
            } else {
                first.output.clone()
            };
            if !output.is_empty() && diff.is_none() {
                let total = output.lines().count();
                let shown = if all || total <= MAX_OUTPUT_LINES { output } else { output.lines().take(MAX_OUTPUT_LINES).collect::<Vec<_>>().join("\n") };
                inside = inside.child(
                    div()
                        .w_full()
                        .p_2()
                        .rounded(px(4.))
                        .bg(c(theme::BG))
                        .font_family(theme::MONO_FONT)
                        .text_size(px(12.))
                        .text_color(c(theme::TEXT_DIM))
                        .child(shown),
                );
                if !all && total > MAX_OUTPUT_LINES {
                    inside = inside.child(
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
            body = body.child(inside);
        }
        row.child(body).into_any_element()
    }

    fn slash_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.slash_matches(cx);
        let sel = self.slash.min(matches.len().saturating_sub(1));
        let first = sel.saturating_sub(SLASH_ROWS - 1);
        let rows = matches.iter().enumerate().skip(first).take(SLASH_ROWS).map(|(n, &i)| {
            let cmd = &self.commands[i];
            let name = cmd.name.clone();
            div()
                .id(("slash", n))
                .relative()
                .flex()
                .gap_3()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(n == sel, |d| d.bg(c(theme::ACCENT_SOFT)))
                .child(div().flex_none().text_color(c(theme::TEXT)).child(format!("/{}", cmd.name)))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT_FAINT)).child(cmd.description.clone()))
                .child(probe(format!("slash:{n}")))
                .on_click(cx.listener(move |this, _, window, cx| this.complete(&name, window, cx)))
        });
        div().when(!matches.is_empty(), |d| {
            d.absolute()
                .bottom_full()
                .left_3()
                .right_3()
                .p_1()
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::PANEL_RAISED))
                .children(rows)
        })
    }

    /// Adds images and long pastes, each with a reference typed at the cursor that marks where it belongs.
    fn attach(&mut self, paths: Vec<PathBuf>, texts: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let mut refs = String::new();
        for path in paths.into_iter().filter(|p| is_image(p) && p.is_file()) {
            self.refs.0 += 1;
            let label = format!("[Image #{}]", self.refs.0);
            refs.push_str(&label);
            refs.push(' ');
            self.attached.push((label, path));
        }
        for text in texts {
            self.refs.1 += 1;
            let label = format!("[Pasted text #{} · {} lines]", self.refs.1, text.lines().count());
            refs.push_str(&label);
            refs.push(' ');
            self.pasted.push((label, text));
        }
        if !refs.is_empty() {
            self.prompt.update(cx, |p, cx| p.insert(refs, window, cx));
        }
        cx.notify();
    }

    fn detach(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.attached.retain(|(l, _)| l != label);
        self.pasted.retain(|(l, _)| l != label);
        let value = self.prompt.read(cx).value().replace(&format!("{label} "), "").replace(label, "");
        self.prompt.update(cx, |p, cx| p.set_value(value, window, cx));
        cx.notify();
    }

    fn attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let remove = |id: SharedString, label: String| {
            div()
                .id(id.clone())
                .relative()
                .p(px(2.))
                .rounded(px(3.))
                .bg(c(theme::PANEL))
                .cursor_pointer()
                .child(crate::pages::kit::icon("x", 10., c(theme::TEXT_DIM)))
                .child(probe(id.to_string()))
                .on_click(cx.listener(move |this, _, window, cx| this.detach(&label, window, cx)))
        };
        let images = self.attached.iter().enumerate().map(|(i, (label, path))| {
            div()
                .relative()
                .size(px(56.))
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::BORDER))
                .overflow_hidden()
                .child(gpui::img(path.clone()).size_full().object_fit(gpui::ObjectFit::Cover))
                .child(remove(format!("unattach:{i}").into(), label.clone()).absolute().top_0().right_0())
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .px_1()
                        .rounded_tr(px(4.))
                        .bg(gpui::rgba(0x000000a0))
                        .text_size(px(10.))
                        .text_color(gpui::rgb(0xffffff))
                        .child(label.trim_start_matches("[Image ").trim_end_matches(']').to_string()),
                )
                .into_any_element()
        });
        let pastes = self.pasted.iter().enumerate().map(|(i, (label, _))| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(paste_chip(label.trim_matches(|c| c == '[' || c == ']').to_string()))
                .child(remove(format!("unpaste:{i}").into(), label.clone()))
                .into_any_element()
        });
        div().flex().flex_wrap().items_end().gap_2().children(images.chain(pastes))
    }

    fn model_label(&self) -> String {
        let by_value = |v: &str| self.models.iter().find(|m| m.value == v);
        let running = self.model.as_deref().and_then(|id| self.models.iter().find(|m| m.resolved == id && m.value != "default"));
        match (self.picked.as_deref().and_then(by_value), running) {
            (Some(m), _) | (None, Some(m)) => m.label.clone(),
            _ => self.model.clone().or_else(|| by_value("default").map(|m| m.label.clone())).unwrap_or_else(|| "Default model".into()),
        }
    }

    fn pick_model(&mut self, value: String, cx: &mut Context<Self>) {
        remember(&self.agent, "model", &value);
        if let Some(b) = &self.backend {
            b.send(ChatInput::SetModel(value.clone()));
        }
        self.picked = Some(value);
        self.model_menu = false;
        cx.notify();
    }

    fn model_list(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.model_menu {
            return None;
        }
        let current = self.model_label();
        let rows = self.models.iter().enumerate().map(|(n, m)| {
            let value = m.value.clone();
            let on = m.label == current;
            div()
                .id(("model", n))
                .relative()
                .flex()
                .flex_col()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                .when(on, |d| d.bg(theme::accent_soft()))
                .child(div().text_color(c(theme::TEXT)).child(m.label.clone()))
                .when(!m.description.is_empty(), |d| d.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(m.description.clone())))
                .child(probe(format!("model:{n}")))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_model(value.clone(), cx)))
        });
        Some(
            div()
                .id("model-list")
                .absolute()
                .bottom_full()
                .left_6()
                .w(px(320.))
                .max_h(px(420.))
                .overflow_y_scroll()
                .mb_1()
                .p_1()
                .rounded(px(8.))
                .border_1()
                .border_color(c(theme::BORDER_STRONG))
                .bg(c(theme::PANEL_RAISED))
                .occlude()
                .children(rows),
        )
    }

    fn empty_state(&self) -> impl IntoElement {
        let (name, color) = brand(&self.agent);
        let tip = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded_full()
                .border_1()
                .border_color(c(theme::BORDER))
                .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(key))
                .child(what)
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .child(div().size(px(44.)).rounded_full().flex().items_center().justify_center().bg(crate::pages::kit::alpha(color, 0.16)).child(crate::pages::kit::icon("spark", 22., c(color))))
            .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(format!("Chat with {name}")))
            .child(div().text_color(c(theme::TEXT_FAINT)).child(self.cwd.display().to_string()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .pt_2()
                    .text_size(px(11.5))
                    .text_color(c(theme::TEXT_DIM))
                    .child(tip("/", "commands"))
                    .child(tip("⌃V", "paste images or long text"))
                    .child(tip("⇧⏎", "new line"))
                    .child(tip("Esc", "stop a turn")),
            )
    }

    fn working(&self) -> Option<impl IntoElement> {
        let started = self.turn?;
        if self.pending().is_some() {
            return None;
        }
        let (_, color) = brand(&self.agent);
        let s = started.elapsed().as_secs();
        let took = if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) };
        Some(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_W))
                    .px_6()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .child(crate::motion::status_pulse("working", 8., c(color), std::time::Duration::from_millis(1200)))
                    .child(crate::motion::blink(div().child("Working"), "working-text", std::time::Duration::from_millis(2000), 0.55))
                    .child(div().text_color(c(theme::TEXT_FAINT)).child(format!("· {took} · Esc to stop"))),
            ),
        )
    }

    /// Moves queued message `i` to the front and stops the turn, whose end sends it.
    fn send_now(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.queue.len() {
            return;
        }
        let q = self.queue.remove(i);
        self.queue.insert(0, q);
        self.prompt.update(cx, |p, cx| p.focus(window, cx));
        if self.turn.is_some() {
            self.interrupt(cx);
        } else {
            let next = self.queue.remove(0);
            self.dispatch(next, cx);
        }
    }

    /// The messages waiting for the turn to end, oldest first, each removable.
    fn queued_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.queue.iter().enumerate().map(|(i, q)| {
            let images = q.attached.len();
            let text = if q.text.is_empty() { format!("{images} image{}", if images == 1 { "" } else { "s" }) } else { q.text.clone() };
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .rounded(px(8.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::PANEL))
                .text_size(px(12.5))
                .child(div().text_color(c(theme::TEXT_FAINT)).child("Queued"))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT)).child(text.replace('\n', " ")))
                .when(images > 0 && !q.text.is_empty(), |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child(format!("+{images} image{}", if images == 1 { "" } else { "s" }))))
                .child(
                    crate::pages::kit::mini(format!("send-now:{i}"), "Send now", crate::pages::kit::Kind::Secondary)
                        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Stop the current turn and send this one").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| this.send_now(i, window, cx))),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("unqueue:{i}")))
                        .relative()
                        .p_0p5()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                        .child(crate::pages::kit::icon("x", 12., c(theme::TEXT_DIM)))
                        .child(probe(format!("unqueue:{i}")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if i < this.queue.len() {
                                this.queue.remove(i);
                            }
                            this.prompt.update(cx, |p, cx| p.focus(window, cx));
                            cx.notify();
                        })),
                )
        });
        div().flex().flex_col().gap_1().pb_1p5().children(rows)
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let modes: Vec<(String, &str)> = self.modes.iter().map(|m| (m.value.clone(), m.label.as_str())).collect();
        let current = self.mode.clone().unwrap_or_default();
        let mode = (modes.len() > 1).then(|| {
            crate::pages::kit::seg("chat-mode", &modes, &current, move |m, _, cx| {
                let _ = this.update(cx, |v, cx| v.set_mode(m, cx));
            })
        });
        let tokens = crate::shell::tokens;
        let spent = (self.spent.0 > 0).then(|| format!("{} tokens · ${:.2}", tokens(self.spent.0), self.spent.1));
        let (agent_name, _) = brand(&self.agent);
        let limit = |n: usize, l: &orbit_core::chat::Limit| {
            let pct = l.used_percent.clamp(0., 100.);
            let tone = if pct >= 90. { theme::DANGER } else if pct >= 80. { theme::WARN } else { theme::TEXT_FAINT };
            let resets = l.resets_at.map(|at| format!(" · resets {}", crate::shell::resets_in(at))).unwrap_or_default();
            let tip = format!("{agent_name} {} limit: {pct:.0}% used{resets}", l.label);
            let label = l.label.clone();
            div()
                .id(("limit", n))
                .flex()
                .items_center()
                .gap_1()
                .text_color(c(tone))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child(label)
                .child(div().w(px(28.)).h(px(4.)).rounded(px(2.)).bg(c(theme::BORDER_STRONG)).overflow_hidden().child(div().h_full().w(px(28. * pct as f32 / 100.)).bg(c(if pct >= 80. { tone } else { theme::ACCENT }))))
                .child(format!("{pct:.0}%"))
        };
        let model = (!self.models.is_empty()).then(|| {
            div()
                .id("chat-model")
                .relative()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(24.))
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::BORDER))
                .cursor_pointer()
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(self.model_label())
                .child(crate::pages::kit::icon("chevron", 10., c(theme::TEXT_FAINT)))
                .child(probe("chat-model"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model_menu = !this.model_menu;
                    cx.notify();
                }))
        });
        let context = self.context.map(|(used, window)| {
            let tip = match window {
                Some(w) => format!("Context: {} of {} tokens", tokens(used), tokens(w)),
                None => format!("Context: {} tokens", tokens(used)),
            };
            let pct = window.map(|w| (used as f64 / w.max(1) as f64 * 100.).clamp(0., 100.));
            let tone = match pct {
                Some(p) if p >= 90. => theme::DANGER,
                Some(p) if p >= 75. => theme::WARN,
                _ => theme::TEXT_FAINT,
            };
            div()
                .id("chat-context")
                .flex()
                .items_center()
                .gap_1()
                .text_color(c(tone))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child("Context")
                .children(pct.map(|p| div().w(px(28.)).h(px(4.)).rounded(px(2.)).bg(c(theme::BORDER_STRONG)).overflow_hidden().child(div().h_full().w(px(28. * p as f32 / 100.)).bg(c(if p >= 75. { tone } else { theme::ACCENT })))))
                .child(match pct {
                    Some(p) => format!("{p:.0}%"),
                    None => tokens(used),
                })
        });
        div()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(11.))
            .text_color(c(theme::TEXT_FAINT))
            .children(mode)
            .children(model)
            .child(div().flex_1())
            .children(context)
            .children(spent)
            .children((self.agent == "claude" && !self.limits.is_empty() && !self.limits.iter().any(|l| l.label == "5h")).then(|| {
                div()
                    .id("limit-5h-unknown")
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Claude Code 5h limit: updates with your next message").build(window, cx))
                    .child("5h —")
            }))
            .children(self.limits.iter().enumerate().map(|(n, l)| limit(n, l)))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.ready();
        let (_, color) = brand(&self.agent);
        let can_send = ready && (!self.prompt.read(cx).value().trim().is_empty() || !self.attached.is_empty() || !self.pasted.is_empty());
        let action = if self.turn.is_some() {
            div()
                .id("chat-stop")
                .relative()
                .size(px(30.))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::danger_soft())
                .cursor_pointer()
                .child(crate::pages::kit::icon("stop", 12., c(theme::DANGER)))
                .child(probe("chat-stop"))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx)))
        } else {
            div()
                .id("chat-send")
                .relative()
                .size(px(30.))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(c(if can_send { color } else { theme::BORDER_STRONG }))
                .cursor_pointer()
                .child(crate::pages::kit::icon("arrow-up", 15., c(if can_send { 0xFFFFFF } else { theme::TEXT_FAINT })))
                .child(probe("chat-send"))
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };
        let focused = gpui::Focusable::focus_handle(self.prompt.read(cx), cx).is_focused(window);
        let this = cx.weak_entity();
        let composer = div()
            .w_full()
            .max_w(px(COLUMN_W))
            .px_6()
            .relative()
            .child(self.slash_list(cx))
            .children(self.model_list(cx))
            .when(!self.queue.is_empty(), |d| d.child(self.queued_list(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2p5()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(if focused { crate::pages::kit::alpha(color, 0.7) } else { c(theme::BORDER_STRONG) })
                    .bg(c(theme::PANEL_RAISED))
                    .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| this.attach(paths.paths().to_vec(), vec![], window, cx)))
                    .when(!self.attached.is_empty() || !self.pasted.is_empty(), |d| d.child(self.attachments(cx)))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .relative()
                                    .child(Textarea::new(&self.prompt).appearance(false).disabled(!ready).on_paste(move |item, window, cx| paste_into(&this, item, window, cx)))
                                    .child(probe("chat-prompt")),
                            )
                            .child(action),
                    )
                    .child(self.toolbar(cx)),
            );
        let body = if self.items.is_empty() {
            self.empty_state().into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(list(self.list.clone(), cx.processor(|this, ix, _, cx| this.render_item(ix, cx))).size_full().pt_3().pb_2())
                .child(crate::scrollbar::vertical("chat", &self.list))
                .into_any_element()
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .capture_key_down(cx.listener(Self::on_key))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.on_slash_key("up", window, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.on_slash_key("down", window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| this.on_slash_key("tab", window, cx)))
            .capture_action(cx.listener(|this, _: &Enter, window, cx| this.on_slash_key("enter", window, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.lightbox.take().is_some() || std::mem::take(&mut this.model_menu) {
                    cx.stop_propagation();
                    return cx.notify();
                }
                this.on_slash_key("escape", window, cx)
            }))
            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveLeft, _, cx| {
                if this.lightbox.is_some() {
                    this.step_lightbox(-1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &gpui_component::input::MoveRight, _, cx| {
                if this.lightbox.is_some() {
                    this.step_lightbox(1, cx);
                    cx.stop_propagation();
                }
            }))
            .child(body)
            .children(self.working())
            .child(div().flex_none().w_full().flex().justify_center().pt_1().pb_3().child(composer))
            .children(self.lightbox.and_then(|ix| self.render_lightbox(ix, cx)))
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

    fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "  chat {} session {:?} ready {} status {} opened {:?}", self.agent, self.session_id, self.ready(), self.status().label(), self.opened);
        if !self.queue.is_empty() {
            let _ = writeln!(out, "    queued: {:?}", self.queue.iter().map(|q| q.text.as_str()).collect::<Vec<_>>());
        }
        for (ix, item) in self.items.iter().enumerate() {
            let _ = match item {
                Item::User { text } => writeln!(out, "    you: {text:?}"),
                Item::Assistant { text, .. } => writeln!(out, "    agent: {text:?}"),
                Item::Thinking { text, open, .. } => writeln!(out, "    thinking ({}): {text:?}", if *open { "open" } else { "collapsed" }),
                Item::Image { path, title, size, user } => writeln!(out, "    image{}: {} {title:?} {size:?}", if *user { " (yours)" } else { "" }, path.display()),
                Item::Note(t) => writeln!(out, "    note: {t:?}"),
                Item::Exited(t) => writeln!(out, "    exited: {t:?}"),
                Item::Footer(t) => writeln!(out, "    footer: {t:?}"),
                Item::Permission { id, title, options, answer, .. } => {
                    let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                    let ticked = self.ticked.get(id).map(|t| format!(" ticked {t:?}")).unwrap_or_default();
                    let typing = self.other.as_ref().filter(|o| o.0 == ix).map(|o| format!(" typing {:?}", o.1.read(cx).value())).unwrap_or_default();
                    writeln!(out, "    permission {title:?} {labels:?} answer {answer:?}{ticked}{typing}")
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
        for (label, path) in &self.attached {
            let _ = writeln!(out, "    attached: {label} {}", path.display());
        }
        for (label, _) in &self.pasted {
            let _ = writeln!(out, "    pasted: {label}");
        }
        if let Some(ix) = self.lightbox {
            let _ = writeln!(out, "    lightbox: {ix}");
        }
        let _ = writeln!(out, "    commands: {}", self.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(" "));
    }
}

/// Takes a pasted image (saved to a temp file), copied image files, or a long text as an attachment;
/// short text pastes go through.
fn paste_into(view: &gpui::WeakEntity<ChatView>, item: &gpui::ClipboardItem, window: &mut Window, cx: &mut App) -> bool {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let mut paths = vec![];
    let mut texts = vec![];
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
            gpui::ClipboardEntry::String(s) if is_long(&s.text) => texts.push(s.text.clone()),
            gpui::ClipboardEntry::String(_) => {}
        }
    }
    if paths.is_empty() && texts.is_empty() {
        return false;
    }
    // After the paste returns: the prompt is still mid-update while its handler runs.
    let view = view.clone();
    window.defer(cx, move |window, cx| {
        let _ = view.update(cx, |v, cx| v.attach(paths, texts, window, cx));
    });
    true
}

/// Claude's plan limits from its status line file; a window that has reset since is left out (unknown).
fn claude_limits(r: orbit_core::agent_hooks::RateLimits) -> Vec<orbit_core::chat::Limit> {
    let limit = |label: &str, w: &orbit_core::agent_hooks::RateWindow| orbit_core::chat::Limit { label: label.into(), used_percent: w.used_percentage, resets_at: Some(w.resets_at) };
    r.five_hour.iter().map(|w| limit("5h", w)).chain(r.seven_day.iter().map(|w| limit("Week", w))).collect()
}

/// The prompt as sent: each paste's reference replaced by its text (or the text added at the end
/// when the reference was deleted).
fn expand_pastes(text: &str, pasted: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (label, content) in pasted {
        if out.contains(label.as_str()) {
            out = out.replacen(label.as_str(), &format!("\n{content}\n"), 1);
        } else {
            out = format!("{out}\n\n{content}");
        }
    }
    out.trim().to_string()
}

fn is_long(text: &str) -> bool {
    text.lines().count() > PASTE_LINES || text.len() > PASTE_CHARS
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn paste_chip(label: String) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL))
        .text_size(px(11.5))
        .text_color(c(theme::TEXT_DIM))
        .child(crate::pages::kit::icon("doc", 12., c(theme::TEXT_DIM)))
        .child(label)
}

/// Byte offset of the `/word` being typed at the end of the prompt, at its start or after whitespace.
/// shortcut: only the word at the end of the text, not at the cursor; upgrade if mid-text completion is wanted.
fn slash_start(text: &str) -> Option<usize> {
    let start = text.rfind(char::is_whitespace).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    (text[start..].starts_with('/') && !text[start + 1..].contains('/')).then_some(start)
}

/// Commands matching the `/word` being typed: name prefix matches first, then names containing it.
fn filter_commands(commands: &[SlashCommand], text: &str) -> Vec<usize> {
    let Some(start) = slash_start(text) else { return vec![] };
    let q = &text[start + 1..];
    let q = q.to_lowercase();
    let (mut prefix, mut inner): (Vec<usize>, Vec<usize>) = (vec![], vec![]);
    for (i, cmd) in commands.iter().enumerate() {
        let name = cmd.name.to_lowercase();
        if name.starts_with(&q) {
            prefix.push(i);
        } else if name.contains(&q) {
            inner.push(i);
        }
    }
    prefix.extend(inner);
    prefix
}

/// The first file under `root` whose path ends with `rel`, for mentions like `Foo.cpp:36` that leave out the folders.
fn find_under(root: &Path, rel: &Path) -> Option<PathBuf> {
    ignore::WalkBuilder::new(root).build().flatten().map(|e| e.into_path()).find(|p| p.ends_with(rel) && p.is_file())
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
    // A chat tab outside the session list, for self-tests; sessions switch with Terminal | Chat.
    commands::register(cx, Command::new("chat.newClaude", "New Claude Chat").run(|a, w, cx| open(a, "claude", w, cx)));
}

#[cfg(test)]
mod tests {
    use super::{file_patch, filter_commands, footer, group_label, linkify, Call};
    use orbit_core::chat::{FileDiff, SlashCommand};

    #[test]
    fn slash_list_filters_a_bare_command_word() {
        let cmds: Vec<SlashCommand> = ["compact", "review", "pr-comments", "init"].iter().map(|n| SlashCommand { name: (*n).into(), description: String::new() }).collect();
        assert_eq!(filter_commands(&cmds, "/"), vec![0, 1, 2, 3]);
        assert_eq!(filter_commands(&cmds, "/CO"), vec![0, 2]);
        assert_eq!(filter_commands(&cmds, "/re"), vec![1]);
        assert!(filter_commands(&cmds, "/review now").is_empty());
        assert_eq!(filter_commands(&cmds, "hi /re"), vec![1]);
        assert_eq!(filter_commands(&cmds, "a\nb /in"), vec![3]);
        assert!(filter_commands(&cmds, "hi/re").is_empty());
        assert!(filter_commands(&cmds, "see /usr/re").is_empty());
    }

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
    fn pastes_expand_where_their_reference_is() {
        let pasted = vec![("[Pasted text #1 · 2 lines]".to_string(), "a\nb".to_string()), ("[Pasted text #2 · 1 lines]".to_string(), "gone".to_string())];
        assert_eq!(super::expand_pastes("see [Pasted text #1 · 2 lines] then", &pasted), "see \na\nb\n then\n\ngone");
    }

    #[test]
    fn footer_leaves_out_unknowns() {
        let d = std::time::Duration::from_millis(12_340);
        assert_eq!(footer(Some(d), Some(1_234_567), Some(0.012)), "12.3s · 1,234,567 tokens · $0.0120");
        assert_eq!(footer(Some(std::time::Duration::from_secs(125)), Some(42), None), "2m 05s · 42 tokens");
        assert_eq!(footer(None, None, None), "");
    }

    #[test]
    fn tool_runs_are_labelled_by_kind() {
        let call = |name: &str| Call { id: String::new(), name: name.into(), summary: String::new(), output: String::new(), ok: None };
        assert_eq!(group_label(&[call("Read"), call("Read")]), "Read 2 files");
        assert_eq!(group_label(&[call("Read"), call("Grep")]), "Read 1 file, searched 1 time");
        assert_eq!(group_label(&[call("Glob"), call("Grep")]), "Searched 2 times");
        assert_eq!(group_label(&[call("Bash"), call("Skill"), call("Bash"), call("Read")]), "Ran 2 commands, read 1 file, used 1 tool");
    }

    #[test]
    fn bare_file_names_are_found_under_the_folder() {
        let dir = std::env::temp_dir().join(format!("orbit-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("repo/src/game")).unwrap();
        std::fs::write(dir.join("repo/src/game/Foo.cpp"), "").unwrap();
        assert_eq!(super::find_under(&dir, std::path::Path::new("Foo.cpp")), Some(dir.join("repo/src/game/Foo.cpp")));
        assert_eq!(super::find_under(&dir, std::path::Path::new("game/Foo.cpp")), Some(dir.join("repo/src/game/Foo.cpp")));
        assert_eq!(super::find_under(&dir, std::path::Path::new("Bar.cpp")), None);
        let _ = std::fs::remove_dir_all(dir);
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
