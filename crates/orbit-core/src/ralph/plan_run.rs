// Ralph over the feature plan (PLAN.md at the workspace root): one run for
// the whole feature, each iteration a fresh agent on the next open task, in
// that task's repository. The plan is re-read every iteration (edits made
// between iterations count); Orbit marks a task done once the agent says
// so AND the task's repo got a commit, so the agent never edits the plan.
use super::prd::{Prd, Story};
use super::supervisor::{self, Iteration, RalphEnv};
use super::{
    agent_command, agent_run, cut_short, human_time, now, prompts, reconciled, record, record_ending, runs, runs_dir, save_summary, snapshot,
    watchdog, Activity, RalphState, RunConfig, RunState,
};
use crate::agent::plan::{parse_plan, PlanItem};
use crate::agent::{live_cmd, set_task_in_content, Access, PLAN_FILE};
use crate::config::{AiSettings, Config};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use crate::events::Events;

/// The run key's "repo" part for plan runs: `<workspace>/plan`.
pub const PLAN_REPO: &str = "plan";
const DONE_MARK: &str = "<task-done/>";

/// The reason inside the agent's `<task-blocked>…</task-blocked>`, if it gave one.
fn blocked_reason(text: &str) -> Option<String> {
    let rest = &text[text.rfind("<task-blocked>")? + "<task-blocked>".len()..];
    let reason = rest.split("</task-blocked>").next().unwrap_or(rest).trim();
    Some(if reason.is_empty() { "the agent gave no reason".to_string() } else { reason.chars().take(1200).collect() })
}

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
    app: Events,
    key: String,
    ws_dir: PathBuf,
    branch: String,
    repos: Vec<String>,
    ai: AiSettings,
    cfg: RunConfig,
    started: Instant,
    progress: PathBuf,
    last_pushed: HashMap<String, Option<String>>,
    /// Each repo's worktree.
    dirs: HashMap<String, PathBuf>,
    /// Tests: a script run instead of the agent.
    stub: Option<PathBuf>,
}

impl PlanEnv {
    fn dir_of(&self, repo: &str) -> PathBuf {
        self.dirs.get(repo).cloned().unwrap_or_else(|| self.ws_dir.join(repo))
    }

