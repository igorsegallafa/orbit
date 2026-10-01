// Ralph over the feature plan (PLAN.md at the workspace root): one run for
// the whole feature, each iteration a fresh agent on the next open task, in
// that task's repository. The plan is re-read every iteration (edits made
// between iterations count); Orbit marks a task done once the agent says
// so AND the task's repo got a commit, so the agent never edits the plan.
use super::prd::{Prd, Story};
use super::supervisor::{self, Iteration, RalphEnv};
use super::{human_time, now, prompts, record, relativize, runs, runs_dir, save_summary, snapshot, RalphState, RunConfig, RunState};
use crate::agent::plan::{parse_plan, PlanItem};
use crate::agent::runner::{self, Line};
use crate::agent::stream::{self, Event};
use crate::agent::{live_cmd, set_task_in_content, Access, PLAN_FILE};
use crate::config::{AiSettings, Config};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// The run key's "repo" part for plan runs: `<workspace>/plan`.
pub const PLAN_REPO: &str = "plan";
const DONE_MARK: &str = "<task-done/>";

/// Claude's closing result: text, is_error, cost, duration ms, turns.
type RunResult = (String, bool, Option<f64>, Option<u64>, Option<u64>);

fn plan_path(ws_dir: &std::path::Path) -> PathBuf {
    ws_dir.join(PLAN_FILE)
}

fn read_items(ws_dir: &std::path::Path) -> (String, Vec<PlanItem>) {
    std::fs::read_to_string(plan_path(ws_dir)).map(|raw| parse_plan(&raw)).unwrap_or_default()
}

/// The workspace repo a task runs in: its named repo (case-insensitive),
/// or the only repo of a single-repo workspace.
fn repo_for(item: &PlanItem, repos: &[String]) -> Option<String> {
    if item.repo.is_empty() {
        return (repos.len() == 1).then(|| repos[0].clone());
    }
    repos.iter().find(|r| r.eq_ignore_ascii_case(&item.repo)).cloned()
}

fn progress_path(workspace: &str) -> Result<PathBuf, String> {
    Ok(crate::workspace::orbit_dir(workspace)?.join("ralph").join("progress.md"))
}

fn ensure_progress(path: &std::path::Path, started: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("# Ralph Progress Log\nStarted: {started}\n\n## Codebase Patterns\n\n---\n"))
        .map_err(|e| e.to_string())
}

/// Tasks with a repository this workspace doesn't have (Ralph can't run them).
fn unknown_repos(items: &[PlanItem], repos: &[String]) -> Vec<String> {
    items
        .iter()
        .filter(|i| !i.done && repo_for(i, repos).is_none())
        .map(|i| {
            if i.repo.is_empty() {
                format!("{} names no repository (end its line with \"— <repo>\")", i.id)
            } else {
                format!("{} runs in \"{}\", not a repository of this workspace", i.id, i.repo)
            }
        })
        .collect()
}

/// The plan as the PRD shape the Ralph view renders (a task = a story,
/// its repo in `repo`).
fn as_prd(title: &str, context: &str, branch: &str, items: &[PlanItem]) -> Prd {
    Prd {
        project: title.to_string(),
        branch_name: branch.to_string(),
        description: context.to_string(),
        user_stories: items
            .iter()
            .map(|i| {
                let mut extra = serde_json::Map::new();
                extra.insert("repo".into(), json!(i.repo));
                Story {
                    id: i.id.clone(),
                    title: i.title.clone(),
                    description: i.description.clone(),
                    acceptance_criteria: i.accept.clone(),
                    priority: i.index as i64 + 1,
                    passes: i.done,
                    notes: String::new(),
                    extra,
                }
            })
            .collect(),
        extra: serde_json::Map::new(),
    }
}

fn plan_title(ws_dir: &std::path::Path) -> String {
    std::fs::read_to_string(plan_path(ws_dir))
        .ok()
        .and_then(|raw| raw.lines().find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string())))
        .unwrap_or_default()
}

struct PlanEnv {
    app: AppHandle,
    key: String,
    workspace: String,
    ws_dir: PathBuf,
    branch: String,
    repos: Vec<String>,
    ai: AiSettings,
    cfg: RunConfig,
    started: Instant,
    progress: PathBuf,
    last_pushed: HashMap<String, Option<String>>,
}

impl PlanEnv {
    fn dir_of(&self, repo: &str) -> PathBuf {
        crate::workspace::repo_path(&self.workspace, repo).unwrap_or_else(|_| self.ws_dir.join(repo))
    }

