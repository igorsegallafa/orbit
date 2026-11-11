// One agent run of an iteration, the same for repo PRD and plan runs: its
// raw log, the live activity and feed events, and what it printed.
use super::supervisor::{self, Iteration};
use super::watchdog::{self, Tracker};
use super::{iteration_timeout, now, record, record_model, relativize, runs, snapshot, Activity, RunConfig};
use crate::agent::runner::{self, Line, Outcome, Report};
use crate::agent::stream::{self, Event};
use crate::events::Events;
use serde_json::json;
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;

/// The activity the UI shows, emitted on `ralph-activity` when it changes
/// or every few seconds.
struct Live {
    app: Events,
    key: String,
    root: String,
    tracker: Tracker,
    emitted_at: u64,
}

impl Live {
    fn line(&mut self, text: &str) {
        let root = &self.root;
        let changed = self.tracker.line(text, |s| relativize(s, root));
        let t = now();
        let activity = Activity { last_event_at: Some(t), tool: self.tracker.tool(), background: self.tracker.background() };
        if let Some(st) = runs().lock().unwrap().get_mut(&self.key) {
            st.activity = activity.clone();
        }
        if changed || t >= self.emitted_at + 5 {
            self.emitted_at = t;
            let _ = self.app.emit("ralph-activity", json!({ "key": self.key, "activity": activity }));
        }
    }
}

/// The agent's closing event.
struct Closing {
    text: String,
    is_error: bool,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
    turns: Option<u64>,
    tokens: Option<u64>,
}

/// What the agent printed.
#[derive(Default)]
pub struct AgentRun {
    result: Option<Closing>,
    /// Its messages, and stray non-JSON stdout lines.
    texts: Vec<String>,
    step_cost: f64,
    step_tokens: u64,
    /// The last lines, stdout and stderr.
    tail: VecDeque<String>,
}

/// Runs `cmd` in `dir` as iteration `n` of the run `key`, under `cfg`'s stop
/// rules; `done` tells the runner the iteration's work is in.
pub fn run(app: &Events, key: &str, n: u32, cmd: std::process::Command, dir: &Path, cfg: &RunConfig, done: impl FnMut() -> bool) -> (Result<Outcome, String>, AgentRun) {
    let run_dir = snapshot(key).map(|s| s.dir).unwrap_or_default();
    let mut raw_log = std::fs::File::create(run_dir.join(format!("iter-{n}.log"))).ok();
    let root = dir.to_string_lossy().to_string();
    let mut live = Live { app: app.clone(), key: key.to_string(), root: root.clone(), tracker: Tracker::default(), emitted_at: 0 };
    let mut out = AgentRun::default();

    let outcome = runner::run_watched(
        cmd,
        dir,
        iteration_timeout(cfg),
        Some(&format!("ralph:{key}")),
        watchdog::watch(cfg),
        |line| {
            let (text, is_err) = match &line {
                Line::Out(l) => (l.as_str(), false),
                Line::Err(l) => (l.as_str(), true),
            };
            if let Some(f) = raw_log.as_mut() {
                let _ = writeln!(f, "{}{text}", if is_err { "[stderr] " } else { "" });
            }
            out.tail.push_back(text.to_string());
            if out.tail.len() > 10 {
                out.tail.pop_front();
            }
            live.line(text);
            let busy = live.tracker.tool().is_some();
            let events = if is_err { vec![] } else { stream::parse_line(text) };
            if events.is_empty() && !text.trim_start().starts_with('{') && !text.trim().is_empty() {
                if !is_err {
                    out.texts.push(text.to_string());
                }
                record(app, key, json!({"kind": "log", "text": text, "stderr": is_err}), |_| {});
            }
            let mut finished = false;
            for e in events {
                match e {
                    Event::Result { text, is_error, cost_usd, duration_ms, turns, tokens } => {
                        finished = true;
                        out.result = Some(Closing { text, is_error, cost_usd, duration_ms, turns, tokens });
                    }
                    Event::Text { text } => {
                        record(app, key, json!({"kind": "text", "text": text}), |_| {});
                        out.texts.push(text);
                    }
                    Event::Thinking { text } => record(app, key, json!({"kind": "thinking", "text": text}), |_| {}),
                    Event::Tool { name, summary } => {
                        let summary = relativize(&summary, &root);
                        record(app, key, json!({"kind": "tool", "name": name, "summary": summary}), |_| {})
                    }
                    Event::Cost { usd, tokens } => {
                        out.step_cost += usd;
                        out.step_tokens += tokens;
                    }
                    Event::Model { name } => record_model(app, key, &name),
                    Event::Session { .. } => {}
                }
            }
            Report { finished, busy }
        },
        done,
    );
    (outcome, out)
}

impl AgentRun {
    fn result_text(&self) -> &str {
        self.result.as_ref().map(|r| r.text.as_str()).unwrap_or("")
    }

    pub fn is_error(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.is_error)
    }

    pub fn cost(&self) -> Option<f64> {
        self.result.as_ref().and_then(|r| r.cost_usd).or((self.step_cost > 0.0).then_some(self.step_cost))
    }

    pub fn tokens(&self) -> Option<u64> {
        self.result.as_ref().and_then(|r| r.tokens).or((self.step_tokens > 0).then_some(self.step_tokens))
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.result.as_ref().and_then(|r| r.duration_ms)
    }

    pub fn turns(&self) -> Option<u64> {
        self.result.as_ref().and_then(|r| r.turns)
    }

    /// Its closing text, else every message.
    pub fn full(&self) -> String {
        if self.result_text().trim().is_empty() { self.texts.join("\n") } else { self.result_text().to_string() }
    }

    /// Its closing text, else its last message.
    pub fn last_message(&self) -> String {
        if self.result_text().trim().is_empty() { self.texts.last().cloned().unwrap_or_default() } else { self.result_text().to_string() }
    }

    /// Limit, Failed or Done, for a run that exited on its own.
    pub fn exited(&self, o: &Outcome) -> Iteration {
        let tail = self.tail.iter().cloned().collect::<Vec<_>>().join("\n");
        let limited = if self.result.is_some() {
            self.is_error() && supervisor::looks_like_limit(self.result_text())
        } else {
            // A clean exit isn't a limit, whatever the last lines say.
            !o.success && supervisor::looks_like_limit(&tail)
        };
        if limited {
            Iteration::Limit
        } else if !o.success || self.is_error() {
            let msg = if self.result_text().trim().is_empty() { tail } else { self.result_text().to_string() };
            Iteration::Failed(msg.chars().take(400).collect())
        } else {
            Iteration::Done
        }
    }
}
