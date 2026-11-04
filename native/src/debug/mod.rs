//! The debug session (`src/lib/dap/session.ts`), one at a time as in Rider:
//! breakpoints, launching a run configuration under the debugger, where it
//! stopped (threads, frames, variables) and the debug console.
pub mod client;
pub mod gutter;
pub mod view;

use crate::run::{self, RunTarget};
use client::{DapClient, DapEvent};
use futures::{FutureExt, StreamExt};
use gpui::{App, AppContext, Context, Entity, Global, Task};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MAX_CONSOLE: usize = 3000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DebugStatus {
    Idle,
    Starting,
    Running,
    Stopped,
    Ended,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub line: u32,
    pub column: u32,
    /// The file as the editor knows it (workspace path), or as reported.
    pub file: Option<PathBuf>,
    /// The file is in the session's repo (it opens as a tab).
    pub in_repo: bool,
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
    /// > 0: has children, loaded on expand.
    pub reference: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConsoleKind {
    Out,
    Err,
    Info,
    Input,
    Result,
}

/// The workspace path of the repo and its real (links resolved) path.
#[derive(Clone)]
struct Roots {
    real: PathBuf,
    linked: PathBuf,
}

pub struct DebugStore {
    pub status: DebugStatus,
    pub target: Option<RunTarget>,
    roots: Option<Roots>,
    client: Option<Arc<DapClient>>,
    pub adapter: Option<String>,
    pub threads: Vec<(i64, String)>,
    pub thread_id: Option<i64>,
    pub frames: Vec<Frame>,
    pub frame_id: Option<i64>,
    pub scopes: Vec<(String, i64)>,
    /// Children by variablesReference (scopes and expanded variables).
    pub vars: HashMap<i64, Vec<Variable>>,
    /// Expanded scopes and variables.
    pub open: HashSet<i64>,
    pub console: Vec<(ConsoleKind, String)>,
    pub stop_reason: Option<String>,
    /// Breakpoint lines (1-based) by the file's workspace path.
    pub breakpoints: BTreeMap<PathBuf, Vec<u32>>,
    _session: Task<()>,
}

struct GlobalDebug(Entity<DebugStore>);

impl Global for GlobalDebug {}

pub fn store(cx: &App) -> Entity<DebugStore> {
    cx.global::<GlobalDebug>().0.clone()
}

pub fn init(cx: &mut App) {
    let breakpoints = run::saved::load().breakpoints;
    let entity = cx.new(|_| DebugStore { breakpoints, ..empty() });
    // Decorations follow the session and the breakpoints.
    cx.observe(&entity, |_, cx| gutter::refresh(cx)).detach();
    // The adapter (and with it the debuggee) goes with the app.
    cx.on_app_quit(|cx| {
        if let Some(c) = store(cx).read(cx).client.clone() {
            c.kill();
        }
        async {}
    })
    .detach();
    cx.set_global(GlobalDebug(entity));
    gutter::init(cx);
}

/// `path` moved from under `from` to under `to`.
fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) })
}

impl DebugStore {
    pub fn is_live(&self) -> bool {
        matches!(self.status, DebugStatus::Starting | DebugStatus::Running | DebugStatus::Stopped)
    }

    pub fn log(&mut self, kind: ConsoleKind, text: &str) {
        let text = text.strip_suffix('\n').unwrap_or(text);
        self.console.extend(text.split('\n').map(|l| (kind, l.trim_end_matches('\r').to_string())));
        if self.console.len() > MAX_CONSOLE {
            self.console.drain(..self.console.len() - MAX_CONSOLE);
        }
    }

    // ---------- breakpoints ----------

