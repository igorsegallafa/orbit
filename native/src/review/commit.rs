//! A commit's review tab (`CommitReviewPane.tsx` + `ReviewPane.tsx`): its
//! files on the left, the selected file's diff (parent vs commit) on the right.
use super::{ui, Load, ReviewTab};
use crate::diff_view::{commit_patch, highlighted_state};
use crate::lsp::file_language;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, FontWeight};
use gpui_component::diff::{Diff, DiffFile, DiffMode, DiffState};
use orbit_core::git::{ChangeEntry, CommitEntry};
use std::path::{Path, PathBuf};

pub struct CommitView {
    pub repo: String,
    pub repo_dir: PathBuf,
    pub commit: CommitEntry,
    pub files: Load<Vec<ChangeEntry>>,
    pub selected: Option<String>,
    diff: Entity<DiffState>,
    changes: usize,
    change_ix: Option<usize>,
    pub error: Option<String>,
}

impl CommitView {
    pub fn new(repo: String, repo_dir: PathBuf, commit: CommitEntry, cx: &mut App) -> Self {
        // The whole file, as Monaco's diff editor shows it.
        let diff = highlighted_state(vec![], DiffMode::Unified, cx);
        diff.update(cx, |s, cx| s.set_context_lines(None, cx));
        Self { repo, repo_dir, commit, files: Load::Loading, selected: None, diff, changes: 0, change_ix: None, error: None }
    }
}

impl ReviewTab {
    /// Reads the commit's files, then shows the first.
    pub(super) fn load_commit(&mut self, cx: &mut Context<Self>) {
        let Some(v) = &self.commit else { return };
        let (dir, sha) = (v.repo_dir.clone(), v.commit.sha.clone());
        cx.spawn(async move |this, cx| {
            let files = cx.background_spawn(async move { orbit_core::git::commit_files(&dir, &sha) }).await;
            let _ = this.update(cx, |t, cx| {
                let Some(v) = &mut t.commit else { return };
                let first = files.as_ref().ok().and_then(|f| f.first()).map(|f| f.path.clone());
                v.files = files.map_or_else(Load::Failed, Load::Ready);
                if let Some(p) = first {
                    t.select_commit_file(p, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_commit_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(v) = &mut self.commit else { return };
        v.selected = Some(path.clone());
        let (dir, sha) = (v.repo_dir.clone(), v.commit.sha.clone());
        cx.spawn(async move |this, cx| {
            let rel = path.clone();
            let patch = cx.background_spawn(async move { commit_patch(&dir, &sha, &rel) }).await;
            let _ = this.update(cx, |t, cx| {
                let Some(v) = &mut t.commit else { return };
                if v.selected.as_deref() != Some(path.as_str()) {
                    return;
                }
                match patch {
                    Ok(patch) => {
                        let language = file_language(Path::new(&path)).highlight;
                        let files: Vec<DiffFile> = DiffFile::parse(&patch).unwrap_or_default().into_iter().map(|f| f.with_language(language)).collect();
                        v.changes = ui::change_blocks(&patch);
                        v.change_ix = None;
                        v.error = None;
                        v.diff.update(cx, |s, cx| s.set_files(files, cx));
                    }
                    Err(e) => v.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(v) = &mut self.commit else { return };
        if v.changes == 0 {
            return;
        }
        v.change_ix = Some(match (v.change_ix, forward) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(v.changes - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
        v.diff.update(cx, |s, cx| if forward { s.next_change(cx) } else { s.previous_change(cx) });
        cx.notify();
    }

    pub(super) fn render_commit(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(v) = &self.commit else { return div() };
        let mut side = div()
            .id("commit-files")
            .w(px(230.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .py(px(8.))
            .px(px(6.))
            .child(
                div()
                    .pt(px(8.))
                    .px(px(4.))
                    .pb(px(6.))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(c(theme::TEXT_FAINT))
                    .child(v.commit.sha.to_uppercase()),
            )
            .child(
                div()
                    .px(px(6.))
                    .pb(px(10.))
                    .text_size(px(11.5))
                    .line_height(px(16.7))
                    .text_color(c(theme::TEXT_DIM))
                    .line_clamp(3)
                    .child(v.commit.message.clone()),
            );
        match &v.files {
            Load::Ready(files) => {
                for f in files {
                    side = side.child(self.commit_file_row(f, cx));
                }
            }
            Load::Failed(e) => side = side.child(div().px(px(6.)).text_size(px(11.5)).text_color(c(theme::DANGER)).child(e.clone())),
            // Two `.modal-row`s of bars, as `CommitReviewPane`.
            _ => side = side.child(crate::motion::skeleton_line(30., 0.7, 11.)).child(crate::motion::skeleton_line(30., 0.55, 11.)),
        }

        let split = v.diff.read(cx).mode() == DiffMode::Split;
        let weak = cx.entity().downgrade();
        let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
        let label = format!("{} · {}/{}", v.commit.sha, v.repo, v.selected.as_deref().unwrap_or("…"));
        let bar = ui::diff_filebar(ui::DiffBar {
            label,
            nav: (v.changes > 0).then_some((v.change_ix, v.changes)),
            split,
            prev: Box::new(move |_, cx| {
                let _ = w1.update(cx, |t, cx| t.step_change(false, cx));
            }),
            next: Box::new(move |_, cx| {
                let _ = w2.update(cx, |t, cx| t.step_change(true, cx));
            }),
            set_split: Box::new(move |on, _, cx| {
                let _ = w3.update(cx, |t, cx| {
                    if let Some(v) = &t.commit {
                        v.diff.update(cx, |s, cx| s.set_mode(if on { DiffMode::Split } else { DiffMode::Unified }, cx));
                    }
                    cx.notify();
                });
            }),
            extra: None,
        });
        let body = match (&v.error, &v.selected) {
            (Some(e), _) => div().p_4().text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone()).into_any_element(),
            (None, None) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::TEXT_FAINT))
                .child("Pick a file to see its diff in this commit")
                .into_any_element(),
            _ => div().flex_1().min_h_0().child(Diff::new(&v.diff).size_full().header_visible(false)).into_any_element(),
        };
        div().size_full().flex().child(side).child(div().flex_1().min_w_0().h_full().flex().flex_col().child(bar).child(body))
    }

    fn commit_file_row(&self, f: &ChangeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.commit.as_ref().and_then(|v| v.selected.as_deref()) == Some(f.path.as_str());
        let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
        let path = f.path.clone();
        ui::row(format!("commit-file:{}", f.path))
            .h(px(19.))
            .px(px(4.))
            .py_0()
            .rounded(px(3.))
            .flex()
            .items_center()
            .gap(px(7.))
            .when(active, |d| d.bg(theme::accent_soft()))
            .child(ui::status_badge(&f.status))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .gap(px(5.))
                    .text_size(px(10.))
                    .when(f.added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{}", f.added))))
                    .when(f.deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{}", f.deleted)))),
            )
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.select_commit_file(path.clone(), cx)))
    }
}
