//! Pages that are not files or sessions: Settings (`SettingsPage.tsx` and its
//! sections), Integrations (`IntegrationsPage.tsx`), and per workspace Ralph
//! (`RalphView.tsx`), AI usage (`UsageBar.tsx`) and CI artifacts
//! (`ArtifactsSection.tsx`). Each page is its own view inside a `PageTab`.
//! Also the update check (`updates.rs`) and the crash log (`crash.rs`).
mod artifacts;
pub mod crash;
mod integrations;
pub mod kit;
mod ralph;
mod repos;
mod settings;
pub mod themes;
pub mod ui;
pub mod updates;
mod usage;

use crate::app::{OrbitApp, Tab, TabKind};
use crate::commands::{self, Command};
use crate::overlay::{ButtonStyle, Dialog, DialogButton, MenuItem};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{
    div, prelude::*, AnyView, App, Context, Entity, FocusHandle, Focusable, SharedString, WeakEntity, Window,
};
use std::fmt::Write as _;
use std::rc::Rc;

pub use settings::Section;

/// A page's view.
pub trait PageView: Render + Sized + 'static {
    fn title(&self) -> SharedString;
    /// Its part of the self-test dump.
    fn dump(&self, out: &mut String, cx: &App);
    /// Called when its tab is opened again.
    fn reopen(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

type DumpFn = Rc<dyn Fn(&mut String, &App)>;
/// A page action run on the root view's behalf (menu rows, reopen).
pub type Action = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct PageTab {
    pub key: String,
    view: AnyView,
    title: Rc<dyn Fn(&App) -> SharedString>,
    dump: DumpFn,
    reopen: Action,
    focus: FocusHandle,
}

impl PageTab {
    pub fn title(&self, cx: &App) -> SharedString {
        (self.title)(cx)
    }
}

impl Focusable for PageTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PageTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).size_full().bg(c(theme::BG)).font_family(theme::UI_FONT).child(self.view.clone())
    }
}

/// The root view, for toasts, dialogs and menus from a page.
#[derive(Clone)]
pub struct Host(pub WeakEntity<OrbitApp>);

impl Host {
    pub fn toast(&self, kind: ToastKind, title: impl Into<SharedString>, description: Option<String>, cx: &mut App) {
        let title = title.into();
        if let Some(app) = self.0.upgrade() {
            app.update(cx, |a, cx| a.toast(kind, title, description.map(Into::into), cx));
        }
    }

    pub fn error(&self, title: impl Into<SharedString>, err: String, cx: &mut App) {
        self.toast(ToastKind::Error, title, Some(err), cx);
    }

    /// A Cancel / `confirm` dialog; `on_confirm` runs on the confirm button.
    pub fn confirm(
        &self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        confirm: impl Into<SharedString>,
        on_confirm: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(app) = self.0.upgrade() else { return };
        let dialog = Dialog {
            title: title.into(),
            message: message.into(),
            buttons: vec![
                DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) },
                DialogButton { label: confirm.into(), style: ButtonStyle::Primary, run: Rc::new(move |_, w, cx| on_confirm(w, cx)) },
            ],
            on_cancel: Rc::new(|_, _, _| {}),
        };
        app.update(cx, |a, cx| a.open_dialog(dialog, window, cx));
    }

    /// Closes the page open under `key` ("Back to app").
    pub fn close(&self, key: &str, window: &mut Window, cx: &mut App) {
        let Some(app) = self.0.upgrade() else { return };
        let key = key.to_string();
        // Deferred: the page asking is being updated right now.
        window.defer(cx, move |window, cx| {
            app.update(cx, |a, cx| {
                if let Some(ix) = a.tab_index(&key, cx) {
                    a.close_tab(ix, false, window, cx);
                }
            });
        });
    }

    /// Opens a file in an editor tab.
    pub fn open_file(&self, path: std::path::PathBuf, window: &mut Window, cx: &mut App) {
        let Some(app) = self.0.upgrade() else { return };
        window.defer(cx, move |window, cx| {
            app.update(cx, |a, cx| {
                a.open_file(path, false, window, cx);
            });
        });
    }

    /// A menu at the mouse: `(label, action)` rows.
    pub fn menu(&self, items: Vec<(String, Action)>, window: &mut Window, cx: &mut App) {
        let Some(app) = self.0.upgrade() else { return };
        let origin = window.mouse_position();
        let items = items.into_iter().map(|(label, run)| MenuItem::new(label, move |_, w, cx| run(w, cx))).collect();
        app.update(cx, |a, cx| a.open_menu(origin, items, window, cx));
    }
}

