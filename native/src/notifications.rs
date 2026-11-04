//! Desktop notifications, as the React app's `notifyOs` (`Inbox.tsx`): an agent
//! finished or needs the user, a Ralph run ended. Skipped while the window is
//! focused on the item's tab, and when Settings → Notifications turns them off.
//! Items arriving within `COALESCE` go out as one notification. Clicking one
//! raises the window on the item's tab.
//!
//! Linux talks to `org.freedesktop.Notifications` through `notify-rust` (app
//! name, icon, `desktop-entry` hint, the `default` action); macOS and Windows go
//! through GPUI's own `UNUserNotificationCenter` / toast support.
use gpui::{App, Global, Task};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const COALESCE: Duration = Duration::from_millis(800);
/// The tag of a merged burst: a newer burst replaces it.
const BATCH_TAG: &str = "orbit:batch";
const LOG_MAX: usize = 20;
/// The Windows AppUserModelID.
const APP_ID: &str = "com.nsx001220.orbit.native";

static ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Replaces an earlier notification with the same tag.
    pub tag: String,
    /// What a click opens: a session id or `ralph:<scope>` (see `sessions::activate`).
    pub key: String,
    pub title: String,
    pub body: String,
}

/// The rule of `App.tsx`'s `pushInbox`: not while the user looks at that tab.
pub fn suppressed(enabled: bool, window_active: bool, tab_active: bool) -> bool {
    !enabled || (window_active && tab_active)
}

/// Queues `note`; an older pending one for the same key gives way.
fn enqueue(pending: &mut Vec<Note>, note: Note) {
    pending.retain(|n| n.key != note.key);
    pending.push(note);
}

/// One notification for a burst: the note itself, or a summary that opens the newest.
fn merge(mut notes: Vec<Note>) -> Option<Note> {
    if notes.len() <= 1 {
        return notes.pop();
    }
    let newest = notes.last()?.key.clone();
    let mut lines: Vec<String> = notes.iter().rev().take(4).map(|n| n.title.clone()).collect();
    if notes.len() > 4 {
        lines.push(format!("and {} more", notes.len() - 4));
    }
    Some(Note { tag: BATCH_TAG.into(), key: newest, title: format!("{} updates in Orbit", notes.len()), body: lines.join("\n") })
}

#[derive(Clone)]
struct Sent {
    note: Note,
    delivered: bool,
}

struct State {
    pending: Vec<Note>,
    flush: Option<Task<()>>,
    /// Tag → key, for clicks.
    tags: HashMap<String, String>,
    log: Vec<Sent>,
    /// Off in self-tests unless `ORBIT_SELFTEST_NOTIFY=1`: they record, not show.
    deliver: bool,
    #[cfg(target_os = "linux")]
    xdg: xdg::Xdg,
}

impl Global for State {}

/// Sets the app identity and the click handler. Called once, before the window opens.
pub fn init(enabled: bool, cx: &mut App) {
    ENABLED.store(enabled, Ordering::Relaxed);
    cx.set_app_identity(APP_ID, "Orbit");
    let self_test = crate::probe::ENABLED.load(Ordering::Relaxed);
    let deliver = !self_test || std::env::var("ORBIT_SELFTEST_NOTIFY").is_ok_and(|v| v == "1");
    #[cfg(target_os = "linux")]
    let xdg = xdg::Xdg::start(cx);
    #[cfg(not(target_os = "linux"))]
    cx.on_system_notification_response(|r, cx| respond(&r.tag, cx));
    cx.set_global(State {
        pending: vec![],
        flush: None,
        tags: HashMap::new(),
        log: vec![],
        deliver,
        #[cfg(target_os = "linux")]
        xdg,
    });
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool, cx: &mut App) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on && cx.has_global::<State>() {
        cx.global_mut::<State>().pending.clear();
    }
    // The state file saves on the next render.
    cx.refresh_windows();
}

