//! The left sidebar, as `src/App.tsx` draws it (`aside.sidebar`, App.css
//! "Sidebar"): the brand, WORKSPACES (Dashboard, Code Review, each workspace
//! with its sessions and recent ones), SESSIONS with "+", REPOS with their
//! sync badges, GENERAL, and the repos/groups footer. Dragged to 64 px it
//! shows icons only. Clicks call the modules that own each page.
use crate::app::{OrbitApp, TabKind};
use crate::commands;
use crate::icons::orbit_icon;
use crate::overlay::MenuItem;
use crate::probe::probe;
use crate::sessions::{self, rename::Place, status::Status, InboxKind, RecentSession};
use crate::theme::{self, c, size};
use crate::toast::ToastKind;
use crate::workspace_ui::{self, PageId};
use gpui::{
    div, percentage, prelude::*, px, Animation, AnimationExt, AnyElement, App, Context, FontWeight, MouseButton,
    MouseDownEvent, SharedString, Stateful, Transformation, Window,
};
use gpui_component::tooltip::Tooltip;
use std::path::{Path, PathBuf};
use crate::motion::{self, TweenExt};

/// Recent sessions listed per scope (`RECENT_SHOWN`).
const RECENT_SHOWN: usize = 5;

/// What a sidebar session row stands for.
#[derive(Clone)]
struct SessionRow {
    tab_key: String,
    /// The session's key; none for an editor.
    session: Option<String>,
    label: SharedString,
    /// None: an editor tab (doc icon).
    status: Option<Status>,
    agent: bool,
    minimized: bool,
    active: bool,
    unread: Option<InboxKind>,
    tooltip: SharedString,
}

/// `.nav-section`: 10px/600 uppercase, faint, padding 10 10 6.
fn section(label: &'static str) -> gpui::Div {
    div()
        .flex_none()
        .pt(px(10.))
        .px(px(10.))
        .pb(px(6.))
        .text_size(px(10.))
        .line_height(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::TEXT_FAINT))
        .child(label)
}

/// `.nav-item`: 32px, padding 0 10, radius 6, gap 9, 13px/500, text-dim; icon slot 18px.
fn nav_item(id: impl Into<SharedString>, icon: &'static str, label: Option<SharedString>, active: bool) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let group: SharedString = format!("g-{id}").into();
    let icon_color = if active { theme::NAV_ACTIVE } else { theme::TEXT_DIM };
    div()
        .id(id.clone())
        .group(group.clone())
        .relative()
        .flex_none()
        .h(px(size::NAV_ITEM))
        .w_full()
        .flex()
        .items_center()
        .when(label.is_some(), |d| d.px(px(10.)).gap(px(9.)))
        .when(label.is_none(), |d| d.justify_center())
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(theme::TEXT_DIM))
        .hover(move |s| if active { s.bg(c(theme::ACCENT_SOFT)).text_color(c(theme::NAV_ACTIVE)) } else { s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)) })
        .when(active, |d| d.bg(c(theme::ACCENT_SOFT)).text_color(c(theme::NAV_ACTIVE)))
        .child(
            div().w(px(18.)).flex_none().flex().justify_center().child(
                orbit_icon(icon, 15., c(icon_color)).when(!active, |s| s.group_hover(group, |s| s.text_color(c(theme::TEXT)))),
            ),
        )
        .children(label.map(|l| div().min_w_0().truncate().child(l)))
        .child(probe(id.to_string()))
}

/// `.nav-ws`: a workspace, folder or repo row (soft hover; active gets the accent bar).
fn nav_ws(id: impl Into<SharedString>, icon: &'static str, label: SharedString, active: bool) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let group: SharedString = format!("g-{id}").into();
    div()
        .id(id.clone())
        .group(group.clone())
        .relative()
        .flex_none()
        .h(px(size::NAV_ITEM))
        .w_full()
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c(theme::TEXT_DIM))
        .hover(move |s| if active { s.bg(c(theme::ACCENT_SOFT)) } else { s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)) })
        .when(active, |d| {
            d.bg(c(theme::ACCENT_SOFT))
                .text_color(c(theme::TEXT))
                // `box-shadow: inset 2px 0 0 var(--accent)`.
                .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).rounded_l(px(6.)).bg(c(theme::ACCENT)))
        })
        .child(div().w(px(18.)).flex_none().flex().justify_center().child(if active {
            orbit_icon(icon, 15., c(theme::ACCENT))
        } else {
            orbit_icon(icon, 15., c(theme::TEXT_DIM)).group_hover(group, |s| s.text_color(c(theme::TEXT)))
        }))
        .child(div().min_w_0().truncate().child(label))
        .child(probe(id.to_string()))
}

