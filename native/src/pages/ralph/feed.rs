//! The activity feed (`RalphView`'s `IterationBlock` / `FeedLine`,
//! `lib/ralphFeed.ts`) and the words for runs and stop reasons.
use super::super::kit;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, FontWeight, HighlightStyle, SharedString, StyledText};
use orbit_core::ralph::Activity;
use serde_json::Value;
use std::ops::Range;
use std::sync::OnceLock;

/// One feed line in plain words (the self-test dump).
pub fn describe(ev: &Value) -> String {
    let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
    match ev["kind"].as_str().unwrap_or("") {
        "run_start" => format!("▶ run started · {} · {}", s("agent"), s("model")),
        "model" => format!("model {}", s("model")),
        "iteration_start" => match ev["story"].as_object() {
            Some(st) => format!("── iteration {} · {} {}", ev["n"], st["id"].as_str().unwrap_or(""), st["title"].as_str().unwrap_or("")),
            None => format!("── iteration {}", ev["n"]),
        },
        "text" => s("text"),
        "tool" => format!("⚙ {} {}", s("name"), s("summary")),
        "log" => format!("{}{}", if ev["stderr"].as_bool() == Some(true) { "stderr: " } else { "" }, s("text")),
        "story_passed" => format!("✓ {} passes", s("id")),
        "iteration_end" => format!(
            "iteration {} {} · {} commits · {}/{} done{}",
            ev["n"],
            s("outcome"),
            ev["commits"].as_u64().unwrap_or(0),
            ev["passed"].as_u64().unwrap_or(0),
            ev["total"].as_u64().unwrap_or(0),
            ev["costUsd"].as_f64().map(|c| format!(" · ${c:.2}")).unwrap_or_default()
        ),
        "push" if ev["ok"].as_bool() == Some(true) => "pushed".into(),
        "push" => format!("push failed: {}", s("message")),
        "limit_wait" => format!("usage limit: waiting until {}", ev["until"]),
        "iteration_idle" | "iteration_exit_grace" | "iteration_timeout" | "reconcile" => watchdog_text(ev),
        "stopped" => format!("■ stopped: {} after {} iterations", reason(&ev["reason"]), ev["iterations"]),
        other => format!("{other} {ev}"),
    }
}

/// The watchdog's feed lines (`FeedLine`): why an iteration was killed and
/// whether its story still counted.
fn watchdog_text(ev: &Value) -> String {
    let id = ev["id"].as_str().unwrap_or("");
    match ev["kind"].as_str().unwrap_or("") {
        "iteration_idle" => format!("No agent output for {} min: killed the iteration's processes", ev["idleMin"]),
        "iteration_exit_grace" => format!("The agent finished but didn't exit: killed its processes after {}s", ev["graceSec"]),
        "iteration_timeout" => format!("Iteration hit its {} min limit: killed its processes", ev["timeoutMin"]),
        _ if ev["passed"].as_bool() == Some(true) => format!("{id} was committed cleanly: counted as passed"),
        _ => {
            let mut why = Vec::new();
            if ev["committed"].as_bool() != Some(true) {
                why.push(format!("no \"feat: [{id}]\" commit"));
            }
            if ev["clean"].as_bool() != Some(true) {
                why.push("uncommitted changes".into());
            }
            if ev["messageOk"].as_bool() != Some(true) {
                why.push("the agent reported a failure".into());
            }
            format!("{id} not counted: {}", why.join(", "))
        }
    }
}

/// An iteration outcome as shown (`outcomeLabel`): "Failed(..)" → "Failed".
pub fn outcome_label(outcome: &str) -> String {
    match outcome.split('(').next().unwrap_or("").trim() {
        "Recovered" => "passed (recovered)".into(),
        other => other.to_string(),
    }
}

/// Seconds since the agent's last output, and whether that's past half the
/// idle timeout (`idleness`).
pub fn idleness(last_event_at: Option<u64>, now: u64, idle_min: f64) -> (u64, bool) {
    let secs = last_event_at.map(|t| now.saturating_sub(t)).unwrap_or(0);
    (secs, idle_min > 0. && secs as f64 >= idle_min * 60. / 2.)
}

