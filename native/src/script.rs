//! Self-test mode: drives the app through its own code paths (GPUI's
//! in-process keystroke and mouse dispatch, never OS input simulation) and
//! writes a state dump, so features can be verified without a person.
//!
//! The steps run in a fixed order, each one optional:
//! workspace → expand → terminals and sessions → open (+cursor) → wait for
//! the language server → type → keys → ctrl-click → hover → diff → terminal
//! drag → steps → shell steps (`--do`, see `steps.rs`) → dump.
//!
//! Besides key specs, `--keys` takes `@namespace::Action` (dispatched to the
//! focused element, like a key binding would), `wait` (100 ms, `wait*20` for
//! 2 s; it also draws a frame) and `type:text` (typed; `\s` is a space).
use crate::app::{OrbitApp, TabKind};
use crate::{sessions, terminal};
use gpui::{
    AnyWindowHandle, AsyncApp, Entity, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, Point, Pixels, WindowHandle,
};
use gpui_component::input::RopeExt;
use gpui_component::Root;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
pub struct Script {
    pub workspace: Option<String>,
    pub expand: usize,
    pub terminals: usize,
    pub cmd: Option<String>,
    /// Files to open in order, relative to the workspace folder (or absolute).
    pub open: Vec<PathBuf>,
    /// Open them as preview tabs (a single click in the tree).
    pub preview: bool,
    /// 1-based `line:column` for the cursor after opening.
    pub cursor: Option<(u32, u32)>,
    /// Seconds to wait for the editor's language server (and first diagnostics).
    pub wait_lsp: u64,
    pub keys: Vec<String>,
    pub typed: Option<String>,
    /// Ctrl+click the word under the cursor, through GPUI mouse events.
    pub ctrl_click: bool,
    pub hover: bool,
    /// Open the diff of the active file.
    pub diff: bool,
    /// Drag-select in the active terminal: `col,row:col,row` (0-based cells).
    pub term_drag: Option<((usize, usize), (usize, usize))>,
    pub scroll: i32,
    /// Sessions to open at start: "shell", "claude", "opencode", "omp" or any command.
    pub sessions: Vec<String>,
    /// `--steps "a; b; …"`, run in order last (see [`run_steps`]).
    pub steps: Vec<String>,
    pub exit_after: Option<u64>,
    pub dump: Option<PathBuf>,
    /// State file to restore from and save to (a self-test uses none otherwise).
    pub state: Option<PathBuf>,
    /// Shell steps run last (`steps.rs`): `--do "rclick:tab:0;click:menu:Close"`.
    pub shell_steps: Vec<String>,
}

fn cell(spec: &str) -> Option<(usize, usize)> {
    let (c, r) = spec.split_once(',')?;
    Some((c.trim().parse().ok()?, r.trim().parse().ok()?))
}

impl Script {
    pub fn from_args() -> Self {
        let mut s = Script::default();
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--workspace" => s.workspace = args.next(),
                "--expand" => s.expand = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "--terminals" => s.terminals = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "--cmd" => s.cmd = args.next(),
                "--open" => s.open.extend(args.next().map(PathBuf::from)),
                "--preview" => s.preview = true,
                "--cursor" => {
                    s.cursor = args.next().and_then(|v| {
                        let (l, c) = v.split_once(':')?;
                        Some((l.parse().ok()?, c.parse().ok()?))
                    })
                }
                "--wait-lsp" => s.wait_lsp = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "--keys" => {
                    // "down*300" repeats a key.
                    for spec in args.next().unwrap_or_default().split_whitespace() {
                        let (key, n) = spec.split_once('*').map_or((spec, 1), |(k, n)| (k, n.parse().unwrap_or(1)));
                        s.keys.extend(std::iter::repeat_n(key.to_string(), n));
                    }
                }
                "--type" => s.typed = args.next().map(|t| t.replace("\\n", "\n")),
                "--ctrl-click" => s.ctrl_click = true,
                "--hover" => s.hover = true,
                "--diff" => s.diff = true,
                "--term-drag" => {
                    s.term_drag = args.next().and_then(|v| {
                        let (a, b) = v.split_once(':')?;
                        Some((cell(a)?, cell(b)?))
                    })
                }
                "--session" => s.sessions.extend(args.next()),
                "--steps" => s.steps.extend(args.next().unwrap_or_default().split(';').map(|t| t.trim().to_string()).filter(|t| !t.is_empty())),
                "--scroll" => s.scroll = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "--exit-after" => s.exit_after = args.next().and_then(|v| v.parse().ok()),
                "--dump" => s.dump = args.next().map(PathBuf::from),
                "--state" => s.state = args.next().map(PathBuf::from),
                "--do" => s.shell_steps.extend(args.next().unwrap_or_default().split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty())),
                _ => eprintln!("unknown argument {a}"),
            }
        }
        s
    }

    pub fn is_active(&self) -> bool {
        self.exit_after.is_some() || self.dump.is_some() || !self.steps.is_empty() || !self.shell_steps.is_empty()
    }
}