/// `.nav-unread`: 7px; green when done, pulsing amber when waiting.
fn unread_dot(id: impl Into<SharedString>, kind: InboxKind) -> AnyElement {
    match kind {
        InboxKind::Done => div().size(px(7.)).flex_none().rounded_full().bg(c(theme::OK)).into_any_element(),
        InboxKind::Waiting => motion::status_pulse(id.into(), 7., c(theme::WARN), motion::STATUS_PULSE),
    }
}

/// `StatusIndicator`: idle ring, spinning arc while working, pulsing dot
/// when busy or waiting, solid green once exited.
pub fn status_indicator(id: impl Into<SharedString>, status: Status) -> AnyElement {
    let id: SharedString = id.into();
    let dot = || div().size(px(8.)).flex_none().rounded_full();
    match status {
        Status::Idle => dot().border_2().border_color(c(theme::TEXT_FAINT)).into_any_element(),
        Status::Exited => dot().bg(c(theme::OK)).into_any_element(),
        Status::Waiting => motion::status_pulse(id, 8., c(theme::WARN), motion::STATUS_PULSE),
        Status::Busy => motion::status_pulse(id, 8., c(theme::ACCENT), motion::STATUS_PULSE),
        // Thinking, running, editing.
        _ => div()
            .relative()
            .size(px(11.))
            .flex_none()
            .child(div().absolute().inset_0().rounded_full().border_2().border_color(c(theme::BORDER_STRONG)))
            .child(
                crate::icons::ui_icon("arc-top", 11., c(theme::ACCENT))
                    .absolute()
                    .top_0()
                    .left_0()
                    .with_animation(gpui::ElementId::Name(id), Animation::new(motion::STATUS_SPIN).repeat_synced(), |s, t| {
                        s.with_transformation(Transformation::rotate(percentage(t)))
                    }),
            )
            .into_any_element(),
    }
}

