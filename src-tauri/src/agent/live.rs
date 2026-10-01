// A headless agent run the UI can follow and stop: every step (files read,
// searches, commands, notes) goes out as an `agent-feed` event tagged with
// the run id, and `cancel(run)` kills it. The reply comes back with the
// conversation id, so the next call can continue it instead of starting cold.
use super::{live_cmd, runner, stream, Access};
use crate::config::AiSettings;
use runner::Line;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use stream::Event;

pub const FEED_EVENT: &str = "agent-feed";

#[derive(Serialize, Clone)]
struct Feed<'a> {
    run: &'a str,
    /// "tool" | "text" | "thinking" | "log"
    kind: &'a str,
    /// Tool name (kind "tool").
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    text: &'a str,
}

pub struct Reply {
    /// The agent's answer (its final message).
    pub text: String,
    /// Conversation id to pass back as `resume`.
    pub session: Option<String>,
}

fn run_key(run: &str) -> String {
    format!("live:{run}")
}

/// Stops the live run `run`. False when none is going.
pub fn cancel(run: &str) -> bool {
    runner::cancel(&run_key(run))
}

/// "/ws/repo/src/a.ts" → "repo/src/a.ts": paths in the feed read relative
/// to the folder the agent runs in.
fn relative(summary: &str, dir: &str) -> String {
    let fwd = dir.replace('\\', "/");
    let mut out = summary.to_string();
    for prefix in [dir.to_string(), fwd] {
        for sep in ["/", "\\"] {
            out = out.replace(&format!("{prefix}{sep}"), "");
        }
    }
    out
}

/// Runs the configured agent on `prompt` in `dir`, streaming its progress
/// as `agent-feed` events for `run`. Errors: the agent failed, timed out, or
/// was cancelled ("cancelled").
#[allow(clippy::too_many_arguments)]
pub fn run_live(
    app: &AppHandle,
    run: &str,
    ai: &AiSettings,
    dir: &Path,
    prompt: &str,
    access: Access,
    resume: Option<&str>,
    timeout: Duration,
) -> Result<Reply, String> {
    let root = dir.to_string_lossy().to_string();
    let emit = |kind: &str, name: &str, text: &str| {
        let _ = app.emit(FEED_EVENT, Feed { run, kind, name, text });
    };
    let mut texts: Vec<String> = Vec::new();
    let mut result: Option<(String, bool)> = None;
    let mut session: Option<String> = resume.map(String::from);
    let mut stderr_tail: Vec<String> = Vec::new();

    let outcome = runner::run_streaming(live_cmd(ai, prompt, access, resume), dir, timeout, Some(&run_key(run)), |line| match line {
        Line::Out(l) => {
            let events = stream::parse_line(&l);
            if events.is_empty() {
                // Plain-text agents (omp) and stray output: shown as is.
                if !l.trim().is_empty() && !l.trim_start().starts_with('{') {
                    emit("text", "", l.trim_end());
                    texts.push(l);
                }
                return;
            }
            for e in events {
                match e {
                    Event::Text { text } => {
                        emit("text", "", &text);
                        texts.push(text);
                    }
                    Event::Tool { name, summary } => emit("tool", &name, &relative(&summary, &root)),
                    Event::Thinking { text } => emit("thinking", "", &text),
                    Event::Result { text, is_error, .. } => result = Some((text, is_error)),
                    Event::Session { id } => {
                        if session.is_none() {
                            session = Some(id);
                        }
                    }
                    Event::Cost { .. } | Event::Model { .. } => {}
                }
            }
        }
        Line::Err(l) => {
            if !l.trim().is_empty() {
                stderr_tail.push(l);
                if stderr_tail.len() > 20 {
                    stderr_tail.remove(0);
                }
            }
        }
    })?;
    if outcome.cancelled {
        return Err("cancelled".into());
    }
    let reply = match &result {
        Some((text, _)) if !text.trim().is_empty() => text.clone(),
        _ => texts.join("\n"),
    };
    let failed = !outcome.success || result.as_ref().is_some_and(|(_, err)| *err);
    if failed {
        let why = if !stderr_tail.is_empty() { stderr_tail.join("\n") } else { reply.clone() };
        let why = why.trim();
        return Err(if why.is_empty() { "the agent exited with an error".into() } else { why.chars().take(600).collect() });
    }
    Ok(Reply { text: reply, session })
}

#[cfg(test)]
mod tests {
    use super::relative;

    #[test]
    fn feed_paths_are_relative_to_the_run_folder() {
        assert_eq!(relative("/w/ws/bet-app/src/a.ts", "/w/ws"), "bet-app/src/a.ts");
        assert_eq!(relative("rg foo /w/ws/x", "/w/ws"), "rg foo x");
        assert_eq!(relative(r"C:\w\ws\a.ts", r"C:\w\ws"), "a.ts");
    }
}