enum Step {
    Key(Keystroke),
    Action(String),
    Wait,
}

fn typed(text: &str) -> impl Iterator<Item = Step> + '_ {
    text.chars().map(|ch| {
        Step::Key(match ch {
            '\n' => Keystroke::parse("enter").unwrap(),
            ' ' => Keystroke { key: "space".into(), key_char: Some(" ".into()), ..Default::default() },
            ch => Keystroke { key: ch.to_lowercase().to_string(), key_char: Some(ch.to_string()), ..Default::default() },
        })
    })
}

/// The typed text, then the keys (so `--type x --keys ctrl-s` saves the edit).
fn input_steps(script: &Script) -> Vec<Step> {
    let mut input: Vec<Step> = script.typed.iter().flat_map(|t| typed(t)).collect();
    for k in &script.keys {
        if let Some(action) = k.strip_prefix('@') {
            input.push(Step::Action(action.to_string()));
        } else if k == "wait" {
            input.push(Step::Wait);
        } else if let Some(text) = k.strip_prefix("type:") {
            input.extend(typed(&text.replace("\\s", " ")).collect::<Vec<_>>());
        } else if let Ok(ks) = Keystroke::parse_with_os(k, gpui::OperatingSystem::Linux) {
            input.push(Step::Key(ks));
        }
    }
    input
}

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

fn ctrl() -> Modifiers {
    Modifiers { control: true, ..Default::default() }
}

/// Runs until `cond` holds or `secs` pass; returns how long it took.
async fn wait_until(cx: &mut AsyncApp, secs: u64, mut cond: impl FnMut(&mut AsyncApp) -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if cond(cx) {
            return Some(t.elapsed());
        }
        sleep(cx, 50).await;
    }
    None
}

