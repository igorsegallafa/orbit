//! AI usage of a workspace (`UsageBar.tsx`): tokens and cost per model with
//! its sessions, from Claude Code's logs and OpenCode's database, and the
//! Claude subscription windows. Refreshed every 30 s, as the bar is. Its
//! sessions can be ended here, and Orbit's leftover session files cleaned.
use super::kit;
use super::{Host, PageView};
use crate::review::Load;
use crate::sessions;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use crate::workspace_ui::{kit as wk, plural};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, FontWeight, SharedString, Subscription, Task, Window};
use orbit_core::agent_hooks::{RateLimits, RateWindow};
use orbit_core::usage::{AiUsage, SessionDetail};
use std::fmt::Write as _;
use std::time::Duration;

pub struct UsagePage {
    host: Host,
    workspace: String,
    usage: Load<AiUsage>,
    limits: RateLimits,
    _poll: Task<()>,
    _sessions: Subscription,
}

pub fn tokens(n: u64) -> String {
    match n {
        1_000_000_000.. => format!("{:.1}B", n as f64 / 1e9),
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}k", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        1_073_741_824.. => format!("{:.1} GB", bytes as f64 / 1_073_741_824.),
        1_048_576.. => format!("{:.1} MB", bytes as f64 / 1_048_576.),
        1024.. => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{bytes} B"),
    }
}

/// "2h 5m" until a unix time.
fn until(unix: u64) -> String {
    let m = unix.saturating_sub(super::ui::now()).div_ceil(60);
    match m {
        0..60 => format!("{m}m"),
        60..1440 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d {}h", m / 1440, (m % 1440) / 60),
    }
}

