//! The update check (`src/lib/updater.ts` without the install): at start and
//! every hour, plus Settings → Updates. A new version is announced once with
//! a toast. Installing comes with the native packaging.
use crate::app::OrbitApp;
use crate::toast::ToastKind;
use gpui::{App, AppContext, Context, Global, WeakEntity, Window};
use std::fmt::Write as _;
use std::time::Duration;

pub const RELEASES_URL: &str = "https://github.com/igorsegallafa/orbit/releases/latest";
const INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The product version (`native/Cargo.toml`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(String),
    Failed(String),
}

pub struct Updater {
    pub status: Status,
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
            Status::Available(v) => format!("Orbit {v} is available. Installing from the native app comes with its packaging: get it from the release page."),
            Status::Failed(_) => "The last check failed.".into(),
        }
    }
}

pub fn state(cx: &App) -> &Updater {
    cx.global::<Updater>()
}

pub fn init(cx: &mut App) {
    cx.set_global(Updater { status: Status::Idle, checked_at: None, announced: None, app: None });
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

/// Asks for the latest release. Background checks stay quiet on failure and
/// toast a version once; a manual check reports what it finds.
pub fn check(manual: bool, cx: &mut App) {
    if cx.global::<Updater>().status == Status::Checking {
        return;
    }
    cx.global_mut::<Updater>().status = Status::Checking;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let r = cx.background_spawn(async { orbit_core::updates::check(version()) }).await;
        cx.update(|cx| {
            let u = cx.global_mut::<Updater>();
            u.checked_at = Some(super::ui::now());
            u.status = match r {
                Ok(Some(rel)) => Status::Available(rel.version),
                Ok(None) => Status::UpToDate,
                Err(e) if manual => Status::Failed(e),
                Err(_) => Status::Idle,
            };
            let announce = match &u.status {
                Status::Available(v) if u.announced.as_ref() != Some(v) => Some(v.clone()),
                _ => None,
            };
            let app = u.app.clone();
            if let Some(v) = &announce {
                u.announced = Some(v.clone());
            }
            if let (Some(v), Some(app)) = (announce, app.and_then(|a| a.upgrade())) {
                app.update(cx, |a, cx| a.toast(ToastKind::Info, format!("Orbit {v} is available"), Some("See Settings → Updates.".into()), cx));
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

pub fn dump(out: &mut String, cx: &App) {
    if let Some(u) = cx.try_global::<Updater>() {
        let _ = writeln!(out, "--- updates: version {} · {:?} · {}", version(), u.status, u.describe());
    }
}
