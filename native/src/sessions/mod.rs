//! Sessions: terminal tabs running a shell or an agent (claude, opencode,
//! omp), as the React app has them. Each has a stable key (`tm:<ms>-<rand>`)
//! that names its scrollback file and routes Claude Code's hooks back to it.
//! Open sessions and ended, resumable ones are saved to
//! `<config>/native-sessions.json`; a restart reopens them (claude resumes its
//! conversation, the rest replay their saved output). Agents that finish a
//! turn or need the user land in the inbox.
//!
//! The actions below are the ids a command palette or keymap can bind.
pub mod rename;
pub mod status;

use crate::app::{OrbitApp, Tab, TabKind};
use crate::terminal::{self, Signal, TerminalEvent, TerminalView};
use futures::StreamExt;
use gpui::{actions, AnyWindowHandle, App, AppContext, Context, Entity, Global, Subscription, WeakEntity, Window};
use orbit_core::pty;
use serde::{Deserialize, Serialize};
use status::Status;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

actions!(
    sessions,
    [
        /// A session of the agent set in Settings → AI.
        NewAgentSession,
        NewClaudeSession,
        NewOpencodeSession,
        NewOmpSession,
        NewCodexSession,
        NewShellSession,
        /// Reopens the most recently ended agent session of the current scope.
        ResumeLastSession,
        /// Cycles through the sessions waiting for the user.
        NextSessionNeedingInput,
        MarkInboxRead,
        ClearInbox,
    ]
);

/// What a new session runs: (menu label, tab label, command; none is the shell).
pub const AGENTS: [(&str, &str, Option<&str>); 5] = [
    ("Terminal", "shell", None),
    ("Claude Code", "claude", Some("claude")),
    ("Open Code", "opencode", Some("opencode")),
    ("OMP", "omp", Some("omp")),
    ("Codex", "codex", Some("codex")),
];

/// One session tab, as saved (field names follow the React app's `TerminalTab`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    /// Workspace name or scope (`dir:<path>`, `@repo`).
    pub workspace: String,
    /// What was launched: "shell" | "claude" | "opencode" | "omp" | "codex".
    pub label: String,
    /// What the session is called: its codename, or the name the user gave it.
    pub session_name: String,
    /// The generated codename while a user name is set (a cleared name restores it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// None: the user's shell.
    pub cmd: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Claude conversation id (`--session-id`), so a restart can `--resume` it.
    #[serde(default)]
    pub agent_session_id: Option<String>,
    /// A folder to start in instead of the repo or scope folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// An ended agent session that can be picked up again.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecentSession {
    pub resume_id: String,
    /// "claude" | "opencode"
    pub agent: String,
    pub workspace: String,
    #[serde(default)]
    pub repo: Option<String>,
    pub session_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    pub ended_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InboxKind {
    Done,
    Waiting,
}

#[derive(Clone, Debug)]
pub struct InboxItem {
    pub key: String,
    pub kind: InboxKind,
    pub title: String,
    pub detail: String,
    pub at: u64,
    pub read: bool,
}

const RECENT_MAX: usize = 30;
const INBOX_MAX: usize = 50;

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    sessions: Vec<SessionMeta>,
    #[serde(default)]
    recent: Vec<RecentSession>,
}

pub struct SessionStore {
    /// Open sessions, oldest first.
    open: Vec<(SessionMeta, WeakEntity<TerminalView>)>,
    pub inbox: Vec<InboxItem>,
    pub recent: Vec<RecentSession>,
    /// Off in self-test runs on the user's own config: nothing is saved or restored there.
    persist: bool,
    subscriptions: HashMap<String, Subscription>,
    _app_observers: Vec<Subscription>,
}

struct Store {
    store: Entity<SessionStore>,
    app: WeakEntity<OrbitApp>,
    window: AnyWindowHandle,
}

impl Global for Store {}

