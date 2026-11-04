//! Rendering of the window: titlebar, workspaces sidebar, tab strip, main
//! area with the bottom panel, files dock, status bar, resizers and the
//! overlays (menus, dialogs, palette, toasts). Layout and look follow
//! `src/App.tsx` and `src/App.css`.
use crate::app::{OrbitApp, Pane, TabKind};
use crate::commands::{self, RunCommand};
use crate::icons;
use crate::probe::probe;
use crate::state::SIDEBAR;
use crate::theme::{self, c, size};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, CursorStyle, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, NavigationDirection, Window,
};
use gpui_component::{Icon, IconName, Sizable, TitleBar};
use std::path::Path;

/// A titlebar button (`.titlebar-btn`). Acts on mouse down, which also keeps
/// the press from starting a window move.
fn titlebar_btn(
    id: &'static str,
    icon: IconName,
    enabled: bool,
    cx: &mut Context<OrbitApp>,
    on: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .w(px(size::TITLEBAR_BTN_W))
        .h(px(size::TITLEBAR_BTN_H))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(c(theme::TEXT_FAINT))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT))))
        .when(!enabled, |d| d.opacity(0.3))
        .child(Icon::new(icon).small())
        .child(probe(id))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if enabled {
                    on(this, window, cx);
                }
            }),
        )
}

/// A drag handle on a pane's edge (`.sidebar-resizer`, `.pane-resizer`).
fn resizer(pane: Pane, lit: bool, cx: &mut Context<OrbitApp>) -> impl IntoElement {
    let vertical = pane != Pane::Panel;
    let id = match pane {
        Pane::Sidebar => "resize-sidebar",
        Pane::Dock => "resize-dock",
        Pane::Panel => "resize-panel",
    };
    div()
        .id(id)
        .relative()
        .flex_none()
        .when(vertical, |d| d.w(px(size::RESIZER)).h_full().cursor(CursorStyle::ResizeLeftRight))
        .when(!vertical, |d| d.h(px(size::RESIZER)).w_full().cursor(CursorStyle::ResizeUpDown))
        .when(lit, |d| d.bg(c(theme::ACCENT)))
        .hover(|s| s.bg(c(theme::ACCENT)))
        .child(probe(id))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if ev.click_count >= 2 {
                    this.reset_pane(pane, cx);
                } else {
                    this.start_drag(pane, ev.position, cx);
                }
            }),
        )
}

fn section_header(label: &'static str) -> impl IntoElement {
    div()
        .px_3()
        .pt_3()
        .pb_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(label)
}