/// Opens the page under `key`, or shows it (and lets it reload) when open.
pub fn open<V: PageView>(
    app: &mut OrbitApp,
    key: String,
    make: impl FnOnce(Host, &mut Window, &mut Context<V>) -> V,
    window: &mut Window,
    cx: &mut Context<OrbitApp>,
) {
    if let Some(ix) = app.tab_index(&key, cx) {
        if let TabKind::Tool(t) = &app.tabs[ix].kind {
            let reopen = t.read(cx).reopen.clone();
            reopen(window, cx);
        }
        return app.activate(ix, window, cx);
    }
    let host = Host(cx.entity().downgrade());
    let view: Entity<V> = cx.new(|cx| make(host, window, cx));
    cx.observe(&view, |_, _, cx| cx.notify()).detach();
    let (v1, v2, v3) = (view.clone(), view.clone(), view.downgrade());
    let tab = cx.new(|cx| PageTab {
        key,
        view: view.into(),
        title: Rc::new(move |cx| v1.read(cx).title()),
        dump: Rc::new(move |out, cx| v2.read(cx).dump(out, cx)),
        reopen: Rc::new(move |window, cx| {
            let _ = v3.update(cx, |v, cx| v.reopen(window, cx));
        }),
        focus: cx.focus_handle(),
    });
    let ix = app.insert_tab(Tab { kind: TabKind::Tool(tab), preview: false });
    app.activate(ix, window, cx);
}

/// The key of the Settings page's tab: the shell can show that tab full-window,
/// since the page draws its own navigation in place of the app sidebar.
pub const SETTINGS_KEY: &str = "page:settings";

/// Opens Settings on `section` (the sidebar's Settings entry uses `Section::Repos`).
pub fn open_settings(app: &mut OrbitApp, section: Section, window: &mut Window, cx: &mut Context<OrbitApp>) {
    open(app, SETTINGS_KEY.into(), |host, window, cx| settings::SettingsPage::new(host, section, window, cx), window, cx);
    // An open Settings tab moves to the section asked for.
    if let Some(ix) = app.tab_index(SETTINGS_KEY, cx) {
        if let TabKind::Tool(t) = &app.tabs[ix].kind {
            if let Ok(view) = t.read(cx).view.clone().downcast::<settings::SettingsPage>() {
                view.update(cx, |p, cx| p.select(section, window, cx));
            }
        }
    }
}

pub fn open_integrations(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    open(app, "page:integrations".into(), integrations::IntegrationsPage::new, window, cx);
}

/// Opens the selected workspace's Ralph page.
pub fn open_ralph(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    open_for_workspace(app, "ralph", window, cx);
}

/// Workspace pages need a selected workspace.
fn open_for_workspace(app: &mut OrbitApp, kind: &'static str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(ws) = app.workspace().cloned() else {
        return app.toast(ToastKind::Info, "Pick a workspace first", None, cx);
    };
    let key = format!("page:{kind}:{}", ws.meta.name);
    match kind {
        "ralph" => open(app, key, |h, w, cx| ralph::RalphPage::new(h, ws, w, cx), window, cx),
        "usage" => open(app, key, |h, w, cx| usage::UsagePage::new(h, ws, w, cx), window, cx),
        _ => open(app, key, |h, w, cx| artifacts::ArtifactsPage::new(h, ws, w, cx), window, cx),
    }
}

