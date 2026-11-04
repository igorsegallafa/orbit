//! New workspace (`WorkspaceCreateModal.tsx`: manual, or from a branch that
//! already exists on origin) and Add repositories (`AddReposModal.tsx`).
//! orbit-core sets each repo up as its config says: a worktree per workspace,
//! or for a linked (branch-only) repo, the branch checked out in its base
//! clone with a link to it in the workspace folder. Both close at once and
//! report in a toast, as the slow part (clone, fetch, worktrees) runs.
use super::{plural, spawn, Modal, PageId, PageTab};
use crate::review::ui::{self, Tone};
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Window};
use gpui_component::input::{Input, InputState};
use orbit_core::config::Config;
use orbit_core::workspace;
use std::collections::BTreeSet;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Manual,
    Branch,
}

/// A configured repo: its name and whether it gets a worktree (else linked).
#[derive(Clone)]
pub struct RepoChoice {
    pub name: String,
    pub worktree: bool,
}

pub struct CreateForm {
    pub source: Source,
    pub name: Entity<InputState>,
    pub branch: Entity<InputState>,
    pub base: String,
    pub base_options: Vec<String>,
    pub repos: Vec<RepoChoice>,
    pub groups: Vec<(String, Vec<String>)>,
    pub selected: BTreeSet<String>,
    pub error: Option<String>,
}

pub struct AddReposForm {
    pub workspace: String,
    pub branch: String,
    pub available: Vec<RepoChoice>,
    pub selected: BTreeSet<String>,
}

/// `Checkout Improvements!` → `checkout-improvements`.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for ch in s.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

impl CreateForm {
    /// The branch typed, else `feat/<slug of the name>`.
    pub fn effective_branch(&self, cx: &App) -> String {
        let typed = self.branch.read(cx).value().trim().to_string();
        if !typed.is_empty() {
            return typed;
        }
        let slug = slugify(&self.name.read(cx).value());
        if slug.is_empty() { String::new() } else { format!("feat/{slug}") }
    }

    /// The name typed, else (from a branch) the branch without `feat/`.
    fn effective_name(&self, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_string();
        if !typed.is_empty() || self.source == Source::Manual {
            return typed;
        }
        let branch = self.branch.read(cx).value().trim().to_string();
        slugify(branch.strip_prefix("feat/").unwrap_or(&branch))
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(
            out,
            "  create form {:?}: name {:?} branch {:?} → {:?} base {} of {:?} repos {:?}{}",
            self.source,
            self.name.read(cx).value(),
            self.branch.read(cx).value(),
            self.effective_branch(cx),
            self.base,
            self.base_options,
            self.selected,
            self.error.as_ref().map(|e| format!(" error: {e}")).unwrap_or_default()
        );
    }
}

impl AddReposForm {
    pub fn dump(&self, out: &mut String) {
        let names: Vec<&str> = self.available.iter().map(|r| r.name.as_str()).collect();
        let _ = writeln!(out, "  add repos form ({}): available {names:?} selected {:?}", self.workspace, self.selected);
    }
}

fn choices(cfg: &Config) -> Vec<RepoChoice> {
    cfg.services.iter().map(|s| RepoChoice { name: s.name.clone(), worktree: s.worktree }).collect()
}

/// A checkbox row for a repo, with the linked-clone note.
fn repo_check(probe: String, r: &RepoChoice, on: bool) -> gpui::Stateful<gpui::Div> {
    ui::row(probe)
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .child(div().w(px(14.)).text_color(c(theme::ACCENT)).child(if on { "✓" } else { "○" }))
        .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(r.name.clone()))
        .when(!r.worktree, |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("linked: switches its base clone's branch")))
}

fn field(label: &'static str) -> gpui::Div {
    div().pt_1().text_xs().text_color(c(theme::TEXT_DIM)).child(label)
}

