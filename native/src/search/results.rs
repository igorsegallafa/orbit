//! Find in Files results kept in the bottom panel (Rider's Find tool window):
//! Ctrl+Enter (Cmd+Enter on macOS) in Find in Files moves its matches here,
//! grouped by file, where they stay while you work. A click opens the match.
use super::{display_line, hit_position};
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, Context, Entity, FontWeight, Global, HighlightStyle, StyledText, Window};
use orbit_core::search::SearchMatch;
use std::ops::Range;
use std::path::PathBuf;

/// One search moved to the panel.
pub struct Found {
    pub query: String,
    /// "12 matches in 4 files", as Find in Files said it.
    pub summary: String,
    /// The workspace folder the matches' `repo/path` are under.
    pub root: PathBuf,
    /// Matches with their hits on the line (byte ranges of `m.text`), in file order.
    pub matches: Vec<(SearchMatch, Vec<Range<usize>>)>,
}

#[derive(Default)]
pub struct FindResults {
    pub found: Option<Found>,
    /// Files folded by a click on their header (`repo/path`).
    collapsed: std::collections::HashSet<String>,
}

pub(crate) struct GlobalFind(pub Entity<FindResults>);

impl Global for GlobalFind {}

pub fn store(cx: &mut App) -> Entity<FindResults> {
    if let Some(g) = cx.try_global::<GlobalFind>() {
        return g.0.clone();
    }
    let e = cx.new(|_| FindResults::default());
    cx.set_global(GlobalFind(e.clone()));
    e
}

impl FindResults {
    pub fn set(&mut self, found: Found, cx: &mut Context<Self>) {
        self.found = Some(found);
        self.collapsed.clear();
        cx.notify();
    }

    pub fn has_results(&self) -> bool {
        self.found.is_some()
    }

    /// For the self-test dump.
    pub fn dump(&self) -> String {
        match &self.found {
            Some(f) => format!("--- find panel {:?}: {} ({} matches)\n", f.query, f.summary, f.matches.len()),
            None => "--- find panel: empty\n".into(),
        }
    }
}

fn open_match(root: &std::path::Path, m: &SearchMatch, hit: Option<&Range<usize>>, cx: &mut App) {
    let path = root.join(&m.repo).join(&m.path);
    let (pos, _) = match hit {
        Some(h) => hit_position(m, h),
        None => (lsp_types::Position::new(m.line.saturating_sub(1) as u32, m.col.saturating_sub(1) as u32), 0),
    };
    crate::run::open_location(path, pos.line + 1, pos.character + 1, cx);
}

/// A line of the list: a file header or one of its matches (index into `matches`).
enum Line {
    File { key: String, name: String, dir: String, count: usize, folded: bool },
    Match(usize),
}

const ROW_H: f32 = 22.;

impl FindResults {
    fn lines(&self) -> Vec<Line> {
        let Some(f) = &self.found else { return vec![] };
        let mut out = vec![];
        let mut ix = 0;
        while ix < f.matches.len() {
            let (repo, path) = (&f.matches[ix].0.repo, &f.matches[ix].0.path);
            let end = f.matches[ix..].iter().position(|(m, _)| &m.repo != repo || &m.path != path).map_or(f.matches.len(), |n| ix + n);
            let key = format!("{repo}/{path}");
            let folded = self.collapsed.contains(&key);
            let (dir, name) = match path.rsplit_once('/') {
                Some((d, n)) => (format!("{repo}/{d}"), n.to_string()),
                None => (repo.clone(), path.clone()),
            };
            out.push(Line::File { key, name, dir, count: end - ix, folded });
            if !folded {
                out.extend((ix..end).map(Line::Match));
            }
            ix = end;
        }
        out
    }

    fn render_line(&self, line: &Line, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(f) = &self.found else { return div().into_any_element() };
        match line {
            Line::File { key, name, dir, count, folded } => {
                let toggle_key = key.clone();
                div()
                    .id(gpui::SharedString::from(format!("find-file:{key}")))
                    .relative()
                    .h(px(ROW_H))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .child(crate::review::ui::lucide(if *folded { "chevron-right" } else { "chevron-down" }, 12., c(theme::TEXT_FAINT)))
                    .child(div().flex_none().text_color(c(theme::TEXT)).font_weight(FontWeight::MEDIUM).child(name.clone()))
                    .child(div().min_w_0().truncate().text_xs().text_color(c(theme::TEXT_FAINT)).child(dir.clone()))
                    .child(div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(count.to_string()))
                    .child(probe(format!("find-file:{key}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&toggle_key) {
                            this.collapsed.insert(toggle_key.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Line::Match(i) => {
                let (m, hits) = &f.matches[*i];
                let mark = HighlightStyle {
                    background_color: Some(theme::hsla(theme::WARN).opacity(0.28)),
                    color: Some(theme::hsla(theme::WARN)),
                    ..Default::default()
                };
                let (text, shown) = display_line(&m.text, hits, 40);
                let (root, m2, first) = (f.root.clone(), m.clone(), hits.first().cloned());
                div()
                    .id(("find-match", *i))
                    .relative()
                    .h(px(ROW_H))
                    .pl(px(36.))
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)))
                    .child(div().w(px(44.)).flex_none().text_color(c(theme::TEXT_FAINT)).child(m.line.to_string()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(StyledText::new(text).with_highlights(shown.into_iter().map(|h| (h, mark)))))
                    .child(probe(format!("find-match:{}/{}:{}", m.repo, m.path, m.line)))
                    .on_click(move |_, _, cx| open_match(&root, &m2, first.as_ref(), cx))
                    .into_any_element()
            }
        }
    }
}

impl Render for FindResults {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(f) = &self.found else {
            let keys = crate::commands::display_keys("ctrl-enter");
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::TEXT_FAINT))
                .child(format!("No results · in Find in Files, {keys} opens them here"))
                .into_any_element();
        };
        let (query, summary) = (f.query.clone(), f.summary.clone());
        let count = self.lines().len();
        let list = gpui::uniform_list(
            "panel-find-list",
            count,
            cx.processor(|this, range: Range<usize>, _window, cx| {
                let lines = this.lines();
                range.filter_map(|ix| lines.get(ix).map(|l| this.render_line(l, cx))).collect()
            }),
        )
        .size_full();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(26.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(div().text_color(c(theme::TEXT_FAINT)).child("Find"))
                    .child(div().font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child(format!("\"{query}\"")))
                    .child(div().text_color(c(theme::TEXT_FAINT)).child(summary)),
            )
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }
}