/// The state after the script, as text.
pub fn dump(app: &OrbitApp, log: &str, cx: &gpui::App) -> String {
    let mut out = String::new();
    let frames = terminal::FRAMES.load(Ordering::Relaxed).max(1);
    let _ = writeln!(
        out,
        "terminal frames: {frames}, avg layout+paint {} us, max {} us",
        terminal::FRAME_US_TOTAL.load(Ordering::Relaxed) / frames,
        terminal::FRAME_US_MAX.load(Ordering::Relaxed)
    );
    out.push_str(log);
    let _ = writeln!(out, "--- workspace: {}", app.workspace().map_or("-", |w| w.meta.name.as_str()));
    let tree = app.tree.read(cx);
    let _ = writeln!(out, "--- tree: {} rows, settled {}", tree.row_count(), tree.is_settled());
    out.push_str(&tree.rows_text(12));
    let _ = writeln!(out, "--- tabs ({}):", app.tabs.len());
    for (ix, tab) in app.tabs.iter().enumerate() {
        let kind = match tab.kind {
            TabKind::Terminal(_) => "terminal",
            TabKind::Editor(_) => "editor",
            TabKind::Diff(_) => "diff",
            TabKind::Review(_) => "review",
            TabKind::Page(_) => "page",
            TabKind::Tool(_) => "tool",
        };
        let _ = writeln!(
            out,
            "  [{ix}] {kind:8} {}{}{}{}",
            tab.title(cx),
            if app.active == Some(ix) { "  (active)" } else { "" },
            if tab.preview { "  (preview)" } else { "" },
            if tab.dirty(cx) { "  (dirty)" } else { "" },
        );
    }
    for server in app.lsp.running() {
        let _ = writeln!(
            out,
            "--- lsp [{}]: {} | cwd {} | open root {}",
            server.language,
            server.command,
            server.real_root.display(),
            server.open_root.display()
        );
        for line in server.log_tail(3) {
            let _ = writeln!(out, "  stderr: {line}");
        }
    }
    match app.active_tab().map(|t| &t.kind) {
        Some(TabKind::Editor(e)) => {
            let e = e.read(cx);
            let (line, col) = e.cursor(cx);
            let state = e.state.read(cx);
            let text = state.text();
            let _ = writeln!(out, "--- active editor: {} at {line}:{col}", e.path.display());
            let _ = writeln!(out, "dirty: {}, lsp attached: {}, lsp status: {}", e.is_dirty(), e.lsp_attached(), e.lsp_status());
            let _ = writeln!(out, "line {line}: {}", text.slice_line((line - 1) as usize).to_string().trim_end());
            let _ = writeln!(out, "selection: {:?}", state.selected_text().to_string());
            let _ = writeln!(out, "status: {}", e.status_text(cx));
            let _ = writeln!(out, "breadcrumb: {}", e.display_path.replace('/', " › "));
            if let Some(m) = &e.message {
                let _ = writeln!(out, "message: {m}");
            }
            if let Some(p) = e.picker_text() {
                let _ = write!(out, "picker: {p}");
            }
            if let Some(sig) = e.signature_text() {
                let _ = writeln!(out, "signature help: {sig}");
            }
            if let Some(b) = e.cpp_banner_text() {
                let _ = writeln!(out, "clangd banner: {b}");
            }
            let menu = state.completion_menu_state();
            if menu.open {
                let labels: Vec<_> = menu.items.iter().take(10).map(|i| i.label.as_str()).collect();
                let _ = writeln!(out, "completion menu ({} items, query {:?}): {}", menu.items.len(), menu.query, labels.join(" | "));
                if let Some(edit) = menu.items.first().and_then(|i| i.text_edit.as_ref()) {
                    let _ = writeln!(out, "  first item edit: {}", serde_json::to_string(edit).unwrap_or_default());
                }
            }
            let actions = state.code_action_menu_state();
            if actions.open {
                let titles: Vec<_> = actions.items.iter().take(10).map(|i| i.action.title.as_str()).collect();
                let _ = writeln!(out, "code actions ({}): {}", actions.items.len(), titles.join(" | "));
            }
            let _ = writeln!(out, "diagnostics: {}", e.diagnostics.len());
            for d in e.diagnostics.iter().take(8) {
                let _ = writeln!(out, "  {}:{} {:?} {}", d.range.start.line + 1, d.range.start.character + 1, d.severity, d.message.lines().next().unwrap_or_default());
            }
            if let Some(err) = &e.last_error {
                let _ = writeln!(out, "error: {err}");
            }
        }
        Some(TabKind::Terminal(t)) => {
            let t = t.read(cx);
            let _ = writeln!(out, "--- active terminal: {}", t.title());
            let _ = writeln!(out, "selection: {:?}", t.selection_text());
            let _ = writeln!(out, "copied: {:?}", t.last_copied);
            out.push_str(t.grid_text().trim_end());
            out.push('\n');
        }
        Some(TabKind::Diff(d)) => {
            let d = d.read(cx);
            let state = d.state.read(cx);
            let _ = writeln!(out, "--- active diff: {} mode {:?}, +{} -{}", d.path.display(), d.mode(cx), d.additions, d.deletions);
            for f in state.files() {
                let _ = writeln!(out, "  file {} {:?} +{} -{}", f.path(), f.status(), f.additions(), f.deletions());
            }
        }
        _ => {}
    }
    out.push_str(&crate::review::dump(app, cx));
    out.push_str(&crate::workspace_ui::dump(app, cx));
    out.push_str(&crate::pages::dump(app, cx));
    let commands = crate::editor::editor_actions();
    let unresolved: Vec<_> = commands.iter().filter(|c| cx.build_action(c.id, None).is_err()).map(|c| c.id).collect();
    let _ = writeln!(out, "--- editor actions: {}, unresolved: {unresolved:?}", commands.len());
    for c in &commands {
        let _ = writeln!(out, "  {:40} {:32} {} ({})", c.id, c.title, c.keys.join(" "), c.context);
    }
    out.push_str(&crate::steps::shell_dump(app, cx));
    out.push_str(&crate::run::dump(cx));
    dump_sessions(&mut out, cx);
    out
}