    pub fn breakpoints_in(&self, file: &Path) -> Vec<u32> {
        self.breakpoints.iter().find(|(k, _)| gutter::same_file(k, file)).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    pub fn toggle_breakpoint(&mut self, file: PathBuf, line: u32, cx: &mut Context<Self>) {
        let key = self.breakpoints.keys().find(|k| gutter::same_file(k, &file)).cloned().unwrap_or(file);
        let lines = self.breakpoints.entry(key.clone()).or_default();
        match lines.iter().position(|l| *l == line) {
            Some(ix) => {
                lines.remove(ix);
            }
            None => {
                lines.push(line);
                lines.sort_unstable();
            }
        }
        if lines.is_empty() {
            self.breakpoints.remove(&key);
        }
        let all = self.breakpoints.clone();
        run::saved::update(|s| s.breakpoints = all);
        if matches!(self.status, DebugStatus::Running | DebugStatus::Stopped) {
            self.send_breakpoints(&key, cx).detach();
        }
        cx.notify();
    }

    /// Editor path → the path the debugger knows (links resolved).
    fn to_debugger(&self, path: &Path) -> PathBuf {
        self.roots.as_ref().and_then(|r| rebase(path, &r.linked, &r.real)).unwrap_or_else(|| path.to_path_buf())
    }

    fn to_editor(&self, path: &Path) -> PathBuf {
        self.roots.as_ref().and_then(|r| rebase(path, &r.real, &r.linked)).unwrap_or_else(|| path.to_path_buf())
    }

    fn send_breakpoints(&self, file: &Path, cx: &mut Context<Self>) -> Task<()> {
        let Some(c) = self.client.clone() else { return Task::ready(()) };
        let lines: Vec<Value> = self.breakpoints_in(file).into_iter().map(|line| json!({ "line": line })).collect();
        let args = json!({ "source": { "path": self.to_debugger(file) }, "breakpoints": lines });
        let request = c.request("setBreakpoints", args);
        let shown = file.display().to_string();
        cx.spawn(async move |this, cx| {
            if let Err(e) = request.await {
                let _ = this.update(cx, |s, cx| {
                    s.log(ConsoleKind::Err, &format!("Breakpoints in {shown}: {e}"));
                    cx.notify();
                });
            }
        })
    }

    /// Files under the session's repo (breakpoints elsewhere belong to others).
    fn session_files(&self) -> Vec<PathBuf> {
        let Some(r) = &self.roots else { return vec![] };
        self.breakpoints.keys().filter(|f| f.starts_with(&r.linked)).cloned().collect()
    }

    // ---------- session ----------

    /// Builds the selected configuration, then runs it under the debugger.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.is_live() {
            return;
        }
        let runs = run::store(cx);
        let Some((target, config, setup_roots)) = runs.read(cx).selected() else { return };
        let Some(program) = config.program.clone() else { return };
        *self = DebugStore { breakpoints: std::mem::take(&mut self.breakpoints), ..empty() };
        self.status = DebugStatus::Starting;
        self.target = Some(target);
        cx.notify();
        let Some(built) = runs.update(cx, |r, cx| r.start_build(cx)) else {
            self.status = DebugStatus::Idle;
            return;
        };
        self._session = cx.spawn(async move |this, cx| {
            if !built.await.unwrap_or(false) {
                let _ = this.update(cx, |s, cx| {
                    s.status = DebugStatus::Idle;
                    cx.notify();
                });
                return;
            }
            let started = this.update(cx, |s, cx| {
                run::show(run::PanelView::Debug, cx);
                s.roots = Some(Roots { real: setup_roots.0, linked: setup_roots.1 });
                let requested = Some(config.debugger.as_str()).filter(|d| !d.is_empty());
                match DapClient::start(requested, &config.cwd) {
                    Ok((c, events)) => {
                        s.log(ConsoleKind::Info, &format!("{} {} · {program}", c.adapter.command, c.adapter.args.join(" ")));
                        s.adapter = Some(c.adapter.kind.clone());
                        s.client = Some(c.clone());
                        cx.notify();
                        Some((c, events))
                    }
                    Err(e) => {
                        s.log(ConsoleKind::Err, &e);
                        s.status = DebugStatus::Ended;
                        cx.notify();
                        None
                    }
                }
            });
            let Ok(Some((c, mut events))) = started else { return };
            // Events, for the rest of the session.
            let pump = this.clone();
            cx.spawn(async move |cx| {
                while let Some(ev) = events.next().await {
                    if pump.update(cx, |s, cx| s.on_event(ev, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            if let Err(e) = launch(&this, &c, &config, &program, cx).await {
                let _ = this.update(cx, |s, cx| {
                    s.log(ConsoleKind::Err, &e);
                    s.stop(cx);
                });
            }
        });
    }

    fn on_event(&mut self, ev: DapEvent, cx: &mut Context<Self>) {
        match ev {
            DapEvent::Exit => self.end(cx),
            DapEvent::Event { event, body } => match event.as_str() {
                "output" if body["category"] != "telemetry" => {
                    let kind = match body["category"].as_str() {
                        Some("stderr") => ConsoleKind::Err,
                        Some("console") => ConsoleKind::Info,
                        _ => ConsoleKind::Out,
                    };
                    self.log(kind, body["output"].as_str().unwrap_or_default());
                }
                "stopped" => self.on_stopped(body, cx),
                "continued" => {
                    self.status = DebugStatus::Running;
                    self.clear_stop();
                }
                "exited" => self.log(ConsoleKind::Info, &format!("Process exited with code {}", body["exitCode"])),
                "terminated" => self.end(cx),
                _ => {}
            },
        }
        cx.notify();
    }

    fn clear_stop(&mut self) {
        self.frames.clear();
        self.frame_id = None;
        self.scopes.clear();
        self.vars.clear();
        self.open.clear();
        self.stop_reason = None;
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, DebugStatus::Ended | DebugStatus::Idle) {
            return;
        }
        if let Some(c) = self.client.take() {
            c.kill();
        }
        self.status = DebugStatus::Ended;
        self.clear_stop();
        self.threads.clear();
        self.log(ConsoleKind::Info, "Debug session ended");
        cx.notify();
    }

    fn on_stopped(&mut self, body: Value, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let threads = c.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let threads: Vec<(i64, String)> = threads
                .await
                .ok()
                .and_then(|b| b["threads"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| Some((t["id"].as_i64()?, t["name"].as_str().unwrap_or_default().to_string())))
                .collect();
            let _ = this.update(cx, |s, cx| {
                let thread = body["threadId"].as_i64().or(threads.first().map(|t| t.0));
                s.status = DebugStatus::Stopped;
                s.threads = threads;
                s.thread_id = thread;
                s.stop_reason = body["description"].as_str().or(body["reason"].as_str()).map(str::to_string);
                cx.notify();
                if let Some(t) = thread {
                    s.select_thread(t, true, cx);
                }
            });
        })
        .detach();
    }

    pub fn select_thread(&mut self, thread: i64, reveal: bool, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let trace = c.request("stackTrace", json!({ "threadId": thread, "startFrame": 0, "levels": 60 }));
        cx.spawn(async move |this, cx| {
            let r = trace.await.unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                let linked = s.roots.as_ref().map(|r| r.linked.clone());
                s.frames = r["stackFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| {
                        let file = f["source"]["path"].as_str().map(|p| s.to_editor(Path::new(p)));
                        Some(Frame {
                            id: f["id"].as_i64()?,
                            name: f["name"].as_str().unwrap_or_default().to_string(),
                            line: f["line"].as_u64().unwrap_or(0) as u32,
                            column: f["column"].as_u64().filter(|c| *c > 0).unwrap_or(1) as u32,
                            in_repo: file.as_ref().zip(linked.as_ref()).is_some_and(|(f, l)| f.starts_with(l)),
                            file,
                        })
                    })
                    .collect();
                s.thread_id = Some(thread);
                // The innermost frame with source in the repo is where you are.
                let top = s.frames.iter().find(|f| f.in_repo).or(s.frames.first()).map(|f| f.id);
                cx.notify();
                if let Some(id) = top {
                    s.select_frame(id, reveal, cx);
                }
            });
        })
        .detach();
    }

    pub fn select_frame(&mut self, frame: i64, reveal: bool, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        self.frame_id = Some(frame);
        self.scopes.clear();
        self.vars.clear();
        self.open.clear();
        cx.notify();
        if let Some(f) = self.frames.iter().find(|f| f.id == frame && f.in_repo) {
            if reveal {
                run::open_location(f.file.clone().unwrap_or_default(), f.line, f.column, cx);
            }
        }
        let scopes = c.request("scopes", json!({ "frameId": frame }));
        cx.spawn(async move |this, cx| {
            let r = scopes.await.unwrap_or_default();
            let _ = this.update(cx, |s, cx| {
                if s.frame_id != Some(frame) {
                    return;
                }
                let list: Vec<(String, i64, bool)> = r["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|sc| {
                        let name = sc["name"].as_str().unwrap_or_default().to_string();
                        (name, sc["variablesReference"].as_i64().unwrap_or(0), sc["expensive"].as_bool().unwrap_or(false))
                    })
                    .collect();
                s.scopes = list.iter().map(|(n, r, _)| (n.clone(), *r)).collect();
                for (_, reference, expensive) in list {
                    if !expensive {
                        s.open.insert(reference);
                        s.load_variables(reference, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_variables(&mut self, reference: i64, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        if reference <= 0 || self.vars.contains_key(&reference) {
            return;
        }
        let request = c.request("variables", json!({ "variablesReference": reference }));
        cx.spawn(async move |this, cx| {
            let r = request.await.unwrap_or_default();
            let list = r["variables"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| Variable {
                    name: v["name"].as_str().unwrap_or_default().to_string(),
                    value: v["value"].as_str().unwrap_or_default().to_string(),
                    ty: v["type"].as_str().filter(|t| !t.is_empty()).map(str::to_string),
                    reference: v["variablesReference"].as_i64().unwrap_or(0),
                })
                .collect();
            let _ = this.update(cx, |s, cx| {
                s.vars.insert(reference, list);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_var(&mut self, reference: i64, cx: &mut Context<Self>) {
        if reference <= 0 {
            return;
        }
        if !self.open.remove(&reference) {
            self.open.insert(reference);
            self.load_variables(reference, cx);
        }
        cx.notify();
    }

    pub fn evaluate(&mut self, expression: String, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        if expression.trim().is_empty() {
            return;
        }
        self.log(ConsoleKind::Input, &expression);
        // gdb runs "repl" input as a CLI command; "watch" evaluates it as an expression.
        let context = if c.adapter.kind == "gdb" { "watch" } else { "repl" };
        let request = c.request("evaluate", json!({ "expression": expression, "frameId": self.frame_id, "context": context }));
        cx.spawn(async move |this, cx| {
            let r = request.await;
            let _ = this.update(cx, |s, cx| {
                match r {
                    Ok(b) => s.log(ConsoleKind::Result, b["result"].as_str().unwrap_or_default()),
                    Err(e) => s.log(ConsoleKind::Err, &e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn step(&mut self, command: &'static str, cx: &mut Context<Self>) {
        let (Some(c), Some(thread)) = (self.client.clone(), self.thread_id) else { return };
        if self.status != DebugStatus::Stopped {
            return;
        }
        let request = c.request(command, json!({ "threadId": thread }));
        cx.spawn(async move |this, cx| {
            if let Err(e) = request.await {
                let _ = this.update(cx, |s, cx| {
                    s.log(ConsoleKind::Err, &e);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn resume(&mut self, cx: &mut Context<Self>) {
        self.step("continue", cx);
    }

    pub fn step_over(&mut self, cx: &mut Context<Self>) {
        self.step("next", cx);
    }

    pub fn step_into(&mut self, cx: &mut Context<Self>) {
        self.step("stepIn", cx);
    }

    pub fn step_out(&mut self, cx: &mut Context<Self>) {
        self.step("stepOut", cx);
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        if self.status != DebugStatus::Running {
            return;
        }
        let thread = self.thread_id.or(self.threads.first().map(|t| t.0)).unwrap_or(1);
        let request = c.request("pause", json!({ "threadId": thread }));
        cx.spawn(async move |this, cx| {
            if let Err(e) = request.await {
                let _ = this.update(cx, |s, cx| {
                    s.log(ConsoleKind::Err, &e);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Ends the session and the program.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else {
            if self.status == DebugStatus::Starting {
                // Still building: cancel that.
                run::store(cx).update(cx, |r, cx| r.stop(cx));
                self.status = DebugStatus::Idle;
                cx.notify();
            }
            return;
        };
        let disconnect = c.request("disconnect", json!({ "terminateDebuggee": true }));
        cx.spawn(async move |_, _| {
            let _ = disconnect.await;
        })
        .detach();
        // Adapters that hang on disconnect are killed.
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            if c.alive() {
                c.kill();
            }
        })
        .detach();
    }

    /// The state for the self-test dump.
    pub fn dump(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "--- debug: {:?}{}{}",
            self.status,
            self.adapter.as_ref().map(|a| format!(" via {a}")).unwrap_or_default(),
            self.stop_reason.as_ref().map(|r| format!(" · {r}")).unwrap_or_default()
        );
        for (file, lines) in &self.breakpoints {
            let _ = writeln!(out, "  breakpoints {} {lines:?}", file.display());
        }
        if !self.threads.is_empty() {
            let threads: Vec<String> = self.threads.iter().map(|(id, n)| format!("{id} {n}")).collect();
            let _ = writeln!(out, "  threads {threads:?}, selected {:?}", self.thread_id);
        }
        for f in self.frames.iter().take(8) {
            let _ = writeln!(
                out,
                "  frame {}{} {} {}:{}{}",
                if Some(f.id) == self.frame_id { "*" } else { " " },
                f.id,
                f.name,
                f.file.as_ref().map_or("?".into(), |p| p.display().to_string()),
                f.line,
                if f.in_repo { "" } else { " (external)" }
            );
        }
        let mut rows = vec![];
        view::variable_rows(self, &mut rows);
        for r in rows.iter().take(30) {
            let _ = writeln!(out, "  var {}{} = {}{}", "  ".repeat(r.depth), r.name, r.value, r.ty.as_ref().map(|t| format!(" : {t}")).unwrap_or_default());
        }
        for (kind, text) in self.console.iter().rev().take(12).rev() {
            let _ = writeln!(out, "  console {kind:?}: {text}");
        }
        out
    }
}

fn empty() -> DebugStore {
    DebugStore {
        status: DebugStatus::Idle,
        target: None,
        roots: None,
        client: None,
        adapter: None,
        threads: vec![],
        thread_id: None,
        frames: vec![],
        frame_id: None,
        scopes: vec![],
        vars: HashMap::new(),
        open: HashSet::new(),
        console: vec![],
        stop_reason: None,
        breakpoints: BTreeMap::new(),
        _session: Task::ready(()),
    }
}

/// The launch sequence of `session.ts`. gdb answers `launch` only after
/// `configurationDone`, so: initialize → launch (not awaited) → initialized
/// → setBreakpoints → configurationDone → launch's answer.
async fn launch(
    this: &gpui::WeakEntity<DebugStore>,
    c: &Arc<DapClient>,
    config: &orbit_core::run::ResolvedRun,
    program: &str,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    let initialized = c.initialized();
    c.request(
        "initialize",
        json!({
            "clientID": "orbit", "clientName": "Orbit", "adapterID": c.adapter.kind,
            "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsVariableType": true,
        }),
    )
    .await?;
    let env: Value = if c.adapter.kind == "gdb" {
        json!(config.env)
    } else {
        json!(config.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>())
    };
    let launched = c.request("launch", json!({ "program": program, "args": config.args, "cwd": config.cwd, "env": env }));
    let mut launched = launched.boxed_local().fuse();
    let mut initialized = initialized.boxed_local().fuse();
    let mut answer = None;
    futures::select! {
        r = initialized => r?,
        r = launched => {
            answer = Some(r.map_err(|e| format!("Launch failed: {e}"))?);
            initialized.await?;
        }
    }
    let files = this.update(cx, |s, _| s.session_files()).map_err(|_| "gone")?;
    let sends: Vec<Task<()>> = this.update(cx, |s, cx| files.iter().map(|f| s.send_breakpoints(f, cx)).collect()).map_err(|_| "gone")?;
    futures::future::join_all(sends).await;
    c.request("configurationDone", json!({})).await?;
    if answer.is_none() {
        launched.await.map_err(|e| format!("Launch failed: {e}"))?;
    }
    let _ = this.update(cx, |s, cx| {
        if s.status == DebugStatus::Starting {
            s.status = DebugStatus::Running;
            cx.notify();
        }
    });
    Ok(())
}