/// "12s", "4m", "1h 5m" (`formatAgo`).
pub fn ago(secs: u64) -> String {
    match secs / 60 {
        0 => format!("{secs}s"),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

/// A `StopReason` (`{kind, detail}`) in short words.
pub fn reason(v: &Value) -> String {
    let detail = v["detail"].as_str().map(|d| format!(" ({d})")).unwrap_or_default();
    let kind = match v["kind"].as_str().unwrap_or("") {
        "complete" => "complete",
        "storyReached" => "reached the story",
        "maxIterations" => "max iterations",
        "stalled" => "stalled (no commits)",
        "limit" => "usage limit",
        "deadline" => "time budget",
        "failed" => "failed",
        "blocked" => "blocked",
        "stopped" => "stopped",
        "paused" => "paused",
        other => other,
    };
    format!("{kind}{detail}")
}

/// `reasonLabel` in `types/ralph.ts`.
pub fn reason_label(v: &Value) -> String {
    let detail = v["detail"].as_str().unwrap_or("");
    match v["kind"].as_str().unwrap_or("") {
        "complete" => "All stories pass".into(),
        "storyReached" => format!("Reached {detail}"),
        "maxIterations" => "Iteration limit reached".into(),
        "stalled" => "Stalled: no commits in the last iterations".into(),
        "limit" => "Stopped on usage limit".into(),
        "deadline" => "Time budget used".into(),
        "failed" => format!("Failed: {detail}"),
        "blocked" => format!("Blocked, needs your decision. {detail}"),
        "stopped" => "Stopped".into(),
        "paused" => "Paused".into(),
        _ => String::new(),
    }
}

/// `formatDuration` in `lib/ralphFeed.ts`.
pub fn duration(secs: u64) -> String {
    let m = secs / 60;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{m}m {}s", secs % 60),
        _ => format!("{}h {}m", m / 60, m % 60),
    }
}

/// The local UTC offset in seconds (`date +%z`, read once).
fn utc_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let z = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let digits: String = z.chars().filter(|c| c.is_ascii_digit()).collect();
        let num = |r: std::ops::Range<usize>| digits.get(r).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        sign * (num(0..2) * 3600 + num(2..4) * 60)
    })
}

/// "04/11/2026, 01:44" in local time (the run list's short date and time).
pub fn local_time(unix: i64) -> String {
    let t = unix + utc_offset();
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d:02}/{m:02}/{y}, {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// "13:05" in local time.
fn local_hm(unix: i64) -> String {
    local_time(unix).rsplit(", ").next().unwrap_or("").to_string()
}

/// One iteration of the feed (`groupByIteration`).
pub struct Group<'a> {
    pub n: u64,
    pub story: Option<(String, String)>,
    pub events: Vec<&'a Value>,
    pub end: Option<&'a Value>,
}

pub fn group_by_iteration(events: &[Value]) -> Vec<Group<'_>> {
    let mut groups = vec![];
    let mut cur = Group { n: 0, story: None, events: vec![], end: None };
    for ev in events {
        match ev["kind"].as_str() {
            Some("iteration_start") => {
                if !cur.events.is_empty() || cur.n > 0 {
                    groups.push(cur);
                }
                let story = ev["story"].as_object().map(|s| (s["id"].as_str().unwrap_or("").to_string(), s["title"].as_str().unwrap_or("").to_string()));
                cur = Group { n: ev["n"].as_u64().unwrap_or(0), story, events: vec![], end: None };
            }
            Some("iteration_end") => cur.end = Some(ev),
            _ => cur.events.push(ev),
        }
    }
    if !cur.events.is_empty() || cur.n > 0 {
        groups.push(cur);
    }
    groups
}

/// `.rv-story-id`.
pub fn story_id(id: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(c(theme::BORDER))
        .font_family(theme::MONO_FONT)
        .text_size(px(10.5))
        .line_height(px(14.))
        .text_color(c(theme::TEXT_FAINT))
        .child(id.into())
}

/// `.rv-story-repo`.
pub fn repo_tag(repo: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.))
        .rounded(px(4.))
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::PANEL_RAISED))
        .font_family(theme::MONO_FONT)
        .text_size(px(10.5))
        .line_height(px(16.))
        .text_color(c(theme::TEXT_DIM))
        .child(repo.into())
}