/// A session's title: the agent's own, else the Orbit tab's name, else its short id.
fn title(s: &SessionDetail, orbit: Option<&str>) -> String {
    match orbit {
        _ if s.title != s.id && !s.title.is_empty() => s.title.clone(),
        Some(name) => name.to_string(),
        None => format!("Session {}", short_id(&s.id)),
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[derive(Clone, Copy, PartialEq)]
enum Tracked {
    Open,
    Recent,
}

impl UsagePage {
    pub fn new(host: Host, ws: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            if this.update_in(cx, |t: &mut Self, w, cx| t.load(w, cx)).is_err() {
                break;
            }
        });
        let sessions = cx.observe(&sessions::store(cx), |_, _, cx| cx.notify());
        let mut page = Self {
            host,
            workspace: ws.meta.name.clone(),
            usage: Load::Loading,
            limits: RateLimits::default(),
            _poll: poll,
            _sessions: sessions,
        };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.workspace.clone();
        super::ui::bg(
            cx,
            move || (orbit_core::usage::for_scope(&name), orbit_core::agent_hooks::rate_limits()),
            |t: &mut Self, (usage, limits), _, _| {
                t.usage = usage.map_or_else(Load::Failed, Load::Ready);
                t.limits = limits;
            },
            window,
        );
    }

    /// The Orbit session (and its name) of agent conversation `id`.
    fn tracked(&self, id: &str, cx: &App) -> Option<(Tracked, String)> {
        let store = sessions::store(cx).read(cx);
        if let Some((m, _)) = store.sessions().find(|(m, _)| m.agent_session_id.as_deref() == Some(id)) {
            return Some((Tracked::Open, m.session_name.clone()));
        }
        store.recent.iter().find(|r| r.resume_id == id).map(|r| (Tracked::Recent, r.session_name.clone()))
    }

    fn open_sessions(&self, cx: &App) -> usize {
        sessions::store(cx).read(cx).sessions().filter(|(m, _)| m.workspace == self.workspace).count()
    }

    /// Ends the open session of conversation `id`, or drops it from Recent.
    fn close(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.tracked(&id, cx) {
            Some((Tracked::Open, _)) => {
                self.host.with_app(window, cx, move |a, w, cx| sessions::end(a, |m| m.agent_session_id.as_deref() == Some(id.as_str()), w, cx))
            }
            Some((Tracked::Recent, _)) => sessions::forget_recent(&id, cx),
            None => {}
        }
    }

    fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.open_sessions(cx);
        let (ws, host) = (self.workspace.clone(), self.host.clone());
        self.host.confirm(
            format!("Close all sessions of {ws}?"),
            format!("Ends {}. Claude and OpenCode conversations stay in Recent and can be resumed.", plural(n, "open session")),
            "Close all",
            move |window, cx| {
                let ws = ws.clone();
                host.with_app(window, cx, move |a, w, cx| sessions::end(a, |m| m.workspace == ws, w, cx));
            },
            window,
            cx,
        );
    }

    /// Finds Orbit's files of sessions no longer open and offers to delete them.
    fn clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live = sessions::live_keys(cx);
        super::ui::bg(
            cx,
            move || orbit_core::pty::leftovers(&live),
            |t: &mut Self, files, window, cx| {
                if files.is_empty() {
                    return t.host.toast(ToastKind::Info, "No temporary files to clean", None, cx);
                }
                let total = size(files.iter().map(|f| f.1).sum());
                let host = t.host.clone();
                let freed = total.clone();
                t.host.confirm(
                    format!("Clean {total} of temporary files?"),
                    format!(
                        "Deletes {} Orbit kept for sessions that are no longer open (saved terminal output, agent hook settings). Claude Code's and OpenCode's own conversations are not touched.",
                        plural(files.len(), "file")
                    ),
                    "Clean",
                    move |window, cx| {
                        let (host, freed, files) = (host.clone(), freed.clone(), files.clone());
                        // Spawned: the dialog runs this inside the root view's update, which a toast needs.
                        window
                            .spawn(cx, async move |cx| {
                                let failed = cx.background_spawn(async move { files.iter().filter(|(p, _)| std::fs::remove_file(p).is_err()).count() }).await;
                                let _ = cx.update(|_, cx| match failed {
                                    0 => host.toast(ToastKind::Success, format!("Freed {freed}"), None, cx),
                                    n => host.error("Some files couldn't be deleted", plural(n, "file"), cx),
                                });
                            })
                            .detach();
                    },
                    window,
                    cx,
                );
            },
            window,
        );
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_sessions(cx);
        let close_all = wk::hint(
            wk::disabled(wk::button_icon("usage-close-all", Some(("x", 13.)), "Close all sessions", wk::Kind::Secondary, wk::TOOL), open == 0),
            if open == 0 { "No open session in this workspace".to_string() } else { format!("End the {} of this workspace", plural(open, "open session")) },
        )
        .when(open > 0, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.close_all(w, cx))));
        div()
            .mb(px(20.))
            .flex()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap(px(20.))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(20.25)).line_height(px(28.)).font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(format!("AI usage · {}", self.workspace)))
                    .child(div().text_size(px(13.)).text_color(c(theme::TEXT_FAINT)).child("Tokens and cost of the agent sessions run in this workspace and its repositories.")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        wk::hint(wk::button_icon("usage-clean", Some(("trash", 13.)), "Clean temporary files", wk::Kind::Secondary, wk::TOOL), "Delete the files Orbit kept for sessions that are no longer open")
                            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.clean(w, cx))),
                    )
                    .child(close_all)
                    .child(div().mx(px(4.)).w(px(1.)).h(px(22.)).bg(c(theme::BORDER)))
                    .child(
                        wk::hint(wk::icon_button("usage-refresh", "refresh", 32., 15., false), "Refresh")
                            .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load(w, cx))),
                    ),
            )
    }

    fn limits(&self) -> Option<gpui::Div> {
        let windows: Vec<_> = [("5h", "Session", &self.limits.five_hour), ("7d", "Weekly", &self.limits.seven_day)]
            .into_iter()
            .filter_map(|(l, n, w)| w.as_ref().map(|w| rate(l, n, w)))
            .collect();
        (!windows.is_empty()).then(|| kit::section().child(kit::section_header("Claude subscription", None, None)).child(kit::rows(windows)))
    }

    fn session_row(&self, ix: usize, s: &SessionDetail, cx: &mut Context<Self>) -> gpui::Div {
        let tracked = self.tracked(&s.id, cx);
        let name = title(s, tracked.as_ref().map(|t| t.1.as_str()));
        let sub = match &tracked {
            Some((_, n)) if *n != name => format!("{n} · {}", short_id(&s.id)),
            _ => short_id(&s.id).to_string(),
        };
        let (dot, state) = match tracked.as_ref().map(|t| t.0) {
            Some(Tracked::Open) => (c(theme::OK), "Open"),
            Some(Tracked::Recent) => (c(theme::TEXT_FAINT), "Ended"),
            None => (c(theme::BORDER_STRONG), ""),
        };
        let close = tracked.map(|(t, _)| {
            let id = s.id.clone();
            let tip = if t == Tracked::Open { "End session" } else { "Remove from recent" };
            wk::hint(wk::icon_button(SharedString::from(format!("usage-close:{ix}")), "x", 24., 12., true), tip)
                .on_click(cx.listener(move |p, _: &ClickEvent, w, cx| p.close(id.clone(), w, cx)))
        });
        let cell = |w: f32, color: u32, text: String| div().flex_none().w(px(w)).text_right().text_size(px(12.)).text_color(c(color)).child(text);
        div()
            .px(px(16.))
            .py(px(10.))
            .flex()
            .items_center()
            .gap(px(14.))
            .child(wk::dot(dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT)).child(name))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .text_size(px(11.5))
                            .text_color(c(theme::TEXT_FAINT))
                            .when(!state.is_empty(), |d| d.child(div().text_color(c(theme::TEXT_DIM)).child(state)).child("·"))
                            .child(div().font_family(theme::MONO_FONT).child(sub)),
                    ),
            )
            .child(cell(170., theme::TEXT_DIM, format!("{} in · {} out", tokens(s.input_tokens), tokens(s.output_tokens))))
            .child(cell(64., theme::TEXT, format!("${:.2}", s.cost_usd)))
            .child(cell(64., theme::TEXT_FAINT, super::ui::ago(s.last_active)))
            .child(div().flex_none().w(px(24.)).children(close))
    }
}

