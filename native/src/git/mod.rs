//! Git flows over a workspace (or a repository's clone, scope `@repo`):
//! commit (`CommitModal.tsx`), push (`PushModal.tsx`), rebase
//! (`RebaseModal.tsx`), squash and merge (`MergeModal.tsx`), and the branch
//! drift banner (`BranchDriftBanner.tsx`). Shown as modals inside a page tab
//! (`crate::workspace_ui`), which they reload once something changed.
pub mod commit;
pub mod diff_pane;
pub mod drift;
pub mod inspect;
pub mod merge;
pub mod prs;
pub mod push;
pub mod rebase;

use crate::review::ui::Tone;
use crate::workspace_ui::kit::{self, Kind};
use crate::theme::{self, c};
use crate::workspace_ui::PageTab;
use gpui::{actions, div, prelude::*, px, AnyElement, App, ClickEvent, Context, Div, KeyBinding, Rgba, SharedString, Stateful};

actions!(orbit_git, [SubmitCommit]);

/// Key context of a commit box: Ctrl+Enter in its inputs commits.
pub const COMMIT_CONTEXT: &str = "OrbitCommit";

pub fn init(cx: &mut App) {
    // Bound on the inputs themselves, ahead of their own Ctrl+Enter (a newline).
    let key = if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" };
    cx.bind_keys([KeyBinding::new(key, SubmitCommit, Some("OrbitCommit > Input"))]);
}

pub enum Flow {
    Commit(commit::CommitFlow),
    Push(push::PushFlow),
    Rebase(rebase::RebaseFlow),
    Merge(merge::MergeFlow),
    Prs(prs::PrsFlow),
    Inspect(inspect::InspectFlow),
}

impl Flow {
    /// Something is running: the modal can't be closed.
    pub fn busy(&self) -> bool {
        match self {
            Flow::Commit(f) => f.busy(),
            Flow::Push(f) => f.running,
            Flow::Rebase(f) => f.busy(),
            Flow::Merge(f) => f.running,
            Flow::Prs(f) => f.creating,
            Flow::Inspect(_) => false,
        }
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        match self {
            Flow::Commit(f) => f.dump(out, cx),
            Flow::Push(f) => f.dump(out),
            Flow::Rebase(f) => f.dump(out),
            Flow::Merge(f) => f.dump(out),
            Flow::Prs(f) => f.dump(out, cx),
            Flow::Inspect(f) => f.dump(out, cx),
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
                Flow::Prs(f) => self.render_prs(f, cx).into_any_element(),
                Flow::Inspect(f) => self.render_inspect(f, cx).into_any_element(),
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
pub fn scrim(card: AnyElement) -> AnyElement {
    use crate::motion;
    // Occluded: clicks never reach the page under it.
    let scrim = div().occlude().absolute().top_0().left_0().size_full().bg(theme::scrim()).flex().items_center().justify_center();
    motion::overlay_in(scrim.child(motion::modal_in(div().flex().max_h_full().child(card), "modal-in")), "modal-overlay-in")
}

/// A modal card (`.modal`): title, hint, body and footer buttons.
pub fn card(width: f32, title: impl Into<SharedString>, hint: Option<SharedString>) -> Stateful<Div> {
    div()
        .id("modal-card")
        .occlude()
        .w(px(width))
        .max_h(px(760.))
        .overflow_y_scroll()
        .px(px(16.))
        .pt(px(16.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .rounded(px(12.))
        .border_1()
        .border_color(c(theme::BORDER_STRONG))
        .bg(c(theme::PANEL))
        .shadow(theme::shadow(20., 60., 0.55))
        .text_color(c(theme::TEXT))
        .text_size(px(13.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.into()))
                .when_some(hint, |d, h| d.child(div().text_size(px(12.5)).line_height(px(18.)).text_color(c(theme::TEXT_DIM)).child(h))),
        )
}

/// `.modal-footer`: right-aligned buttons over a top border, flush with the card.
pub fn footer() -> Div {
    div().mt(px(6.)).mx(px(-16.)).px(px(16.)).py(px(12.)).border_t_1().border_color(c(theme::BORDER)).flex().items_center().justify_end().gap(px(8.))
}

/// A flow's button: footer actions (`*-submit`) at control height, the rest mini.
pub fn btn(id: impl Into<SharedString>, label: impl Into<SharedString>, tone: Tone) -> Stateful<Div> {
    let id: SharedString = id.into();
    let size = if id.ends_with("-submit") || id.as_ref() == "push-create-prs" { kit::NORMAL } else { kit::MINI };
    match tone {
        Tone::Primary => kit::button(id, label, Kind::Primary, size),
        Tone::Plain => kit::button(id, label, Kind::Secondary, size),
        Tone::Danger => kit::button(id, label, Kind::Secondary, size).text_color(c(theme::DANGER)),
    }
}

/// The modal's Cancel / Close button.
pub fn cancel_button(label: &'static str, cx: &mut Context<PageTab>) -> impl IntoElement {
    kit::button("modal-cancel", label, Kind::Secondary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
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
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(8.))
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::PANEL_RAISED))
        .text_size(px(13.))
        .child(
            div()
                .flex_none()
                .size(px(20.))
                .rounded_full()
                .border_1()
                .when(mark == Mark::Pending, |d| d.border_dashed())
                .border_color(if mark == Mark::Pending { c(theme::TEXT_FAINT) } else { color })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(color)
                .child(if mark == Mark::Pending { "" } else { g }),
        )
        .child(div().flex_none().font_family(theme::MONO_FONT).font_weight(gpui::FontWeight::BOLD).child(repo.into()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.5))
                .text_color(if mark == Mark::Error { c(theme::DANGER) } else { c(theme::TEXT_DIM) })
                .child(detail.into()),
        )
}
