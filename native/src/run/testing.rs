//! The Tests tab of the bottom panel, a small IntelliJ test runner: each
//! repo's runners (`orbit_core::testing`: cargo, CTest, vitest, jest) with
//! their tests in a tree (runner → suite → test), pass/fail/skip marks and
//! counts, Run All / Run Selected / Rerun Failed / Stop, and the selected
//! test's output (a runner or suite shows the whole run's).
use crate::icons;
use crate::logview::LogView;
use crate::probe::probe;
use crate::theme::{self, c, size};
use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{div, prelude::*, px, uniform_list, App, Context, Entity, Global, MouseButton, SharedString, Task, Window};
use gpui_component::tooltip::Tooltip;
use orbit_core::testing::{self, Case, Parser, Status};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_LOG: usize = 20_000;

#[derive(Clone, Default)]
struct Res {
    /// None while running.
    status: Option<Status>,
    output: Vec<String>,
    message: Option<String>,
}

enum Load {
    Loading,
    Ready,
    Error(String),
}

struct Project {
    repo: String,
    p: testing::Project,
    /// The repo as the workspace shows it (editor tabs) and its real folder.
    linked_root: PathBuf,
    root: PathBuf,
    load: Load,
    cases: Vec<Case>,
    results: HashMap<String, Res>,
    /// The last run's output.
    log: Vec<String>,
    log_total: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Sel {
    Project(usize),
    Suite(usize, String),
    Case(usize, String),
}

#[derive(Default, Clone, Copy)]
struct Counts {
    passed: usize,
    failed: usize,
    skipped: usize,
    running: usize,
    total: usize,
}

impl Counts {
    fn add(&mut self, r: Option<&Res>) {
        self.total += 1;
        match r.map(|r| r.status) {
            Some(Some(Status::Passed)) => self.passed += 1,
            Some(Some(Status::Failed)) => self.failed += 1,
            Some(Some(Status::Skipped)) => self.skipped += 1,
            Some(None) => self.running += 1,
            None => {}
        }
    }
}

struct Row {
    sel: Sel,
    depth: usize,
    label: String,
    probe: String,
    expanded: Option<bool>,
    /// A test's result, or a group's counts.
    status: Option<Option<Status>>,
    counts: Option<Counts>,
}

/// What one run does: per project, its tests (None: all).
type Plan = Vec<(usize, Option<Vec<Case>>)>;

pub struct Tests {
    loaded_for: Option<(String, Vec<String>)>,
    projects: Vec<Project>,
    /// Projects folded, and suites unfolded ("<project>/<suite>").
    collapsed: HashSet<usize>,
    expanded: HashSet<String>,
    selected: Option<Sel>,
    rows: Vec<Row>,
    output: Entity<LogView>,
    /// What the output shows, and the run it shows it from.
    shown: Option<(Sel, u64)>,
    run_seq: u64,
    running: Option<Arc<AtomicBool>>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    /// Run All was asked before the tests were listed.
    run_when_listed: bool,
    _load: Task<()>,
    _run: Task<()>,
}

struct GlobalTests(Entity<Tests>);

impl Global for GlobalTests {}

pub fn store(cx: &App) -> Entity<Tests> {
    cx.global::<GlobalTests>().0.clone()
}

pub fn init(cx: &mut App) {
    let entity = cx.new(|cx| Tests {
        loaded_for: None,
        projects: vec![],
        collapsed: HashSet::new(),
        expanded: HashSet::new(),
        selected: None,
        rows: vec![],
        output: cx.new(|cx| LogView::new("tests-output", MAX_LOG, cx)),
        shown: None,
        run_seq: 0,
        running: None,
        started: None,
        elapsed: None,
        run_when_listed: false,
        _load: Task::ready(()),
        _run: Task::ready(()),
    });
    // Test processes go with the app.
    cx.on_app_quit(|_| {
        testing::cancel("run");
        testing::cancel("list");
        async {}
    })
    .detach();
    cx.set_global(GlobalTests(entity));
}

/// The workspace and repos the run store follows.
fn workspace(cx: &App) -> Option<(String, Vec<String>)> {
    super::store(cx).read(cx).loaded_for.clone()
}

impl Tests {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn has_failed(&self) -> bool {
        self.projects.iter().any(|p| p.results.values().any(|r| r.status == Some(Status::Failed)))
    }

