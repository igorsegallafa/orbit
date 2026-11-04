//! A repository's page (`RepoPage.tsx`), on its base clone: the header
//! tools, the branch / sync / pull request bar, banners (paused operation,
//! branch deleted on origin, a workspace holding the clone), Changes with
//! the commit box, History, Branches, and the workspaces, worktrees and
//! stashes that use it.
use super::icons::icon;
use super::kit::{self, Kind};
use super::{plural, spawn, Page, PageTab};
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::probe::probe;
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, AnyElement, App, ClickEvent, Context, Div, Entity, Focusable, FontWeight, Window};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::Sizable;
use orbit_core::git::{ChangeEntry, CommitEntry};
use orbit_core::github::WsPrStatus;
use orbit_core::repo::{self, BranchInfo, RepoOverview};
use orbit_core::workspace::Workspace;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::rc::Rc;

/// A deferred action on the page (dialog buttons, toolbar buttons).
type Action = Rc<dyn Fn(&mut PageTab, &mut Window, &mut Context<PageTab>)>;

pub struct RepoView {
    pub name: String,
    pub overview: Load<RepoOverview>,
    pub history: Load<Vec<CommitEntry>>,
    pub changes: Load<Vec<ChangeEntry>>,
    /// Files left out of the commit.
    pub excluded: BTreeSet<String>,
    pub prs: Option<Vec<WsPrStatus>>,
    pub workspaces: Vec<Workspace>,
    pub busy: Option<String>,
    pub fetching: bool,
    pub drafting: bool,
    pub switcher: bool,
    pub show_remote: bool,
    /// The branch switcher's filter / new branch name.
    pub new_branch: Entity<InputState>,
    pub filter: Entity<InputState>,
    pub summary: Entity<InputState>,
    pub description: Entity<TextareaState>,
}

impl RepoView {
    pub fn new(name: &str, window: &mut Window, cx: &mut App) -> Self {
        Self {
            name: name.to_string(),
            overview: Load::Loading,
            history: Load::Loading,
            changes: Load::Loading,
            excluded: BTreeSet::new(),
            prs: None,
            workspaces: vec![],
            busy: None,
            fetching: false,
            drafting: false,
            switcher: false,
            show_remote: false,
            new_branch: cx.new(|cx| InputState::new(window, cx).placeholder("Filter or create a branch…")),
            filter: cx.new(|cx| InputState::new(window, cx).placeholder("Filter")),
            summary: cx.new(|cx| InputState::new(window, cx).placeholder("Summary (required)")),
            description: cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6).placeholder("Description")),
        }
    }

    fn selected(&self) -> Vec<String> {
        self.changes.ready().into_iter().flatten().filter(|c| !self.excluded.contains(&c.path)).map(|c| c.path.clone()).collect()
    }

    /// The workspace whose branch the base clone is on.
    fn holder(&self) -> Option<&Workspace> {
        let branch = self.overview.ready()?.branch.as_ref()?;
        self.workspaces.iter().find(|w| &w.branch == branch && w.repos.contains(&self.name))
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "  overview: {}{}", super::load_text(&self.overview), self.busy.as_ref().map(|b| format!(" (busy: {b})")).unwrap_or_default());
        if let Some(o) = self.overview.ready() {
            let _ = writeln!(
                out,
                "    on {} upstream {} ahead {} behind {} changes {} operation {:?} conflicts {:?}",
                o.branch.as_deref().unwrap_or("-"),
                o.upstream.as_deref().unwrap_or("-"),
                o.ahead,
                o.behind,
                o.changes,
                o.operation,
                o.conflicts
            );
            for b in &o.branches {
                let _ = writeln!(
                    out,
                    "    branch {}{}{} ↑{} ↓{}{}",
                    b.name,
                    if b.current { " (current)" } else { "" },
                    if b.remote_only { " (remote)" } else { "" },
                    b.ahead,
                    b.behind,
                    b.worktree.as_ref().map(|w| format!(" worktree {w}")).unwrap_or_default()
                );
            }
            for s in &o.stashes {
                let _ = writeln!(out, "    stash {} {}", s.index, s.message);
            }
            for w in &o.worktrees {
                let _ = writeln!(out, "    worktree {} {:?} ws {:?}{}", w.path, w.branch, w.workspace, if w.missing { " missing" } else { "" });
            }
        }
        for ch in self.changes.ready().into_iter().flatten() {
            let _ = writeln!(out, "    change {} {}{}", ch.status, ch.path, if self.excluded.contains(&ch.path) { " (excluded)" } else { "" });
        }
        for c in self.history.ready().into_iter().flatten().take(5) {
            let _ = writeln!(out, "    commit {} {}", c.sha, c.message);
        }
        let _ = writeln!(out, "    new branch input {:?} summary {:?}", self.new_branch.read(cx).value(), self.summary.read(cx).value());
    }
}

/// What the sync cell offers (`syncAction` in RepoPage.tsx).
enum Sync {
    Fetch,
    Publish,
    Pull { rebase: bool },
    Push,
}

