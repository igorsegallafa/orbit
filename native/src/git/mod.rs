//! Git flows over a workspace (or a repository's clone, scope `@repo`):
//! commit (`CommitModal.tsx`), push (`PushModal.tsx`), rebase
//! (`RebaseModal.tsx`), squash and merge (`MergeModal.tsx`), and the branch
//! drift banner (`BranchDriftBanner.tsx`). Shown as modals of a page tab
//! (`crate::workspace_ui`) over the whole window, which they reload once something changed.
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
use gpui::{actions, anchored, deferred, div, point, prelude::*, px, AnyElement, App, ClickEvent, Context, Div, Rgba, SharedString, Stateful, Window};

actions!(orbit_git, [SubmitCommit]);

/// Key context of a commit box: Ctrl+Enter in its inputs commits.
pub const COMMIT_CONTEXT: &str = "OrbitCommit";

pub fn init(cx: &mut App) {
    // Bound on the inputs themselves, ahead of their own Ctrl+Enter (a newline).
    cx.bind_keys([crate::commands::binding("ctrl-enter", SubmitCommit, Some("OrbitCommit > Input"))]);
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

/// Paint order of a page's modal: over the page's own deferred popovers, under
/// the shell's overlays and gpui-component's popups (`POPUP_PRIORITY`).
pub const MODAL_PRIORITY: usize = 10;
/// Paint order of the shell's overlays (search, menus, dialogs, toasts).
pub const SHELL_OVERLAY_PRIORITY: usize = 50;

/// The dimmed backdrop over the whole window (`.modal-overlay` is `position:
/// fixed`) with the modal card centered on it. The page tab renders it, so it
/// keeps the tab's focus and actions, and hides with the tab.
/// `on_outside`: a click on the scrim, outside the card; callers close the modal as Esc does.
pub fn scrim(card: AnyElement, window: &Window, on_outside: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    use crate::motion;
    // Occluded: clicks never reach the window under it, nor the scrim through the card.
    let vp = window.viewport_size();
    let scrim = div().id("modal-scrim").occlude().w(vp.width).h(vp.height).bg(theme::scrim()).flex().items_center().justify_center().on_mouse_down(gpui::MouseButton::Left, on_outside);
    let scrim = motion::overlay_in(scrim.child(motion::modal_in(div().occlude().flex().max_h_full().child(card), "modal-in")), "modal-overlay-in");
    deferred(anchored().position(point(px(0.), px(0.))).child(scrim)).with_priority(MODAL_PRIORITY).into_any_element()
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
    btn_busy(id, label, tone, false)
}

/// `btn` with a spinner before the label while `busy`.
pub fn btn_busy(id: impl Into<SharedString>, label: impl Into<SharedString>, tone: Tone, busy: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    let size = if id.ends_with("-submit") || id.as_ref() == "push-create-prs" { kit::NORMAL } else { kit::MINI };
    match tone {
        Tone::Primary => kit::button_busy(id, label, Kind::Primary, size, busy),
        Tone::Plain => kit::button_busy(id, label, Kind::Secondary, size, busy),
        Tone::Danger => kit::button_busy(id, label, Kind::Secondary, size, busy).text_color(c(theme::DANGER)),
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
            Mark::Pending => ("", c(theme::TEXT_FAINT)),
            Mark::Working => ("", c(theme::ACCENT)),
            Mark::Ok => ("✓", c(theme::OK)),
            Mark::Warn => ("!", c(theme::WARN)),
            Mark::Error => ("✕", c(theme::DANGER)),
        }
    }
}

/// One repo's line in a flow: mark, name, detail, then actions.
pub fn status_row(mark: Mark, repo: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    let (g, color) = mark.glyph();
    let repo: SharedString = repo.into();
    let glyph = match mark {
        Mark::Working => kit::spinner(gpui::ElementId::Name(format!("flow-spin:{repo}").into()), 20., color).into_any_element(),
        _ => g.into_any_element(),
    };
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
                .when(mark != Mark::Working, |d| d.border_1())
                .when(mark == Mark::Pending, |d| d.border_dashed())
                .border_color(if mark == Mark::Pending { c(theme::TEXT_FAINT) } else { color })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(color)
                .child(glyph),
        )
        .child(div().flex_none().font_family(theme::MONO_FONT).font_weight(gpui::FontWeight::BOLD).child(repo))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.5))
                .text_color(if mark == Mark::Error { c(theme::DANGER) } else { c(theme::TEXT_DIM) })
                .child(detail.into()),
        )
}
