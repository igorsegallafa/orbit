//! REVIEW.md notes in the PR review: answered inline, fixed by an agent
//! session, or turned into pending GitHub comments.
use super::super::notes::{self as rnotes, Anchor};
use super::super::send::{self, Candidate};
use super::pr_ui::{self, lucide, tint, Tone};
use super::{check_box, load_saved, save, thread_card, ComposerKind, Draft, PrReviewPage, Sel};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, SharedString, Window};
use gpui_component::diff::DiffSide;
use orbit_core::notes::{Note, NotesFile};
use orbit_core::workspace::PrRef;
use std::collections::HashSet;
use std::path::Path;

/// A note ready for a PR: `(repo, number, draft, note id)`.
type Ready = (String, u64, Draft, String);

/// The ready candidates, and why the others can't go.
fn drafts_of(cands: &[Candidate]) -> (Vec<Ready>, Vec<String>) {
    let (mut ready, mut problems) = (vec![], vec![]);
    for c in cands {
        match &c.problem {
            Some(p) => problems.push(format!("{}/{}:{}: {p}", c.note.repo, c.note.path, c.note.start)),
            None => {
                let d = send::comment(c);
                let draft = Draft { id: format!("note-{}", c.note.id), path: d.path, side: d.side, line: d.line, start_line: d.start_line, body: d.body };
                ready.push((c.pr.repo.clone(), c.pr.number, draft, c.note.id.clone()));
            }
        }
    }
    (ready, problems)
}

impl PrReviewPage {
    pub(super) fn ws_name(&self) -> Option<String> {
        self.workspace.as_ref().map(|w| w.meta.name.clone())
    }

    /// Notes go on the new code of a PR checked out in a workspace.
    pub(super) fn can_note(&self, side: DiffSide) -> bool {
        self.workspace.is_some() && side == DiffSide::Modified
    }

    pub(super) fn reload_notes(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.ws_name() else {
            self.notes = None;
            return;
        };
        self.notes_stamp = rnotes::stamp(&ws).and_then(|(_, t)| t);
        match rnotes::list(&ws) {
            Ok(f) => self.notes = Some(f),
            Err(e) => self.host.error("Couldn't read the review notes", e, cx),
        }
        cx.notify();
    }

    pub(super) fn poll_notes(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.ws_name() else { return };
        if rnotes::stamp(&ws).and_then(|(_, t)| t) != self.notes_stamp {
            self.reload_notes(cx);
        }
    }

    fn notes_result(&mut self, r: Result<NotesFile, String>, cx: &mut Context<Self>) -> bool {
        match r {
            Ok(f) => {
                self.notes = Some(f);
                self.notes_stamp = self.ws_name().and_then(|ws| rnotes::stamp(&ws)).and_then(|(_, t)| t);
                cx.notify();
                true
            }
            Err(e) => {
                self.host.error("Couldn't update REVIEW.md", e, cx);
                false
            }
        }
    }

    /// This PR's notes on `path` (open ones unless "Show done"), where their code is at the head.
    pub(super) fn file_notes(&self, path: &str) -> Vec<(Note, Anchor)> {
        let repo = &self.pr().repo;
        let mut out: Vec<(Note, Anchor)> = self
            .notes
            .iter()
            .flat_map(|f| &f.notes)
            .filter(|n| &n.repo == repo && n.path == path && (self.show_done || !n.done))
            .map(|n| (n.clone(), rnotes::anchor(n, Some(&self.head_text))))
            .collect();
        out.sort_by_key(|(_, a)| (a.end, a.start));
        out
    }

    pub(super) fn open_notes_of(&self, path: &str) -> usize {
        rnotes::open(self.notes.as_ref()).iter().filter(|n| n.repo == self.pr().repo && n.path == path).count()
    }