fn ago(unix: Option<i64>) -> String {
    let Some(t) = unix else { return "Never fetched".into() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let s = (now - t).max(0);
    let when = match s {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    };
    format!("Last fetched {when}")
}

impl PageTab {
    fn repo_view(&mut self) -> Option<&mut RepoView> {
        match &mut self.page {
            Page::Repo(r) => Some(r),
            _ => None,
        }
    }

    pub fn load_repo(&mut self, fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        if fetch {
            r.fetching = true;
        }
        let name = r.name.clone();
        spawn(
            window,
            cx,
            move || {
                if fetch {
                    let _ = repo::fetch(&name);
                }
                let ov = repo::overview(&name);
                let changes = ov.as_ref().map_err(Clone::clone).and_then(|o| orbit_core::git::changes(std::path::Path::new(&o.path)));
                let workspaces = orbit_core::workspace::list().unwrap_or_default().into_iter().filter(|w| w.repos.contains(&name)).collect::<Vec<_>>();
                (ov, repo::history(&name, 40), changes, workspaces, repo::branch_prs(&name).ok())
            },
            |t, (ov, hist, changes, workspaces, prs), _, _| {
                if let Some(r) = t.repo_view() {
                    r.fetching = false;
                    r.overview = ov.map_or_else(Load::Failed, Load::Ready);
                    r.history = hist.map_or_else(Load::Failed, Load::Ready);
                    if let Ok(list) = &changes {
                        r.excluded.retain(|p| list.iter().any(|c| &c.path == p));
                    }
                    r.changes = changes.map_or_else(Load::Failed, Load::Ready);
                    r.workspaces = workspaces;
                    r.prs = prs;
                }
            },
        );
        cx.notify();
    }

    /// Runs one git action on the clone, then reloads; `ok` is the success toast.
    fn repo_run(&mut self, label: &str, ok: Option<String>, op: impl FnOnce(&str) -> Result<(), String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        if r.busy.is_some() {
            return;
        }
        r.busy = Some(label.to_string());
        let (name, label) = (r.name.clone(), label.to_string());
        spawn(window, cx, move || op(&name), move |t, res, window, cx| {
            if let Some(r) = t.repo_view() {
                r.busy = None;
            }
            match res {
                Ok(()) => t.toast(ToastKind::Success, ok.unwrap_or_else(|| format!("{label}: done")), None, window, cx),
                Err(e) => t.toast(ToastKind::Error, format!("{label} failed"), Some(e), window, cx),
            }
            t.load_repo(false, window, cx);
        });
        cx.notify();
    }

    /// Asks with `buttons` (label, action); Cancel is added first.
    fn repo_ask(&mut self, title: String, message: String, buttons: Vec<(&'static str, Action)>, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let mut list = vec![DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) }];
        let n = buttons.len();
        for (i, (label, f)) in buttons.into_iter().enumerate() {
            let page = page.clone();
            list.push(DialogButton {
                label: label.into(),
                style: if i + 1 == n { ButtonStyle::Primary } else { ButtonStyle::Secondary },
                run: Rc::new(move |_: &mut crate::app::OrbitApp, w: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
                    let f = f.clone();
                    let _ = page.update(cx, |t, cx| f(t, w, cx));
                }),
            });
        }
        self.with_app(window, cx, move |a, w, cx| {
            a.open_dialog(Dialog { title: title.into(), message: message.into(), buttons: list, on_cancel: Rc::new(|_, _, _| {}) }, w, cx)
        });
    }

    fn repo_switch(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(changes) = self.repo_view().and_then(|r| {
            r.switcher = false;
            r.overview.ready().map(|o| o.changes)
        }) else {
            return;
        };
        let go = |stash: bool, branch: String| -> Action {
            Rc::new(move |t, w, cx| {
                let b = branch.clone();
                let ok = if stash { format!("Switched to {b}; your changes are stashed") } else { format!("Switched to {b}") };
                t.repo_run("Switch branch", Some(ok), move |n| repo::switch(n, &b, stash), w, cx)
            })
        };
        if changes == 0 {
            return go(false, branch)(self, window, cx);
        }
        self.repo_ask(
            format!("Switch to {branch}"),
            format!("You have {}. Bring them along, or stash them here?", plural(changes, "uncommitted change")),
            vec![("Stash and switch", go(true, branch.clone())), ("Bring changes", go(false, branch))],
            window,
            cx,
        );
    }

    fn repo_create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        let name = r.new_branch.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        r.switcher = false;
        r.new_branch.update(cx, |i, cx| i.set_value("", window, cx));
        let b = name.clone();
        self.repo_run("Create branch", Some(format!("Created and switched to {name}")), move |n| repo::create_branch(n, &b, None), window, cx);
    }

    fn repo_delete(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        let b = branch.clone();
        let del: Action = Rc::new(move |t, w, cx| {
            let b2 = b.clone();
            t.repo_run("Delete branch", Some(format!("Deleted {b}")), move |n| repo::delete_branch(n, &b2, true), w, cx)
        });
        self.repo_ask("Delete branch".into(), format!("Delete {branch}? Commits only on it are lost."), vec![("Delete", del)], window, cx)
    }

    fn repo_discard(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let what = if paths.len() == 1 { paths[0].clone() } else { plural(paths.len(), "file") };
        let go: Action = Rc::new(move |t, w, cx| {
            let p = paths.clone();
            t.repo_run("Discard", Some("Changes discarded".into()), move |n| repo::discard(n, &p), w, cx)
        });
        self.repo_ask("Discard changes".into(), format!("Discard the changes to {what}? New files go to the Trash."), vec![("Discard", go)], window, cx)
    }

    fn repo_commit_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        let summary = r.summary.read(cx).value().trim().to_string();
        let description = r.description.read(cx).value().trim().to_string();
        let paths = r.selected();
        if summary.is_empty() || paths.is_empty() {
            return;
        }
        let message = if description.is_empty() { summary.clone() } else { format!("{summary}\n\n{description}") };
        r.summary.update(cx, |i, cx| i.set_value("", window, cx));
        r.description.update(cx, |i, cx| i.set_value("", window, cx));
        self.repo_run("Commit", Some(format!("Committed \"{summary}\"")), move |n| repo::commit(n, &message, &paths), window, cx);
    }

    fn repo_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        let paths = r.selected();
        if paths.is_empty() || r.drafting {
            return;
        }
        r.drafting = true;
        let name = r.name.clone();
        spawn(window, cx, move || orbit_core::agent::commit_message(&format!("@{name}"), &name, &paths), |t, res, window, cx| {
            let Some(r) = t.repo_view() else { return };
            r.drafting = false;
            match res {
                Ok(m) => {
                    let (head, rest) = m.message.split_once('\n').unwrap_or((m.message.as_str(), ""));
                    let (head, rest) = (head.trim().to_string(), rest.trim().to_string());
                    r.summary.update(cx, |i, cx| i.set_value(head, window, cx));
                    r.description.update(cx, |i, cx| i.set_value(rest, window, cx));
                }
                Err(e) => t.toast(ToastKind::Error, "Couldn't draft the message", Some(e), window, cx),
            }
        });
        cx.notify();
    }

    fn repo_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.repo_view().map(|r| r.name.clone()) else { return };
        self.open_push(format!("@{name}"), vec![name], vec![], window, cx);
    }

    fn repo_open_diff(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.repo_view().and_then(|r| r.overview.ready().map(|o| o.path.clone())) else { return };
        let file = std::path::Path::new(&dir).join(path);
        self.with_app(window, cx, move |a, w, cx| crate::review::open_diff_at(a, file, None, w, cx));
    }

    fn repo_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.repo_view().and_then(|r| r.overview.ready().map(|o| o.path.clone())) else { return };
        self.with_app(window, cx, move |a, w, cx| crate::sessions::new_shell_in(a, std::path::Path::new(&dir), w, cx));
    }

    fn repo_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.repo_view().and_then(|r| r.overview.ready().map(|o| o.path.clone())) else { return };
        spawn(window, cx, move || orbit_core::settings::launch_editor(std::path::Path::new(&dir)), |t, res, window, cx| {
            if let Err(e) = res {
                t.toast(ToastKind::Error, "Couldn't open the editor", Some(e), window, cx);
            }
        });
    }

    fn repo_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.repo_view() else { return };
        let Some(ov) = r.overview.ready() else { return };
        match sync_of(ov) {
            Sync::Fetch => self.load_repo(true, window, cx),
            Sync::Pull { rebase } => self.repo_run("Pull", Some("Pulled from origin".into()), move |n| repo::pull(n, rebase), window, cx),
            Sync::Push | Sync::Publish => self.repo_push(window, cx),
        }
    }

    pub(super) fn render_repo(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let Page::Repo(r) = &self.page else { return div().into_any_element() };
        let ov = r.overview.ready();
        let wrap = div().id("repo-page").size_full().overflow_y_scroll().px(px(36.)).py(px(24.));
        if let Some(o) = ov.filter(|o| !o.cloned) {
            return wrap
                .child(header(r, Some(o), cx))
                .child(div().text_size(px(13.)).text_color(c(theme::TEXT_DIM)).child(format!("{} is not cloned yet (Settings → Repositories clones it).", r.name)))
                .into_any_element();
        }
        if let Load::Failed(e) = &r.overview {
            return wrap.child(header(r, None, cx)).child(div().text_size(px(13.)).text_color(c(theme::DANGER)).child(e.clone())).into_any_element();
        }
        wrap.child(header(r, ov, cx))
            .child(bar(r, ov, cx))
            .children(ov.map(|o| banners(r, o, cx)))
            .child(
                div()
                    .mb(px(12.))
                    .flex()
                    .gap(px(12.))
                    .child(div().flex_basis(px(0.)).flex_grow(1.25).min_w_0().child(changes_panel(r, ov, cx)))
                    .child(div().flex_basis(px(0.)).flex_grow(1.).min_w_0().child(history_panel(r, ov, cx))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(div().flex_basis(px(0.)).flex_grow(1.25).min_w_0().child(branches_panel(r, ov, cx)))
                    .child(div().flex_basis(px(0.)).flex_grow(1.).min_w_0().child(side_panel(r, ov, cx))),
            )
            .into_any_element()
    }
}

