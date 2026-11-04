//! Keyboard shortcut inhibition while a terminal has focus (Wayland's
//! `zwp_keyboard_shortcuts_inhibit_manager_v1`), so GNOME's Super+V, Super+A…
//! reach the shell: under Toshy those are the physical Ctrl+V, Ctrl+A.
//!
//! The protocol objects live on GPUI's own `wl_display` (from the window's raw
//! handle) with a separate event queue; GPUI's loop reads the socket and we
//! drain our queue. X11 has no equivalent: there every key goes to the app.
//! Mutter still runs `restore-shortcuts` (Super+Escape) and VT switching.
use gpui::{App, FocusHandle, WeakFocusHandle, Window};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::time::Duration;

thread_local! {
    static STATE: RefCell<Inhibit> = RefCell::new(Inhibit::default());
}

#[derive(Default)]
struct Inhibit {
    /// None: the default, `default_on()`.
    enabled: Option<bool>,
    terminals: Vec<WeakFocusHandle>,
    alt_held: bool,
    /// Requested at least once in this run.
    requested: bool,
    /// The one-time toast about GNOME's permission prompt was shown (saved).
    explained: bool,
    /// The compositor granted it at least once in this run.
    granted: bool,
    wl: Wl,
}

#[derive(Default)]
enum Wl {
    #[default]
    Untried,
    Unavailable(String),
    /// Never dropped: at exit GPUI's display may already be gone, and freeing our
    /// queue and proxies on it would crash.
    #[cfg(target_os = "linux")]
    Ready(std::mem::ManuallyDrop<Box<wl::Conn>>),
}

/// On by default in GNOME Wayland sessions, where Super shortcuts take keys Toshy users type.
pub fn default_on() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
    cfg!(target_os = "linux") && var("XDG_SESSION_TYPE") == "wayland" && var("XDG_CURRENT_DESKTOP").contains("gnome")
}

pub fn init(saved: Option<bool>, explained: bool) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.enabled = saved;
        s.explained = explained;
    });
}

pub fn explained() -> bool {
    STATE.with(|s| s.borrow().explained)
}

pub fn enabled() -> bool {
    STATE.with(|s| s.borrow().enabled.unwrap_or_else(default_on))
}

/// The value to save: None while it is the default.
pub fn saved() -> Option<bool> {
    STATE.with(|s| s.borrow().enabled).filter(|&on| on != default_on())
}

pub fn set_enabled(on: bool, cx: &mut App) {
    STATE.with(|s| s.borrow_mut().enabled = Some(on));
    // The next render syncs and saves.
    cx.refresh_windows();
}

pub fn register(focus: &FocusHandle) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.terminals.retain(|h| h.upgrade().is_some());
        s.terminals.push(focus.downgrade());
    });
}

/// The inhibitor gives way while Alt is down.
pub fn releases_for_alt() -> bool {
    STATE.with(|s| s.borrow().granted)
}

/// Alt+Tab is a regular GNOME binding, so it is inhibited too: while Alt is
/// down the inhibitor is dropped (once granted, so no permission prompt repeats).
pub fn set_alt(held: bool, window: &Window, cx: &mut App) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let changed = s.granted && s.alt_held != held;
        s.alt_held = held && s.granted;
        changed
    });
    if changed {
        sync(window, cx);
    }
}

/// Holds the inhibitor while a terminal has focus.
/// Returns true when the one-time explanation is due.
pub fn sync(window: &Window, cx: &mut App) -> bool {
    let (first, explain) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let focused = s.terminals.iter().filter_map(|h| h.upgrade()).any(|h| h.is_focused(window));
        // Not tied to window activation: the compositor applies it only while the
        // surface has keyboard focus, and GNOME's prompt takes that focus (dropping
        // the inhibitor then would dismiss the prompt unanswered).
        s.alt_held &= focused;
        let want = s.enabled.unwrap_or_else(default_on) && focused && !s.alt_held;
        let first = s.apply(want, window);
        let explain = first && !s.explained;
        s.explained |= explain;
        (first, explain)
    });
    if first {
        // GPUI's loop reads the socket; drain our queue for the answer a little while.
        cx.spawn(async move |cx| {
            for _ in 0..50 {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if poll() {
                    break;
                }
            }
        })
        .detach();
    }
    explain
}

/// Dispatches pending events; true once the compositor answered.
fn poll() -> bool {
    STATE.with(|s| s.borrow_mut().poll())
}

/// One line for the self-test dump.
pub fn status() -> String {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.poll();
        let mut out = format!("enabled {}, default {}", s.enabled.unwrap_or_else(default_on), default_on());
        let _ = match &s.wl {
            Wl::Untried => write!(out, ", not connected"),
            Wl::Unavailable(why) => write!(out, ", unavailable: {why}"),
            #[cfg(target_os = "linux")]
            Wl::Ready(c) => write!(out, ", {}", c.describe()),
        };
        let _ = write!(out, ", requested {}, granted {}", s.requested, s.granted);
        out
    })
}

