//! Updates (`src/lib/updater.ts`): a check at start, every hour and from
//! Settings → Updates; a toast once per new version. Installing downloads the
//! release, checks its signature and replaces the app (`orbit_core::updates`),
//! then offers a restart.
use crate::app::OrbitApp;
use crate::toast::{ToastAction, ToastKind};
use gpui::{App, AppContext, Context, Global, WeakEntity, Window};
use orbit_core::updates::{self as core, Release, Target};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const RELEASES_URL: &str = "https://github.com/igorsegallafa/orbit/releases/latest";
const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The app's version: the release tag's, or Cargo.toml's (build.rs).
pub fn version() -> &'static str {
    env!("ORBIT_VERSION")
}

/// Where an update goes, or why this install can't update itself.
pub fn target() -> &'static Result<Target, String> {
    static T: OnceLock<Result<Target, String>> = OnceLock::new();
    T.get_or_init(Target::detect)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(String),
    Downloading { version: String, percent: Option<u8> },
    Ready(String),
    Failed(String),
}

pub struct Updater {
    pub status: Status,
    release: Option<Release>,
    checked_at: Option<u64>,
    /// The version a toast was shown for.
    announced: Option<String>,
    app: Option<WeakEntity<OrbitApp>>,
}

impl Global for Updater {}

impl Updater {
    pub fn describe(&self) -> String {
        match &self.status {
            Status::Idle => "Not checked yet.".into(),
            Status::Checking => "Checking for a new release…".into(),
            Status::UpToDate => format!("You're on the latest version (checked {}).", super::ui::ago(self.checked_at.unwrap_or(0) as i64)),
            Status::Available(v) => match target() {
                Ok(_) => format!("Orbit {v} is available."),
                Err(why) => format!("Orbit {v} is available. {why}"),
            },
            Status::Downloading { version, percent } => {
                format!("Downloading Orbit {version}{}…", percent.map(|p| format!(" · {p}%")).unwrap_or_default())
            }
            Status::Ready(v) => format!("Orbit {v} is installed. Restart to update."),
            Status::Failed(_) => "The last update failed.".into(),
        }
    }
}

pub fn state(cx: &App) -> &Updater {
    cx.global::<Updater>()
}

pub fn init(cx: &mut App) {
    core::cleanup();
    cx.set_global(Updater { status: Status::Idle, release: None, checked_at: None, announced: None, app: None });
    cx.observe_new(|_: &mut OrbitApp, _: Option<&mut Window>, cx: &mut Context<OrbitApp>| {
        cx.global_mut::<Updater>().app = Some(cx.entity().downgrade());
        // A self-test checks only when pointed at a test manifest.
        let auto = !crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) || std::env::var_os("ORBIT_UPDATE_URL").is_some();
        if auto {
            cx.spawn(async move |_, cx| loop {
                cx.update(|cx| check(false, cx));
                cx.background_executor().timer(INTERVAL).await;
            })
            .detach();
        }
    })
    .detach();
}

fn toast(cx: &mut App, f: impl FnOnce(&mut OrbitApp, &mut Context<OrbitApp>)) {
    if let Some(app) = cx.global::<Updater>().app.clone().and_then(|a| a.upgrade()) {
        app.update(cx, f);
    }
}

fn action(label: &str, run: fn(&mut App)) -> ToastAction {
    ToastAction { label: label.to_string().into(), run: Rc::new(run) }
}

