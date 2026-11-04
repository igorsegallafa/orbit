//! Live agent status, as `src/lib/agentStatus.ts` derives it: spinner verbs in
//! the output ("✽ Thinking…", "Running tests…") and, when the agent reports
//! them, Claude Code's hooks, which win over the heuristics.
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Busy,
    Thinking,
    Running,
    Editing,
    Waiting,
    Exited,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Busy => "busy",
            Status::Thinking => "thinking",
            Status::Running => "running",
            Status::Editing => "editing",
            Status::Waiting => "waiting",
            Status::Exited => "exited",
        }
    }

    pub fn working(self) -> bool {
        matches!(self, Status::Busy | Status::Thinking | Status::Running | Status::Editing)
    }
}

/// Lifecycle reported by the agent itself (Claude Code hooks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    Working,
    Waiting,
    Done,
}

const NOTIFICATION_WAITING: &[&str] = &["permission_prompt", "elicitation_dialog", "elicitation_url_dialog", "agent_needs_input"];

/// The hook state an `agent-event` means, if any.
pub fn hook_of(event: &str, notification_type: Option<&str>) -> Option<Hook> {
    match event {
        "UserPromptSubmit" | "PostToolUse" => Some(Hook::Working),
        "Stop" | "StopFailure" => Some(Hook::Done),
        "Notification" if notification_type.is_some_and(|t| NOTIFICATION_WAITING.contains(&t)) => Some(Hook::Waiting),
        _ => None,
    }
}

const THINKING: &[&str] = &["thinking", "pondering", "reflecting", "analyzing", "analyze", "considering", "planning"];
const RUNNING: &[&str] = &["running", "executing", "testing", "building", "installing", "compiling", "linting", "fetch", "fetching"];
const EDITING: &[&str] = &["editing", "writing", "updating", "patching", "creating", "modifying", "refactoring"];

/// Chunks arriving within this window mark the session as active.
const ACTIVITY_WINDOW: Duration = Duration::from_millis(2500);
/// A matched verb only counts while fresh: agents keep streaming output long
/// after the spinner line scrolled away.
const VERB_WINDOW: Duration = Duration::from_millis(4000);
/// The spinner verb is near the end of a chunk.
pub const TAIL_BYTES: usize = 2048;

pub struct Tracker {
    last_verb: Status,
    last_verb_at: Option<Instant>,
    last_activity: Option<Instant>,
    exited: bool,
    hook: Option<Hook>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { last_verb: Status::Idle, last_verb_at: None, last_activity: None, exited: false, hook: None }
    }
}

/// Removes CSI sequences so escape codes don't split the words.
fn strip_csi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn verb_of(line: &str) -> Option<Status> {
    let lower = strip_csi(line).to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
    [(THINKING, Status::Thinking), (RUNNING, Status::Running), (EDITING, Status::Editing)]
        .into_iter()
        .find(|(set, _)| has(set))
        .map(|(_, s)| s)
}

impl Tracker {
    pub fn set_hook(&mut self, hook: Hook) {
        self.hook = Some(hook);
    }

    pub fn has_hooks(&self) -> bool {
        self.hook.is_some()
    }

    pub fn feed(&mut self, data: &str, now: Instant) {
        let lines: Vec<&str> = data.split(['\r', '\n']).filter(|l| !l.is_empty()).collect();
        for line in &lines[lines.len().saturating_sub(8)..] {
            if let Some(s) = verb_of(line) {
                self.last_verb = s;
                self.last_verb_at = Some(now);
            }
        }
        self.last_activity = Some(now);
    }

    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    pub fn status(&self, now: Instant) -> Status {
        if self.exited {
            return Status::Exited;
        }
        if self.hook == Some(Hook::Waiting) {
            return Status::Waiting;
        }
        let since = |t: Option<Instant>| t.map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        // Long silent thinking still counts as working when the agent says so.
        if since(self.last_activity) > ACTIVITY_WINDOW {
            return if self.hook == Some(Hook::Working) { Status::Busy } else { Status::Idle };
        }
        if since(self.last_verb_at) > VERB_WINDOW {
            return Status::Busy;
        }
        self.last_verb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_verbs_then_silence() {
        let t0 = Instant::now();
        let mut t = Tracker::default();
        assert_eq!(t.status(t0), Status::Idle);
        t.feed("\x1b[1m✽ Thinking…\x1b[0m (3s)\r\n", t0);
        assert_eq!(t.status(t0), Status::Thinking);
        t.feed("Running tests", t0 + Duration::from_millis(100));
        assert_eq!(t.status(t0 + Duration::from_millis(200)), Status::Running);
        // Output keeps flowing with no fresh verb: generic busy.
        t.feed("plain answer text", t0 + Duration::from_millis(4500));
        assert_eq!(t.status(t0 + Duration::from_millis(4600)), Status::Busy);
        assert_eq!(t.status(t0 + Duration::from_secs(10)), Status::Idle);
        t.mark_exited();
        assert_eq!(t.status(t0), Status::Exited);
    }

    #[test]
    fn hooks_override_the_heuristics() {
        let t0 = Instant::now();
        let mut t = Tracker::default();
        t.set_hook(Hook::Working);
        assert_eq!(t.status(t0 + Duration::from_secs(60)), Status::Busy);
        t.set_hook(Hook::Waiting);
        t.feed("Editing main.rs", t0);
        assert_eq!(t.status(t0), Status::Waiting);
        assert_eq!(hook_of("Notification", Some("permission_prompt")), Some(Hook::Waiting));
        assert_eq!(hook_of("Notification", Some("idle_prompt")), None);
        assert_eq!(hook_of("StopFailure", None), Some(Hook::Done));
    }
}
