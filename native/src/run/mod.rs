//! Build and Run (`src/lib/runStore.ts`): the selected run configuration
//! per workspace (from each repo's `.orbit/run.json`, through
//! `orbit_core::run`), the current build with its output and problems, and
//! the running programs' terminals. One build at a time (launches that need
//! one wait their turn) and any number of programs, launched by Rider's
//! rules (`launch`). The title bar widget is `widget.rs`, the bottom panel `panel.rs`.
pub mod configs;
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
    AnyElement, AnyWindowHandle, App, AppContext, Context, Entity, EntityId, Global, IntoElement, KeyBinding,
    ParentElement, SharedString, Styled, Subscription, Task, WeakEntity, Window,
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
    /// The repo's build folder (the chosen or automatic preset's): Build All builds it.
    pub build_dir: Option<String>,
    /// Its CMake targets (Edit Configurations' Target list).
    pub targets: Vec<orbit_core::cmake_api::CmakeTarget>,
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
    /// As printed, colours included (the panel shows them).
    pub raw: SharedString,
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
    /// Build All: every target of the repo's build folder.
    pub all: bool,
    pub status: BuildStatus,
    pub lines: Vec<OutputLine>,
    /// Lines ever printed (`lines` keeps the last `MAX_LINES`).
    pub total: usize,
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
    /// The instance's name ("game", "game (2)").
    pub name: String,
    pub target: RunTarget,
    pub exited: bool,
    _watch: Subscription,
}

impl RunProc {
    pub fn id(&self) -> EntityId {
        self.view.entity_id()
    }
}

/// What a launch needs: the configuration and its repo's (real, workspace) roots.
pub struct Launch {
    pub target: RunTarget,
    pub config: ResolvedRun,
    pub roots: (PathBuf, PathBuf),
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
    pub runs: Vec<RunProc>,
    /// The run the Run tab shows (None: the last).
    pub shown_run: Option<EntityId>,
    pub view: PanelView,
    pub panel: Option<Entity<panel::Panel>>,
    /// Edit Configurations, while open.
    pub dialog: Option<Entity<configs::RunConfigs>>,
    _load: Task<()>,
    _build: Task<()>,
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
    store(cx).read(cx).config().is_some_and(|c| c.program.is_some() || c.raw.is_compound())
}

fn active_status(cx: &App) -> Option<DebugStatus> {
    debug::active(cx).map(|s| s.read(cx).status)
}

fn paused(cx: &App) -> bool {
    active_status(cx) == Some(DebugStatus::Stopped)
}

/// The configurations of the live programs and debug sessions, one per instance.
pub fn live_targets(cx: &App) -> Vec<RunTarget> {
    let runs = store(cx).read(cx).runs.iter().filter(|r| !r.exited).map(|r| r.target.clone());
    let sessions = debug::store(cx).read(cx).sessions.iter().map(|s| s.read(cx)).filter(|s| s.is_live()).filter_map(|s| s.target.clone());
    runs.chain(sessions).collect()
}

fn live(cx: &App) -> bool {
    !live_targets(cx).is_empty()
}

/// The same configuration (whatever the preset).
pub fn same(a: &RunTarget, b: &RunTarget) -> bool {
    a.workspace == b.workspace && a.repo == b.repo && a.name == b.name
}

/// A new instance's name beside the live ones': the configuration's, then "name (2)", "name (3)"…
pub fn instance_name(config: &str, live: &[String]) -> String {
    (1..).map(|n| if n == 1 { config.to_string() } else { format!("{config} ({n})") }).find(|n| !live.contains(n)).unwrap_or_default()
}

/// Rider's rule: launching a configuration that runs restarts it, unless it allows multiple instances.
pub fn restarts(live: usize, allow_multiple: bool) -> bool {
    live > 0 && !allow_multiple
}