fn dump_sessions(out: &mut String, cx: &gpui::App) {
    let store = sessions::store(cx);
    let store = store.read(cx);
    let _ = writeln!(out, "--- sessions:");
    for (meta, view) in store.sessions() {
        let (status, exited, osc) = view.map_or(("closed", false, None), |v| {
            let v = v.read(cx);
            (v.status.label(), v.exited(), v.program_title())
        });
        let _ = writeln!(
            out,
            "  {} {} [{}] {} in {} args {:?}{}{}",
            meta.id,
            meta.session_name,
            meta.label,
            status,
            meta.workspace,
            meta.args,
            if exited { " (exited)" } else { "" },
            osc.map(|t| format!(" title {t:?}")).unwrap_or_default(),
        );
    }
    let _ = writeln!(out, "--- inbox ({} unread):", store.inbox.iter().filter(|i| !i.read).count());
    for i in &store.inbox {
        let _ = writeln!(out, "  {:?} {} | {} | {}{}", i.kind, i.key, i.title, i.detail, if i.read { " (read)" } else { "" });
    }
    let _ = writeln!(out, "--- recent:");
    for r in &store.recent {
        let _ = writeln!(out, "  {} {} {} in {}", r.agent, r.resume_id, r.session_name, r.workspace);
    }
}

/// Starts the scripted steps that happen at window creation.
pub fn start(script: &Script, app: &mut OrbitApp, window: &mut gpui::Window, cx: &mut gpui::Context<OrbitApp>) {
    if let Some(name) = &script.workspace {
        match app.workspaces.iter().position(|w| &w.meta.name == name) {
            Some(ix) => {
                app.select_workspace(ix, cx);
                app.tree.update(cx, |tree, cx| tree.expand_levels(script.expand, cx));
            }
            None => eprintln!("no workspace named {name}"),
        }
    }
    for _ in 0..script.terminals {
        app.new_terminal(script.cmd.clone(), window, cx);
    }
    for cmd in &script.sessions {
        let cmd = (cmd != "shell").then_some(cmd.as_str());
        sessions::new_session(app, cmd, None, window, cx);
    }
}

