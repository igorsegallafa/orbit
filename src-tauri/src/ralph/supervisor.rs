// Decides when an unattended Ralph run keeps going. Every effect is behind
// `RalphEnv`, so the stopping rules are tested without spawning agents.
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Iterations to run (ignored with `until_complete`).
    pub max_iterations: u32,
    pub until_complete: bool,
    /// On a usage limit: wait `limit_wait_min` and retry, or stop.
    pub wait_on_limit: bool,
    pub limit_wait_min: u64,
    /// Wall-clock budget for the whole run (0 = none).
    pub max_hours: f64,
    /// Consecutive iterations without a new commit before giving up.
    pub stall_after: u32,
    /// Stop once this story passes.
    #[serde(default)]
    pub stop_after_story: Option<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_iterations: 10,
            until_complete: false,
            wait_on_limit: true,
            limit_wait_min: 15,
            max_hours: 8.0,
            stall_after: 3,
            stop_after_story: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Iteration {
    /// Agent finished normally.
    Done,
    /// Agent printed <promise>COMPLETE</promise>.
    Complete,
    /// Usage/rate limit hit; nothing was attempted.
    Limit,
    /// Agent exited with an error.
    Failed(String),
    /// Stopped by the user mid-iteration.
    Cancelled,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum StopReason {
    Complete,
    StoryReached(String),
    MaxIterations,
    Stalled,
    Limit,
    Deadline,
    Failed(String),
    Stopped,
    Paused,
}

pub trait RalphEnv {
    fn run_iteration(&mut self, n: u32) -> Iteration;
    fn head(&mut self) -> Option<String>;
    /// Sleeps; false when the run was stopped meanwhile.
    fn sleep(&mut self, d: Duration) -> bool;
    /// Seconds since the run started.
    fn elapsed(&self) -> f64;
    fn all_passed(&mut self) -> bool;
    fn story_passed(&mut self, id: &str) -> bool;
    fn stop_requested(&mut self) -> bool;
    fn pause_requested(&mut self) -> bool;
    fn on_limit_wait(&mut self, _wait: Duration) {}
}

#[derive(Debug, PartialEq)]
pub struct Summary {
    pub reason: StopReason,
    pub iterations: u32,
    pub limit_waits: u32,
}

const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const BETWEEN_ITERATIONS: Duration = Duration::from_secs(2);

pub fn supervise(limits: &Limits, env: &mut impl RalphEnv) -> Summary {
    let deadline = if limits.max_hours > 0.0 { limits.max_hours * 3600.0 } else { f64::INFINITY };
    let limit_wait = Duration::from_secs(limits.limit_wait_min * 60);
    let (mut iterations, mut limit_waits, mut idle, mut failures) = (0u32, 0u32, 0u32, 0u32);
    let done = |reason, iterations, limit_waits| Summary { reason, iterations, limit_waits };

    loop {
        if env.stop_requested() {
            return done(StopReason::Stopped, iterations, limit_waits);
        }
        if env.pause_requested() {
            return done(StopReason::Paused, iterations, limit_waits);
        }
        let before = env.head();
        let outcome = env.run_iteration(iterations + 1);
        match outcome {
            Iteration::Cancelled => return done(StopReason::Stopped, iterations, limit_waits),
            Iteration::Limit => {
                // Not a failed iteration: nothing was attempted.
                limit_waits += 1;
                if !limits.wait_on_limit {
                    return done(StopReason::Limit, iterations, limit_waits);
                }
                if env.elapsed() + limit_wait.as_secs_f64() > deadline {
                    return done(StopReason::Deadline, iterations, limit_waits);
                }
                env.on_limit_wait(limit_wait);
                if !env.sleep(limit_wait) {
                    return done(StopReason::Stopped, iterations, limit_waits);
                }
                continue;
            }
            _ => {}
        }
        iterations += 1;

        if outcome == Iteration::Complete || env.all_passed() {
            return done(StopReason::Complete, iterations, limit_waits);
        }
        if let Some(id) = &limits.stop_after_story {
            if env.story_passed(id) {
                return done(StopReason::StoryReached(id.clone()), iterations, limit_waits);
            }
        }
        match &outcome {
            Iteration::Failed(e) => {
                failures += 1;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    return done(StopReason::Failed(e.clone()), iterations, limit_waits);
                }
            }
            _ => failures = 0,
        }
        // Commits are the only progress signal: uncommitted edits don't
        // count, stopping for review is the safe reading of them.
        if env.head() == before {
            idle += 1;
            if limits.stall_after > 0 && idle >= limits.stall_after {
                return done(StopReason::Stalled, iterations, limit_waits);
            }
        } else {
            idle = 0;
        }
        if !limits.until_complete && iterations >= limits.max_iterations {
            return done(StopReason::MaxIterations, iterations, limit_waits);
        }
        if env.elapsed() > deadline {
            return done(StopReason::Deadline, iterations, limit_waits);
        }
        if !env.sleep(BETWEEN_ITERATIONS) {
            return done(StopReason::Stopped, iterations, limit_waits);
        }
    }
}

