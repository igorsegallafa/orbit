// Ralph: an autonomous loop that implements a PRD one user story per agent
// iteration (snarktank/ralph pattern), run natively by Orbit so every tool
// call is visible live instead of only when a story finishes.
mod plan_run;
pub mod prd;
mod prompts;
mod supervisor;
mod watchdog;

use crate::agent::runner::{self, Ended, Line, Outcome};
use crate::agent::{agent_cmd, Access, GrillRound};
use crate::config::{AiSettings, Config};
use prd::Prd;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use crate::agent::stream::{self, Event};
use supervisor::{Iteration, RalphEnv};
pub use supervisor::{Limits, StopReason};
pub use watchdog::Activity;
use watchdog::{Check, Tracker};
use crate::events::Events;

const MAX_EVENTS_IN_MEMORY: usize = 600;
const GENERATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const INTERVIEW_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    #[serde(flatten)]
    pub limits: Limits,
    /// Agent/model for this run; the AI settings when empty.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "yes")]
    pub push: bool,
    #[serde(default = "default_iteration_timeout")]
    pub iteration_timeout_min: u64,
    /// Minutes without a stream event before the iteration's process tree
    /// is killed (0 = off; fractions allowed).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_min: f64,
    /// Seconds a finished agent (its result printed) may stay alive before
    /// its tree is killed (0 = off).
    #[serde(default = "default_exit_grace")]
    pub exit_grace_sec: u64,
    /// Appended to the iteration prompt for this run only.
    #[serde(default)]
    pub extra_instructions: String,
}

fn yes() -> bool {
    true
}
fn default_iteration_timeout() -> u64 {
    120
}
fn default_idle_timeout() -> f64 {
    15.0
}
fn default_exit_grace() -> u64 {
    120
}

impl Default for RunConfig {
    fn default() -> Self {
        default_config()
    }
}

/// Live state of a run, kept in memory while Orbit is open (history is on
/// disk in <workspace>/.orbit/ralph/runs/).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub id: String,
    pub repo: String,
    pub running: bool,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub reason: Option<StopReason>,
    pub iteration: u32,
    pub current_story: Option<String>,
    pub passed: usize,
    pub total: usize,
    pub commits: u32,
    pub cost_usd: f64,
    pub limit_until: Option<u64>,
    /// The model the agent reported running (Claude's init line), latest
    /// iteration: proof of what ran, not just what was asked for.
    pub model: Option<String>,
    pub config: RunConfig,
    /// The live iteration's activity (reset between iterations).
    pub activity: Activity,
    /// Commits no remote has, counted when the run stops.
    pub unpushed: u32,
    pub events: VecDeque<Value>,
    #[serde(skip)]
    stop: bool,
    #[serde(skip)]
    pause: bool,
    #[serde(skip)]
    dir: PathBuf,
}

fn runs() -> &'static Mutex<HashMap<String, RunState>> {
    static RUNS: OnceLock<Mutex<HashMap<String, RunState>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn run_key(workspace: &str, repo: &str) -> String {
    format!("{workspace}/{repo}")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix seconds -> "YYYY-MM-DD HH:MM UTC" (civil-from-days, no chrono).
fn human_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

fn repo_dir(workspace: &str, repo: &str) -> Result<PathBuf, String> {
    let dir = crate::workspace::repo_path(workspace, repo)?;
    if !dir.exists() {
        return Err(format!("worktree for '{repo}' not found"));
    }
    Ok(dir)
}

fn custom_prompt_path(workspace: &str) -> Result<PathBuf, String> {
    Ok(crate::workspace::orbit_dir(workspace)?.join("ralph-prompt.md"))
}

fn runs_dir(workspace: &str) -> Result<PathBuf, String> {
    Ok(crate::workspace::orbit_dir(workspace)?.join("ralph").join("runs"))
}

fn iteration_template(workspace: &str) -> (String, bool) {
    match custom_prompt_path(workspace).ok().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(t) if !t.trim().is_empty() => (t, true),
        _ => (prompts::ITERATION.to_string(), false),
    }
}

/// Records an event: memory buffer, events.jsonl on disk and the
/// `ralph-event` channel for the UI.
fn record(app: &Events, key: &str, mut ev: Value, update: impl FnOnce(&mut RunState)) {
    ev["ts"] = json!(now());
    let dir = {
        let mut map = runs().lock().unwrap();
        let Some(st) = map.get_mut(key) else { return };
        update(st);
        st.events.push_back(ev.clone());
        while st.events.len() > MAX_EVENTS_IN_MEMORY {
            st.events.pop_front();
        }
        st.dir.clone()
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("events.jsonl")) {
        let _ = writeln!(f, "{ev}");
    }
    let _ = app.emit("ralph-event", json!({ "key": key, "event": ev }));
}