pub fn store(cx: &App) -> Entity<SessionStore> {
    cx.global::<Store>().store.clone()
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// `tm:<ms>-<5 base-36 chars>`, like the React app's tab ids.
fn new_key() -> String {
    let mut r = random();
    let suffix: String = (0..5)
        .map(|_| {
            let d = (r % 36) as u32;
            r /= 36;
            char::from_digit(d, 36).unwrap()
        })
        .collect();
    format!("tm:{}-{suffix}", now_ms())
}

/// A random (v4) UUID for claude's `--session-id`.
fn new_uuid() -> String {
    let (a, b) = (random(), random().rotate_left(17) ^ 0x5bd1_e995);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

const ADJECTIVES: [&str; 20] = [
    "amber", "bold", "calm", "daring", "eager", "fierce", "gentle", "jolly", "keen", "lunar", "nimble", "quiet", "rapid", "solar",
    "swift", "tidal", "ultra", "vivid", "witty", "zesty",
];
const NOUNS: [&str; 20] = [
    "comet", "falcon", "lynx", "meteor", "nebula", "nova", "otter", "pegasus", "phoenix", "quasar", "quokka", "raven", "rocket",
    "sparrow", "sputnik", "tiger", "voyager", "panda", "orion", "magnetar",
];

/// Docker-style codename not among `taken` (`src/lib/names.ts`).
pub fn codename<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let used: Vec<&str> = taken.into_iter().collect();
    for _ in 0..50 {
        let r = random();
        let name = format!("{}-{}", ADJECTIVES[(r % 20) as usize], NOUNS[((r >> 8) % 20) as usize]);
        if !used.contains(&name.as_str()) {
            return name;
        }
    }
    format!("session-{}", now_ms() % 10000)
}

/// Names the session `name`, keeping its codename aside; empty (or the
/// codename itself) restores the codename.
fn set_name(meta: &mut SessionMeta, name: &str) {
    let codename = meta.codename.take().unwrap_or_else(|| meta.session_name.clone());
    if name.is_empty() || name == codename {
        meta.session_name = codename;
    } else {
        meta.session_name = name.to_string();
        meta.codename = Some(codename);
    }
}

/// Human label for a scope (`scopeLabel` in `src/lib/folderScope.ts`).
pub fn scope_label(scope: &str) -> String {
    match scope.strip_prefix("dir:") {
        Some("~") => "Home".into(),
        Some(p) => p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(p).to_string(),
        None => scope.strip_prefix('@').unwrap_or(scope).to_string(),
    }
}

fn saved_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("native-sessions.json"))
}