impl OrbitApp {
    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws_name = self.workspace().map(|w| w.meta.name.clone());
        TitleBar::new()
            .h(px(size::TITLEBAR))
            .pl(px(10.))
            .bg(c(theme::BG))
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .child(
                        div()
                            .pr(px(10.))
                            .text_size(px(13.))
                            .font_weight(FontWeight(650.))
                            .text_color(c(theme::TEXT_FAINT))
                            .child("Orbit"),
                    )
                    .child(titlebar_btn(
                        "tb-sidebar",
                        if self.layout.sidebar_hidden { IconName::PanelLeftOpen } else { IconName::PanelLeft },
                        true,
                        cx,
                        |a, _, cx| a.toggle_sidebar(cx),
                    ))
                    .child(titlebar_btn("tb-back", IconName::ArrowLeft, self.nav.can_go_back(), cx, |a, w, cx| a.navigate(true, w, cx)))
                    .child(titlebar_btn("tb-forward", IconName::ArrowRight, self.nav.can_go_forward(), cx, |a, w, cx| {
                        a.navigate(false, w, cx)
                    }))
                    .child(
                        // Workspace switcher.
                        div()
                            .id("tb-workspace")
                            .relative()
                            .ml_2()
                            .h(px(size::TITLEBAR_BTN_H))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded(px(6.))
                            .text_size(px(size::TAB_FONT))
                            .text_color(c(if ws_name.is_some() { theme::TEXT_DIM } else { theme::TEXT_FAINT }))
                            .cursor_pointer()
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                            .child(ws_name.unwrap_or_else(|| "No workspace".into()))
                            .child(Icon::new(IconName::ChevronDown).xsmall())
                            .child(probe("tb-workspace"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    let at = gpui::point(ev.position.x - px(8.), px(size::TITLEBAR + 2.));
                                    this.open_workspace_menu(at, window, cx);
                                }),
                            ),
                    )
                    .child(div().flex_1())
                    .children(self.titlebar_items.iter().cloned())
                    .child(titlebar_btn(
                        "tb-dock",
                        if self.layout.dock_hidden { IconName::PanelRightOpen } else { IconName::PanelRight },
                        self.selected.is_some(),
                        cx,
                        |a, _, cx| a.toggle_dock(cx),
                    )),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.layout.sidebar_collapsed();
        div()
            .id("workspaces")
            .w(px(self.layout.sidebar_width))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(c(theme::PANEL))
            .flex()
            .flex_col()
            .when(!collapsed, |d| d.child(section_header("WORKSPACES")))
            .when(collapsed, |d| d.pt_3().items_center().gap_1())
            .children(self.workspaces.iter().enumerate().map(|(ix, ws)| {
                let selected = self.selected == Some(ix);
                let row = div()
                    .id(("ws", ix))
                    .relative()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::accent_soft()))
                    .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                    .child(probe(format!("ws:{}", ws.meta.name)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_workspace(ix, cx)));
                if collapsed {
                    // Icon-only: the workspace's initial.
                    let initial = ws.meta.name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
                    row.size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(if selected { theme::ACCENT } else { theme::TEXT_DIM }))
                        .child(initial)
                } else {
                    row.mx_2()
                        .px_2()
                        .py_1()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().text_color(c(theme::TEXT)).child(ws.meta.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::TEXT_FAINT))
                                .child(format!("{} · {} repos", ws.meta.branch, ws.meta.repos.len())),
                        )
                }
            }))
            .when(!collapsed, |d| d.child(crate::review::sidebar(self, cx)))
            .when(!collapsed, |d| d.child(self.sessions.clone()))
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tab-strip")
            .h(px(size::TAB_STRIP))
            .flex_none()
            .flex()
            .items_end()
            .gap(px(2.))
            .pt(px(6.))
            .px_2()
            .overflow_x_scroll()
            .bg(c(theme::PANEL))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(self.tabs.iter().enumerate().flat_map(|(ix, tab)| {
                let active = self.active == Some(ix);
                let dirty = tab.dirty(cx);
                // A divider where the files start, after the sessions.
                let group_start = tab.is_file() && ix > 0 && !self.tabs[ix - 1].is_file();
                let icon = match tab.kind {
                    TabKind::Terminal(_) => "terminal",
                    TabKind::Editor(_) => "doc",
                    TabKind::Diff(_) | TabKind::Review(_) => "diff",
                };
                let divider = group_start.then(|| div().w(px(1.)).h(px(18.)).mb(px(9.)).mx(px(2.)).flex_none().bg(c(theme::BORDER_STRONG)));
                let el = div()
                    .id(("tab", ix))
                    .group("tab")
                    .relative()
                    .h(px(size::TAB))
                    .flex_none()
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(size::TAB_FONT))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(gpui::transparent_black())
                    .text_color(c(theme::TEXT_DIM))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                    .when(active, |d| d.bg(c(theme::BG)).text_color(c(theme::TEXT)).border_color(c(theme::ACCENT)))
                    .child(div().text_color(c(theme::TEXT_DIM)).child(icons::ui_icon(icon, 13.)))
                    .child(div().when(tab.preview, |d| d.italic()).child(tab.title(cx)))
                    .child(
                        div()
                            .id(("close", ix))
                            .h(px(20.))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .text_size(px(14.))
                            .text_color(c(theme::TEXT_FAINT))
                            .hover(|s| s.bg(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                            // Dirty: a dot that turns into × on hover.
                            // (Colours, not display: a hover may not change layout between prepaint and paint.)
                            .when(dirty, |d| {
                                d.relative()
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(6.))
                                            .left(px(4.))
                                            .size(px(8.))
                                            .rounded_full()
                                            .bg(c(theme::TEXT_DIM))
                                            .group_hover("tab", |s| s.bg(gpui::transparent_black())),
                                    )
                                    .child(div().text_color(gpui::transparent_black()).group_hover("tab", |s| s.text_color(c(theme::TEXT_FAINT))).child("×"))
                            })
                            .when(!dirty, |d| d.child("×"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(ix, false, window, cx)
                                }),
                            ),
                    )
                    .child(probe(format!("tab:{ix}")))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        // Double click keeps a preview tab open.
                        if ev.click_count() >= 2 {
                            this.keep_open(ix, cx);
                        }
                        this.activate(ix, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.open_tab_menu(ix, ev.position, window, cx)),
                    )
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab(ix, false, window, cx)));
                divider.map(IntoElement::into_any_element).into_iter().chain([el.into_any_element()])
            }))
            .child(
                div()
                    .id("new-tab")
                    .relative()
                    .size(px(28.))
                    .mb(px(4.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(c(theme::TEXT_FAINT))
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                    .child(Icon::new(IconName::Plus).small())
                    .child(probe("new-tab"))
                    // The command, so a later stage's "new session" takes over the button too.
                    .on_click(cx.listener(|this, _, window, cx| {
                        commands::run("app.newTerminal", this, window, cx);
                    })),
            )
    }

    fn render_content(&self) -> AnyElement {
        match self.active_tab().map(|t| &t.kind) {
            Some(TabKind::Terminal(t)) => t.clone().into_any_element(),
            Some(TabKind::Editor(e)) => e.clone().into_any_element(),
            Some(TabKind::Diff(d)) => d.clone().into_any_element(),
            Some(TabKind::Review(r)) => r.clone().into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::TEXT_FAINT))
                .child("Ctrl+Shift+T opens a terminal · click a file to open it · Ctrl+Shift+A finds an action")
                .into_any_element(),
        }
    }

    /// Placeholder for the Build/Run/Problems panel (`BottomPanel.tsx`), which arrives with that stage.
    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(self.layout.panel_height))
            .flex_none()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_size(px(size::TAB_FONT))
                    .text_color(c(theme::TEXT_DIM))
                    .child("Build")
                    .child("Run")
                    .child("Problems")
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("panel-close")
                            .relative()
                            .px_1()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_color(c(theme::TEXT_FAINT))
                            .hover(|s| s.bg(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                            .child("×")
                            .child(probe("panel-close"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c(theme::TEXT_FAINT))
                    .child("Build output, the run terminal and problems arrive with the Run/Build stage."),
            )
    }

    fn render_dock(&self) -> impl IntoElement {
        div()
            .w(px(self.layout.dock_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .child(section_header("FILES"))
            .child(div().flex_1().min_h_0().child(self.tree.clone()))
    }

    fn render_status(&self) -> impl IntoElement {
        let servers = self.lsp.running();
        div()
            .h(px(size::STATUS_BAR))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .justify_end()
            .gap_3()
            .text_xs()
            .bg(c(theme::PANEL))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_color(c(theme::TEXT_FAINT))
            .children(servers.iter().map(|s| {
                let name = s.command.split(' ').next().and_then(|b| Path::new(b).file_name()).map(|n| n.to_string_lossy().to_string());
                let repo = s.open_root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                format!("{} · {repo}", name.unwrap_or_default())
            }))
    }

    /// While a resizer is dragged: a layer over everything that keeps the cursor and the mouse.
    fn render_drag_layer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let drag = self.drag.as_ref().filter(|d| d.moved)?;
        Some(
            div()
                .id("drag-layer")
                .absolute()
                .inset_0()
                .occlude()
                .cursor(if drag.pane == Pane::Panel { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight })
                .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.drag_to(ev.position, window, cx);
                    } else {
                        this.end_drag(cx);
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx))),
        )
    }
}