impl OrbitApp {
    /// The sidebar's session rows of `scope`: its terminals, and (for a
    /// workspace) the editors of files inside it, in tab order.
    fn session_rows(&self, scope: &str, ws_dir: Option<&Path>, cx: &App) -> Vec<SessionRow> {
        let store = sessions::store(cx).read(cx);
        let active = self.active;
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, t)| {
                let key = t.key(cx);
                match &t.kind {
                    TabKind::Terminal(v) => {
                        let v = v.read(cx);
                        (v.meta.workspace == scope).then(|| {
                            let minimized = self.minimized.contains(&key);
                            let name: SharedString = v.meta.session_name.clone().into();
                            SessionRow {
                                tab_key: key,
                                session: Some(v.meta.id.clone()),
                                label: name.clone(),
                                status: Some(v.status),
                                agent: v.is_agent(),
                                minimized,
                                active: active == Some(ix),
                                unread: store.unread(&v.meta.id),
                                tooltip: if minimized { format!("{name} — minimized, still running").into() } else { name },
                            }
                        })
                    }
                    TabKind::Editor(e) => {
                        let path = &e.read(cx).path;
                        let dir = ws_dir?;
                        let rel = path.strip_prefix(dir).ok()?;
                        let repo = rel.components().next()?.as_os_str().to_string_lossy().to_string();
                        let file = path.file_name()?.to_string_lossy().to_string();
                        Some(SessionRow {
                            tab_key: key,
                            session: None,
                            label: format!("{repo}/{file}").into(),
                            status: None,
                            agent: false,
                            minimized: false,
                            active: active == Some(ix),
                            unread: None,
                            tooltip: rel.to_string_lossy().to_string().into(),
                        })
                    }
                    _ => None,
                }
            })
            .collect()
    }

    fn session_row(&self, row: SessionRow, cx: &mut Context<Self>) -> AnyElement {
        let key = row.tab_key.clone();
        let menu_key = row.tab_key.clone();
        let (agent, minimized) = (row.agent, row.minimized);
        let icon = match row.status {
            Some(s) => div().flex_none().w(px(8.)).flex().justify_center().child(status_indicator(format!("st-{key}"), s)).into_any_element(),
            None => div().w(px(18.)).flex_none().flex().justify_center().child(orbit_icon("doc", 13., c(theme::TEXT_DIM))).into_any_element(),
        };
        let tip = row.tooltip.clone();
        let session = row.session.clone();
        let label = match session.as_deref().and_then(|k| self.session_rename_field(k, Place::Sidebar, cx)) {
            Some(field) => field,
            None => div().min_w_0().truncate().when(minimized, |d| d.italic().opacity(0.75)).child(row.label.clone()).into_any_element(),
        };
        div()
            .id(SharedString::from(format!("session:{}", row.label)))
            .relative()
            .flex_none()
            .h(px(size::NAV_SUB))
            .w_full()
            .pl(px(30.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(9.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c(if row.active { theme::NAV_ACTIVE } else { theme::TEXT_DIM }))
            .hover(|s| s.text_color(c(theme::TEXT)))
            .child(icon)
            .child(label)
            .children(row.unread.map(|k| div().ml_auto().child(unread_dot(format!("unread-{key}"), k))))
            .child(probe(format!("session:{}", row.label)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |a, _, w, cx| a.activate_key(&key, w, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |a, ev: &MouseDownEvent, w, cx| {
                    let k = menu_key.clone();
                    let open = {
                        let k = k.clone();
                        MenuItem::new("Open", move |a, w, cx| a.activate_key(&k, w, cx))
                    };
                    let mut items = vec![open];
                    if let Some(id) = session.clone() {
                        items.push(MenuItem::new("Rename session", move |a, w, cx| a.start_session_rename(&id, Place::Sidebar, w, cx)));
                    }
                    if agent {
                        if !minimized {
                            let k2 = k.clone();
                            items.push(MenuItem::new("Minimize", move |a, w, cx| {
                                if let Some(ix) = a.tab_index(&k2, cx) {
                                    a.minimize_tab(ix, w, cx);
                                }
                            }));
                        }
                        items.push(
                            MenuItem::new("End session", move |a, w, cx| {
                                if let Some(ix) = a.tab_index(&k, cx) {
                                    a.close_tab(ix, true, w, cx);
                                }
                            })
                            .danger(),
                        );
                    } else {
                        items.push(
                            MenuItem::new("Close", move |a, w, cx| {
                                if let Some(ix) = a.tab_index(&k, cx) {
                                    a.close_tab(ix, false, w, cx);
                                }
                            })
                            .danger(),
                        );
                    }
                    a.open_menu(ev.position, items, w, cx);
                }),
            )
            .into_any_element()
    }

    /// "RECENT SESSIONS" under a scope: ended agent sessions, click resumes.
    fn recent_rows(&self, scope: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list: Vec<RecentSession> = sessions::store(cx).read(cx).recent.iter().filter(|r| r.workspace == scope).take(RECENT_SHOWN).cloned().collect();
        if list.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .child(div().pt(px(6.)).pr(px(12.)).pb(px(2.)).pl(px(30.)).text_size(px(10.)).line_height(px(14.)).text_color(c(theme::TEXT_FAINT)).child("RECENT SESSIONS"))
                .children(list.into_iter().map(|r| {
                    let id: SharedString = format!("recent:{}", r.session_name).into();
                    let group: SharedString = format!("g-{id}").into();
                    let tip: SharedString = format!("Resume {} ({})", r.session_name, r.agent).into();
                    let (r1, r2) = (r.clone(), r.clone());
                    div()
                        .id(id.clone())
                        .group(group.clone())
                        .relative()
                        .h(px(size::NAV_SUB))
                        .w_full()
                        .pl(px(30.))
                        .pr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(theme::TEXT_FAINT))
                        .hover(|s| s.text_color(c(theme::TEXT)))
                        .child(div().w(px(18.)).flex_none().flex().justify_center().child(crate::icons::ui_icon("rotate-ccw", 10., c(theme::TEXT_FAINT)).group_hover(group.clone(), |s| s.text_color(c(theme::TEXT)))))
                        .child(div().min_w_0().truncate().child(r.session_name.clone()))
                        .child(
                            div()
                                .id(SharedString::from(format!("recent-forget:{}", r.session_name)))
                                .relative()
                                .ml_auto()
                                .px(px(4.))
                                .text_color(gpui::transparent_black())
                                .group_hover(group, |s| s.text_color(c(theme::TEXT_FAINT)))
                                .hover(|s| s.text_color(c(theme::TEXT)))
                                .child("×")
                                .child(probe(format!("recent-forget:{}", r.session_name)))
                                .tooltip(|window, cx| Tooltip::new("Remove from recent").build(window, cx))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    sessions::forget_recent(&r2.resume_id, cx);
                                })),
                        )
                        .child(probe(id.to_string()))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |a, _, w, cx| sessions::resume(a, &r1, w, cx)))
                })
                .collect::<Vec<_>>())
                .into_any_element(),
        )
    }

    /// Activates a tab by key (a sidebar row, a menu).
    pub fn activate_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tab_index(key, cx) {
            self.activate(ix, window, cx);
        }
    }

    fn is_active_key(&self, pred: impl Fn(&str) -> bool, cx: &App) -> bool {
        self.active_tab().is_some_and(|t| pred(&t.key(cx)))
    }

    /// Opens a workspace's page tab (`openWorkspaceTab`).
    pub fn open_workspace_page(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.workspaces.iter().position(|w| w.meta.name == name) {
            if self.selected != Some(ix) {
                self.select_workspace(ix, cx);
            }
        }
        workspace_ui::open(self, PageId::Workspace(name.to_string()), window, cx);
    }

    fn workspace_menu(&mut self, name: String, dir: PathBuf, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (n1, n2) = (name.clone(), name.clone());
        let (d1, d2) = (dir.clone(), dir);
        let items = vec![
            MenuItem::new("Open workspace", move |a, w, cx| a.open_workspace_page(&n1, w, cx)),
            MenuItem::new("New terminal", move |a, w, cx| sessions::new_session_in(a, n2.clone(), None, None, w, cx)),
            MenuItem::new("Open in editor", move |_, _, cx| {
                let dir = d1.clone();
                cx.spawn(async move |this, cx| {
                    let r = cx.background_spawn(async move { orbit_core::settings::launch_editor(&dir) }).await;
                    if let Err(e) = r {
                        let _ = this.update(cx, |a, cx| a.toast(ToastKind::Error, "Couldn't open the editor", Some(e.into()), cx));
                    }
                })
                .detach();
            }),
            MenuItem::new("Open folder in Finder", move |a, _, cx| {
                if crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                    a.toast(ToastKind::Info, "Revealed", Some(d2.display().to_string().into()), cx);
                } else {
                    cx.reveal_path(&d2);
                }
            }),
        ];
        self.open_menu(position, items, window, cx);
    }

    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.layout.sidebar_collapsed();
        let on = |p: &dyn Fn(&str) -> bool| self.is_active_key(p, cx);
        let dashboard_on = on(&|k| k == "page:dashboard");
        let review_on = on(&|k| k == "page:code-review");
        let settings_on = on(&|k| k == "page:settings");
        let integrations_on = on(&|k| k == "page:integrations");
        let label = |s: &'static str| (!collapsed).then(|| SharedString::from(s));

        let dashboard = nav_item("side-dashboard", "home", label("Dashboard"), dashboard_on)
            .tooltip(|window, cx| Tooltip::new("Dashboard").build(window, cx))
            .on_click(cx.listener(|a, _, w, cx| a.show_dashboard(w, cx)));
        let code_review = nav_item("review-prs", "git", label("Code Review"), review_on)
            .tooltip(|window, cx| Tooltip::new("Code Review").build(window, cx))
            .on_click(cx.listener(|a, _, w, cx| crate::review::open_code_review(a, w, cx)));
        let settings = nav_item("nav-settings", "settings", label("Settings"), settings_on)
            .tooltip(|window, cx| Tooltip::new("Settings").build(window, cx))
            .on_click(cx.listener(|a, _, w, cx| {
                commands::run("app.settings", a, w, cx);
            }));
        let integrations = nav_item("nav-integrations", "plug", label("Integrations"), integrations_on)
            .tooltip(|window, cx| Tooltip::new("Integrations").build(window, cx))
            .on_click(cx.listener(|a, _, w, cx| {
                commands::run("app.integrations", a, w, cx);
            }));

        let brand = div()
            .flex_none()
            .h(px(20.))
            .flex()
            .items_center()
            .gap(px(7.))
            .when(!collapsed, |d| d.px(px(8.)))
            .when(collapsed, |d| d.justify_center())
            .child(orbit_icon("satellite", if collapsed { 20. } else { 18. }, c(theme::ACCENT)))
            .when(!collapsed, |d| {
                d.child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child("Orbit")).child(
                    div().mt(px(2.)).text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_FAINT)).child("multi-repo workspace"),
                )
            });

        let mut nav = div().id("sidebar-nav").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.));
        if collapsed {
            nav = nav.child(dashboard).child(code_review).child(settings).child(integrations);
        } else {
            nav = nav.child(section("WORKSPACES")).child(dashboard).child(code_review);
            nav = nav.children(self.render_workspaces(cx));
            nav = nav.child(self.render_sessions_header(cx));
            nav = nav.children(self.render_folder_scopes(cx));
            if !self.services.is_empty() {
                nav = nav.child(section("REPOS"));
            }
            nav = nav.children(self.render_repos(cx));
            nav = nav.child(section("GENERAL")).child(settings).child(integrations);
        }

        let stat = |value: usize, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(11.5))
                .text_color(c(theme::TEXT_FAINT))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child(value.to_string()))
                .child(label)
        };
        let footer = (!collapsed).then(|| {
            div()
                .flex_none()
                .flex()
                .gap(px(16.))
                .pt(px(10.))
                .px(px(8.))
                .border_t_1()
                .border_color(c(theme::BORDER))
                .child(stat(self.services.len(), "repos"))
                .child(stat(self.group_count, "groups"))
        });

        div()
            .id("sidebar")
            .w(px(self.layout.sidebar_width))
            .flex_none()
            .h_full()
            .overflow_hidden()
            .bg(c(theme::PANEL))
            .py(px(16.))
            .px(px(12.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(brand)
            .child(nav)
            .children(footer)
    }

    fn render_workspaces(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut list: Vec<&crate::workspace::Workspace> = self.workspaces.iter().collect();
        // Variants sit right after the workspace they race for.
        list.sort_by(|a, b| {
            let base = |w: &crate::workspace::Workspace| w.meta.variant_of.clone().unwrap_or_else(|| w.meta.name.clone());
            base(a).cmp(&base(b)).then(a.meta.variant_of.is_some().cmp(&b.meta.variant_of.is_some())).then(a.meta.name.cmp(&b.meta.name))
        });
        list.into_iter()
            .map(|ws| {
                let name = ws.meta.name.clone();
                let rows = self.session_rows(&name, Some(&ws.dir), cx);
                let recent = self.recent_rows(&name, cx);
                let page_key = format!("page:ws:{name}");
                let active = self.is_active_key(|k| k == page_key, cx);
                let expanded = !self.ws_collapsed.contains(&name);
                let label: SharedString = match &ws.meta.variant_of {
                    Some(base) => name.get(base.len() + 2..).unwrap_or(&name).to_string().into(),
                    None => name.clone().into(),
                };
                let mut kinds: Vec<InboxKind> = rows.iter().filter_map(|r| r.unread).collect();
                // A Ralph run's inbox item (`ralph:<ws>`), as App.tsx.
                kinds.extend(sessions::store(cx).read(cx).unread(&format!("ralph:{name}")));
                let unread = if kinds.contains(&InboxKind::Waiting) { Some(InboxKind::Waiting) } else { kinds.first().copied() };
                let ralph = orbit_core::ralph::running().unwrap_or_default().iter().any(|k| k.starts_with(&format!("{name}/")));
                let has_children = !rows.is_empty() || recent.is_some();
                let row_id: SharedString = format!("ws:{name}").into();
                let (n_click, n_chev, n_menu, dir) = (name.clone(), name.clone(), name.clone(), ws.dir.clone());
                let chevron = has_children.then(|| {
                    let tip = if expanded { "Collapse sessions" } else { "Expand sessions" };
                    let rot_id = SharedString::from(format!("chevron-rot:{name}"));
                    let el = div()
                        .id(SharedString::from(format!("chevron:{name}")))
                        .relative()
                        .ml_auto()
                        .size(px(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .hover(|s| s.bg(c(theme::ACCENT_SOFT)))
                        .child(probe(format!("chevron:{name}")))
                        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                        .on_click(cx.listener(move |a, _, _, cx| {
                            cx.stop_propagation();
                            if !a.ws_collapsed.remove(&n_chev) {
                                a.ws_collapsed.insert(n_chev.clone());
                            }
                            cx.notify();
                        }));
                    // `.nav-chevron`: hidden at scale .6, shown on row hover (`h`), turned 90° when open.
                    move |h: f32| {
                        let shown = if expanded { 1. } else { h };
                        let mut color = c(if expanded { theme::TEXT } else { theme::TEXT_DIM });
                        color.a *= shown;
                        let scale = 0.6 + 0.4 * shown;
                        let icon = orbit_icon("chevron-right", 8., color).tween(rot_id, if expanded { 0.25 } else { 0. }, motion::TRANSITION, motion::standard(), move |s, r| {
                            s.with_transformation(Transformation::rotate(percentage(r)).with_scaling(gpui::size(scale, scale)))
                        });
                        el.child(icon)
                    }
                });
                let row = nav_ws(row_id, "satellite", label, active)
                    .tooltip({
                        let n = name.clone();
                        move |window, cx| Tooltip::new(n.clone()).build(window, cx)
                    })
                    .when(ralph, |d| d.child(ralph_dot(format!("ralph-{name}"))))
                    .children(unread.map(|k| unread_dot(format!("ws-unread-{name}"), k)))
                    .on_click(cx.listener(move |a, _, w, cx| a.open_workspace_page(&n_click, w, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |a, ev: &MouseDownEvent, w, cx| a.workspace_menu(n_menu.clone(), dir.clone(), ev.position, w, cx)),
                    )
                    .hover_tween(SharedString::from(format!("ws-hover:{name}")), motion::TRANSITION, motion::standard(), move |d, h| {
                        d.children(chevron.map(|f| f(h)))
                    });
                div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .when(expanded, |d| d.children(rows.into_iter().map(|r| self.session_row(r, cx))).children(recent))
                    .into_any_element()
            })
            .collect()
    }

    /// "SESSIONS" with its "+" (`.nav-section-row`; the button's 34px height makes the row 50px).
    fn render_sessions_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(10.))
            .pl(px(10.))
            .pr(px(6.))
            .pb(px(6.))
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(theme::TEXT_FAINT))
            .child("SESSIONS")
            .child(
                div()
                    .id("sessions-add")
                    .relative()
                    .w(px(18.))
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .group("sessions-add")
                    .hover(|s| s.bg(c(theme::TEXT_SOFT)))
                    .child(orbit_icon("plus", 12., c(theme::TEXT_FAINT)).group_hover("sessions-add", |s| s.text_color(c(theme::TEXT))))
                    .child(probe("sessions-add"))
                    .tooltip(|window, cx| Tooltip::new("New session").build(window, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|a, ev: &MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            // Below the button: its bottom is ~17px under the press.
                            a.open_new_session_menu(gpui::point(ev.position.x - px(9.), ev.position.y + px(21.)), None, w, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Sessions started outside workspaces, grouped by folder (`dir:<path>`), A→Z.
    fn render_folder_scopes(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = sessions::store(cx).read(cx);
        let mut scopes: Vec<String> = store
            .sessions()
            .map(|(m, _)| m.workspace.clone())
            .chain(store.recent.iter().map(|r| r.workspace.clone()))
            .filter(|s| s.starts_with("dir:"))
            .collect();
        scopes.sort_by_key(|s| sessions::scope_label(s).to_lowercase());
        scopes.dedup();
        scopes
            .into_iter()
            .map(|scope| {
                let rows = self.session_rows(&scope, None, cx);
                let recent = self.recent_rows(&scope, cx);
                let unread = rows.iter().find_map(|r| r.unread);
                let path = scope.trim_start_matches("dir:").to_string();
                let tip: SharedString = format!("{path} — new session here").into();
                let s2 = scope.clone();
                let row = nav_ws(format!("folder:{}", sessions::scope_label(&scope)), "folder", sessions::scope_label(&scope).into(), false)
                    .children(unread.map(|_| unread_dot(format!("folder-unread-{scope}"), InboxKind::Done)))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |a, ev: &MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            a.open_new_session_menu(gpui::point(ev.position.x, ev.position.y + px(20.)), Some(s2.clone()), w, cx);
                        }),
                    );
                div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .children(rows.into_iter().map(|r| self.session_row(r, cx)))
                    .children(recent)
                    .into_any_element()
            })
            .collect()
    }

    fn render_repos(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut names = self.services.clone();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let scope = format!("@{name}");
                let rows = self.session_rows(&scope, None, cx);
                let recent = self.recent_rows(&scope, cx);
                let unread = rows.iter().find_map(|r| r.unread);
                let key = format!("page:repo:{name}");
                let active = self.is_active_key(|k| k == key, cx);
                let b = self.briefs.get(&name);
                let tip: SharedString = match b {
                    None => name.clone(),
                    Some(b) if !b.cloned => format!("{name} · not cloned"),
                    Some(b) => {
                        let mut parts = vec![format!("{name} on {}", b.branch.clone().unwrap_or_else(|| "detached HEAD".into()))];
                        if b.changes > 0 {
                            parts.push(format!("{} changed file{}", b.changes, if b.changes == 1 { "" } else { "s" }));
                        }
                        if b.ahead > 0 {
                            parts.push(format!("{} to push", b.ahead));
                        }
                        if b.behind > 0 {
                            parts.push(format!("{} to pull", b.behind));
                        }
                        parts.join(" · ")
                    }
                }
                .into();
                let ralph = orbit_core::ralph::running().unwrap_or_default().iter().any(|k| k.starts_with(&format!("@{name}/")));
                let mut badges = div().ml_auto().flex_none().flex().items_center().gap(px(6.));
                if ralph {
                    badges = badges.child(ralph_dot(format!("ralph-@{name}")));
                }
                if let Some(b) = b.filter(|b| b.cloned) {
                    if b.ahead > 0 || b.behind > 0 {
                        let mut sync = String::new();
                        if b.ahead > 0 {
                            sync.push_str(&format!("↑{}", b.ahead));
                        }
                        if b.behind > 0 {
                            sync.push_str(&format!("↓{}", b.behind));
                        }
                        badges = badges.child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::INFO)).child(sync));
                    }
                    if b.changes > 0 {
                        badges = badges.child(div().size(px(6.)).rounded_full().bg(c(theme::WARN)));
                    }
                }
                if b.is_some_and(|b| !b.cloned) {
                    badges = badges.child(div().text_size(px(10.)).text_color(c(theme::TEXT_FAINT)).child("not cloned"));
                }
                if unread.is_some() {
                    badges = badges.child(unread_dot(format!("repo-unread-{name}"), InboxKind::Done));
                }
                let n = name.clone();
                let row = nav_ws(format!("repo:{name}"), "repo", name.clone().into(), active)
                    .child(badges)
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |a, _, w, cx| {
                        workspace_ui::open(a, PageId::Repo(n.clone()), w, cx);
                    }));
                div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .children(rows.into_iter().map(|r| self.session_row(r, cx)))
                    .children(recent)
                    .into_any_element()
            })
            .collect()
    }
}

/// `.nav-ralph`: a 7px accent dot with a 3px soft ring, `rv-pulse`.
fn ralph_dot(id: String) -> gpui::Div {
    div().ml_auto().flex_none().child(motion::rv_pulse(SharedString::from(id), 7., 3., c(theme::ACCENT), theme::accent_soft()))
}