impl PageTab {
    fn create_form(&mut self) -> Option<&mut CreateForm> {
        match &mut self.modal {
            Some(Modal::Create(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_create(&mut self, repos: Option<Vec<String>>, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => return self.toast(ToastKind::Error, "Couldn't read the config", Some(e), window, cx),
        };
        let mut groups: Vec<(String, Vec<String>)> = cfg.groups.clone().into_iter().collect();
        groups.sort();
        self.modal = Some(Modal::Create(CreateForm {
            source: Source::Manual,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("checkout-improvements")),
            branch: cx.new(|cx| InputState::new(window, cx).placeholder("feat/… (from the name when empty)")),
            base: "main".into(),
            base_options: vec!["main".into(), "master".into()],
            repos: choices(&cfg),
            groups,
            selected: repos.unwrap_or_default().into_iter().collect(),
            error: None,
        }));
        self.load_base_options(window, cx);
        cx.notify();
    }

    /// Base branch suggestions from the first selected (cloned) repo.
    fn load_base_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        let repos: Vec<String> = f.selected.iter().cloned().collect();
        if repos.is_empty() {
            return;
        }
        spawn(window, cx, move || workspace::base_branches(&repos), |t, res, _, _| {
            let (Some(f), Ok(options)) = (t.create_form(), res) else { return };
            if !options.contains(&f.base) {
                f.base = options[0].clone();
            }
            f.base_options = options;
        });
    }

    fn create_toggle(&mut self, repo: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.create_form() {
            if !f.selected.remove(repo) {
                f.selected.insert(repo.to_string());
            }
        }
        self.load_base_options(window, cx);
        cx.notify();
    }

    fn create_toggle_group(&mut self, group: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.create_form() {
            let members = f.groups.iter().find(|(g, _)| g == group).map(|(_, m)| m.clone()).unwrap_or_default();
            let all_in = members.iter().all(|m| f.selected.contains(m));
            for m in members {
                if all_in {
                    f.selected.remove(&m);
                } else {
                    f.selected.insert(m);
                }
            }
        }
        self.load_base_options(window, cx);
        cx.notify();
    }

    fn create_pick_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        let page = cx.entity().downgrade();
        let items = f
            .base_options
            .iter()
            .map(|b| {
                let (b, page) = (b.clone(), page.clone());
                crate::overlay::MenuItem::new(b.clone(), move |_: &mut crate::app::OrbitApp, _: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
                    let b = b.clone();
                    let _ = page.update(cx, |t, cx| {
                        if let Some(f) = t.create_form() {
                            f.base = b;
                        }
                        cx.notify();
                    });
                })
            })
            .collect();
        let at = window.mouse_position();
        self.with_app(window, cx, move |a, w, cx| a.open_menu(at, items, w, cx));
    }

    fn create_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        let name = f.effective_name(cx);
        let branch = f.effective_branch(cx);
        let (source, base, repos) = (f.source, f.base.clone(), f.selected.iter().cloned().collect::<Vec<_>>());
        let missing = match source {
            Source::Branch if branch.is_empty() || name.is_empty() => Some("Enter the existing branch name."),
            Source::Manual if name.is_empty() || repos.is_empty() || branch.is_empty() => Some("Name and at least one repository are required."),
            _ => None,
        };
        if let Some(m) = missing {
            f.error = Some(m.into());
            return cx.notify();
        }
        self.close_modal(cx);
        let what = match source {
            Source::Manual => format!("{} on {branch}", plural(repos.len(), "repo")),
            Source::Branch => format!("Checking out {branch}"),
        };
        self.toast(ToastKind::Info, format!("Creating workspace {name}…"), Some(what), window, cx);
        let ws = name.clone();
        spawn(
            window,
            cx,
            move || match source {
                Source::Manual => workspace::create(&ws, &branch, &base, &repos, None).map(|w| (w, vec![])),
                Source::Branch => workspace::create_from_branch(&ws, &branch),
            },
            move |t, res, window, cx| match res {
                Ok((w, failures)) => {
                    if failures.is_empty() {
                        t.toast(ToastKind::Success, format!("Workspace {name} created"), Some(format!("{} on {}", plural(w.repos.len(), "repo"), w.branch)), window, cx);
                    } else {
                        t.toast(ToastKind::Info, format!("Workspace {name} created with problems"), Some(failures.join("\n")), window, cx);
                    }
                    t.reload(false, window, cx);
                    t.with_app(window, cx, move |a, w, cx| {
                        super::reload_workspaces(a, Some(&name), cx);
                        super::open(a, PageId::Workspace(name), w, cx);
                    });
                }
                Err(e) => t.toast(ToastKind::Error, format!("Couldn't create {name}"), Some(e), window, cx),
            },
        );
    }

    pub(super) fn render_create(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Modal::Create(f)) = &self.modal else { return div().into_any_element() };
        let from_branch = f.source == Source::Branch;
        let source = |id: &'static str, label: &'static str, s: Source, cx: &mut Context<Self>| {
            ui::button(id, label, if f.source == s { Tone::Primary } else { Tone::Plain }).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                if let Some(f) = t.create_form() {
                    f.source = s;
                    f.error = None;
                }
                cx.notify();
            }))
        };
        let branch_hint = if from_branch {
            "Sets up a branch that already exists on origin, in every cloned repository that has it.".to_string()
        } else {
            let b = f.effective_branch(cx);
            if b.is_empty() { String::new() } else { format!("Branch: {b}") }
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().flex().gap_1().child(source("create-source-manual", "Manual", Source::Manual, cx)).child(source(
                "create-source-branch",
                "Existing branch",
                Source::Branch,
                cx,
            )))
            .child(field("Workspace name"))
            .child(div().id("create-name").relative().child(Input::new(&f.name)).child(crate::probe::probe("create-name")))
            .child(field("Branch"))
            .child(div().id("create-branch").relative().child(Input::new(&f.branch)).child(crate::probe::probe("create-branch")))
            .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(branch_hint));
        if !from_branch {
            body = body
                .child(field("Base branch"))
                .child(div().flex().child(
                    ui::button("create-base", format!("{} ▾", f.base), Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.create_pick_base(w, cx))),
                ))
                .child(field("Repositories").child(format!(" · {} selected", f.selected.len())))
                .when(!f.groups.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().gap_1().children(f.groups.iter().map(|(g, members)| {
                        let all_in = !members.is_empty() && members.iter().all(|m| f.selected.contains(m));
                        let g2 = g.clone();
                        ui::button(format!("create-group:{g}"), g.clone(), if all_in { Tone::Primary } else { Tone::Plain })
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_toggle_group(&g2, w, cx)))
                    })))
                })
                .child(div().flex().flex_col().children(f.repos.iter().map(|r| {
                    let name = r.name.clone();
                    repo_check(format!("create-repo:{}", r.name), r, f.selected.contains(&r.name))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_toggle(&name, w, cx)))
                })))
                .when(f.repos.is_empty(), |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("No repositories configured. Add some in Settings first.")));
        }
        crate::git::card(520., "New workspace", None)
            .child(body)
            .when_some(f.error.clone(), |d, e| d.child(div().text_xs().text_color(c(theme::DANGER)).child(e)))
            .child(
                crate::git::footer()
                    .child(crate::git::cancel_button("Cancel", cx))
                    .child(ui::button("create-submit", "Create workspace", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.create_submit(w, cx)))),
            )
            .into_any_element()
    }

    pub fn open_add_repos(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let super::Page::Workspace(p) = &self.page else { return };
        let Some(meta) = p.meta.clone() else {
            return self.toast(ToastKind::Info, "Still reading the workspace", None, window, cx);
        };
        let cfg = Config::load().unwrap_or_default();
        let available = choices(&cfg).into_iter().filter(|r| !meta.repos.contains(&r.name)).collect();
        self.modal = Some(Modal::AddRepos(AddReposForm { workspace: meta.name, branch: meta.branch, available, selected: BTreeSet::new() }));
        cx.notify();
    }

    fn add_repos_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::AddRepos(f)) = &self.modal else { return };
        if f.selected.is_empty() {
            return;
        }
        let (ws, branch, repos) = (f.workspace.clone(), f.branch.clone(), f.selected.iter().cloned().collect::<Vec<_>>());
        self.close_modal(cx);
        self.toast(ToastKind::Info, format!("Adding {} to {ws}…", plural(repos.len(), "repo")), Some(format!("On {branch}")), window, cx);
        let (name, list) = (ws.clone(), repos.clone());
        spawn(window, cx, move || workspace::add_repos(&name, &list), move |t, res, window, cx| match res {
            Ok(_) => {
                t.toast(ToastKind::Success, format!("Added to {ws}"), Some(repos.join(", ")), window, cx);
                t.with_app(window, cx, |a, _, cx| super::reload_workspaces(a, None, cx));
                t.reload(false, window, cx);
            }
            Err(e) => t.toast(ToastKind::Error, format!("Couldn't add repos to {ws}"), Some(e), window, cx),
        });
    }

    pub(super) fn render_add_repos(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Modal::AddRepos(f)) = &self.modal else { return div().into_any_element() };
        let list = div().flex().flex_col().children(f.available.iter().map(|r| {
            let name = r.name.clone();
            repo_check(format!("add-repo:{}", r.name), r, f.selected.contains(&r.name)).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                if let Some(Modal::AddRepos(f)) = &mut t.modal {
                    if !f.selected.remove(&name) {
                        f.selected.insert(name.clone());
                    }
                }
                cx.notify();
            }))
        }));
        crate::git::card(460., "Add repositories", Some(format!("{} selected · on {}", f.selected.len(), f.branch).into()))
            .child(list)
            .when(f.available.is_empty(), |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("Every configured repository is already in this workspace.")))
            .child(
                crate::git::footer()
                    .child(crate::git::cancel_button("Cancel", cx))
                    .child(ui::button("add-repos-submit", "Add to workspace", Tone::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add_repos_submit(w, cx)))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugifies_names() {
        assert_eq!(super::slugify("  Checkout Improvements! "), "checkout-improvements");
        assert_eq!(super::slugify("a--b__c"), "a-b-c");
        assert_eq!(super::slugify("!!"), "");
    }
}
