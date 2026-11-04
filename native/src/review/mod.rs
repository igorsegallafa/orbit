//! Review: a workspace's local changes with REVIEW.md notes (`ChangesReview.tsx`)
//! and its pull requests with status, checks and failed-check logs
//! (`PrsModal.tsx`, the workspace page's PR tracker and checks). Each repo's
//! changed file opens as a diff tab (`diff_view.rs`), where notes are left and
//! answered inline.
mod changes;
pub mod code_review;
mod feature;
mod pr_review;
mod pr_ui;
pub mod notes;
mod prs;
mod send;
pub mod ui;

use crate::app::{OrbitApp, Tab, TabKind};
use crate::commands::{self, Command};
use crate::diff_view::DiffTab;
use crate::probe::probe;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, SharedString, Task, WeakEntity, Window,
};
use orbit_core::git::ChangeEntry;
use orbit_core::github::{PullRequest, WsPrStatus};
use orbit_core::notes::NotesFile;
use orbit_core::prs::WsCheck;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

pub use code_review::open_code_review;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Page {
    Changes,
    PullRequests,
}

impl Page {
    fn slug(self) -> &'static str {
        match self {
            Page::Changes => "changes",
            Page::PullRequests => "prs",
        }
    }
}

/// Something read in the background.
#[derive(Clone, Debug, Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}

pub struct ReviewTab {
    pub page: Page,
    pub workspace: String,
    pub dir: PathBuf,
    pub repos: Vec<String>,
    pub branch: String,
    pub base: String,
    pub app: WeakEntity<OrbitApp>,
    focus: FocusHandle,
    // Changes
    pub changes: Vec<(String, Load<Vec<ChangeEntry>>)>,
    pub notes: Option<NotesFile>,
    pub pr_refs: Vec<orbit_core::workspace::PrRef>,
    changes_busy: bool,
    pub show_done: bool,
    pub message: Option<String>,
    // Pull requests
    pub prs: Load<Vec<PullRequest>>,
    pub status: Load<Vec<WsPrStatus>>,
    pub checks: Load<Vec<WsCheck>>,
    pub log: Option<(String, Load<String>)>,
    pub create: Vec<prs::CreateRow>,
    pub creating: bool,
    pub sending: bool,
    _poll: Task<()>,
}

impl ReviewTab {
    fn new(app: WeakEntity<OrbitApp>, ws: &Workspace, page: Page, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repos = ws.meta.repos.clone();
        // Changes follow the disk; PR data is asked for on open and refresh.
        let poll = cx.spawn(async move |this: WeakEntity<Self>, cx| loop {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            if this.update(cx, |t, cx| t.poll(cx)).is_err() {
                break;
            }
        });
        let mut tab = Self {
            page,
            workspace: ws.meta.name.clone(),
            dir: ws.dir.clone(),
            changes: repos.iter().map(|r| (r.clone(), Load::Idle)).collect(),
            repos,
            branch: ws.meta.branch.clone(),
            base: ws.meta.base.clone(),
            app,
            focus: cx.focus_handle(),
            notes: None,
            pr_refs: vec![],
            changes_busy: false,
            show_done: false,
            message: None,
            prs: Load::Idle,
            status: Load::Idle,
            checks: Load::Idle,
            log: None,
            create: vec![],
            creating: false,
            sending: false,
            _poll: poll,
        };
        match page {
            Page::Changes => tab.refresh_changes(cx),
            Page::PullRequests => tab.refresh_prs(window, cx),
        }
        tab
    }

    pub fn title(&self) -> SharedString {
        match self.page {
            Page::Changes => format!("Changes · {}", self.workspace).into(),
            Page::PullRequests => format!("Pull requests · {}", self.workspace).into(),
        }
    }

    pub fn key(&self) -> String {
        format!("review:{}:{}", self.page.slug(), self.workspace)
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Changes {
            self.refresh_changes(cx);
        }
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            Page::Changes => self.refresh_changes(cx),
            Page::PullRequests => self.refresh_prs(window, cx),
        }
    }

    /// Runs `f` on the root view (opening tabs, menus, dialogs).
    pub fn with_app(&self, window: &mut Window, cx: &mut App, f: impl FnOnce(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>)) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |a, cx| f(a, window, cx));
        }
    }
}

impl Focusable for ReviewTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReviewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Changes => self.render_changes(window, cx).into_any_element(),
            Page::PullRequests => self.render_prs(window, cx).into_any_element(),
        };
        div().track_focus(&self.focus).size_full().flex().flex_col().bg(c(theme::BG)).font_family(theme::UI_FONT).child(body)
    }
}

