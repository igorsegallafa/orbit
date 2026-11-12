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
use gpui_component::text::{TextView, TextViewState};
use orbit_core::chat::{ChatBackend, ChatEvent, ChatInput};
use regex::Regex;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::LazyLock;

enum Item {
    User(String),
    /// `md` renders `text` with `path:line` mentions turned into links.
    Assistant { id: String, text: String, md: Entity<TextViewState>, _parsed: Subscription },
    Thinking { id: String, text: String, open: bool },
    /// `size` is the drawn size, at most `MAX_IMAGE_H` tall.
    Image { path: PathBuf, title: String, size: Option<(f32, f32)> },
    /// Errors and the session's end.
    Note(String),
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
        let backend = orbit_core::chat::start(agent, &cwd.clone(), resume, move |e| {
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
            cwd,
            opened: None,
            backend: None,
            session_id: None,
            items: vec![],
            list,
            prompt,
            attached: vec![],
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
            ChatEvent::TurnEnded { error: Some(e), .. } => self.push(Item::Note(e)),
            ChatEvent::Image { path, title } => {
                let size = image_size(&path);
                self.push(Item::Image { path, title, size });
            }
            ChatEvent::Exited { error } => {
                self.backend = None;
                self.push(Item::Note(error.map_or("The session ended.".into(), |e| format!("The session ended: {e}"))));
            }
            _ => {}
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt.read(cx).value().trim().to_string();
        if (text.is_empty() && self.attached.is_empty()) || !self.ready() {
            return;
        }
        let images = std::mem::take(&mut self.attached);
        if let Some(b) = &self.backend {
            b.send(ChatInput::Prompt { text: text.clone(), images: images.clone() });
        }
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
        }
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
        let ready = self.ready();
        let this = cx.weak_entity();
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
                    .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| this.attach(paths.paths().to_vec(), cx)))
                    .when(!self.attached.is_empty(), |d| d.child(self.attachments(cx)))
                    .child(div().relative().child(Textarea::new(&self.prompt).disabled(!ready).on_paste(move |item, _, cx| {
                        paste_images(&this, item, cx)
                    })).child(probe("chat-prompt"))),
            )
    }
}

impl PageView for ChatView {
    fn title(&self) -> SharedString {
        format!("{} (chat)", self.agent).into()
    }

    fn dump(&self, out: &mut String, _: &App) {
        let _ = writeln!(out, "  chat {} session {:?} ready {} opened {:?}", self.agent, self.session_id, self.ready(), self.opened);
        for item in &self.items {
            let _ = match item {
                Item::User(t) => writeln!(out, "    you: {t:?}"),
                Item::Assistant { text, .. } => writeln!(out, "    agent: {text:?}"),
                Item::Thinking { text, open, .. } => writeln!(out, "    thinking ({}): {text:?}", if *open { "open" } else { "collapsed" }),
                Item::Image { path, title, size } => writeln!(out, "    image: {} {title:?} {size:?}", path.display()),
                Item::Note(t) => writeln!(out, "    note: {t:?}"),
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

#[cfg(test)]
mod tests {
    use super::linkify;

    #[test]
    fn links_file_lines_outside_code() {
        assert_eq!(linkify("see src/a.rs:42 now"), "see [src/a.rs:42](src/a.rs#L42) now");
        assert_eq!(linkify("in `src/a.rs:4:2`."), "in [`src/a.rs:4:2`](src/a.rs#L4).");
        assert_eq!(linkify("`let x = a.rs:1` and `b`"), "`let x = a.rs:1` and `b`");
        assert_eq!(linkify("```\nsrc/a.rs:1\n```\nb.rs:2"), "```\nsrc/a.rs:1\n```\n[b.rs:2](b.rs#L2)");
        assert_eq!(linkify("http://host.com:80 and [x](a.rs:3) and `a.rs:5"), "http://host.com:80 and [x](a.rs:3) and `a.rs:5");
    }
}