fn sync_of(ov: &RepoOverview) -> Sync {
    if ov.branch.is_some() && (ov.upstream.is_none() || ov.upstream_gone) {
        Sync::Publish
    } else if ov.behind > 0 {
        Sync::Pull { rebase: ov.ahead > 0 }
    } else if ov.ahead > 0 {
        Sync::Push
    } else {
        Sync::Fetch
    }
}

fn header(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let tool = |id: &'static str, icon_spec: (&'static str, f32), label: &'static str, kind: Kind, tip: String, f: fn(&mut PageTab, &mut Window, &mut Context<PageTab>), cx: &mut Context<PageTab>| {
        kit::hint(kit::button_icon(id, Some(icon_spec), label, kind, kit::TOOL), tip).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx)))
    };
    let name = r.name.clone();
    let path = ov.map(|o| o.path.clone());
    let refresh = if r.fetching {
        div()
            .id("repo-fetch")
            .relative()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .opacity(0.45)
            .child(kit::spinner("repo-fetch-spin", 15., c(theme::ACCENT)))
            .child(probe("repo-fetch"))
    } else {
        kit::hint(kit::icon_button("repo-fetch", "refresh", 32., 15., false), "Fetch origin and refresh").on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load_repo(true, w, cx)))
    };
    div()
        .mb(px(20.))
        .flex()
        .flex_wrap()
        .items_start()
        .justify_between()
        .gap(px(20.))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon("repo", 18., c(theme::TEXT_DIM)))
                        .child(div().text_size(px(20.25)).line_height(px(28.)).font_weight(FontWeight::BOLD).child(r.name.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.5))
                        .when_some(ov.and_then(|o| o.owner_repo.clone()), |d, or| {
                            let url = format!("https://github.com/{or}");
                            d.child(
                                div()
                                    .id("repo-gh")
                                    .h(px(22.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .rounded(px(5.))
                                    .bg(theme::accent_soft())
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(kit::mix(c(theme::INFO), c(theme::TEXT), 0.4))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(kit::alpha(c(theme::ACCENT), 0.28)))
                                    .child(icon("github", 11., kit::mix(c(theme::INFO), c(theme::TEXT), 0.4)))
                                    .child(or)
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        })
                        .when_some(path.clone(), |d, p| d.child(div().max_w(px(520.)).truncate().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(p))),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    kit::hint(kit::button_icon("repo-new-ws", Some(("plus", 14.)), "New workspace", Kind::Primary, kit::TOOL), format!("New workspace with {name}: a branch in its own worktree"))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_create(Some(vec![name.clone()]), w, cx))),
                )
                .child(tool(
                    "repo-ralph",
                    ("spark", 14.),
                    "Ralph",
                    Kind::Secondary,
                    "Write a PRD and let the agent implement it story by story on this branch".into(),
                    |t, w, cx| t.toast(ToastKind::Info, "Ralph runs from a workspace", Some("Create a workspace with this repo, then open Ralph there.".into()), w, cx),
                    cx,
                ))
                .child(tool("repo-session", ("terminal", 14.), "New session", Kind::Secondary, "Terminal session in this clone".into(), |t, w, cx| t.repo_new_session(w, cx), cx))
                .child(tool("repo-editor", ("code", 14.), "Editor", Kind::Secondary, "Open the clone in VS Code or Zed".into(), |t, w, cx| t.repo_editor(w, cx), cx))
                .child(kit::hint(kit::icon_button("repo-folder", "folder", 32., 15., false), path.clone().unwrap_or_else(|| "Reveal folder".into())).on_click(move |_, _, cx| {
                    if let Some(p) = &path {
                        if crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        cx.reveal_path(std::path::Path::new(p));
                    }
                }))
                .child(div().mx(px(4.)).w(px(1.)).h(px(22.)).bg(c(theme::BORDER)))
                .child(refresh),
        )
}