    pub(super) fn add_note(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(body), Sel::File(path)) = (self.ws_name(), self.composer_text(cx), self.selected.clone()) else { return };
        let Some(ComposerKind::New { start, end, .. }) = self.composer.as_ref().map(|c| c.kind.clone()) else { return };
        let (start, end) = (start as u32, end as u32);
        let snippet = rnotes::snippet(&self.head_text, start, end);
        let r = rnotes::add(&ws, &self.pr().repo, &path, start, end, &body, snippet);
        if self.notes_result(r, cx) {
            self.cancel_composer(cx);
        }
    }

    /// Saves the composer as a note's new text, or appends it as a reply.
    pub(super) fn edit_note(&mut self, id: &str, reply: bool, cx: &mut Context<Self>) {
        let (Some(ws), Some(text)) = (self.ws_name(), self.composer_text(cx)) else { return };
        let r = match self.notes.iter().flat_map(|f| &f.notes).find(|n| n.id == id) {
            Some(n) if reply => rnotes::reply(&ws, n, &text),
            Some(_) => rnotes::edit(&ws, id, &text),
            None => Err("that note changed meanwhile; try again".into()),
        };
        if self.notes_result(r, cx) {
            self.cancel_composer(cx);
        }
    }

    fn set_note_done(&mut self, id: &str, done: bool, cx: &mut Context<Self>) {
        let Some(ws) = self.ws_name() else { return };
        let r = rnotes::set_done(&ws, id, done);
        self.notes_result(r, cx);
    }

    fn delete_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ws) = self.ws_name() else { return };
        let r = rnotes::delete(&ws, &[id.to_string()]);
        self.notes_result(r, cx);
    }

    fn fix_menu(&self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), Some(file)) = (self.ws_name(), &self.notes) else { return };
        let items = rnotes::fix_menu(&ws, Path::new(&file.file), rnotes::open(Some(file)).len(), cx);
        let at = ev.position;
        super::super::code_review::with_app(&self.host, window, cx, |a, w, cx| a.open_menu(at, items, w, cx));
    }

    /// Notes already turned into drafts, in any of this tab's PRs.
    fn sent_ids(&self) -> HashSet<String> {
        self.prs.iter().flat_map(|p| load_saved(&format!("{}#{}", p.owner_repo, p.number)).sent).chain(self.saved.sent.iter().cloned()).collect()
    }

    /// Open notes of this tab's repos not sent to GitHub yet.
    fn sendable(&self) -> Vec<Note> {
        let sent = self.sent_ids();
        rnotes::open(self.notes.as_ref())
            .into_iter()
            .filter(|n| self.prs.iter().any(|p| p.repo == n.repo) && !sent.contains(&n.id))
            .cloned()
            .collect()
    }

    /// Open notes become pending comments of their PR; sent ones are ticked so nothing takes them again.
    fn send_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), notes) = (self.ws_name(), self.sendable()) else { return };
        if notes.is_empty() || self.sending_notes {
            return;
        }
        let refs: Vec<PrRef> = self.prs.iter().map(|p| PrRef { repo: p.repo.clone(), number: p.number, url: p.url.clone() }).collect();
        self.sending_notes = true;
        cx.spawn_in(window, async move |this, cx| {
            let ws2 = ws.clone();
            let cands = cx.background_spawn(async move { send::plan(&ws2, &refs, &notes) }).await;
            let _ = this.update(cx, |p, cx| {
                p.sending_notes = false;
                let (ready, problems) = drafts_of(&cands);
                let mut ids = vec![];
                for (repo, number, draft, note) in ready {
                    let Some(pr) = p.prs.iter().find(|x| x.repo == repo && x.number == number) else { continue };
                    let key = format!("{}#{}", pr.owner_repo, number);
                    if key == p.key() {
                        p.saved.drafts.push(draft);
                        p.saved.sent.push(note.clone());
                    } else {
                        let mut saved = load_saved(&key);
                        saved.drafts.push(draft);
                        saved.sent.push(note.clone());
                        save(&key, &saved);
                    }
                    ids.push(note);
                }
                p.persist();
                for id in &ids {
                    let r = rnotes::set_done(&ws, id, true);
                    p.notes_result(r, cx);
                }
                let n = ids.len();
                let s = if n == 1 { "" } else { "s" };
                let detail = (!problems.is_empty()).then(|| format!("Kept open (not on the PR's changed lines):\n{}", problems.join("\n")));
                if n > 0 {
                    p.host.toast(ToastKind::Success, format!("{n} note{s} added to your pending review"), detail.or(Some("Press Review to submit them to GitHub".into())), cx);
                } else {
                    p.host.toast(ToastKind::Info, "No note could go to GitHub", detail, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Fix N notes with agent ▾" in the file bar: every open note of the workspace.
    pub(super) fn fix_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.workspace.as_ref()?;
        let open = rnotes::open(self.notes.as_ref()).len();
        if open == 0 {
            return None;
        }
        let s = if open == 1 { "" } else { "s" };
        Some(
            pr_ui::tip(
                pr_ui::btn_mini("pr-fix-notes", format!("Fix {open} note{s} with agent ▾"))
                    .on_mouse_down(MouseButton::Left, cx.listener(|p, ev: &MouseDownEvent, w, cx| p.fix_menu(ev, w, cx))),
                "An agent session works through the open notes in REVIEW.md and ticks each one; done notes are skipped",
            )
            .into_any_element(),
        )
    }

    /// The side's NOTES block: this PR's notes, the done toggle, and the ways out.
    pub(super) fn render_notes_side(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ws = self.ws_name()?;
        let repo = self.pr().repo.clone();
        let mine: Vec<&Note> = self.notes.iter().flat_map(|f| &f.notes).filter(|n| n.repo == repo).collect();
        let open = mine.iter().filter(|n| !n.done).count();
        let done = mine.len() - open;
        let sendable = self.sendable().len();
        let sent: HashSet<String> = self.saved.sent.iter().cloned().collect();
        let link = |id: &'static str, label: String| {
            div()
                .id(id)
                .relative()
                .text_size(px(11.5))
                .text_color(c(theme::ACCENT))
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(label)
                .child(probe(id))
        };
        let mut block = div()
            .flex()
            .flex_col()
            .p(px(6.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(theme::TEXT_FAINT))
                    .child("NOTES")
                    .child(div().mr_auto().font_weight(FontWeight::MEDIUM).child(format!("{open} open")))
                    .when(done > 0, |d| {
                        d.child(
                            link("pr-notes-done", if self.show_done { "Hide done".into() } else { format!("Show done ({done})") }).on_click(cx.listener(|p, _, _, cx| {
                                p.show_done = !p.show_done;
                                cx.notify();
                            })),
                        )
                    }),
            );
        if mine.is_empty() {
            block = block.child(
                div().px(px(10.)).py(px(4.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("Notes for an agent, kept in {ws}'s REVIEW.md. Click + beside a line to leave one.")),
            );
        }
        for n in mine.into_iter().filter(|n| self.show_done || !n.done) {
            let (id, done, path) = (n.id.clone(), n.done, n.path.clone());
            let loc = format!("{}:{}", n.path.rsplit('/').next().unwrap_or(&n.path), n.start);
            let sent_here = sent.contains(&n.id);
            block = block.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(3.))
                    .child(
                        div()
                            .id(SharedString::from(format!("pr-side-note-done:{}", n.start)))
                            .relative()
                            .mt(px(2.))
                            .cursor_pointer()
                            .child(check_box(done))
                            .child(probe(format!("pr-side-note-done:{}", n.start)))
                            .on_click(cx.listener(move |p, _, _, cx| p.set_note_done(&id, !done, cx))),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("pr-side-note:{}", n.start)))
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .cursor_pointer()
                            .rounded(px(4.))
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .font_family(theme::MONO_FONT)
                                    .text_size(px(11.))
                                    .text_color(c(theme::TEXT_FAINT))
                                    .child(loc)
                                    .when(sent_here, |d| d.child(div().font_family(theme::UI_FONT).text_color(c(theme::INFO)).child("sent to GitHub"))),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(c(if done { theme::TEXT_FAINT } else { theme::TEXT }))
                                    .when(done, |d| d.line_through())
                                    .child(n.body.lines().next().unwrap_or_default().to_string()),
                            )
                            .child(probe(format!("pr-side-note:{}", n.start)))
                            .on_click(cx.listener(move |p, _, window, cx| p.select(Sel::File(path.clone()), window, cx))),
                    ),
            );
        }
        let ws2 = ws.clone();
        block = block.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(10.))
                .pt(px(8.))
                .pb(px(4.))
                .when(sendable > 0, |d| {
                    let s = if sendable == 1 { "" } else { "s" };
                    let label = if self.sending_notes { "Finding the notes in the PR…".to_string() } else { format!("Send {sendable} note{s} to GitHub review") };
                    d.child(pr_ui::tip(
                        link("pr-notes-send", label).on_click(cx.listener(|p, _, window, cx| p.send_notes(window, cx))),
                        "Open notes become pending comments of this feature's PRs (submit them with Review); sent notes are ticked here",
                    ))
                })
                .child(link("pr-review-changes", "Review local changes →".into()).on_click(cx.listener(move |p, _, window, cx| {
                    let name = ws2.clone();
                    super::super::code_review::with_app(&p.host, window, cx, move |a, w, cx| super::super::open_for(a, &name, super::super::Page::Changes, w, cx));
                }))),
        );
        Some(block.into_any_element())
    }

    /// Notes whose code isn't in the pushed head, above the diff.
    pub(super) fn render_lost_notes(&self, lost: &[(Note, Anchor)], cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("pr-lost-notes")
            .max_h(gpui::relative(0.38))
            .overflow_y_scroll()
            .px(px(14.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(c(theme::BG))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child("NOTES ON CODE THAT ISN'T IN THE PUSHED HEAD"))
            .children(lost.iter().map(|(n, at)| div().flex_none().child(self.render_note(n.clone(), *at, cx))))
            .into_any_element()
    }

    /// A REVIEW.md note's card: tick, reply, edit, delete.
    pub(super) fn render_note(&self, n: Note, at: Anchor, cx: &mut Context<Self>) -> AnyElement {
        let editing = matches!(self.composer.as_ref().map(|c| &c.kind), Some(ComposerKind::EditNote(id)) if *id == n.id);
        let replying = matches!(self.composer.as_ref().map(|c| &c.kind), Some(ComposerKind::ReplyNote(id)) if *id == n.id);
        let sent = self.saved.sent.contains(&n.id);
        let k = n.start;
        let (id_done, id_reply, id_edit, id_del, body, done) = (n.id.clone(), n.id.clone(), n.id.clone(), n.id.clone(), n.body.clone(), n.done);
        let tool = |name: String, icon: &'static str, tip: &'static str| {
            pr_ui::tip(div().id(SharedString::from(name.clone())).relative().cursor_pointer().child(lucide(icon, 12., c(theme::TEXT_FAINT))).child(probe(name)), tip)
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .child(
                div()
                    .id(SharedString::from(format!("pr-note-done:{k}")))
                    .relative()
                    .cursor_pointer()
                    .child(check_box(done))
                    .child(probe(format!("pr-note-done:{k}")))
                    .on_click(cx.listener(move |p, _, _, cx| p.set_note_done(&id_done, !done, cx))),
            )
            .child(div().font_weight(FontWeight::BOLD).child("Note"))
            .child(pr_ui::decision_pill("REVIEW.md", Tone::Muted))
            .when(done && !sent, |d| d.child(pr_ui::decision_pill("Done", Tone::Ok)))
            .when(sent, |d| d.child(pr_ui::decision_pill("Sent to GitHub", Tone::Muted)))
            .when(at.moved, |d| d.child(pr_ui::decision_pill("moved", Tone::Muted)))
            .when(at.lost, |d| d.child(pr_ui::decision_pill("not pushed", Tone::Warn)))
            .when(at.start != at.end, |d| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("Lines {}–{}", at.start, at.end))))
            .when(at.lost, |d| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("{}:{}", n.path, n.start))))
            .when(!editing && !replying, |d| {
                d.child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            tool(format!("pr-note-reply:{k}"), "reply", "Reply (reopens the note)")
                                .on_click(cx.listener(move |p, _, window, cx| p.open_composer(ComposerKind::ReplyNote(id_reply.clone()), "", window, cx))),
                        )
                        .child(
                            tool(format!("pr-note-edit:{k}"), "pencil", "Edit note")
                                .on_click(cx.listener(move |p, _, window, cx| p.open_composer(ComposerKind::EditNote(id_edit.clone()), &body, window, cx))),
                        )
                        .child(tool(format!("pr-note-del:{k}"), "trash", "Delete note").on_click(cx.listener(move |p, _, _, cx| p.delete_note(&id_del, cx)))),
                )
            });
        let mut main = div().flex_1().min_w_0().flex().flex_col().gap(px(4.)).child(head);
        if editing {
            main = main.child(self.render_composer(cx));
        } else {
            main = main.children(n.body.split("\n\n").map(|para| {
                let reply = para.strip_prefix(rnotes::REPLY_PREFIX);
                div()
                    .text_size(px(12.5))
                    .text_color(c(if done { theme::TEXT_DIM } else { theme::TEXT }))
                    .when(reply.is_some(), |d| d.pl_3().border_l_2().border_color(c(theme::BORDER_STRONG)))
                    .child(reply.unwrap_or(para).to_string())
            }));
        }
        if replying {
            main = main.child(div().pt(px(4.)).child(self.render_composer(cx)));
        }
        thread_card(false)
            .border_color(tint(theme::PURPLE, 0.45))
            .when(done, |d| d.opacity(0.6))
            .child(div().px(px(12.)).py(px(10.)).child(main))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, start: u32, end: u32, problem: Option<&str>) -> Candidate {
        Candidate {
            note: Note { id: id.into(), done: false, repo: "api".into(), path: "src/a.rs".into(), start, end, body: format!("fix {id}"), snippet: String::new() },
            pr: PrRef { repo: "api".into(), number: 7, url: String::new() },
            owner_repo: "o/api".into(),
            head_sha: "h".into(),
            start,
            end,
            problem: problem.map(str::to_string),
        }
    }

    #[test]
    fn ready_notes_become_drafts_on_the_head_lines_and_the_rest_are_reported() {
        let (ready, problems) = drafts_of(&[cand("n1", 12, 14, None), cand("n2", 20, 20, None), cand("n3", 3, 3, Some("outside the PR's changed lines"))]);
        assert_eq!(ready.len(), 2);
        let (repo, number, d, note) = &ready[0];
        assert_eq!((repo.as_str(), *number, note.as_str()), ("api", 7, "n1"));
        assert_eq!(d, &Draft { id: "note-n1".into(), path: "src/a.rs".into(), side: "RIGHT".into(), line: 14, start_line: Some(12), body: "fix n1".into() });
        assert_eq!(ready[1].2.start_line, None, "one line has no start");
        assert_eq!(problems, ["api/src/a.rs:3: outside the PR's changed lines"]);
    }
}