/// The model an iteration's agent reported; an event only when it changes,
/// so the feed shows it once per run (and again if it ever switches).
fn record_model(app: &Events, key: &str, name: &str) {
    if snapshot(key).is_some_and(|s| s.model.as_deref() == Some(name)) {
        return;
    }
    record(app, key, json!({"kind": "model", "model": name}), |st| st.model = Some(name.to_string()));
}

fn snapshot(key: &str) -> Option<RunState> {
    runs().lock().unwrap().get(key).cloned()
}

fn save_summary(key: &str) {
    if let Some(mut st) = snapshot(key) {
        st.events.clear();
        if let Ok(raw) = serde_json::to_string_pretty(&st) {
            let _ = std::fs::write(st.dir.join("run.json"), raw);
        }
    }
}

/// Per-line bookkeeping shared by both run kinds: the activity the UI
/// shows, emitted on `ralph-activity` when it changes or every few seconds.
struct Live {
    app: Events,
    key: String,
    root: String,
    tracker: Tracker,
    emitted_at: u64,
}

impl Live {
    fn new(app: &Events, key: &str, root: &str) -> Self {
        Live { app: app.clone(), key: key.to_string(), root: root.to_string(), tracker: Tracker::default(), emitted_at: 0 }
    }

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

fn iteration_timeout(cfg: &RunConfig) -> Duration {
    Duration::from_secs(cfg.iteration_timeout_min.max(1) * 60)
}

/// The iteration was killed or cut short instead of exiting on its own.
fn cut_short(o: &Outcome) -> bool {
    o.cancelled || o.ended != Ended::Exited
}

/// Feed events for why the runner killed an iteration.
fn record_ending(app: &Events, key: &str, n: u32, o: &Outcome, cfg: &RunConfig) {
    let ev = match o.ended {
        Ended::Idle => json!({"kind": "iteration_idle", "n": n, "idleMin": cfg.idle_timeout_min}),
        Ended::ExitGrace => json!({"kind": "iteration_exit_grace", "n": n, "graceSec": cfg.exit_grace_sec}),
        Ended::Timeout => json!({"kind": "iteration_timeout", "n": n, "timeoutMin": cfg.iteration_timeout_min}),
        Ended::Exited => return,
    };
    record(app, key, ev, |_| {});
}

/// Outcome of a cut-short iteration from what it left behind.
fn reconciled(app: &Events, key: &str, id: &str, c: &Check, cancelled: bool) -> Iteration {
    record(
        app,
        key,
        json!({"kind": "reconcile", "id": id, "committed": c.committed, "clean": c.clean, "messageOk": c.message_ok, "passed": c.passed()}),
        |_| {},
    );
    if c.passed() {
        Iteration::Recovered
    } else if cancelled {
        Iteration::Cancelled
    } else {
        Iteration::Stalled
    }
}

/// The agent command, or a test stub script in its place.
fn agent_command(stub: &Option<PathBuf>, real: impl FnOnce() -> std::process::Command) -> std::process::Command {
    match stub {
        Some(script) => {
            let mut c = crate::proc::cmd("sh");
            c.arg(script).stdin(std::process::Stdio::null());
            c
        }
        None => real(),
    }
}

struct RealEnv {
    app: Events,
    key: String,
    dir: PathBuf,
    branch: String,
    repo: String,
    workspace: String,
    ai: AiSettings,
    cfg: RunConfig,
    started: Instant,
    last_pushed: Option<String>,
    /// Tests: a script run instead of the agent.
    stub: Option<PathBuf>,
}

impl RealEnv {
    fn head_of(&self) -> Option<String> {
        crate::proc::run("git", &["rev-parse", "HEAD"], Some(&self.dir))
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn commits_since(&self, from: &Option<String>) -> u32 {
        let Some(from) = from else { return 0 };
        crate::proc::run("git", &["rev-list", "--count", &format!("{from}..HEAD")], Some(&self.dir))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    fn push_if_moved(&mut self) {
        let head = self.head_of();
        if !self.cfg.push || head.is_none() || head == self.last_pushed {
            return;
        }
        let r = crate::proc::run("git", &["push", "-u", "origin", "HEAD"], Some(&self.dir));
        let ok = r.is_ok();
        record(&self.app, &self.key, json!({"kind": "push", "ok": ok, "message": r.err().unwrap_or_default()}), |_| {});
        if ok {
            self.last_pushed = head;
        }
    }

    fn prd(&self) -> Option<Prd> {
        prd::read(&self.dir).ok().flatten()
    }

    /// A cut-short iteration whose story is committed cleanly passes; its
    /// prd.json box is ticked here when the agent didn't get to it.
    fn reconcile(&self, story: Option<&str>, head_before: &Option<String>, message: &str, is_error: bool, cancelled: bool) -> Iteration {
        let Some(id) = story else { return if cancelled { Iteration::Cancelled } else { Iteration::Stalled } };
        let c = watchdog::check(&self.dir, head_before, id, &[".orbit/", "scripts/ralph/"], message, is_error);
        let it = reconciled(&self.app, &self.key, id, &c, cancelled);
        if it == Iteration::Recovered {
            if let Some(mut p) = self.prd() {
                if let Some(s) = p.user_stories.iter_mut().find(|s| s.id == id && !s.passes) {
                    s.passes = true;
                    let _ = prd::write(&self.dir, &p);
                }
            }
        }
        it
    }
}

impl RalphEnv for RealEnv {
    fn run_iteration(&mut self, n: u32) -> Iteration {
        let before = self.prd();
        let story = before.as_ref().and_then(|p| p.next_story()).map(|s| (s.id.clone(), s.title.clone()));
        let head_before = self.head_of();
        record(
            &self.app,
            &self.key,
            json!({"kind": "iteration_start", "n": n, "story": story.as_ref().map(|(id, t)| json!({"id": id, "title": t}))}),
            |st| {
                st.iteration = n;
                st.current_story = story.as_ref().map(|(id, _)| id.clone());
                st.limit_until = None;
                st.activity = Activity { last_event_at: Some(now()), ..Default::default() };
            },
        );

        let cmd = agent_command(&self.stub, || {
            let (template, _) = iteration_template(&self.workspace);
            let prompt = prompts::render_iteration(
                &template,
                &prd::prd_path(&self.dir).to_string_lossy(),
                &prd::progress_path(&self.dir).to_string_lossy(),
                &self.branch,
                &self.repo,
                &self.cfg.extra_instructions,
            );
            agent_cmd(&self.ai, &prompt, Access::Full, self.ai.agent_bin() == "claude")
        });
        let run_dir = snapshot(&self.key).map(|s| s.dir).unwrap_or_default();
        let mut raw_log = std::fs::File::create(run_dir.join(format!("iter-{n}.log"))).ok();
        let mut result: Option<Event> = None;
        let mut last_text = String::new();
        let mut tail: VecDeque<String> = VecDeque::new();
        let (app, key) = (self.app.clone(), self.key.clone());
        let root = self.dir.to_string_lossy().to_string();
        let mut live = Live::new(&app, &key, &root);

        let outcome = runner::run_watched(
            cmd,
            &self.dir,
            iteration_timeout(&self.cfg),
            Some(&format!("ralph:{}", self.key)),
            watchdog::watch(&self.cfg),
            |line| {
                let (text, is_err) = match &line {
                    Line::Out(l) => (l.as_str(), false),
                    Line::Err(l) => (l.as_str(), true),
                };
                if let Some(f) = raw_log.as_mut() {
                    let _ = writeln!(f, "{}{text}", if is_err { "[stderr] " } else { "" });
                }
                tail.push_back(text.to_string());
                if tail.len() > 10 {
                    tail.pop_front();
                }
                if !is_err {
                    live.line(text);
                }
                let events = if is_err { vec![] } else { stream::parse_line(text) };
                if events.is_empty() && !text.trim_start().starts_with('{') && !text.trim().is_empty() {
                    record(&app, &key, json!({"kind": "log", "text": text, "stderr": is_err}), |_| {});
                }
                let mut finished = false;
                for e in events {
                    match &e {
                        Event::Result { .. } => {
                            finished = true;
                            result = Some(e)
                        }
                        Event::Text { text } => {
                            last_text = text.clone();
                            record(&app, &key, json!({"kind": "text", "text": text}), |_| {})
                        }
                        Event::Tool { name, summary } => {
                            let summary = relativize(summary, &root);
                            record(&app, &key, json!({"kind": "tool", "name": name, "summary": summary}), |_| {})
                        }
                        Event::Model { name } => record_model(&app, &key, name),
                        Event::Session { .. } | Event::Cost { .. } | Event::Thinking { .. } => {}
                    }
                }
                finished
            },
        );

        let (res_text, res_error, cost, duration, turns) = match &result {
            Some(Event::Result { text, is_error, cost_usd, duration_ms, turns }) => {
                (text.clone(), *is_error, *cost_usd, *duration_ms, *turns)
            }
            _ => (String::new(), false, None, None, None),
        };
        let tail_text = tail.iter().cloned().collect::<Vec<_>>().join("\n");
        let iteration = match &outcome {
            Err(e) => Iteration::Failed(e.clone()),
            Ok(o) if cut_short(o) => {
                record_ending(&self.app, &self.key, n, o, &self.cfg);
                let message = if res_text.trim().is_empty() { &last_text } else { &res_text };
                self.reconcile(story.as_ref().map(|(id, _)| id.as_str()), &head_before, message, res_error, o.cancelled)
            }
            Ok(o) => {
                let limited = if result.is_some() {
                    res_error && supervisor::looks_like_limit(&res_text)
                } else {
                    supervisor::looks_like_limit(&tail_text)
                };
                let full = if result.is_some() { res_text.clone() } else { o.stdout.clone() };
                if limited {
                    Iteration::Limit
                } else if full.contains("<promise>COMPLETE</promise>") {
                    Iteration::Complete
                } else if !o.success || res_error {
                    let msg = if res_text.trim().is_empty() { tail_text.clone() } else { res_text.clone() };
                    Iteration::Failed(msg.chars().take(400).collect())
                } else {
                    Iteration::Done
                }
            }
        };

        let after = self.prd();
        let newly = match (&after, &before) {
            (Some(a), Some(b)) => a.newly_passed(b),
            _ => vec![],
        };
        for id in &newly {
            record(&self.app, &self.key, json!({"kind": "story_passed", "id": id}), |_| {});
        }
        let commits = self.commits_since(&head_before);
        let (passed, total) = after.as_ref().map(|p| (p.passed(), p.user_stories.len())).unwrap_or((0, 0));
        record(
            &self.app,
            &self.key,
            json!({
                "kind": "iteration_end", "n": n, "outcome": format!("{iteration:?}"),
                "commits": commits, "costUsd": cost, "durationMs": duration, "turns": turns,
                "passed": passed, "total": total,
            }),
            |st| {
                st.commits += commits;
                st.cost_usd += cost.unwrap_or(0.0);
                st.passed = passed;
                st.total = total;
                st.activity = Activity::default();
            },
        );
        if iteration != Iteration::Limit && iteration != Iteration::Cancelled {
            self.push_if_moved();
        }
        save_summary(&self.key);
        iteration
    }

    fn head(&mut self) -> Option<String> {
        self.head_of()
    }

    fn sleep(&mut self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.stop_requested() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(500).min(end - Instant::now()));
        }
        !self.stop_requested()
    }

    fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn all_passed(&mut self) -> bool {
        self.prd().is_some_and(|p| p.all_passed())
    }

    fn story_passed(&mut self, id: &str) -> bool {
        self.prd().is_some_and(|p| p.user_stories.iter().any(|s| s.id == id && s.passes))
    }

    fn stop_requested(&mut self) -> bool {
        runs().lock().unwrap().get(&self.key).is_some_and(|s| s.stop)
    }

    fn pause_requested(&mut self) -> bool {
        runs().lock().unwrap().get(&self.key).is_some_and(|s| s.pause)
    }

    fn on_limit_wait(&mut self, wait: Duration) {
        let until = now() + wait.as_secs();
        record(&self.app, &self.key, json!({"kind": "limit_wait", "until": until}), |st| st.limit_until = Some(until));
    }
}

// ---------- commands ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RalphState {
    pub prd: Option<Prd>,
    pub progress: String,
    pub run: Option<RunState>,
    pub prompt: String,
    pub prompt_custom: bool,
    pub default_config: RunConfig,
    /// Plan runs: whether the workspace has a PLAN.md at all.
    #[serde(default)]
    pub plan_exists: bool,
    /// Plan runs: what would stop a run (e.g. a task naming an unknown repo).
    #[serde(default)]
    pub warnings: Vec<String>,
}

