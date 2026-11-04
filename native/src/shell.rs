//! The window's frame, as `src/App.tsx` lays it out: titlebar (Orbit, sidebar
//! toggle, back/forward, the run widget slot, inbox, dock toggle), the
//! sidebar (`sidebar.rs`), the tab strip with its "+" menu, the content and
//! bottom panel, the right dock (Files / Find / Git), the AI usage bar, and
//! the overlays. Look and metrics follow `src/App.css`.
use crate::app::{OrbitApp, Pane, Tab, TabKind};
use crate::commands::{self, RunCommand};
use crate::icons::orbit_icon;
use crate::overlay::MenuItem;
use crate::probe::probe;
use crate::scope::{self, Scope};
use crate::sessions::{self, InboxKind, AGENTS};
use crate::state::SIDEBAR;
use crate::theme::{self, c, size};
use crate::toast::ToastKind;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, CursorStyle, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, NavigationDirection, PathPromptOptions, Pixels, Point, SharedString, Window,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::TitleBar;
use orbit_core::git::{ChangeEntry, CommitEntry};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DockTab {
    #[default]
    Files,
    Git,
}

/// The dock's Git tab: the shown repo's changes and commits.
#[derive(Default)]
pub struct GitPanel {
    pub dir: Option<PathBuf>,
    pub changes: Vec<ChangeEntry>,
    pub commits: Vec<CommitEntry>,
    pub loading: bool,
}

/// The New Session menu (`NewSessionMenu.tsx`): where, then what.
pub struct NewSessionMenu {
    pub origin: Point<Pixels>,
    /// The scope in focus (workspace or repo): offered first and preselected.
    pub focus: Option<(String, String, &'static str)>,
    pub picked: Option<String>,
    /// A folder chosen with "Choose folder…".
    pub extra: Option<String>,
}

/// The titlebar's workspace select, open: its filter and highlighted row.
#[derive(Default)]
pub struct WsSwitcher {
    pub query: String,
    pub highlighted: usize,
}

/// A row of the workspace select.
#[derive(Clone, Debug, PartialEq)]
pub enum SwitchItem {
    Scope(Scope),
    NewWorkspace,
}

/// Frame state that isn't layout (layout is saved in `state::Layout`).
#[derive(Default)]
pub struct Chrome {
    pub dock_tab: DockTab,
    pub git: GitPanel,
    pub inbox_open: bool,
    pub new_session: Option<NewSessionMenu>,
    pub ws_switcher: Option<WsSwitcher>,
    /// The select's left edge, for the popover.
    pub ws_switcher_x: Rc<Cell<f32>>,
}

const HOME_SCOPE: &str = "dir:~";

/// A titlebar button (`.titlebar-btn`: 28×24, radius 6, faint; hover raises).
/// Acts on mouse down, which also keeps the press from starting a window move.
fn titlebar_btn(
    id: &'static str,
    icon: &'static str,
    icon_size: f32,
    enabled: bool,
    tip: impl Into<SharedString>,
    cx: &mut Context<OrbitApp>,
    on: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let tip: SharedString = tip.into();
    div()
        .id(id)
        .group(id)
        .relative()
        .w(px(size::TITLEBAR_BTN_W))
        .h(px(size::TITLEBAR_BTN_H))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_RAISED))))
        .when(!enabled, |d| d.opacity(0.3))
        .child(orbit_icon(icon, icon_size, c(theme::TEXT_FAINT)).when(enabled, |s| s.group_hover(id, |s| s.text_color(c(theme::TEXT)))))
        .child(probe(id))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
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

/// A drag handle on a pane's edge (`.sidebar-resizer`, `.dock-resizer`, the panel's).
fn resizer(pane: Pane, lit: bool, cx: &mut Context<OrbitApp>) -> impl IntoElement {
    let vertical = pane != Pane::Panel;
    let id = match pane {
        Pane::Sidebar => "resize-sidebar",
        Pane::Dock => "resize-dock",
        Pane::Panel => "resize-panel",
    };
    // 4px, overlapping the pane's edge by half (`margin-left: -2px`).
    let handle = div()
        .id(id)
        .absolute()
        .when(vertical, |d| d.top_0().bottom_0().left(px(-2.)).w(px(size::RESIZER)).cursor(CursorStyle::ResizeLeftRight))
        .when(!vertical, |d| d.left_0().right_0().top(px(-2.)).h(px(size::RESIZER)).cursor(CursorStyle::ResizeUpDown))
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
        );
    div().relative().flex_none().when(vertical, |d| d.w_0().h_full()).when(!vertical, |d| d.h_0().w_full()).child(handle)
}