    /// Lists the tests again when the workspace changed.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let ws = workspace(cx);
        if ws != self.loaded_for {
            self.refresh(cx);
        }
    }

    /// Finds each repo's runners and lists their tests, one runner at a time.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let ws = workspace(cx);
        self.loaded_for = ws.clone();
        self.projects.clear();
        self.selected = None;
        self.shown = None;
        cx.notify();
        let Some((name, repos)) = ws else { return };
        let target = super::store(cx).read(cx).target.clone();
        self._load = cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    repos
                        .iter()
                        .filter_map(|repo| {
                            let root = orbit_core::run::root_of(&name, repo).ok()?;
                            let linked = orbit_core::workspace::repo_path(&name, repo).ok()?;
                            let preset = target.as_ref().filter(|t| &t.repo == repo).and_then(|t| t.preset.clone());
                            Some(testing::detect(&root, preset.as_deref()).into_iter().map(move |p| (repo.clone(), p, root.clone(), linked.clone())).collect::<Vec<_>>())
                        })
                        .flatten()
                        .collect::<Vec<_>>()
                })
                .await;
            let n = found.len();
            let ok = this.update(cx, |t, cx| {
                t.projects = found
                    .into_iter()
                    .map(|(repo, p, root, linked_root)| Project {
                        repo,
                        p,
                        linked_root,
                        root,
                        load: Load::Loading,
                        cases: vec![],
                        results: HashMap::new(),
                        log: vec![],
                        log_total: 0,
                    })
                    .collect();
                cx.notify();
            });
            if ok.is_err() {
                return;
            }
            for ix in 0..n {
                let Ok(p) = this.update(cx, |t, _| t.projects.get(ix).map(|p| p.p.clone())) else { return };
                let Some(p) = p else { return };
                let listed = cx.background_spawn(async move { testing::list(&p, "list") }).await;
                let _ = this.update(cx, |t, cx| {
                    let Some(proj) = t.projects.get_mut(ix) else { return };
                    match listed {
                        Ok(cases) => {
                            // Tests a run already reported stay.
                            let known: HashSet<String> = cases.iter().map(|c| c.id.clone()).collect();
                            let extra: Vec<Case> = proj.cases.drain(..).filter(|c| !known.contains(&c.id)).collect();
                            proj.cases = cases.into_iter().chain(extra).collect();
                            proj.load = Load::Ready;
                        }
                        Err(e) => proj.load = Load::Error(e),
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |t, cx| {
                if std::mem::take(&mut t.run_when_listed) {
                    t.run_all(cx);
                }
            });
        });
    }

    pub fn run_all(&mut self, cx: &mut Context<Self>) {
        if workspace(cx) != self.loaded_for || self.projects.is_empty() {
            self.run_when_listed = true;
            self.refresh(cx);
            return;
        }
        let plan = (0..self.projects.len()).map(|ix| (ix, None)).collect();
        self.start(plan, cx);
    }

    pub fn rerun_failed(&mut self, cx: &mut Context<Self>) {
        let plan: Plan = self
            .projects
            .iter()
            .enumerate()
            .filter_map(|(ix, p)| {
                let failed: Vec<Case> = p.cases.iter().filter(|c| p.results.get(&c.id).is_some_and(|r| r.status == Some(Status::Failed))).cloned().collect();
                (!failed.is_empty()).then_some((ix, Some(failed)))
            })
            .collect();
        self.start(plan, cx);
    }

    pub fn run_selected(&mut self, cx: &mut Context<Self>) {
        let plan = match self.selected.clone() {
            None => return self.run_all(cx),
            Some(Sel::Project(ix)) => vec![(ix, None)],
            Some(Sel::Suite(ix, suite)) => {
                let Some(p) = self.projects.get(ix) else { return };
                vec![(ix, Some(p.cases.iter().filter(|c| c.suite == suite).cloned().collect()))]
            }
            Some(Sel::Case(ix, id)) => {
                let Some(case) = self.projects.get(ix).and_then(|p| p.cases.iter().find(|c| c.id == id)) else { return };
                vec![(ix, Some(vec![case.clone()]))]
            }
        };
        self.start(plan, cx);
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = &self.running {
            flag.store(true, Ordering::SeqCst);
            testing::cancel("run");
        }
        if self.projects.iter().any(|p| matches!(p.load, Load::Loading)) {
            testing::cancel("list");
        }
        cx.notify();
    }

    fn start(&mut self, plan: Plan, cx: &mut Context<Self>) {
        if self.running.is_some() || plan.is_empty() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running = Some(cancelled.clone());
        self.started = Some(Instant::now());
        self.elapsed = None;
        self.run_seq += 1;
        // Everything the plan runs is marked running at once.
        for (ix, cases) in &plan {
            let Some(p) = self.projects.get_mut(*ix) else { continue };
            let ids: Vec<String> = match cases {
                Some(cases) => cases.iter().map(|c| c.id.clone()).collect(),
                None => p.cases.iter().map(|c| c.id.clone()).collect(),
            };
            for id in ids {
                p.results.insert(id, Res::default());
            }
            p.log.clear();
            p.log_total = 0;
        }
        cx.notify();
        self._run = cx.spawn(async move |this, cx| {
            for (ix, cases) in plan {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(Some(p)) = this.update(cx, |t, _| t.projects.get(ix).map(|p| p.p.clone())) else { break };
                // Only the asked tests take results: a filtered vitest file reports the others skipped.
                let wanted: Option<HashSet<String>> = cases.as_ref().map(|c| c.iter().map(|c| c.id.clone()).collect());
                let mut parser = Parser::new(p.kind);
                let (tx, mut rx) = mpsc::unbounded::<String>();
                let dir = p.dir.clone();
                let work = cx.background_spawn(async move {
                    testing::run(&p, cases.as_deref(), "run", |l| {
                        let _ = tx.unbounded_send(l);
                    })
                });
                while let Some(first) = rx.next().await {
                    let mut batch = vec![first];
                    while let Ok(l) = rx.try_recv() {
                        batch.push(l);
                    }
                    // A Stop that came before the process was registered missed it.
                    if cancelled.load(Ordering::SeqCst) {
                        testing::cancel("run");
                    }
                    for l in &batch {
                        parser.feed(l);
                    }
                    if this.update(cx, |t, cx| t.apply(ix, batch, &parser, wanted.as_ref(), cx)).is_err() {
                        return;
                    }
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                }
                let end = work.await;
                let mut lines = vec![];
                match &end {
                    Ok(e) => {
                        if let Some(report) = &e.report {
                            parser.report(report, &dir);
                        }
                        if e.cancelled {
                            lines.push("Tests stopped".to_string());
                        }
                    }
                    Err(e) => lines.push(e.clone()),
                }
                let _ = this.update(cx, |t, cx| {
                    t.apply(ix, lines, &parser, wanted.as_ref(), cx);
                    // Tests that never reported didn't run.
                    if let Some(proj) = t.projects.get_mut(ix) {
                        proj.results.retain(|_, r| r.status.is_some());
                    }
                });
            }
            let _ = this.update(cx, |t, cx| {
                t.running = None;
                t.elapsed = t.started.map(|s| s.elapsed());
                cx.notify();
            });
        });
    }

    /// New output lines of project `ix` and the parser's results so far.
    fn apply(&mut self, ix: usize, lines: Vec<String>, parser: &Parser, wanted: Option<&HashSet<String>>, cx: &mut Context<Self>) {
        let Some(p) = self.projects.get_mut(ix) else { return };
        p.log_total += lines.len();
        p.log.extend(lines);
        if p.log.len() > MAX_LOG {
            p.log.drain(..p.log.len() - MAX_LOG);
        }
        for o in &parser.results {
            if wanted.is_some_and(|w| !w.contains(&o.case.id)) {
                continue;
            }
            if !p.cases.iter().any(|c| c.id == o.case.id) {
                p.cases.push(o.case.clone());
            }
            let r = p.results.entry(o.case.id.clone()).or_default();
            if o.status.is_some() {
                r.status = o.status;
            }
            r.output.clone_from(&o.output);
            r.message.clone_from(&o.message);
        }
        cx.notify();
    }

    fn select(&mut self, sel: Sel, cx: &mut Context<Self>) {
        self.selected = Some(sel);
        cx.notify();
    }

    fn toggle(&mut self, sel: &Sel, cx: &mut Context<Self>) {
        match sel {
            Sel::Project(ix) => {
                if !self.collapsed.remove(ix) {
                    self.collapsed.insert(*ix);
                }
            }
            Sel::Suite(ix, s) => {
                let key = format!("{ix}/{s}");
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
            }
            Sel::Case(..) => {}
        }
        cx.notify();
    }

    /// Opens the selected test's source (CTest's DEF_SOURCE_LINE, a JS test's file).
    fn open(&self, sel: &Sel, cx: &mut App) {
        let Sel::Case(ix, id) = sel else { return };
        let Some(p) = self.projects.get(*ix) else { return };
        let Some(case) = p.cases.iter().find(|c| &c.id == id) else { return };
        let (path, line) = match (&case.location, &case.file) {
            (Some(loc), _) => {
                let (file, line) = loc.rsplit_once(':').map_or((loc.as_str(), 1), |(f, l)| (f, l.parse().unwrap_or(1)));
                (p.linked_root.join(file), line)
            }
            (None, Some(file)) => (PathBuf::from(file).strip_prefix(&p.root).map_or_else(|_| PathBuf::from(file), |r| p.linked_root.join(r)), 1),
            _ => return,
        };
        super::open_location(path, line, 1, cx);
    }

    fn build_rows(&mut self) {
        let mut rows = vec![];
        for (ix, p) in self.projects.iter().enumerate() {
            let mut suites: Vec<(&str, Counts)> = vec![];
            let mut all = Counts::default();
            for c in &p.cases {
                let r = p.results.get(&c.id);
                all.add(r);
                match suites.iter_mut().find(|(s, _)| *s == c.suite) {
                    Some((_, n)) => n.add(r),
                    None => {
                        let mut n = Counts::default();
                        n.add(r);
                        suites.push((&c.suite, n));
                    }
                }
            }
            let open = !self.collapsed.contains(&ix);
            let label = match &p.load {
                Load::Loading => format!("{} · {} (listing…)", p.repo, p.p.label),
                Load::Error(_) => format!("{} · {} (not listed)", p.repo, p.p.label),
                Load::Ready => format!("{} · {}", p.repo, p.p.label),
            };
            rows.push(Row { sel: Sel::Project(ix), depth: 0, label, probe: format!("test:{}/{}", p.repo, p.p.label), expanded: Some(open), status: None, counts: Some(all) });
            if !open {
                continue;
            }
            for (suite, counts) in suites {
                let open = self.expanded.contains(&format!("{ix}/{suite}"));
                rows.push(Row { sel: Sel::Suite(ix, suite.to_string()), depth: 1, label: suite.to_string(), probe: format!("test:{suite}"), expanded: Some(open), status: None, counts: Some(counts) });
                if !open {
                    continue;
                }
                for c in p.cases.iter().filter(|c| c.suite == suite) {
                    let status = p.results.get(&c.id).map(|r| r.status);
                    rows.push(Row { sel: Sel::Case(ix, c.id.clone()), depth: 2, label: c.label.clone(), probe: format!("test:{suite}/{}", c.label), expanded: None, status, counts: None });
                }
            }
        }
        self.rows = rows;
    }

    fn totals(&self) -> Counts {
        let mut n = Counts::default();
        for p in &self.projects {
            for c in &p.cases {
                n.add(p.results.get(&c.id));
            }
        }
        n
    }

    /// The selection's lines for the output view: a test's output and failure,
    /// else its runner's whole log.
    fn sync_output(&mut self, cx: &mut Context<Self>) {
        let sel = self.selected.clone().unwrap_or(Sel::Project(0));
        let key = (sel.clone(), self.run_seq);
        if self.shown.as_ref() != Some(&key) {
            self.shown = Some(key);
            self.output.update(cx, |v, cx| v.clear(cx));
        }
        let ix = match &sel {
            Sel::Project(ix) | Sel::Suite(ix, _) | Sel::Case(ix, _) => *ix,
        };
        let Some(p) = self.projects.get(ix) else { return };
        let (total, lines): (usize, Vec<(String, u32)>) = match &sel {
            Sel::Case(_, id) => {
                let r = p.results.get(id);
                let mut lines: Vec<(String, u32)> = r.map(|r| r.output.iter().map(|l| (l.clone(), theme::TEXT)).collect()).unwrap_or_default();
                if let Some(m) = r.and_then(|r| r.message.as_ref()) {
                    lines.extend(m.lines().map(|l| (l.to_string(), theme::DANGER)));
                }
                if lines.is_empty() {
                    let text = match r.map(|r| r.status) {
                        Some(None) => "Running…",
                        Some(Some(_)) => "No output",
                        None => "Not run yet",
                    };
                    lines.push((text.to_string(), theme::TEXT_FAINT));
                }
                (lines.len(), lines)
            }
            _ => {
                if let (Load::Error(e), true) = (&p.load, p.log.is_empty()) {
                    let lines: Vec<(String, u32)> = e.lines().map(|l| (l.to_string(), theme::DANGER)).collect();
                    (lines.len(), lines)
                } else {
                    let fresh = p.log_total.saturating_sub(self.output.read(cx).seen()).min(p.log.len());
                    (p.log_total, p.log[p.log.len() - fresh..].iter().map(|l| (l.clone(), theme::TEXT)).collect())
                }
            }
        };
        self.output.update(cx, |v, cx| v.sync(total, &lines, |(t, color)| (t.clone(), *color, None), cx));
    }

    /// The state for the self-test dump.
    pub fn dump(&self, cx: &App) -> String {
        use std::fmt::Write as _;
        let n = self.totals();
        let mut out = format!(
            "--- tests: {} projects, {} tests, {} passed, {} failed, {} skipped, {} running{}\n",
            self.projects.len(),
            n.total,
            n.passed,
            n.failed,
            n.skipped,
            n.running,
            if self.is_running() { " (run in progress)" } else { "" }
        );
        for p in &self.projects {
            let state = match &p.load {
                Load::Loading => "listing".to_string(),
                Load::Ready => "ready".into(),
                Load::Error(e) => format!("error: {}", e.lines().last().unwrap_or_default()),
            };
            let suites: HashSet<&str> = p.cases.iter().map(|c| c.suite.as_str()).collect();
            let _ = writeln!(out, "  {} · {} [{}] {} tests in {} suites, {} log lines", p.repo, p.p.label, state, p.cases.len(), suites.len(), p.log_total);
            for c in p.cases.iter().filter(|c| p.results.get(&c.id).is_some_and(|r| r.status == Some(Status::Failed))).take(8) {
                let _ = writeln!(out, "    failed {}", c.id);
            }
        }
        let _ = writeln!(out, "  selected {:?}, {} rows", self.selected, self.rows.len());
        let _ = writeln!(out, "  {}", self.output.read(cx).dump());
        out
    }
}