fn default_config() -> RunConfig {
    RunConfig {
        limits: Limits::default(),
        agent: None,
        model: None,
        // Off until asked: a run pushing on its own surprises people.
        push: false,
        iteration_timeout_min: default_iteration_timeout(),
        idle_timeout_min: default_idle_timeout(),
        exit_grace_sec: default_exit_grace(),
        extra_instructions: String::new(),
    }
}

pub fn state(workspace: String, repo: String) -> Result<RalphState, String> {
    let dir = repo_dir(&workspace, &repo)?;
    let (prompt, prompt_custom) = iteration_template(&workspace);
    Ok(RalphState {
        prd: prd::read(&dir)?,
        progress: prd::read_progress(&dir),
        run: snapshot(&run_key(&workspace, &repo)),
        prompt,
        prompt_custom,
        default_config: default_config(),
        plan_exists: false,
        warnings: vec![],
    })
}

pub fn save_prd(workspace: String, repo: String, prd: Prd) -> Result<(), String> {
    prd::write(&repo_dir(&workspace, &repo)?, &prd)
}

/// Saves the workspace's iteration prompt; None/empty restores the default.
pub fn save_prompt(workspace: String, prompt: Option<String>) -> Result<(), String> {
    let p = custom_prompt_path(&workspace)?;
    match prompt.filter(|t| !t.trim().is_empty() && t.trim() != prompts::ITERATION.trim()) {
        Some(t) => {
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&p, t).map_err(|e| e.to_string())
        }
        None => {
            let _ = std::fs::remove_file(&p);
            Ok(())
        }
    }
}

