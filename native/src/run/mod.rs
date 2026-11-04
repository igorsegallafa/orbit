//! Build and Run (`src/lib/runStore.ts`): the selected run configuration
//! per workspace (from each repo's `.orbit/run.json`, through
//! `orbit_core::run`), the current build with its output and problems, and
//! the running program's terminal. One build and one run at a time, as in
//! Rider. The title bar widget is `widget.rs`, the bottom panel `panel.rs`.
pub mod problems;
pub mod saved;
pub mod script;
mod panel;
mod widget;

use crate::app::OrbitApp;
use crate::commands::{self, Command, RunCommand};
use crate::debug::{self, DebugStatus};
use crate::sessions::SessionMeta;
use crate::terminal::{self, TerminalView};
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, Context, Entity, Global, IntoElement, KeyBinding, ParentElement,
    SharedString, Styled, Subscription, Task, WeakEntity, Window,
};
use orbit_core::events::Events;
use orbit_core::run::ResolvedRun;
use problems::{Problem, Severity};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use panel::PanelView;
pub use saved::RunTarget;

const MAX_LINES: usize = 5000;

/// A repo's run configurations, resolved.
#[derive(Clone)]
pub struct Setup {
    /// The repo folder builds run in (links resolved).
    pub root: PathBuf,
    /// The same folder as the workspace shows it (editor tabs use it).
    pub linked_root: PathBuf,
    pub configs: Vec<ResolvedRun>,
    pub presets: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildStatus {
    Running,
    Ok,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stream {
    Out,
    Err,
    /// Orbit's own lines: the command, timing.
    Info,
}

pub struct OutputLine {
    pub stream: Stream,
    pub text: SharedString,
    /// The problem this line reports (index into `Build::problems`).
    pub problem: Option<usize>,
}

pub struct BuildProblem {
    pub problem: Problem,
    /// The repo file as the editor opens it (workspace path).
    pub open: Option<PathBuf>,
}

pub struct Build {
    pub key: String,
    pub target: RunTarget,
    pub status: BuildStatus,
    pub lines: Vec<OutputLine>,
    pub problems: Vec<BuildProblem>,
    keys: HashMap<String, usize>,
    pub started: Instant,
    pub elapsed: Option<Duration>,
    /// Where relative paths in compiler messages start (the build folder).
    base_dir: Option<String>,
    root: PathBuf,
    linked_root: PathBuf,
}

impl Build {
    pub fn counts(&self) -> (usize, usize) {
        let errors = self.problems.iter().filter(|p| p.problem.severity == Severity::Error).count();
        (errors, self.problems.len() - errors)
    }
}

pub struct RunProc {
    pub view: Entity<TerminalView>,
    pub name: String,
    pub exited: bool,
    _watch: Subscription,
}

pub struct RunStore {
    pub app: WeakEntity<OrbitApp>,
    pub window: Option<AnyWindowHandle>,
    /// The workspace and repos the setups were loaded for.
    loaded_for: Option<(String, Vec<String>)>,
    pub setups: Vec<(String, Setup)>,
    pub target: Option<RunTarget>,
    pub loading: bool,
    pub build: Option<Build>,
    pub run: Option<RunProc>,
    pub view: PanelView,
    pub panel: Option<Entity<panel::Panel>>,
    _load: Task<()>,
    _build: Task<()>,
    _run_after: Task<()>,
}

struct GlobalRun(Entity<RunStore>);

impl Global for GlobalRun {}

pub fn store(cx: &App) -> Entity<RunStore> {
    cx.global::<GlobalRun>().0.clone()
}

fn building(cx: &App) -> bool {
    store(cx).read(cx).is_building()
}

fn has_program(cx: &App) -> bool {
    store(cx).read(cx).config().is_some_and(|c| c.program.is_some())
}

fn paused(cx: &App) -> bool {
    debug::store(cx).read(cx).status == DebugStatus::Stopped
}

/// Commands, keys and the stores. Called from `main.rs` after the editor's keys.
pub fn init(cx: &mut App) {
    let entity = cx.new(|_| RunStore {
        app: WeakEntity::new_invalid(),
        window: None,
        loaded_for: None,
        setups: vec![],
        target: None,
        loading: false,
        build: None,
        run: None,
        view: PanelView::Build,
        panel: None,
        _load: Task::ready(()),
        _build: Task::ready(()),
        _run_after: Task::ready(()),
    });
    cx.set_global(GlobalRun(entity));
    debug::init(cx);

    let ready = |_: &OrbitApp, cx: &App| store(cx).read(cx).config().is_some() && !building(cx);
    let live = |_: &OrbitApp, cx: &App| debug::store(cx).read(cx).is_live();
    let with_debug = |f: fn(&mut debug::DebugStore, &mut Context<debug::DebugStore>)| {
        move |_: &mut OrbitApp, _: &mut Window, cx: &mut Context<OrbitApp>| debug::store(cx).update(cx, f)
    };
    for command in [
        Command::new("run.build", "Build").keys(["ctrl-f9"]).when(ready).run(|a, _, cx| {
            a.layout.panel_open = true;
            store(cx).update(cx, |s, cx| {
                s.start_build(cx);
            });
        }),
        Command::new("run.run", "Run").keys(["shift-f10"]).when(move |a, cx| ready(a, cx) && has_program(cx)).run(|a, _, cx| {
            a.layout.panel_open = true;
            store(cx).update(cx, |s, cx| s.build_and_run(cx));
        }),
        Command::new("run.debug", "Debug")
            .keys(["shift-f9"])
            .when(move |a, cx| ready(a, cx) && has_program(cx) && !live(a, cx))
            .run(|a, _, cx| {
                a.layout.panel_open = true;
                debug::store(cx).update(cx, |d, cx| d.start(cx));
            }),
        Command::new("run.stop", "Stop")
            .keys(["ctrl-f2"])
            .when(move |a, cx| building(cx) || store(cx).read(cx).is_running() || live(a, cx))
            .run(|_, _, cx| {
                if debug::store(cx).read(cx).is_live() {
                    debug::store(cx).update(cx, |d, cx| d.stop(cx));
                } else {
                    store(cx).update(cx, |s, cx| s.stop(cx));
                }
            }),
        Command::new("debug.pause", "Pause Program")
            .when(|_, cx| debug::store(cx).read(cx).status == DebugStatus::Running)
            .run(with_debug(debug::DebugStore::pause)),
        Command::new("debug.resume", "Resume Program").keys(["f9"]).when(|_, cx| paused(cx)).run(with_debug(debug::DebugStore::resume)),
        Command::new("debug.stepOver", "Step Over").keys(["f8"]).when(|_, cx| paused(cx)).run(with_debug(debug::DebugStore::step_over)),
        Command::new("debug.stepInto", "Step Into").keys(["f7"]).when(|_, cx| paused(cx)).run(with_debug(debug::DebugStore::step_into)),
        Command::new("debug.stepOut", "Step Out").keys(["shift-f8"]).when(|_, cx| paused(cx)).run(with_debug(debug::DebugStore::step_out)),
        Command::new("debug.toggleBreakpoint", "Toggle Breakpoint")
            .keys(["ctrl-f8"])
            .when(|a, _| a.active_editor().is_some())
            .run(|a, _, cx| {
                let Some(e) = a.active_editor() else { return };
                let (path, (line, _)) = (e.read(cx).path.clone(), e.read(cx).cursor(cx));
                debug::store(cx).update(cx, |d, cx| d.toggle_breakpoint(path, line, cx));
            }),
        Command::new("run.showPanel", "Show Build Output").run(|a, _, cx| {
            a.layout.panel_open = true;
            store(cx).update(cx, |s, cx| s.set_view(PanelView::Build, cx));
        }),
        Command::new("run.showProblems", "Show Build Problems").run(|a, _, cx| {
            a.layout.panel_open = true;
            store(cx).update(cx, |s, cx| s.set_view(PanelView::Problems, cx));
        }),
    ] {
        commands::register(cx, command);
    }
    // In the editor F8/Shift+F8 are next/previous problem: while paused the
    // stepping keys come first (bound later in the same context), otherwise
    // they fall through to the editor's.
    let editor = Some("OrbitEditor");
    let key = |id: &str| RunCommand { id: id.to_string().into(), from_key: true };
    cx.bind_keys([
        KeyBinding::new("f8", key("debug.stepOver"), editor),
        KeyBinding::new("shift-f8", key("debug.stepOut"), editor),
    ]);
}

/// Puts the run widget in the titlebar and makes the panel. Called once the
/// root view exists (`main.rs`).
pub fn attach(app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let weak = cx.weak_entity();
    let handle = window.window_handle();
    store(cx).update(cx, |s, _| {
        s.app = weak.clone();
        s.window = Some(handle);
    });
    let widget = cx.new(|cx| widget::RunWidget::new(weak.clone(), cx));
    app.titlebar_items.push(widget.into());
    let inbox = cx.new(|cx| crate::sessions::inbox::InboxButton::new(weak.clone(), cx));
    app.titlebar_items.push(inbox.into());
    let panel = cx.new(|cx| panel::Panel::new(weak, window, cx));
    store(cx).update(cx, |s, _| s.panel = Some(panel));
}

/// The bottom panel (`BottomPanel.tsx`), in the shell's slot.
pub fn render_panel(app: &OrbitApp, cx: &App) -> AnyElement {
    let body = store(cx).read(cx).panel.clone();
    gpui::div()
        .h(gpui::px(app.layout.panel_height))
        .flex_none()
        .flex()
        .flex_col()
        .children(body)
        .into_any_element()
}

/// Opens the panel on `view` (from outside the run store).
pub fn show(view: PanelView, cx: &mut App) {
    store(cx).update(cx, |s, cx| s.reveal(view, cx));
}

/// Deferred: callers may be inside the root view's update.
fn open_panel(app: WeakEntity<OrbitApp>, cx: &mut App) {
    cx.defer(move |cx| {
        let _ = app.update(cx, |a, cx| {
            if !a.layout.panel_open {
                a.layout.panel_open = true;
                cx.notify();
            }
        });
    });
}

/// Opens a file at a 1-based line and column in an editor tab.
pub fn open_location(path: PathBuf, line: u32, column: u32, cx: &mut App) {
    let s = store(cx).read(cx);
    let (app, window) = (s.app.clone(), s.window);
    let Some(window) = window else { return };
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            let _ = app.update(cx, |a, cx| {
                if let Some(editor) = a.open_file(path, false, window, cx) {
                    let p = lsp_types::Position::new(line.saturating_sub(1), column.saturating_sub(1));
                    editor.update(cx, |e, cx| e.reveal(lsp_types::Range::new(p, p), window, cx));
                }
            });
        });
    });
}

