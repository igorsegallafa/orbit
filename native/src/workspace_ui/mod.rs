//! Workspace pages: the dashboard (`DashboardPage.tsx`), a workspace's page
//! (`WorkspaceDetailPage.tsx`: delivery flow, branch drift, repos, PRs) and a
//! repository's page (`RepoPage.tsx`), plus the create-workspace and
//! add-repositories forms. The git flows they open (commit, push, rebase,
//! merge) are in `crate::git`; each page tab shows them as an in-tab modal.
pub mod dashboard;
pub mod forms;
pub mod icons;
pub mod kit;
pub mod plan;
pub mod race;
pub mod repo_page;
pub mod workspace_page;

use crate::app::{OrbitApp, Tab, TabKind};
use crate::commands::{self, Command};
use crate::git::{self, Flow};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, App, Context, FocusHandle, Focusable, SharedString, Task, WeakEntity, Window};
use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Duration;

pub enum Page {
    Dashboard(dashboard::Dashboard),
    Workspace(workspace_page::WsPage),
    Repo(repo_page::RepoView),
}

/// A form or git flow shown over the page.
pub enum Modal {
    Create(Box<forms::CreateForm>),
    AddRepos(forms::AddReposForm),
    Flow(Flow),
    Plan(Box<plan::PlanForm>),
    Race(Box<race::RaceForm>),
}

/// Work for the tab to run later.
pub type Deferred = Box<dyn FnOnce(&mut PageTab, &mut Window, &mut Context<PageTab>)>;

pub struct PageTab {
    pub page: Page,
    pub modal: Option<Modal>,
    /// Runs once the workspace has been read (a command opened the page with it).
    pub pending: Option<Deferred>,
    pub app: WeakEntity<OrbitApp>,
    /// The page body's width on the last frame (the dashboard's grid columns).
    pub width: Rc<Cell<f32>>,
    focus: FocusHandle,
    _poll: Task<()>,
    _plan_poll: Task<()>,
}

impl PageTab {
    fn new(app: WeakEntity<OrbitApp>, page: Page, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The Tauri pages fetch every minute while open.
        let poll = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(Duration::from_secs(60)).await;
            if this.update_in(cx, |t, w, cx| t.reload(true, w, cx)).is_err() {
                break;
            }
        });
        // Agents tick PLAN.md as they work: re-read it every 5 s.
        let plan_poll = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            if this.update_in(cx, |t, w, cx| t.load_plan(w, cx)).is_err() {
                break;
            }
        });
        let mut tab = Self {
            page,
            modal: None,
            pending: None,
            app,
            width: Rc::new(Cell::new(1200.)),
            focus: cx.focus_handle(),
            _poll: poll,
            _plan_poll: plan_poll,
        };
        if let Page::Repo(r) = &tab.page {
            let (a, b, c) = (r.new_branch.clone(), r.filter.clone(), r.summary.clone());
            watch(&a, window, cx);
            watch(&b, window, cx);
            watch(&c, window, cx);
        }
        tab.reload(false, window, cx);
        tab
    }

    pub fn title(&self) -> SharedString {
        match &self.page {
            Page::Dashboard(_) => "Dashboard".into(),
            Page::Workspace(p) => format!("Workspace · {}", p.name).into(),
            Page::Repo(r) => format!("Repository · {}", r.name).into(),
        }
    }

    pub fn key(&self) -> String {
        page_key(&self.page_id())
    }

    fn page_id(&self) -> PageId {
        match &self.page {
            Page::Dashboard(_) => PageId::Dashboard,
            Page::Workspace(p) => PageId::Workspace(p.name.clone()),
            Page::Repo(r) => PageId::Repo(r.name.clone()),
        }
    }

    /// Re-reads the page; `fetch` also asks the remotes (and GitHub).
    pub fn reload(&mut self, fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        match &self.page {
            Page::Dashboard(_) => self.load_dashboard(window, cx),
            Page::Workspace(_) => self.load_workspace(fetch, window, cx),
            Page::Repo(_) => self.load_repo(fetch, window, cx),
        }
    }

    /// Runs `f` on the root view (tabs, toasts, dialogs), once this tab's
    /// update is over: the app reads every tab, this one included.
    pub fn with_app(&self, window: &mut Window, cx: &mut App, f: impl FnOnce(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static) {
        let app = self.app.clone();
        window.defer(cx, move |window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |a, cx| f(a, window, cx));
            }
        });
    }

    pub fn toast(&self, kind: ToastKind, title: impl Into<SharedString>, description: Option<String>, window: &mut Window, cx: &mut App) {
        let title = title.into();
        self.with_app(window, cx, move |a, _, cx| a.toast(kind, title, description.map(Into::into), cx));
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal = None;
        cx.notify();
    }

    pub fn flow_mut(&mut self) -> Option<&mut Flow> {
        match &mut self.modal {
            Some(Modal::Flow(f)) => Some(f),
            _ => None,
        }
    }
}

