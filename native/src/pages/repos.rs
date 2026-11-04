//! Settings → Repositories and Groups (`ReposSection.tsx`, `FoldersCard.tsx`,
//! `RepoFormModal.tsx`, `GithubRepoModal.tsx`, `GroupsSection.tsx`). The
//! modals are inline cards here.
use super::settings::SettingsPage;
use super::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, SharedString, Window};
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

    pub(super) fn render_repos(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let Some(cfg) = self.cfg().cloned() else { return ui::faint("Loading…").into_any_element() };
        let mut out = div().flex().flex_col().gap_3();

        // Folders.
        out = out.child(ui::heading("Folders")).child(
            ui::card()
                .child(ui::setting(
                    "Clones",
                    "Where Orbit clones repositories (empty: the default).",
                    div().w(px(360.)).child(ui::input("folders-repos", &self.repos.repos_dir)),
                ))
                .child(ui::setting(
                    "Workspaces",
                    "Where workspaces and their worktrees live (empty: the default).",
                    div().w(px(360.)).child(ui::input("folders-workspaces", &self.repos.workspaces_dir)),
                ))
                .child(div().px_4().py_2().flex().justify_end().child(ui::button("folders-save", "Save folders", Tone::Primary).on_click(cx.listener(
                    |t, _: &ClickEvent, w, cx| {
                        let (r, ws) = (text(&t.repos.repos_dir, cx), text(&t.repos.workspaces_dir, cx));
                        t.edit(
                            "Couldn't save the folders",
                            move || settings::set_folders(Some(r), Some(ws)),
                            |t, w, cx| {
                                t.load_folders(w, cx);
                                t.host.toast(ToastKind::Success, "Folders saved", None, cx);
                            },
                            w,
                            cx,
                        )
                    },
                )))),
        );

        // Repositories.
        out = out.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(ui::heading(format!("Repositories · {}", cfg.services.len())))
                .child(div().flex_1())
                .child(ui::button("repo-add", "Add manually", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_form(None, w, cx))))
                .child(ui::button("repo-add-local", "Add local clone", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                    t.repos.form = None;
                    t.repos.github = None;
                    t.repos.local = Some(input(w, cx, "/path/to/an/existing/checkout", ""));
                    cx.notify();
                })))
                .child(ui::button("repo-add-github", "Add from GitHub", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_github(false, w, cx)))),
        );
        if let Some(form) = &self.repos.form {
            out = out.child(self.render_form(form, cx));
        }
        if let Some(local) = &self.repos.local {
            out = out.child(
                ui::card().p_3().flex().flex_col().gap_2().child(ui::muted("A folder with a git checkout: Orbit uses it as is and reads its origin.")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(ui::input("local-path", local))
                        .child(ui::button("local-cancel", "Cancel", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.repos.local = None;
                            cx.notify();
                        })))
                        .child(ui::button("local-add", "Add", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add_local(w, cx)))),
                ),
            );
        }
        if let Some(g) = &self.repos.github {
            out = out.child(self.render_github(g, &cfg.services, cx));
        }
        if cfg.services.is_empty() {
            out = out.child(ui::card().p_4().child(ui::muted(
                "No repositories yet. Add the repositories your workspaces will span. Orbit clones them once and creates worktrees per workspace.",
            )));
        } else {
            let mut list = ui::card();
            for s in &cfg.services {
                let st = self.repos.clone.get(&s.name).copied();
                let (n1, n2, n3, s1, s2) = (s.name.clone(), s.name.clone(), s.name.clone(), s.clone(), s.clone());
                let status: AnyElement = match st {
                    None => ui::faint("…").into_any_element(),
                    Some(CloneState::Cloned) => div().flex().items_center().gap_1().child(ui::dot(c(theme::OK))).child(ui::muted("Cloned")).into_any_element(),
                    Some(CloneState::Cloning) => ui::muted("Cloning…").into_any_element(),
                    Some(CloneState::Missing) => ui::button(format!("repo:{}:clone", s.name), "Clone", Tone::Plain)
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.clone_repo(n1.clone(), w, cx)))
                        .into_any_element(),
                };
                let mut title = div().flex().items_center().gap_2().child(div().text_sm().text_color(c(theme::TEXT)).child(s.name.clone()));
                if s.build.is_some() {
                    title = title.child(ui::pill("build", c(theme::TEXT_DIM)));
                }
                if !s.worktree {
                    title = title.child(ui::pill("branch in base clone", c(theme::TEXT_DIM)));
                }
                list = list.child(
                    div()
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .border_b_1()
                        .border_color(c(theme::BORDER))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(title)
                                .child(ui::faint(s.repo.clone()))
                                .when_some(s.path.clone(), |d, p| d.child(ui::faint(format!("📁 {p}")))),
                        )
                        .child(status)
                        .when(st == Some(CloneState::Cloned), |d| {
                            d.child(ui::button(format!("repo:{}:reveal", s.name), "Open folder", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.reveal_repo(&n2, cx))))
                        })
                        .child(ui::button(format!("repo:{}:edit", s.name), "Edit", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.open_form(Some(s1.clone()), w, cx))))
                        .child(ui::button(format!("repo:{}:remove", n3), "Remove", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.remove_repo(s2.clone(), w, cx)))),
                );
            }
            out = out.child(list);
        }
        out.into_any_element()
    }

    fn render_form(&self, f: &RepoForm, cx: &mut Context<Self>) -> AnyElement {
        let editing = f.editing.is_some();
        let cloned = f.editing.as_ref().is_some_and(|s| self.repos.clone.get(&s.name) == Some(&CloneState::Cloned));
        let per_os = matches!(f.editing.as_ref().and_then(|s| s.build.as_ref()), Some(BuildCmd::PerOs { .. }));
        let field = |label: &'static str, el: AnyElement| div().flex().items_center().gap_3().child(div().w(px(150.)).flex_none().text_xs().text_color(c(theme::TEXT_DIM)).child(label)).child(el);
        let mut card = ui::card()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().text_color(c(theme::TEXT)).child(match &f.editing {
                Some(s) => format!("Edit {}", s.name),
                None => "Add a repository".into(),
            }))
            .child(field("Name", if editing { ui::code(f.editing.as_ref().map(|s| s.name.clone()).unwrap_or_default()).into_any_element() } else { ui::input("form-name", &f.name) }))
            .child(field("Repository URL", ui::input("form-url", &f.url)));
        if !editing {
            card = card.child(field("Local path", ui::input("form-path", &f.path)));
        } else if let Some(p) = f.editing.as_ref().and_then(|s| s.path.clone()) {
            card = card.child(field("Local path", ui::code(if cloned { format!("{p} (cloned)") } else { p }).into_any_element()));
        }
        card = card
            .child(field(
                "Build command",
                if per_os { ui::muted("One command per OS (edit those in the Tauri app)").into_any_element() } else { ui::input("form-build", &f.build) },
            ))
            .child(field("Output folder", ui::input("form-output", &f.output)))
            .child(field("Share only", ui::input("form-shared", &f.shared)))
            .child(field(
                "Checkout",
                div()
                    .flex()
                    .gap_4()
                    .child(ui::check("form-worktree", "Worktree per workspace", !f.branch_only).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.branch_only = false;
                        }
                        cx.notify();
                    })))
                    .child(ui::check("form-branch-only", "Branch in the base clone", f.branch_only).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.branch_only = true;
                        }
                        cx.notify();
                    })))
                    .when(f.branch_only, |d| {
                        d.child(ui::check("form-auto-switch", "Switch branches automatically", f.auto_switch).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
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
                .gap_2()
                .pt_1()
                .when(!editing, |d| {
                    d.child(ui::check("form-clone-now", "Clone now", f.clone_now).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        if let Some(f) = &mut t.repos.form {
                            f.clone_now = !f.clone_now;
                        }
                        cx.notify();
                    })))
                })
                .child(div().flex_1())
                .child(ui::button("form-cancel", "Cancel", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.repos.form = None;
                    cx.notify();
                })))
                .child(ui::button("form-save", if editing { "Save" } else { "Add repository" }, Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.save_form(w, cx)))),
        )
        .into_any_element()
    }

    fn render_github(&self, g: &GithubPicker, known: &[Service], cx: &mut Context<Self>) -> AnyElement {
        let q = g.filter.read(cx).value().trim().to_lowercase();
        let mut card = ui::card().p_3().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(ui::input("github-filter", &g.filter))
                .child(ui::button("github-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.open_github(true, w, cx))))
                .child(ui::button("github-close", "Close", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.repos.github = None;
                    cx.notify();
                }))),
        );
        card = match &g.list {
            Load::Ready(list) => {
                let rows: Vec<_> = list.iter().filter(|r| q.is_empty() || r.name_with_owner.to_lowercase().contains(&q)).take(60).collect();
                let mut body = div().id("github-list").max_h(px(320.)).overflow_y_scroll().flex().flex_col();
                for r in rows {
                    let added = known.iter().any(|s| s.name == r.name);
                    let repo = r.clone();
                    body = body.child(
                        div()
                            .px_2()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(div().flex_1().text_color(c(theme::TEXT)).child(r.name_with_owner.clone()))
                            .when(r.is_private, |d| d.child(ui::pill("private", c(theme::TEXT_DIM))))
                            .child(if added {
                                ui::faint("added").into_any_element()
                            } else {
                                ui::button(SharedString::from(format!("github:{}", r.name_with_owner)), "Add", Tone::Plain)
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.add_github(repo.clone(), w, cx)))
                                    .into_any_element()
                            }),
                    );
                }
                card.child(body)
            }
            Load::Failed(e) => card.child(ui::error(e.clone())),
            _ => card.child(ui::faint("Loading your GitHub repositories…")),
        };
        card.into_any_element()
    }

    pub(super) fn render_groups(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(cfg) = self.cfg().cloned() else { return ui::faint("Loading…").into_any_element() };
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
        let mut out = div().flex().flex_col().gap_3().child(
            ui::card()
                .p_3()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ui::input("group-new", &self.groups.new_group))
                .child(ui::button("group-create", "Create group", Tone::Primary).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| create(t, w, cx)))),
        );
        let mut names: Vec<&String> = cfg.groups.keys().collect();
        names.sort();
        if names.is_empty() {
            out = out.child(ui::muted(if cfg.services.is_empty() {
                "Add repositories first, then group them to reuse selections when creating workspaces."
            } else {
                "No groups yet. Create one above to bundle repositories you often change together."
            }));
        }
        let mut list = ui::card();
        for g in names {
            let members = cfg.groups.get(g).cloned().unwrap_or_default();
            let editing = self.groups.editing.as_ref().filter(|(e, _)| e == g).map(|(_, m)| m.clone());
            let (g1, g2, m1) = (g.clone(), g.clone(), members.clone());
            let mut row = div().px_4().py_2().flex().flex_col().gap_2().border_b_1().border_color(c(theme::BORDER)).child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().text_color(c(theme::TEXT)).child(g.clone()))
                    .child(div().flex_1().flex().flex_wrap().gap_1().children(if members.is_empty() {
                        vec![ui::pill("empty", c(theme::TEXT_FAINT))]
                    } else {
                        members.iter().map(|m| ui::pill(m.clone(), c(theme::TEXT_DIM))).collect()
                    }))
                    .child(ui::button(format!("group:{g}:edit"), "Edit", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        t.groups.editing = Some((g1.clone(), m1.clone()));
                        cx.notify();
                    })))
                    .child(ui::button(format!("group:{g}:remove"), "Remove", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
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
            );
            if let Some(draft) = editing {
                let mut checks = div().flex().flex_wrap().gap_3();
                for s in &cfg.services {
                    let name = s.name.clone();
                    checks = checks.child(ui::check(format!("group-member:{}", s.name), s.name.clone(), draft.contains(&s.name)).on_click(cx.listener(
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
                    checks = checks.child(ui::muted("No repositories to add. Configure them in Repositories first."));
                }
                row = row.child(ui::faint(format!("{} of {} repositories selected", draft.len(), cfg.services.len()))).child(checks).child(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(ui::button("group-cancel", "Cancel", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.groups.editing = None;
                            cx.notify();
                        })))
                        .child(ui::button("group-save", "Save", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                            let Some((g, m)) = t.groups.editing.clone() else { return };
                            t.edit("Couldn't save the group", move || settings::set_group_members(&g, m), |t, _, _| t.groups.editing = None, w, cx);
                        }))),
                );
            }
            list = list.child(row);
        }
        out.child(list).into_any_element()
    }
}