/// One cell of the top bar: icon, small label, bold value.
fn bar_cell(id: &'static str, icon_name: &'static str, label: String, value: String, primary: bool, muted: bool) -> gpui::Stateful<Div> {
    let accent = kit::mix(c(theme::INFO), c(theme::TEXT), 0.7);
    div()
        .id(id)
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(58.))
        .px(px(16.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .when(primary, |d| d.bg(gpui::linear_gradient(180., gpui::linear_color_stop(theme::accent_soft(), 0.), gpui::linear_color_stop(kit::alpha(c(theme::PANEL), 0.), 0.9))))
        .child(icon(icon_name, 15., if primary { accent } else { c(theme::TEXT_DIM) }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(if primary { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                        .text_color(if primary { accent } else { c(theme::TEXT_FAINT) })
                        .child(label),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.5))
                        .font_weight(if muted { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD })
                        .text_color(if muted { c(theme::TEXT_FAINT) } else { c(theme::TEXT) })
                        .child(value),
                ),
        )
        .child(probe(id))
}

fn bar(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let busy = r.busy.is_some();
    let branch = ov.map(|o| o.branch.clone().unwrap_or_else(|| format!("detached at {}", o.head))).unwrap_or_else(|| "…".into());
    let fetched = ago(ov.and_then(|o| o.last_fetch));
    let (sync_icon, sync_label, sync_detail, primary, sync_hint) = match ov.map(sync_of) {
        _ if r.fetching => ("refresh", "Fetch origin".to_string(), "Fetching…".to_string(), false, "Check origin for new commits".to_string()),
        None | Some(Sync::Fetch) => ("refresh", "Fetch origin".into(), fetched, false, "Check origin for new commits".into()),
        Some(Sync::Publish) => ("push", "Publish branch".into(), "Push it to origin".into(), true, format!("Push {branch} to origin and track it")),
        Some(Sync::Pull { rebase }) => {
            let o = ov.unwrap();
            (
                "download",
                format!("Pull origin ↓{}{}", o.behind, if o.ahead > 0 { format!(" ↑{}", o.ahead) } else { String::new() }),
                if rebase { "Diverged: your commits go on top".into() } else { fetched },
                true,
                if rebase { "Rebase your local commits on top of origin".into() } else { "Fast-forward to origin".into() },
            )
        }
        Some(Sync::Push) => {
            let o = ov.unwrap();
            ("push", format!("Push origin ↑{}", o.ahead), fetched, true, format!("Push {} to {}", plural(o.ahead, "commit"), o.upstream.clone().unwrap_or_default()))
        }
    };
    let sync_label = match r.busy.as_deref() {
        Some(b @ ("Pull" | "Push" | "Publish")) => format!("{b}ing…"),
        _ => sync_label,
    };
    let default_branch = ov.and_then(|o| o.default_branch.clone()).unwrap_or_else(|| "main".into());
    let on_default = ov.and_then(|o| o.branch.as_ref()) == Some(&default_branch);
    let prs = r.prs.as_ref();
    let open_pr = prs.and_then(|p| p.iter().find(|p| p.state == "OPEN")).cloned();
    let merged_pr = prs.and_then(|p| p.iter().find(|p| p.state == "MERGED")).cloned();
    let pr_cell = if on_default {
        bar_cell("repo-pr", "pull-request", "Pull request".into(), "Create a branch to open one".into(), false, true).opacity(0.8)
    } else if let Some(pr) = open_pr {
        let url = pr.url.clone();
        kit::hint(bar_cell("repo-pr", "pull-request", "Pull request · open".into(), format!("#{} {}", pr.number, pr.title), false, false), pr.title.clone())
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
            .on_click(move |_, _, cx| cx.open_url(&url))
    } else if let Some(pr) = merged_pr {
        let url = pr.url.clone();
        kit::hint(bar_cell("repo-pr", "pull-request", "Pull request · merged".into(), format!("#{} {}", pr.number, pr.title), false, false), "Open on GitHub")
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
            .on_click(move |_, _, cx| cx.open_url(&url))
    } else {
        let can = ov.is_some_and(|o| o.upstream.is_some() && !o.upstream_gone) && !busy;
        let url = ov.and_then(|o| o.owner_repo.clone().map(|or| format!("https://github.com/{or}/compare/{default_branch}...{}?expand=1", o.branch.clone().unwrap_or_default())));
        let cell = kit::hint(
            bar_cell("repo-pr", "pull-request", "Pull request".into(), if prs.is_none() { "Checking…".into() } else { "Create pull request".into() }, false, false),
            if can { format!("Open a PR from {branch} into {default_branch}") } else { "Publish the branch first".into() },
        );
        if can {
            cell.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED))).on_click(move |_, _, cx| {
                if let Some(u) = &url {
                    cx.open_url(u)
                }
            })
        } else {
            cell.opacity(0.45)
        }
    };
    let switcher = r.switcher.then(|| switcher(r, ov, cx));
    div()
        .mb(px(14.))
        .flex()
        .rounded(px(10.))
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::PANEL))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .child(
                    bar_cell("repo-branch", "branch", "Current branch".into(), branch, false, false)
                        .rounded_l(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                        .child(icon("chevron-down", 12., c(theme::TEXT_FAINT)))
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                            if let Some(r) = t.repo_view() {
                                r.switcher = !r.switcher;
                                let h = r.new_branch.read(cx).focus_handle(cx);
                                if r.switcher {
                                    w.focus(&h, cx);
                                }
                            }
                            cx.notify();
                        })),
                )
                .children(switcher),
        )
        .child(
            div().flex_1().min_w_0().flex().border_l_1().border_color(c(theme::BORDER)).child(
                kit::hint(bar_cell("repo-sync", sync_icon, sync_label, sync_detail, primary, false), sync_hint)
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .when(!busy, |d| d.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_sync(w, cx)))),
            ),
        )
        .child(div().flex_1().min_w_0().flex().border_l_1().border_color(c(theme::BORDER)).rounded_r(px(10.)).child(pr_cell))
}