/// One round of the PRD clarifying interview (same stepper as Plan's grill).
pub fn interview(
    workspace: String,
    repo: String,
    brief: String,
    answers: Vec<(String, String)>,
    rounds_done: usize,
    max_rounds: Option<usize>,
) -> Result<GrillRound, String> {
    let dir = repo_dir(&workspace, &repo)?;
    let history = if answers.is_empty() {
        "None yet, this is round 1.".to_string()
    } else {
        answers.iter().map(|(q, a)| format!("- Q: {q}\n  A: {a}")).collect::<Vec<_>>().join("\n")
    };
    let cap = match max_rounds {
        Some(max) if rounds_done + 1 >= max => {
            "\n# HARD LIMIT\nThis is the LAST round: set done=true with a summary of everything decided.".to_string()
        }
        Some(max) => format!("\n# Round budget\nRound {} of {max}.", rounds_done + 1),
        None => String::new(),
    };
    let ai = Config::load()?.ai;
    let reply = runner::run_capture(
        agent_cmd(&ai, &prompts::interview(&brief, &repo, &history, &cap), Access::ReadOnly, false),
        &dir,
        INTERVIEW_TIMEOUT,
    )?;
    crate::agent::parse_grill_json(&reply)
}

/// Writes tasks/prd-<workspace>.md and scripts/ralph/prd.json with the
/// agent (streams `ralph-prd-progress`), then normalizes the JSON.
pub fn generate_prd(
    app: Events,
    workspace: String,
    repo: String,
    brief: String,
    decisions: String,
) -> Result<Prd, String> {
    let dir = repo_dir(&workspace, &repo)?;
    let branch = crate::workspace::scope_branch(&workspace)?;
    let date = human_time(now())[..10].to_string();
    if let Some(dest) = prd::archive_if_other_branch(&dir, &branch, &date)? {
        let _ = app.emit("ralph-prd-progress", format!("[orbit] archived the previous PRD to {}", dest.display()));
    }
    let md = dir.join("tasks").join(format!("prd-{}.md", workspace.trim_start_matches('@')));
    std::fs::create_dir_all(md.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(prd::ralph_dir(&dir)).map_err(|e| e.to_string())?;
    let prompt = prompts::write_prd(
        &brief,
        if decisions.trim().is_empty() { "(none: use your judgment and list assumptions under Open Questions)" } else { &decisions },
        &md.to_string_lossy(),
        &prd::prd_path(&dir).to_string_lossy(),
        &branch,
        &repo,
    );
    let ai = Config::load()?.ai;
    let o = runner::run_streaming(
        agent_cmd(&ai, &prompt, Access::Edit, false),
        &dir,
        GENERATE_TIMEOUT,
        Some(&format!("ralph-prd:{}", run_key(&workspace, &repo))),
        |l| {
            let (Line::Out(s) | Line::Err(s)) = l;
            let _ = app.emit("ralph-prd-progress", s);
        },
    )?;
    if o.cancelled {
        return Err("cancelled".into());
    }
    if !o.success {
        return Err(format!("agent failed: {}", o.stderr.trim()));
    }
    let mut prd = prd::read(&dir)?.ok_or("the agent did not write scripts/ralph/prd.json")?;
    if prd.user_stories.is_empty() {
        return Err("the generated PRD has no user stories".into());
    }
    prd::normalize(&mut prd, &branch);
    prd::write(&dir, &prd)?;
    Ok(prd)
}

pub fn cancel_generate(workspace: String, repo: String) -> Result<bool, String> {
    Ok(runner::cancel(&format!("ralph-prd:{}", run_key(&workspace, &repo))))
}

pub fn start(app: Events, workspace: String, repo: String, config: RunConfig) -> Result<(), String> {
    let key = run_key(&workspace, &repo);
    let dir = repo_dir(&workspace, &repo)?;
    let prd = prd::read(&dir)?.ok_or("no scripts/ralph/prd.json yet: create the PRD first")?;
    if prd.all_passed() {
        return Err("every story already passes".into());
    }
    let branch = crate::workspace::scope_branch(&workspace)?;
    // In a repo view Ralph works in the clone itself: it commits every story,
    // which must not land straight on the default branch.
    if crate::workspace::repo_scope(&workspace).is_some() && crate::git::default_branch(&dir).as_deref() == Some(branch.as_str()) {
        return Err(format!("Ralph commits each story: switch {repo} to a feature branch first (it's on {branch})"));
    }
    let mut ai = Config::load()?.ai;
    if let Some(a) = config.agent.clone().filter(|a| !a.is_empty()) {
        ai.agent = a;
    }
    if let Some(m) = config.model.clone().filter(|m| !m.is_empty()) {
        ai.model = m;
    }
    let started = now();
    let run_dir = runs_dir(&workspace)?.join(format!("{started}-{repo}"));
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    prd::ensure_progress(&dir, &human_time(started))?;

    {
        let mut map = runs().lock().unwrap();
        if map.get(&key).is_some_and(|s| s.running) {
            return Err("Ralph is already running for this repo".into());
        }
        map.insert(
            key.clone(),
            RunState {
                id: format!("{started}-{repo}"),
                repo: repo.clone(),
                running: true,
                started_at: started,
                finished_at: None,
                reason: None,
                iteration: 0,
                current_story: prd.next_story().map(|s| s.id.clone()),
                passed: prd.passed(),
                total: prd.user_stories.len(),
                commits: 0,
                cost_usd: 0.0,
                limit_until: None,
                model: None,
                config: config.clone(),
                activity: Activity::default(),
                unpushed: 0,
                events: VecDeque::new(),
                stop: false,
                pause: false,
                dir: run_dir,
            },
        );
    }
    record(&app, &key, json!({"kind": "run_start", "agent": ai.agent, "model": ai.model}), |_| {});

    let mut env = RealEnv {
        app: app.clone(),
        key: key.clone(),
        last_pushed: crate::proc::run("git", &["rev-parse", "@{u}"], Some(&dir)).ok().map(|s| s.trim().to_string()),
        dir,
        branch,
        repo,
        workspace,
        ai,
        cfg: config.clone(),
        started: Instant::now(),
        stub: None,
    };
    std::thread::spawn(move || {
        let summary = supervisor::supervise(&config.limits, &mut env);
        finish(&app, &key, &summary, watchdog::unpushed(&env.dir));
    });
    Ok(())
}

/// Records the run's end, with how many commits it left unpushed.
fn finish(app: &Events, key: &str, summary: &supervisor::Summary, unpushed: u32) {
    record(
        app,
        key,
        json!({"kind": "stopped", "reason": summary.reason, "iterations": summary.iterations, "limitWaits": summary.limit_waits, "unpushed": unpushed}),
        |st| {
            st.running = false;
            st.finished_at = Some(now());
            st.reason = Some(summary.reason.clone());
            st.limit_until = None;
            st.activity = Activity::default();
            st.unpushed = unpushed;
        },
    );
    save_summary(key);
}

/// Pushes what a stopped run left unpushed (its "N commits not pushed"
/// button); returns how many are still unpushed.
pub fn push(workspace: String, repo: String) -> Result<u32, String> {
    let key = run_key(&workspace, &repo);
    let dirs = if repo == plan_run::PLAN_REPO { plan_run::repo_dirs(&workspace)? } else { vec![(repo.clone(), repo_dir(&workspace, &repo)?)] };
    let events = Events::new(crate::events::NoEvents);
    let mut errors = Vec::new();
    for (name, dir) in &dirs {
        if watchdog::unpushed(dir) == 0 {
            continue;
        }
        let r = crate::proc::run("git", &["push", "-u", "origin", "HEAD"], Some(dir));
        if let Err(e) = &r {
            errors.push(format!("{name}: {e}"));
        }
        record(&events, &key, json!({"kind": "push", "ok": r.is_ok(), "repo": name, "message": r.err().unwrap_or_default()}), |_| {});
    }
    let left: u32 = dirs.iter().map(|(_, d)| watchdog::unpushed(d)).sum();
    if let Some(st) = runs().lock().unwrap().get_mut(&key) {
        st.unpushed = left;
    }
    save_summary(&key);
    if errors.is_empty() {
        Ok(left)
    } else {
        Err(errors.join("; "))
    }
}

/// Ralph over the feature plan: the plan's tasks (as a PRD) and the run.
pub fn plan_state(workspace: String) -> Result<RalphState, String> {
    plan_run::state(&workspace)
}

/// Starts Ralph on the plan's open tasks (key `<workspace>/plan`; stop,
/// pause and history take repo "plan").
pub fn plan_start(app: Events, workspace: String, config: RunConfig) -> Result<(), String> {
    plan_run::start(app, workspace, config)
}

/// Keys (`workspace/repo`) of the runs in progress, for app-wide indicators.
pub fn running() -> Result<Vec<String>, String> {
    Ok(runs().lock().unwrap().iter().filter(|(_, st)| st.running).map(|(k, _)| k.clone()).collect())
}

/// Stops now: kills the running iteration (its commits stay).
pub fn stop(workspace: String, repo: String) -> Result<(), String> {
    let key = run_key(&workspace, &repo);
    if let Some(st) = runs().lock().unwrap().get_mut(&key) {
        st.stop = true;
    }
    runner::cancel(&format!("ralph:{key}"));
    Ok(())
}

/// Stops after the current iteration finishes.
pub fn pause(workspace: String, repo: String) -> Result<(), String> {
    if let Some(st) = runs().lock().unwrap().get_mut(&run_key(&workspace, &repo)) {
        st.pause = true;
    }
    Ok(())
}

/// Past runs of this workspace (newest first), from their run.json.
pub fn past_runs(workspace: String) -> Result<Vec<Value>, String> {
    let dir = runs_dir(&workspace)?;
    let mut out: Vec<Value> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| std::fs::read_to_string(e.path().join("run.json")).ok())
                .filter_map(|raw| serde_json::from_str(&raw).ok())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|v: &Value| std::cmp::Reverse(v["startedAt"].as_u64().unwrap_or(0)));
    Ok(out)
}