/// `**bold**` and `` `code` `` markers stripped: the text and its runs
/// (true: bold, false: code). Agent output is shown as text, nothing more.
pub fn parse_inline(text: &str) -> (String, Vec<(Range<usize>, bool)>) {
    let mut out = String::new();
    let mut runs = vec![];
    let mut rest = text;
    while !rest.is_empty() {
        let (at, mark) = match (rest.find("**"), rest.find('`')) {
            (Some(b), Some(k)) if k < b => (k, "`"),
            (Some(b), _) => (b, "**"),
            (None, Some(k)) => (k, "`"),
            (None, None) => break,
        };
        let inner = &rest[at + mark.len()..];
        let Some(close) = inner.find(mark).filter(|&e| e > 0) else { break };
        out.push_str(&rest[..at]);
        let start = out.len();
        out.push_str(&inner[..close]);
        runs.push((start..out.len(), mark == "**"));
        rest = &inner[close + mark.len()..];
    }
    out.push_str(rest);
    (out, runs)
}

/// `inlineMarkdown`: the parsed runs as bold or code-tinted text.
fn inline_markdown(text: &str) -> StyledText {
    let (text, runs) = parse_inline(text);
    let runs = runs.into_iter().map(|(r, bold)| {
        let style = if bold {
            HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() }
        } else {
            HighlightStyle { background_color: Some(c(theme::PANEL_RAISED).into()), ..Default::default() }
        };
        (r, style)
    });
    StyledText::new(text).with_highlights(runs)
}

/// A feed line (`FeedLine`).
pub fn feed_line(ev: &Value) -> Option<gpui::AnyElement> {
    let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
    let event = |text: String, color: u32, bold: bool| {
        div()
            .my(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(c(color))
            .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
            .child(text)
            .into_any_element()
    };
    Some(match ev["kind"].as_str().unwrap_or("") {
        "tool" => {
            let name = s("name");
            let (bg, fg) = match name.as_str() {
                "Read" => (kit::alpha(theme::ACCENT, 0.14), c(theme::INFO)),
                "Grep" | "Glob" | "WebSearch" | "WebFetch" | "List" | "Webfetch" | "Websearch" => (kit::alpha(theme::PURPLE, 0.14), kit::mix(theme::PURPLE, theme::TEXT, 0.6)),
                "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Patch" => (kit::alpha(theme::WARN, 0.14), c(theme::WARN)),
                "Bash" => (kit::alpha(theme::OK, 0.12), kit::mix(theme::OK, theme::TEXT, 0.7)),
                _ => (c(theme::PANEL_RAISED), c(theme::TEXT_DIM)),
            };
            div()
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().w(px(54.)).flex_none().py(px(2.)).rounded(px(4.)).bg(bg).flex().justify_center().text_size(px(10.)).font_weight(FontWeight::BOLD).text_color(fg).child(name))
                .child(div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(11.5)).text_color(c(theme::TEXT_DIM)).child(s("summary")))
                .into_any_element()
        }
        "thinking" => div().my(px(4.)).text_size(px(12.)).italic().line_clamp(2).text_color(c(theme::TEXT_FAINT)).child(s("text").split_whitespace().collect::<Vec<_>>().join(" ")).into_any_element(),
        "text" => {
            let text = s("text").replace("<promise>COMPLETE</promise>", "").replace("<task-done/>", "").trim().to_string();
            if text.is_empty() {
                return None;
            }
            div()
                .my(px(6.))
                .pl(px(12.))
                .py(px(2.))
                .border_l_2()
                .border_color(c(theme::BORDER_STRONG))
                .line_height(px(18.75))
                .text_color(c(theme::TEXT))
                .child(inline_markdown(&text))
                .into_any_element()
        }
        "log" => div()
            .font_family(theme::MONO_FONT)
            .text_size(px(11.))
            .text_color(c(if ev["stderr"].as_bool() == Some(true) { theme::DANGER } else { theme::TEXT_FAINT }))
            .child(s("text"))
            .into_any_element(),
        "story_passed" => div()
            .my(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(theme::OK))
            .child(kit::icon("check", 12., c(theme::OK)))
            .child(format!("{} passes", s("id")))
            .into_any_element(),
        "push" if ev["ok"].as_bool() == Some(true) => {
            event(format!("↑ Pushed {}to origin", ev["repo"].as_str().map(|r| format!("{r} ")).unwrap_or_default()), theme::TEXT_FAINT, false)
        }
        "push" => event(format!("Push failed{}: {}", ev["repo"].as_str().map(|r| format!(" in {r}")).unwrap_or_default(), s("message")), theme::DANGER, false),
        "limit_wait" => event(format!("Usage limit reached · retrying at {}", local_hm(ev["until"].as_i64().unwrap_or(0))), theme::WARN, false),
        "run_start" => event(format!("Run started with {} · {}", s("agent"), s("model")), theme::TEXT_FAINT, false),
        "model" => event(format!("Agent is running {}", s("model")), theme::TEXT_FAINT, false),
        "iteration_idle" | "iteration_exit_grace" | "iteration_timeout" => event(watchdog_text(ev), theme::WARN, false),
        "reconcile" if ev["passed"].as_bool() == Some(true) => div()
            .my(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(theme::OK))
            .child(kit::icon("check", 12., c(theme::OK)))
            .child(watchdog_text(ev))
            .into_any_element(),
        "reconcile" => event(watchdog_text(ev), theme::DANGER, false),
        "stopped" => {
            let n = ev["iterations"].as_u64().unwrap_or(0);
            let ok = ev["reason"]["kind"].as_str() == Some("complete");
            event(format!("{} · {n} iteration{}", reason_label(&ev["reason"]), if n == 1 { "" } else { "s" }), if ok { theme::OK } else { theme::TEXT_FAINT }, ok)
        }
        _ => return None,
    })
}