    fn head_of(&self, repo: &str) -> Option<String> {
        crate::proc::run("git", &["rev-parse", "HEAD"], Some(&self.dir_of(repo))).ok().map(|s| s.trim().to_string())
    }

    fn commits_since(&self, repo: &str, from: &Option<String>) -> u32 {
        let Some(from) = from else { return 0 };
        crate::proc::run("git", &["rev-list", "--count", &format!("{from}..HEAD")], Some(&self.dir_of(repo)))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    fn push_if_moved(&mut self, repo: &str) {
        let head = self.head_of(repo);
        let last = self.last_pushed.get(repo).cloned().flatten();
        if !self.cfg.push || head.is_none() || head == last {
            return;
        }
        let r = crate::proc::run("git", &["push", "-u", "origin", "HEAD"], Some(&self.dir_of(repo)));
        let ok = r.is_ok();
        record(&self.app, &self.key, json!({"kind": "push", "ok": ok, "repo": repo, "message": r.err().unwrap_or_default()}), |_| {});
        if ok {
            self.last_pushed.insert(repo.to_string(), head);
        }
    }

    /// Marks the task done in PLAN.md (by position, like PlanProgress).
    fn mark_done(&self, index: usize) -> Result<(), String> {
        let path = plan_path(&self.ws_dir);
        let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let updated = set_task_in_content(&raw, index, true)?;
        std::fs::write(&path, updated).map_err(|e| e.to_string())
    }

    fn counts(&self) -> (usize, usize) {
        let (_, items) = read_items(&self.ws_dir);
        (items.iter().filter(|i| i.done).count(), items.len())
    }
}

impl RalphEnv for PlanEnv {
    fn run_iteration(&mut self, n: u32) -> Iteration {
        let (context, items) = read_items(&self.ws_dir);
        let Some(task) = items.iter().find(|i| !i.done).cloned() else { return Iteration::Complete };
        let Some(repo) = repo_for(&task, &self.repos) else {
            let why = unknown_repos(std::slice::from_ref(&task), &self.repos).join("");
            record(&self.app, &self.key, json!({"kind": "log", "text": why, "stderr": true}), |_| {});
            return Iteration::Failed(why);
        };
        let dir = self.dir_of(&repo);
        let head_before = self.head_of(&repo);
        record(
            &self.app,
            &self.key,
            json!({"kind": "iteration_start", "n": n, "story": {"id": task.id, "title": task.title, "repo": repo}}),
            |st| {
                st.iteration = n;
                st.current_story = Some(task.id.clone());
                st.limit_until = None;
            },
        );

        let mut body = String::new();
        if !task.description.is_empty() {
            body.push_str(&task.description);
            body.push_str("\n\n");
        }
        body.push_str("Acceptance criteria:\n");
        if task.accept.is_empty() {
            body.push_str("- (none listed) the change works, and the repo's typecheck/lint/tests pass\n");
        } else {
            for c in &task.accept {
                body.push_str(&format!("- {c}\n"));
            }
        }
        let prompt = prompts::render_plan_iteration(
            &repo,
            &self.branch,
            &context,
            &task.id,
            &task.title,
            &body,
            &self.progress.to_string_lossy(),
            &plan_path(&self.ws_dir).to_string_lossy(),
            &self.cfg.extra_instructions,
        );

        let run_dir = snapshot(&self.key).map(|s| s.dir).unwrap_or_default();
        let mut raw_log = std::fs::File::create(run_dir.join(format!("iter-{n}.log"))).ok();
        let mut result: Option<RunResult> = None;
        let mut texts: Vec<String> = Vec::new();
        let mut step_cost = 0.0;
        let mut tail: VecDeque<String> = VecDeque::new();
        let (app, key) = (self.app.clone(), self.key.clone());
        let root = dir.to_string_lossy().to_string();

        let outcome = runner::run_streaming(
            live_cmd(&self.ai, &prompt, Access::Full, None),
            &dir,
            Duration::from_secs(self.cfg.iteration_timeout_min.max(1) * 60),
            Some(&format!("ralph:{}", self.key)),
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
                let events = if is_err { vec![] } else { stream::parse_line(text) };
                if events.is_empty() && !text.trim_start().starts_with('{') && !text.trim().is_empty() {
                    if !is_err {
                        texts.push(text.to_string());
                    }
                    record(&app, &key, json!({"kind": "log", "text": text, "stderr": is_err}), |_| {});
                }
                for e in events {
                    match e {
                        Event::Result { text, is_error, cost_usd, duration_ms, turns } => {
                            result = Some((text, is_error, cost_usd, duration_ms, turns))
                        }
                        Event::Text { text } => {
                            record(&app, &key, json!({"kind": "text", "text": text}), |_| {});
                            texts.push(text);
                        }
                        Event::Thinking { text } => record(&app, &key, json!({"kind": "thinking", "text": text}), |_| {}),
                        Event::Tool { name, summary } => {
                            let summary = relativize(&summary, &root);
                            record(&app, &key, json!({"kind": "tool", "name": name, "summary": summary}), |_| {})
                        }
                        Event::Cost { usd } => step_cost += usd,
                        Event::Model { name } => super::record_model(&app, &key, &name),
                        Event::Session { .. } => {}
                    }
                }
            },
        );