/// The branch picker under the branch cell: filter, create, switch.
fn switcher(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> AnyElement {
    let q = r.new_branch.read(cx).value().trim().to_lowercase();
    let branches: Vec<&BranchInfo> = ov.map(|o| o.branches.iter().filter(|b| !b.remote_only && (q.is_empty() || b.name.to_lowercase().contains(&q))).collect()).unwrap_or_default();
    let exact = ov.is_some_and(|o| o.branches.iter().any(|b| b.name.to_lowercase() == q));
    div()
        .occlude()
        .absolute()
        .top(px(64.))
        .left_0()
        .w(px(380.))
        .rounded(px(10.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL_RAISED))
        .shadow(theme::shadow(16., 40., 0.55))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .child(icon("search", 12., c(theme::TEXT_FAINT)))
                .child(div().id("repo-branch-name").relative().flex_1().child(Input::new(&r.new_branch).appearance(false)).child(probe("repo-branch-name"))),
        )
        .when(!q.is_empty() && !exact, |d| {
            let name = r.new_branch.read(cx).value().trim().to_string();
            d.child(
                div().p(px(4.)).border_b_1().border_color(c(theme::BORDER)).child(
                    div()
                        .id("repo-create-branch")
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .text_size(px(12.5))
                        .text_color(c(theme::TEXT_DIM))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::accent_soft()).text_color(c(theme::TEXT)))
                        .child(icon("plus", 12., c(theme::TEXT_DIM)))
                        .child("Create branch")
                        .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(name))
                        .child(probe("repo-create-branch"))
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_create_branch(w, cx))),
                ),
            )
        })
        .child(
            div().id("repo-switcher-list").max_h(px(340.)).overflow_y_scroll().p(px(4.)).children(branches.into_iter().map(|b| {
                let name = b.name.clone();
                let free = !b.current && b.worktree.is_none();
                div()
                    .id(gpui::ElementId::Name(format!("repo-switch:{}", b.name).into()))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .text_color(if b.current { c(theme::TEXT) } else { c(theme::TEXT_DIM) })
                    .when(b.current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(free, |d| d.cursor_pointer().hover(|s| s.bg(theme::accent_soft()).text_color(c(theme::TEXT))))
                    .child(icon("branch", 13., if b.current { c(theme::ACCENT) } else { c(theme::TEXT_DIM) }))
                    .child(div().flex_1().min_w_0().truncate().child(b.name.clone()))
                    .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(b.updated.clone()))
                    .child(probe(format!("repo-switch:{}", b.name)))
                    .when(free, |d| d.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_switch(name.clone(), w, cx))))
            })),
        )
        .into_any_element()
}

fn banner(warn: Option<u32>) -> Div {
    let (border, bg) = match warn {
        Some(tone) => (kit::alpha(c(tone), 0.33), kit::alpha(c(tone), 0.07)),
        None => (theme::accent_soft(), kit::alpha(c(theme::ACCENT), 0.06)),
    };
    div()
        .mb(px(12.))
        .px(px(12.))
        .py(px(9.))
        .flex()
        .items_center()
        .gap(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(px(12.5))
        .text_color(c(theme::TEXT_DIM))
}

fn strong(text: impl Into<gpui::SharedString>) -> Div {
    div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(text.into())
}

fn banners(r: &RepoView, ov: &RepoOverview, cx: &mut Context<PageTab>) -> Div {
    let busy = r.busy.is_some();
    let default_branch = ov.default_branch.clone().unwrap_or_else(|| "main".into());
    let on_default = ov.branch.as_ref() == Some(&default_branch);
    let mut out = div().flex().flex_col();
    if let Some(op) = ov.operation.clone() {
        let title = format!("{}{} in progress", op[..1].to_uppercase(), &op[1..]);
        let detail = if ov.conflicts.is_empty() { " · no conflicts left".to_string() } else { format!(" · {}", plural(ov.conflicts.len(), "conflicted file")) };
        let conflicts = !ov.conflicts.is_empty();
        out = out.child(
            banner(Some(theme::WARN))
                .child(div().flex().child(strong(title)).child(detail))
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .gap(px(6.))
                        .when(conflicts, |d| {
                            d.child(kit::button_icon("repo-op-ai", Some(("spark", 12.)), "Resolve with AI", Kind::Secondary, kit::MINI).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                                t.repo_run("Resolve conflicts", Some("Conflicts resolved by the agent".into()), |n| orbit_core::agent::resolve_conflicts(&format!("@{n}"), n).map(|_| ()), w, cx)
                            })))
                        })
                        .child(
                            kit::disabled(kit::button("repo-op-continue", "Continue", Kind::Primary, kit::MINI), busy || conflicts).when(!busy && !conflicts, |b| {
                                b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_run("Continue", None, |n| repo::operation(n, "continue"), w, cx)))
                            }),
                        )
                        .child(kit::button("repo-op-abort", "Abort", Kind::Secondary, kit::MINI).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_run("Abort", None, |n| repo::operation(n, "abort"), w, cx)))),
                ),
        );
    }
    if ov.upstream_gone && !on_default {
        let upstream = ov.upstream.clone().unwrap_or_default();
        let current = ov.branch.clone().unwrap_or_default();
        let target = default_branch.clone();
        out = out.child(
            banner(Some(theme::PURPLE)).child(div().flex().child(strong(upstream)).child(" was deleted on GitHub.")).child(
                div().ml_auto().child(
                    kit::disabled(kit::button("repo-finish", format!("Switch to {default_branch} and delete branch"), Kind::Secondary, kit::MINI), busy || ov.changes > 0).when(
                        !busy && ov.changes == 0,
                        |b| {
                            b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                let (target, current) = (target.clone(), current.clone());
                                t.repo_run(
                                    "Finish branch",
                                    Some(format!("Back on {target}; {current} deleted")),
                                    move |n| repo::switch(n, &target, false).and_then(|_| repo::delete_branch(n, &current, true)),
                                    w,
                                    cx,
                                )
                            }))
                        },
                    ),
                ),
            ),
        );
    }
    if let Some(h) = r.holder() {
        let name = h.name.clone();
        out = out.child(
            banner(None)
                .child(div().flex().flex_wrap().child("This clone is checked out on ").child(strong(h.branch.clone())).child(" for the workspace ").child(strong(h.name.clone())).child("."))
                .child(div().ml_auto().child(kit::button("repo-open-ws", "Open workspace", Kind::Secondary, kit::MINI).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    let name = name.clone();
                    t.with_app(w, cx, move |a, w, cx| super::open_workspace(a, &name, w, cx));
                })))),
        );
    }
    out
}

fn panel() -> Div {
    div().flex().flex_col().rounded(px(10.)).border_1().border_color(c(theme::BORDER)).bg(c(theme::PANEL)).overflow_hidden()
}

fn panel_head() -> Div {
    div().flex_none().min_h(px(42.)).pl(px(14.)).pr(px(12.)).flex().items_center().gap(px(10.)).border_b_1().border_color(c(theme::BORDER))
}