/// `fmtTokens`.
fn tokens(n: u64) -> String {
    match n {
        1_000_000_000.. => format!("{:.1}B", n as f64 / 1e9),
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}k", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

/// Re-reads the repo briefs (sidebar badges) and the focused workspace's AI
/// usage (status bar) in the background, as `useRepoBriefs` and `UsageBar` poll.
pub fn start_polls(window: &mut Window, cx: &mut Context<OrbitApp>) {
    cx.spawn_in(window, async move |this, cx| loop {
        let briefs = cx.background_executor().spawn(async { orbit_core::repo::briefs() }).await;
        let ok = this.update(cx, |a, cx| {
            if let Ok(list) = briefs {
                a.briefs = list.into_iter().map(|b| (b.name.clone(), b)).collect();
                cx.notify();
            }
        });
        if ok.is_err() {
            break;
        }
        cx.background_executor().timer(Duration::from_secs(30)).await;
    })
    .detach();
    cx.spawn_in(window, async move |this, cx| {
        let mut last: Option<(String, Instant)> = None;
        loop {
            let Ok(focus) = this.update(cx, |a, cx| a.focus_workspace(cx).map(|w| w.meta.name.clone())) else { break };
            let due = match (&focus, &last) {
                (Some(f), Some((l, at))) => f != l || at.elapsed() >= Duration::from_secs(30),
                (Some(_), None) => true,
                _ => false,
            };
            if let Some(name) = focus.filter(|_| due) {
                last = Some((name.clone(), Instant::now()));
                let n = name.clone();
                let (usage, limits) = cx
                    .background_executor()
                    .spawn(async move { (orbit_core::usage::for_scope(&n).ok(), orbit_core::agent_hooks::rate_limits()) })
                    .await;
                let _ = this.update(cx, |a, cx| {
                    a.usage = Some((name, usage, limits));
                    cx.notify();
                });
            }
            cx.background_executor().timer(Duration::from_millis(1500)).await;
        }
    })
    .detach();
}

impl OrbitApp {
    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hidden = self.layout.sidebar_hidden;
        let dock_hidden = self.layout.dock_hidden;
        let has_focus = self.focus_workspace(cx).is_some();
        let unread = sessions::store(cx).read(cx).inbox.iter().filter(|i| !i.read).count();
        let inbox_on = self.chrome.inbox_open;
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
                    .child(
                        // `.titlebar-name`.
                        div().pr(px(10.)).text_size(px(13.)).font_weight(FontWeight(650.)).text_color(c(theme::TEXT_FAINT)).child("Orbit"),
                    )
                    .child(titlebar_btn(
                        "tb-sidebar",
                        if hidden { "panel-left-expand" } else { "panel-left" },
                        15.,
                        true,
                        if hidden { "Show sidebar" } else { "Hide sidebar" },
                        cx,
                        |a, _, cx| a.toggle_sidebar(cx),
                    ))
                    .child(titlebar_btn("tb-back", "arrow-left", 15., self.nav.can_go_back(), "Back (Alt+← or the mouse back button)", cx, |a, w, cx| {
                        a.navigate(true, w, cx)
                    }))
                    .child(titlebar_btn(
                        "tb-forward",
                        "arrow-right",
                        15.,
                        self.nav.can_go_forward(),
                        "Forward (Alt+→ or the mouse forward button)",
                        cx,
                        |a, w, cx| a.navigate(false, w, cx),
                    ))
                    .child(self.render_ws_select(cx))
                    .child(div().flex_1())
                    .children(self.titlebar_items.iter().cloned())
                    .child(
                        titlebar_btn("tb-inbox", "bell", 14., true, "Notifications", cx, |a, w, cx| a.toggle_inbox(w, cx))
                            .when(inbox_on, |d| d.bg(c(theme::PANEL_RAISED)))
                            .when(unread > 0, |d| {
                                d.child(
                                    // `.inbox-badge`.
                                    div()
                                        .absolute()
                                        .top(px(1.))
                                        .right(px(1.))
                                        .min_w(px(14.))
                                        .h(px(14.))
                                        .px(px(3.))
                                        .rounded_full()
                                        .bg(c(theme::ACCENT))
                                        .text_color(c(theme::ON_ACCENT))
                                        .text_size(px(9.))
                                        .font_weight(FontWeight::BOLD)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(if unread > 9 { "9+".to_string() } else { unread.to_string() }),
                                )
                            }),
                    )
                    .child(titlebar_btn(
                        "tb-dock",
                        if dock_hidden { "panel-right-expand" } else { "panel-right" },
                        15.,
                        has_focus,
                        if dock_hidden { "Show panel" } else { "Hide panel" },
                        cx,
                        |a, _, cx| a.toggle_dock(cx),
                    )),
            )
    }

    /// The workspace select's button: the active scope, after back/forward.
    fn render_ws_select(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.chrome.ws_switcher.is_some();
        let ws = self.workspace();
        let name: SharedString = ws.map_or(scope::GLOBAL_LABEL.into(), |w| w.meta.name.clone().into());
        let cell = self.chrome.ws_switcher_x.clone();
        div()
            .id("tb-workspace")
            .group("tb-workspace")
            .relative()
            .ml(px(6.))
            .h(px(size::TITLEBAR_BTN_H))
            .max_w(px(260.))
            .min_w_0()
            .px(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c(if ws.is_some() { theme::TEXT_DIM } else { theme::TEXT_FAINT }))
            .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
            .when(open, |d| d.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
            .child(gpui::canvas(move |b, _, _| cell.set(f32::from(b.origin.x)), |_, _, _, _| {}).absolute().inset_0())
            .child(orbit_icon(if ws.is_some() { "satellite" } else { "home" }, 13., c(theme::TEXT_FAINT)).group_hover("tb-workspace", |s| s.text_color(c(theme::TEXT_DIM))))
            .child(div().min_w_0().truncate().child(name))
            .child(orbit_icon("chevron", 9., c(theme::TEXT_FAINT)).with_transformation(gpui::Transformation::rotate(gpui::percentage(0.25))))
            .child(probe("tb-workspace"))
            .tooltip(|window, cx| Tooltip::new("Switch workspace (Ctrl+Alt+W)").build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|a, _, w, cx| {
                    cx.stop_propagation();
                    a.toggle_workspace_switcher(w, cx);
                }),
            )
    }

    pub fn toggle_workspace_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.ws_switcher.is_some() {
            self.close_popovers(window, cx);
        } else {
            self.open_workspace_switcher(window, cx);
        }
    }

    pub fn open_workspace_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.inbox_open = false;
        self.chrome.new_session = None;
        self.chrome.ws_switcher = Some(WsSwitcher::default());
        self.chrome.ws_switcher.as_mut().unwrap().highlighted = self.default_switch_row();
        window.focus(&self.overlay_focus, cx);
        window.prevent_default();
        cx.notify();
    }

    /// The select's rows for its filter: the global scope, the workspaces
    /// by name, and "New workspace…" last.
    pub fn switch_items(&self) -> Vec<SwitchItem> {
        let query = self.chrome.ws_switcher.as_ref().map_or("", |s| s.query.as_str());
        let mut names: Vec<&str> = self.workspaces.iter().map(|w| w.meta.name.as_str()).collect();
        names.sort_unstable();
        let mut items: Vec<SwitchItem> = vec![];
        if scope::matches(query, scope::GLOBAL_LABEL) {
            items.push(SwitchItem::Scope(None));
        }
        items.extend(names.into_iter().filter(|n| scope::matches(query, n)).map(|n| SwitchItem::Scope(Some(n.to_string()))));
        items.push(SwitchItem::NewWorkspace);
        items
    }

    /// The active scope's row, else the first.
    fn default_switch_row(&self) -> usize {
        let current = SwitchItem::Scope(self.scope());
        self.switch_items().iter().position(|i| *i == current).unwrap_or(0)
    }

    pub fn pick_switch_item(&mut self, item: SwitchItem, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popovers(window, cx);
        match item {
            SwitchItem::Scope(s) => self.switch_scope(s, window, cx),
            SwitchItem::NewWorkspace => {
                commands::run("workspace.new", self, window, cx);
            }
        }
    }

    /// Keys while the select is open: type to filter, ↑/↓, Enter, Esc.
    fn on_switcher_key(&mut self, ev: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.switch_items();
        let Some(sw) = self.chrome.ws_switcher.as_mut() else { return };
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let n = items.len();
        match ks.key.as_str() {
            "escape" => {
                self.close_popovers(window, cx);
            }
            "enter" => {
                if let Some(item) = items.get(sw.highlighted).cloned() {
                    self.pick_switch_item(item, window, cx);
                }
            }
            "up" => sw.highlighted = (sw.highlighted + n - 1) % n,
            "down" | "tab" => sw.highlighted = (sw.highlighted + 1) % n,
            "backspace" => {
                if m.control {
                    sw.query.clear();
                } else {
                    sw.query.pop();
                }
                self.refilter();
            }
            _ if m.control || m.alt || m.platform => return,
            _ => match ks.key_char.as_deref() {
                Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                    sw.query.push_str(t);
                    self.refilter();
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// After the filter changed: the first match (the active scope's row when cleared).
    fn refilter(&mut self) {
        let empty = self.chrome.ws_switcher.as_ref().is_none_or(|s| s.query.is_empty());
        let row = if empty { self.default_switch_row() } else { 0 };
        if let Some(sw) = self.chrome.ws_switcher.as_mut() {
            sw.highlighted = row;
        }
    }

    fn render_ws_switcher(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sw = self.chrome.ws_switcher.as_ref()?;
        let current = self.scope();
        // Open tabs per scope, sessions included.
        let mut counts: std::collections::HashMap<Scope, usize> = Default::default();
        for t in &self.tabs {
            if let Some(s) = self.tab_scope(t, cx) {
                *counts.entry(s).or_default() += 1;
            }
        }
        let items = self.switch_items();
        let new_at = items.len() - 1;
        let rows = items.into_iter().enumerate().map(|(ix, item)| {
            let lit = ix == sw.highlighted;
            let (icon, label, branch, on, count, probe_id) = match &item {
                SwitchItem::Scope(s) => {
                    let ws = s.as_ref().and_then(|n| self.workspaces.iter().find(|w| &w.meta.name == n));
                    let label: SharedString = s.clone().unwrap_or_else(|| scope::GLOBAL_LABEL.into()).into();
                    (
                        if s.is_some() { "satellite" } else { "home" },
                        label.clone(),
                        ws.map(|w| w.meta.branch.clone()).filter(|b| !b.is_empty()),
                        *s == current,
                        counts.get(s).copied().unwrap_or(0),
                        format!("wsw:{label}"),
                    )
                }
                SwitchItem::NewWorkspace => ("plus", "New workspace…".into(), None, false, 0, "wsw:New workspace…".to_string()),
            };
            let row = div()
                .id(("wsw-row", ix))
                .relative()
                .h(px(size::MENU_ITEM))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .whitespace_nowrap()
                .text_size(px(size::TAB_FONT))
                .text_color(c(if on { theme::ACCENT } else { theme::TEXT }))
                .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(lit, |d| d.bg(c(theme::ACCENT_SOFT)))
                .hover(|s| s.bg(c(theme::ACCENT_SOFT)))
                .child(orbit_icon(icon, 13., c(if on { theme::ACCENT } else { theme::TEXT_FAINT })))
                .child(div().flex_none().max_w(px(170.)).truncate().child(label))
                .children(branch.map(|b| div().min_w_0().truncate().font_weight(FontWeight::NORMAL).text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(b)))
                .child(div().flex_1())
                .when(count > 0, |d| d.child(div().flex_none().text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_FAINT)).child(count.to_string())))
                .child(div().w(px(12.)).flex_none().when(on, |d| d.child(orbit_icon("check", 12., c(theme::ACCENT)))))
                .child(probe(probe_id))
                .on_mouse_down(MouseButton::Left, cx.listener(move |a, _, w, cx| {
                    cx.stop_propagation();
                    a.pick_switch_item(item.clone(), w, cx);
                }));
            if ix == new_at {
                div().child(div().h(px(1.)).my(px(4.)).bg(c(theme::BORDER))).child(row).into_any_element()
            } else {
                row.into_any_element()
            }
        });
        let query = sw.query.clone();
        let filter = div()
            .h(px(32.))
            .px(px(10.))
            .mb(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_size(px(size::TAB_FONT))
            .child(orbit_icon("search", 13., c(theme::TEXT_FAINT)))
            .child(if query.is_empty() {
                div().text_color(c(theme::TEXT_FAINT)).child("Switch workspace…")
            } else {
                div().text_color(c(theme::TEXT)).child(query)
            })
            .child(div().w(px(1.)).h(px(14.)).bg(c(theme::ACCENT)))
            .child(div().flex_1())
            .child(div().text_size(px(10.5)).text_color(c(theme::TEXT_FAINT)).child("Ctrl+Alt+W"));
        Some(
            div()
                .id("wsw-backdrop")
                .track_focus(&self.overlay_focus)
                .absolute()
                .inset_0()
                .on_key_down(cx.listener(Self::on_switcher_key))
                .on_mouse_down(MouseButton::Left, cx.listener(|a, _, w, cx| {
                    a.close_popovers(w, cx);
                }))
                .child(
                    div()
                        .id("ws-switcher")
                        .occlude()
                        .absolute()
                        .top(px(size::TITLEBAR + 2.))
                        .left(px(self.chrome.ws_switcher_x.get().max(8.)))
                        .w(px(320.))
                        .p(px(size::MENU_PAD))
                        .bg(c(theme::PANEL_RAISED))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(8.))
                        .shadow(theme::shadow(12., 32., 0.5))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(filter)
                        .child(div().id("wsw-list").max_h(px(380.)).overflow_y_scroll().children(rows)),
                ),
        )
    }

    pub fn toggle_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.inbox_open = !self.chrome.inbox_open;
        if self.chrome.inbox_open {
            self.chrome.new_session = None;
            self.chrome.ws_switcher = None;
            window.focus(&self.overlay_focus, cx);
        } else {
            self.restore_focus(window, cx);
        }
        cx.notify();
    }

    /// Closes the inbox or the New Session menu; whether one was open.
    pub(crate) fn close_popovers(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = self.chrome.inbox_open || self.chrome.new_session.is_some() || self.chrome.ws_switcher.is_some();
        if open {
            self.chrome.inbox_open = false;
            self.chrome.new_session = None;
            self.chrome.ws_switcher = None;
            self.restore_focus(window, cx);
            cx.notify();
        }
        open
    }

    /// The New Session menu at `origin`; `scope` preselects a folder scope.
    pub fn open_new_session_menu(&mut self, origin: Point<Pixels>, scope: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let focus = match scope {
            Some(s) => Some((s.clone(), sessions::scope_label(&s), "folder")),
            None => self.focus_workspace(cx).map(|w| (w.meta.name.clone(), w.meta.name.clone(), "workspace")),
        };
        self.chrome.inbox_open = false;
        self.chrome.ws_switcher = None;
        self.chrome.new_session = Some(NewSessionMenu { origin, focus, picked: None, extra: None });
        window.focus(&self.overlay_focus, cx);
        window.prevent_default();
        cx.notify();
    }

    fn render_inbox(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.chrome.inbox_open {
            return None;
        }
        let store = sessions::store(cx).read(cx);
        let items = store.inbox.clone();
        let unread = items.iter().filter(|i| !i.read).count();
        let link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .relative()
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(c(theme::TEXT_FAINT))
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(label)
                .child(probe(id))
        };
        let head = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_size(px(12.5))
            .child(div().font_weight(FontWeight::BOLD).child("Notifications"))
            .when(!items.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .when(unread > 0, |d| {
                            d.child(link("inbox-read", "Mark all read").on_click(cx.listener(|_, _, _, cx| sessions::store(cx).update(cx, |s, cx| s.mark_all_read(cx)))))
                        })
                        .child(link("inbox-clear", "Clear").on_click(cx.listener(|_, _, _, cx| sessions::store(cx).update(cx, |s, cx| s.clear_inbox(cx))))),
                )
            });
        let body = if items.is_empty() {
            div()
                .px(px(14.))
                .py(px(18.))
                .text_size(px(12.))
                .text_color(c(theme::TEXT_FAINT))
                .child("You'll hear here when an agent finishes or needs you.")
                .into_any_element()
        } else {
            div()
                .id("inbox-list")
                .max_h(px(360.))
                .overflow_y_scroll()
                .p(px(4.))
                .children(items.into_iter().enumerate().map(|(ix, it)| {
                    let key = it.key.clone();
                    let dot = match (it.read, it.kind) {
                        (true, _) => theme::TEXT_FAINT,
                        (false, InboxKind::Done) => theme::OK,
                        (false, InboxKind::Waiting) => theme::WARN,
                    };
                    let secs = sessions::now_ms().saturating_sub(it.at) / 1000;
                    let ago = match secs {
                        0..=59 => "just now".to_string(),
                        60..=3599 => format!("{}m ago", secs / 60),
                        3600..=86399 => format!("{}h ago", secs / 3600),
                        _ => format!("{}d ago", secs / 86400),
                    };
                    div()
                        .id(("inbox-item", ix))
                        .relative()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_color(c(if it.read { theme::TEXT_DIM } else { theme::TEXT }))
                        .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                        .child(div().mt(px(4.)).size(px(8.)).flex_none().rounded_full().bg(c(dot)))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().text_size(px(12.5)).when(!it.read, |d| d.font_weight(FontWeight::SEMIBOLD)).child(it.title.clone()))
                                .child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).truncate().child(format!("{} · {ago}", it.detail))),
                        )
                        .child(probe(format!("inbox:{ix}")))
                        .on_click(cx.listener(move |a, _, w, cx| {
                            a.chrome.inbox_open = false;
                            sessions::activate(a, &key, w, cx);
                            cx.notify();
                        }))
                }))
                .into_any_element()
        };
        Some(
            div()
                .id("inbox-backdrop")
                .key_context(crate::overlay::CONTEXT)
                .track_focus(&self.overlay_focus)
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, cx.listener(|a, _, w, cx| {
                    a.close_popovers(w, cx);
                }))
                .child(
                    div()
                        .id("inbox-pop")
                        .occlude()
                        .absolute()
                        // Under the bell, right-aligned to it.
                        .top(px(size::TITLEBAR + 2.))
                        .right(px(170.))
                        .w(px(340.))
                        .bg(c(theme::PANEL))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(10.))
                        .shadow(theme::shadow(16., 40., 0.5))
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(head)
                        .child(body),
                ),
        )
    }

    fn render_new_session_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.chrome.new_session.as_ref()?;
        let target = menu.picked.clone().or_else(|| menu.focus.as_ref().map(|f| f.0.clone())).unwrap_or_else(|| HOME_SCOPE.into());
        let mut locations: Vec<(String, &'static str, String, Option<&'static str>)> = vec![];
        if let Some((scope, label, kind)) = &menu.focus {
            let icon = match *kind {
                "workspace" => "satellite",
                "repo" => "repo",
                _ => "folder",
            };
            locations.push((scope.clone(), icon, label.clone(), Some(kind)));
        }
        if menu.focus.as_ref().is_none_or(|f| f.0 != HOME_SCOPE) {
            locations.push((HOME_SCOPE.into(), "home", "Home".into(), None));
        }
        if let Some(extra) = menu.extra.as_ref().filter(|e| menu.focus.as_ref().is_none_or(|f| &f.0 != *e)) {
            locations.push((extra.clone(), "folder", sessions::scope_label(extra), None));
        }
        for repo in &self.services {
            let scope = format!("@{repo}");
            let cloned = self.briefs.get(repo).is_none_or(|b| b.cloned);
            if cloned && menu.focus.as_ref().is_none_or(|f| f.0 != scope) {
                locations.push((scope, "repo", repo.clone(), Some("repo")));
            }
        }
        let item = |id: SharedString| {
            div()
                .id(id.clone())
                .relative()
                .h(px(size::MENU_ITEM))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .whitespace_nowrap()
                .text_size(px(size::TAB_FONT))
                .text_color(c(theme::TEXT))
                .hover(|s| s.bg(c(theme::ACCENT_SOFT)))
                .child(probe(id.to_string()))
        };
        let rows = locations.into_iter().map(|(scope, icon, label, hint)| {
            let on = scope == target;
            let s = scope.clone();
            item(format!("nsm:{label}").into())
                .when(on, |d| {
                    d.bg(c(theme::ACCENT_STRONG_SOFT))
                        .hover(|s| s.bg(c(theme::ACCENT_STRONG_SOFT)))
                        .text_color(c(theme::ACCENT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).rounded_l(px(5.)).bg(c(theme::ACCENT)))
                })
                .child(orbit_icon(icon, 13., c(if on { theme::ACCENT } else { theme::TEXT_FAINT })))
                .child(div().min_w_0().truncate().child(label))
                .children(hint.map(|h| {
                    div().ml_auto().pl(px(8.)).text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(c(if on { theme::ACCENT } else { theme::TEXT_FAINT })).child(h)
                }))
                .child(div().when(hint.is_none(), |d| d.ml_auto()).when(hint.is_some(), |d| d.ml(px(6.))).w(px(12.)).flex_none().when(on, |d| d.child(orbit_icon("check", 12., c(theme::ACCENT)))))
                .on_click(cx.listener(move |a, _, _, cx| {
                    if let Some(m) = a.chrome.new_session.as_mut() {
                        m.picked = Some(s.clone());
                    }
                    cx.notify();
                }))
        });
        let choose = item("nsm:Choose folder…".into())
            .child(orbit_icon("folder", 13., c(theme::TEXT_FAINT)))
            .child("Choose folder…")
            .on_click(cx.listener(|a, _, _, cx| {
                if crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                    return a.toast(ToastKind::Info, "Choose folder (no dialog in self-test)", None, cx);
                }
                let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open a session in…".into()) });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = rx.await {
                        if let Some(p) = paths.into_iter().next() {
                            let scope = format!("dir:{}", p.display());
                            let _ = this.update(cx, |a, cx| {
                                if let Some(m) = a.chrome.new_session.as_mut() {
                                    m.extra = Some(scope.clone());
                                    m.picked = Some(scope);
                                }
                                cx.notify();
                            });
                        }
                    }
                })
                .detach();
            }));
        let agents = AGENTS.iter().map(|(label, _, cmd)| {
            let target = target.clone();
            let cmd = *cmd;
            item(format!("nsm:{label}").into()).child(*label).on_click(cx.listener(move |a, _, w, cx| {
                a.chrome.new_session = None;
                sessions::new_session_in(a, target.clone(), cmd, None, w, cx);
                cx.notify();
            }))
        });
        let viewport = window.viewport_size();
        let width = 260.;
        let x = menu.origin.x.min(viewport.width - px(width + 8.)).max(px(8.));
        let y = menu.origin.y.min(viewport.height - px(460.)).max(px(8.));
        Some(
            div()
                .id("nsm-backdrop")
                .key_context(crate::overlay::CONTEXT)
                .track_focus(&self.overlay_focus)
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, cx.listener(|a, _, w, cx| {
                    a.close_popovers(w, cx);
                }))
                .child(
                    div()
                        .id("new-session-menu")
                        .occlude()
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(px(width))
                        .p(px(size::MENU_PAD))
                        .bg(c(theme::PANEL_RAISED))
                        .border_1()
                        .border_color(c(theme::BORDER_STRONG))
                        .rounded(px(8.))
                        .shadow(theme::shadow(12., 32., 0.5))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().pt(px(6.)).px(px(10.)).pb(px(4.)).text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child("OPEN IN"))
                        .child(div().id("nsm-locations").max_h(px(240.)).overflow_y_scroll().children(rows))
                        .child(choose)
                        .child(div().h(px(1.)).my(px(4.)).bg(c(theme::BORDER)))
                        .children(agents),
                ),
        )
    }

    /// Strip icon and label of a tab, as App.tsx names them.
    fn tab_face(&self, tab: &Tab, cx: &App) -> (&'static str, SharedString) {
        let key = tab.key(cx);
        let rel = |path: &Path| -> String {
            let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let repo = self.workspaces.iter().find_map(|w| path.strip_prefix(&w.dir).ok()).and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().to_string());
            match repo {
                Some(repo) if repo != file => format!("{repo}/{file}"),
                _ => file,
            }
        };
        match &tab.kind {
            TabKind::Terminal(t) => ("terminal", t.read(cx).meta.session_name.clone().into()),
            TabKind::Editor(e) => ("doc", rel(&e.read(cx).path).into()),
            TabKind::Diff(d) => ("diff", format!("{} (diff)", rel(&d.read(cx).path)).into()),
            TabKind::Page(_) => match key.strip_prefix("page:ws:") {
                Some(name) => ("satellite", name.to_string().into()),
                None => ("doc", tab.title(cx)),
            },
            TabKind::Review(_) => match key.strip_prefix("review:changes:") {
                Some(ws) => ("diff", format!("Review · {}", sessions::scope_label(ws)).into()),
                None => ("diff", tab.title(cx)),
            },
            TabKind::Tool(_) => match key.strip_prefix("page:ralph:") {
                Some(ws) => ("spark", format!("Ralph · {}", ws.trim_start_matches('@')).into()),
                None => ("doc", tab.title(cx)),
            },
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strip = self.strip_tabs(cx);
        let mut els: Vec<AnyElement> = vec![];
        for (pos, &ix) in strip.iter().enumerate() {
            let tab = &self.tabs[ix];
            let active = self.active == Some(ix);
            let dirty = tab.dirty(cx);
            // A divider where the files start, after the sessions and views.
            let group_start = tab.is_file() && pos > 0 && !self.tabs[strip[pos - 1]].is_file();
            let (icon, label) = self.tab_face(tab, cx);
            let status = match &tab.kind {
                TabKind::Terminal(t) => Some(t.read(cx).status),
                _ => None,
            };
            let agent = tab.is_agent(cx);
            let close_tip: SharedString = if agent {
                "Minimize (the session keeps running) — End session from the tab's menu".into()
            } else if dirty {
                "Unsaved changes — close".into()
            } else {
                "Close".into()
            };
            let group: SharedString = format!("tab-{pos}").into();
            if group_start {
                els.push(div().w(px(1.)).h(px(18.)).mb(px(9.)).mx(px(2.)).flex_none().bg(c(theme::BORDER_STRONG)).into_any_element());
            }
            let el = div()
                .id(("tab", pos))
                .group(group.clone())
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
                .when(active, |d| d.bg(c(theme::BG)).text_color(c(theme::TEXT)).border_color(c(theme::ACCENT)).hover(|s| s.bg(c(theme::BG))))
                .child(orbit_icon(icon, 13., c(theme::TEXT_DIM)))
                .child(div().when(tab.preview, |d| d.italic()).child(label))
                .children(status.map(|s| crate::sidebar::status_indicator(format!("tab-st-{pos}"), s)))
                .child(
                    div()
                        .id(("close", pos))
                        .h(px(20.))
                        .min_w(px(18.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
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
                                        .left(px(5.))
                                        .size(px(8.))
                                        .rounded_full()
                                        .bg(c(theme::TEXT_DIM))
                                        .group_hover(group.clone(), |s| s.bg(gpui::transparent_black())),
                                )
                                .child(div().text_color(gpui::transparent_black()).group_hover(group.clone(), |s| s.text_color(c(theme::TEXT_FAINT))).child("×"))
                        })
                        .when(!dirty, |d| d.child("×"))
                        .tooltip(move |window, cx| Tooltip::new(close_tip.clone()).build(window, cx))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.dismiss_tab(ix, window, cx)
                            }),
                        ),
                )
                .child(probe(format!("tab:{pos}")))
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
                .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.dismiss_tab(ix, window, cx)));
            els.push(el.into_any_element());
        }
        div()
            .id("tab-strip")
            .h(px(size::TAB_STRIP))
            .flex_none()
            .flex()
            .items_end()
            .gap(px(2.))
            .pt(px(6.))
            .px(px(8.))
            .overflow_x_scroll()
            .bg(c(theme::PANEL))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(els)
            .child(
                div()
                    .id("new-tab")
                    .group("new-tab")
                    .relative()
                    .size(px(28.))
                    .mb(px(4.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(orbit_icon("plus", 14., c(theme::TEXT_FAINT)).group_hover("new-tab", |s| s.text_color(c(theme::TEXT))))
                    .child(probe("new-tab"))
                    .tooltip(|window, cx| Tooltip::new("New…").build(window, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // At the button's left, 4px under it.
                            let at = gpui::point(ev.position.x - px(14.), ev.position.y + px(18.));
                            this.open_new_session_menu(at, None, window, cx);
                        }),
                    ),
            )
    }

    fn render_content(&self) -> AnyElement {
        match self.active_tab().map(|t| &t.kind) {
            Some(TabKind::Terminal(t)) => t.clone().into_any_element(),
            Some(TabKind::Editor(e)) => e.clone().into_any_element(),
            Some(TabKind::Diff(d)) => d.clone().into_any_element(),
            Some(TabKind::Review(r)) => r.clone().into_any_element(),
            Some(TabKind::Page(p)) => p.clone().into_any_element(),
            Some(TabKind::Tool(p)) => p.clone().into_any_element(),
            None => div().size_full().bg(c(theme::BG)).into_any_element(),
        }
    }

    /// The Build/Run/Problems/Debug panel (`src/run/panel.rs`).
    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        crate::run::render_panel(self, cx)
    }

    /// The right dock (`aside.dock`): icon toolbar, then the Files or Git panel.
    fn render_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.chrome.dock_tab;
        let icon_btn = |id: &'static str, icon: &'static str, on: bool, tip: &'static str| {
            div()
                .id(id)
                .group(id)
                .relative()
                .w(px(30.))
                .h(px(28.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .when(on, |d| d.bg(c(theme::ACCENT_SOFT)).hover(|s| s.bg(c(theme::ACCENT_SOFT))))
                .child(if on {
                    orbit_icon(icon, 17., c(theme::NAV_ACTIVE))
                } else {
                    orbit_icon(icon, 17., c(theme::TEXT_DIM)).group_hover(id, |s| s.text_color(c(theme::TEXT)))
                })
                .child(probe(id))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        };
        let icons = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .pt(px(6.))
            .px(px(8.))
            .pb(px(2.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icon_btn("dock-files", "folder", tab == DockTab::Files, "Files").on_click(cx.listener(|a, _, _, cx| {
                a.chrome.dock_tab = DockTab::Files;
                cx.notify();
            })))
            .child(icon_btn("dock-find", "search", false, "Find in Files (Ctrl+Shift+F)").on_click(cx.listener(|a, _, w, cx| a.open_find(w, cx))))
            .child(icon_btn("dock-git", "git", tab == DockTab::Git, "Git").on_click(cx.listener(|a, _, _, cx| {
                a.chrome.dock_tab = DockTab::Git;
                a.load_git(cx);
                cx.notify();
            })));
        let header = |label: &'static str| {
            div()
                .flex_none()
                .pt(px(12.))
                .px(px(12.))
                .pb(px(10.))
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(c(theme::TEXT_FAINT))
                .child(label)
        };
        let tree = self.tree.read(cx);
        let repo = tree.focus_root().and_then(|r| tree.roots().into_iter().find(|(_, p)| p == r)).map(|(n, _)| n);
        // `.dock-select`: the repo picker.
        let select = div()
            .id("dock-repo")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(28.))
            .px(px(6.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(c(theme::TEXT))
            .child(div().flex_1().min_w_0().truncate().child(repo.unwrap_or_default()))
            .child(div().flex_none().text_size(px(9.)).child("▼"))
            .child(probe("dock-repo"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|a, ev: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    let roots = a.tree.read(cx).roots();
                    let items = roots
                        .into_iter()
                        .map(|(name, path)| {
                            MenuItem::new(name, move |a: &mut OrbitApp, _: &mut Window, cx: &mut Context<OrbitApp>| {
                                let path = path.clone();
                                a.tree.update(cx, |t, cx| t.set_focus_root(path, cx));
                                a.load_git(cx);
                            })
                        })
                        .collect();
                    a.open_menu(gpui::point(ev.position.x - px(100.), ev.position.y + px(16.)), items, w, cx);
                }),
            );
        let toolbar = div().flex_none().flex().items_center().gap(px(6.)).px(px(8.)).pb(px(8.)).child(select).when(tab == DockTab::Git, |d| {
            d.child(
                div()
                    .id("git-refresh")
                    .group("git-refresh")
                    .relative()
                    .size(px(28.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                    .child(orbit_icon("refresh", 14., c(theme::TEXT_FAINT)).group_hover("git-refresh", |s| s.text_color(c(theme::TEXT))))
                    .child(probe("git-refresh"))
                    .tooltip(|window, cx| Tooltip::new("Refresh").build(window, cx))
                    .on_click(cx.listener(|a, _, _, cx| a.load_git(cx))),
            )
        });
        let body = match tab {
            DockTab::Files => div().flex_1().min_h_0().child(self.tree.clone()).into_any_element(),
            DockTab::Git => self.render_git(cx).into_any_element(),
        };
        div()
            .w(px(self.layout.dock_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .child(icons)
            .child(header(if tab == DockTab::Files { "FILES" } else { "GIT" }))
            .child(toolbar)
            .child(body)
    }

    /// Loads the dock repo's changes and commits (`GitPanel.tsx`).
    pub fn load_git(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.tree.read(cx).focus_root().map(Path::to_path_buf) else { return };
        self.chrome.git.loading = true;
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let (changes, commits) = cx
                .background_spawn(async move { (orbit_core::git::changes(&d).unwrap_or_default(), orbit_core::git::commits(&d, 50).unwrap_or_default()) })
                .await;
            let _ = this.update(cx, |a, cx| {
                a.chrome.git = GitPanel { dir: Some(dir), changes, commits, loading: false };
                cx.notify();
            });
        })
        .detach();
    }

    fn render_git(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let git = &self.chrome.git;
        let label = |text: &'static str| {
            div().pt(px(8.)).px(px(4.)).pb(px(6.)).text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT_FAINT)).child(text)
        };
        let dir = git.dir.clone();
        let changes = git.changes.iter().enumerate().map(|(ix, ch)| {
            let (color, letter) = match ch.status.as_str() {
                "A" => (theme::OK, "A"),
                "D" => (theme::DANGER, "D"),
                "U" => (theme::INFO, "U"),
                _ => (theme::WARN, "M"),
            };
            let path = Path::new(&ch.path);
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let parent = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let full = dir.as_ref().map(|d| d.join(&ch.path));
            div()
                .id(("git-change", ix))
                .relative()
                .h(px(19.))
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(5.))
                .rounded(px(3.))
                .cursor_pointer()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(c(theme::TEXT_DIM))
                .hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                .child(
                    div()
                        .size(px(14.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(c(color))
                        .bg(theme::alpha(color, 0.12))
                        .child(letter),
                )
                .child(div().min_w_0().truncate().child(name.clone()))
                .when(!parent.is_empty(), |d| d.child(div().flex_none().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(parent)))
                .child(probe(format!("git-change:{name}")))
                .on_click(cx.listener(move |a, _, w, cx| {
                    if let Some(p) = full.clone() {
                        a.open_diff(p, w, cx);
                    }
                }))
        });
        let commits = git.commits.iter().enumerate().map(|(ix, cm)| {
            div()
                .id(("git-commit", ix))
                .flex()
                .flex_col()
                .gap(px(1.))
                .px(px(6.))
                .py(px(5.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .child(div().truncate().text_size(px(11.5)).line_height(px(16.)).text_color(c(theme::TEXT)).child(cm.message.clone()))
                .child(div().truncate().text_size(px(10.)).line_height(px(14.)).text_color(c(theme::TEXT_FAINT)).child(format!("{} · {}", cm.when, cm.author)))
        });
        let empty = |t: &'static str| div().px(px(6.)).py(px(4.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(t);
        div()
            .id("git-panel")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pt(px(4.))
            .px(px(8.))
            .pb(px(12.))
            .child(
                div().flex().items_center().justify_between().child(label("CHANGES")).child(
                    div()
                        .id("git-review-all")
                        .relative()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(c(theme::ACCENT))
                        .hover(|s| s.text_color(c(theme::ACCENT_HOVER)))
                        .child("Review all")
                        .child(probe("git-review-all"))
                        .on_click(cx.listener(|a, _, w, cx| crate::review::open(a, crate::review::Page::Changes, w, cx))),
                ),
            )
            .when(git.changes.is_empty(), |d| d.child(empty(if git.loading { "Loading…" } else { "No changes" })))
            .children(changes)
            .child(label("COMMITS"))
            .when(git.commits.is_empty() && !git.loading, |d| d.child(empty("No commits")))
            .children(commits)
    }

    /// `footer.usage-bar`: the focused workspace's AI usage and Claude's plan limits.
    fn render_status(&self, cx: &App) -> impl IntoElement {
        let ws = self.focus_workspace(cx).map(|w| w.meta.name.clone()).unwrap_or_default();
        let current = self.usage.as_ref().filter(|u| u.0 == ws);
        let models: AnyElement = match current.and_then(|u| u.1.as_ref()) {
            Some(u) if !u.by_model.is_empty() => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .overflow_hidden()
                .children(u.by_model.iter().map(|m| {
                    div()
                        .flex_none()
                        .px(px(9.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(c(theme::INFO_SOFT))
                        .text_color(c(theme::INFO))
                        .child(m.model.clone())
                        .child(
                            div()
                                .ml(px(4.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(c(theme::TEXT_DIM))
                                .child(format!("{} in / {} out", tokens(m.input_tokens), tokens(m.output_tokens))),
                        )
                        .flex()
                }))
                .into_any_element(),
            Some(_) => div().text_color(c(theme::TEXT_FAINT)).child("no AI usage yet").into_any_element(),
            None => div().text_color(c(theme::TEXT_FAINT)).child("loading…").into_any_element(),
        };
        let chip = |label: &'static str, w: &orbit_core::agent_hooks::RateWindow| {
            let pct = w.used_percentage.clamp(0., 100.);
            let level = if pct >= 90. {
                theme::DANGER
            } else if pct >= 80. {
                theme::WARN
            } else {
                theme::ACCENT
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(c(if pct >= 80. { level } else { theme::TEXT_DIM }))
                .child(label)
                .child(div().w(px(38.)).h(px(4.)).rounded(px(2.)).bg(c(theme::BORDER_STRONG)).overflow_hidden().child(div().h_full().w(px(38. * pct as f32 / 100.)).rounded(px(2.)).bg(c(level))))
                .child(format!("{pct:.0}%"))
        };
        let limits = current.map(|u| &u.2);
        let rate: AnyElement = match limits.filter(|l| l.five_hour.is_some() || l.seven_day.is_some()) {
            Some(l) => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .children(l.five_hour.as_ref().map(|w| chip("5h", w)))
                .children(l.seven_day.as_ref().map(|w| chip("Week", w)))
                .into_any_element(),
            None => div().text_color(c(theme::TEXT_FAINT)).child("Limits —").into_any_element(),
        };
        let label = ws.trim_start_matches('@').to_string();
        let sessions = match current.and_then(|u| u.1.as_ref()).map(|u| u.sessions) {
            Some(n) if n > 0 => format!("{n} sessions · {label}"),
            _ => label,
        };
        div()
            .id("usage-bar")
            .h(px(size::STATUS_BAR))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(12.))
            .text_size(px(11.))
            .bg(c(theme::PANEL))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_color(c(theme::TEXT_DIM))
            .child(div().flex_1().min_w_0().flex().items_center().child(models))
            .child(div().flex_none().child(rate))
            .child(div().flex_none().ml(px(2.)).text_color(c(theme::TEXT_FAINT)).child(sessions))
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
        let focus = self.focus_workspace(cx).is_some();
        // Settings is its own screen: no tabs over it, and its nav replaces the
        // sidebar (the page draws it). No tabs at all: no strip.
        let on_settings = self.active_tab().is_some_and(|t| t.key(cx) == crate::pages::SETTINGS_KEY);
        let show_dock = focus && !self.layout.dock_hidden && !on_settings;
        let show_strip = !on_settings && !self.strip_tabs(cx).is_empty();
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(show_strip, |d| d.child(self.render_tabs(cx)))
            .child(div().flex_1().min_h_0().bg(c(theme::BG)).child(self.render_content()))
            .when(self.layout.panel_open, |d| d.child(resizer(Pane::Panel, false, cx)).child(self.render_panel(cx)));
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
                    .when(!self.layout.sidebar_hidden && !on_settings, |d| d.child(self.render_sidebar(cx)).child(resizer(Pane::Sidebar, sidebar_lit, cx)))
                    .child(main)
                    .when(show_dock, |d| d.child(resizer(Pane::Dock, false, cx)).child(self.render_dock(cx))),
            )
            .when(focus && !on_settings, |d| d.child(self.render_status(cx)))
            .children(self.render_drag_layer(cx))
            .children(self.render_toasts(cx))
            .children(self.render_palette(cx))
            .children(self.render_inbox(cx))
            .children(self.render_new_session_menu(window, cx))
            .children(self.render_ws_switcher(cx))
            .children(self.render_menu(window, cx))
            .children(self.render_dialog(cx))
    }
}