        let (res_text, res_error, cost, duration, turns) = result.clone().unwrap_or_default();
        let cost = cost.or((step_cost > 0.0).then_some(step_cost));
        let full = if res_text.trim().is_empty() { texts.join("\n") } else { res_text.clone() };
        let tail_text = tail.iter().cloned().collect::<Vec<_>>().join("\n");
        let mut iteration = match &outcome {
            Err(e) => Iteration::Failed(e.clone()),
            Ok(o) if o.cancelled => Iteration::Cancelled,
            Ok(o) => {
                let limited = if result.is_some() {
                    res_error && supervisor::looks_like_limit(&res_text)
                } else {
                    !o.success && supervisor::looks_like_limit(&tail_text)
                };
                if limited {
                    Iteration::Limit
                } else if !o.success || res_error {
                    let msg = if res_text.trim().is_empty() { tail_text.clone() } else { res_text.clone() };
                    Iteration::Failed(msg.chars().take(400).collect())
                } else {
                    Iteration::Done
                }
            }
        };

        let commits = self.commits_since(&repo, &head_before);
        if iteration == Iteration::Done {
            if full.contains(DONE_MARK) && commits > 0 {
                match self.mark_done(task.index) {
                    Ok(()) => record(&self.app, &self.key, json!({"kind": "story_passed", "id": task.id}), |_| {}),
                    Err(e) => record(&self.app, &self.key, json!({"kind": "log", "text": format!("couldn't mark {} done in PLAN.md: {e}", task.id), "stderr": true}), |_| {}),
                }
            } else if full.contains(DONE_MARK) {
                record(
                    &self.app,
                    &self.key,
                    json!({"kind": "log", "text": format!("{} was reported done, but {repo} has no new commit: it stays open", task.id), "stderr": true}),
                    |_| {},
                );
            }
        }
        let (passed, total) = self.counts();
        if iteration == Iteration::Done && total > 0 && passed == total {
            iteration = Iteration::Complete;
        }
        record(
            &self.app,
            &self.key,
            json!({
                "kind": "iteration_end", "n": n, "outcome": format!("{iteration:?}"), "repo": repo,
                "commits": commits, "costUsd": cost, "durationMs": duration, "turns": turns,
                "passed": passed, "total": total,
            }),
            |st| {
                st.commits += commits;
                st.cost_usd += cost.unwrap_or(0.0);
                st.passed = passed;
                st.total = total;
            },
        );
        if iteration != Iteration::Limit && iteration != Iteration::Cancelled {
            self.push_if_moved(&repo);
        }
        save_summary(&self.key);
        iteration
    }

    /// Every repo's HEAD: a commit anywhere is progress (stall detection).
    fn head(&mut self) -> Option<String> {
        let heads: Vec<String> = self.repos.iter().map(|r| self.head_of(r).unwrap_or_default()).collect();
        Some(heads.join(","))
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
        let (passed, total) = self.counts();
        total > 0 && passed == total
    }