fn empty(text: impl Into<gpui::SharedString>) -> Div {
    div().px(px(14.)).py(px(16.)).flex().flex_col().items_start().gap(px(6.)).text_size(px(12.5)).text_color(c(theme::TEXT_FAINT)).child(text.into())
}

/// `.git-status-badge`: the status letter on a tinted square.
fn status_badge(status: &str) -> Div {
    let color = match status {
        "A" | "U" => c(theme::OK),
        "D" => c(theme::DANGER),
        _ => c(theme::WARN),
    };
    div()
        .flex_none()
        .size(px(14.))
        .rounded(px(3.))
        .bg(kit::alpha(color, 0.12))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(status.to_string())
}

fn changes_panel(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let changes = r.changes.ready().cloned().unwrap_or_default();
    let selected = r.selected();
    let busy = r.busy.is_some();
    let op = ov.is_some_and(|o| o.operation.is_some());
    let branch = ov.and_then(|o| o.branch.clone()).unwrap_or_else(|| "HEAD".into());
    let all = !changes.is_empty() && selected.len() == changes.len();
    let head = panel_head()
        .when(!changes.is_empty(), |d| {
            d.child(div().id("repo-check-all").cursor_pointer().child(kit::check(all, !all && !selected.is_empty())).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                if let Some(r) = t.repo_view() {
                    if all {
                        r.excluded = r.changes.ready().into_iter().flatten().map(|c| c.path.clone()).collect();
                    } else {
                        r.excluded.clear();
                    }
                }
                cx.notify();
            })))
        })
        .child(kit::section_title("Changes"))
        .when(!changes.is_empty(), |d| d.child(kit::count(changes.len()).ml(px(-4.))))
        .when(!changes.is_empty(), |d| {
            let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
            d.child(
                div()
                    .ml_auto()
                    .flex()
                    .gap(px(2.))
                    .child(kit::hint(kit::icon_button("repo-stash", "stash", 28., 14., false), "Stash all changes").on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                        t.repo_run("Stash", Some("Changes stashed".into()), repo::stash, w, cx)
                    })))
                    .child(kit::hint(kit::icon_button("repo-discard-all", "trash", 28., 14., true), "Discard all changes").on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.repo_discard(paths.clone(), w, cx)
                    }))),
            )
        });
    let files = match &r.changes {
        Load::Ready(list) if list.is_empty() => {
            let ahead = ov.map_or(0, |o| o.ahead);
            empty(if ahead > 0 {
                format!("{} waiting to be pushed.", plural(ahead, "commit"))
            } else {
                "Edit files here, in your editor or with an agent session — changes show up as you work.".to_string()
            })
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child("No local changes"))
            .into_any_element()
        }
        Load::Ready(list) => div()
            .flex()
            .flex_col()
            .children(list.iter().map(|ch| {
                let on = !r.excluded.contains(&ch.path);
                let (name, dir) = match ch.path.rsplit_once('/') {
                    Some((d, n)) => (n.to_string(), d.to_string()),
                    None => (ch.path.clone(), String::new()),
                };
                let group = format!("repo-file:{}", ch.path);
                let (p1, p2, p3) = (ch.path.clone(), ch.path.clone(), ch.path.clone());
                div()
                    .id(gpui::ElementId::Name(group.clone().into()))
                    .group(group.clone())
                    .h(px(30.))
                    .pl(px(14.))
                    .pr(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(div().id(gpui::ElementId::Name(format!("repo-file-check:{}", ch.path).into())).cursor_pointer().child(kit::check(on, false)).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        if let Some(r) = t.repo_view() {
                            if !r.excluded.remove(&p1) {
                                r.excluded.insert(p1.clone());
                            }
                        }
                        cx.notify();
                    })))
                    .child(
                        kit::hint(
                            div()
                                .id(gpui::ElementId::Name(format!("repo-file-open:{}", ch.path).into()))
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.5))
                                .cursor_pointer()
                                .when(!on, |d| d.opacity(0.5))
                                .child(status_badge(&ch.status))
                                .child(div().flex_none().child(name))
                                .child(div().min_w_0().truncate().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(dir)),
                            format!("Review changes of {}", ch.path),
                        )
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_open_diff(p2.clone(), w, cx))),
                    )
                    .when(ch.added > 0 || ch.deleted > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_size(px(11.))
                                .when(ch.added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{}", ch.added))))
                                .when(ch.deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{}", ch.deleted)))),
                        )
                    })
                    .child(
                        div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                            kit::hint(kit::icon_button(format!("repo-discard:{}", ch.path), "undo", 24., 13., true), "Discard changes")
                                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_discard(vec![p3.clone()], w, cx))),
                        ),
                    )
            }))
            .into_any_element(),
        Load::Failed(e) => empty(e.clone()).into_any_element(),
        _ => div().px(px(14.)).py(px(9.)).child(div().w(gpui::relative(0.7)).h(px(12.)).rounded(px(4.)).bg(c(theme::PANEL_RAISED))).into_any_element(),
    };
    let none = changes.is_empty();
    let can = !selected.is_empty() && !busy && !op && !r.summary.read(cx).value().trim().is_empty();
    let label = if r.busy.as_deref() == Some("Commit") {
        "Committing…".to_string()
    } else if selected.is_empty() {
        "Select files to commit".to_string()
    } else {
        format!("Commit {} to {branch}", plural(selected.len(), "file"))
    };
    panel()
        .h_full()
        .child(head)
        .child(div().id("repo-files").flex_1().min_h(px(120.)).max_h(px(340.)).overflow_y_scroll().py(px(4.)).child(files))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .pt(px(12.))
                .pb(px(14.))
                .border_t_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(div().id("repo-summary").relative().flex_1().child(Input::new(&r.summary).disabled(none || op)).child(probe("repo-summary")))
                        .child(if r.drafting {
                            div()
                                .id("repo-draft")
                                .size(px(34.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(c(theme::BORDER_STRONG))
                                .child(kit::spinner("repo-draft-spin", 14., c(theme::ACCENT)))
                        } else {
                            kit::hint(kit::disabled(kit::icon_button("repo-draft", "spark", 34., 14., false), none), "Draft the message with AI")
                                .when(!none, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_draft(w, cx))))
                        }),
                )
                .child(div().id("repo-description").relative().child(Textarea::new(&r.description).disabled(none || op)).child(probe("repo-description")))
                .child(
                    kit::disabled(kit::button("repo-commit", label, Kind::Primary, kit::NORMAL).w_full(), !can)
                        .when(can, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.repo_commit_now(w, cx)))),
                ),
        )
}

