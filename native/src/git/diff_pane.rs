//! A diff pane for modals (`ReviewPane.tsx` inside `CommitModal.tsx`): one
//! file's changes in gpui-component's diff view, with the review tab's file
//! bar (‹ n/m › and the split / unified toggle).
use crate::diff_view::{commit_patch, git_patch, highlighted_state};
use crate::lsp::file_language;
use crate::review::ui;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, Context, Entity, SharedString, Window};
use gpui_component::diff::{Diff, DiffFile, DiffMode, DiffState};
use std::path::{Path, PathBuf};

/// What a pane shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The working tree against HEAD.
    Working { dir: PathBuf, rel: String },
    /// A commit against its parent, or a `base...head` range from the fork point.
    Rev { dir: PathBuf, rev: String, rel: String },
}

impl Source {
    fn patch(&self) -> Result<String, String> {
        match self {
            Source::Working { dir, rel } => git_patch(dir, rel).map_err(|e| format!("{e:#}")),
            Source::Rev { dir, rev, rel } if rev.contains("...") => range_patch(dir, rev, rel),
            Source::Rev { dir, rev, rel } => commit_patch(dir, rev, rel).map_err(|e| format!("{e:#}")),
        }
    }

    fn rel(&self) -> &str {
        match self {
            Source::Working { rel, .. } | Source::Rev { rel, .. } => rel,
        }
    }
}

fn range_patch(dir: &Path, range: &str, rel: &str) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["diff", "--no-color", "--no-ext-diff", "--unified=1000000", range, "--", rel])
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub struct DiffPane {
    label: String,
    source: Option<Source>,
    state: Entity<DiffState>,
    sel: crate::diff_select::Handle,
    changes: usize,
    change_ix: Option<usize>,
    error: Option<String>,
    /// Shown with no file.
    empty: SharedString,
}

impl DiffPane {
    pub fn new(empty: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        // The whole file, as Monaco's diff editor shows it.
        let state = highlighted_state(vec![], DiffMode::Unified, cx);
        state.update(cx, |s, cx| s.set_context_lines(None, cx));
        Self { label: String::new(), source: None, state, sel: crate::diff_select::new(), changes: 0, change_ix: None, error: None, empty: empty.into() }
    }

    /// The selection and last copy, for the self-test dump.
    pub fn selection_dump(&self, cx: &App) -> String {
        crate::diff_select::dump(&self.sel, &self.state, cx)
    }

    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    /// Shows `source` under `label` (the file bar's text).
    pub fn show(&mut self, label: String, source: Source, cx: &mut Context<Self>) {
        if self.source.as_ref() == Some(&source) {
            return;
        }
        self.label = label;
        self.source = Some(source.clone());
        self.error = None;
        cx.spawn(async move |this, cx| {
            let s = source.clone();
            let patch = cx.background_spawn(async move { s.patch() }).await;
            let _ = this.update(cx, |p, cx| {
                if p.source.as_ref() != Some(&source) {
                    return;
                }
                match patch {
                    Ok(patch) => {
                        let language = file_language(Path::new(source.rel())).highlight;
                        let files: Vec<DiffFile> = DiffFile::parse(&patch).unwrap_or_default().into_iter().map(|f| f.with_language(language)).collect();
                        p.changes = ui::change_blocks(&patch);
                        p.change_ix = None;
                        p.state.update(cx, |s, cx| s.set_files(files, cx));
                    }
                    Err(e) => p.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.source = None;
        self.label.clear();
        self.state.update(cx, |s, cx| s.set_files(vec![], cx));
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.changes == 0 {
            return;
        }
        self.change_ix = Some(match (self.change_ix, forward) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(self.changes - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
        self.state.update(cx, |s, cx| if forward { s.next_change(cx) } else { s.previous_change(cx) });
        cx.notify();
    }
}

impl Render for DiffPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full().min_h_0().flex().flex_col().bg(c(theme::BG));
        if self.source.is_none() {
            return root.items_center().justify_center().text_size(px(12.5)).text_color(c(theme::TEXT_FAINT)).child(self.empty.clone());
        }
        let split = self.state.read(cx).mode() == DiffMode::Split;
        let weak = cx.entity().downgrade();
        let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
        let bar = ui::diff_filebar(ui::DiffBar {
            label: self.label.clone(),
            nav: Some((self.change_ix, self.changes)),
            split,
            prev: Box::new(move |_, cx| {
                let _ = w1.update(cx, |p, cx| p.step(false, cx));
            }),
            next: Box::new(move |_, cx| {
                let _ = w2.update(cx, |p, cx| p.step(true, cx));
            }),
            set_split: Box::new(move |on, _, cx| {
                let _ = w3.update(cx, |p, cx| {
                    p.state.update(cx, |s, cx| s.set_mode(if on { DiffMode::Split } else { DiffMode::Unified }, cx));
                    cx.notify();
                });
            }),
            extra: None,
        });
        let body = match &self.error {
            Some(e) => div().p_4().text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone()).into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(crate::diff_select::attach(Diff::new(&self.state).size_full().header_visible(false), &self.sel, &self.state, &[], None, cx))
                .child(crate::probe::probe("pane-diff"))
                .into_any_element(),
        };
        root.child(bar).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A range diffs from the fork point: main's later commits stay out.
    #[test]
    fn range_patch_starts_at_the_fork_point() {
        let dir = std::env::temp_dir().join(format!("orbit-diff-pane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "init"]);
        git(&dir, &["checkout", "-qb", "feat"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&dir, &["commit", "-qam", "two"]);
        git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("b.txt"), "main only\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "b"]);
        git(&dir, &["checkout", "-q", "feat"]);

        let a = Source::Rev { dir: dir.clone(), rev: "main...HEAD".into(), rel: "a.txt".into() }.patch().unwrap();
        assert!(a.contains("+two"), "{a}");
        let b = Source::Rev { dir: dir.clone(), rev: "main...HEAD".into(), rel: "b.txt".into() }.patch().unwrap();
        assert!(b.trim().is_empty(), "main's own commit is not part of the range: {b}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