impl Render for OrbitApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_save(cx);
        let show_dock = self.selected.is_some() && !self.layout.dock_hidden;
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_h_0().child(self.render_content()))
            .when(self.layout.panel_open, |d| {
                d.child(resizer(Pane::Panel, false, cx)).child(self.render_panel(cx))
            });
        let sidebar_lit = self.layout.sidebar_width <= SIDEBAR.min + 1.;
        div()
            .id("orbit-app")
            .key_context("OrbitApp")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .text_color(c(theme::TEXT))
            .font_family(theme::UI_FONT)
            .text_size(px(size::FONT))
            .on_action(cx.listener(|this, action: &RunCommand, window, cx| commands::on_run_command(this, action, window, cx)))
            .on_action(cx.listener(Self::on_overlay_up))
            .on_action(cx.listener(Self::on_overlay_down))
            .on_action(cx.listener(Self::on_overlay_confirm))
            .on_action(cx.listener(Self::on_overlay_cancel))
            // A resizer's drag, until the drag layer is up.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if this.drag.is_some() {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.drag_to(ev.position, window, cx);
                    } else {
                        this.end_drag(cx);
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            // Mouse side buttons, on release as the React app.
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, w, cx| this.navigate(true, w, cx)))
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _, w, cx| this.navigate(false, w, cx)))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!self.layout.sidebar_hidden, |d| d.child(self.render_sidebar(cx)).child(resizer(Pane::Sidebar, sidebar_lit, cx)))
                    .child(main)
                    .when(show_dock, |d| {
                        d.child(resizer(Pane::Dock, false, cx)).child(div().w(px(1.)).h_full().bg(c(theme::BORDER))).child(self.render_dock())
                    }),
            )
            .child(self.render_status())
            .children(self.render_drag_layer(cx))
            .children(self.render_toasts(cx))
            .children(self.render_palette(cx))
            .children(self.render_menu(window, cx))
            .children(self.render_dialog(cx))
    }
}