/// Opens (or shows) the selected workspace's review page.
pub fn open(app: &mut OrbitApp, page: Page, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(ws) = app.workspace().cloned() else {
        return app.toast(crate::toast::ToastKind::Info, "Pick a workspace first", None, cx);
    };
    let key = format!("review:{}:{}", page.slug(), ws.meta.name);
    if let Some(ix) = app.tab_index(&key, cx) {
        if let TabKind::Review(t) = &app.tabs[ix].kind {
            t.update(cx, |t, cx| t.refresh(window, cx));
        }
        return app.activate(ix, window, cx);
    }
    let weak = cx.entity().downgrade();
    let view = cx.new(|cx| ReviewTab::new(weak, &ws, page, window, cx));
    cx.observe(&view, |_, _, cx| cx.notify()).detach();
    let handle = view.focus_handle(cx);
    let ix = app.insert_tab(Tab { kind: TabKind::Review(view), preview: false });
    app.activate(ix, window, cx);
    focus_next_frame(handle, window);
}

/// A tab opened from a click is focused once it has been drawn: a handle
/// focused before that loses the focus to the click's target.
fn focus_next_frame(handle: FocusHandle, window: &mut Window) {
    window.on_next_frame(move |window, cx| window.focus(&handle, cx));
}

/// The diff tab of `path`, opened at `line` when given.
pub fn open_diff_at(app: &mut OrbitApp, path: PathBuf, line: Option<u32>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(d) = app.open_diff(path, window, cx) else { return };
    if let Some(line) = line {
        d.update(cx, |d, cx| d.reveal_line(line, cx));
    }
    focus_next_frame(d.read(cx).state.focus_handle(cx), window);
}

fn active_diff(app: &OrbitApp) -> Option<Entity<DiffTab>> {
    match &app.active_tab()?.kind {
        TabKind::Diff(d) => Some(d.clone()),
        _ => None,
    }
}

/// The review's commands and keys.
pub fn init(cx: &mut App) {
    let has_ws = |a: &OrbitApp, _: &App| a.workspace().is_some();
    let in_diff = |a: &OrbitApp, _: &App| active_diff(a).is_some();
    for command in [
        Command::new("review.changes", "Review Changes").when(has_ws).run(|a, w, cx| open(a, Page::Changes, w, cx)),
        Command::new("review.codeReview", "Code Review").run(open_code_review),
        Command::new("review.pullRequests", "Pull Requests and Checks").when(has_ws).run(|a, w, cx| open(a, Page::PullRequests, w, cx)),
        Command::new("diff.nextChange", "Next Change").keys(["f7"]).when(in_diff).run(|a, _, cx| {
            if let Some(d) = active_diff(a) {
                d.update(cx, |d, cx| d.next_change(true, cx));
            }
        }),
        Command::new("diff.previousChange", "Previous Change").keys(["shift-f7"]).when(in_diff).run(|a, _, cx| {
            if let Some(d) = active_diff(a) {
                d.update(cx, |d, cx| d.next_change(false, cx));
            }
        }),
        Command::new("diff.toggleMode", "Toggle Split / Unified Diff").when(in_diff).run(|a, _, cx| {
            if let Some(d) = active_diff(a) {
                d.update(cx, |d, cx| d.toggle_mode(cx));
            }
        }),
        Command::new("review.commentSelection", "Add Review Note on Selected Lines")
            .when(move |a, cx| active_diff(a).is_some_and(|d| d.read(cx).review.is_some()))
            .run(|a, w, cx| {
                if let Some(d) = active_diff(a) {
                    d.update(cx, |d, cx| d.comment_on_selection(w, cx));
                }
            }),
    ] {
        commands::register(cx, command);
    }
}

/// The sidebar's entries for the selected workspace.
pub fn sidebar(app: &OrbitApp, cx: &mut Context<OrbitApp>) -> impl IntoElement {
    let entry = |id: &'static str, label: &'static str, page: Page, cx: &mut Context<OrbitApp>| {
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
            .on_click(cx.listener(move |a, _, w, cx| open(a, page, w, cx)))
    };
    div().when(app.workspace().is_some(), |d| {
        d.child(
            div()
                .px_3()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c(theme::TEXT_FAINT))
                .child("REVIEW"),
        )
        .child(entry("review-changes", "Changes", Page::Changes, cx))
        .child(entry("review-prs", "Pull requests", Page::PullRequests, cx))
    })
}