/// `IterationBlock`: the iteration's head (story, outcome, commits, time,
/// cost) and its lines. A live one shows the time since the agent's last
/// output (yellow from half the idle timeout) and its running commands.
pub fn iteration_block(g: &Group, live: bool, activity: Option<&Activity>, idle_min: f64, now: u64) -> gpui::Div {
    let activity = activity.filter(|a| live && g.end.is_none() && a.last_event_at.is_some());
    let (idle_secs, warn) = idleness(activity.and_then(|a| a.last_event_at), now, idle_min);
    let mut block = div().pt(px(12.));
    if g.n > 0 {
        let story: gpui::AnyElement = match &g.story {
            Some((id, title)) => div().flex().items_center().gap(px(6.)).min_w_0().child(story_id(id.clone())).child(div().truncate().child(title.clone())).into_any_element(),
            None => div().child("Iteration").into_any_element(),
        };
        let meta = match g.end {
            Some(end) => {
                let outcome = end["outcome"].as_str().map(outcome_label).filter(|o| !o.is_empty() && o != "Done" && o != "Complete");
                let commits = end["commits"].as_u64().unwrap_or(0);
                let mut text = format!("{commits} commit{}", if commits == 1 { "" } else { "s" });
                if let Some(ms) = end["durationMs"].as_u64().filter(|ms| *ms > 0) {
                    text += &format!(" · {}", duration(ms / 1000));
                }
                if let Some(cost) = end["costUsd"].as_f64().filter(|c| *c > 0.) {
                    text += &format!(" · ${cost:.2}");
                }
                let color = |o: &str| if o == "passed (recovered)" { theme::OK } else { theme::DANGER };
                Some(div().flex().items_center().gap(px(6.)).children(outcome.map(|o| div().font_weight(FontWeight::SEMIBOLD).text_color(c(color(&o))).child(o))).child(text))
            }
            None if live => {
                let label = if activity.is_some() { format!("last event {} ago", ago(idle_secs)) } else { "working".into() };
                Some(div().flex().items_center().gap(px(6.)).when(warn, |d| d.text_color(c(theme::WARN))).child(crate::workspace_ui::kit::spinner(("ralph-iter", g.n as usize), 11., c(theme::ACCENT))).child(label))
            }
            None => None,
        };
        block = block.child(
            div()
                .py(px(8.))
                .mb(px(6.))
                .flex()
                .items_center()
                .gap(px(10.))
                .border_b_1()
                .border_color(c(theme::BORDER))
                .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(c(theme::ACCENT)).child(format!("#{}", g.n)))
                .child(div().flex_1().min_w_0().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(story))
                .children(meta.map(|m| m.flex_none().text_size(px(11.)).text_color(c(if warn { theme::WARN } else { theme::TEXT_FAINT })))),
        );
        if let Some(a) = activity {
            let lines = a.tool.iter().map(|t| format!("▸ {t}")).chain(a.background.iter().map(|b| format!("⟳ background: {b}")));
            block = block.child(
                div()
                    .mb(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .font_family(theme::MONO_FONT)
                    .text_size(px(11.))
                    .text_color(c(if warn { theme::WARN } else { theme::TEXT_FAINT }))
                    .children(lines.map(|l| div().truncate().child(l))),
            );
        }
    }
    block.child(div().flex().flex_col().gap(px(2.)).children(g.events.iter().filter_map(|e| feed_line(e))))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn formats_durations_and_groups_iterations() {
        assert_eq!(super::duration(59), "59s");
        assert_eq!(super::duration(125), "2m 5s");
        assert_eq!(super::duration(7 * 3600 + 19 * 60), "7h 19m");
        let events = vec![
            json!({"kind": "run_start"}),
            json!({"kind": "iteration_start", "n": 1, "story": {"id": "T1", "title": "A"}}),
            json!({"kind": "tool", "name": "Read"}),
            json!({"kind": "iteration_end", "n": 1, "commits": 1}),
        ];
        let groups = super::group_by_iteration(&events);
        assert_eq!(groups.len(), 2);
        assert_eq!((groups[1].n, groups[1].events.len(), groups[1].end.is_some()), (1, 1, true));
    }

    #[test]
    fn describes_feed_events() {
        let d = super::describe;
        assert_eq!(d(&json!({"kind": "iteration_start", "n": 2, "story": {"id": "US-002", "title": "Login"}})), "── iteration 2 · US-002 Login");
        assert_eq!(d(&json!({"kind": "tool", "name": "Edit", "summary": "src/a.rs"})), "⚙ Edit src/a.rs");
        assert_eq!(d(&json!({"kind": "story_passed", "id": "US-1"})), "✓ US-1 passes");
        assert_eq!(
            d(&json!({"kind": "stopped", "reason": {"kind": "maxIterations"}, "iterations": 3})),
            "■ stopped: max iterations after 3 iterations"
        );
        assert_eq!(d(&json!({"kind": "iteration_end", "n": 1, "outcome": "Done", "commits": 1, "passed": 1, "total": 2, "costUsd": 0.5})), "iteration 1 Done · 1 commits · 1/2 done · $0.50");
    }

    #[test]
    fn idle_warning_and_watchdog_lines() {
        assert_eq!(super::idleness(Some(1000), 1000 + 420, 15.), (420, false));
        assert_eq!(super::idleness(Some(1000), 1000 + 450, 15.), (450, true));
        assert_eq!(super::idleness(Some(1000), 5000, 0.), (4000, false));
        assert_eq!((super::ago(42), super::ago(485), super::ago(3900)), ("42s".into(), "8m".into(), "1h 5m".into()));
        assert_eq!(super::outcome_label("Recovered"), "passed (recovered)");
        assert_eq!(super::outcome_label("Failed(\"x\")"), "Failed");
        let d = super::describe;
        assert_eq!(d(&json!({"kind": "iteration_idle", "n": 1, "idleMin": 15.0})), "No agent output for 15.0 min: killed the iteration's processes");
        assert_eq!(d(&json!({"kind": "reconcile", "id": "T1", "committed": true, "clean": false, "messageOk": true, "passed": false})), "T1 not counted: uncommitted changes");
    }

    #[test]
    fn strips_inline_markdown() {
        let (text, runs) = super::parse_inline("Implementing **US-001** in `impl.txt`, a*b");
        assert_eq!(text, "Implementing US-001 in impl.txt, a*b");
        assert_eq!(runs, vec![(13..19, true), (23..31, false)]);
    }
}