/// Events of a past run (events.jsonl), for replaying its feed.
pub fn run_events(workspace: String, id: String) -> Result<Vec<Value>, String> {
    if id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid run id".into());
    }
    let p: PathBuf = runs_dir(&workspace)?.join(&id).join("events.jsonl");
    Ok(read_jsonl(&p))
}

/// Drops the worktree prefix from paths in a tool summary (agents use
/// absolute paths; the repo-relative part is what's worth reading).
fn relativize(summary: &str, root: &str) -> String {
    let fwd = root.replace('\\', "/");
    let mut out = summary.to_string();
    for prefix in [root.to_string(), fwd] {
        for sep in ["\\", "/"] {
            out = out.replace(&format!("{prefix}{sep}"), "");
        }
        out = out.replace(&prefix, ".");
    }
    out
}

fn read_jsonl(p: &Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_paths_are_shown_relative_to_the_worktree() {
        let root = r"C:\ws\feat\web";
        assert_eq!(relativize(r"C:\ws\feat\web\src\a.ts", root), r"src\a.ts");
        assert_eq!(relativize(r#"cd /d "C:\ws\feat\web" && npm test"#, root), r#"cd /d "." && npm test"#);
        assert_eq!(relativize("/ws/feat/web/src/a.ts", "/ws/feat/web"), "src/a.ts");
        assert_eq!(relativize("C:/ws/feat/web/x.rs", root), "x.rs");
    }

    #[test]
    fn human_time_formats_utc() {
        assert_eq!(human_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(human_time(1_789_992_000), "2026-09-21 12:00 UTC");
    }

    #[test]
    fn run_config_accepts_partial_json_from_the_ui() {
        let c: RunConfig = serde_json::from_value(json!({
            "maxIterations": 3, "untilComplete": false, "waitOnLimit": true, "limitWaitMin": 5,
            "maxHours": 2.0, "stallAfter": 2, "model": "claude-opus-5"
        }))
        .unwrap();
        assert_eq!(c.limits.max_iterations, 3);
        assert!(c.push);
        assert_eq!(c.iteration_timeout_min, 120);
        assert_eq!(c.idle_timeout_min, 15.0);
        assert_eq!(c.exit_grace_sec, 120);
    }

    #[test]
    fn prompt_renders_paths_and_extra_instructions() {
        let p = prompts::render_iteration(prompts::ITERATION, "/w/prd.json", "/w/progress.txt", "feat/x", "web", "Use pnpm");
        assert!(p.contains("`/w/prd.json`") && p.contains("branch `feat/x`"));
        assert!(p.ends_with("Use pnpm"));
        assert!(p.contains("never on `pgrep -f"));
        let plain = prompts::render_iteration(prompts::ITERATION, "a", "b", "c", "d", "");
        assert!(plain.ends_with(watchdog::BACKGROUND_RULES));
        assert!(!p.contains('{') || p.contains("<promise>"));
    }


    /// Stub agents are shell scripts, and the checks use process groups.
    #[cfg(unix)]
    mod guard {
        use super::*;

        use crate::events::NoEvents;
        use watchdog::fixture::{self, git, quick, RESULT_DONE};

        /// A repo PRD with one story, run by `agent`.
        fn prd_env(name: &str, cfg: RunConfig, agent: &str) -> (PathBuf, RealEnv) {
            let root = fixture::scratch(name);
            let repo = root.join("app");
            fixture::repo(&repo, &root.join("remote.git"));
            let story = prd::Story {
                id: "US-001".into(),
                title: "First".into(),
                description: String::new(),
                acceptance_criteria: vec![],
                priority: 1,
                passes: false,
                notes: String::new(),
                extra: Default::default(),
            };
            let p = Prd { project: "p".into(), branch_name: "feat/x".into(), description: String::new(), user_stories: vec![story], extra: Default::default() };
            prd::write(&repo, &p).unwrap();
            let run_dir = root.join("run");
            std::fs::create_dir_all(&run_dir).unwrap();
            let key = run_key(&format!("guard-{name}"), "app");
            fixture::start_run(&key, &run_dir, &cfg);
            let env = RealEnv {
                app: Events::new(NoEvents),
                key,
                last_pushed: Some(git(&repo, &["rev-parse", "HEAD"]).trim().to_string()),
                dir: repo,
                branch: "feat/x".into(),
                repo: "app".into(),
                workspace: format!("guard-{name}"),
                ai: AiSettings::default(),
                cfg,
                started: Instant::now(),
                stub: Some(fixture::stub(&root, agent)),
            };
            (root, env)
        }

        const COMMIT_US1: &str = "git commit --allow-empty -qm 'feat: [US-001] - First'";

        #[test]
        fn a_hung_prd_iteration_with_its_commit_ticks_prd_json() {
            let cfg = RunConfig { idle_timeout_min: 10.0, ..quick() };
            let (root, mut env) = prd_env("prd-grace", cfg, &format!("{COMMIT_US1}\n{RESULT_DONE}\nsleep 1200 &\nwait"));
            assert_eq!(env.run_iteration(1), Iteration::Recovered);
            assert!(env.story_passed("US-001"));
            assert_eq!(git(&root.join("remote.git"), &["rev-parse", "feat/x"]), git(&env.dir, &["rev-parse", "HEAD"]));
            assert!(fixture::group_empty(&fixture::pgid(&root)));
        }

        #[test]
        fn stopping_a_hung_iteration_keeps_its_committed_story() {
            let cfg = RunConfig { idle_timeout_min: 10.0, exit_grace_sec: 0, ..quick() };
            let (root, mut env) = prd_env("prd-stop", cfg, &format!("{COMMIT_US1}\n{RESULT_DONE}\nsleep 1200 &\nwait"));
            let pid = root.join("pid");
            std::thread::spawn(move || {
                while !pid.exists() {
                    std::thread::sleep(Duration::from_millis(50));
                }
                std::thread::sleep(Duration::from_millis(500));
                stop("guard-prd-stop".into(), "app".into()).unwrap();
            });
            let s = supervisor::supervise(&Limits { max_iterations: 5, ..Default::default() }, &mut env);
            // Its only story passed, so the run completes instead of stopping.
            assert_eq!((s.reason, s.iterations), (StopReason::Complete, 1));
            assert!(env.story_passed("US-001"));
            assert!(fixture::group_empty(&fixture::pgid(&root)));
        }
    }
}