fn rate(label: &str, name: &str, w: &RateWindow) -> gpui::Div {
    let pct = w.used_percentage.clamp(0., 100.);
    let color = if pct >= 90. {
        theme::DANGER
    } else if pct >= 80. {
        theme::WARN
    } else {
        theme::ACCENT
    };
    div()
        .px(px(16.))
        .py(px(12.))
        .flex()
        .items_center()
        .gap(px(14.))
        .text_size(px(12.5))
        .child(div().w(px(110.)).text_color(c(theme::TEXT)).child(format!("{name} ({label})")))
        .child(div().flex_1().h(px(6.)).rounded_full().bg(c(theme::PANEL_HOVER)).child(div().h_full().rounded_full().w(gpui::relative(pct as f32 / 100.)).bg(c(color))))
        .child(div().w(px(190.)).text_right().text_color(c(theme::TEXT_DIM)).child(format!("{pct:.0}% used · resets in {}", until(w.resets_at))))
}

fn stat(label: &str, value: String) -> gpui::Div {
    div()
        .flex_1()
        .px(px(16.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(label.to_uppercase()))
        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(value))
}

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().w_full().max_w(px(1080.)).mx_auto().flex().flex_col().gap(px(22.)).child(self.header(cx));
        let page = |body| div().id("usage-body").size_full().overflow_y_scroll().px(px(44.)).py(px(28.)).child(body);
        let usage = match &self.usage {
            Load::Ready(u) => u,
            Load::Failed(e) => return page(body.child(kit::error_box(e.clone()))),
            _ => return page(body.child(kit::card().child(crate::motion::skeleton_table(4, 4)))),
        };
        if usage.by_model.is_empty() {
            body = body.children(self.limits()).child(
                kit::card().py(px(40.)).items_center().gap(px(8.)).child(kit::icon("pulse", 22., c(theme::TEXT_FAINT))).child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child("No AI usage yet")).child(kit::hint("Agent sessions run in this workspace and its repositories show up here, with their tokens and cost.")),
            );
            return page(body);
        }
        let sum = |f: fn(&orbit_core::usage::ModelUsage) -> u64| usage.by_model.iter().map(f).sum::<u64>();
        let cost: f64 = usage.by_model.iter().map(|m| m.cost_usd).sum();
        let summary = kit::card().flex_row().child(stat("Sessions", usage.sessions.to_string())).child(stat("Cost", format!("${cost:.2}"))).child(stat("Input", tokens(sum(|m| m.input_tokens)))).child(stat("Output", tokens(sum(|m| m.output_tokens)))).child(stat("Cache", tokens(sum(|m| m.cache_tokens))));
        body = body.child(summary).children(self.limits());
        let mut ix = 0;
        for m in &usage.by_model {
            let totals = div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(format!("${:.2} · {} in · {} out", m.cost_usd, tokens(m.input_tokens), tokens(m.output_tokens)));
            let rows: Vec<_> = m.sessions.iter().map(|s| {
                ix += 1;
                self.session_row(ix, s, cx)
            }).collect();
            body = body.child(kit::section().child(kit::section_header(&m.model, Some(m.sessions.len()), Some(totals))).child(kit::rows(rows)));
        }
        page(body)
    }
}