fn history_panel(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let busy = r.busy.is_some();
    let ahead = ov.map_or(0, |o| o.ahead);
    let tracked = ov.is_some_and(|o| o.upstream.is_some() && !o.upstream_gone);
    let head = panel_head()
        .child(
            div()
                .flex()
                .gap(px(2.))
                .child(div().px(px(10.)).py(px(4.)).rounded(px(6.)).bg(c(theme::PANEL_RAISED)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child("HISTORY"))
                .child(div().px(px(10.)).py(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child("COMPARE")),
        )
        .when(tracked && ahead > 0, |d| {
            d.child(
                div()
                    .ml_auto()
                    .h(px(18.))
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .rounded(px(9.))
                    .border_1()
                    .border_color(kit::alpha(c(theme::INFO), 0.35))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c(theme::INFO))
                    .child(format!("↑ {ahead} not pushed")),
            )
        });
    let list = match &r.history {
        Load::Ready(list) if list.is_empty() => empty("No commits yet.").into_any_element(),
        Load::Ready(list) => div()
            .flex()
            .flex_col()
            .children(list.iter().enumerate().map(|(i, cm)| {
                let unpushed = tracked && i < ahead;
                let group = format!("repo-commit:{}", cm.sha);
                let (sha, msg) = (cm.sha.clone(), cm.message.clone());
                div()
                    .id(gpui::ElementId::Name(group.clone().into()))
                    .group(group.clone())
                    .pl(px(14.))
                    .pr(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(unpushed, |d| d.border_l_2().border_color(c(theme::ACCENT)).pl(px(12.)))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(
                        kit::hint(
                            div()
                                .flex_1()
                                .min_w_0()
                                .py(px(7.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().truncate().text_size(px(12.5)).child(cm.message.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.))
                                        .text_size(px(11.))
                                        .text_color(c(theme::TEXT_FAINT))
                                        .when(unpushed, |d| d.child(div().font_weight(FontWeight::BOLD).text_color(c(theme::INFO)).child("↑")))
                                        .child(format!("{} · {}", cm.author, cm.when)),
                                )
                                .id(gpui::ElementId::Name(format!("repo-commit-main:{}", cm.sha).into())),
                            format!("{}\n\n{} · {}", cm.message, cm.sha, cm.author),
                        ),
                    )
                    .map(|d| {
                        if i == 0 && unpushed {
                            d.child(kit::hint(kit::icon_button("repo-undo", "undo", 28., 13., false), "Undo this commit (keeps its changes)").when(!busy, |b| {
                                b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                                    t.repo_run("Undo commit", Some("Commit undone — its changes are back in Changes".into()), repo::undo_commit, w, cx)
                                }))
                            }))
                        } else {
                            d.child(
                                div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                                    kit::hint(kit::button(format!("repo-revert:{}", cm.sha), "Revert", Kind::Secondary, kit::MINI), "Add a commit that undoes this one").on_click(cx.listener(
                                        move |t, _: &ClickEvent, w, cx| {
                                            let (sha, msg) = (sha.clone(), msg.clone());
                                            let m2 = msg.clone();
                                            let go: Action = Rc::new(move |t, w, cx| {
                                                let s = sha.clone();
                                                t.repo_run("Revert", Some(format!("Reverted \"{msg}\"")), move |n| repo::revert(n, &s), w, cx)
                                            });
                                            t.repo_ask(
                                                "Revert commit".into(),
                                                format!("Add a new commit that undoes \"{m2}\"? History stays as it is, so this is safe on pushed commits."),
                                                vec![("Revert", go)],
                                                w,
                                                cx,
                                            )
                                        },
                                    )),
                                ),
                            )
                        }
                    })
            }))
            .into_any_element(),
        Load::Failed(e) => empty(e.clone()).into_any_element(),
        _ => div().px(px(14.)).py(px(9.)).child(div().w(gpui::relative(0.8)).h(px(12.)).rounded(px(4.)).bg(c(theme::PANEL_RAISED))).into_any_element(),
    };
    panel().h_full().child(head).child(div().id("repo-commits").flex_1().max_h(px(520.)).overflow_y_scroll().py(px(4.)).child(list))
}

fn branches_panel(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let busy = r.busy.is_some();
    let q = r.filter.read(cx).value().trim().to_lowercase();
    let matches = |b: &&&BranchInfo| q.is_empty() || b.name.to_lowercase().contains(&q);
    let all: Vec<&BranchInfo> = ov.map(|o| o.branches.iter().collect()).unwrap_or_default();
    let locals: Vec<&BranchInfo> = all.iter().copied().filter(|b| !b.remote_only).collect();
    let remotes: Vec<&BranchInfo> = all.iter().copied().filter(|b| b.remote_only).collect();
    let pill = |text: String, color: gpui::Rgba, border: gpui::Rgba| {
        div().flex_none().h(px(18.)).px(px(7.)).flex().items_center().rounded(px(9.)).border_1().border_color(border).text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(color).child(text)
    };
    let row = |b: &BranchInfo, cx: &mut Context<PageTab>| {
        let group = format!("repo-branch:{}", b.name);
        let free = !b.current && b.worktree.is_none();
        let (n1, n2) = (b.name.clone(), b.name.clone());
        let owner = b.worktree.as_ref().and_then(|w| ov.and_then(|o| o.worktrees.iter().find(|x| &x.path == w)).and_then(|x| x.workspace.clone()));
        div()
            .id(gpui::ElementId::Name(group.clone().into()))
            .group(group.clone())
            .min_h(px(34.))
            .pl(px(14.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(if b.current { c(theme::TEXT) } else { c(theme::TEXT_DIM) })
            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
            .child(icon("branch", 13., if b.current { c(theme::ACCENT) } else { c(theme::TEXT_DIM) }))
            .child(div().min_w_0().truncate().child(b.name.clone()))
            .when(b.current, |d| d.child(pill("current".into(), kit::mix(c(theme::INFO), c(theme::TEXT), 0.7), kit::alpha(c(theme::ACCENT), 0.4))))
            .when(b.remote_only, |d| d.child(pill("origin".into(), c(theme::TEXT_DIM), c(theme::BORDER_STRONG))))
            .when(b.gone, |d| d.child(pill("deleted on origin".into(), c(theme::PURPLE), kit::alpha(c(theme::PURPLE), 0.35))))
            .when_some(owner, |d, o| d.child(pill(format!("in {o}"), c(theme::TEXT_DIM), c(theme::BORDER_STRONG))))
            .when(b.ahead > 0, |d| d.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::INFO)).child(format!("↑{}", b.ahead))))
            .when(b.behind > 0, |d| d.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::INFO)).child(format!("↓{}", b.behind))))
            .child(div().ml_auto().flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(b.updated.clone()))
            .child(
                div()
                    .w(px(92.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(px(4.))
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .when(free, |d| {
                        d.child(kit::button(format!("repo-switch:{}", b.name), "Switch", Kind::Secondary, kit::MINI).when(!busy, |bt| {
                            bt.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_switch(n1.clone(), w, cx)))
                        }))
                    })
                    .when(free && !b.remote_only, |d| {
                        d.child(kit::icon_button(format!("repo-delete:{}", b.name), "trash", 24., 13., true).when(!busy, |bt| {
                            bt.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_delete(n2.clone(), w, cx)))
                        }))
                    }),
            )
    };
    let mut list = div().id("repo-branches").max_h(px(360.)).overflow_y_scroll().py(px(4.)).flex().flex_col();
    for b in locals.iter().filter(matches) {
        list = list.child(row(b, cx));
    }
    if !remotes.is_empty() {
        let show = r.show_remote;
        list = list.child(
            div()
                .id("repo-remote-toggle")
                .px(px(14.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.5))
                .text_color(c(theme::TEXT_FAINT))
                .cursor_pointer()
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(icon("chevron-right", 10., c(theme::TEXT_FAINT)))
                .child(format!("{} {} only on origin", if show { "Hide" } else { "Show" }, plural(remotes.len(), "branch").replace("branchs", "branches")))
                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    if let Some(r) = t.repo_view() {
                        r.show_remote = !r.show_remote;
                    }
                    cx.notify();
                })),
        );
        if show || !q.is_empty() {
            for b in remotes.iter().filter(matches) {
                list = list.child(row(b, cx));
            }
        }
    }
    panel()
        .child(
            panel_head()
                .child(kit::section_title("Branches"))
                .when(ov.is_some(), |d| d.child(kit::count(locals.len()).ml(px(-4.))))
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(icon("search", 12., c(theme::TEXT_FAINT)))
                        .child(div().id("repo-filter").w(px(140.)).child(Input::new(&r.filter).small())),
                ),
        )
        .child(list)
}