impl Inhibit {
    fn apply(&mut self, want: bool, window: &Window) -> bool {
        self.poll();
        #[cfg(target_os = "linux")]
        {
            if want && matches!(self.wl, Wl::Untried) {
                self.wl = match wl::Conn::new(window) {
                    Ok(c) => Wl::Ready(std::mem::ManuallyDrop::new(Box::new(c))),
                    Err(e) => {
                        eprintln!("shortcut inhibitor unavailable: {e}");
                        Wl::Unavailable(e)
                    }
                };
            }
            if let Wl::Ready(c) = &mut self.wl {
                c.set(want);
                if want && !self.requested {
                    self.requested = true;
                    return true;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (want, window);
        false
    }

    fn poll(&mut self) -> bool {
        #[cfg(target_os = "linux")]
        if let Wl::Ready(c) = &mut self.wl {
            let answered = c.poll();
            self.granted |= c.active();
            return answered;
        }
        true
    }
}

#[cfg(target_os = "linux")]
mod wl {
    use gpui::Window;
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use wayland_backend::client::{Backend, ObjectId};
    use wayland_client::{
        globals::{registry_queue_init, GlobalListContents},
        protocol::{wl_registry::WlRegistry, wl_seat::WlSeat, wl_surface::WlSurface},
        Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    };
    use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
        zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1 as Manager,
        zwp_keyboard_shortcuts_inhibitor_v1::{Event, ZwpKeyboardShortcutsInhibitorV1 as Inhibitor},
    };

    #[derive(Default)]
    pub struct Events {
        /// The compositor's last word: Some(true) active, Some(false) inactive.
        active: Option<bool>,
        /// Inhibitors created and the first events, for the dump.
        created: u32,
        log: Vec<&'static str>,
    }

    pub struct Conn {
        conn: Connection,
        queue: EventQueue<Events>,
        events: Events,
        manager: Manager,
        seat: WlSeat,
        surface: WlSurface,
        inhibitor: Option<Inhibitor>,
    }

    impl Conn {
        pub fn new(window: &Window) -> Result<Self, String> {
            let display = match window.display_handle().map_err(|e| e.to_string())?.as_raw() {
                RawDisplayHandle::Wayland(d) => d.display,
                _ => return Err("not a Wayland session".into()),
            };
            let surface = match HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?.as_raw() {
                RawWindowHandle::Wayland(w) => w.surface,
                _ => return Err("not a Wayland window".into()),
            };
            // Safety: GPUI's display and surface outlive the app's only window.
            let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
            let conn = Connection::from_backend(backend);
            let (globals, queue) = registry_queue_init::<Events>(&conn).map_err(|e| e.to_string())?;
            let qh = queue.handle();
            let manager = globals.bind::<Manager, _, _>(&qh, 1..=1, ()).map_err(|_| "the compositor lacks keyboard-shortcuts-inhibit".to_string())?;
            let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
            let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }.map_err(|_| "bad surface")?;
            let surface = WlSurface::from_id(&conn, id).map_err(|_| "bad surface")?;
            Ok(Self { conn, queue, events: Events::default(), manager, seat, surface, inhibitor: None })
        }

        pub fn set(&mut self, on: bool) {
            match (on, self.inhibitor.is_some()) {
                (true, false) => {
                    let qh: QueueHandle<Events> = self.queue.handle();
                    self.inhibitor = Some(self.manager.inhibit_shortcuts(&self.surface, &self.seat, &qh, ()));
                    self.events.created += 1;
                    self.events.active = None;
                }
                (false, true) => {
                    if let Some(i) = self.inhibitor.take() {
                        i.destroy();
                    }
                    self.events.active = None;
                }
                _ => return,
            }
            let _ = self.conn.flush();
        }

        /// True once the compositor answered the current inhibitor (or none is held).
        pub fn poll(&mut self) -> bool {
            let _ = self.queue.dispatch_pending(&mut self.events);
            self.inhibitor.is_none() || self.events.active.is_some()
        }

        pub fn active(&self) -> bool {
            self.inhibitor.is_some() && self.events.active == Some(true)
        }

        pub fn describe(&self) -> String {
            let state = match (&self.inhibitor, self.events.active) {
                (None, _) => "released",
                (Some(_), None) => "requested (no answer yet)",
                (Some(_), Some(true)) => "active",
                (Some(_), Some(false)) => "inactive",
            };
            format!("inhibitor {state}, created {}, events {:?}", self.events.created, self.events.log)
        }
    }

    impl Dispatch<Inhibitor, ()> for Events {
        fn event(s: &mut Self, _: &Inhibitor, event: Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            let (active, name) = match event {
                Event::Active => (true, "active"),
                Event::Inactive => (false, "inactive"),
                _ => return,
            };
            // Each Alt press makes a new one: keep the log short.
            if s.log.len() < 8 {
                eprintln!("shortcut inhibitor {name}");
                s.log.push(name);
            }
            s.active = Some(active);
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for Events {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    wayland_client::delegate_noop!(Events: ignore WlSeat);
    wayland_client::delegate_noop!(Events: Manager);
}