impl SessionStore {
    fn save(&self) {
        if !self.persist {
            return;
        }
        let Some(path) = saved_path() else { return };
        let saved = Saved { sessions: self.open.iter().map(|(m, _)| m.clone()).collect(), recent: self.recent.clone() };
        if let Ok(json) = serde_json::to_string_pretty(&saved) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn view(&self, key: &str) -> Option<Entity<TerminalView>> {
        self.open.iter().find(|(m, _)| m.id == key).and_then(|(_, v)| v.upgrade())
    }

    pub fn sessions(&self) -> impl Iterator<Item = (&SessionMeta, Option<Entity<TerminalView>>)> {
        self.open.iter().map(|(m, v)| (m, v.upgrade()))
    }

    pub fn unread(&self, key: &str) -> Option<InboxKind> {
        self.inbox.iter().find(|i| i.key == key && !i.read).map(|i| i.kind)
    }

    fn settle(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.inbox.iter().any(|i| i.key == key && !i.read) {
            self.inbox.iter_mut().filter(|i| i.key == key).for_each(|i| i.read = true);
            cx.notify();
        }
    }

    /// One live item per session: older ones for it are settled.
    fn push_inbox(&mut self, item: InboxItem, cx: &mut Context<Self>) {
        self.inbox.iter_mut().filter(|i| i.key == item.key).for_each(|i| i.read = true);
        self.inbox.insert(0, item);
        self.inbox.truncate(INBOX_MAX);
        cx.notify();
    }

    /// An item from outside the sessions (a Ralph run ending); `key` is what a click opens.
    pub fn notify(&mut self, key: String, done: bool, title: String, detail: String, cx: &mut Context<Self>) {
        let kind = if done { InboxKind::Done } else { InboxKind::Waiting };
        crate::notifications::post(key.clone(), title.clone(), detail.clone(), cx);
        self.push_inbox(InboxItem { key, kind, title, detail, at: now_ms(), read: false }, cx);
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.inbox.iter_mut().for_each(|i| i.read = true);
        cx.notify();
    }

    pub fn clear_inbox(&mut self, cx: &mut Context<Self>) {
        self.inbox.clear();
        cx.notify();
    }

    /// Renames open session `key`; an empty name restores its codename.
    pub fn rename(&mut self, key: &str, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let name = name.trim();
        let Some(ix) = self.open.iter().position(|(m, _)| m.id == key) else { return Ok(()) };
        let ws = self.open[ix].0.workspace.clone();
        if !name.is_empty() && self.open.iter().any(|(m, _)| m.id != key && m.workspace == ws && m.session_name == name) {
            return Err(format!("a session named '{name}' already exists in this workspace"));
        }
        let (meta, view) = &mut self.open[ix];
        let old = meta.session_name.clone();
        set_name(meta, name);
        if meta.session_name == old {
            return Ok(());
        }
        let meta = meta.clone();
        if let Some(view) = view.upgrade() {
            view.update(cx, |v, cx| {
                v.meta = meta.clone();
                cx.notify();
            });
        }
        let prefix = format!("{old} ");
        for i in self.inbox.iter_mut().filter(|i| i.key == key) {
            if let Some(rest) = i.title.strip_prefix(&prefix) {
                i.title = format!("{} {rest}", meta.session_name);
            }
        }
        self.save();
        cx.notify();
        Ok(())
    }

    fn add_recent(&mut self, entry: RecentSession, cx: &mut Context<Self>) {
        self.recent.retain(|r| r.resume_id != entry.resume_id);
        self.recent.insert(0, entry);
        self.recent.truncate(RECENT_MAX);
        self.save();
        cx.notify();
    }
}

/// Sets up the session store, Claude Code's hook listener and the session
/// actions (the sidebar draws the sessions, `sidebar.rs`). Called once from `OrbitApp::new`.
pub fn init(window: &mut Window, cx: &mut Context<OrbitApp>) {
    // Self-test runs (which disable the clipboard) keep away from the user's
    // saved sessions unless they point ORBIT_CONFIG_DIR elsewhere.
    let persist = !terminal::CLIPBOARD_DISABLED.load(Ordering::Relaxed) || std::env::var_os("ORBIT_CONFIG_DIR").is_some();
    // Read now: sessions opened before the deferred restore are saved over it.
    let saved: Saved = if persist {
        saved_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    } else {
        Saved::default()
    };
    let store = cx.new(|_| SessionStore {
        open: vec![],
        inbox: vec![],
        recent: saved.recent,
        persist,
        subscriptions: HashMap::new(),
        _app_observers: vec![],
    });
    let app = cx.weak_entity();
    cx.set_global(Store { store: store.clone(), app, window: window.window_handle() });

    // Hook events arrive on the listener's threads; the UI routes them by key.
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<serde_json::Value>();
    orbit_core::agent_hooks::start(orbit_core::events::Events::new(move |event: &str, payload: serde_json::Value| {
        if event == "agent-event" {
            let _ = tx.unbounded_send(payload);
        }
    }));
    cx.spawn(async move |_, cx| {
        while let Some(ev) = rx.next().await {
            cx.update(|cx| on_agent_event(&ev, cx));
        }
    })
    .detach();

    register_actions(cx);
    cx.on_app_quit(|_, cx| {
        let s = self::store(cx).read(cx);
        // Nothing to restore from: drop what the sessions wrote.
        if !s.persist {
            for (m, _) in &s.open {
                pty::forget(&m.id);
            }
        }
        async {}
    })
    .detach();
    let to_restore = saved.sessions;
    cx.defer_in(window, move |a, w, cx| restore(a, to_restore, w, cx));
}

fn on_agent_event(ev: &serde_json::Value, cx: &mut App) {
    let s = |k: &str| ev.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(key), Some(event)) = (s("key"), s("event")) else { return };
    let Some(hook) = status::hook_of(&event, s("notificationType").as_deref()) else { return };
    let Some(view) = store(cx).read(cx).view(&key) else { return };
    view.update(cx, |v, cx| v.on_hook(hook, s("message"), cx));
}

/// Runs `f` on the app and its window after the current dispatch (global
/// action handlers run while the window is busy).
fn with_app(cx: &mut App, f: impl FnOnce(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static) {
    let Some(g) = cx.try_global::<Store>() else { return };
    let (app, window) = (g.app.clone(), g.window);
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| app.update(cx, |a, cx| f(a, window, cx)));
    });
}