fn side_panel(r: &RepoView, ov: Option<&RepoOverview>, cx: &mut Context<PageTab>) -> impl IntoElement {
    let busy = r.busy.is_some();
    let name = r.name.clone();
    let side_row = |id: String| {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .w_full()
            .min_h(px(34.))
            .pl(px(14.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
    };
    let meta = |t: String| div().ml_auto().flex_none().font_family(theme::MONO_FONT).text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(t);
    let mut list = div().py(px(4.)).flex().flex_col();
    if r.workspaces.is_empty() {
        list = list.child(empty(format!("No workspace uses {name} yet. A workspace puts a feature branch in its own worktree, so the clone stays free.")));
    }
    for w in &r.workspaces {
        let wname = w.name.clone();
        list = list.child(
            side_row(format!("repo-ws:{}", w.name))
                .cursor_pointer()
                .child(icon("satellite", 13., c(theme::TEXT_DIM)))
                .child(div().min_w_0().truncate().child(w.name.clone()))
                .child(meta(w.branch.clone()))
                .on_click(cx.listener(move |t, _: &ClickEvent, win, cx| {
                    let n = wname.clone();
                    t.with_app(win, cx, move |a, w, cx| super::open_workspace(a, &n, w, cx));
                })),
        );
    }
    for w in ov.into_iter().flat_map(|o| o.worktrees.iter()).filter(|w| !w.main && w.workspace.is_none()) {
        let leaf = w.path.rsplit(['/', '\\']).next().unwrap_or(&w.path).to_string();
        list = list.child(
            kit::hint(side_row(format!("repo-wt:{}", w.path)), w.path.clone())
                .child(icon("branch", 13., c(theme::TEXT_DIM)))
                .child(div().min_w_0().truncate().child(leaf))
                .child(meta(if w.missing { "folder deleted".into() } else { w.branch.clone().unwrap_or_else(|| "detached".into()) }))
                .when(w.missing, |d| {
                    d.child(kit::button("repo-prune", "Prune", Kind::Secondary, kit::MINI).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                        t.repo_run("Prune worktrees", None, repo::prune_worktrees, w, cx)
                    })))
                }),
        );
    }
    let stashes = ov.map(|o| o.stashes.clone()).unwrap_or_default();
    let name2 = r.name.clone();
    panel()
        .child(
            panel_head().child(kit::section_title("Workspaces")).when(!r.workspaces.is_empty(), |d| d.child(kit::count(r.workspaces.len()).ml(px(-4.)))).child(
                div().ml_auto().child(
                    kit::hint(kit::button_icon("repo-new-ws-side", Some(("plus", 11.)), "New workspace", Kind::Secondary, kit::MINI), format!("New workspace with {name2}: a branch in its own worktree"))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_create(Some(vec![name2.clone()]), w, cx))),
                ),
            ),
        )
        .child(list)
        .when(!stashes.is_empty(), |d| {
            d.child(panel_head().border_t_1().child(kit::section_title("Stashes")).child(kit::count(stashes.len()).ml(px(-4.)))).child(div().py(px(4.)).flex().flex_col().children(
                stashes.into_iter().map(|s| {
                    let ix = s.index;
                    side_row(format!("repo-stash:{ix}"))
                        .child(icon("stash", 13., c(theme::TEXT_DIM)))
                        .child(div().min_w_0().truncate().child(s.message.clone()))
                        .child(meta(s.when.clone()))
                        .child(kit::button(format!("repo-stash-pop:{ix}"), "Restore", Kind::Secondary, kit::MINI).when(!busy, |b| {
                            b.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_run("Restore stash", Some("Stash restored".into()), move |n| repo::stash_pop(n, ix), w, cx)))
                        }))
                        .child(kit::hint(kit::icon_button(format!("repo-stash-drop:{ix}"), "trash", 24., 13., true), "Drop stash").on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            let go: Action = Rc::new(move |t, w, cx| t.repo_run("Drop stash", Some("Stash dropped".into()), move |n| repo::stash_drop(n, ix), w, cx));
                            t.repo_ask("Drop stash".into(), "Throw this stash away?".into(), vec![("Drop", go)], w, cx)
                        })))
                }),
            ))
        })
}
