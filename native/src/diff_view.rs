//! Diff tab: `git diff HEAD` of one file, shown with gpui-component's
//! virtualized patch view (split or unified) and tree-sitter colors.
use crate::lsp::file_language;
use crate::theme::{self, c};
use anyhow::{bail, Context as _, Result};
use gpui::{div, prelude::*, px, Context, Entity, MouseButton, SharedString, Window};
use gpui_component::diff::{Diff, DiffFile, DiffMode, DiffState};
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::Rope;
use gpui_component::ActiveTheme;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct DiffTab {
    pub path: PathBuf,
    pub title: SharedString,
    pub state: Entity<DiffState>,
    pub additions: usize,
    pub deletions: usize,
}

/// The patch of `path` against HEAD, from the repo that holds it.
pub fn git_diff(path: &Path) -> Result<String> {
    let dir = path.parent().context("file has no folder")?;
    let out = Command::new("git")
        .args(["diff", "--no-color", "--no-ext-diff", "HEAD", "--"])
        .arg(path.file_name().context("no file name")?)
        .current_dir(dir)
        .output()
        .context("git not available")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl DiffTab {
    /// The changed files of `path`'s diff against HEAD, ready for `new`.
    pub fn load(path: &Path) -> Result<Vec<DiffFile>> {
        let patch = git_diff(path)?;
        if patch.trim().is_empty() {
            bail!("no changes against HEAD");
        }
        let language = file_language(path).highlight;
        Ok(DiffFile::parse(&patch)
            .map_err(|e| anyhow::anyhow!("{}", e.message()))?
            .into_iter()
            .map(|f| f.with_language(language))
            .collect())
    }

    pub fn new(path: PathBuf, files: Vec<DiffFile>, split: bool, cx: &mut Context<Self>) -> Self {
        let (additions, deletions) = files.iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions()));
        let highlight_theme = cx.theme().highlight_theme.clone();
        let mode = if split { DiffMode::Split } else { DiffMode::Unified };
        let state = cx.new(|cx| {
            DiffState::new(files, cx).with_mode(mode).with_syntax_highlighter(move |language, text| {
                let mut highlighter = SyntaxHighlighter::new(language);
                highlighter.update(None, &Rope::from(text), None);
                highlighter.styles(&(0..text.len()), &*highlight_theme)
            })
        });
        let title = format!("{} (diff)", path.file_name().unwrap_or_default().to_string_lossy());
        Self { path, title: title.into(), state, additions, deletions }
    }

    pub fn mode(&self, cx: &gpui::App) -> DiffMode {
        self.state.read(cx).mode()
    }

    fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        let next = if self.mode(cx) == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
        self.state.update(cx, |s, cx| s.set_mode(next, cx));
        cx.notify();
    }
}

impl Render for DiffTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let split = self.mode(cx) == DiffMode::Split;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(c(theme::TEXT_DIM))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(div().flex_1().child(self.path.to_string_lossy().to_string()))
                    .child(div().text_color(c(theme::OK)).child(format!("+{}", self.additions)))
                    .child(div().text_color(c(theme::DANGER)).child(format!("−{}", self.deletions)))
                    .child(
                        div()
                            .id("diff-mode")
                            .cursor_pointer()
                            .px_2()
                            .rounded_sm()
                            .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                            .child(if split { "Unified" } else { "Split" })
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.toggle_mode(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(Diff::new(&self.state).size_full()))
    }
}