/// Build problems as editor diagnostics (source "build"), next to the language server's.
pub fn apply_problems(cx: &mut App) {
    let by_file: HashMap<PathBuf, Vec<Problem>> = {
        let s = store(cx).read(cx);
        let mut map: HashMap<PathBuf, Vec<Problem>> = HashMap::new();
        for p in s.build.iter().flat_map(|b| &b.problems) {
            if let Some(path) = &p.open {
                map.entry(path.clone()).or_default().push(p.problem.clone());
            }
        }
        map
    };
    for e in debug::gutter::editors(cx) {
        let path = e.read(cx).path.clone();
        let list = by_file.get(&path).cloned().unwrap_or_default();
        if list.is_empty() && e.read(cx).build_diagnostics.is_empty() {
            continue;
        }
        e.update(cx, |e, cx| {
            use gpui_component::input::RopeExt;
            let text = e.state.read(cx).text().clone();
            let lines = text.lines_len().max(1);
            let diagnostics = list
                .into_iter()
                .map(|p| {
                    let row = (p.line.max(1) as usize - 1).min(lines - 1);
                    let len = text.slice_line(row).to_string().trim_end_matches(['\n', '\r']).chars().count() as u32;
                    let col = p.column.saturating_sub(1).min(len);
                    lsp_types::Diagnostic {
                        range: lsp_types::Range::new(lsp_types::Position::new(row as u32, col), lsp_types::Position::new(row as u32, len)),
                        severity: Some(match p.severity {
                            Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
                            Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                        }),
                        code: p.code.map(lsp_types::NumberOrString::String),
                        source: Some("build".into()),
                        message: p.message,
                        ..Default::default()
                    }
                })
                .collect();
            e.set_build_diagnostics(diagnostics, cx);
        });
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

impl RunStore {
    pub fn is_building(&self) -> bool {
        self.build.as_ref().is_some_and(|b| b.status == BuildStatus::Running)
    }

    pub fn is_running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| !r.exited)
    }

    pub fn setup(&self, repo: &str) -> Option<&Setup> {
        self.setups.iter().find(|(r, _)| r == repo).map(|(_, s)| s)
    }

    pub fn config(&self) -> Option<&ResolvedRun> {
        let t = self.target.as_ref()?;
        self.setup(&t.repo)?.configs.iter().find(|c| c.name == t.name)
    }

    /// The target, its configuration and the repo's (real, workspace) roots.
    pub fn selected(&self) -> Option<(RunTarget, ResolvedRun, (PathBuf, PathBuf))> {
        let t = self.target.clone()?;
        let setup = self.setup(&t.repo)?;
        let config = setup.configs.iter().find(|c| c.name == t.name)?.clone();
        Some((t, config, (setup.root.clone(), setup.linked_root.clone())))
    }

    pub fn set_view(&mut self, view: PanelView, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    /// Shows `view` and opens the panel.
    pub fn reveal(&mut self, view: PanelView, cx: &mut Context<Self>) {
        self.set_view(view, cx);
        open_panel(self.app.clone(), cx);
    }

    // ---------- selection ----------

    /// Follows the root view's workspace: loads its repos' configurations.
    pub fn sync_workspace(&mut self, ws: Option<(String, Vec<String>)>, cx: &mut Context<Self>) {
        if self.loaded_for == ws {
            return;
        }
        self.loaded_for = ws.clone();
        self.setups.clear();
        self.target = None;
        cx.notify();
        if let Some((name, _)) = ws {
            let preferred = saved::load().selection.get(&name).cloned();
            self.reload(preferred, cx);
        }
    }

    fn reload(&mut self, preferred: Option<RunTarget>, cx: &mut Context<Self>) {
        let Some((ws, repos)) = self.loaded_for.clone() else { return };
        self.loading = true;
        let want = preferred.clone();
        let name = ws.clone();
        let found = cx.background_spawn(async move {
            repos
                .iter()
                .filter_map(|repo| {
                    let preset = want.as_ref().filter(|t| &t.repo == repo).and_then(|t| t.preset.clone());
                    let s = orbit_core::run::setup(&name, repo, preset.as_deref()).ok()?;
                    (!s.configs.is_empty()).then(|| {
                        let setup = Setup {
                            root: s.root.into(),
                            linked_root: s.linked_root.into(),
                            configs: s.configs,
                            presets: s.presets,
                        };
                        (repo.clone(), setup)
                    })
                })
                .collect::<Vec<_>>()
        });
        self._load = cx.spawn(async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |s, cx| {
                if s.loaded_for.as_ref().map(|l| &l.0) != Some(&ws) {
                    return;
                }
                let valid = |t: &RunTarget| found.iter().any(|(r, s)| *r == t.repo && s.configs.iter().any(|c| c.name == t.name));
                s.target = preferred.filter(valid).or_else(|| {
                    let (repo, setup) = found.first()?;
                    Some(RunTarget { workspace: ws.clone(), repo: repo.clone(), name: setup.configs[0].name.clone(), preset: None })
                });
                s.setups = found;
                s.loading = false;
                cx.notify();
            });
        });
    }

    /// Picks a configuration (and preset) and remembers it for the workspace.
    pub fn choose(&mut self, next: RunTarget, cx: &mut Context<Self>) {
        let saved = next.clone();
        saved::update(|s| {
            s.selection.insert(saved.workspace.clone(), saved);
        });
        let reload = self.target.as_ref().is_none_or(|t| t.repo != next.repo || t.preset != next.preset);
        if reload {
            self.reload(Some(next), cx);
        } else {
            self.target = Some(next);
            cx.notify();
        }
    }

    // ---------- build ----------

    /// Builds the selected configuration; the receiver gets whether it succeeded.
    pub fn start_build(&mut self, cx: &mut Context<Self>) -> Option<oneshot::Receiver<bool>> {
        if self.is_building() {
            return None;
        }
        let (target, config, (root, linked_root)) = self.selected()?;
        let key = format!("{}/{}/{}", target.workspace, target.repo, target.name);
        let (tx, mut rx) = mpsc::unbounded::<(Stream, String)>();
        let events = Events::new(move |event: &str, payload: Value| {
            if event == "run-output" {
                let stream = match payload["stream"].as_str() {
                    Some("err") => Stream::Err,
                    Some("info") => Stream::Info,
                    _ => Stream::Out,
                };
                let _ = tx.unbounded_send((stream, payload["line"].as_str().unwrap_or_default().to_string()));
            }
        });
        self.build = Some(Build {
            key: key.clone(),
            target: target.clone(),
            status: BuildStatus::Running,
            lines: vec![],
            problems: vec![],
            keys: HashMap::new(),
            started: Instant::now(),
            elapsed: None,
            base_dir: config.build_dir.clone(),
            root,
            linked_root,
        });
        self.reveal(PanelView::Build, cx);
        cx.defer(apply_problems);
        let (done_tx, done_rx) = oneshot::channel();
        let (ws, repo, name, preset, k) = (target.workspace, target.repo, target.name, target.preset, key.clone());
        let work = cx.background_spawn(async move { orbit_core::run::build(&events, &ws, &repo, &name, preset.as_deref(), &k) });
        self._build = cx.spawn(async move |this, cx| {
            // Output arrives line by line; the panel redraws at most every 100 ms.
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(line) = rx.try_recv() {
                    batch.push(line);
                }
                if this.update(cx, |s, cx| s.push_lines(&key, batch, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
            let result = work.await;
            let ok = this.update(cx, |s, cx| s.finish_build(&key, result, cx)).unwrap_or(false);
            let _ = done_tx.send(ok);
        });
        Some(done_rx)
    }

    fn push_lines(&mut self, key: &str, batch: Vec<(Stream, String)>, cx: &mut Context<Self>) {
        let Some(b) = self.build.as_mut().filter(|b| b.key == key) else { return };
        let mut new_problems = false;
        for (stream, raw) in batch {
            let text = problems::strip_ansi(&raw);
            let mut problem = None;
            if stream != Stream::Info {
                if let Some(p) = problems::parse(&raw, b.base_dir.as_deref()) {
                    let k = p.key();
                    let ix = match b.keys.get(&k) {
                        Some(ix) => *ix,
                        None => {
                            let open = p.file.as_deref().and_then(|f| debug_rebase(Path::new(f), &b.root, &b.linked_root));
                            b.problems.push(BuildProblem { problem: p, open });
                            b.keys.insert(k, b.problems.len() - 1);
                            new_problems = true;
                            b.problems.len() - 1
                        }
                    };
                    problem = Some(ix);
                }
            }
            b.lines.push(OutputLine { stream, text: text.into(), problem });
        }
        if b.lines.len() > MAX_LINES {
            b.lines.drain(..b.lines.len() - MAX_LINES);
        }
        cx.notify();
        if new_problems {
            cx.defer(apply_problems);
        }
    }

    fn finish_build(&mut self, key: &str, result: Result<orbit_core::run::BuildExit, String>, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.build.as_mut().filter(|b| b.key == key) else { return false };
        b.status = match result {
            Ok(exit) if exit.cancelled => BuildStatus::Cancelled,
            Ok(exit) if exit.success => BuildStatus::Ok,
            Ok(_) => BuildStatus::Failed,
            Err(e) => {
                b.lines.push(OutputLine { stream: Stream::Err, text: e.into(), problem: None });
                BuildStatus::Failed
            }
        };
        b.elapsed = Some(b.started.elapsed());
        let (status, has_problems) = (b.status, !b.problems.is_empty());
        if status == BuildStatus::Failed && has_problems {
            self.reveal(PanelView::Problems, cx);
        }
        cx.notify();
        status == BuildStatus::Ok
    }

    // ---------- run ----------

    /// Builds, then starts the program in the panel's Run terminal.
    pub fn build_and_run(&mut self, cx: &mut Context<Self>) {
        let Some((_, config, _)) = self.selected() else { return };
        if config.program.is_none() {
            self.start_build(cx);
            return;
        }
        if self.is_running() {
            self.stop_program(cx);
        }
        let Some(built) = self.start_build(cx) else { return };
        self._run_after = cx.spawn(async move |this, cx| {
            if built.await.unwrap_or(false) {
                let _ = this.update(cx, |s, cx| s.start_program(config, cx));
            }
        });
    }

    fn start_program(&mut self, config: ResolvedRun, cx: &mut Context<Self>) {
        let Some(program) = config.program.clone() else { return };
        let Some(target) = self.target.clone() else { return };
        let meta = SessionMeta {
            id: format!("run:{}", now_ms()),
            workspace: target.workspace,
            label: "run".into(),
            session_name: config.name.clone(),
            cmd: Some(program.clone()),
            args: config.args.clone(),
            repo: Some(target.repo),
            agent_session_id: None,
            cwd: Some(config.cwd.clone()),
        };
        let spec = orbit_core::pty::Spawn {
            cwd: config.cwd.clone().into(),
            cmd: Some(program),
            args: config.args.clone(),
            env: config.env.clone().into_iter().collect(),
            size: terminal::spawn_size(),
            key: None,
            scrollback: false,
            program_version: env!("CARGO_PKG_VERSION").into(),
        };
        let view = cx.new(|cx| TerminalView::new(meta, spec, None, None, cx));
        let watch = cx.observe(&view, |s: &mut RunStore, view, cx| {
            let exited = view.read(cx).exited();
            if let Some(r) = s.run.as_mut().filter(|r| r.view == view && !r.exited && exited) {
                r.exited = true;
                cx.notify();
            }
        });
        self.run = Some(RunProc { view, name: config.name, exited: false, _watch: watch });
        self.reveal(PanelView::Run, cx);
    }

    /// Ctrl+C to the program.
    fn stop_program(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.run.as_ref().filter(|r| !r.exited) {
            r.view.read(cx).send(b"\x03");
        }
    }

    /// Stops the build, or the running program.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.build.as_ref().filter(|b| b.status == BuildStatus::Running) {
            orbit_core::run::cancel(&b.key);
            return;
        }
        self.stop_program(cx);
    }

    pub fn clear_run(&mut self, cx: &mut Context<Self>) {
        if self.run.as_ref().is_some_and(|r| r.exited) {
            self.run = None;
            if self.view == PanelView::Run {
                self.view = PanelView::Build;
            }
            cx.notify();
        }
    }

    /// The state for the self-test dump.
    pub fn dump(&self, cx: &App) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let configs: Vec<String> = self
            .setups
            .iter()
            .flat_map(|(repo, s)| s.configs.iter().map(move |c| format!("{repo}/{} [{}]", c.name, c.preset.clone().unwrap_or_default())))
            .collect();
        let _ = writeln!(out, "--- run: target {:?}, configs {configs:?}, panel view {:?}", self.target.as_ref().map(|t| (&t.repo, &t.name, &t.preset)), self.view);
        if let Some(c) = self.config() {
            let _ = writeln!(out, "  config: build `{}` | program {:?} | cwd {} | debugger {:?}", c.build_label, c.program, c.cwd, c.debugger);
        }
        if let Some((_, s)) = self.setups.iter().find(|(r, _)| self.target.as_ref().is_some_and(|t| &t.repo == r)) {
            let _ = writeln!(out, "  presets: {:?}", s.presets);
        }
        if let Some(b) = &self.build {
            let (errors, warnings) = b.counts();
            let _ = writeln!(
                out,
                "  build {:?} {} lines, {errors} errors, {warnings} warnings{}",
                b.status,
                b.lines.len(),
                b.elapsed.map(|d| format!(", {:.1}s", d.as_secs_f32())).unwrap_or_default()
            );
            for l in b.lines.iter().rev().take(6).rev() {
                let _ = writeln!(out, "    {:?}{} {}", l.stream, if l.problem.is_some() { " (link)" } else { "" }, l.text);
            }
            for p in b.problems.iter().take(8) {
                let q = &p.problem;
                let _ = writeln!(
                    out,
                    "    problem {:?} {}:{}:{} {}{} → {}",
                    q.severity,
                    q.file.as_deref().unwrap_or("-"),
                    q.line,
                    q.column,
                    q.message,
                    q.code.as_ref().map(|c| format!(" [{c}]")).unwrap_or_default(),
                    p.open.as_ref().map_or("-".into(), |o| o.display().to_string())
                );
            }
        }
        if let Some(r) = &self.run {
            let v = r.view.read(cx);
            let _ = writeln!(out, "  run {}{}:", r.name, if r.exited { " (exited)" } else { "" });
            for line in v.grid_text().trim_end().lines().filter(|l| !l.trim().is_empty()).take(12) {
                let _ = writeln!(out, "    | {line}");
            }
        }
        out
    }
}

/// `path` moved from under `from` to under `to`.
fn debug_rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(to.join(rest))
}

/// The whole Build/Run/Debug state, for the self-test dump.
pub fn dump(cx: &App) -> String {
    let mut out = store(cx).read(cx).dump(cx);
    out.push_str(&debug::store(cx).read(cx).dump());
    out.push_str(&debug::gutter::dump(cx));
    out
}