    fn unpushed(&self) -> u32 {
        self.repos.iter().map(|r| watchdog::unpushed(&self.dir_of(r))).sum()
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
                st.activity = Activity { last_event_at: Some(now()), ..Default::default() };
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

        let cmd = agent_command(&self.stub, || live_cmd(&self.ai, &prompt, Access::Full, None));
        let (outcome, run) = agent_run::run(&self.app, &self.key, n, cmd, &dir, &self.cfg, || watchdog::work_done(&dir, &head_before, &task.id, &[".orbit/"]));
        let full = run.full();
        let (cost, duration, turns, tokens) = (run.cost(), run.duration_ms(), run.turns(), run.tokens());
        let mut iteration = match &outcome {
            Err(e) => Iteration::Failed(e.clone()),
            Ok(o) if cut_short(o) => {
                record_ending(&self.app, &self.key, n, o, &self.cfg);
                let c = watchdog::check(&dir, &head_before, &task.id, &[".orbit/"], &run.last_message(), run.is_error());
                let it = reconciled(&self.app, &self.key, &task.id, &c, o.cancelled);
                if it == Iteration::Recovered {
                    match self.mark_done(task.index) {
                        Ok(()) => record(&self.app, &self.key, json!({"kind": "story_passed", "id": task.id}), |_| {}),
                        Err(e) => record(&self.app, &self.key, json!({"kind": "log", "text": format!("couldn't mark {} done in PLAN.md: {e}", task.id), "stderr": true}), |_| {}),
                    }
                }
                it
            }
            Ok(o) => run.exited(o),
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
        if iteration == Iteration::Done && !full.contains(DONE_MARK) {
            if let Some(why) = blocked_reason(&full) {
                iteration = Iteration::Blocked(format!("{}: {why}", task.id));
            }
        }
        record(
            &self.app,
            &self.key,
            json!({
                "kind": "iteration_end", "n": n, "outcome": format!("{iteration:?}"), "repo": repo,
                "commits": commits, "costUsd": cost, "tokens": tokens, "durationMs": duration, "turns": turns,
                "passed": passed, "total": total,
            }),
            |st| {
                st.commits += commits;
                st.cost_usd += cost.unwrap_or(0.0);
                st.tokens += tokens.unwrap_or(0);
                st.passed = passed;
                st.total = total;
                st.activity = Activity::default();
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

pub fn start(app: Events, workspace: String, config: RunConfig) -> Result<(), String> {
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
    let ai = super::run_ai(Config::load()?.ai, &config);
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
                tokens: 0,
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

    let dirs = repo_dirs(&workspace)?.into_iter().collect();
    let mut env = PlanEnv {
        app: app.clone(),
        key: key.clone(),
        last_pushed: HashMap::new(),
        dirs,
        stub: None,
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
        super::finish(&app, &key, &summary, env.unpushed());
    });
    Ok(())
}

/// Each workspace repo's worktree.
pub fn repo_dirs(workspace: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let (ws_dir, meta) = workspace_dir(workspace)?;
    Ok(meta
        .repos
        .iter()
        .map(|r| (r.clone(), crate::workspace::repo_path(workspace, r).unwrap_or_else(|_| ws_dir.join(r))))
        .collect())
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

    #[test]
    fn reads_the_blocked_reason() {
        assert_eq!(blocked_reason("notes\n<task-blocked>\ncriterion 2 can't pass: (a) relax it (b) drop it\n</task-blocked>").as_deref(), Some("criterion 2 can't pass: (a) relax it (b) drop it"));
        assert_eq!(blocked_reason("<task-blocked>cut off").as_deref(), Some("cut off"));
        assert_eq!(blocked_reason("all good\n<task-done/>"), None);
    }


    /// Stub agents are shell scripts, and the checks use process groups.
    #[cfg(unix)]
    mod guard {
        use super::*;

        use crate::events::NoEvents;
        use crate::ralph::watchdog::fixture::{self, git, group_empty, pgid, quick, RESULT_DONE};
        use crate::ralph::Limits;

        struct Case {
            root: PathBuf,
            repo: PathBuf,
            run_dir: PathBuf,
            env: PlanEnv,
        }

        /// A one-task plan over a scratch repo, with `agent` as the stub agent.
        fn case(name: &str, cfg: RunConfig, agent: &str) -> Case {
            let root = fixture::scratch(name);
            let ws = root.join("ws");
            let repo = ws.join("app");
            fixture::repo(&repo, &root.join("remote.git"));
            std::fs::write(ws.join(PLAN_FILE), "# Feature\n\n## Tasks\n\n- [ ] 1. First task — app\n").unwrap();
            let run_dir = root.join("run");
            std::fs::create_dir_all(&run_dir).unwrap();
            let key = format!("guard-test/{name}");
            fixture::start_run(&key, &run_dir, &cfg);
            let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
            let env = PlanEnv {
                app: Events::new(NoEvents),
                key,
                ws_dir: ws,
                branch: "feat/x".into(),
                repos: vec!["app".into()],
                ai: AiSettings::default(),
                cfg,
                started: Instant::now(),
                progress: root.join("progress.md"),
                last_pushed: HashMap::from([("app".to_string(), Some(head))]),
                dirs: HashMap::from([("app".to_string(), repo.clone())]),
                stub: Some(fixture::stub(&root, agent)),
            };
            Case { root, repo, run_dir, env }
        }

        impl Case {
            fn kinds(&self) -> Vec<String> {
                fixture::events(&self.run_dir).iter().map(|e| e["kind"].as_str().unwrap_or("").to_string()).collect()
            }
            fn event(&self, kind: &str) -> serde_json::Value {
                fixture::events(&self.run_dir).into_iter().find(|e| e["kind"] == kind).unwrap_or_default()
            }
            fn ticked(&self) -> bool {
                read_items(&self.env.ws_dir).1[0].done
            }
            fn pushed(&self) -> bool {
                let remote = git(&self.root.join("remote.git"), &["rev-parse", "feat/x"]);
                remote.trim() == git(&self.repo, &["rev-parse", "HEAD"]).trim()
            }
        }

        const COMMIT_T1: &str = "git commit --allow-empty -qm 'feat: [T1] - First task'";

        #[test]
        fn an_idle_iteration_is_killed_with_its_whole_tree() {
            let agent = "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}'\nsleep 1200 &\nsleep 1200";
            let mut c = case("idle", quick(), agent);
            let start = Instant::now();
            assert_eq!(c.env.run_iteration(1), Iteration::Stalled);
            assert!(start.elapsed() < Duration::from_secs(15), "{:?}", start.elapsed());
            assert_eq!(c.event("iteration_idle")["idleMin"], 0.05);
            assert_eq!(c.event("iteration_end")["outcome"], "Stalled");
            assert!(group_empty(&pgid(&c.root)));
            assert!(!c.ticked());
        }

        #[test]
        fn a_finished_agent_that_stays_alive_is_recovered() {
            let cfg = RunConfig { idle_timeout_min: 10.0, ..quick() };
            let mut c = case("grace", cfg, &format!("{COMMIT_T1}\n{RESULT_DONE}\nsleep 1200 &\nwait"));
            let start = Instant::now();
            assert_eq!(c.env.run_iteration(1), Iteration::Recovered);
            // Exit grace (1 s) plus the SIGTERM round trip, not the 10 min idle.
            assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
            assert_eq!(c.event("iteration_exit_grace")["graceSec"], 1);
            assert_eq!(c.event("reconcile")["passed"], true);
            assert_eq!(c.event("iteration_end")["outcome"], "Recovered");
            assert!(c.kinds().contains(&"story_passed".to_string()));
            assert!(c.ticked());
            assert!(c.pushed());
            assert!(group_empty(&pgid(&c.root)));
        }

        #[test]
        fn a_silent_agent_is_found_done_by_its_commit() {
            // No result line (omp, OpenCode): the commit alone says the task is done.
            let cfg = RunConfig { idle_timeout_min: 10.0, ..quick() };
            let mut c = case("silent", cfg, &format!("{COMMIT_T1}\necho 'Implemented it.'\nsleep 1200 &\nwait"));
            let start = Instant::now();
            assert_eq!(c.env.run_iteration(1), Iteration::Recovered);
            // A done poll, the exit grace and the SIGTERM round trip, not the 10 min idle.
            assert!(start.elapsed() < Duration::from_secs(12), "{:?}", start.elapsed());
            assert!(c.ticked());
            assert!(group_empty(&pgid(&c.root)));
        }

        #[test]
        fn a_task_stuck_without_its_commit_stalls_and_counts_toward_stall_after() {
            let agent = "echo wip > wip.txt\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"Still building\"}'\nsleep 1200 &\nwait";
            let mut c = case("stuck", quick(), agent);
            let limits = Limits { max_iterations: 5, stall_after: 2, ..Default::default() };
            let s = supervisor::supervise(&limits, &mut c.env);
            assert_eq!((s.reason, s.iterations), (supervisor::StopReason::Stalled, 2));
            let ends: Vec<_> = fixture::events(&c.run_dir).into_iter().filter(|e| e["kind"] == "iteration_end").map(|e| e["outcome"].clone()).collect();
            assert_eq!(ends, ["Stalled", "Stalled"]);
            assert_eq!(c.event("reconcile")["committed"], false);
            assert!(!c.ticked());
        }

        #[test]
        fn a_normal_iteration_is_unchanged() {
            let mut c = case("normal", quick(), &format!("{COMMIT_T1}\n{RESULT_DONE}"));
            assert_eq!(c.env.run_iteration(1), Iteration::Complete);
            let kinds = c.kinds();
            for k in ["iteration_idle", "iteration_exit_grace", "reconcile"] {
                assert!(!kinds.contains(&k.to_string()), "{k} in {kinds:?}");
            }
            assert!(c.ticked());
            assert!(c.pushed());
        }
    }
}