fn register_actions(cx: &mut App) {
    rename::bind_keys(cx);
    cx.on_action(|_: &NewAgentSession, cx| {
        let bin = orbit_core::config::Config::load().map(|c| c.ai.agent_bin()).unwrap_or_else(|_| "claude".into());
        with_app(cx, move |a, w, cx| new_session(a, Some(&bin), None, w, cx));
    });
    cx.on_action(|_: &NewClaudeSession, cx| with_app(cx, |a, w, cx| new_session(a, Some("claude"), None, w, cx)));
    cx.on_action(|_: &NewOpencodeSession, cx| with_app(cx, |a, w, cx| new_session(a, Some("opencode"), None, w, cx)));
    cx.on_action(|_: &NewOmpSession, cx| with_app(cx, |a, w, cx| new_session(a, Some("omp"), None, w, cx)));
    cx.on_action(|_: &NewCodexSession, cx| with_app(cx, |a, w, cx| new_session(a, Some("codex"), None, w, cx)));
    cx.on_action(|_: &NewShellSession, cx| with_app(cx, |a, w, cx| new_session(a, None, None, w, cx)));
    cx.on_action(|_: &ResumeLastSession, cx| {
        with_app(cx, |a, w, cx| {
            let scope = scope(a);
            let last = store(cx).read(cx).recent.iter().find(|r| r.workspace == scope).cloned();
            match last {
                Some(r) => resume(a, &r, w, cx),
                None => a.toast(crate::toast::ToastKind::Info, "No ended session to resume here", None, cx),
            }
        })
    });
    cx.on_action(|_: &NextSessionNeedingInput, cx| with_app(cx, next_needing_input));
    cx.on_action(|_: &MarkInboxRead, cx| store(cx).update(cx, |s, cx| s.mark_all_read(cx)));
    cx.on_action(|_: &ClearInbox, cx| store(cx).update(cx, |s, cx| s.clear_inbox(cx)));
}

/// Where a new session runs: the selected workspace, else home.
pub fn scope(app: &OrbitApp) -> String {
    app.workspace().map_or_else(|| "dir:~".to_string(), |w| w.meta.name.clone())
}

/// A new session of `cmd` (none: the shell) in the current scope. `input` is
/// typed into it once it runs.
pub fn new_session(app: &mut OrbitApp, cmd: Option<&str>, input: Option<String>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let scope = scope(app);
    new_session_in(app, scope, cmd, input, window, cx);
}

/// A new session in `scope` (a workspace name, `@repo` or `dir:<path>`): the New Session menu's pick.
pub fn new_session_in(app: &mut OrbitApp, scope: String, cmd: Option<&str>, input: Option<String>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let label = AGENTS.iter().find(|a| a.2 == cmd).map_or_else(|| cmd.unwrap_or("shell").to_string(), |a| a.1.to_string());
    let taken: Vec<String> = store(cx).read(cx).open.iter().map(|(m, _)| m.session_name.clone()).collect();
    let agent_session_id = (cmd == Some("claude")).then(new_uuid);
    let meta = SessionMeta {
        id: new_key(),
        workspace: scope,
        label,
        session_name: codename(taken.iter().map(String::as_str)),
        codename: None,
        cmd: cmd.map(str::to_string),
        // A fixed id makes a claude session resumable.
        args: agent_session_id.iter().flat_map(|id| ["--session-id".to_string(), id.clone()]).collect(),
        repo: None,
        agent_session_id,
        cwd: None,
    };
    let args = meta.args.clone();
    open(app, meta, args, None, input, window, cx);
}