/// Usage/rate-limit wording from Claude Code (same pattern as ralph.sh).
pub fn looks_like_limit(text: &str) -> bool {
    let t = text.to_lowercase();
    let near = |a: &str, b: &[&str], gap: usize| {
        t.match_indices(a).any(|(i, _)| {
            let rest = &t[i + a.len()..];
            let window: String = rest.chars().take(gap + 8).collect();
            b.iter().any(|w| window.contains(w))
        })
    };
    near("limit", &["reached", "resets", "reset"], 40)
        || t.contains("hit your") && t.contains("limit")
        || t.contains("reached your") && t.contains("limit")
        || t.contains("rate limit")
        || t.contains("rate-limit")
        || t.contains("ratelimit")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripted env: each iteration returns the next outcome and optionally commits.
    struct Fake {
        script: Vec<(Iteration, bool)>,
        n: usize,
        head: u32,
        clock: f64,
        slept: Vec<Duration>,
        passed_after: Option<usize>,
        stop_at: Option<usize>,
    }

    impl Fake {
        fn new(script: Vec<(Iteration, bool)>) -> Self {
            Fake { script, n: 0, head: 0, clock: 0.0, slept: vec![], passed_after: None, stop_at: None }
        }
    }

    impl RalphEnv for Fake {
        fn run_iteration(&mut self, _n: u32) -> Iteration {
            let (it, commit) = self.script.get(self.n).cloned().unwrap_or((Iteration::Done, true));
            self.n += 1;
            if commit {
                self.head += 1;
            }
            self.clock += 60.0;
            it
        }
        fn head(&mut self) -> Option<String> {
            Some(self.head.to_string())
        }
        fn sleep(&mut self, d: Duration) -> bool {
            self.slept.push(d);
            self.clock += d.as_secs_f64();
            true
        }
        fn elapsed(&self) -> f64 {
            self.clock
        }
        fn all_passed(&mut self) -> bool {
            self.passed_after.is_some_and(|k| self.n >= k)
        }
        fn story_passed(&mut self, _id: &str) -> bool {
            self.n >= 2
        }
        fn stop_requested(&mut self) -> bool {
            self.stop_at.is_some_and(|k| self.n >= k)
        }
        fn pause_requested(&mut self) -> bool {
            false
        }
    }

    fn limits() -> Limits {
        Limits { max_iterations: 5, stall_after: 2, ..Default::default() }
    }

    #[test]
    fn completes_on_promise() {
        let mut f = Fake::new(vec![(Iteration::Done, true), (Iteration::Complete, true)]);
        let s = supervise(&limits(), &mut f);
        assert_eq!(s, Summary { reason: StopReason::Complete, iterations: 2, limit_waits: 0 });
    }

    #[test]
    fn completes_when_every_story_passes_even_without_promise() {
        let mut f = Fake::new(vec![]);
        f.passed_after = Some(3);
        assert_eq!(supervise(&limits(), &mut f).reason, StopReason::Complete);
        assert_eq!(f.n, 3);
    }

    #[test]
    fn stops_at_max_iterations() {
        let mut f = Fake::new(vec![]);
        let s = supervise(&limits(), &mut f);
        assert_eq!((s.reason, s.iterations), (StopReason::MaxIterations, 5));
    }

    #[test]
    fn stalls_after_iterations_without_commits() {
        let mut f = Fake::new(vec![(Iteration::Done, true), (Iteration::Done, false), (Iteration::Done, false)]);
        let s = supervise(&limits(), &mut f);
        assert_eq!((s.reason, s.iterations), (StopReason::Stalled, 3));
    }

    #[test]
    fn limit_waits_do_not_consume_iterations() {
        let mut f = Fake::new(vec![(Iteration::Limit, false), (Iteration::Limit, false), (Iteration::Complete, true)]);
        let s = supervise(&limits(), &mut f);
        assert_eq!(s, Summary { reason: StopReason::Complete, iterations: 1, limit_waits: 2 });
        assert_eq!(f.slept[0], Duration::from_secs(15 * 60));
    }

    #[test]
    fn limit_stops_when_configured_or_past_deadline() {
        let mut f = Fake::new(vec![(Iteration::Limit, false)]);
        let l = Limits { wait_on_limit: false, ..limits() };
        assert_eq!(supervise(&l, &mut f).reason, StopReason::Limit);

        let mut f = Fake::new(vec![(Iteration::Limit, false)]);
        let l = Limits { max_hours: 0.1, ..limits() };
        assert_eq!(supervise(&l, &mut f).reason, StopReason::Deadline);
    }

    #[test]
    fn gives_up_after_consecutive_failures() {
        let fail = || (Iteration::Failed("boom".into()), true);
        let mut f = Fake::new(vec![fail(), fail(), fail()]);
        assert_eq!(supervise(&limits(), &mut f).reason, StopReason::Failed("boom".into()));
    }

    #[test]
    fn stop_after_story_and_user_stop() {
        let mut f = Fake::new(vec![]);
        let l = Limits { stop_after_story: Some("US-002".into()), ..limits() };
        assert_eq!(supervise(&l, &mut f).reason, StopReason::StoryReached("US-002".into()));

        let mut f = Fake::new(vec![]);
        f.stop_at = Some(1);
        let s = supervise(&limits(), &mut f);
        assert_eq!((s.reason, s.iterations), (StopReason::Stopped, 1));
    }

    #[test]
    fn detects_claude_limit_messages() {
        assert!(looks_like_limit("Claude AI usage limit reached|1760000000"));
        assert!(looks_like_limit("5-hour limit reached ∙ resets 3pm"));
        assert!(looks_like_limit("You've hit your usage limit"));
        assert!(looks_like_limit("API Error: rate limit exceeded"));
        assert!(!looks_like_limit("Implemented the rate calculator; all tests pass"));
        assert!(!looks_like_limit("added a limit parameter to the query"));
    }
}