/// Runs or debugs `target` beside whatever else runs; its live instances are
/// stopped first unless it allows multiple, its finished one of that kind makes room.
pub fn launch(target: RunTarget, debug: bool, cx: &mut App) {
    let runs = store(cx);
    let Some(launch) = runs.read(cx).resolve(&target) else { return };
    if launch.config.program.is_none() {
        return;
    }
    let mut live: Vec<String> = runs.read(cx).runs.iter().filter(|r| !r.exited && same(&r.target, &target)).map(|r| r.name.clone()).collect();
    for s in &debug::store(cx).read(cx).sessions {
        let s = s.read(cx);
        if s.is_live() && s.target.as_ref().is_some_and(|t| same(t, &target)) {
            live.push(s.name.clone());
        }
    }
    let restart = restarts(live.len(), launch.config.raw.allow_multiple);
    let name = instance_name(&target.name, if restart { &[] } else { &live });
    runs.update(cx, |s, cx| {
        // A Run of it still building is superseded too.
        if !launch.config.raw.allow_multiple {
            s.cancel_build_of(&target);
        }
        s.remove_runs(cx, |r| same(&r.target, &target) && if r.exited { !debug } else { restart })
    });
    debug::store(cx).update(cx, |d, cx| {
        d.remove(cx, |s, cx| {
            let s = s.read(cx);
            s.target.as_ref().is_some_and(|t| same(t, &target)) && if s.is_live() { restart } else { debug }
        })
    });
    let built = runs.update(cx, |s, cx| s.prepare_launch(&launch, cx));
    if debug {
        debug::store(cx).update(cx, |d, cx| d.start(name, launch, built, cx));
    } else {
        runs.update(cx, |s, cx| s.run_after(name, launch, built, cx));
    }
}

/// Stops the live programs and debug sessions whose configuration `f` picks.
fn stop_where(f: impl Fn(&RunTarget) -> bool, cx: &mut App) {
    store(cx).update(cx, |s, cx| {
        for r in s.runs.iter().filter(|r| !r.exited && f(&r.target)) {
            r.view.read(cx).send(b"\x03");
        }
    });
    for session in debug::store(cx).read(cx).sessions.clone() {
        session.update(cx, |s, cx| {
            if s.is_live() && s.target.as_ref().is_some_and(&f) {
                s.stop(cx);
            }
        });
    }
}

