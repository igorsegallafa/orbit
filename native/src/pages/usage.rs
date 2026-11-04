//! AI usage of a workspace (`UsageBar.tsx`): tokens and cost per model with
//! its sessions, from Claude Code's logs and OpenCode's database, and the
//! Claude subscription windows. Refreshed every 30 s, as the bar is.
use super::ui::{self, Tone};
use super::{Host, PageView};
use crate::review::Load;
use crate::theme::{self, c};
use crate::workspace::Workspace;
use gpui::{div, prelude::*, px, App, ClickEvent, Context, SharedString, Task, Window};
use orbit_core::agent_hooks::{RateLimits, RateWindow};
use orbit_core::usage::AiUsage;
use std::fmt::Write as _;
use std::time::Duration;

pub struct UsagePage {
    _host: Host,
    workspace: String,
    usage: Load<AiUsage>,
    limits: RateLimits,
    /// The model whose sessions are listed.
    open: Option<String>,
    _poll: Task<()>,
}

pub fn tokens(n: u64) -> String {
    match n {
        1_000_000_000.. => format!("{:.1}B", n as f64 / 1e9),
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}k", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

/// "2h 5m" until a unix time.
fn until(unix: u64) -> String {
    let m = unix.saturating_sub(ui::now()).div_ceil(60);
    match m {
        0..60 => format!("{m}m"),
        60..1440 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d {}h", m / 1440, (m % 1440) / 60),
    }
}

impl UsagePage {
    pub fn new(host: Host, ws: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            if this.update_in(cx, |t: &mut Self, w, cx| t.load(w, cx)).is_err() {
                break;
            }
        });
        let mut page =
            Self { _host: host, workspace: ws.meta.name.clone(), usage: Load::Loading, limits: RateLimits::default(), open: None, _poll: poll };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.workspace.clone();
        ui::bg(
            cx,
            move || (orbit_core::usage::for_scope(&name), orbit_core::agent_hooks::rate_limits()),
            |t: &mut Self, (usage, limits), _, _| {
                t.usage = usage.map_or_else(Load::Failed, Load::Ready);
                t.limits = limits;
            },
            window,
        );
    }

    fn rate(&self, label: &str, name: &str, w: &RateWindow) -> gpui::Div {
        let pct = w.used_percentage.clamp(0., 100.);
        let color = if pct >= 90. {
            theme::DANGER
        } else if pct >= 80. {
            theme::WARN
        } else {
            theme::ACCENT
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(c(theme::TEXT_DIM))
            .child(format!("{label} ({name})"))
            .child(div().w(px(120.)).h(px(6.)).rounded_full().bg(c(theme::PANEL_HOVER)).child(div().h_full().rounded_full().w(px(1.2 * pct as f32)).bg(c(color))))
            .child(format!("{pct:.0}% · resets in {}", until(w.resets_at)))
    }
}

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = ui::page("usage-body", format!("AI usage · {}", self.workspace), "Tokens and cost of the agent sessions run in this workspace and its repositories.");
        if self.limits.five_hour.is_some() || self.limits.seven_day.is_some() {
            let mut limits = ui::card().p_3().flex().flex_col().gap_1().child(ui::heading("Claude subscription"));
            if let Some(w) = &self.limits.five_hour {
                limits = limits.child(self.rate("5h", "session", w));
            }
            if let Some(w) = &self.limits.seven_day {
                limits = limits.child(self.rate("7d", "weekly", w));
            }
            page = page.child(limits);
        }
        let usage = match &self.usage {
            Load::Ready(u) => u,
            Load::Failed(e) => return page.child(ui::error(e.clone())),
            _ => return page.child(ui::card().child(crate::motion::skeleton_table(4, 4))),
        };
        if usage.by_model.is_empty() {
            return page.child(ui::muted("No AI usage in this workspace yet."));
        }
        let total: f64 = usage.by_model.iter().map(|m| m.cost_usd).sum();
        page = page.child(ui::muted(format!("{} sessions · ${total:.2}", usage.sessions)));
        let mut card = ui::card();
        for m in &usage.by_model {
            let open = self.open.as_deref() == Some(m.model.as_str());
            let model = m.model.clone();
            card = card.child(
                ui::row(SharedString::from(format!("usage:{}", m.model)))
                    .mx_2()
                    .my_1()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_sm()
                    .child(div().flex_1().text_color(c(theme::TEXT)).child(m.model.clone()))
                    .child(ui::muted(format!("{} in / {} out · {} cache", tokens(m.input_tokens), tokens(m.output_tokens), tokens(m.cache_tokens))))
                    .child(div().w(px(70.)).text_right().text_color(c(theme::TEXT)).child(format!("${:.2}", m.cost_usd)))
                    .child(ui::faint(format!("{} sessions {}", m.sessions.len(), if open { "▾" } else { "▸" })))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        t.open = if t.open.as_deref() == Some(model.as_str()) { None } else { Some(model.clone()) };
                        cx.notify();
                    })),
            );
            if open {
                for s in &m.sessions {
                    card = card.child(
                        div()
                            .pl_8()
                            .pr_4()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_xs()
                            .child(div().flex_1().min_w_0().truncate().text_color(c(theme::TEXT_DIM)).child(s.title.clone()))
                            .child(ui::faint(format!("{} in / {} out", tokens(s.input_tokens), tokens(s.output_tokens))))
                            .child(ui::faint(format!("${:.2}", s.cost_usd)))
                            .child(ui::faint(ui::ago(s.last_active))),
                    );
                }
            }
        }
        page.child(card).child(div().child(ui::button("usage-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load(w, cx)))))
    }
}

impl PageView for UsagePage {
    fn title(&self) -> SharedString {
        format!("AI usage · {}", self.workspace).into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    fn dump(&self, out: &mut String, _: &App) {
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
                }
            }
            other => {
                let _ = writeln!(out, "  usage: {}", ui::load_label(other));
            }
        }
        let _ = writeln!(out, "  limits: 5h {:?} · 7d {:?}", self.limits.five_hour.as_ref().map(|w| w.used_percentage), self.limits.seven_day.as_ref().map(|w| w.used_percentage));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_tokens_like_the_usage_bar() {
        assert_eq!(super::tokens(999), "999");
        assert_eq!(super::tokens(1_500), "1.5k");
        assert_eq!(super::tokens(2_300_000), "2.3M");
        assert_eq!(super::tokens(4_000_000_000), "4.0B");
    }
}