fn tool(id: &'static str, glyph: &'static str, hint: &'static str, enabled: bool, cx: &mut Context<Tests>, on: fn(&mut Tests, &mut Context<Tests>)) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))))
        .when(!enabled, |d| d.opacity(0.35))
        .child(match glyph {
            "refresh" => icons::orbit_icon(glyph, 13., c(theme::TEXT_DIM)),
            _ => icons::ui_icon(glyph, 13., c(theme::TEXT_DIM)),
        })
        .child(probe(id))
        .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
        .on_click(cx.listener(move |t, _, _, cx| {
            if enabled {
                on(t, cx);
            }
        }))
}

fn mark(status: Option<Status>, id: SharedString) -> gpui::AnyElement {
    match status {
        Some(Status::Passed) => icons::orbit_icon("check", 11., c(theme::OK)).into_any_element(),
        Some(Status::Failed) => icons::orbit_icon("x", 11., c(theme::DANGER)).into_any_element(),
        Some(Status::Skipped) => icons::orbit_icon("circle", 9., c(theme::WARN)).into_any_element(),
        None => crate::workspace_ui::kit::spinner(id, 11., c(theme::INFO)).into_any_element(),
    }
}

impl Render for Tests {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        self.build_rows();
        self.sync_output(cx);
        let running = self.is_running();
        let busy = running || self.projects.iter().any(|p| matches!(p.load, Load::Loading));
        let n = self.totals();
        let summary = if running {
            format!("Running… {}/{}", n.total - n.running, n.total)
        } else if n.passed + n.failed + n.skipped == 0 {
            format!("{} tests", n.total)
        } else {
            let mut s = format!("{} passed", n.passed);
            if n.failed > 0 {
                s += &format!(" · {} failed", n.failed);
            }
            if n.skipped > 0 {
                s += &format!(" · {} skipped", n.skipped);
            }
            s + &format!(" of {}", n.total) + &self.elapsed.map(|d| format!(" · {:.1}s", d.as_secs_f32())).unwrap_or_default()
        };
        let has_failed = self.has_failed();
        let toolbar = div()
            .h(px(30.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(tool("tests-run-all", "play", "Run All Tests", !running && !self.projects.is_empty(), cx, Tests::run_all))
            .child(tool("tests-run-selected", "play-fill", "Run Selected", !running && self.selected.is_some(), cx, Tests::run_selected))
            .child(tool("tests-rerun-failed", "rotate-ccw", "Rerun Failed Tests", !running && has_failed, cx, Tests::rerun_failed))
            .child(tool("tests-stop", "stop", "Stop", busy, cx, Tests::stop))
            .child(tool("tests-refresh", "refresh", "Refresh Tests", !busy, cx, Tests::refresh))
            .child(
                div()
                    .ml_2()
                    .text_xs()
                    .text_color(c(if n.failed > 0 { theme::DANGER } else if n.passed > 0 && !running { theme::OK } else { theme::TEXT_FAINT }))
                    .child(summary),
            );
        let tree = if self.rows.is_empty() {
            let text = if self.loaded_for.is_none() { "Open a workspace to see its tests" } else if busy { "Looking for tests…" } else { "No tests found (cargo, CTest, vitest or jest)" };
            div().size_full().flex().items_center().justify_center().text_color(c(theme::TEXT_FAINT)).child(text).into_any_element()
        } else {
            uniform_list(
                "tests-tree",
                self.rows.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|ix| {
                            let row = this.rows.get(ix)?;
                            let selected = this.selected.as_ref() == Some(&row.sel);
                            let chevron = row.expanded.map(|open| {
                                let icon = icons::orbit_icon("chevron-right", 7., c(theme::TEXT_FAINT));
                                if open {
                                    icon.with_transformation(gpui::Transformation::rotate(gpui::percentage(0.25)))
                                } else {
                                    icon
                                }
                            });
                            let status = match (row.status, row.counts) {
                                (Some(s), _) => Some(s),
                                (None, Some(n)) if n.running > 0 => Some(None),
                                (None, Some(n)) if n.failed > 0 => Some(Some(Status::Failed)),
                                (None, Some(n)) if n.passed > 0 && n.passed + n.skipped == n.total => Some(Some(Status::Passed)),
                                _ => None,
                            };
                            let counts = row.counts.map(|n| {
                                let mut s = String::new();
                                if n.failed > 0 {
                                    s += &format!("{} failed · ", n.failed);
                                }
                                if n.passed + n.failed + n.skipped > 0 {
                                    s += &format!("{}/{}", n.passed, n.total);
                                } else {
                                    s += &n.total.to_string();
                                }
                                s
                            });
                            let (sel, toggle) = (row.sel.clone(), row.sel.clone());
                            Some(
                                div()
                                    .id(ix)
                                    .relative()
                                    .h(px(22.))
                                    .w_full()
                                    .pl(px(6. + row.depth as f32 * 14.))
                                    .pr_2()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .text_color(c(theme::TEXT_DIM))
                                    .hover(|s| s.bg(c(theme::PANEL_HOVER)).text_color(c(theme::TEXT)))
                                    .when(selected, |d| d.bg(c(theme::ACCENT_SOFT)).text_color(c(theme::TEXT)))
                                    .child(
                                        div()
                                            .id(("tests-chevron", ix))
                                            .w(px(10.))
                                            .flex_none()
                                            .flex()
                                            .justify_center()
                                            .children(chevron)
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |t, _, _, cx| {
                                                cx.stop_propagation();
                                                t.toggle(&toggle, cx);
                                            })),
                                    )
                                    .child(div().w(px(12.)).flex_none().flex().justify_center().children(status.map(|s| mark(s, SharedString::from(format!("tests-spin-{ix}"))))))
                                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(row.label.clone()))
                                    .children(counts.map(|s| div().flex_none().text_xs().text_color(c(theme::TEXT_FAINT)).child(s)))
                                    .child(probe(row.probe.clone()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |t, ev: &gpui::MouseDownEvent, _, cx| {
                                            // Double click on the selected row: a group folds, a test opens its source.
                                            let again = ev.click_count >= 2 && t.selected.as_ref() == Some(&sel);
                                            match (again, &sel) {
                                                (false, _) => t.select(sel.clone(), cx),
                                                (true, Sel::Case(..)) => t.open(&sel, cx),
                                                (true, _) => t.toggle(&sel, cx),
                                            }
                                        }),
                                    ),
                            )
                        })
                        .collect()
                }),
            )
            .size_full()
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(size::TAB_FONT))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().w(gpui::relative(0.42)).flex_none().h_full().border_r_1().border_color(c(theme::BORDER)).child(tree))
                    .child(div().flex_1().min_w_0().h_full().child(self.output.clone())),
            )
    }
}
