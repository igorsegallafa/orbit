//! Settings → Repositories and Groups (`ReposSection.tsx`, `FoldersCard.tsx`,
//! `RepoFormModal.tsx`, `GithubRepoModal.tsx`, `GroupsSection.tsx`). The
//! modals are inline cards here.
use super::settings::SettingsPage;
use super::kit::{self, Kind};
use super::ui;
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, AnyElement, App, ClickEvent, Context, Div, Entity, FontWeight, SharedString, Window};
use gpui_component::input::InputState;
use orbit_core::config::{BuildCmd, Service};
use orbit_core::github::GithubRepo;
use orbit_core::settings::{self, Folders};
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CloneState {
    Cloned,
    Missing,
    Cloning,
}

/// The add/edit form (`RepoFormModal.tsx`).
pub struct RepoForm {
    /// The repo being edited; None adds one.
    editing: Option<Service>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    path: Entity<InputState>,
    build: Entity<InputState>,
    output: Entity<InputState>,
    shared: Entity<InputState>,
    branch_only: bool,
    auto_switch: bool,
    clone_now: bool,
}

pub struct GithubPicker {
    filter: Entity<InputState>,
    list: Load<Vec<GithubRepo>>,
}

pub struct ReposState {
    clone: HashMap<String, CloneState>,
    folders: Option<Folders>,
    repos_dir: Entity<InputState>,
    workspaces_dir: Entity<InputState>,
    form: Option<RepoForm>,
    local: Option<Entity<InputState>>,
    github: Option<GithubPicker>,
}

impl ReposState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            clone: HashMap::new(),
            folders: None,
            repos_dir: cx.new(|cx| InputState::new(window, cx)),
            workspaces_dir: cx.new(|cx| InputState::new(window, cx)),
            form: None,
            local: None,
            github: None,
        }
    }

    pub fn clone_label(&self, name: &str) -> &'static str {
        match self.clone.get(name) {
            Some(CloneState::Cloned) => "cloned",
            Some(CloneState::Missing) => "missing",
            Some(CloneState::Cloning) => "cloning",
            None => "unknown",
        }
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        if let Some(f) = &self.folders {
            let _ = writeln!(out, "  folders: clones {} · workspaces {}", f.repos_dir, f.workspaces_dir);
        }
        if let Some(f) = &self.form {
            let _ = writeln!(
                out,
                "  repo form ({}): name {:?} url {:?} path {:?} build {:?} branch-only {} clone-now {}",
                f.editing.as_ref().map_or("add", |s| s.name.as_str()),
                f.name.read(cx).value(),
                f.url.read(cx).value(),
                f.path.read(cx).value(),
                f.build.read(cx).value(),
                f.branch_only,
                f.clone_now
            );
        }
        if let Some(g) = &self.github {
            let _ = writeln!(out, "  github picker: {}", ui::load_label(&g.list));
        }
    }
}

pub struct GroupsState {
    new_group: Entity<InputState>,
    /// The group being edited and its draft members.
    editing: Option<(String, Vec<String>)>,
}

impl GroupsState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self { new_group: cx.new(|cx| InputState::new(window, cx).placeholder("New group name")), editing: None }
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let typed = self.new_group.read(cx).value();
        if !typed.is_empty() {
            let _ = writeln!(out, "  new group input {typed:?}");
        }
        if let Some((g, m)) = &self.editing {
            let _ = writeln!(out, "  editing group {g}: {m:?}");
        }
    }
}

fn input(window: &mut Window, cx: &mut Context<SettingsPage>, placeholder: &str, value: &str) -> Entity<InputState> {
    let (p, v) = (placeholder.to_string(), value.to_string());
    cx.new(|cx| InputState::new(window, cx).placeholder(p).default_value(v))
}

fn text(i: &Entity<InputState>, cx: &App) -> String {
    i.read(cx).value().trim().to_string()
}