/// The review part of the self-test dump.
pub fn dump(app: &OrbitApp, cx: &App) -> String {
    let mut out = String::new();
    for tab in &app.tabs {
        match &tab.kind {
            TabKind::Review(t) => t.read(cx).dump(&mut out, cx),
            TabKind::Diff(d) => {
                let d = d.read(cx);
                let Some(r) = &d.review else { continue };
                let _ = writeln!(out, "--- diff review {} ({} / {}): {} notes", r.at.rel, r.at.workspace, r.at.repo, r.notes.len());
                for (n, at) in r.shown() {
                    let _ = writeln!(
                        out,
                        "  note {}-{}{}{}{} {:?}",
                        at.start,
                        at.end,
                        if n.done { " done" } else { "" },
                        if at.moved { " moved" } else { "" },
                        if at.lost { " lost" } else { "" },
                        n.body
                    );
                }
                if let Some(cm) = &d.composer {
                    let _ = writeln!(out, "  composer {:?}: {:?}", cm.kind, cm.input.read(cx).value());
                }
                if let Some(m) = &d.message {
                    let _ = writeln!(out, "  message: {m}");
                }
            }
            _ => {}
        }
    }
    out
}

impl ReviewTab {
    fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "--- review {}{}", self.title(), self.message.as_ref().map(|m| format!(" (message: {m})")).unwrap_or_default());
        match self.page {
            Page::Changes => {
                for (repo, load) in &self.changes {
                    match load {
                        Load::Ready(list) => {
                            for e in list {
                                let _ = writeln!(out, "  {repo}: {} {} +{} -{}", e.status, e.path, e.added, e.deleted);
                            }
                        }
                        other => {
                            let _ = writeln!(out, "  {repo}: {}", load_label(other));
                        }
                    }
                }
                if let Some(f) = &self.notes {
                    let _ = writeln!(out, "  notes file {} ({} notes, {} open)", f.file, f.notes.len(), f.notes.iter().filter(|n| !n.done).count());
                    for n in &f.notes {
                        let _ = writeln!(out, "    [{}] {}/{}:{}-{} {:?}", if n.done { "x" } else { " " }, n.repo, n.path, n.start, n.end, n.body);
                    }
                }
            }
            Page::PullRequests => {
                let _ = writeln!(out, "  prs: {}", load_label(&self.prs));
                for p in self.prs.ready().into_iter().flatten() {
                    let _ = writeln!(out, "    {} #{} {} [{}] {}", p.repo, p.number, p.title, p.branch, p.url);
                }
                let _ = writeln!(out, "  status: {}", load_label(&self.status));
                for s in self.status.ready().into_iter().flatten() {
                    let _ = writeln!(out, "    {} #{} {} {}{}", s.repo, s.number, s.state, s.title, if s.is_draft { " (draft)" } else { "" });
                }
                let _ = writeln!(out, "  checks: {}", load_label(&self.checks));
                for w in self.checks.ready().into_iter().flatten() {
                    let _ = writeln!(out, "    {} #{} {}", w.repo, w.pr_number, w.status);
                    for ch in &w.checks {
                        let _ = writeln!(out, "      {} {} {}", ch.bucket, ch.name, ch.link);
                    }
                }
                for row in &self.create {
                    let _ = writeln!(out, "  create {} include {} title {:?}", row.repo, row.include, row.title.read(cx).value());
                }
                if let Some((title, log)) = &self.log {
                    let text = match log {
                        Load::Ready(t) => format!("{} lines, first {:?}", t.lines().count(), t.lines().next().unwrap_or_default()),
                        other => load_label(other),
                    };
                    let _ = writeln!(out, "  log {title}: {text}");
                }
            }
        }
    }
}

fn load_label<T>(l: &Load<T>) -> String {
    match l {
        Load::Idle => "idle".into(),
        Load::Loading => "loading".into(),
        Load::Ready(_) => "ready".into(),
        Load::Failed(e) => format!("failed: {e}"),
    }
}

/// A header bar with a title and actions on the right.
fn toolbar(title: impl IntoElement) -> gpui::Div {
    div()
        .h(px(34.))
        .flex_none()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(c(theme::TEXT))
        .border_b_1()
        .border_color(c(theme::BORDER))
        .child(title)
        .child(div().flex_1())
}