/// Runs `work` off the UI thread, then `done` on the tab.
pub fn spawn<T: Send + 'static>(
    window: &mut Window,
    cx: &mut Context<PageTab>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(&mut PageTab, T, &mut Window, &mut Context<PageTab>) + 'static,
) {
    cx.spawn_in(window, async move |this, cx| {
        let r = cx.background_executor().spawn(async move { work() }).await;
        let _ = this.update_in(cx, |t, w, cx| {
            done(t, r, w, cx);
            cx.notify();
        });
    })
    .detach();
}

impl Focusable for PageTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PageTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.page {
            Page::Dashboard(_) => self.render_dashboard(cx).into_any_element(),
            Page::Workspace(_) => self.render_workspace(cx).into_any_element(),
            Page::Repo(_) => self.render_repo(window, cx).into_any_element(),
        };
        let modal = match &self.modal {
            None => None,
            Some(Modal::Create(_)) => Some(self.render_create(cx).into_any_element()),
            Some(Modal::AddRepos(_)) => Some(self.render_add_repos(cx).into_any_element()),
            Some(Modal::Flow(_)) => Some(self.render_flow(cx).into_any_element()),
            Some(Modal::Plan(_)) => Some(self.render_plan(cx).into_any_element()),
            Some(Modal::Race(_)) => Some(self.render_race(cx).into_any_element()),
        };
        div()
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .font_family(theme::UI_FONT)
            .text_size(px(theme::size::FONT))
            .line_height(gpui::relative(1.36))
            .text_color(c(theme::TEXT))
            .child(body)
            .when_some(modal, |d, m| d.child(git::scrim(m)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageId {
    Dashboard,
    Workspace(String),
    Repo(String),
}

fn page_key(id: &PageId) -> String {
    match id {
        PageId::Dashboard => "page:dashboard".into(),
        PageId::Workspace(n) => format!("page:ws:{n}"),
        PageId::Repo(n) => format!("page:repo:{n}"),
    }
}

/// Opens (or shows and refreshes) a page.
pub fn open(app: &mut OrbitApp, id: PageId, window: &mut Window, cx: &mut Context<OrbitApp>) -> gpui::Entity<PageTab> {
    if let Some(ix) = app.tab_index(&page_key(&id), cx) {
        if let TabKind::Page(t) = &app.tabs[ix].kind {
            let t = t.clone();
            t.update(cx, |t, cx| t.reload(false, window, cx));
            app.activate(ix, window, cx);
            return t;
        }
    }
    let page = match &id {
        PageId::Dashboard => Page::Dashboard(dashboard::Dashboard::default()),
        PageId::Workspace(n) => Page::Workspace(workspace_page::WsPage::new(n)),
        PageId::Repo(n) => Page::Repo(repo_page::RepoView::new(n, window, cx)),
    };
    let weak = cx.entity().downgrade();
    let view = cx.new(|cx| PageTab::new(weak, page, window, cx));
    cx.observe(&view, |_, _, cx| cx.notify()).detach();
    let handle = view.focus_handle(cx);
    let ix = app.insert_tab(Tab { kind: TabKind::Page(view.clone()), preview: false });
    app.activate(ix, window, cx);
    window.on_next_frame(move |window, cx| window.focus(&handle, cx));
    view
}

/// Re-renders the tab as `input` changes (the page reads its value).
pub fn watch<T: gpui::EventEmitter<gpui_component::input::InputEvent>>(input: &gpui::Entity<T>, window: &mut Window, cx: &mut Context<PageTab>) {
    cx.subscribe_in(input, window, |_, _, ev: &gpui_component::input::InputEvent, _, cx| {
        if matches!(ev, gpui_component::input::InputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
}

/// Records the width of the element it is placed in (absolutely, full size).
pub fn measure(cell: Rc<Cell<f32>>) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            let w = f32::from(bounds.size.width);
            if (cell.get() - w).abs() > 0.5 {
                cell.set(w);
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Selects workspace `name` and opens its page.
pub fn open_workspace(app: &mut OrbitApp, name: &str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    if let Some(ix) = app.workspaces.iter().position(|ws| ws.meta.name == name) {
        app.select_workspace(ix, cx);
    }
    open(app, PageId::Workspace(name.to_string()), window, cx);
}

/// Runs command `id` with workspace `name` selected.
pub fn run_for(app: &mut OrbitApp, name: &str, id: &str, window: &mut Window, cx: &mut Context<OrbitApp>) {
    if let Some(ix) = app.workspaces.iter().position(|ws| ws.meta.name == name) {
        if app.selected != Some(ix) {
            app.select_workspace(ix, cx);
        }
    }
    commands::run(id, app, window, cx);
}

/// The selected workspace's page, with `modal` opened over it.
fn open_ws_with(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>, modal: impl FnOnce(&mut PageTab, &mut Window, &mut Context<PageTab>) + 'static) {
    let Some(name) = app.workspace().map(|w| w.meta.name.clone()) else {
        return app.toast(ToastKind::Info, "Pick a workspace first", None, cx);
    };
    let page = open(app, PageId::Workspace(name), window, cx);
    page.update(cx, |t, cx| match &t.page {
        Page::Workspace(p) if p.meta.is_none() || p.statuses.ready().is_none() => t.pending = Some(Box::new(modal)),
        _ => modal(t, window, cx),
    });
}

/// Re-reads the workspace list into the app, keeping (or making) `select` the selection.
pub fn reload_workspaces(app: &mut OrbitApp, select: Option<&str>, cx: &mut Context<OrbitApp>) {
    let current = select.map(str::to_string).or_else(|| app.workspace().map(|w| w.meta.name.clone()));
    app.workspaces = crate::workspace::list_workspaces();
    match current.and_then(|n| app.workspaces.iter().position(|w| w.meta.name == n)) {
        // Re-selecting also refreshes the tree's roots (new repos).
        Some(ix) => app.select_workspace(ix, cx),
        None => {
            app.selected = None;
            cx.notify();
        }
    }
}

pub fn init(cx: &mut App) {
    let has_ws = |a: &OrbitApp, _: &App| a.workspace().is_some();
    for command in [
        Command::new("workspace.dashboard", "Open Dashboard").run(|a, w, cx| {
            open(a, PageId::Dashboard, w, cx);
        }),
        Command::new("workspace.overview", "Open Workspace Page").when(has_ws).run(|a, w, cx| open_ws_with(a, w, cx, |_, _, _| {})),
        Command::new("workspace.new", "New Workspace…").run(|a, w, cx| {
            let page = open(a, PageId::Dashboard, w, cx);
            page.update(cx, |t, cx| t.open_create(None, w, cx));
        }),
        Command::new("workspace.addRepos", "Add Repositories to Workspace…")
            .when(has_ws)
            .run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.open_add_repos(w, cx))),
        Command::new("workspace.remove", "Remove Workspace…")
            .when(has_ws)
            .run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.confirm_remove(false, w, cx))),
        Command::new("git.commit", "Commit…").when(has_ws).run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.start_commit(w, cx))),
        Command::new("git.push", "Push…").when(has_ws).run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.start_push(w, cx))),
        Command::new("git.rebase", "Rebase onto Base Branch…").when(has_ws).run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.start_rebase(w, cx))),
        Command::new("git.merge", "Squash and Merge Pull Requests…")
            .when(has_ws)
            .run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.start_merge(w, cx))),
        Command::new("git.syncBranches", "Switch Repositories Back to the Workspace Branch")
            .when(has_ws)
            .run(|a, w, cx| open_ws_with(a, w, cx, |t, w, cx| t.sync_drift(false, w, cx))),
        Command::new("repo.open", "Open Repository…").run(|a, w, cx| {
            let names: Vec<String> = orbit_core::config::Config::load().map(|c| c.services.into_iter().map(|s| s.name).collect()).unwrap_or_default();
            if names.is_empty() {
                return a.toast(ToastKind::Info, "No repositories configured", None, cx);
            }
            let items = names
                .into_iter()
                .map(|n| {
                    crate::overlay::MenuItem::new(n.clone(), move |a: &mut OrbitApp, w: &mut Window, cx: &mut Context<OrbitApp>| {
                        open(a, PageId::Repo(n.clone()), w, cx);
                    })
                })
                .collect();
            a.open_menu(gpui::point(px(420.), px(90.)), items, w, cx);
        }),
    ] {
        commands::register(cx, command);
    }
}

