//! A repository's page (`RepoPage.tsx`), on its base clone: branch and sync
//! state (fetch, pull, push or publish), commit (the commit flow on the
//! `@repo` scope), stash, a paused operation (continue, abort, resolve with
//! AI), branches (switch, create, delete), history (undo, revert), stashes
//! and the worktrees other workspaces hold.
use super::{plural, spawn, Page, PageTab};
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use crate::review::ui::{self, Tone};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, Window};
use gpui_component::input::{Input, InputState};
use orbit_core::git::CommitEntry;
use orbit_core::repo::{self, RepoOverview};
use std::fmt::Write as _;
use std::rc::Rc;

/// A deferred action on the page (dialog buttons, toolbar buttons).
type Action = Rc<dyn Fn(&mut PageTab, &mut Window, &mut Context<PageTab>)>;

pub struct RepoView {
    pub name: String,
    pub overview: Load<RepoOverview>,
    pub history: Load<Vec<CommitEntry>>,
    pub busy: Option<String>,
    pub new_branch: Entity<InputState>,
}

impl RepoView {
    pub fn new(name: &str, window: &mut Window, cx: &mut App) -> Self {
        Self {
            name: name.to_string(),
            overview: Load::Loading,
            history: Load::Loading,
            busy: None,
            new_branch: cx.new(|cx| InputState::new(window, cx).placeholder("new-branch-name")),
        }
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
        for c in self.history.ready().into_iter().flatten().take(5) {
            let _ = writeln!(out, "    commit {} {}", c.sha, c.message);
        }
        let _ = writeln!(out, "    new branch input {:?}", self.new_branch.read(cx).value());
    }
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
        let name = r.name.clone();
        spawn(
            window,
            cx,
            move || {
                if fetch {
                    let _ = repo::fetch(&name);
                }
                (repo::overview(&name), repo::history(&name, 40))
            },
            |t, (ov, hist), _, _| {
                if let Some(r) = t.repo_view() {
                    r.overview = ov.map_or_else(Load::Failed, Load::Ready);
                    r.history = hist.map_or_else(Load::Failed, Load::Ready);
                }
            },
        );
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
        let Some(changes) = self.repo_view().and_then(|r| r.overview.ready().map(|o| o.changes)) else { return };
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
        r.new_branch.update(cx, |i, cx| i.set_value("", window, cx));
        let b = name.clone();
        self.repo_run("Create branch", Some(format!("Created and switched to {name}")), move |n| repo::create_branch(n, &b, None), window, cx);
    }

    fn repo_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.repo_view().map(|r| r.name.clone()) else { return };
        self.open_commit(format!("@{name}"), vec![name], window, cx);
    }

    fn repo_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.repo_view().map(|r| r.name.clone()) else { return };
        self.open_push(format!("@{name}"), vec![name], vec![], window, cx);
    }

    pub(super) fn render_repo(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let Page::Repo(r) = &self.page else { return div().into_any_element() };
        let ov = match &r.overview {
            Load::Ready(o) => o,
            Load::Failed(e) => return div().p_4().text_sm().text_color(c(theme::DANGER)).child(e.clone()).into_any_element(),
            _ => return div().p_4().text_sm().text_color(c(theme::TEXT_FAINT)).child(format!("Reading {}…", r.name)).into_any_element(),
        };
        if !ov.cloned {
            return div().p_4().text_sm().text_color(c(theme::TEXT_DIM)).child(format!("{} is not cloned yet (Settings → Repositories clones it).", r.name)).into_any_element();
        }
        let busy = r.busy.is_some();
        let branch = ov.branch.clone().unwrap_or_else(|| format!("detached at {}", ov.head));
        let sync = match (&ov.upstream, ov.upstream_gone) {
            (None, _) => "no upstream".to_string(),
            (Some(u), true) => format!("{u} (gone)"),
            (Some(u), false) => format!("{u} ↑{} ↓{}", ov.ahead, ov.behind),
        };
        let btn = |id: &'static str, label: String, tone: Tone, f: Action, cx: &mut Context<PageTab>| {
            ui::button(id, label, tone).when(busy, |d| d.opacity(0.5)).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| f(t, w, cx)))
        };
        let publish = ov.upstream.is_none() || ov.upstream_gone;
        let head = div()
            .px_4()
            .pt_4()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_xl().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(r.name.clone()))
                    .child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(format!("{branch} · {sync} · {}", plural(ov.changes, "change"))))
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(ov.path.clone()))
                    .when_some(r.busy.clone(), |d, b| d.child(div().text_xs().text_color(c(theme::ACCENT)).child(format!("{b}…")))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_1()
                    .child(btn("repo-fetch", "Fetch".into(), Tone::Plain, Rc::new(|t, w, cx| t.load_repo(true, w, cx)), cx))
                    .when(ov.behind > 0, |d| {
                        d.child(btn("repo-pull", format!("Pull ↓{}", ov.behind), Tone::Plain, Rc::new(|t, w, cx| t.repo_run("Pull", Some("Pulled from origin".into()), |n| repo::pull(n, false), w, cx)), cx))
                            .child(btn("repo-pull-rebase", "Pull --rebase".into(), Tone::Plain, Rc::new(|t, w, cx| t.repo_run("Pull", Some("Pulled from origin".into()), |n| repo::pull(n, true), w, cx)), cx))
                    })
                    .when(ov.branch.is_some() && (ov.ahead > 0 || publish), |d| {
                        d.child(btn("repo-push", if publish { "Publish".into() } else { format!("Push ↑{}", ov.ahead) }, Tone::Plain, Rc::new(|t, w, cx| t.repo_push(w, cx)), cx))
                    })
                    .when(ov.changes > 0, |d| {
                        d.child(btn("repo-commit", "Commit…".into(), Tone::Primary, Rc::new(|t, w, cx| t.repo_commit(w, cx)), cx))
                            .child(btn("repo-stash", "Stash".into(), Tone::Plain, Rc::new(|t, w, cx| t.repo_run("Stash", Some("Changes stashed".into()), repo::stash, w, cx)), cx))
                    })
                    .child({
                        let name = r.name.clone();
                        btn("repo-new-ws", "New workspace…".into(), Tone::Plain, Rc::new(move |t, w, cx| t.open_create(Some(vec![name.clone()]), w, cx)), cx)
                    }),
            );
        let op = ov.operation.clone().map(|op| {
            div()
                .mx_4()
                .mt_3()
                .p_2()
                .rounded(px(8.))
                .border_1()
                .border_color(c(theme::WARN))
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().flex_1().text_color(c(theme::WARN)).child(if ov.conflicts.is_empty() {
                    format!("A {op} is in progress.")
                } else {
                    format!("A {op} is paused on conflicts: {}", ov.conflicts.join(", "))
                }))
                .when(!ov.conflicts.is_empty(), |d| {
                    d.child(btn(
                        "repo-op-ai",
                        "Resolve with AI".into(),
                        Tone::Plain,
                        Rc::new(|t, w, cx| {
                            t.repo_run("Resolve conflicts", Some("Conflicts resolved by the agent".into()), |n| orbit_core::agent::resolve_conflicts(&format!("@{n}"), n).map(|_| ()), w, cx)
                        }),
                        cx,
                    ))
                })
                .child(btn("repo-op-continue", "Continue".into(), Tone::Primary, Rc::new(|t, w, cx| t.repo_run("Continue", None, |n| repo::operation(n, "continue"), w, cx)), cx))
                .child(btn("repo-op-abort", "Abort".into(), Tone::Danger, Rc::new(|t, w, cx| t.repo_run("Abort", None, |n| repo::operation(n, "abort"), w, cx)), cx))
        });
        let branches = div().px_2().flex().flex_col().children(ov.branches.iter().map(|b| {
            let free = !b.current && b.worktree.is_none();
            let (n1, n2) = (b.name.clone(), b.name.clone());
            let detail = format!(
                "{}{}{} · {} · {}",
                b.upstream.as_deref().unwrap_or(if b.remote_only { "remote only" } else { "local" }),
                if b.gone { " (gone)" } else { "" },
                if b.ahead + b.behind > 0 { format!(" ↑{} ↓{}", b.ahead, b.behind) } else { String::new() },
                b.subject,
                b.updated
            );
            div()
                .px_2()
                .py(px(3.))
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().w(px(12.)).text_color(c(theme::OK)).child(if b.current { "●" } else { "" }))
                .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(b.name.clone()))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(c(theme::TEXT_FAINT)).child(detail))
                .when_some(b.worktree.clone(), |d, w| d.child(ui::pill(format!("worktree {w}"), c(theme::TEXT_DIM))))
                .when(free, |d| {
                    d.child(ui::button(format!("repo-switch:{}", b.name), "Switch", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.repo_switch(n1.clone(), w, cx))))
                })
                .when(free && !b.remote_only, |d| {
                    d.child(ui::button(format!("repo-delete:{}", b.name), "Delete", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let b = n2.clone();
                        let del: Action = Rc::new(move |t, w, cx| {
                            let b2 = b.clone();
                            t.repo_run("Delete branch", Some(format!("Deleted {b}")), move |n| repo::delete_branch(n, &b2, true), w, cx)
                        });
                        t.repo_ask("Delete branch".into(), format!("Delete {n2}? Commits only on it are lost."), vec![("Delete", del)], w, cx)
                    })))
                })
        }));
        let new_branch = div()
            .px_4()
            .pt_1()
            .flex()
            .items_center()
            .gap_2()
            .child(div().id("repo-branch-name").relative().w(px(260.)).child(Input::new(&r.new_branch)).child(crate::probe::probe("repo-branch-name")))
            .child(btn("repo-create-branch", "Create branch".into(), Tone::Plain, Rc::new(|t, w, cx| t.repo_create_branch(w, cx)), cx));
        let history = match &r.history {
            Load::Ready(list) => div().px_2().flex().flex_col().children(list.iter().enumerate().map(|(i, cm)| {
                let (sha, msg) = (cm.sha.clone(), cm.message.clone());
                div()
                    .px_2()
                    .py(px(2.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(div().font_family(theme::MONO_FONT).text_color(c(theme::ACCENT)).child(cm.sha.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT)).child(cm.message.clone()))
                    .child(div().text_color(c(theme::TEXT_FAINT)).child(format!("{} · {}", cm.author, cm.when)))
                    .when(i == 0, |d| {
                        d.child(btn(
                            "repo-undo",
                            "Undo".into(),
                            Tone::Plain,
                            Rc::new(|t, w, cx| t.repo_run("Undo commit", Some("Commit undone; its changes are back in the working tree".into()), repo::undo_commit, w, cx)),
                            cx,
                        ))
                    })
                    .child(ui::button(format!("repo-revert:{}", cm.sha), "Revert", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let (sha, msg) = (sha.clone(), msg.clone());
                        let go: Action = Rc::new(move |t, w, cx| {
                            let s = sha.clone();
                            t.repo_run("Revert", Some(format!("Reverted \"{msg}\"")), move |n| repo::revert(n, &s), w, cx)
                        });
                        t.repo_ask("Revert commit".into(), "Add a commit that undoes this one?".into(), vec![("Revert", go)], w, cx)
                    })))
            })),
            Load::Failed(e) => div().px_4().text_xs().text_color(c(theme::DANGER)).child(e.clone()),
            _ => div().px_4().text_xs().text_color(c(theme::TEXT_FAINT)).child("Loading…"),
        };
        let stashes = div().px_2().flex().flex_col().children(ov.stashes.iter().map(|s| {
            let ix = s.index;
            div()
                .px_2()
                .py(px(2.))
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .child(div().flex_1().text_color(c(theme::TEXT)).child(s.message.clone()))
                .child(div().text_color(c(theme::TEXT_FAINT)).child(s.when.clone()))
                .child(ui::button(format!("repo-stash-pop:{ix}"), "Restore", Tone::Plain).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    t.repo_run("Restore stash", Some("Stash restored".into()), move |n| repo::stash_pop(n, ix), w, cx)
                })))
                .child(ui::button(format!("repo-stash-drop:{ix}"), "Drop", Tone::Danger).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    let go: Action =
                        Rc::new(move |t, w, cx| t.repo_run("Drop stash", Some("Stash dropped".into()), move |n| repo::stash_drop(n, ix), w, cx));
                    t.repo_ask("Drop stash".into(), "Throw this stash away?".into(), vec![("Drop", go)], w, cx)
                })))
        }));
        let missing = ov.worktrees.iter().any(|w| w.missing);
        let worktrees = div().px_2().flex().flex_col().children(ov.worktrees.iter().filter(|w| !w.main).map(|w| {
            div()
                .px_2()
                .py(px(2.))
                .flex()
                .gap_2()
                .text_xs()
                .child(div().text_color(c(theme::TEXT)).child(w.workspace.clone().unwrap_or_else(|| w.path.clone())))
                .child(div().text_color(c(theme::TEXT_FAINT)).child(w.branch.clone().unwrap_or_default()))
                .when(w.missing, |d| d.child(ui::pill("missing", c(theme::WARN))))
        }));
        div()
            .id("repo-page")
            .size_full()
            .overflow_y_scroll()
            .pb_6()
            .child(head)
            .children(op)
            .child(ui::section(format!("BRANCHES · {}", ov.branches.len())))
            .child(branches)
            .child(new_branch)
            .child(ui::section("HISTORY"))
            .child(history)
            .when(!ov.stashes.is_empty(), |d| d.child(ui::section(format!("STASHES · {}", ov.stashes.len()))).child(stashes))
            .when(ov.worktrees.len() > 1, |d| {
                d.child(ui::section("WORKTREES").when(missing, |s| {
                    s.child(btn("repo-prune", "Prune".into(), Tone::Plain, Rc::new(|t, w, cx| t.repo_run("Prune worktrees", None, repo::prune_worktrees, w, cx)), cx))
                }))
                .child(worktrees)
            })
            .into_any_element()
    }
}
