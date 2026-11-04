//! The Changes page: every changed file of every repo, the workspace's
//! REVIEW.md notes, and handing the open notes to an agent or a PR.
use super::ui::{self, Tone};
use super::{notes as rnotes, toolbar, Load, ReviewTab};
use crate::overlay::MenuItem;
use crate::sessions;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, ClickEvent, Context, MouseButton, MouseDownEvent, Window};
use orbit_core::git::ChangeEntry;
use orbit_core::notes::{Note, NotesFile};
use orbit_core::workspace::PrRef;
use std::path::PathBuf;

/// What one background refresh reads.
pub(super) struct Snapshot {
    changes: Vec<(String, Result<Vec<ChangeEntry>, String>)>,
    notes: Result<NotesFile, String>,
    pr_refs: Vec<PrRef>,
}

fn snapshot(ws: String, dir: PathBuf, repos: Vec<String>) -> Snapshot {
    let changes = repos.into_iter().map(|r| (r.clone(), orbit_core::git::changes(&dir.join(&r)))).collect();
    let pr_refs = orbit_core::workspace::load_meta(&dir).map(|m| m.pr_refs).unwrap_or_default();
    Snapshot { changes, notes: rnotes::list(&ws), pr_refs }
}

impl ReviewTab {
    pub(super) fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        if self.changes_busy {
            return;
        }
        self.changes_busy = true;
        for (_, l) in &mut self.changes {
            if matches!(l, Load::Idle) {
                *l = Load::Loading;
            }
        }
        let (ws, dir, repos) = (self.workspace.clone(), self.dir.clone(), self.repos.clone());
        cx.spawn(async move |this, cx| {
            let snap = cx.background_spawn(async move { snapshot(ws, dir, repos) }).await;
            let _ = this.update(cx, |t, cx| {
                t.changes_busy = false;
                t.apply_snapshot(snap);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snap: Snapshot) {
        self.changes = snap.changes.into_iter().map(|(r, res)| (r, res.map_or_else(Load::Failed, Load::Ready))).collect();
        match snap.notes {
            Ok(f) => self.notes = Some(f),
            Err(e) => self.message = Some(e),
        }
        self.pr_refs = snap.pr_refs;
    }

    pub fn open_notes(&self) -> Vec<&Note> {
        self.notes.iter().flat_map(|f| f.notes.iter()).filter(|n| !n.done).collect()
    }

    fn is_changed(&self, repo: &str, path: &str) -> bool {
        self.changes.iter().any(|(r, l)| r == repo && l.ready().is_some_and(|list| list.iter().any(|e| e.path == path)))
    }

    fn notes_result(&mut self, result: Result<NotesFile, String>, cx: &mut Context<Self>) {
        match result {
            Ok(f) => {
                self.notes = Some(f);
                self.message = None;
            }
            Err(e) => self.message = Some(e),
        }
        cx.notify();
    }

    pub fn set_note_done(&mut self, id: &str, done: bool, cx: &mut Context<Self>) {
        let r = rnotes::set_done(&self.workspace, id, done);
        self.notes_result(r, cx);
    }

    pub fn delete_notes(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let r = rnotes::delete(&self.workspace, ids);
        self.notes_result(r, cx);
    }

    /// A note's file: its diff while it has changes, else the file itself.
    fn jump_to_note(&self, note: &Note, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.dir.join(&note.repo).join(&note.path);
        let line = note.start;
        if self.is_changed(&note.repo, &note.path) {
            self.with_app(window, cx, |a, w, cx| super::open_diff_at(a, path, Some(line), w, cx));
        } else {
            self.with_app(window, cx, |a, w, cx| {
                if let Some(e) = a.open_file(path, false, w, cx) {
                    let p = lsp_types::Position::new(line.saturating_sub(1), 0);
                    e.update(cx, |e, cx| e.reveal(lsp_types::Range::new(p, p), w, cx));
                }
            });
        }
    }

    /// "Fix N notes with agent": the workspace's agent sessions, or a new one.
    fn fix_menu(&self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = &self.notes else { return };
        let prompt = rnotes::prompt(std::path::Path::new(&file.file));
        let count = self.open_notes().len();
        let ws = self.workspace.clone();
        let agents: Vec<(String, String)> = sessions::store(cx)
            .read(cx)
            .sessions()
            .filter(|(m, v)| m.workspace == ws && m.cmd.is_some() && v.is_some())
            .map(|(m, _)| (m.id.clone(), m.session_name.clone()))
            .collect();
        let mut items: Vec<MenuItem> = agents
            .into_iter()
            .map(|(id, name)| {
                let prompt = prompt.clone();
                MenuItem::new(format!("Send to {name}"), move |a, _, cx| {
                    let Some(view) = sessions::store(cx).read(cx).view(&id) else { return };
                    view.read(cx).send(prompt.as_bytes());
                    view.read(cx).send(b"\r");
                    let s = if count == 1 { "" } else { "s" };
                    a.toast(crate::toast::ToastKind::Success, format!("Sent {count} note{s} to {name}"), None, cx);
                })
            })
            .collect();
        items.push(MenuItem::new("New agent session", move |a, w, cx| {
            let bin = orbit_core::config::Config::load().map(|c| c.ai.agent_bin()).unwrap_or_else(|_| "claude".into());
            sessions::new_session(a, Some(&bin), Some(prompt.clone()), w, cx);
        }));
        let at = ev.position;
        self.with_app(window, cx, |a, w, cx| a.open_menu(at, items, w, cx));
    }

    pub(super) fn render_changes(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files: Vec<(String, ChangeEntry)> =
            self.changes.iter().flat_map(|(r, l)| l.ready().into_iter().flatten().map(move |e| (r.clone(), e.clone()))).collect();
        let loading = self.changes.iter().any(|(_, l)| matches!(l, Load::Loading | Load::Idle));
        let (add, del) = files.iter().fold((0, 0), |(a, d), (_, e)| (a + e.added, d + e.deleted));
        let open = self.open_notes().len();
        let has_done = self.notes.iter().flat_map(|f| &f.notes).any(|n| n.done);
        let pr_notes = self.pr_refs.iter().any(|p| self.open_notes().iter().any(|n| n.repo == p.repo));
        let s = |n: usize| if n == 1 { "" } else { "s" };

        let summary = div()
            .flex()
            .items_center()
            .gap_2()
            .child(format!("{} file{}", files.len(), s(files.len())))
            .child(div().text_color(c(theme::OK)).child(format!("+{add}")))
            .child(div().text_color(c(theme::DANGER)).child(format!("−{del}")))
            .when(loading, |d| d.child(div().text_color(c(theme::TEXT_FAINT)).child("…")));
        let bar = toolbar(summary)
            .when_some(self.message.clone(), |d, m| d.child(div().text_xs().text_color(c(theme::WARN)).child(m)))
            .child(ui::button("review-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.refresh(w, cx))))
            .when(pr_notes, |d| {
                d.child(
                    ui::button("review-send-pr", if self.sending { "Sending…" } else { "Send to PR…" }, Tone::Plain)
                        .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.send_to_pr(w, cx))),
                )
            })
            .child({
                let label = if open > 0 { format!("Fix {open} note{} with agent ▾", s(open)) } else { "No open notes".into() };
                let b = ui::button("review-fix", label, if open > 0 { Tone::Primary } else { Tone::Plain });
                if open > 0 {
                    b.on_mouse_down(MouseButton::Left, cx.listener(|t, ev: &MouseDownEvent, w, cx| t.fix_menu(ev, w, cx)))
                } else {
                    b
                }
            });

        // Changed files, per repo.
        let mut list = div().id("review-files").flex_1().min_w_0().overflow_y_scroll().pb_4();
        for (repo, load) in &self.changes {
            let header = ui::section(repo.clone());
            list = match load {
                Load::Ready(entries) if entries.is_empty() => list.child(header.child(div().font_weight(gpui::FontWeight::NORMAL).child("clean"))),
                Load::Ready(entries) => list.child(header).children(entries.iter().map(|e| self.file_row(repo, e, cx))),
                Load::Failed(e) => list.child(header).child(div().px_3().text_xs().text_color(c(theme::DANGER)).child(e.clone())),
                _ => list.child(header.child("…")),
            };
        }
        if !loading && files.is_empty() {
            list = list.child(
                div()
                    .p_6()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .text_color(c(theme::TEXT_DIM))
                    .child("No local changes")
                    .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("Edits made by you or an agent show up here as they happen.")),
            );
        }

        // Notes.
        let shown: Vec<Note> = self.notes.iter().flat_map(|f| &f.notes).filter(|n| self.show_done || !n.done).cloned().collect();
        let all = self.notes.as_ref().map_or(0, |f| f.notes.len());
        let mut notes = div()
            .id("review-notes")
            .w(px(380.))
            .flex_none()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .pb_4()
            .child(
                ui::section(format!("NOTES · {open} open"))
                    .child(div().flex_1())
                    .child(
                        ui::button("review-show-done", if self.show_done { "Hide resolved" } else { "Show resolved" }, Tone::Plain)
                            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                t.show_done = !t.show_done;
                                cx.notify();
                            })),
                    )
                    .when(has_done, |d| {
                        d.child(ui::button("review-clear-done", "Clear resolved", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            let ids: Vec<String> = t.notes.iter().flat_map(|f| &f.notes).filter(|n| n.done).map(|n| n.id.clone()).collect();
                            t.delete_notes(&ids, cx);
                        })))
                    }),
            );
        if shown.is_empty() {
            let hint = if all > 0 { "All notes resolved" } else { "Open a changed file and use + beside a line (or a selection) to leave a note" };
            notes = notes.child(div().px_3().py_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(hint));
        }
        for (k, n) in shown.into_iter().enumerate() {
            notes = notes.child(self.note_row(k + 1, n, cx));
        }
        if let Some(f) = &self.notes {
            notes = notes.child(div().px_3().pt_2().text_xs().text_color(c(theme::TEXT_FAINT)).child(f.file.clone()));
        }

        div().size_full().flex().flex_col().child(bar).child(div().flex_1().min_h_0().flex().child(list).child(notes))
    }

    fn file_row(&self, repo: &str, e: &ChangeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let (letter, color) = ui::status_letter(&e.status);
        let count = self.open_notes().iter().filter(|n| n.repo == repo && n.path == e.path).count();
        let (name, folder) = match e.path.rsplit_once('/') {
            Some((dir, name)) => (name.to_string(), dir.to_string()),
            None => (e.path.clone(), String::new()),
        };
        let path = self.dir.join(repo).join(&e.path);
        ui::row(format!("review-file:{repo}/{}", e.path))
            .mx_2()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .child(div().w(px(12.)).flex_none().text_xs().text_color(color).child(letter))
            .child(div().text_color(c(theme::TEXT)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(c(theme::TEXT_FAINT)).child(folder))
            .when(count > 0, |d| d.child(ui::pill(count.to_string(), c(theme::ACCENT))))
            .when(e.added > 0, |d| d.child(div().text_xs().text_color(c(theme::OK)).child(format!("+{}", e.added))))
            .when(e.deleted > 0, |d| d.child(div().text_xs().text_color(c(theme::DANGER)).child(format!("−{}", e.deleted))))
            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                let path = path.clone();
                t.with_app(w, cx, |a, w, cx| super::open_diff_at(a, path, None, w, cx));
            }))
    }

    fn note_row(&self, k: usize, n: Note, cx: &mut Context<Self>) -> impl IntoElement {
        let loc = format!(
            "{}:{}",
            n.path.rsplit('/').next().unwrap_or(&n.path),
            if n.start == n.end { n.start.to_string() } else { format!("{}–{}", n.start, n.end) }
        );
        let first = n.body.lines().next().unwrap_or_default().to_string();
        let (id_done, id_del, done) = (n.id.clone(), n.id.clone(), n.done);
        div()
            .mx_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                ui::button(format!("review-note:{k}:done"), if done { "✓" } else { " " }, Tone::Plain)
                    .w(px(20.))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.set_note_done(&id_done, !done, cx))),
            )
            .child(
                ui::row(format!("review-note:{k}"))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_xs().text_color(c(theme::ACCENT)).child(format!("{} · {loc}", n.repo)))
                    .child(div().truncate().text_sm().text_color(c(if done { theme::TEXT_FAINT } else { theme::TEXT })).child(first))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.jump_to_note(&n, w, cx))),
            )
            .child(
                ui::button(format!("review-note:{k}:delete"), "×", Tone::Plain)
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.delete_notes(std::slice::from_ref(&id_del), cx))),
            )
    }
}
