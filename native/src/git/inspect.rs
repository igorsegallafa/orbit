//! A commit, or a `base...head` range, of a repository's clone: its files on
//! the left, the selected file's diff on the right (`CommitReviewPane.tsx`,
//! which the repo page's History and Compare open).
use super::diff_pane::{DiffPane, Source};
use super::{cancel_button, Flow};
use crate::review::{ui, Load};
use crate::theme::{self, c};
use crate::workspace_ui::{kit, plural, spawn, Modal, PageTab};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, FontWeight, Window};
use orbit_core::git::ChangeEntry;
use std::fmt::Write as _;
use std::path::PathBuf;

pub struct InspectFlow {
    pub dir: PathBuf,
    /// A sha, or `base...head`.
    pub rev: String,
    pub title: String,
    /// `sha · author · when`, or the range.
    pub meta: String,
    pub files: Load<Vec<ChangeEntry>>,
    pub selected: Option<String>,
    pub pane: Entity<DiffPane>,
}

impl InspectFlow {
    pub fn dump(&self, out: &mut String, cx: &App) {
        let _ = writeln!(out, "  inspect {} ({}): {}", self.rev, self.title, crate::workspace_ui::load_text(&self.files));
        for f in self.files.ready().into_iter().flatten() {
            let _ = writeln!(out, "    {} {} +{} -{}", ui::status_letter(&f.status).0, f.path, f.added, f.deleted);
        }
        let _ = writeln!(out, "    selected {:?} diff {:?}", self.selected, self.pane.read(cx).source());
        out.push_str(&self.pane.read(cx).selection_dump(cx));
    }
}

impl PageTab {
    fn inspect_flow(&mut self) -> Option<&mut InspectFlow> {
        match self.flow_mut() {
            Some(Flow::Inspect(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_inspect(&mut self, dir: PathBuf, rev: String, title: String, meta: String, window: &mut Window, cx: &mut Context<Self>) {
        let pane = cx.new(|cx| DiffPane::new("Pick a file to see its diff", cx));
        self.modal = Some(Modal::Flow(Flow::Inspect(InspectFlow { dir: dir.clone(), rev: rev.clone(), title, meta, files: Load::Loading, selected: None, pane })));
        spawn(window, cx, move || orbit_core::git::commit_files(&dir, &rev), |t, res, _, cx| {
            let Some(f) = t.inspect_flow() else { return };
            let first = res.as_ref().ok().and_then(|l| l.first()).map(|x| x.path.clone());
            f.files = res.map_or_else(Load::Failed, Load::Ready);
            if let Some(p) = first {
                t.inspect_select(p, cx);
            }
        });
        cx.notify();
    }

    fn inspect_select(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(f) = self.inspect_flow() else { return };
        f.selected = Some(path.clone());
        let source = Source::Rev { dir: f.dir.clone(), rev: f.rev.clone(), rel: path.clone() };
        f.pane.update(cx, |p, cx| p.show(path, source, cx));
        cx.notify();
    }

    pub(super) fn render_inspect(&self, f: &InspectFlow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("inspect-files").w(px(300.)).flex_none().h_full().overflow_y_scroll().p(px(8.)).flex().flex_col().gap(px(1.)).border_r_1().border_color(c(theme::BORDER));
        match &f.files {
            Load::Ready(files) if files.is_empty() => list = list.child(div().p(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("No file changes")),
            Load::Ready(files) => {
                for file in files {
                    let on = f.selected.as_deref() == Some(file.path.as_str());
                    let (name, dir) = match file.path.rsplit_once('/') {
                        Some((d, n)) => (n.to_string(), d.to_string()),
                        None => (file.path.clone(), String::new()),
                    };
                    let path = file.path.clone();
                    list = list.child(
                        div()
                            .id(gpui::ElementId::Name(format!("inspect-file:{}", file.path).into()))
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .map(|d| if on { d.bg(theme::accent_soft()) } else { d.hover(|s| s.bg(c(theme::PANEL_HOVER))) })
                            .child(ui::status_badge(&file.status))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_baseline()
                                    .overflow_hidden()
                                    .child(div().flex_none().text_color(c(theme::TEXT)).child(name))
                                    .when(!dir.is_empty(), |d| d.child(div().ml(px(6.)).min_w_0().truncate().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(dir))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(px(5.))
                                    .text_size(px(11.))
                                    .when(file.added > 0, |d| d.child(div().text_color(c(theme::OK)).child(format!("+{}", file.added))))
                                    .when(file.deleted > 0, |d| d.child(div().text_color(c(theme::DANGER)).child(format!("−{}", file.deleted)))),
                            )
                            .child(crate::probe::probe(format!("inspect-file:{}", file.path)))
                            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.inspect_select(path.clone(), cx))),
                    );
                }
            }
            Load::Failed(e) => list = list.child(div().p(px(6.)).text_size(px(12.)).text_color(c(theme::DANGER)).child(e.clone())),
            _ => list = list.child(div().p(px(6.)).text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child("Loading…")),
        }
        let count = f.files.ready().map(|l| plural(l.len(), "file"));
        let head = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(18.))
            .pt(px(16.))
            .pb(px(12.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().truncate().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(f.title.clone()))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .child(div().font_family(theme::MONO_FONT).child(f.meta.clone()))
                    .children(count.map(|n| div().text_color(c(theme::TEXT_FAINT)).child(format!("· {n}")))),
            );
        kit::modal("modal-card", 1180.)
            .max_w(gpui::relative(0.94))
            .h(px(780.))
            .max_h(gpui::relative(0.94))
            .text_size(px(13.))
            .child(head)
            .child(div().flex_1().min_h_0().flex().child(list).child(div().flex_1().min_w_0().h_full().flex().bg(c(theme::BG)).child(f.pane.clone())))
            .child(kit::modal_footer().px(px(18.)).child(cancel_button("Close", cx)))
    }
}