/// The steps that need time: run on the app's async context after the window opened.
pub async fn run(script: Script, window: WindowHandle<Root>, app: Entity<OrbitApp>, cx: &mut AsyncApp) {
    let any_window = AnyWindowHandle::from(window);
    let started = Instant::now();
    let mut log = String::new();
    // Let the first frames happen.
    sleep(cx, 500).await;

    if script.expand > 0 {
        wait_until(cx, 60, |cx| app.read_with(cx, |a, cx| a.tree.read(cx).is_settled())).await;
        let (rows, took) = app.read_with(cx, |a, cx| (a.tree.read(cx).row_count(), a.tree.read(cx).last_load_time));
        let _ = writeln!(log, "tree: expanded {} levels in the background, {rows} rows in {took:?}", script.expand);
    }

    let mut opened = None;
    for rel in &script.open {
        let path = app.read_with(cx, |a, _| a.workspace().map_or(rel.clone(), |w| w.dir.join(rel)));
        opened = any_window
            .update(cx, |_, window, cx| app.update(cx, |a, cx| a.open_file(path.clone(), script.preview, window, cx)))
            .ok()
            .flatten();
        if opened.is_none() {
            let _ = writeln!(log, "open {}: failed", path.display());
        }
    }
    if let Some(editor) = opened {
        if script.wait_lsp > 0 {
            let attached = wait_until(cx, script.wait_lsp, |cx| editor.read_with(cx, |e, _| e.lsp_attached())).await;
            let _ = writeln!(log, "lsp attached after {attached:?}");
            let diags = wait_until(cx, script.wait_lsp, |cx| editor.read_with(cx, |e, _| !e.diagnostics.is_empty())).await;
            let _ = writeln!(log, "first diagnostics after {diags:?} (none is fine for a clean file)");
        }
        if let Some((line, col)) = script.cursor {
            let _ = any_window.update(cx, |_, window, cx| {
                editor.update(cx, |e, cx| {
                    let p = lsp_types::Position::new(line - 1, col - 1);
                    e.reveal(lsp_types::Range::new(p, p), window, cx)
                })
            });
            sleep(cx, 300).await;
        }
    }

    let input = input_steps(&script);
    if !input.is_empty() {
        let t = Instant::now();
        let n = input.len();
        for step in input {
            match step {
                Step::Key(ks) => {
                    let _ = any_window.update(cx, |_, window, cx| window.dispatch_keystroke(ks, cx));
                }
                Step::Action(name) => {
                    let _ = any_window.update(cx, |_, window, cx| match cx.build_action(&name, None) {
                        Ok(action) => window.dispatch_action(action, cx),
                        Err(e) => {
                            let _ = writeln!(log, "action {name}: {e}");
                        }
                    });
                }
                Step::Wait => {
                    sleep(cx, 100).await;
                    // A hidden or occluded window gets no frame callbacks; overlays
                    // (completion, code actions) sync their state when drawn.
                    let _ = any_window.update(cx, |_, window, cx| window.draw(cx).clear(cx));
                }
            }
            // Yield a frame so each key is handled (and repainted) like real input.
            sleep(cx, 4).await;
        }
        let _ = writeln!(log, "dispatched {n} keystrokes in {} ms", t.elapsed().as_millis());
        // Asynchronous results (go to definition) land after this.
        sleep(cx, 1500).await;
    }

    if script.ctrl_click {
        let target = app.read_with(cx, |a, cx| {
            let e = a.active_editor()?;
            let state = e.read(cx).state.read(cx);
            let range = state.text().word_range(state.cursor())?;
            let b = state.range_to_bounds(&range)?;
            Some((e.read(cx).path.clone(), b.center(), state.text().slice(range).to_string()))
        });
        match target {
            Some((from, pos, word)) => {
                let _ = writeln!(log, "ctrl-click on {word:?} at {pos:?} in {}", from.display());
                ctrl_click(any_window, pos, cx).await;
            }
            None => {
                let _ = writeln!(log, "ctrl-click: no word under the cursor on screen");
            }
        }
    }

    if script.hover {
        let task = any_window
            .update(cx, |_, window, cx| {
                let editor = app.read(cx).active_editor()?;
                Some(editor.update(cx, |e, cx| e.hover_at_cursor(window, cx)))
            })
            .ok()
            .flatten();
        if let Some(task) = task {
            let text = task.await;
            let _ = writeln!(log, "hover: {}", text.as_deref().unwrap_or("(none)").replace('\n', "\n  | "));
        }
    }

    if script.diff {
        let _ = any_window.update(cx, |_, window, cx| {
            app.update(cx, |a, cx| {
                if let Some(path) = a.active_editor().map(|e| e.read(cx).path.clone()) {
                    a.open_diff(path, window, cx);
                }
            })
        });
        sleep(cx, 500).await;
    }

    if let Some((from, to)) = script.term_drag {
        let points = app.read_with(cx, |a, cx| {
            let t = a.active_terminal()?;
            let t = t.read(cx);
            Some((t.cell_position(from.0, from.1)?, t.cell_position(to.0, to.1)?))
        });
        match points {
            // Just left of the center: the drag starts on the cell's left half, so the cell is included.
            Some((a, b)) => drag(any_window, a - gpui::point(gpui::px(3.), gpui::px(0.)), b, cx).await,
            None => {
                let _ = writeln!(log, "term-drag: no painted terminal is active");
            }
        }
    }

    if script.scroll != 0 {
        app.update(cx, |a, cx| {
            if let Some(t) = a.active_terminal() {
                t.update(cx, |t, cx| t.scroll_lines(script.scroll, cx));
            }
        });
    }

    run_steps(&script.steps, any_window, &app, &mut log, cx).await;
    crate::steps::run(&script.shell_steps, any_window, &app, &mut log, cx).await;

    finish(script, &app, log, started, cx).await
}