/// The pages' part of the self-test dump.
pub fn dump(app: &OrbitApp, cx: &App) -> String {
    let mut out = String::new();
    for tab in &app.tabs {
        if let TabKind::Page(p) = &tab.kind {
            let p = p.read(cx);
            let _ = writeln!(out, "--- page {}", p.title());
            match &p.page {
                Page::Dashboard(d) => d.dump(&mut out),
                Page::Workspace(w) => w.dump(&mut out),
                Page::Repo(r) => r.dump(&mut out, cx),
            }
            match &p.modal {
                Some(Modal::Create(f)) => f.dump(&mut out, cx),
                Some(Modal::AddRepos(f)) => f.dump(&mut out),
                Some(Modal::Flow(f)) => f.dump(&mut out, cx),
                Some(Modal::Plan(f)) => f.dump(&mut out, cx),
                Some(Modal::Race(f)) => f.dump(&mut out, cx),
                None => {}
            }
        }
    }
    out
}

pub fn load_text<T>(l: &Load<T>) -> String {
    match l {
        Load::Idle => "idle".into(),
        Load::Loading => "loading".into(),
        Load::Ready(_) => "ready".into(),
        Load::Failed(e) => format!("failed: {e}"),
    }
}

/// Plural helper: `n repo` / `n repos`.
pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
