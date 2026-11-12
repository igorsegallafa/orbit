//! Chat sessions: Orbit renders an agent's conversation itself from
//! `orbit_core::chat` events (user bubbles, streamed replies, thinking rows)
//! with a prompt box at the bottom. Deltas are applied in batches: every
//! event that arrived while the UI thread was busy lands in one update.
use crate::app::OrbitApp;
use crate::commands::{self, Command};
use crate::pages::{self, Host, PageView};
use crate::probe::probe;
use crate::theme::{self, c};
use futures::StreamExt as _;
use gpui::{
    div, list, prelude::*, px, App, Context, Entity, FollowMode, ListAlignment, ListState, SharedString, Subscription,
    Task, Window,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use orbit_core::chat::{ChatBackend, ChatEvent, ChatInput};
use std::fmt::Write as _;
use std::path::PathBuf;

enum Item {
    User(String),
    Assistant { id: String, text: String },
    Thinking { id: String, text: String, open: bool },
    /// Errors and the session's end.
    Note(String),
}

pub struct ChatView {
    agent: String,
    backend: Option<Box<dyn ChatBackend>>,
    session_id: Option<String>,
    items: Vec<Item>,
    list: ListState,
    prompt: Entity<TextareaState>,
    _events: Task<()>,
    _prompt: Subscription,
}

impl ChatView {
    pub fn new(agent: &str, cwd: PathBuf, resume: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Message the agent…"));
        let sub = cx.subscribe_in(&prompt, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { shift: false, .. }) {
                this.send(window, cx);
            }
        });
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let backend = orbit_core::chat::start(agent, &cwd, resume, move |e| {
            let _ = tx.unbounded_send(e);
        });
        let events = cx.spawn_in(window, async move |this, cx| {
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
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let mut view = Self {
            agent: agent.into(),
            backend: None,
            session_id: None,
            items: vec![],
            list,
            prompt,
            _events: events,
            _prompt: sub,
        };
        match backend {
            Ok(b) => view.backend = Some(b),
            Err(e) => view.push(Item::Note(e)),
        }
        view.prompt.update(cx, |p, cx| p.focus(window, cx));
        view
    }

    /// Claude takes a prompt as soon as it runs and reports its id only after the first one;
    /// the JSON-RPC agents need their handshake (which ends with the id) first.
    fn ready(&self) -> bool {
        self.backend.is_some() && (self.session_id.is_some() || self.agent == "claude")
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        let n = self.items.len();
        self.list.splice(n - 1..n - 1, 1);
    }

    /// Appends to the current turn's reply (or thinking) with this id, or starts one.
    fn append(&mut self, thinking: bool, id: String, delta: &str) {
        let at = self.items.iter().rposition(|i| match i {
            Item::Assistant { id: x, .. } if !thinking => *x == id,
            Item::Thinking { id: x, .. } if thinking => *x == id,
            _ => false,
        });
        let turn_start = self.items.iter().rposition(|i| matches!(i, Item::User(_)));
        match at.filter(|&ix| turn_start.is_none_or(|t| ix > t)) {
            Some(ix) => {
                if let Item::Assistant { text, .. } | Item::Thinking { text, .. } = &mut self.items[ix] {
                    text.push_str(delta);
                }
                self.list.splice(ix..ix + 1, 1);
            }
            None if thinking => self.push(Item::Thinking { id, text: delta.into(), open: false }),
            None => self.push(Item::Assistant { id, text: delta.into() }),
        }
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
            ChatEvent::TextDelta { message_id, text } => self.append(false, message_id, &text),
            ChatEvent::ThinkingDelta { message_id, text } => self.append(true, message_id, &text),
            ChatEvent::TurnEnded { error: Some(e), .. } => self.push(Item::Note(e)),
            ChatEvent::Exited { error } => {
                self.backend = None;
                self.push(Item::Note(error.map_or("The session ended.".into(), |e| format!("The session ended: {e}"))));
            }
            _ => {}
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).value().trim().to_string();
        if text.is_empty() || !self.ready() {
            return;
        }
        if let Some(b) = &self.backend {
            b.send(ChatInput::Prompt { text: text.clone(), images: vec![] });
        }
        self.push(Item::User(text));
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
            Item::Assistant { text, .. } => row.text_color(c(theme::TEXT)).child(div().w_full().child(text.clone())).into_any_element(),
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
                        this.list.splice(ix..ix + 1, 1);
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Item::Note(text) => row.text_color(c(theme::DANGER)).child(text.clone()).into_any_element(),
        }
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
        let ready = self.ready();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(list(self.list.clone(), cx.processor(|this, ix, _, cx| this.render_item(ix, cx))).flex_1().py_2())
            .child(
                div()
                    .relative()
                    .p_3()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .child(Textarea::new(&self.prompt).disabled(!ready))
                    .child(probe("chat-prompt")),
            )
    }
}

impl PageView for ChatView {
    fn title(&self) -> SharedString {
        format!("{} (chat)", self.agent).into()
    }

    fn dump(&self, out: &mut String, _: &App) {
        let _ = writeln!(out, "  chat {} session {:?} ready {}", self.agent, self.session_id, self.ready());
        for item in &self.items {
            let _ = match item {
                Item::User(t) => writeln!(out, "    you: {t:?}"),
                Item::Assistant { text, .. } => writeln!(out, "    agent: {text:?}"),
                Item::Thinking { text, open, .. } => writeln!(out, "    thinking ({}): {text:?}", if *open { "open" } else { "collapsed" }),
                Item::Note(t) => writeln!(out, "    note: {t:?}"),
            };
        }
    }
}

/// Opens a new chat tab on the selected workspace's folder (or Orbit's own).
fn open(app: &mut OrbitApp, agent: &'static str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cwd = app.workspace().map(|w| w.dir.clone()).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    pages::open(app, format!("chat:{n}"), move |_: Host, window, cx| ChatView::new(agent, cwd, None, window, cx), window, cx);
}

pub fn init(cx: &mut App) {
    // shortcut: a palette entry for testing, until the New Session menu lists chat sessions.
    commands::register(cx, Command::new("chat.newClaude", "New Claude Chat").run(|a, w, cx| open(a, "claude", w, cx)));
}