/// Raises a desktop notification for inbox item `key` unless the user is
/// looking at its tab when the burst goes out. Safe to call from inside entity updates.
pub fn post(key: impl Into<String>, title: impl Into<String>, body: impl Into<String>, cx: &mut App) {
    if !enabled() || !cx.has_global::<State>() {
        return;
    }
    let key = key.into();
    let note = Note { tag: format!("orbit:{key}"), key, title: title.into(), body: body.into() };
    let s = cx.global_mut::<State>();
    enqueue(&mut s.pending, note);
    if s.flush.is_none() {
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(COALESCE).await;
            cx.update(flush);
        });
        cx.global_mut::<State>().flush = Some(task);
    }
}

/// Shows a notification now, ignoring focus (Settings' test button).
pub fn test(cx: &mut App) {
    let note = Note { tag: "orbit:test".into(), key: String::new(), title: "Orbit".into(), body: "Notifications are working.".into() };
    show(note, cx);
}

fn flush(cx: &mut App) {
    let pending = std::mem::take(&mut cx.global_mut::<State>().pending);
    cx.global_mut::<State>().flush = None;
    let notes: Vec<Note> = pending
        .into_iter()
        .filter(|n| {
            let (window_active, tab_active) = crate::sessions::focus_of(&n.key, cx);
            !suppressed(enabled(), window_active, tab_active)
        })
        .collect();
    if let Some(note) = merge(notes) {
        show(note, cx);
    }
}

fn show(note: Note, cx: &mut App) {
    let s = cx.global_mut::<State>();
    s.tags.insert(note.tag.clone(), note.key.clone());
    s.log.insert(0, Sent { note: note.clone(), delivered: s.deliver });
    s.log.truncate(LOG_MAX);
    if !s.deliver {
        return;
    }
    #[cfg(target_os = "linux")]
    s.xdg.show(&note);
    #[cfg(not(target_os = "linux"))]
    cx.show_system_notification(gpui::SystemNotification {
        tag: note.tag.into(),
        title: note.title.into(),
        body: note.body.into(),
        actions: vec![],
    });
}

/// A click on the notification tagged `tag`: the window comes forward on its tab.
pub fn respond(tag: &str, cx: &mut App) {
    let Some(key) = cx.try_global::<State>().and_then(|s| s.tags.get(tag).cloned()) else { return };
    cx.activate(true);
    crate::sessions::reveal(key, cx);
}

/// Self-test: a click on the newest notification, through the same handler as a real one.
pub fn click_latest(cx: &mut App) -> Option<String> {
    let tag = cx.try_global::<State>()?.log.first()?.note.tag.clone();
    respond(&tag, cx);
    Some(tag)
}

pub fn dump(out: &mut String, cx: &App) {
    use std::fmt::Write as _;
    let Some(s) = cx.try_global::<State>() else { return };
    let _ = writeln!(out, "--- notifications ({}, {} pending):", if enabled() { "on" } else { "off" }, s.pending.len());
    for e in &s.log {
        let how = if e.delivered { "shown" } else { "recorded" };
        let _ = writeln!(out, "  [{how}] {} → {}: {} · {}", e.note.tag, e.note.key, e.note.title, e.note.body.replace('\n', " / "));
    }
}

#[cfg(target_os = "linux")]
mod xdg {
    use super::Note;
    use futures::channel::mpsc::{unbounded, UnboundedSender};
    use futures::StreamExt;
    use gpui::App;
    use notify_rust::{Hint, Notification};
    use std::collections::HashMap;
    use std::sync::OnceLock;

    enum Event {
        Shown { tag: String, id: u32 },
        Clicked(u32),
    }

    pub struct Xdg {
        tx: UnboundedSender<Event>,
        /// Tag → server id, so a newer notification replaces the older in place.
        ids: HashMap<String, u32>,
    }

