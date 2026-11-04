//! New workspace (`WorkspaceCreateModal.tsx`: manual, from a branch that
//! already exists on origin, or from a Shortcut/Linear card) and Add
//! repositories (`AddReposModal.tsx`).
//! orbit-core sets each repo up as its config says: a worktree per workspace,
//! or for a linked (branch-only) repo, the branch checked out in its base
//! clone with a link to it in the workspace folder. Both close at once and
//! report in a toast, as the slow part (clone, fetch, worktrees) runs.
use super::kit;
use super::{plural, spawn, Modal, PageId, PageTab};
use crate::review::ui;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Window};
use gpui_component::input::{Input, InputEvent, InputState};
use orbit_core::config::Config;
use orbit_core::integrations::{self, CardInfo, TrackerKind};
use orbit_core::workspace::{self, CardRef};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Manual,
    Branch,
    Shortcut,
    Linear,
}

impl Source {
    fn tracker(self) -> Option<TrackerKind> {
        match self {
            Source::Shortcut => Some(TrackerKind::Shortcut),
            Source::Linear => Some(TrackerKind::Linear),
            Source::Manual | Source::Branch => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Branch => "branch",
            Source::Shortcut => "shortcut",
            Source::Linear => "linear",
        }
    }
}

/// The card picker of a tracker source.
pub struct CardPicker {
    pub query: Entity<InputState>,
    /// None while loading.
    pub cards: Option<Vec<CardInfo>>,
    pub picked: Option<CardInfo>,
    /// Which search the list belongs to: older answers are dropped.
    generation: u64,
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
    /// Trackers with a token (their sources are disabled otherwise).
    pub connected: Vec<TrackerKind>,
    pub picker: CardPicker,
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
        if let Some(card) = &self.picker.picked {
            return format!("feat/{}", card.id);
        }
        let slug = slugify(&self.name.read(cx).value());
        if slug.is_empty() { String::new() } else { format!("feat/{slug}") }
    }

    /// The name typed, else (from a branch) the branch without `feat/`.
    fn effective_name(&self, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_string();
        if !typed.is_empty() || self.source != Source::Branch {
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
        if self.source.tracker().is_some() {
            let p = &self.picker;
            let cards = p.cards.as_ref().map(|c| c.iter().map(|c| format!("{} {} [{}]", c.id, c.title, c.state)).collect::<Vec<_>>());
            let _ = writeln!(
                out,
                "  cards (query {:?}): {} · picked {:?}",
                p.query.read(cx).value(),
                cards.map_or("loading".into(), |c| format!("{c:?}")),
                p.picked.as_ref().map(|c| &c.id)
            );
        }
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

/// A checkbox row in the repositories list (`.wizard-repo`).
fn repo_check(probe: String, r: &RepoChoice, on: bool) -> gpui::Stateful<gpui::Div> {
    let _ = r.worktree;
    div()
        .id(gpui::ElementId::Name(probe.clone().into()))
        .relative()
        .h(px(34.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(12.))
        .text_size(px(13.))
        .text_color(c(theme::TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
        .child(kit::check(on, false))
        .child(r.name.clone())
        .child(crate::probe::probe(probe))
}

/// A form label (`.form label`).
fn field(label: &'static str) -> gpui::Div {
    div().text_size(px(12.5)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(label)
}

/// The repositories box with its "n selected" count.
fn repos_box(count: usize, rows: impl IntoIterator<Item = gpui::AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .justify_between()
                .child(field("Repositories"))
                .child(div().text_size(px(11.5)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::ACCENT)).child(format!("{count} selected"))),
        )
        .child(
            div()
                .id("repos-box")
                .max_h(px(280.))
                .overflow_y_scroll()
                .py(px(4.))
                .rounded(px(8.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .flex()
                .flex_col()
                .children(rows),
        )
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
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search cards…"));
        cx.subscribe_in(&query, window, |t: &mut Self, _, ev: &InputEvent, w, cx| {
            if matches!(ev, InputEvent::Change) {
                t.load_cards(true, w, cx);
            }
        })
        .detach();
        let connected = [TrackerKind::Shortcut, TrackerKind::Linear].into_iter().filter(|k| integrations::status(*k).connected).collect();
        self.modal = Some(Modal::Create(Box::new(CreateForm {
            source: Source::Manual,
            name: cx.new(|cx| InputState::new(window, cx).placeholder("checkout-improvements")),
            branch: cx.new(|cx| InputState::new(window, cx).placeholder("feat/… (from the name when empty)")),
            base: "main".into(),
            base_options: vec!["main".into(), "master".into()],
            repos: choices(&cfg),
            groups,
            selected: repos.unwrap_or_default().into_iter().collect(),
            error: None,
            connected,
            picker: CardPicker { query, cards: None, picked: None, generation: 0 },
        })));
        if let Some(Modal::Create(f)) = &self.modal {
            let (n, b) = (f.name.clone(), f.branch.clone());
            super::watch(&n, window, cx);
            super::watch(&b, window, cx);
        }
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

    fn create_source(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        if f.source == source {
            return;
        }
        if source.tracker().is_some_and(|k| !f.connected.contains(&k)) {
            f.error = Some(format!("Connect {} in Integrations first.", source.key()));
            return cx.notify();
        }
        f.source = source;
        f.error = None;
        // The card's pre-filled branch goes with it; typed text stays.
        if f.picker.picked.take().is_some() {
            f.branch.update(cx, |b, cx| b.set_value("", window, cx));
        }
        f.picker.query.update(cx, |q, cx| q.set_value("", window, cx));
        self.load_cards(false, window, cx);
        cx.notify();
    }

    /// Cards matching the query, after a pause in typing (`delay`).
    fn load_cards(&mut self, delay: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        let Some(kind) = f.source.tracker() else { return };
        f.picker.generation += 1;
        f.picker.cards = None;
        let generation = f.picker.generation;
        let current = move |t: &mut Self| t.create_form().is_some_and(|f| f.picker.generation == generation);
        cx.spawn_in(window, async move |this, cx| {
            if delay {
                cx.background_executor().timer(Duration::from_millis(300)).await;
            }
            let query = this.update(cx, |t, cx| current(t).then(|| t.create_form().map(|f| f.picker.query.read(cx).value().trim().to_string())).flatten());
            let Ok(Some(query)) = query else { return };
            let r = cx.background_executor().spawn(async move { integrations::fetch_cards(kind, &query) }).await;
            let _ = this.update_in(cx, |t, w, cx| {
                if !current(t) {
                    return;
                }
                let error = r.as_ref().err().cloned();
                if let Some(f) = t.create_form() {
                    f.picker.cards = Some(r.unwrap_or_default());
                }
                if let Some(e) = error {
                    t.toast(ToastKind::Error, "Couldn't load the cards", Some(e), w, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Picks a card: the name and branch are filled from it.
    fn create_pick_card(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.create_form() else { return };
        let Some(card) = f.picker.cards.as_ref().and_then(|c| c.iter().find(|c| c.id == id)).cloned() else { return };
        let name: String = slugify(&card.title).chars().take(40).collect();
        let branch = card.branch.clone().unwrap_or_else(|| format!("feat/{}", card.id));
        f.name.update(cx, |i, cx| i.set_value(name.trim_end_matches('-').to_string(), window, cx));
        f.branch.update(cx, |i, cx| i.set_value(branch, window, cx));
        f.picker.picked = Some(card);
        cx.notify();
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
        let card = f.picker.picked.as_ref().filter(|_| source.tracker().is_some()).map(|c| CardRef {
            kind: source.key().into(),
            id: c.id.clone(),
            title: c.title.clone(),
            url: c.url.clone(),
        });
        let missing = match source {
            Source::Branch if branch.is_empty() || name.is_empty() => Some("Enter the existing branch name."),
            Source::Branch => None,
            _ if name.is_empty() || repos.is_empty() || branch.is_empty() => Some("Name and at least one repository are required."),
            _ => None,
        };
        if let Some(m) = missing {
            f.error = Some(m.into());
            return cx.notify();
        }
        self.close_modal(cx);
        let what = match source {
            Source::Branch => format!("Checking out {branch}"),
            _ => format!("{} on {branch}", plural(repos.len(), "repo")),
        };
        self.toast(ToastKind::Info, format!("Creating workspace {name}…"), Some(what), window, cx);
        let ws = name.clone();
        spawn(
            window,
            cx,
            move || match source {
                Source::Branch => workspace::create_from_branch(&ws, &branch),
                _ => workspace::create(&ws, &branch, &base, &repos, card).map(|w| (w, vec![])),
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
        let source = |s: Source, label: &'static str, cx: &mut Context<Self>| {
            let off = s.tracker().is_some_and(|k| !f.connected.contains(&k));
            let on = f.source == s;
            div()
                .id(gpui::ElementId::Name(format!("create-source-{}", s.key()).into()))
                .relative()
                .flex_1()
                .h(px(34.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .border_1()
                .border_color(if on { c(theme::ACCENT) } else { c(theme::BORDER_STRONG) })
                .bg(if on { theme::accent_soft() } else { c(theme::PANEL_RAISED) })
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(if on { c(theme::TEXT) } else { c(theme::TEXT_DIM) })
                .when(off, |d| d.opacity(0.5))
                .cursor_pointer()
                .hover(|st| st.border_color(c(theme::ACCENT)))
                .child(label)
                .child(crate::probe::probe(format!("create-source-{}", s.key())))
                .when(off, |d| kit::hint(d, "Not connected: set it up in Integrations"))
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_source(s, w, cx)))
        };
        let branch_hint = if from_branch {
            "Sets up a branch that already exists on origin, in every cloned repository that has it.".to_string()
        } else {
            let b = f.effective_branch(cx);
            if b.is_empty() || !f.branch.read(cx).value().trim().is_empty() { String::new() } else { format!("Branch: {b}") }
        };
        let input = |id: &'static str, state: &Entity<InputState>| div().id(id).relative().child(Input::new(state)).child(crate::probe::probe(id));
        let mut body = div()
            .id("create-body")
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(20.))
            .py(px(18.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(source(Source::Manual, "Manual", cx))
                    .child(source(Source::Branch, "Existing branch", cx))
                    .child(source(Source::Shortcut, "Shortcut", cx))
                    .child(source(Source::Linear, "Linear", cx)),
            )
            .when(f.source.tracker().is_some(), |d| d.child(self.render_cards(f, cx)))
            .child(div().flex().flex_col().gap(px(8.)).child(field("Workspace name")).child(input("create-name", &f.name)));
        if from_branch {
            body = body.child(div().flex().flex_col().gap(px(8.)).child(field("Branch")).child(input("create-branch", &f.branch)));
        } else {
            body = body.child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(div().flex_1().flex().flex_col().gap(px(8.)).child(field("Branch")).child(input("create-branch", &f.branch)))
                    .child(
                        div().flex_1().flex().flex_col().gap(px(8.)).child(field("Base branch")).child(
                            div()
                                .id("create-base")
                                .relative()
                                .h(px(34.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(c(theme::BORDER_STRONG))
                                .bg(c(theme::BG))
                                .text_size(px(13.))
                                .cursor_pointer()
                                .hover(|s| s.border_color(c(theme::ACCENT)))
                                .child(f.base.clone())
                                .child(div().text_size(px(9.)).text_color(c(theme::TEXT_FAINT)).child("▾"))
                                .child(crate::probe::probe("create-base"))
                                .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.create_pick_base(w, cx))),
                        ),
                    ),
            );
        }
        body = body.when(!branch_hint.is_empty(), |d| d.child(div().mt(px(-6.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(branch_hint)));
        if !from_branch {
            body = body
                .when(!f.groups.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().gap(px(6.)).children(f.groups.iter().map(|(g, members)| {
                        let all_in = !members.is_empty() && members.iter().all(|m| f.selected.contains(m));
                        let g2 = g.clone();
                        kit::button(format!("create-group:{g}"), g.clone(), if all_in { kit::Kind::Primary } else { kit::Kind::Secondary }, kit::MINI)
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_toggle_group(&g2, w, cx)))
                    })))
                })
                .child(repos_box(
                    f.selected.len(),
                    f.repos.iter().map(|r| {
                        let name = r.name.clone();
                        repo_check(format!("create-repo:{}", r.name), r, f.selected.contains(&r.name))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_toggle(&name, w, cx)))
                            .into_any_element()
                    }),
                ))
                .when(f.repos.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("No repositories configured. Add some in Settings first.")));
        }
        let ready = !f.effective_name(cx).is_empty() && (from_branch || (!f.selected.is_empty() && !f.effective_branch(cx).is_empty()));
        kit::modal("create-modal", 500.)
            .child(kit::modal_header("New workspace"))
            .child(body.when_some(f.error.clone(), |d, e| d.child(div().text_size(px(12.)).text_color(c(theme::DANGER)).child(e))))
            .child(
                kit::modal_footer()
                    .child(crate::git::cancel_button("Cancel", cx))
                    .child(kit::disabled(kit::button("create-submit", "Create workspace", kit::Kind::Primary, kit::NORMAL), !ready).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.create_submit(w, cx)))),
            )
            .into_any_element()
    }


    fn render_cards(&self, f: &CreateForm, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &f.picker;
        let note = |text: String| div().text_xs().text_color(c(theme::TEXT_FAINT)).child(text).into_any_element();
        let rows = match &p.cards {
            // `WorkspaceCreateModal`'s three skeleton rows.
            None => [0.7, 0.55, 0.62].into_iter().map(|w| crate::motion::skeleton_line(44., w, 12.).into_any_element()).collect(),
            Some(cards) if cards.is_empty() => vec![note(format!("No cards found. Connect {} in Integrations or try another search.", f.source.key()))],
            Some(cards) => cards
                .iter()
                .map(|card| {
                    let picked = p.picked.as_ref().is_some_and(|x| x.id == card.id);
                    let id = card.id.clone();
                    ui::row(format!("create-card:{}", card.id))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(div().w(px(14.)).text_color(c(theme::ACCENT)).child(if picked { "✓" } else { "" }))
                        .child(div().font_family(theme::MONO_FONT).text_xs().text_color(c(theme::ACCENT)).child(card.id.clone()))
                        .child(div().flex_1().truncate().text_color(c(theme::TEXT)).child(card.title.clone()))
                        .when(!card.state.is_empty(), |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(card.state.clone())))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.create_pick_card(&id, w, cx)))
                        .into_any_element()
                })
                .collect(),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(field("Pick a card"))
            .child(div().id("create-card-search").relative().child(Input::new(&p.query)).child(crate::probe::probe("create-card-search")))
            .child(div().id("create-cards").flex().flex_col().max_h(px(220.)).overflow_y_scroll().children(rows))
            .when_some(p.picked.as_ref(), |d, card| {
                d.child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(format!("Selected {}: name and branch filled from the card.", card.id)))
            })
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
        let rows = f.available.iter().map(|r| {
            let name = r.name.clone();
            repo_check(format!("add-repo:{}", r.name), r, f.selected.contains(&r.name))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    if let Some(Modal::AddRepos(f)) = &mut t.modal {
                        if !f.selected.remove(&name) {
                            f.selected.insert(name.clone());
                        }
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
        let body = div()
            .px(px(20.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(repos_box(f.selected.len(), rows))
            .when(f.available.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Every configured repository is already in this workspace.")));
        kit::modal("add-repos-modal", 500.)
            .child(kit::modal_header("Add repositories"))
            .child(body)
            .child(
                kit::modal_footer()
                    .child(crate::git::cancel_button("Cancel", cx))
                    .child(
                        kit::disabled(kit::button("add-repos-submit", "Add to workspace", kit::Kind::Primary, kit::NORMAL), f.selected.is_empty())
                            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add_repos_submit(w, cx))),
                    ),
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