/// An agent session in workspace `ws` seeded with `prompt` (`openPromptedSession`
/// in App.tsx): claude, omp and codex take it as an argument, others get it typed in.
#[allow(clippy::too_many_arguments)]
pub fn new_prompted_session(
    app: &mut OrbitApp,
    ws: &str,
    agent: &str,
    model: &str,
    prompt: &str,
    name: Option<&str>,
    auto_edit: bool,
    window: &mut Window,
    cx: &mut Context<OrbitApp>,
) {
    let argv = matches!(agent, "claude" | "omp" | "codex");
    let agent_session_id = (agent == "claude").then(new_uuid);
    let mut args: Vec<String> = agent_session_id.iter().flat_map(|id| ["--session-id".to_string(), id.clone()]).collect();
    if !model.is_empty() {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if auto_edit && agent == "claude" {
        args.extend(["--permission-mode".to_string(), "acceptEdits".to_string()]);
    }
    if argv {
        args.push(prompt.to_string());
    }
    let taken: Vec<String> = store(cx).read(cx).open.iter().map(|(m, _)| m.session_name.clone()).collect();
    let meta = SessionMeta {
        id: new_key(),
        workspace: ws.to_string(),
        label: agent.to_string(),
        session_name: name.map_or_else(|| codename(taken.iter().map(String::as_str)), str::to_string),
        codename: None,
        cmd: Some(agent.to_string()),
        args: args.clone(),
        repo: None,
        agent_session_id,
        cwd: None,
    };
    open(app, meta, args, None, (!argv).then(|| prompt.to_string()), window, cx);
}

/// A shell session started in `dir` (the file tree's Open in Terminal).
pub fn new_shell_in(app: &mut OrbitApp, dir: &std::path::Path, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let taken: Vec<String> = store(cx).read(cx).open.iter().map(|(m, _)| m.session_name.clone()).collect();
    let meta = SessionMeta {
        id: new_key(),
        workspace: scope(app),
        label: "shell".into(),
        session_name: codename(taken.iter().map(String::as_str)),
        codename: None,
        cmd: None,
        args: vec![],
        repo: None,
        agent_session_id: None,
        cwd: Some(dir.to_string_lossy().to_string()),
    };
    open(app, meta, vec![], None, None, window, cx);
}

/// Opens `meta` as a tab running `args`, after replaying `replay`.
fn open(
    app: &mut OrbitApp,
    meta: SessionMeta,
    args: Vec<String>,
    replay: Option<Vec<u8>>,
    input: Option<String>,
    window: &mut Window,
    cx: &mut Context<OrbitApp>,
) -> Option<Entity<TerminalView>> {
    let cwd = match pty::session_dir(&meta.workspace, meta.repo.as_deref(), meta.cwd.as_deref()) {
        Ok(dir) => dir,
        Err(e) => {
            app.toast_error(format!("{}: {e}", meta.session_name), cx);
            return None;
        }
    };
    let persist = store(cx).read(cx).persist;
    let spec = pty::Spawn {
        cwd,
        cmd: meta.cmd.clone(),
        args,
        env: vec![],
        size: terminal::spawn_size(),
        key: Some(meta.id.clone()),
        scrollback: persist,
        program_version: env!("CARGO_PKG_VERSION").into(),
        tty_only: false,
    };
    let view = cx.new(|cx| TerminalView::new(meta.clone(), spec, replay, input, cx));
    cx.observe(&view, |_, _, cx| cx.notify()).detach();
    let store = store(cx);
    store.update(cx, |s, cx| {
        let key = meta.id.clone();
        let sub = cx.subscribe(&view, move |s: &mut SessionStore, view, ev: &TerminalEvent, cx| on_terminal_event(s, &view, ev, cx));
        s.subscriptions.insert(key, sub);
        s.open.push((meta, view.downgrade()));
        s.save();
        cx.notify();
    });
    let ix = app.insert_tab(Tab { kind: TabKind::Terminal(view.clone()), preview: false });
    app.activate(ix, window, cx);
    Some(view)
}

fn on_terminal_event(s: &mut SessionStore, view: &Entity<TerminalView>, ev: &TerminalEvent, cx: &mut Context<SessionStore>) {
    let TerminalEvent::Signal(sig) = ev else {
        cx.notify();
        return;
    };
    let meta = view.read(cx).meta.clone();
    let (app, window) = {
        let g = cx.global::<Store>();
        (g.app.clone(), g.window)
    };
    let active = app.upgrade().and_then(|a| a.read(cx).active_terminal()).is_some_and(|t| &t == view);
    let looking = active && window.update(cx, |_, w, _| w.is_window_active()).unwrap_or(false);
    let where_ = scope_label(&meta.workspace);
    let (kind, title, detail) = match sig {
        Signal::Done => (InboxKind::Done, format!("{} finished", meta.session_name), where_),
        Signal::Waiting(msg) => (
            InboxKind::Waiting,
            format!("{} needs your attention", meta.session_name),
            msg.as_ref().map_or(where_.clone(), |m| format!("{where_} · {m}")),
        ),
    };
    if !looking {
        crate::notifications::post(meta.id.clone(), title.clone(), detail.clone(), cx);
    }
    s.push_inbox(InboxItem { key: meta.id, kind, title, detail, at: now_ms(), read: looking }, cx);
}

/// Whether the window is focused, and whether inbox item `key`'s tab is the active one.
pub fn focus_of(key: &str, cx: &mut App) -> (bool, bool) {
    let Some(g) = cx.try_global::<Store>() else { return (false, false) };
    let (app, window) = (g.app.clone(), g.window);
    let window_active = window.update(cx, |_, w, _| w.is_window_active()).unwrap_or(false);
    let tab_active = app.upgrade().is_some_and(|a| {
        a.read(cx).active_tab().is_some_and(|t| t.saved_key(cx) == format!("session:{key}") || t.key(cx) == format!("page:{key}"))
    });
    (window_active, tab_active)
}

/// Brings the window forward on inbox item `key`'s tab (a notification click).
pub fn reveal(key: String, cx: &mut App) {
    with_app(cx, move |a, window, cx| {
        window.activate_window();
        activate(a, &key, window, cx);
    });
}

/// The tab of session `key` is gone for good: stop it, forget its saved
/// output, and keep claude/opencode conversations in Recent.
pub fn closed(view: &Entity<TerminalView>, cx: &mut App) {
    let meta = view.read(cx).meta.clone();
    pty::forget(&meta.id);
    let store = store(cx);
    store.update(cx, |s, cx| {
        s.open.retain(|(m, _)| m.id != meta.id);
        s.subscriptions.remove(&meta.id);
        s.inbox.retain(|i| i.key != meta.id);
        s.save();
        cx.notify();
    });
    let m = meta.clone();
    let recent = move |resume_id: String| RecentSession {
        resume_id,
        agent: m.cmd.clone().unwrap_or_default(),
        workspace: m.workspace.clone(),
        repo: m.repo.clone(),
        session_name: m.session_name.clone(),
        codename: m.codename.clone(),
        ended_at: now_ms(),
    };
    match meta.cmd.as_deref() {
        Some("claude") => {
            if let Some(id) = meta.agent_session_id.clone() {
                store.update(cx, |s, cx| s.add_recent(recent(id), cx));
            }
        }
        Some("opencode") => {
            // The key carries the launch time: the session opencode made after it.
            let since = meta.id.strip_prefix("tm:").and_then(|r| r.split('-').next()).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            let (ws, repo) = (meta.workspace.clone(), meta.repo.clone());
            let task = cx.background_spawn(async move { pty::opencode_session_id(&ws, repo.as_deref(), since) });
            cx.spawn(async move |cx| {
                if let Ok(Some(id)) = task.await {
                    store.update(cx, |s, cx| s.add_recent(recent(id), cx));
                }
            })
            .detach();
        }
        _ => {}
    }
}

/// Ends the open sessions `which` picks, as the sidebar's "End session" does.
pub fn end(app: &mut OrbitApp, which: impl Fn(&SessionMeta) -> bool, window: &mut Window, cx: &mut Context<OrbitApp>) {
    while let Some(ix) = app.tabs.iter().position(|t| matches!(&t.kind, TabKind::Terminal(v) if which(&v.read(cx).meta))) {
        app.close_tab(ix, true, window, cx);
    }
}

pub fn forget_recent(resume_id: &str, cx: &mut App) {
    store(cx).update(cx, |s, cx| {
        s.recent.retain(|r| r.resume_id != resume_id);
        s.save();
        cx.notify();
    });
}

/// Keys of the sessions open here or saved by any Orbit window: their files are in use.
pub fn live_keys(cx: &App) -> Vec<String> {
    let saved: Saved = saved_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    store(cx).read(cx).open.iter().map(|(m, _)| m.id.clone()).chain(saved.sessions.into_iter().map(|m| m.id)).collect()
}

/// Reopens a recent session in a new tab, picking the conversation up.
pub fn resume(app: &mut OrbitApp, r: &RecentSession, window: &mut Window, cx: &mut Context<OrbitApp>) {
    store(cx).update(cx, |s, cx| {
        s.recent.retain(|x| x.resume_id != r.resume_id);
        s.save();
        cx.notify();
    });
    let claude = r.agent == "claude";
    let args = if claude { vec!["--resume".into(), r.resume_id.clone()] } else { vec!["--session".into(), r.resume_id.clone()] };
    let meta = SessionMeta {
        id: new_key(),
        workspace: r.workspace.clone(),
        label: r.agent.clone(),
        session_name: r.session_name.clone(),
        codename: r.codename.clone(),
        cmd: Some(r.agent.clone()),
        args: args.clone(),
        repo: r.repo.clone(),
        agent_session_id: claude.then(|| r.resume_id.clone()),
        cwd: None,
    };
    open(app, meta, args, None, None, window, cx);
}

/// Reopens the sessions saved by the last run: claude picks its conversation
/// back up (and redraws it); other programs start fresh under the output they
/// printed before.
fn restore(app: &mut OrbitApp, saved: Vec<SessionMeta>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    // Seeing a session's tab (in a focused window) settles its notifications.
    let observers = vec![
        cx.observe_in(&cx.entity(), window, |a, _, window, cx| settle_active(a, window, cx)),
        cx.observe_window_activation(window, settle_active),
    ];
    let store = store(cx);
    store.update(cx, |s, _| s._app_observers = observers);
    // The shell restored the files first: their active tab stays active.
    let active = app.active_tab().map(|t| t.key(cx));
    let first = app.tabs.len();
    let dir_of = |m: &SessionMeta| pty::session_dir(&m.workspace, m.repo.as_deref(), m.cwd.as_deref()).ok();
    // omp tabs per folder, paired by launch order with the sessions last written there.
    let mut omp: HashMap<std::path::PathBuf, Vec<(u64, String)>> = HashMap::new();
    for m in saved.iter().filter(|m| m.cmd.as_deref().is_some_and(pty::is_omp)) {
        if let (Some(d), Some(at)) = (dir_of(m), launched_ms(&m.id)) {
            omp.entry(d).or_default().push((at, m.id.clone()));
        }
    }
    let mut omp_ids: HashMap<String, String> = HashMap::new();
    for (dir, mut tabs) in omp {
        tabs.sort();
        let ids = pty::omp_recent_sessions(&dir, tabs[0].0, tabs.len());
        // Fewer sessions than tabs: the newest tabs get them.
        let skip = tabs.len() - ids.len();
        omp_ids.extend(tabs.into_iter().skip(skip).map(|(_, key)| key).zip(ids));
    }
    for meta in saved {
        let dir = dir_of(&meta);
        let (args, replay) = match (&meta.cmd, &meta.agent_session_id) {
            (Some(cmd), Some(id)) if pty::is_claude(cmd) => {
                let exists = dir.as_deref().is_some_and(|d| pty::claude_session_exists(d, id));
                (vec![if exists { "--resume" } else { "--session-id" }.to_string(), id.clone()], None)
            }
            (Some(cmd), _) if pty::is_omp(cmd) => {
                match omp_ids.remove(&meta.id) {
                    // Resumed omp redraws the conversation itself.
                    Some(id) => (vec!["--resume".to_string(), id], None),
                    None => (vec![], Some(pty::scrollback(&meta.id))),
                }
            }
            _ => (vec![], Some(pty::scrollback(&meta.id))),
        };
        open(app, meta, args, replay, None, window, cx);
    }
    match active.and_then(|k| app.tab_index(&k, cx)) {
        Some(ix) => app.activate(ix, window, cx),
        // Sessions go before files: the first one is at 0.
        None if app.tabs.len() > first => app.activate(0, window, cx),
        None => {}
    }
}

/// The launch time a tab key carries (`tm:<ms>-<rand>`).
fn launched_ms(key: &str) -> Option<u64> {
    key.strip_prefix("tm:")?.split('-').next()?.parse().ok()
}

fn settle_active(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    if let Some(t) = app.active_terminal().filter(|_| window.is_window_active()) {
        let key = t.read(cx).meta.id.clone();
        store(cx).update(cx, |s, cx| s.settle(&key, cx));
    }
}

/// Focuses the session's tab and settles its notifications.
pub fn activate(app: &mut OrbitApp, key: &str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let ix = app.tabs.iter().position(|t| matches!(&t.kind, TabKind::Terminal(v) if v.read(cx).meta.id == key));
    if let Some(ix) = ix {
        app.activate(ix, window, cx);
        store(cx).update(cx, |s, cx| s.settle(key, cx));
    } else if let Some(scope) = key.strip_prefix("ralph:") {
        crate::pages::open_ralph(app, scope, window, cx);
        store(cx).update(cx, |s, cx| s.settle(key, cx));
    }
}

/// The next session, in tab order after the active one, that waits for the user.
fn next_needing_input(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let store = store(cx);
    let waiting: Vec<(usize, String)> = app
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(ix, t)| match &t.kind {
            TabKind::Terminal(v) => {
                let v = v.read(cx);
                let needs = v.status == Status::Waiting || store.read(cx).unread(&v.meta.id) == Some(InboxKind::Waiting);
                needs.then(|| (ix, v.meta.id.clone()))
            }
            _ => None,
        })
        .collect();
    let after = app.active.unwrap_or(usize::MAX);
    let next = waiting.iter().find(|(ix, _)| after != usize::MAX && *ix > after).or(waiting.first());
    match next {
        Some((_, key)) => {
            let key = key.clone();
            activate(app, &key, window, cx);
        }
        None => {
            app.toast(crate::toast::ToastKind::Info, "No session is waiting for input", None, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_uuids_and_names() {
        let key = new_key();
        assert!(key.starts_with("tm:") && key.len() > 10, "{key}");
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-')));
        let id = new_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        let name = codename(["amber-comet"]);
        assert!(name.contains('-') && name != "amber-comet");
        assert_eq!(scope_label("dir:~"), "Home");
        assert_eq!(scope_label("dir:/srv/app/"), "app");
        assert_eq!(scope_label("@api"), "api");
        assert_eq!(scope_label("feature-x"), "feature-x");
    }

    #[test]
    fn saved_sessions_read_the_react_field_names() {
        let raw = r#"{"sessions":[{"id":"tm:1-abcde","workspace":"dir:~","label":"claude","sessionName":"swift-otter","cmd":"claude","args":["--session-id","x"],"agentSessionId":"x"}],"recent":[]}"#;
        let saved: Saved = serde_json::from_str(raw).unwrap();
        assert_eq!(saved.sessions[0].session_name, "swift-otter");
        assert_eq!(saved.sessions[0].agent_session_id.as_deref(), Some("x"));
        assert!(saved.sessions[0].repo.is_none());
    }

    fn meta(name: &str) -> SessionMeta {
        SessionMeta {
            id: "tm:1-abcde".into(),
            workspace: "dir:~".into(),
            label: "claude".into(),
            session_name: name.into(),
            codename: None,
            cmd: Some("claude".into()),
            args: vec![],
            repo: None,
            agent_session_id: Some("x".into()),
            cwd: None,
        }
    }

    #[test]
    fn renaming_keeps_the_codename_aside() {
        let mut m = meta("swift-otter");
        set_name(&mut m, "auth fix");
        assert_eq!((m.session_name.as_str(), m.codename.as_deref()), ("auth fix", Some("swift-otter")));
        set_name(&mut m, "login bug");
        assert_eq!((m.session_name.as_str(), m.codename.as_deref()), ("login bug", Some("swift-otter")));
        set_name(&mut m, "");
        assert_eq!((m.session_name.as_str(), m.codename.as_deref()), ("swift-otter", None));
        set_name(&mut m, "x");
        set_name(&mut m, "swift-otter");
        assert_eq!((m.session_name.as_str(), m.codename.as_deref()), ("swift-otter", None));
    }

    #[test]
    fn names_persist_and_old_state_has_none() {
        let mut m = meta("swift-otter");
        set_name(&mut m, "auth fix");
        let recent = RecentSession {
            resume_id: "x".into(),
            agent: "claude".into(),
            workspace: "dir:~".into(),
            repo: None,
            session_name: m.session_name.clone(),
            codename: m.codename.clone(),
            ended_at: 1,
        };
        let json = serde_json::to_string(&Saved { sessions: vec![m.clone()], recent: vec![recent.clone()] }).unwrap();
        assert!(json.contains(r#""sessionName":"auth fix""#) && json.contains(r#""codename":"swift-otter""#), "{json}");
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sessions, vec![m]);
        assert_eq!(back.recent, vec![recent]);
        // Unnamed sessions don't write the field; old files without it read as unnamed.
        let plain = serde_json::to_string(&meta("swift-otter")).unwrap();
        assert!(!plain.contains("codename"), "{plain}");
        let old = r#"{"sessions":[],"recent":[{"resumeId":"x","agent":"claude","workspace":"dir:~","sessionName":"swift-otter","endedAt":1}]}"#;
        let old: Saved = serde_json::from_str(old).unwrap();
        assert_eq!((old.recent[0].session_name.as_str(), old.recent[0].codename.as_ref()), ("swift-otter", None));
    }
}