    fn story_passed(&mut self, id: &str) -> bool {
        read_items(&self.ws_dir).1.iter().any(|i| i.id == id && i.done)
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

fn workspace_dir(workspace: &str) -> Result<(PathBuf, crate::workspace::Workspace), String> {
    let ws_dir = crate::workspace::ws_dir(workspace)?;
    let meta = crate::workspace::load_meta(&ws_dir)?;
    Ok((ws_dir, meta))
}

/// The plan's tasks for the Ralph view (as a PRD), plus what would stop a run.
pub fn state(workspace: &str) -> Result<RalphState, String> {
    let (ws_dir, meta) = workspace_dir(workspace)?;
    let exists = plan_path(&ws_dir).exists();
    let (context, items) = read_items(&ws_dir);
    let progress = progress_path(workspace).ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    Ok(RalphState {
        prd: (exists && !items.is_empty()).then(|| as_prd(&plan_title(&ws_dir), &context, &meta.branch, &items)),
        progress,
        run: snapshot(&super::run_key(workspace, PLAN_REPO)),
        prompt: prompts::PLAN_ITERATION.to_string(),
        prompt_custom: false,
        default_config: super::default_config(),
        plan_exists: exists,
        warnings: unknown_repos(&items, &meta.repos),
    })
}

pub fn start(app: AppHandle, workspace: String, config: RunConfig) -> Result<(), String> {
    let key = super::run_key(&workspace, PLAN_REPO);
    let (ws_dir, meta) = workspace_dir(&workspace)?;
    if !plan_path(&ws_dir).exists() {
        return Err("no PLAN.md yet: plan the feature first".into());
    }
    let (_, items) = read_items(&ws_dir);
    let pending: Vec<&PlanItem> = items.iter().filter(|i| !i.done).collect();
    if items.is_empty() {
        return Err("PLAN.md has no tasks (\"- [ ] …\" lines)".into());
    }
    if pending.is_empty() {
        return Err("every task of the plan is already done".into());
    }
    let problems = unknown_repos(&items, &meta.repos);
    if !problems.is_empty() {
        return Err(format!("fix the plan first: {}", problems.join("; ")));
    }
    let mut ai = Config::load()?.ai;
    if let Some(a) = config.agent.clone().filter(|a| !a.is_empty()) {
        ai.agent = a;
    }
    if let Some(m) = config.model.clone().filter(|m| !m.is_empty()) {
        ai.model = m;
    }
    let started = now();
    let id = format!("{started}-{PLAN_REPO}");
    let run_dir = runs_dir(&workspace)?.join(&id);
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    let progress = progress_path(&workspace)?;
    ensure_progress(&progress, &human_time(started))?;

    {
        let mut map = runs().lock().unwrap();
        if map.get(&key).is_some_and(|s| s.running) {
            return Err("Ralph is already running for this feature".into());
        }
        map.insert(
            key.clone(),
            RunState {
                id,
                repo: PLAN_REPO.into(),
                running: true,
                started_at: started,
                finished_at: None,
                reason: None,
                iteration: 0,
                current_story: pending.first().map(|i| i.id.clone()),
                passed: items.len() - pending.len(),
                total: items.len(),
                commits: 0,
                cost_usd: 0.0,
                limit_until: None,
                model: None,
                config: config.clone(),
                events: VecDeque::new(),
                stop: false,
                pause: false,
                dir: run_dir,
            },
        );
    }
    record(&app, &key, json!({"kind": "run_start", "agent": ai.agent, "model": ai.model}), |_| {});

    let mut env = PlanEnv {
        app: app.clone(),
        key: key.clone(),
        last_pushed: HashMap::new(),
        workspace: workspace.clone(),
        branch: meta.branch.clone(),
        repos: meta.repos.clone(),
        ws_dir,
        ai,
        cfg: config.clone(),
        started: Instant::now(),
        progress,
    };
    for repo in &meta.repos {
        let upstream = crate::proc::run("git", &["rev-parse", "@{u}"], Some(&env.dir_of(repo))).ok().map(|s| s.trim().to_string());
        env.last_pushed.insert(repo.clone(), upstream);
    }
    std::thread::spawn(move || {
        let summary = supervisor::supervise(&config.limits, &mut env);
        record(
            &app,
            &key,
            json!({"kind": "stopped", "reason": summary.reason, "iterations": summary.iterations, "limitWaits": summary.limit_waits}),
            |st| {
                st.running = false;
                st.finished_at = Some(now());
                st.reason = Some(summary.reason.clone());
                st.limit_until = None;
            },
        );
        save_summary(&key);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, repo: &str, done: bool) -> PlanItem {
        PlanItem { index: 0, id: id.into(), title: "t".into(), repo: repo.into(), description: String::new(), accept: vec![], done }
    }

    #[test]
    fn tasks_resolve_to_workspace_repos() {
        let repos = vec!["bet-app".to_string(), "bonus-engine-admin".to_string()];
        assert_eq!(repo_for(&item("T1", "Bet-App", false), &repos).as_deref(), Some("bet-app"));
        assert_eq!(repo_for(&item("T2", "", false), &repos), None);
        assert_eq!(repo_for(&item("T2", "", false), &repos[..1]).as_deref(), Some("bet-app"));
        let problems = unknown_repos(&[item("T3", "api", false), item("T4", "api", true), item("T5", "", false)], &repos);
        assert_eq!(problems.len(), 2);
        assert!(problems[0].contains("T3") && problems[1].contains("T5"));
    }
}