/// Stop (Ctrl+F2): the build, else the selected configuration's instances
/// (a compound's: its configurations'), else everything.
fn stop(cx: &mut App) {
    let runs = store(cx);
    if runs.read(cx).is_building() {
        runs.update(cx, |s, cx| s.stop(cx));
        return;
    }
    let Some(target) = runs.read(cx).target.clone() else { return };
    let members = runs.read(cx).members(&target);
    let mine = |t: &RunTarget| members.iter().any(|m| same(m, t));
    let any_mine = live_targets(cx).iter().any(mine);
    stop_where(|t| !any_mine || mine(t), cx);
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
        runs: vec![],
        shown_run: None,
        view: PanelView::Build,
        panel: None,
        dialog: None,
        _load: Task::ready(()),
        _build: Task::ready(()),
    });
    cx.set_global(GlobalRun(entity));
    debug::init(cx);

    let ready = |_: &OrbitApp, cx: &App| store(cx).read(cx).config().is_some_and(|c| !c.raw.is_compound()) && !building(cx);
    // Run and Debug don't wait for a build to end: theirs queues behind it.
    let launchable = |_: &OrbitApp, cx: &App| has_program(cx);
    let with_debug = |f: fn(&mut debug::Session, &mut Context<debug::Session>)| {
        move |_: &mut OrbitApp, _: &mut Window, cx: &mut Context<OrbitApp>| {
            if let Some(s) = debug::active(cx) {
                s.update(cx, f);
            }
        }
    };
    let selected = |cx: &App| {
        let s = store(cx).read(cx);
        s.target.as_ref().map(|t| s.members(t)).unwrap_or_default()
    };
    for command in [
        Command::new("run.build", "Build").keys(["ctrl-f9"]).when(ready).run(|a, _, cx| {
            a.layout.panel_open = true;
            store(cx).update(cx, |s, cx| {
                s.start_build(cx);
            });
        }),
        Command::new("run.buildAll", "Build All")
            .keys(["ctrl-shift-f9"])
            .when(move |a, cx| ready(a, cx) && store(cx).read(cx).can_build_all())
            .run(|a, _, cx| {
                a.layout.panel_open = true;
                store(cx).update(cx, |s, cx| {
                    s.start_build_all(cx);
                });
            }),
        Command::new("run.editConfigurations", "Edit Run Configurations…")
            .when(|_, cx| !store(cx).read(cx).setups.is_empty())
            .run(|_, window, cx| {
                open_configurations(window, cx);
                cx.notify();
            }),
        Command::new("run.run", "Run").keys(["shift-f10"]).when(launchable).run(move |a, _, cx| {
            a.layout.panel_open = true;
            for t in selected(cx) {
                launch(t, false, cx);
            }
        }),
        Command::new("run.debug", "Debug").keys(["shift-f9"]).when(launchable).run(move |a, _, cx| {
            a.layout.panel_open = true;
            for t in selected(cx) {
                launch(t, true, cx);
            }
        }),
        Command::new("run.stop", "Stop").keys(["ctrl-f2"]).when(|_, cx| building(cx) || live(cx)).run(|_, _, cx| stop(cx)),
        Command::new("run.stopAll", "Stop All").when(|_, cx| building(cx) || live(cx)).run(|_, _, cx| {
            store(cx).update(cx, |s, cx| s.stop(cx));
            stop_where(|_| true, cx);
        }),
        Command::new("debug.pause", "Pause Program")
            .when(|_, cx| active_status(cx) == Some(DebugStatus::Running))
            .run(with_debug(debug::Session::pause)),
        Command::new("debug.resume", "Resume Program").keys(["f9"]).when(|_, cx| paused(cx)).run(with_debug(debug::Session::resume)),
        Command::new("debug.stepOver", "Step Over").keys(["f8"]).when(|_, cx| paused(cx)).run(with_debug(debug::Session::step_over)),
        Command::new("debug.stepInto", "Step Into").keys(["f7"]).when(|_, cx| paused(cx)).run(with_debug(debug::Session::step_into)),
        Command::new("debug.stepOut", "Step Out").keys(["shift-f8"]).when(|_, cx| paused(cx)).run(with_debug(debug::Session::step_out)),
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
    let panel = cx.new(|cx| panel::Panel::new(weak, window, cx));
    store(cx).update(cx, |s, _| s.panel = Some(panel));
}

/// Opens Edit Configurations on the selected configuration.
pub fn open_configurations(window: &mut Window, cx: &mut App) {
    let s = store(cx);
    if s.read(cx).dialog.is_some() {
        return;
    }
    let (app, target) = (s.read(cx).app.clone(), s.read(cx).target.clone());
    let dialog = cx.new(|cx| configs::RunConfigs::new(app, target, window, cx));
    s.update(cx, |s, cx| {
        s.dialog = Some(dialog);
        cx.notify();
    });
}