impl PageView for UsagePage {
    fn title(&self) -> SharedString {
        format!("AI usage · {}", self.workspace).into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    fn dump(&self, out: &mut String, cx: &App) {
        match &self.usage {
            Load::Ready(u) => {
                let _ = writeln!(out, "  sessions {}", u.sessions);
                for m in &u.by_model {
                    let _ = writeln!(
                        out,
                        "  model {} {} in / {} out / {} cache · ${:.2} · {} sessions",
                        m.model,
                        tokens(m.input_tokens),
                        tokens(m.output_tokens),
                        tokens(m.cache_tokens),
                        m.cost_usd,
                        m.sessions.len()
                    );
                    for s in &m.sessions {
                        let t = self.tracked(&s.id, cx);
                        let state = match t.as_ref().map(|t| t.0) {
                            Some(Tracked::Open) => "open",
                            Some(Tracked::Recent) => "recent",
                            None => "-",
                        };
                        let _ = writeln!(out, "    session {} [{state}] {}", short_id(&s.id), title(s, t.as_ref().map(|t| t.1.as_str())));
                    }
                }
            }
            other => {
                let _ = writeln!(out, "  usage: {}", super::ui::load_label(other));
            }
        }
        let _ = writeln!(out, "  open sessions: {}", self.open_sessions(cx));
        let _ = writeln!(out, "  limits: 5h {:?} · 7d {:?}", self.limits.five_hour.as_ref().map(|w| w.used_percentage), self.limits.seven_day.as_ref().map(|w| w.used_percentage));
    }
}

#[cfg(test)]
mod tests {
    use orbit_core::usage::SessionDetail;

    #[test]
    fn formats_tokens_like_the_usage_bar() {
        assert_eq!(super::tokens(999), "999");
        assert_eq!(super::tokens(1_500), "1.5k");
        assert_eq!(super::tokens(2_300_000), "2.3M");
        assert_eq!(super::tokens(4_000_000_000), "4.0B");
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(super::size(512), "512 B");
        assert_eq!(super::size(2048), "2 KB");
        assert_eq!(super::size(5 * 1_048_576), "5.0 MB");
    }

    #[test]
    fn untitled_sessions_get_a_readable_name() {
        let id = "3f2a9c1e-1111-4a2b-9c3d-aaaaaaaaaaaa";
        let s = |title: &str| SessionDetail { id: id.into(), title: title.into(), ..Default::default() };
        assert_eq!(super::title(&s("Fix the login"), Some("swift-otter")), "Fix the login");
        assert_eq!(super::title(&s(id), Some("swift-otter")), "swift-otter");
        assert_eq!(super::title(&s(id), None), "Session 3f2a9c1e");
    }
}
