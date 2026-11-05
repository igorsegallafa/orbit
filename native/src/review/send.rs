//! "Send to PR": open notes posted as one review per pull request
//! (`SendToPrModal.tsx`). GitHub only takes comments on lines of the PR's diff
//! at its pushed head, so each note is found again in the pushed file by its
//! code; notes on unpushed code or outside the diff stay behind, with why.
use super::{notes as rnotes, ReviewTab};
use crate::overlay::{ButtonStyle, Dialog, DialogButton};
use gpui::{Context, Window};
use orbit_core::notes::Note;
use orbit_core::review::{self, Commentable, DraftComment};
use orbit_core::workspace::PrRef;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub note: Note,
    pub pr: PrRef,
    pub owner_repo: String,
    pub head_sha: String,
    pub start: u32,
    pub end: u32,
    pub problem: Option<String>,
}

/// The hunk range on the head side holding `line`.
pub fn hunk_of(c: Option<&Commentable>, line: u32) -> Option<[u64; 2]> {
    c?.right.iter().copied().find(|[a, b]| (*a..=*b).contains(&(line as u64)))
}

/// Why a note anchored at `start..=end` can't go to the PR, if it can't.
pub fn problem(in_pr: bool, head: Option<&str>, at: rnotes::Anchor, c: Option<&Commentable>) -> Option<String> {
    if head.is_none() || !in_pr {
        return Some("file isn't part of the PR".into());
    }
    if at.lost {
        return Some("code isn't pushed yet".into());
    }
    match hunk_of(c, at.end) {
        Some([a, _]) if at.start as u64 >= a => None,
        _ => Some("outside the PR's changed lines".into()),
    }
}

/// Checks every open note of a PR'd repo against its PR (network: gh).
pub(super) fn plan(ws: &str, refs: &[PrRef], notes: &[Note]) -> Vec<Candidate> {
    let mut out = vec![];
    for pr in refs {
        let mine: Vec<&Note> = notes.iter().filter(|n| n.repo == pr.repo && !n.done).collect();
        if mine.is_empty() {
            continue;
        }
        let looked = orbit_core::prs::owner_repo(ws, &pr.repo).and_then(|or| {
            let detail = orbit_core::github::pr_detail(&or, pr.number)?;
            let data = review::review_data(&or, pr.number, true)?;
            Ok((or, detail, data))
        });
        let (owner_repo, detail, data) = match looked {
            Ok(v) => v,
            Err(e) => {
                out.extend(mine.into_iter().map(|n| Candidate {
                    note: n.clone(),
                    pr: pr.clone(),
                    owner_repo: String::new(),
                    head_sha: String::new(),
                    start: n.start,
                    end: n.end,
                    problem: Some(e.clone()),
                }));
                continue;
            }
        };
        let mut heads: std::collections::HashMap<String, Option<String>> = Default::default();
        for n in mine {
            let head = heads
                .entry(n.path.clone())
                .or_insert_with(|| orbit_core::github::pr_file_diff(&owner_repo, &detail.head_sha, &detail.base_sha, &n.path).ok().map(|d| d.modified))
                .clone();
            let at = rnotes::anchor(n, head.as_deref());
            let in_pr = detail.files.iter().any(|f| f.path == n.path);
            out.push(Candidate {
                note: n.clone(),
                pr: pr.clone(),
                owner_repo: owner_repo.clone(),
                head_sha: detail.head_sha.clone(),
                start: at.start,
                end: at.end,
                problem: problem(in_pr, head.as_deref(), at, data.commentable.get(&n.path)),
            });
        }
    }
    out
}

/// The review comment a note becomes, on the pushed head's lines.
pub fn comment(c: &Candidate) -> DraftComment {
    DraftComment {
        path: c.note.path.clone(),
        side: "RIGHT".into(),
        line: c.end as u64,
        start_line: (c.start < c.end).then_some(c.start as u64),
        body: c.note.body.clone(),
    }
}