/// Edit Configurations over the window, while open (the shell draws it).
pub fn render_dialog(window: &Window, cx: &App) -> Option<AnyElement> {
    let dialog = store(cx).read(cx).dialog.clone()?;
    Some(crate::git::scrim(dialog.into_any_element(), window))
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
        self.runs.iter().any(|r| !r.exited)
    }

    pub fn shown(&self) -> Option<&RunProc> {
        self.runs.iter().find(|r| Some(r.id()) == self.shown_run).or(self.runs.last())
    }

    pub fn can_build_all(&self) -> bool {
        self.target.as_ref().and_then(|t| self.setup(&t.repo)).is_some_and(|s| s.build_dir.is_some())
    }

    pub fn workspace_name(&self) -> Option<String> {
        self.loaded_for.as_ref().map(|l| l.0.clone())
    }

    pub fn setup(&self, repo: &str) -> Option<&Setup> {
        self.setups.iter().find(|(r, _)| r == repo).map(|(_, s)| s)
    }

    pub fn config(&self) -> Option<&ResolvedRun> {
        let t = self.target.as_ref()?;
        self.setup(&t.repo)?.configs.iter().find(|c| c.name == t.name)
    }

    /// What launching `t` launches: a compound's configurations, else `t`.
    pub fn members(&self, t: &RunTarget) -> Vec<RunTarget> {
        let Some(setup) = self.setup(&t.repo) else { return vec![] };
        match setup.configs.iter().find(|c| c.name == t.name) {
            Some(c) if c.raw.is_compound() => {
                let (parts, _) = orbit_core::run::compound_parts(&c.raw, setup.configs.iter().map(|c| &c.raw));
                parts.into_iter().map(|name| RunTarget { name, ..t.clone() }).collect()
            }
            Some(_) => vec![t.clone()],
            None => vec![],
        }
    }

    /// `t` with its configuration and the repo's roots.
    pub fn resolve(&self, t: &RunTarget) -> Option<Launch> {
        let setup = self.setup(&t.repo)?;
        let config = setup.configs.iter().find(|c| c.name == t.name)?.clone();
        Some(Launch { target: t.clone(), config, roots: (setup.root.clone(), setup.linked_root.clone()) })
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
                            build_dir: s.build_dir,
                            targets: s.targets,
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

    /// Edit Configurations saved: reload, following a renamed selection
    /// (`renamed`: repo, old name, new name).
    pub fn configs_saved(&mut self, renamed: Option<(String, String, String)>, cx: &mut Context<Self>) {
        let mut target = self.target.clone();
        if let (Some(t), Some((repo, old, new))) = (target.as_mut(), renamed) {
            if t.repo == repo && t.name == old {
                t.name = new;
                let saved = t.clone();
                saved::update(|s| {
                    s.selection.insert(saved.workspace.clone(), saved);
                });
            }
        }
        self.reload(target, cx);
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
        let t = self.target.clone()?;
        self.build_inner(&t, false, cx)
    }

    /// Builds every target of the selected configuration's repo.
    pub fn start_build_all(&mut self, cx: &mut Context<Self>) -> Option<oneshot::Receiver<bool>> {
        let t = self.target.clone()?;
        self.build_inner(&t, true, cx)
    }

    /// Before Run or Debug: the build, queued behind the current one, or a ready
    /// answer when "Before launch: Build" is off. Dropping the receiver gives up the wait.
    pub fn prepare_launch(&mut self, launch: &Launch, cx: &mut Context<Self>) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        if !launch.config.build_before_launch {
            let _ = tx.send(true);
            return rx;
        }
        let target = launch.target.clone();
        cx.spawn(async move |this, cx| loop {
            if tx.is_canceled() {
                return;
            }
            match this.update(cx, |s, cx| (!s.is_building()).then(|| s.build_inner(&target, false, cx))) {
                Ok(Some(built)) => {
                    let ok = match built {
                        Some(done) => done.await.unwrap_or(false),
                        None => false,
                    };
                    let _ = tx.send(ok);
                    return;
                }
                Ok(None) => cx.background_executor().timer(Duration::from_millis(200)).await,
                Err(_) => return,
            }
        })
        .detach();
        rx
    }

    pub fn cancel_build_of(&mut self, t: &RunTarget) {
        if let Some(b) = self.build.as_ref().filter(|b| b.status == BuildStatus::Running && same(&b.target, t)) {
            orbit_core::run::cancel(&b.key);
        }
    }

    fn build_inner(&mut self, target: &RunTarget, all: bool, cx: &mut Context<Self>) -> Option<oneshot::Receiver<bool>> {
        if self.is_building() {
            return None;
        }
        let Launch { target, config, roots: (root, linked_root) } = self.resolve(target)?;
        let base_dir = if all { self.setup(&target.repo)?.build_dir.clone() } else { config.build_dir.clone() };
        let key = format!("{}/{}/{}", target.workspace, target.repo, if all { "*all*" } else { target.name.as_str() });
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
            all,
            status: BuildStatus::Running,
            lines: vec![],
            total: 0,
            problems: vec![],
            keys: HashMap::new(),
            started: Instant::now(),
            elapsed: None,
            base_dir,
            root,
            linked_root,
        });
        self.reveal(PanelView::Build, cx);
        cx.defer(apply_problems);
        let (done_tx, done_rx) = oneshot::channel();
        let (ws, repo, name, preset, k) = (target.workspace, target.repo, target.name, target.preset, key.clone());
        let work = cx.background_spawn(async move {
            let what = if all { orbit_core::run::BuildWhat::All } else { orbit_core::run::BuildWhat::Config(&name) };
            orbit_core::run::build(&events, &ws, &repo, what, preset.as_deref(), &k)
        });
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
            b.lines.push(OutputLine { stream, text: text.into(), raw: raw.into(), problem });
            b.total += 1;
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
                let text: SharedString = e.into();
                b.lines.push(OutputLine { stream: Stream::Err, text: text.clone(), raw: text, problem: None });
                b.total += 1;
                BuildStatus::Failed
            }
        };
        b.elapsed = Some(b.started.elapsed());
        let (status, has_problems) = (b.status, !b.problems.is_empty());
        if status == BuildStatus::Failed && has_problems {
            self.reveal(PanelView::Problems, cx);
        }
        // A build may have configured: CMake's targets can have changed.
        if status != BuildStatus::Cancelled {
            self.reload(self.target.clone(), cx);
        }
        cx.notify();
        status == BuildStatus::Ok
    }

    // ---------- run ----------

    /// Once built, starts the program in a Run terminal of the panel.
    fn run_after(&mut self, name: String, launch: Launch, built: oneshot::Receiver<bool>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if built.await.unwrap_or(false) {
                let _ = this.update(cx, |s, cx| s.start_program(name, launch, cx));
            }
        })
        .detach();
    }

    fn start_program(&mut self, name: String, launch: Launch, cx: &mut Context<Self>) {
        let Launch { target, config, .. } = launch;
        let Some(program) = config.program.clone() else { return };
        let meta = SessionMeta {
            id: format!("run:{}", now_ms()),
            workspace: target.workspace.clone(),
            label: "run".into(),
            session_name: name.clone(),
            codename: None,
            cmd: Some(program.clone()),
            args: config.args.clone(),
            repo: Some(target.repo.clone()),
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
            tty_only: false,
        };
        let view = cx.new(|cx| TerminalView::new(meta, spec, None, None, cx));
        let watch = cx.observe(&view, |s: &mut RunStore, view, cx| {
            let exited = view.read(cx).exited();
            if let Some(r) = s.runs.iter_mut().find(|r| r.view == view && !r.exited && exited) {
                r.exited = true;
                cx.notify();
            }
        });
        self.shown_run = Some(view.entity_id());
        self.runs.push(RunProc { view, name, target, exited: false, _watch: watch });
        self.reveal(PanelView::Run, cx);
    }

    pub fn stop(&mut self, _: &mut Context<Self>) {
        if let Some(b) = self.build.as_ref().filter(|b| b.status == BuildStatus::Running) {
            orbit_core::run::cancel(&b.key);
        }
    }

    /// Drops the runs `f` picks (a live one gets Ctrl+C first; its terminal closes with it).
    fn remove_runs(&mut self, cx: &mut Context<Self>, f: impl Fn(&RunProc) -> bool) {
        for r in self.runs.iter().filter(|r| !r.exited && f(r)) {
            r.view.read(cx).send(b"\x03");
        }
        self.runs.retain(|r| !f(r));
        if self.runs.is_empty() && self.view == PanelView::Run {
            self.view = PanelView::Build;
        }
        cx.notify();
    }

    pub fn show_run(&mut self, id: EntityId, cx: &mut Context<Self>) {
        self.shown_run = Some(id);
        cx.notify();
    }

    /// The instance tab's × (and Clear): stops a live run, closes a finished one.
    pub fn close_run(&mut self, id: EntityId, cx: &mut Context<Self>) {
        match self.runs.iter().find(|r| r.id() == id) {
            Some(r) if !r.exited => r.view.read(cx).send(b"\x03"),
            Some(_) => self.remove_runs(cx, |r| r.id() == id),
            None => {}
        }
    }

    /// The state for the self-test dump.
    pub fn dump(&self, cx: &App) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let configs: Vec<String> = self
            .setups
            .iter()
            .flat_map(|(repo, s)| s.configs.iter().map(move |c| format!("{repo}/{} [{}, {:?}]", c.name, c.preset.clone().unwrap_or_default(), c.origin)))
            .collect();
        let _ = writeln!(out, "--- run: target {:?}, configs {configs:?}, panel view {:?}", self.target.as_ref().map(|t| (&t.repo, &t.name, &t.preset)), self.view);
        if let Some(c) = self.config() {
            let _ = writeln!(
                out,
                "  config: build `{}` | program {:?} | cwd {} | env {:?} | debugger {:?} | build first {}",
                c.build_label, c.program, c.cwd, c.env, c.debugger, c.build_before_launch
            );
        }
        if let Some((_, s)) = self.setups.iter().find(|(r, _)| self.target.as_ref().is_some_and(|t| &t.repo == r)) {
            let _ = writeln!(out, "  presets: {:?}", s.presets);
            let targets: Vec<String> = s.targets.iter().map(|t| format!("{}:{}", t.name, t.kind)).collect();
            let _ = writeln!(out, "  targets of {}: {targets:?}", s.build_dir.as_deref().unwrap_or("-"));
        }
        if let Some(b) = &self.build {
            let (errors, warnings) = b.counts();
            let _ = writeln!(
                out,
                "  build{} {:?} {} lines, {errors} errors, {warnings} warnings{}",
                if b.all { " all" } else { "" },
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
        if let Some(d) = &self.dialog {
            out.push_str(&d.read(cx).dump(cx));
        }
        if self.runs.len() > 1 {
            let names: Vec<String> = self.runs.iter().map(|r| format!("{}{}", r.name, if r.exited { " (exited)" } else { "" })).collect();
            let _ = writeln!(out, "  runs {names:?}");
        }
        if let Some(r) = self.shown() {
            let v = r.view.read(cx);
            let _ = writeln!(out, "  run {}{}: {}", r.name, if r.exited { " (exited)" } else { "" }, v.scroll_summary());
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
    out.push_str(&debug::store(cx).read(cx).dump(cx));
    if let Some(p) = store(cx).read(cx).panel.as_ref() {
        out.push_str(&p.read(cx).dump(cx));
    }
    out.push_str(&debug::gutter::dump(cx));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_are_numbered_from_the_first_free_name() {
        let live = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(instance_name("game", &[]), "game");
        assert_eq!(instance_name("game", &live(&["game"])), "game (2)");
        assert_eq!(instance_name("game", &live(&["game", "game (2)"])), "game (3)");
        // A freed name is reused.
        assert_eq!(instance_name("game", &live(&["game (2)"])), "game");
        assert_eq!(instance_name("game", &live(&["game", "game (3)"])), "game (2)");
    }

    #[test]
    fn a_running_configuration_restarts_unless_it_allows_multiple() {
        assert!(!restarts(0, false));
        assert!(!restarts(0, true));
        assert!(restarts(1, false));
        assert!(!restarts(1, true));
        assert!(!restarts(3, true));
    }
}