/// Free-form steps, `;`-separated:
/// `wait:<text>[@secs]` until the active terminal shows `text` ·
/// `status:<status>[@secs]` until some session has that status ·
/// `action:<name>` dispatches a GPUI action by name (`sessions::NextSessionNeedingInput`) ·
/// `type:<text>` types into the focused view, `keys:<specs>` presses keys (`ctrl-c`) (`\n` is Enter) · `sleep:<ms>` ·
/// `dump` writes the sessions, inbox and recent list into the log.
/// Waits also record how late the UI thread ran its timers (a blocked UI).
async fn run_steps(steps: &[String], window: AnyWindowHandle, app: &Entity<OrbitApp>, log: &mut String, cx: &mut AsyncApp) {
    let mut worst_lag = Duration::ZERO;
    for step in steps {
        let (kind, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        let (arg, secs) = match arg.rsplit_once('@') {
            Some((a, n)) if n.parse::<u64>().is_ok() => (a, n.parse().unwrap()),
            _ => (arg, 30),
        };
        match kind {
            "wait" | "status" => {
                let t = Instant::now();
                let mut found = false;
                while t.elapsed() < Duration::from_secs(secs) {
                    found = app.read_with(cx, |a, cx| match kind {
                        "wait" => a.active_terminal().is_some_and(|v| v.read(cx).grid_text().contains(arg)),
                        _ => sessions::store(cx).read(cx).sessions().any(|(_, v)| v.is_some_and(|v| v.read(cx).status.label() == arg)),
                    });
                    if found {
                        break;
                    }
                    let tick = Instant::now();
                    sleep(cx, 50).await;
                    worst_lag = worst_lag.max(tick.elapsed().saturating_sub(Duration::from_millis(50)));
                }
                let _ = writeln!(log, "step {step}: {} after {} ms", if found { "found" } else { "TIMED OUT" }, t.elapsed().as_millis());
            }
            "action" => {
                let built = cx.update(|cx| cx.build_action(arg, None));
                match built {
                    Ok(action) => {
                        let _ = window.update(cx, |_, window, cx| window.dispatch_action(action, cx));
                        let _ = writeln!(log, "step {step}: dispatched");
                    }
                    Err(e) => {
                        let _ = writeln!(log, "step {step}: {e}");
                    }
                }
                sleep(cx, 300).await;
            }
            "type" | "keys" => {
                let typed = match kind {
                    "type" => Script { typed: Some(arg.replace("\\n", "\n")), ..Default::default() },
                    _ => Script { keys: arg.split_whitespace().map(str::to_string).collect(), ..Default::default() },
                };
                for step in input_steps(&typed) {
                    match step {
                        Step::Key(ks) => {
                            let _ = window.update(cx, |_, window, cx| window.dispatch_keystroke(ks, cx));
                        }
                        Step::Action(name) => {
                            if let Ok(action) = cx.update(|cx| cx.build_action(&name, None)) {
                                let _ = window.update(cx, |_, window, cx| window.dispatch_action(action, cx));
                            }
                        }
                        Step::Wait => sleep(cx, 100).await,
                    }
                    sleep(cx, 4).await;
                }
                let _ = writeln!(log, "step {step}: typed");
            }
            "sleep" => sleep(cx, arg.parse().unwrap_or(0)).await,
            "dump" => {
                let _ = writeln!(log, "step dump:");
                cx.update(|cx| dump_sessions(log, cx));
            }
            _ => {
                let _ = writeln!(log, "step {step}: unknown");
            }
        }
    }
    if !steps.is_empty() {
        let _ = writeln!(log, "ui timer lag while waiting: max {} ms", worst_lag.as_millis());
    }
}

fn send(window: AnyWindowHandle, event: PlatformInput, cx: &mut AsyncApp) {
    let _ = window.update(cx, |_, window, cx| window.dispatch_event(event, cx));
}

async fn ctrl_click(window: AnyWindowHandle, pos: Point<Pixels>, cx: &mut AsyncApp) {
    // Hovering with Ctrl held asks the server for the definition first,
    // which is what makes the symbol clickable.
    send(window, PlatformInput::MouseMove(MouseMoveEvent { position: pos, pressed_button: None, modifiers: ctrl() }), cx);
    sleep(cx, 1500).await;
    send(
        window,
        PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: pos, modifiers: ctrl(), click_count: 1, first_mouse: false }),
        cx,
    );
    send(window, PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: pos, modifiers: ctrl(), click_count: 1 }), cx);
    sleep(cx, 1000).await;
}

async fn drag(window: AnyWindowHandle, from: Point<Pixels>, to: Point<Pixels>, cx: &mut AsyncApp) {
    let none = Modifiers::default();
    send(
        window,
        PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: from, modifiers: none, click_count: 1, first_mouse: false }),
        cx,
    );
    sleep(cx, 20).await;
    let steps = 8;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = from + (to - from) * t;
        send(window, PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(MouseButton::Left), modifiers: none }), cx);
        sleep(cx, 10).await;
    }
    send(window, PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: to, modifiers: none, click_count: 1 }), cx);
    sleep(cx, 100).await;
}

async fn finish(script: Script, app: &Entity<OrbitApp>, log: String, started: Instant, cx: &mut AsyncApp) {
    let Some(secs) = script.exit_after else { return };
    let rest = Duration::from_secs(secs).saturating_sub(started.elapsed() + Duration::from_millis(500));
    cx.background_executor().timer(rest).await;
    let text = app.read_with(cx, |a, cx| dump(a, &log, cx));
    eprint!("{text}");
    if let Some(path) = &script.dump {
        let _ = std::fs::write(path, &text);
    }
    app.update(cx, |a, _| a.lsp.shutdown_all());
    cx.update(|cx| cx.quit());
}