impl SettingsPage {
    pub(super) fn load_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::bg(
            cx,
            settings::folders,
            |t: &mut Self, r, window, cx| {
                let Ok(f) = r else { return };
                let cfg = t.cfg().cloned().unwrap_or_default();
                let (repos, wss) = (cfg.repos_dir.unwrap_or_default(), cfg.workspaces_dir.unwrap_or_default());
                t.repos.repos_dir = input(window, cx, &f.default_repos_dir, &repos);
                t.repos.workspaces_dir = input(window, cx, &f.default_workspaces_dir, &wss);
                t.repos.folders = Some(f);
            },
            window,
        );
    }

    /// Clone status of every repo not known yet.
    pub(super) fn refresh_clone_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self.cfg().map(|c| c.services.iter().map(|s| s.name.clone()).filter(|n| !self.repos.clone.contains_key(n)).collect()).unwrap_or_default();
        if names.is_empty() {
            return;
        }
        ui::bg(
            cx,
            move || names.into_iter().filter_map(|n| settings::clone_status(&n).ok().map(|ok| (n, ok))).collect::<Vec<_>>(),
            |t: &mut Self, list, _, _| {
                for (n, ok) in list {
                    t.repos.clone.insert(n, if ok { CloneState::Cloned } else { CloneState::Missing });
                }
            },
            window,
        );
    }

    fn clone_repo(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.repos.clone.insert(name.clone(), CloneState::Cloning);
        self.host.toast(ToastKind::Info, format!("Cloning {name}…"), None, cx);
        let n = name.clone();
        ui::bg(
            cx,
            move || settings::clone_service(&n),
            move |t: &mut Self, r, _, cx| match r {
                Ok(()) => {
                    t.repos.clone.insert(name.clone(), CloneState::Cloned);
                    t.host.toast(ToastKind::Success, format!("{name} cloned"), None, cx);
                }
                Err(e) => {
                    t.repos.clone.insert(name.clone(), CloneState::Missing);
                    t.host.error(format!("Couldn't clone {name}"), e, cx);
                }
            },
            window,
        );
    }

    fn reveal_repo(&mut self, name: &str, cx: &mut Context<Self>) {
        match settings::clone_folder(name) {
            Ok(dir) if crate::terminal::CLIPBOARD_DISABLED.load(std::sync::atomic::Ordering::Relaxed) => {
                self.host.toast(ToastKind::Info, "Revealed", Some(dir.display().to_string()), cx)
            }
            Ok(dir) => cx.reveal_path(&dir),
            Err(e) => self.host.error("Couldn't open the folder", e, cx),
        }
    }

    fn open_form(&mut self, editing: Option<Service>, window: &mut Window, cx: &mut Context<Self>) {
        let s = editing.clone().unwrap_or_else(|| Service::new(String::new(), String::new()));
        let build = match &s.build {
            Some(BuildCmd::All(c)) => c.clone(),
            _ => String::new(),
        };
        let default_path = self.repos.folders.as_ref().map(|f| format!("{}/<name>", f.repos_dir)).unwrap_or_default();
        self.repos.form = Some(RepoForm {
            name: input(window, cx, "my-service", &s.name),
            url: input(window, cx, "git@github.com:org/repo.git", &s.repo),
            path: input(window, cx, &format!("Default: {default_path}"), s.path.as_deref().unwrap_or("")),
            build: input(window, cx, "pnpm install && pnpm build", &build),
            output: input(window, cx, "dist", s.build_output.as_deref().unwrap_or("")),
            shared: input(window, cx, "All of it (comma-separated sub-folders)", &s.build_output_shared.join(", ")),
            branch_only: !s.worktree,
            auto_switch: s.auto_switch,
            clone_now: true,
            editing,
        });
        self.repos.local = None;
        self.repos.github = None;
        cx.notify();
    }

    fn save_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &self.repos.form else { return };
        let (name, url, path) = (text(&f.name, cx), text(&f.url, cx), text(&f.path, cx));
        if name.is_empty() || url.is_empty() {
            return self.host.error("Couldn't save the repository", "Name and URL are required".into(), cx);
        }
        let build_text = text(&f.build, cx);
        // Per-OS commands are kept as they are: this form edits one command.
        let build = match f.editing.as_ref().and_then(|s| s.build.clone()) {
            Some(per_os @ BuildCmd::PerOs { .. }) => Some(per_os),
            _ => (!build_text.is_empty()).then_some(BuildCmd::All(build_text)),
        };
        let shared: Vec<String> = text(&f.shared, cx).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let output = text(&f.output, cx);
        let service = Service {
            name: name.clone(),
            repo: url.clone(),
            build,
            build_output: (!output.is_empty()).then_some(output),
            build_output_shared: shared,
            worktree: !f.branch_only,
            auto_switch: f.branch_only && f.auto_switch,
            path: (!path.is_empty()).then_some(path.clone()),
        };
        let adding = f.editing.is_none();
        let clone_now = adding && f.clone_now;
        let n = name.clone();
        self.edit(
            "Couldn't save the repository",
            move || {
                if adding {
                    settings::add_service(&n, &url, Some(path))?;
                } else {
                    settings::update_service(&n, &url)?;
                }
                settings::update_service_settings(service)
            },
            move |t, window, cx| {
                t.repos.form = None;
                t.repos.clone.remove(&name);
                t.refresh_clone_status(window, cx);
                if clone_now {
                    t.clone_repo(name, window, cx);
                } else {
                    t.host.toast(ToastKind::Success, format!("{name} {}", if adding { "added" } else { "saved" }), None, cx);
                }
            },
            window,
            cx,
        );
    }

    fn add_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = &self.repos.local else { return };
        let path = text(i, cx);
        if path.is_empty() {
            return;
        }
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
        let n = name.clone();
        self.edit(
            "Couldn't add the local clone",
            move || {
                let origin = settings::local_repo_origin(&path)?;
                settings::add_service(&n, &origin, Some(path))
            },
            move |t, window, cx| {
                t.repos.local = None;
                t.refresh_clone_status(window, cx);
                t.host.toast(ToastKind::Success, format!("{name} added"), None, cx);
            },
            window,
            cx,
        );
    }

    fn open_github(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.repos.github.is_none() || force {
            let filter = input(window, cx, "Filter repositories", "");
            cx.observe(&filter, |_, _, cx| cx.notify()).detach();
            self.repos.github = Some(GithubPicker { filter, list: Load::Loading });
        }
        self.repos.form = None;
        self.repos.local = None;
        ui::bg(
            cx,
            move || orbit_core::github::list_accessible_repos(force),
            |t: &mut Self, r, _, _| {
                if let Some(g) = &mut t.repos.github {
                    g.list = r.map_or_else(Load::Failed, Load::Ready);
                }
            },
            window,
        );
    }

    fn add_github(&mut self, repo: GithubRepo, window: &mut Window, cx: &mut Context<Self>) {
        // HTTPS: the gh login authenticates it; SSH would need a key set up.
        let url = format!("https://github.com/{}.git", repo.name_with_owner);
        let name = repo.name.clone();
        let n = name.clone();
        self.edit(
            "Couldn't add the repository",
            move || settings::add_service(&n, &url, None),
            move |t, window, cx| {
                t.refresh_clone_status(window, cx);
                t.host.toast(ToastKind::Success, format!("{name} added"), Some("Clone it from the list".into()), cx);
            },
            window,
            cx,
        );
    }

    fn remove_repo(&mut self, svc: Service, window: &mut Window, cx: &mut Context<Self>) {
        let message = if svc.custom_path().is_some() {
            "It leaves Orbit's list. The folder at its custom path is kept on disk."
        } else {
            "It leaves Orbit's list and its clone in the clones folder is deleted."
        };
        let me = cx.entity().downgrade();
        let name = svc.name.clone();
        self.host.confirm(
            format!("Remove {name}?"),
            message,
            "Remove",
            move |window, cx| {
                let n = name.clone();
                let _ = me.update(cx, |t, cx| {
                    let n2 = n.clone();
                    t.edit(
                        "Couldn't remove the repository",
                        move || settings::remove_service(&n2),
                        move |t, _, cx| {
                            t.repos.clone.remove(&n);
                            t.host.toast(ToastKind::Success, format!("{n} removed"), None, cx);
                        },
                        window,
                        cx,
                    )
                });
            },
            window,
            cx,
        );
    }

    /// "Browse…": a native folder picker that fills `input`.
    fn browse(&mut self, input: Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.first().map(|p| p.display().to_string()) else { return };
            let _ = input.update_in(cx, |i, w, cx| i.set_value(path, w, cx));
        })
        .detach();
    }

    fn folders_dirty(&self, cx: &App) -> bool {
        let cfg = self.cfg().cloned().unwrap_or_default();
        text(&self.repos.repos_dir, cx) != cfg.repos_dir.unwrap_or_default() || text(&self.repos.workspaces_dir, cx) != cfg.workspaces_dir.unwrap_or_default()
    }

    fn render_folders(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let path_field = |name: &'static str, label: &'static str, hint: &'static str, input: &Entity<InputState>, cx: &mut Context<Self>| {
            let i = input.clone();
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child(label))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(kit::field(name, input, window, cx).font_family(theme::MONO_FONT))
                        .child(
                            kit::button(format!("{name}:browse"), "Browse…", Kind::Secondary)
                                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.browse(i.clone(), w, cx))),
                        ),
                )
                .child(div().text_size(px(11.)).line_height(px(15.4)).text_color(c(theme::TEXT_FAINT)).child(hint))
        };
        let mut card = kit::card().p(px(16.)).child(
            div()
                .flex()
                .gap(px(12.))
                .child(path_field("folders-repos", "Clones folder", "New repositories are cloned here unless you pick a folder for them.", &self.repos.repos_dir, cx))
                .child(path_field(
                    "folders-workspaces",
                    "Workspaces folder",
                    "Each workspace gets a folder here with one worktree per repository.",
                    &self.repos.workspaces_dir,
                    cx,
                )),
        );
        if self.folders_dirty(cx) {
            card = card.child(
                div()
                    .mt(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .mr_auto()
                            .text_size(px(11.))
                            .text_color(c(theme::TEXT_FAINT))
                            .child("Existing clones and workspaces are not moved; Orbit starts using the new folders."),
                    )
                    .child(kit::button("folders-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load_folders(w, cx))))
                    .child(kit::button("folders-save", "Save folders", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                        let opt = |s: String| (!s.is_empty()).then_some(s);
                        let (r, ws) = (opt(text(&t.repos.repos_dir, cx)), opt(text(&t.repos.workspaces_dir, cx)));
                        t.edit(
                            "Couldn't save the folders",
                            move || settings::set_folders(r, ws),
                            |t, w, cx| {
                                t.load_folders(w, cx);
                                t.host.toast(ToastKind::Success, "Folders saved", None, cx);
                            },
                            w,
                            cx,
                        )
                    }))),
            );
        }
        card
    }

    pub(super) fn render_repos(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(cfg) = self.cfg().cloned() else {
            return kit::section().child(kit::skeleton(gpui::relative(0.3), 16.)).child(kit::skeleton(gpui::relative(1.), 110.)).into_any_element();
        };
        let mut out = kit::section().child(kit::section_header("Folders", None, None)).child(self.render_folders(window, cx));

        let actions = div()
            .child(kit::icon_label_button("repo-add", "plus", "Add manually", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_form(None, w, cx))))
            .child(kit::icon_label_button("repo-add-local", "folder", "Add local clone", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                t.repos.form = None;
                t.repos.github = None;
                t.repos.local = Some(input(w, cx, "/path/to/an/existing/checkout", ""));
                cx.notify();
            })))
            .child(kit::icon_label_button("repo-add-github", "git", "Add from GitHub", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_github(false, w, cx))));
        out = out.child(kit::section_header("Repositories", Some(cfg.services.len()), Some(actions)));
        if let Some(form) = &self.repos.form {
            out = out.child(self.render_form(form, window, cx));
        }
        if let Some(local) = &self.repos.local {
            out = out.child(
                kit::card().p(px(16.)).gap(px(10.)).child(kit::hint("A folder with a git checkout: Orbit uses it as is and reads its origin.")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(kit::field("local-path", local, window, cx).font_family(theme::MONO_FONT))
                        .child(kit::button("local-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.repos.local = None;
                            cx.notify();
                        })))
                        .child(kit::button("local-add", "Add", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add_local(w, cx)))),
                ),
            );
        }
        if let Some(g) = &self.repos.github {
            out = out.child(self.render_github(g, &cfg.services, window, cx));
        }
        if cfg.services.is_empty() {
            out = out.child(
                kit::card()
                    .p(px(32.))
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(40.)).rounded(px(10.)).bg(c(theme::BG)).flex().items_center().justify_center().child(kit::icon("git", 20., c(theme::TEXT_DIM))))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("No repositories yet"))
                    .child(kit::hint("Add the repositories your workspaces will span. Orbit clones them once and creates worktrees per workspace."))
                    .child(kit::button("repo-empty-github", "Add from GitHub", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_github(false, w, cx)))),
            );
            return out.into_any_element();
        }
        let mono = |text: String| div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(11.5)).line_height(px(15.)).text_color(c(theme::TEXT_FAINT)).child(text);
        let rows = cfg.services.iter().map(|s| {
            let st = self.repos.clone.get(&s.name).copied();
            let (n1, n2, s1, s2, s3) = (s.name.clone(), s.name.clone(), s.clone(), s.clone(), s.clone());
            let status: AnyElement = match st {
                None => kit::skeleton(px(70.), 12.).pill().into_any_element(),
                Some(CloneState::Cloned) => {
                    div().flex().items_center().gap(px(7.)).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(super::settings::repo_dot()).child("Cloned").into_any_element()
                }
                Some(CloneState::Cloning) => div().text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child("Cloning…").into_any_element(),
                Some(CloneState::Missing) => kit::icon_label_button(format!("repo:{}:clone", s.name), "download", "Clone", Kind::Secondary)
                    .h(px(30.))
                    .text_size(px(12.5))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.clone_repo(n1.clone(), w, cx)))
                    .into_any_element(),
            };
            let mut title = div().flex().items_center().gap(px(8.)).child(div().text_size(px(13.5)).line_height(px(17.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(s.name.clone()));
            if s.build.is_some() {
                title = title.child(kit::outline_pill("build", c(theme::TEXT_DIM), c(theme::BORDER_STRONG)).font_weight(FontWeight::NORMAL));
            }
            if !s.worktree {
                title = title.child(kit::outline_pill("branch in base clone", c(theme::TEXT_DIM), c(theme::BORDER_STRONG)).font_weight(FontWeight::NORMAL));
            }
            let cloned = st == Some(CloneState::Cloned);
            let group = SharedString::from(format!("repo-row-{}", s.name));
            div()
                .id(SharedString::from(format!("repo:{}", s.name)))
                .group(group.clone())
                .px(px(14.))
                .py(px(12.))
                .flex()
                .items_center()
                .gap(px(14.))
                .hover(|d| d.bg(c(theme::PANEL_RAISED)))
                .on_click(cx.listener(move |t, ev: &ClickEvent, w, cx| {
                    if ev.click_count() == 2 {
                        t.open_form(Some(s3.clone()), w, cx)
                    }
                }))
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .bg(c(theme::BG))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(kit::icon("git", 15., c(theme::TEXT_DIM))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(title)
                        .child(mono(s.repo.clone()))
                        .when_some(s.path.clone(), |d, p| d.child(div().flex().items_center().gap(px(5.)).child(kit::icon("folder", 11., c(theme::TEXT_FAINT))).child(mono(p)))),
                )
                .child(div().min_w(px(104.)).flex().justify_end().child(status))
                .child(
                    div()
                        .flex()
                        .gap(px(2.))
                        .opacity(0.35)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            kit::disabled(kit::icon_button(format!("repo:{}:reveal", s.name), "folder"), !cloned)
                                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.reveal_repo(&n2, cx))),
                        )
                        .child(kit::icon_button(format!("repo:{}:edit", s.name), "pencil").on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_form(Some(s1.clone()), w, cx))))
                        .child(kit::icon_button(format!("repo:{}:remove", s.name), "trash").on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.remove_repo(s2.clone(), w, cx)))),
                )
        });
        out.child(kit::card().children(rows.enumerate().map(|(i, r)| r.when(i > 0, |r| r.border_t_1().border_color(c(theme::BORDER)))))).into_any_element()
    }

    fn render_form(&self, f: &RepoForm, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let editing = f.editing.is_some();
        let cloned = f.editing.as_ref().is_some_and(|s| self.repos.clone.get(&s.name) == Some(&CloneState::Cloned));
        let per_os = matches!(f.editing.as_ref().and_then(|s| s.build.as_ref()), Some(BuildCmd::PerOs { .. }));
        let label = |text: &'static str| div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child(text);
        let field = |text: &'static str, el: AnyElement| div().flex_1().min_w_0().flex().flex_col().gap(px(5.)).child(label(text)).child(el);
        let mono = |text: String| div().h(px(34.)).flex().items_center().font_family(theme::MONO_FONT).text_size(px(12.)).text_color(c(theme::TEXT_DIM)).child(text);
        let name: AnyElement = if editing { mono(f.editing.as_ref().map(|s| s.name.clone()).unwrap_or_default()).into_any_element() } else { kit::field("form-name", &f.name, window, cx).into_any_element() };
        let mut card = kit::card()
            .p(px(16.))
            .gap(px(12.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(match &f.editing {
                Some(s) => format!("Edit {}", s.name),
                None => "Add a repository".into(),
            }))
            .child(div().flex().gap(px(12.)).child(field("Name", name)).child(field("Repository URL", kit::field("form-url", &f.url, window, cx).into_any_element())));
        if !editing {
            card = card.child(field("Local path", kit::field("form-path", &f.path, window, cx).font_family(theme::MONO_FONT).into_any_element()));
        } else if let Some(p) = f.editing.as_ref().and_then(|s| s.path.clone()) {
            card = card.child(field("Local path", mono(if cloned { format!("{p} (cloned)") } else { p }).into_any_element()));
        }
        let build: AnyElement = if per_os {
            mono("One command per OS (edit those in the Tauri app)".into()).into_any_element()
        } else {
            kit::field("form-build", &f.build, window, cx).font_family(theme::MONO_FONT).into_any_element()
        };
        card = card
            .child(field("Build command", build))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(field("Output folder", kit::field("form-output", &f.output, window, cx).into_any_element()))
                    .child(field("Share only", kit::field("form-shared", &f.shared, window, cx).into_any_element())),
            )
            .child(field(
                "Checkout",
                div()
                    .flex()
                    .gap(px(16.))
                    .child(kit::check("form-worktree", "Worktree per workspace", !f.branch_only).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.branch_only = false;
                        }
                        cx.notify();
                    })))
                    .child(kit::check("form-branch-only", "Branch in the base clone", f.branch_only).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.branch_only = true;
                        }
                        cx.notify();
                    })))
                    .when(f.branch_only, |d| {
                        d.child(kit::check("form-auto-switch", "Switch branches automatically", f.auto_switch).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            if let Some(f) = &mut t.repos.form {
                                f.auto_switch = !f.auto_switch;
                            }
                            cx.notify();
                        })))
                    })
                    .into_any_element(),
            ));
        card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pt(px(4.))
                .when(!editing, |d| {
                    d.child(kit::check("form-clone-now", "Clone now", f.clone_now).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.clone_now = !f.clone_now;
                        }
                        cx.notify();
                    })))
                })
                .child(div().flex_1())
                .child(kit::button("form-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.repos.form = None;
                    cx.notify();
                })))
                .child(kit::button("form-save", if editing { "Save" } else { "Add repository" }, Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.save_form(w, cx)))),
        )
        .into_any_element()
    }

    fn render_github(&self, g: &GithubPicker, known: &[Service], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let q = g.filter.read(cx).value().trim().to_lowercase();
        let mut card = kit::card().child(
            div()
                .px(px(14.))
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .child(kit::field("github-filter", &g.filter, window, cx))
                .child(kit::icon_button("github-refresh", "refresh").on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_github(true, w, cx))))
                .child(kit::button("github-close", "Close", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.repos.github = None;
                    cx.notify();
                }))),
        );
        card = match &g.list {
            Load::Ready(list) => {
                let rows: Vec<_> = list.iter().filter(|r| q.is_empty() || r.name_with_owner.to_lowercase().contains(&q)).take(60).collect();
                let mut body = div().id("github-list").max_h(px(320.)).overflow_y_scroll().p(px(6.)).flex().flex_col();
                for r in rows {
                    let added = known.iter().any(|s| s.name == r.name);
                    let repo = r.clone();
                    body = body.child(
                        div()
                            .px(px(10.))
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(6.))
                            .text_size(px(13.))
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                            .child(div().flex_1().text_color(c(theme::TEXT)).child(r.name_with_owner.clone()))
                            .when(r.is_private, |d| d.child(kit::tag("private", true)))
                            .child(if added {
                                div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("added").into_any_element()
                            } else {
                                kit::mini(SharedString::from(format!("github:{}", r.name_with_owner)), "Add", Kind::Secondary)
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.add_github(repo.clone(), w, cx)))
                                    .into_any_element()
                            }),
                    );
                }
                card.child(body)
            }
            Load::Failed(e) => card.child(div().p(px(14.)).child(kit::error_box(e.clone()))),
            _ => card.child(div().p(px(14.)).child(kit::hint("Loading your GitHub repositories…"))),
        };
        card.into_any_element()
    }

    pub(super) fn render_groups(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(cfg) = self.cfg().cloned() else { return kit::skeleton(gpui::relative(1.), 66.).into_any_element() };
        let create = |t: &mut Self, w: &mut Window, cx: &mut Context<Self>| {
            let name = text(&t.groups.new_group, cx);
            if name.is_empty() {
                return;
            }
            let n = name.clone();
            t.edit(
                "Couldn't create the group",
                move || settings::create_group(&n),
                |t, w, cx| t.groups.new_group.update(cx, |i, cx| i.set_value("", w, cx)),
                w,
                cx,
            );
        };
        let mut out = kit::section().child(
            kit::card()
                .p(px(16.))
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(kit::field("group-new", &self.groups.new_group, window, cx))
                .child(kit::button("group-create", "Create group", Kind::Primary).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| create(t, w, cx)))),
        );
        let mut names: Vec<&String> = cfg.groups.keys().collect();
        names.sort();
        if names.is_empty() {
            return out
                .child(kit::empty(if cfg.services.is_empty() {
                    "Add repositories first, then group them to reuse selections when creating workspaces."
                } else {
                    "No groups yet. Create one above to bundle repositories you often change together."
                }))
                .into_any_element();
        }
        let mut list = div().flex().flex_col().gap(px(10.));
        for g in names {
            let members = cfg.groups.get(g).cloned().unwrap_or_default();
            let editing = self.groups.editing.as_ref().filter(|(e, _)| e == g).map(|(_, m)| m.clone());
            let (g1, g2, m1) = (g.clone(), g.clone(), members.clone());
            let group = SharedString::from(format!("group-row-{g}"));
            let mut row = kit::card().p(px(16.)).gap(px(12.)).child(
                div()
                    .id(SharedString::from(format!("group:{g}")))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .child(div().mb(px(7.)).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(g.clone()))
                            .child(div().flex().flex_wrap().gap(px(6.)).children(if members.is_empty() {
                                vec![kit::tag("empty", true)]
                            } else {
                                members.iter().map(|m| kit::tag(m.clone(), false)).collect()
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .child(kit::link(format!("group:{g}:edit"), "Edit").on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                                t.groups.editing = Some((g1.clone(), m1.clone()));
                                cx.notify();
                            })))
                            .child(kit::link(format!("group:{g}:remove"), "Remove").text_color(c(theme::DANGER)).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                let me = cx.entity().downgrade();
                                let g = g2.clone();
                                t.host.confirm(
                                    "Remove group",
                                    format!("Remove group '{g}'?"),
                                    "Remove",
                                    move |w, cx| {
                                        let g = g.clone();
                                        let _ = me.update(cx, |t, cx| t.edit("Couldn't remove the group", move || settings::delete_group(&g), |_, _, _| {}, w, cx));
                                    },
                                    w,
                                    cx,
                                );
                            }))),
                    ),
            );
            if let Some(draft) = editing {
                let mut checks = div().flex().flex_wrap().gap(px(14.));
                for s in &cfg.services {
                    let name = s.name.clone();
                    checks = checks.child(kit::check(format!("group-member:{}", s.name), s.name.clone(), draft.contains(&s.name)).on_click(cx.listener(
                        move |t, _: &ClickEvent, _, cx| {
                            if let Some((_, m)) = &mut t.groups.editing {
                                if let Some(ix) = m.iter().position(|x| *x == name) {
                                    m.remove(ix);
                                } else {
                                    m.push(name.clone());
                                }
                            }
                            cx.notify();
                        },
                    )));
                }
                if cfg.services.is_empty() {
                    checks = checks.child(kit::hint("No repositories to add. Configure them in Repositories first."));
                }
                row = row.child(kit::hint(format!("{} of {} repositories selected", draft.len(), cfg.services.len()))).child(checks).child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .justify_end()
                        .child(kit::button("group-cancel", "Cancel", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.groups.editing = None;
                            cx.notify();
                        })))
                        .child(kit::button("group-save", "Save", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                            let Some((g, m)) = t.groups.editing.clone() else { return };
                            t.edit("Couldn't save the group", move || settings::set_group_members(&g, m), |t, _, _| t.groups.editing = None, w, cx);
                        }))),
                );
            }
            list = list.child(row);
        }
        out = out.child(list);
        out.into_any_element()
    }
}