/// Posts the ready candidates, one review per PR; returns the posted note ids.
fn post(cands: &[Candidate]) -> (Vec<String>, Vec<String>) {
    let (mut sent, mut errors) = (vec![], vec![]);
    let mut prs: Vec<&PrRef> = cands.iter().filter(|c| c.problem.is_none()).map(|c| &c.pr).collect();
    prs.dedup_by(|a, b| a.repo == b.repo && a.number == b.number);
    for pr in prs {
        let mine: Vec<&Candidate> = cands.iter().filter(|c| c.problem.is_none() && c.pr.repo == pr.repo && c.pr.number == pr.number).collect();
        let comments: Vec<DraftComment> = mine.iter().map(|c| comment(c)).collect();
        match review::submit_review(&mine[0].owner_repo, pr.number, &mine[0].head_sha, "COMMENT", "", &comments) {
            Ok(()) => sent.extend(mine.iter().map(|c| c.note.id.clone())),
            Err(e) => errors.push(format!("{} #{}: {e}", pr.repo, pr.number)),
        }
    }
    (sent, errors)
}

impl ReviewTab {
    pub(super) fn send_to_pr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        self.sending = true;
        let notes: Vec<Note> = self.open_notes().into_iter().cloned().collect();
        let (ws, refs) = (self.workspace.clone(), self.pr_refs.clone());
        cx.spawn_in(window, async move |this, cx| {
            let cands = cx.background_executor().spawn(async move { plan(&ws, &refs, &notes) }).await;
            let _ = this.update_in(cx, |t, window, cx| {
                t.sending = false;
                t.confirm_send(cands, window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm_send(&mut self, cands: Vec<Candidate>, window: &mut Window, cx: &mut Context<Self>) {
        let ready = cands.iter().filter(|c| c.problem.is_none()).count();
        let mut message = format!("{ready} of {} open notes can go to the pull request as review comments.", cands.len());
        for c in cands.iter().filter(|c| c.problem.is_some()) {
            message.push_str(&format!("\n• {}:{}: {}", c.note.path, c.note.start, c.problem.as_deref().unwrap_or_default()));
        }
        let me = cx.entity().downgrade();
        let cands = Rc::new(cands);
        let button = |label: &str, remove: bool| {
            let (me, cands) = (me.clone(), cands.clone());
            DialogButton {
                label: label.to_string().into(),
                style: if remove { ButtonStyle::Primary } else { ButtonStyle::Secondary },
                run: Rc::new(move |_, window, cx| {
                    let _ = me.update(cx, |t, cx| t.post(cands.as_ref().clone(), remove, window, cx));
                }),
            }
        };
        let mut buttons = vec![DialogButton { label: "Cancel".into(), style: ButtonStyle::Secondary, run: Rc::new(|_, _, _| {}) }];
        if ready > 0 {
            buttons.push(button("Post and keep notes", false));
            buttons.push(button("Post and remove notes", true));
        }
        let dialog = Dialog { title: "Send notes to the pull request".into(), message: message.into(), buttons, on_cancel: Rc::new(|_, _, _| {}) };
        self.with_app(window, cx, |a, w, cx| a.open_dialog(dialog, w, cx));
    }

    fn post(&mut self, cands: Vec<Candidate>, remove: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.sending = true;
        cx.spawn_in(window, async move |this, cx| {
            let (sent, errors) = cx.background_executor().spawn(async move { post(&cands) }).await;
            let _ = this.update_in(cx, |t, window, cx| {
                t.sending = false;
                if !sent.is_empty() {
                    let n = sent.len();
                    t.with_app(window, cx, |a, _, cx| {
                        a.toast(crate::toast::ToastKind::Success, format!("Posted {n} review comment{}", if n == 1 { "" } else { "s" }), None, cx)
                    });
                    if remove {
                        t.delete_notes(&sent, cx);
                    }
                }
                if !errors.is_empty() {
                    t.message = Some(errors.join("; "));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rnotes::Anchor;

    #[test]
    fn notes_go_only_on_pushed_lines_inside_the_diff() {
        let c = Commentable { right: vec![[10, 20]], left: vec![] };
        let at = |start, end, lost| Anchor { start, end, moved: false, lost };
        assert_eq!(problem(true, Some("x"), at(12, 14, false), Some(&c)), None);
        assert_eq!(problem(true, Some("x"), at(8, 12, false), Some(&c)).as_deref(), Some("outside the PR's changed lines"));
        assert_eq!(problem(true, Some("x"), at(30, 30, false), Some(&c)).as_deref(), Some("outside the PR's changed lines"));
        assert_eq!(problem(true, Some("x"), at(12, 12, true), Some(&c)).as_deref(), Some("code isn't pushed yet"));
        assert_eq!(problem(false, Some("x"), at(12, 12, false), Some(&c)).as_deref(), Some("file isn't part of the PR"));
        assert_eq!(problem(true, None, at(12, 12, false), Some(&c)).as_deref(), Some("file isn't part of the PR"));
    }
}