/// The pages' commands, plus the update check and the crash notice.
pub fn init(cx: &mut App) {
    let has_ws = |a: &OrbitApp, _: &App| a.workspace().is_some();
    for command in [
        Command::new("app.settings", "Open Settings").keys(["ctrl-,"]).run(|a, w, cx| open_settings(a, Section::Repos, w, cx)),
        Command::new("settings.languages", "Settings: Languages").run(|a, w, cx| open_settings(a, Section::Languages, w, cx)),
        Command::new("settings.health", "Settings: Health").run(|a, w, cx| open_settings(a, Section::Health, w, cx)),
        Command::new("settings.ai", "Settings: AI").run(|a, w, cx| open_settings(a, Section::Ai, w, cx)),
        Command::new("app.integrations", "Open Integrations").run(open_integrations),
        Command::new("app.checkForUpdates", "Check for Updates").run(|a, w, cx| {
            open_settings(a, Section::Updates, w, cx);
            updates::check(true, cx);
        }),
        Command::new("ralph.open", "Ralph").when(has_ws).run(open_ralph),
        Command::new("workspace.usage", "AI Usage").when(has_ws).run(|a, w, cx| open_for_workspace(a, "usage", w, cx)),
        Command::new("workspace.artifacts", "CI Artifacts").when(has_ws).run(|a, w, cx| open_for_workspace(a, "artifacts", w, cx)),
        Command::new("workspace.openInEditor", "Open in External Editor…").when(has_ws).run(open_repo_in_editor),
        Command::new("workspace.revealFolder", "Open Workspace Folder").when(has_ws).run(|a, _, cx| {
            let Some(dir) = a.workspace().map(|ws| ws.dir.clone()) else { return };
            if crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                a.toast(ToastKind::Info, "Revealed", Some(dir.display().to_string().into()), cx);
            } else {
                cx.reveal_path(&dir);
            }
        }),
    ] {
        commands::register(cx, command);
    }
    updates::init(cx);
    crash::init(cx);
}

/// A menu of the whole workspace and its repos; the pick opens in VS Code or Zed.
fn open_repo_in_editor(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(ws) = app.workspace().cloned() else { return };
    let items = std::iter::once(None)
        .chain(ws.meta.repos.iter().map(Some))
        .map(|repo| {
            let (name, repo, dir) = (ws.meta.name.clone(), repo.cloned(), ws.dir.clone());
            MenuItem::new(repo.clone().unwrap_or_else(|| "Whole workspace".into()), move |a, _, cx| {
                let (name, repo, dir) = (name.clone(), repo.clone(), dir.clone());
                cx.spawn(async move |this, cx| {
                    let r = cx
                        .background_spawn(async move {
                            match repo {
                                Some(repo) => orbit_core::settings::open_in_editor(&name, &repo),
                                None => orbit_core::settings::launch_editor(&dir),
                            }
                        })
                        .await;
                    if let Err(e) = r {
                        let _ = this.update(cx, |a, cx| a.toast(ToastKind::Error, "Couldn't open the editor", Some(e.into()), cx));
                    }
                })
                .detach();
                let _ = a;
            })
        })
        .collect();
    let origin = window.mouse_position();
    app.open_menu(origin, items, window, cx);
}

/// The sidebar's entries: the workspace's pages, then Settings and Integrations.
pub fn sidebar(app: &OrbitApp, cx: &mut Context<OrbitApp>) -> impl IntoElement {
    let entry = |id: &'static str, label: &'static str, run: fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>), cx: &mut Context<OrbitApp>| {
        div()
            .id(id)
            .relative()
            .mx_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(c(theme::TEXT_DIM))
            .hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)))
            .child(label)
            .child(probe(id))
            .on_click(cx.listener(move |a, _, w, cx| run(a, w, cx)))
    };
    let running = app.workspace().is_some_and(|ws| ralph::is_running(&ws.meta.name));
    div()
        .when(app.workspace().is_some(), |d| {
            d.child(ui::nav_title("WORKSPACE"))
                .child(
                    entry("nav-ralph", "Ralph", |a, w, cx| open_for_workspace(a, "ralph", w, cx), cx)
                        .when(running, |d| d.child(div().absolute().right_2().top_2().size_2().rounded_full().bg(c(theme::OK)))),
                )
                .child(entry("nav-usage", "AI usage", |a, w, cx| open_for_workspace(a, "usage", w, cx), cx))
                .child(entry("nav-artifacts", "CI artifacts", |a, w, cx| open_for_workspace(a, "artifacts", w, cx), cx))
        })
        .child(ui::nav_title("ORBIT"))
        .child(entry("nav-integrations", "Integrations", open_integrations, cx))
        .child(entry("nav-settings", "Settings", |a, w, cx| open_settings(a, Section::Repos, w, cx), cx))
}

/// The pages' part of the self-test dump.
pub fn dump(app: &OrbitApp, cx: &App) -> String {
    let mut out = String::new();
    for tab in &app.tabs {
        if let TabKind::Tool(t) = &tab.kind {
            let t = t.read(cx);
            let _ = writeln!(out, "--- page {} ({})", t.title(cx), t.key);
            (t.dump.clone())(&mut out, cx);
        }
    }
    updates::dump(&mut out, cx);
    out
}