/// Asks for the latest release. Background checks stay quiet on failure and
/// toast a version once; a manual check reports what it finds.
pub fn check(manual: bool, cx: &mut App) {
    if matches!(cx.global::<Updater>().status, Status::Checking | Status::Downloading { .. } | Status::Ready(_)) {
        return;
    }
    cx.global_mut::<Updater>().status = Status::Checking;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let r = cx.background_spawn(async { core::check(version()) }).await;
        cx.update(|cx| {
            let u = cx.global_mut::<Updater>();
            u.checked_at = Some(super::ui::now());
            u.status = match &r {
                Ok(Some(rel)) => Status::Available(rel.version.clone()),
                Ok(None) => Status::UpToDate,
                Err(e) if manual => Status::Failed(e.clone()),
                Err(_) => Status::Idle,
            };
            if let Ok(rel) = r {
                u.release = rel;
            }
            let announce = match &u.status {
                Status::Available(v) if !manual && u.announced.as_ref() != Some(v) => Some(v.clone()),
                _ => None,
            };
            if let Some(v) = announce {
                u.announced = Some(v.clone());
                let installable = target().is_ok();
                toast(cx, |a, cx| {
                    if installable {
                        a.toast_action(format!("Orbit {v} is available"), Some("Restart to install the update.".into()), action("Update", install), cx)
                    } else {
                        a.toast(ToastKind::Info, format!("Orbit {v} is available"), Some("See Settings → Updates.".into()), cx)
                    }
                });
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Downloads, verifies and installs the pending release; then offers a restart.
pub fn install(cx: &mut App) {
    let u = cx.global::<Updater>();
    let Some(release) = u.release.clone().filter(|_| matches!(u.status, Status::Available(_) | Status::Failed(_))) else { return };
    let target = match target() {
        Ok(t) => t.clone(),
        Err(why) => {
            cx.global_mut::<Updater>().status = Status::Failed(why.clone());
            cx.refresh_windows();
            return;
        }
    };
    let version = release.version.clone();
    cx.global_mut::<Updater>().status = Status::Downloading { version: version.clone(), percent: None };
    cx.refresh_windows();
    let progress: Arc<Mutex<Option<u8>>> = Arc::default();
    let result: Arc<Mutex<Option<Result<(), String>>>> = Arc::default();
    cx.background_spawn({
        let (progress, result) = (progress.clone(), result.clone());
        async move {
            let r = core::install(&release, &target, &core::public_key(), |done, total| {
                *progress.lock().unwrap() = total.filter(|t| *t > 0).map(|t| (done.min(t) * 100 / t) as u8);
            });
            *result.lock().unwrap() = Some(r);
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let r = loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if let Some(r) = result.lock().unwrap().take() {
                break r;
            }
            let percent = *progress.lock().unwrap();
            let version = version.clone();
            cx.update(|cx| {
                cx.global_mut::<Updater>().status = Status::Downloading { version, percent };
                cx.refresh_windows();
            });
        };
        cx.update(|cx| {
            match r {
                Ok(()) => {
                    eprintln!("update: installed {version}");
                    cx.global_mut::<Updater>().status = Status::Ready(version.clone());
                    toast(cx, |a, cx| {
                        a.toast_action(format!("Orbit {version} is ready"), Some("Restart to finish the update.".into()), action("Restart to update", restart), cx)
                    });
                }
                Err(e) => {
                    eprintln!("update: failed: {e}");
                    cx.global_mut::<Updater>().status = Status::Failed(e.clone());
                    toast(cx, |a, cx| a.toast(ToastKind::Error, "Update failed", Some(e.into()), cx));
                }
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Starts the installed version and quits this one.
pub fn restart(cx: &mut App) {
    if !matches!(cx.global::<Updater>().status, Status::Ready(_)) {
        return;
    }
    let Ok(target) = target() else { return };
    match core::relaunch(target, std::env::args_os().skip(1)) {
        Ok(()) => cx.quit(),
        Err(e) => {
            cx.global_mut::<Updater>().status = Status::Failed(e.clone());
            toast(cx, |a, cx| a.toast(ToastKind::Error, "Restart failed", Some(e.into()), cx));
            cx.refresh_windows();
        }
    }
}

pub fn dump(out: &mut String, cx: &App) {
    if let Some(u) = cx.try_global::<Updater>() {
        let _ = writeln!(out, "--- updates: version {} · {:?} · {}", version(), u.status, u.describe());
    }
}
