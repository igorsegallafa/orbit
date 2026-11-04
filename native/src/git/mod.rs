//! Git flows over a workspace (or a repository's clone, scope `@repo`):
//! commit (`CommitModal.tsx`), push (`PushModal.tsx`), rebase
//! (`RebaseModal.tsx`), squash and merge (`MergeModal.tsx`), and the branch
//! drift banner (`BranchDriftBanner.tsx`). Shown as modals inside a page tab
//! (`crate::workspace_ui`), which they reload once something changed.
pub mod commit;
pub mod drift;
pub mod merge;
pub mod push;
pub mod rebase;

use crate::review::ui::{self, Tone};
use crate::theme::{self, c};
use crate::workspace_ui::PageTab;
use gpui::{div, prelude::*, px, AnyElement, App, ClickEvent, Context, Div, Rgba, SharedString, Stateful};

pub enum Flow {
    Commit(commit::CommitFlow),
    Push(push::PushFlow),
    Rebase(rebase::RebaseFlow),
    Merge(merge::MergeFlow),
}

impl Flow {
    /// Something is running: the modal can't be closed.
    pub fn busy(&self) -> bool {
        match self {
            Flow::Commit(f) => f.busy(),
            Flow::Push(f) => f.running,
            Flow::Rebase(f) => f.busy(),
            Flow::Merge(f) => f.running,
        }
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        match self {
            Flow::Commit(f) => f.dump(out, cx),
            Flow::Push(f) => f.dump(out),
            Flow::Rebase(f) => f.dump(out),
            Flow::Merge(f) => f.dump(out),
        }
    }
}

impl PageTab {
    pub fn render_flow(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.modal {
            Some(crate::workspace_ui::Modal::Flow(f)) => match f {
                Flow::Commit(f) => self.render_commit(f, cx).into_any_element(),
                Flow::Push(f) => self.render_push(f, cx).into_any_element(),
                Flow::Rebase(f) => self.render_rebase(f, cx).into_any_element(),
                Flow::Merge(f) => self.render_merge(f, cx).into_any_element(),
            },
            _ => div().into_any_element(),
        }
    }

    /// Closes the flow unless it is running.
    pub fn cancel_flow(&mut self, cx: &mut Context<Self>) {
        if self.flow_mut().is_some_and(|f| f.busy()) {
            return;
        }
        self.close_modal(cx);
    }
}

/// The dimmed backdrop with the modal card centered on it.
pub fn scrim(card: AnyElement) -> Div {
    // Occluded: clicks never reach the page under it.
    div().occlude().absolute().top_0().left_0().size_full().bg(theme::scrim()).flex().items_start().justify_center().pt(px(48.)).child(card)
}

/// A modal card: title, hint, body and footer buttons.
pub fn card(width: f32, title: impl Into<SharedString>, hint: Option<SharedString>) -> Stateful<Div> {
    div()
        .id("modal-card")
        .w(px(width))
        .max_h(px(620.))
        .overflow_y_scroll()
        .p_4()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(theme::radius())
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL))
        .shadow(theme::shadow(8., 24., 0.4))
        .text_color(c(theme::TEXT))
        .child(div().text_base().font_weight(gpui::FontWeight::SEMIBOLD).child(title.into()))
        .when_some(hint, |d, h| d.child(div().text_xs().text_color(c(theme::TEXT_DIM)).child(h)))
}

pub fn footer() -> Div {
    div().pt_2().flex().items_center().justify_end().gap_2()
}

/// The modal's Cancel / Close button.
pub fn cancel_button(label: &'static str, cx: &mut Context<PageTab>) -> impl IntoElement {
    ui::button("modal-cancel", label, Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
        if matches!(t.modal, Some(crate::workspace_ui::Modal::Flow(_))) {
            t.cancel_flow(cx)
        } else {
            t.close_modal(cx)
        }
    }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Pending,
    Working,
    Ok,
    Warn,
    Error,
}

impl Mark {
    fn glyph(self) -> (&'static str, Rgba) {
        match self {
            Mark::Pending => ("○", c(theme::TEXT_FAINT)),
            Mark::Working => ("◌", c(theme::ACCENT)),
            Mark::Ok => ("✓", c(theme::OK)),
            Mark::Warn => ("!", c(theme::WARN)),
            Mark::Error => ("✕", c(theme::DANGER)),
        }
    }
}

/// One repo's line in a flow: mark, name, detail, then actions.
pub fn status_row(mark: Mark, repo: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    let (g, color) = mark.glyph();
    div()
        .flex()
        .items_center()
        .gap_2()
        .py(px(3.))
        .text_sm()
        .child(div().w(px(14.)).flex_none().text_color(color).child(g))
        .child(div().flex_none().font_family(theme::MONO_FONT).text_xs().child(repo.into()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(if mark == Mark::Error { c(theme::DANGER) } else { c(theme::TEXT_DIM) })
                .child(detail.into()),
        )
}