    impl Xdg {
        pub fn start(cx: &mut App) -> Self {
            let (tx, mut rx) = unbounded::<Event>();
            cx.spawn(async move |cx| {
                while let Some(ev) = rx.next().await {
                    cx.update(|cx| match ev {
                        Event::Shown { tag, id } => {
                            cx.global_mut::<super::State>().xdg.ids.insert(tag, id);
                        }
                        Event::Clicked(id) => {
                            let tag = cx.global::<super::State>().xdg.ids.iter().find(|(_, v)| **v == id).map(|(t, _)| t.clone());
                            if let Some(tag) = tag {
                                super::respond(&tag, cx);
                            }
                        }
                    });
                }
            })
            .detach();
            Self { tx, ids: HashMap::new() }
        }

        pub fn show(&self, note: &Note) {
            let mut n = Notification::new();
            n.appname("Orbit")
                .summary(&note.title)
                .body(&escape(&note.body))
                .hint(Hint::DesktopEntry("orbit".into()))
                .action("default", "Open");
            if let Some(icon) = icon() {
                n.icon(icon);
            }
            if let Some(&id) = self.ids.get(&note.tag) {
                n.id(id);
            }
            let (tx, tag) = (self.tx.clone(), note.tag.clone());
            // `show` and `wait_for_action` block on D-Bus: off the UI thread.
            let spawned = std::thread::Builder::new().name("notification".into()).spawn(move || match n.show() {
                Ok(handle) => {
                    let id = handle.id();
                    let _ = tx.unbounded_send(Event::Shown { tag, id });
                    handle.wait_for_action(|action| {
                        if action == "default" {
                            let _ = tx.unbounded_send(Event::Clicked(id));
                        }
                    });
                }
                Err(e) => eprintln!("notification: {e}"),
            });
            if let Err(e) = spawned {
                eprintln!("notification thread: {e}");
            }
        }
    }

    /// The body is markup for most servers.
    pub(super) fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// The app icon as a file (installs also have it in the icon theme, through the desktop entry).
    fn icon() -> Option<&'static str> {
        static PATH: OnceLock<Option<String>> = OnceLock::new();
        PATH.get_or_init(|| {
            let path = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir).join("orbit-native-icon.png");
            if !path.exists() {
                std::fs::write(&path, include_bytes!("../packaging/icons/128x128.png")).ok()?;
            }
            path.to_str().map(str::to_string)
        })
        .as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(key: &str, title: &str) -> Note {
        Note { tag: format!("orbit:{key}"), key: key.into(), title: title.into(), body: "Home".into() }
    }

    #[test]
    fn focus_and_settings_rules() {
        // Window focused on that tab: no notification.
        assert!(suppressed(true, true, true));
        // Another tab active, or the window in the background: notify.
        assert!(!suppressed(true, true, false));
        assert!(!suppressed(true, false, true));
        assert!(!suppressed(true, false, false));
        // Turned off in Settings: never.
        assert!(suppressed(false, false, false));
    }

    #[test]
    fn a_burst_becomes_one_notification() {
        let mut pending = vec![];
        enqueue(&mut pending, note("a", "alpha needs your attention"));
        enqueue(&mut pending, note("b", "bravo finished"));
        // The same session again: only its latest state counts.
        enqueue(&mut pending, note("a", "alpha finished"));
        assert_eq!(pending.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["bravo finished", "alpha finished"]);
        let merged = merge(pending).unwrap();
        assert_eq!(merged.tag, BATCH_TAG);
        assert_eq!(merged.key, "a", "a click opens the newest");
        assert_eq!(merged.title, "2 updates in Orbit");
        assert_eq!(merged.body, "alpha finished\nbravo finished");
    }

    #[test]
    fn single_notes_pass_through_and_long_bursts_are_cut() {
        assert_eq!(merge(vec![]), None);
        assert_eq!(merge(vec![note("a", "x")]), Some(note("a", "x")));
        let many: Vec<Note> = (0..6).map(|i| note(&i.to_string(), &format!("s{i}"))).collect();
        let m = merge(many).unwrap();
        assert_eq!(m.title, "6 updates in Orbit");
        assert_eq!(m.body, "s5\ns4\ns3\ns2\nand 2 more");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bodies_are_escaped_for_markup() {
        assert_eq!(xdg::escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }
}
