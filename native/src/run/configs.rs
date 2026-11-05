//! Edit Configurations (Rider's Run/Debug Configurations, minimal): every
//! configuration of the workspace's repos on the left (saved and discovered
//! CMake executables and Cargo bins), the selected one's name, target, executable,
//! arguments, working directory, environment, where it is stored and
//! whether it builds first on the right. OK / Apply write `.orbit/run.json`
//! ("Store as project file") or `.orbit/run.local.json` through
//! `orbit_core::run::save_config`, which keeps the files' other fields and
//! line endings. Opened from the configuration list and `run.editConfigurations`.
use super::{store, RunTarget};
use crate::app::OrbitApp;
use crate::overlay::MenuItem;
use crate::probe::probe;
use crate::theme::{self, c};
use crate::workspace_ui::kit::{self, Kind};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, FocusHandle, FontWeight, MouseButton, MouseDownEvent, SharedString, Subscription, WeakEntity, Window};
use gpui_component::input::{InputEvent, InputState};
use orbit_core::cmake_api::CmakeTarget;
use orbit_core::run::{Origin, RunConfig};
use std::path::PathBuf;

struct Repo {
    name: String,
    root: PathBuf,
    targets: Vec<CmakeTarget>,
}

#[derive(Clone)]
struct Item {
    repo: usize,
    config: RunConfig,
    origin: Origin,
    /// Its name in the file it is saved in (None: discovered or new).
    saved_as: Option<String>,
    shared: bool,
    /// As loaded or last saved, to tell what changed.
    original: Option<(RunConfig, bool)>,
}

impl Item {
    fn dirty(&self) -> bool {
        match &self.original {
            Some((config, shared)) => *config != self.config || *shared != self.shared,
            None => true,
        }
    }
}

struct EnvRow {
    key: Entity<InputState>,
    value: Entity<InputState>,
}

struct Form {
    name: Entity<InputState>,
    build: Entity<InputState>,
    program: Entity<InputState>,
    args: Entity<InputState>,
    cwd: Entity<InputState>,
    env: Vec<EnvRow>,
}

pub struct RunConfigs {
    app: WeakEntity<OrbitApp>,
    workspace: String,
    repos: Vec<Repo>,
    items: Vec<Item>,
    /// Saved configurations removed in the dialog, applied with the rest.
    removed: Vec<(usize, String)>,
    selected: Option<usize>,
    form: Form,
    error: Option<SharedString>,
    /// What the last Apply saved, until something changes again.
    saved: Option<SharedString>,
    focus: FocusHandle,
    /// Each env name input's last length: a jump of more than one character is a paste.
    key_lens: std::collections::HashMap<gpui::EntityId, usize>,
    _subs: Vec<Subscription>,
}

fn input(window: &mut Window, cx: &mut Context<RunConfigs>, placeholder: &str) -> Entity<InputState> {
    let p = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(p))
}

fn text(i: &Entity<InputState>, cx: &App) -> String {
    i.read(cx).value().trim().to_string()
}

/// Program arguments as one line: split on spaces, double quotes group.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quoted, mut any) = (String::new(), false, false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            ' ' | '\t' if !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => {
                cur.push(ch);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

pub fn join_args(args: &[String]) -> String {
    args.iter().map(|a| if a.is_empty() || a.contains([' ', '\t']) { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}

impl RunConfigs {
    /// The dialog over the run store's setups, `select`ing a configuration.
    pub fn new(app: WeakEntity<OrbitApp>, select: Option<RunTarget>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = store(cx).read(cx);
        let workspace = s.target.as_ref().map(|t| t.workspace.clone()).or_else(|| s.workspace_name()).unwrap_or_default();
        let mut repos = Vec::new();
        let mut items = Vec::new();
        for (repo, setup) in &s.setups {
            let ix = repos.len();
            repos.push(Repo { name: repo.clone(), root: setup.root.clone(), targets: setup.targets.clone() });
            for c in &setup.configs {
                let shared = c.origin == Origin::Project;
                let saved_as = (c.origin != Origin::Discovered).then(|| c.name.clone());
                items.push(Item { repo: ix, config: c.raw.clone(), origin: c.origin, saved_as, shared, original: Some((c.raw.clone(), shared)) });
            }
        }
        let selected = select
            .and_then(|t| items.iter().position(|i| repos[i.repo].name == t.repo && i.config.name == t.name))
            .or((!items.is_empty()).then_some(0));
        let form = Form {
            name: input(window, cx, "Name"),
            build: input(window, cx, "make all"),
            program: input(window, cx, ""),
            args: input(window, cx, "--flag value \"with spaces\""),
            cwd: input(window, cx, ""),
            env: vec![],
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut this = Self {
            app,
            workspace,
            repos,
            items,
            removed: vec![],
            selected: None,
            form,
            error: None,
            saved: None,
            focus,
            key_lens: Default::default(),
            _subs: vec![],
        };
        // The list shows the name as it is typed, and every edit updates the unsaved marks and Apply.
        let f = &this.form;
        for i in [f.name.clone(), f.build.clone(), f.program.clone(), f.args.clone(), f.cwd.clone()] {
            this.watch(&i, cx);
        }
        this.select(selected, window, cx);
        this
    }

    // ---------- the form ----------

    /// An edit in `input` moves the form into the configuration (its unsaved mark, Apply).
    fn watch(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        self._subs.push(cx.subscribe(input, |t: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                t.commit(cx);
                cx.notify();
            }
        }));
    }

    /// Moves the inputs' text into the selected configuration.
    fn commit(&mut self, cx: &App) {
        let Some(ix) = self.selected else { return };
        let f = &self.form;
        let item = &mut self.items[ix];
        item.config.name = text(&f.name, cx);
        if item.config.kind == "command" {
            item.config.build = text(&f.build, cx);
        }
        item.config.program = text(&f.program, cx);
        item.config.args = split_args(&text(&f.args, cx));
        item.config.cwd = text(&f.cwd, cx);
        item.config.env = f.env.iter().map(|r| (text(&r.key, cx), r.value.read(cx).value().to_string())).filter(|(k, _)| !k.is_empty()).collect();
    }

    fn select(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        self.selected = ix;
        self.error = None;
        let config = ix.map(|ix| self.items[ix].config.clone()).unwrap_or_default();
        let (default_program, default_cwd) = ix.map(|ix| self.defaults(ix)).unwrap_or_default();
        let set = |i: &Entity<InputState>, v: String, window: &mut Window, cx: &mut Context<Self>| i.update(cx, |i, cx| i.set_value(v, window, cx));
        set(&self.form.name, config.name.clone(), window, cx);
        set(&self.form.build, config.build.clone(), window, cx);
        set(&self.form.program, config.program.clone(), window, cx);
        set(&self.form.args, join_args(&config.args), window, cx);
        set(&self.form.cwd, config.cwd.clone(), window, cx);
        self.form.program.update(cx, |i, cx| i.set_placeholder(default_program.unwrap_or_else(|| "Path to the program".into()), window, cx));
        self.form.cwd.update(cx, |i, cx| i.set_placeholder(default_cwd.unwrap_or_else(|| "${repo}".into()), window, cx));
        self.form.env.clear();
        for (k, v) in &config.env {
            self.add_env_row(k, v, None, window, cx);
        }
        cx.notify();
    }

    /// What an empty Executable and Working directory mean for item `ix`.
    fn defaults(&self, ix: usize) -> (Option<String>, Option<String>) {
        let item = &self.items[ix];
        orbit_core::run::defaults(&item.config, &self.repos[item.repo].targets)
    }

    fn add_env_row(&mut self, key: &str, value: &str, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let k = cx.new(|cx| InputState::new(window, cx).placeholder("NAME").default_value(key.to_string()));
        let v = cx.new(|cx| InputState::new(window, cx).placeholder("value").default_value(value.to_string()));
        // Pasting `K=V;K2=V2` into a name splits it into rows.
        self._subs.push(cx.subscribe_in(&k, window, |t: &mut Self, input, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                t.split_pasted(input.clone(), window, cx);
            }
        }));
        self.watch(&k, cx);
        self.watch(&v, cx);
        let row = EnvRow { key: k, value: v };
        match at {
            Some(at) => self.form.env.insert(at.min(self.form.env.len()), row),
            None => self.form.env.push(row),
        }
    }

    fn split_pasted(&mut self, input: Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let raw = input.read(cx).value().to_string();
        let len = raw.chars().count();
        let before = self.key_lens.insert(input.entity_id(), len).unwrap_or(0);
        if !raw.contains('=') || len <= before + 1 {
            return;
        }
        let pairs = orbit_core::run::parse_env(&raw);
        let Some(row) = self.form.env.iter().position(|r| r.key == input) else { return };
        let Some(((k, v), rest)) = pairs.split_first() else { return };
        let value = self.form.env[row].value.clone();
        input.update(cx, |i, cx| i.set_value(k.clone(), window, cx));
        value.update(cx, |i, cx| i.set_value(v.clone(), window, cx));
        for (n, (k, v)) in rest.iter().enumerate() {
            self.add_env_row(k, v, Some(row + 1 + n), window, cx);
        }
        cx.notify();
    }

    fn set_kind(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let Some(ix) = self.selected else { return };
        self.items[ix].config.kind = kind.into();
        self.selected = None;
        self.select(Some(ix), window, cx);
    }

    /// Checks or unchecks one of a compound's configurations (checked ones launch in the order checked).
    fn toggle_part(&mut self, name: String, cx: &mut Context<Self>) {
        self.commit(cx);
        let Some(ix) = self.selected else { return };
        let list = &mut self.items[ix].config.configs;
        match list.iter().position(|n| *n == name) {
            Some(at) => {
                list.remove(at);
            }
            None => list.push(name),
        }
        cx.notify();
    }

    fn compound_problem(&self, ix: usize) -> Option<String> {
        let i = &self.items[ix];
        let all = self.items.iter().map(|o| (self.repos[o.repo].name.as_str(), &o.config));
        i.config.is_compound().then(|| orbit_core::run::compound_parts(&i.config, &self.repos[i.repo].name, all).1.into_iter().next()).flatten()
    }

    fn set_target(&mut self, target: String, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let Some(ix) = self.selected else { return };
        self.items[ix].config.target = target;
        self.items[ix].config.kind = kind.into();
        // Reload the form: the placeholders follow the target.
        self.selected = None;
        self.select(Some(ix), window, cx);
    }

    fn open_targets(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let repo = &self.repos[self.items[ix].repo];
        let cargo = self.items[ix].config.kind == "cargo";
        let mut targets: Vec<&CmakeTarget> = repo.targets.iter().filter(|t| t.package.is_some() == cargo).collect();
        targets.sort_by_key(|t| if t.runnable() { 0 } else if t.kind == "UTILITY" { 2 } else { 1 });
        let this = cx.entity().downgrade();
        let items: Vec<MenuItem> = targets
            .into_iter()
            .map(|t| {
                let (name, this) = (t.name.clone(), this.clone());
                let label = if t.runnable() && t.kind != "example" { t.name.clone() } else { format!("{} · {}", t.name, kind_label(&t.kind)) };
                let kind = if cargo { "cargo" } else { "cmake" };
                MenuItem::new(label, move |_, window, cx| {
                    let name = name.clone();
                    let _ = this.update(cx, |t, cx| t.set_target(name, kind, window, cx));
                })
            })
            .collect();
        let at = ev.position;
        let _ = self.app.update(cx, |a, cx| a.open_menu(at, items, window, cx));
    }

    // ---------- list actions ----------

    fn add(&mut self, duplicate: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let base = self.selected.map(|ix| self.items[ix].clone());
        let repo = base.as_ref().map_or(0, |b| b.repo);
        if self.repos.is_empty() {
            return;
        }
        let mut config = match (&base, duplicate) {
            (Some(b), true) => b.config.clone(),
            _ => RunConfig { kind: "cmake".into(), preset: "${auto}".into(), ..Default::default() },
        };
        let stem = if duplicate { format!("{} (copy)", config.name) } else { "Unnamed".into() };
        config.name = self.unique_name(repo, &stem);
        let shared = duplicate && base.as_ref().is_some_and(|b| b.shared);
        let origin = if shared { Origin::Project } else { Origin::Local };
        self.items.push(Item { repo, config, origin, saved_as: None, shared, original: None });
        let ix = self.items.len() - 1;
        self.selected = None;
        self.select(Some(ix), window, cx);
    }

    fn unique_name(&self, repo: usize, stem: &str) -> String {
        let taken = |n: &str| self.items.iter().any(|i| i.repo == repo && i.config.name == n);
        if !taken(stem) {
            return stem.to_string();
        }
        (2..).map(|n| format!("{stem} {n}")).find(|n| !taken(n)).unwrap_or_default()
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let item = &self.items[ix];
        if item.origin == Origin::Discovered && item.saved_as.is_none() {
            return;
        }
        if let Some(name) = &item.saved_as {
            self.removed.push((item.repo, name.clone()));
        }
        self.items.remove(ix);
        self.selected = None;
        let next = (!self.items.is_empty()).then(|| ix.min(self.items.len() - 1));
        self.select(next, window, cx);
    }

    // ---------- saving ----------

    /// Something to save: an edited, new or removed configuration.
    fn has_changes(&self, cx: &App) -> bool {
        !self.removed.is_empty() || (0..self.items.len()).any(|ix| self.unsaved(ix, cx))
    }

    /// Item `ix` differs from its file, or its form has a row the
    /// configuration can't hold yet (a value without a name).
    fn unsaved(&self, ix: usize, cx: &App) -> bool {
        self.items[ix].dirty() || (self.selected == Some(ix) && env_problem(&self.env_rows(cx)).is_some())
    }

    fn env_rows(&self, cx: &App) -> Vec<(String, String)> {
        self.form.env.iter().map(|r| (text(&r.key, cx), r.value.read(cx).value().to_string())).collect()
    }

    /// What can't be saved, and the configuration it is in.
    fn validate(&self, cx: &App) -> Result<(), (Option<usize>, String)> {
        for (ix, i) in self.items.iter().enumerate() {
            if i.config.name.trim().is_empty() {
                return Err((Some(ix), "A configuration needs a name".into()));
            }
            if self.items.iter().skip(ix + 1).any(|o| o.repo == i.repo && o.config.name == i.config.name) {
                return Err((Some(ix), format!("Two configurations are named '{}'", i.config.name)));
            }
            if let Some(e) = self.compound_problem(ix) {
                return Err((Some(ix), e));
            }
        }
        // The form's rows: an empty name would be dropped silently.
        env_problem(&self.env_rows(cx)).map_or(Ok(()), |e| Err((self.selected, e)))
    }

    /// Writes what changed; true when everything was saved. A failure keeps
    /// the edits (and the unsaved marks) and says why in the footer.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.commit(cx);
        self.error = None;
        self.saved = None;
        if let Err((at, e)) = self.validate(cx) {
            if at.is_some() && at != self.selected {
                self.select(at, window, cx);
            }
            self.error = Some(e.into());
            cx.notify();
            return false;
        }
        let mut renamed: Option<(String, String, String)> = None;
        let mut done: Vec<(String, &'static str)> = vec![];
        let mut removed = 0;
        for (repo, name) in std::mem::take(&mut self.removed) {
            match orbit_core::run::remove_config(&self.repos[repo].root, &name) {
                Ok(()) => removed += 1,
                Err(e) => {
                    self.error = Some(format!("Removing '{name}': {e}").into());
                    self.removed.push((repo, name));
                }
            }
        }
        for item in self.items.iter_mut().filter(|i| i.dirty()) {
            let root = &self.repos[item.repo].root;
            let file = if item.shared { ".orbit/run.json" } else { ".orbit/run.local.json" };
            match orbit_core::run::save_config(root, &item.config, item.shared, item.saved_as.as_deref()) {
                Ok(()) => {
                    done.push((item.config.name.clone(), file));
                    if let Some(old) = item.saved_as.as_ref().filter(|n| **n != item.config.name) {
                        renamed = Some((self.repos[item.repo].name.clone(), old.clone(), item.config.name.clone()));
                    } else if item.origin == Origin::Discovered {
                        renamed = renamed.or(Some((self.repos[item.repo].name.clone(), item.config.target.clone(), item.config.name.clone())));
                    }
                    item.saved_as = Some(item.config.name.clone());
                    item.origin = if item.shared { Origin::Project } else { Origin::Local };
                    item.original = Some((item.config.clone(), item.shared));
                }
                Err(e) => self.error = Some(format!("Saving '{}' to {file}: {e}", item.config.name).into()),
            }
        }
        let ok = self.error.is_none();
        if let Some(msg) = saved_message(&done, removed) {
            if ok {
                let _ = self.app.update(cx, |a, cx| a.toast(crate::toast::ToastKind::Success, msg.clone(), None, cx));
            }
            self.saved = Some(msg.into());
        }
        store(cx).update(cx, |s, cx| s.configs_saved(renamed, cx));
        cx.notify();
        ok
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        store(cx).update(cx, |s, cx| {
            s.dialog = None;
            cx.notify();
        });
        let _ = self.app.update(cx, |_, cx| cx.notify());
    }

    /// The dialog for the self-test dump.
    pub fn dump(&self, cx: &App) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let names: Vec<String> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, i)| {
                let repo = if self.repos.len() > 1 { format!("{}/", self.repos[i.repo].name) } else { String::new() };
                format!("{repo}{} [{:?}{}]", i.config.name, i.origin, if self.unsaved(ix, cx) { ", changed" } else { "" })
            })
            .collect();
        let _ = writeln!(out, "--- run configurations dialog ({}): {names:?}", self.workspace);
        if let Some(ix) = self.selected {
            let i = &self.items[ix];
            let f = &self.form;
            let env: Vec<String> = f.env.iter().map(|r| format!("{}={}", text(&r.key, cx), r.value.read(cx).value())).collect();
            let _ = writeln!(
                out,
                "  selected {} | target {} | program {:?} | args {:?} | cwd {:?} | env {env:?} | project file {} | build first {} | allow multiple {} | kind {} {:?} | file {}",
                text(&f.name, cx),
                i.config.target,
                text(&f.program, cx),
                text(&f.args, cx),
                text(&f.cwd, cx),
                i.shared,
                i.config.build_before_launch.unwrap_or(true),
                i.config.allow_multiple,
                i.config.kind,
                i.config.configs,
                self.repos[i.repo].root.join(".orbit").join(if i.shared { "run.json" } else { "run.local.json" }).display(),
            );
        }
        if let Some(e) = &self.error {
            let _ = writeln!(out, "  error: {e}");
        }
        if let Some(m) = &self.saved {
            let _ = writeln!(out, "  saved: {m}");
        }
        let _ = writeln!(out, "  apply enabled {}", self.has_changes(cx));
        out
    }
}

/// A problem with environment rows (name, value): a value without a name, a
/// name a shell can't set, or the same name twice.
fn env_problem(rows: &[(String, String)]) -> Option<String> {
    for (n, (k, v)) in rows.iter().enumerate() {
        if k.is_empty() {
            if !v.trim().is_empty() {
                return Some(format!("Environment variable {} has a value but no name", n + 1));
            }
            continue;
        }
        if k.contains(['=', ' ', '\t']) || k.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(format!("'{k}' is not a valid environment variable name"));
        }
        if rows[..n].iter().any(|(o, _)| o == k) {
            return Some(format!("The environment variable {k} is set twice"));
        }
    }
    None
}

/// The footer's (and the toast's) line after a save.
fn saved_message(done: &[(String, &'static str)], removed: usize) -> Option<String> {
    let removed_part = match removed {
        0 => None,
        1 => Some("removed 1 configuration".to_string()),
        n => Some(format!("removed {n} configurations")),
    };
    let saved_part = match done {
        [] => None,
        [(name, file)] => Some(format!("Saved '{name}' to {file}")),
        _ => {
            let mut files: Vec<&str> = done.iter().map(|(_, f)| *f).collect();
            files.sort_unstable();
            files.dedup();
            Some(format!("Saved {} configurations to {}", done.len(), files.join(" and ")))
        }
    };
    match (saved_part, removed_part) {
        (Some(s), Some(r)) => Some(format!("{s}; {r}")),
        (Some(s), None) => Some(s),
        (None, Some(r)) => Some(format!("{}{}", r[..1].to_uppercase(), &r[1..])),
        (None, None) => None,
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "EXECUTABLE" | "bin" => "executable",
        "example" => "example",
        "STATIC_LIBRARY" => "static library",
        "SHARED_LIBRARY" => "shared library",
        "MODULE_LIBRARY" => "module",
        "OBJECT_LIBRARY" => "objects",
        "INTERFACE_LIBRARY" => "interface",
        _ => "utility",
    }
}

/// A form row: the label column, then the control.
fn row(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(150.)).flex_none().text_size(px(12.5)).text_color(c(theme::TEXT_DIM)).child(label))
        .child(div().flex_1().min_w_0().child(control))
}

fn field(name: &'static str, i: &Entity<InputState>) -> impl IntoElement {
    div().relative().w_full().child(kit::input(i)).child(probe(format!("rc-input:{name}")))
}

fn hint(text: impl Into<SharedString>) -> gpui::Div {
    div().pl(px(162.)).mt(px(-6.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(text.into())
}

fn checkbox(id: &'static str, label: &'static str, on: bool, cx: &mut Context<RunConfigs>, f: fn(&mut Item)) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_size(px(13.))
        .child(kit::check(on, false))
        .child(label)
        .child(probe(id))
        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
            t.commit(cx);
            if let Some(ix) = t.selected {
                f(&mut t.items[ix]);
            }
            cx.notify();
        }))
}

impl RunConfigs {
    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let multi = self.repos.len() > 1;
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (r, repo) in self.repos.iter().enumerate() {
            if multi {
                rows.push(div().px(px(8.)).pt(px(8.)).pb(px(2.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(repo.name.clone()).into_any_element());
            }
            let discovered = if repo.targets.iter().any(|t| t.package.is_none()) { "CMake targets" } else { "Cargo targets" };
            for (group, label) in [(Origin::Project, "Project"), (Origin::Local, "Local"), (Origin::Discovered, discovered)] {
                let members: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].repo == r && self.items[i].origin == group).collect();
                if members.is_empty() {
                    continue;
                }
                rows.push(
                    div()
                        .px(px(8.))
                        .pt(px(8.))
                        .pb(px(3.))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(theme::TEXT_FAINT))
                        .child(label.to_uppercase())
                        .into_any_element(),
                );
                for ix in members {
                    let item = &self.items[ix];
                    let selected = self.selected == Some(ix);
                    let unsaved = self.unsaved(ix, cx);
                    let name = item.config.name.clone();
                    rows.push(
                        div()
                            .id(("rc-item", ix))
                            .relative()
                            .h(px(26.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(5.))
                            .text_size(px(12.5))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(c(theme::ACCENT_SOFT)).text_color(c(theme::TEXT)))
                            .when(!selected, |d| d.hover(|s| s.bg(c(theme::PANEL_RAISED))))
                            .child(div().flex_1().min_w_0().truncate().when(unsaved, |d| d.italic()).child(if name.is_empty() { "(unnamed)".to_string() } else { name.clone() }))
                            .when(unsaved, |d| d.child(div().text_size(px(10.)).text_color(c(theme::ACCENT)).child("●")))
                            .child(probe(format!("rc-item:{name}")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |t, _, window, cx| {
                                    if t.selected != Some(ix) {
                                        t.select(Some(ix), window, cx);
                                    }
                                }),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        let removable = self.selected.is_some_and(|ix| self.items[ix].saved_as.is_some() || self.items[ix].original.is_none());
        let tool = |id: &'static str, icon: &'static str, tip: &'static str| kit::hint(kit::icon_button(id, icon, 26., 13., false), tip);
        div()
            .w(px(230.))
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::BG))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(4.))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(tool("rc-add", "plus", "Add configuration").on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add(false, w, cx))))
                    .child(tool("rc-dup", "stash", "Duplicate").on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add(true, w, cx))))
                    .child(kit::disabled(tool("rc-remove", "trash", "Remove (saved configurations)"), !removable).on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        if removable {
                            t.remove(w, cx)
                        }
                    }))),
            )
            .child(div().id("rc-list").flex_1().min_h_0().overflow_y_scroll().p(px(4.)).children(rows))
    }

    fn render_form(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ix) = self.selected else {
            return div().flex_1().flex().items_center().justify_center().text_color(c(theme::TEXT_FAINT)).child("No configurations").into_any_element();
        };
        let item = &self.items[ix];
        let repo = &self.repos[item.repo];
        let target_label = if item.config.target.is_empty() { "None".to_string() } else { item.config.target.clone() };
        let cargo = item.config.kind == "cargo";
        let target_kind = repo.targets.iter().find(|t| t.name == item.config.target && t.package.is_some() == cargo).map(|t| kind_label(&t.kind));
        let has_targets = repo.targets.iter().any(|t| t.package.is_some() == cargo);
        let has_cargo = cargo || repo.targets.iter().any(|t| t.package.is_some());
        let command = item.config.kind == "command";
        let compound = item.config.is_compound();
        let kinds = [("cmake", "CMake target"), ("cargo", "Cargo target"), ("command", "Command"), ("compound", "Compound")].into_iter().filter(|(k, _)| has_cargo || *k != "cargo");
        let kinds = div().flex().gap(px(6.)).children(kinds.enumerate().map(|(n, (kind, label))| {
            let on = item.config.kind == kind;
            div()
                .id(("rc-kind", n))
                .relative()
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(c(if on { theme::ACCENT } else { theme::BORDER_STRONG }))
                .when(on, |d| d.bg(c(theme::ACCENT_SOFT)))
                .text_size(px(12.5))
                .text_color(c(if on { theme::TEXT } else { theme::TEXT_DIM }))
                .cursor_pointer()
                .child(label)
                .child(probe(format!("rc-kind:{kind}")))
                .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.set_kind(kind, window, cx)))
        }));
        // This repo's configurations first, then the other repos' (a compound can mix them).
        let mut parts: Vec<&Item> = self.items.iter().filter(|o| !o.config.is_compound() && !o.config.name.is_empty()).collect();
        parts.sort_by_key(|o| o.repo != item.repo);
        let several = self.repos.len() > 1;
        let checklist = div().flex().flex_col().gap(px(6.)).children(parts.into_iter().enumerate().map(|(n, o)| {
            let other = &self.repos[o.repo].name;
            let name = orbit_core::run::part_ref(&repo.name, other, &o.config.name);
            let order = item.config.configs.iter().position(|c| *c == name);
            let toggle = name.clone();
            let label = o.config.name.clone();
            let from = several.then(|| div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(other.clone()));
            div()
                .id(("rc-part", n))
                .relative()
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_size(px(13.))
                .child(kit::check(order.is_some(), false))
                .child(label)
                .children(from)
                .children(order.map(|o| div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(format!("#{}", o + 1))))
                .child(probe(format!("rc-part:{name}")))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.toggle_part(toggle.clone(), cx)))
        }));
        let problem = self.compound_problem(ix);
        let target = div()
            .id("rc-target")
            .relative()
            .h(px(theme::size::CONTROL))
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(6.))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .bg(c(theme::BG))
            .text_size(px(13.))
            .when(has_targets, |d| d.cursor_pointer())
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(target_label)
                    .children(target_kind.filter(|k| !matches!(*k, "executable" | "example")).map(|k| div().text_color(c(theme::WARN)).child(format!("{k} · builds, does not run")))),
            )
            .child(crate::workspace_ui::icons::icon("chevron-down", 12., c(theme::TEXT_FAINT)))
            .child(probe("rc-target"))
            .on_mouse_down(MouseButton::Left, cx.listener(|t, ev: &MouseDownEvent, window, cx| t.open_targets(ev, window, cx)));
        let env_rows = self.form.env.iter().enumerate().map(|(n, r)| {
            div()
                .flex()
                .gap(px(6.))
                .items_center()
                .child(div().w(px(220.)).flex_none().child(field_owned(format!("env-key:{n}"), &r.key)))
                .child(div().flex_1().min_w_0().child(field_owned(format!("env-value:{n}"), &r.value)))
                .child(kit::icon_button(SharedString::from(format!("rc-env-remove:{n}")), "x", 28., 12., true).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.commit(cx);
                    if n < t.form.env.len() {
                        t.form.env.remove(n);
                        t.commit(cx);
                    }
                    cx.notify();
                })))
        });
        let shared_file = if item.shared { ".orbit/run.json — shared, commit it" } else { ".orbit/run.local.json — yours, keep it out of git" };
        let browse = kit::button("rc-browse", "Browse…", Kind::Secondary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, window, cx| t.browse(window, cx)));
        let _ = window;
        let form = div()
            .id("rc-form")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(row("Name", field("name", &self.form.name)))
            .child(row("Kind", kinds));
        if compound {
            return form
                .child(row("Configurations", checklist))
                .child(hint(problem.unwrap_or_else(|| "Run and Debug launch the checked configurations in this order".into())))
                .child(div().h(px(1.)).bg(c(theme::BORDER)))
                .child(checkbox("rc-shared", "Store as project file", item.shared, cx, |i| i.shared = !i.shared))
                .child(div().mt(px(-8.)).pl(px(24.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(shared_file))
                .into_any_element();
        }
        form
            .when(!command, |d| d.child(row("Target", target)))
            .when(command, |d| d.child(row("Build command", field("build", &self.form.build))))
            .child(row("Executable", field("program", &self.form.program)))
            .child(hint("Empty: the target's artifact. Variables: ${buildDir} ${repo} ${exe} $HOME ~"))
            .child(row("Program arguments", field("args", &self.form.args)))
            .child(row("Working directory", div().flex().gap(px(6.)).child(div().flex_1().min_w_0().child(field("cwd", &self.form.cwd))).child(browse)))
            .child(hint("Empty: the artifact's folder (the repo for other configurations)"))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(div().w(px(150.)).flex_none().pt(px(8.)).text_size(px(12.5)).text_color(c(theme::TEXT_DIM)).child("Environment variables"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .children(env_rows)
                            .child(
                                div().flex().items_center().gap(px(10.)).child(kit::link("rc-env-add", "+ Add variable").on_click(cx.listener(|t, _: &ClickEvent, window, cx| {
                                    t.commit(cx);
                                    t.add_env_row("", "", None, window, cx);
                                    if let Some(r) = t.form.env.last() {
                                        r.key.update(cx, |i, cx| i.focus(window, cx));
                                    }
                                    cx.notify();
                                })))
                                .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child("Paste K=V;K2=V2 into a name to add several")),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(c(theme::BORDER)))
            .child(checkbox("rc-shared", "Store as project file", item.shared, cx, |i| i.shared = !i.shared))
            .child(div().mt(px(-8.)).pl(px(24.)).text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(shared_file))
            .child(checkbox("rc-build-first", "Before launch: Build", item.config.build_before_launch.unwrap_or(true), cx, |i| {
                let on = i.config.build_before_launch.unwrap_or(true);
                i.config.build_before_launch = if on { Some(false) } else { None };
            }))
            .child(checkbox("rc-allow-multiple", "Allow multiple instances", item.config.allow_multiple, cx, |i| i.config.allow_multiple = !i.config.allow_multiple))
            .into_any_element()
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::terminal::CLIPBOARD_DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
            return; // self-test: no native dialogs
        }
        let input = self.form.cwd.clone();
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Working directory".into()) });
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.first().map(|p| p.display().to_string()) else { return };
            let _ = input.update_in(cx, |i, w, cx| i.set_value(path, w, cx));
        })
        .detach();
    }
}

fn field_owned(name: String, i: &Entity<InputState>) -> impl IntoElement {
    div().relative().w_full().child(kit::input(i)).child(probe(format!("rc-input:{name}")))
}

impl Render for RunConfigs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        let height = (vp.height - px(80.)).min(px(620.));
        let changes = self.has_changes(cx);
        if changes {
            self.saved = None;
        }
        let footer_status = match (&self.error, &self.saved) {
            (Some(e), _) => div()
                .id("rc-status")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(c(theme::DANGER))
                .child(crate::workspace_ui::icons::icon("x", 13., c(theme::DANGER)))
                .child(div().min_w_0().truncate().child(e.clone())),
            (None, Some(m)) => div()
                .id("rc-status")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(c(theme::OK))
                .child(crate::workspace_ui::icons::icon("check", 13., c(theme::OK)))
                .child(div().min_w_0().truncate().child(m.clone())),
            (None, None) => div()
                .id("rc-status")
                .flex_1()
                .min_w_0()
                .text_size(px(12.5))
                .text_color(c(theme::TEXT_FAINT))
                .when(changes, |d| d.child("Unsaved changes")),
        };
        kit::modal("run-configs", 900.)
            .h(height)
            .track_focus(&self.focus)
            .key_context("OrbitRunConfigs")
            .font_family(theme::UI_FONT)
            .text_size(px(theme::size::FONT))
            .on_key_down(cx.listener(|t, ev: &gpui::KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    t.close(cx);
                }
            }))
            .child(kit::modal_header("Run/Debug Configurations"))
            .child(div().flex_1().min_h_0().flex().child(self.render_list(cx)).child(self.render_form(window, cx)))
            .child(
                kit::modal_footer()
                    .child(footer_status)
                    .child(kit::button("rc-cancel", "Cancel", Kind::Secondary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.close(cx))))
                    .child(kit::disabled(kit::button("rc-apply", "Apply", Kind::Secondary, kit::NORMAL), !changes).on_click(cx.listener(move |t, _: &ClickEvent, window, cx| {
                        if changes {
                            t.apply(window, cx);
                        }
                    })))
                    .child(kit::button("rc-ok", "OK", Kind::Primary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, window, cx| {
                        if t.apply(window, cx) {
                            t.close(cx);
                        }
                    }))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_round_trip() {
        let args = split_args("--map \"Ricarten Town\" -v  x");
        assert_eq!(args, ["--map", "Ricarten Town", "-v", "x"]);
        assert_eq!(join_args(&args), "--map \"Ricarten Town\" -v x");
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn env_rows_that_cannot_be_saved() {
        let row = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(env_problem(&[row("A", "1"), row("", "")]), None);
        assert!(env_problem(&[row("", "orphan")]).unwrap().contains("no name"));
        assert!(env_problem(&[row("A B", "1")]).unwrap().contains("not a valid"));
        assert!(env_problem(&[row("A=B", "1")]).unwrap().contains("not a valid"));
        assert!(env_problem(&[row("1X", "1")]).unwrap().contains("not a valid"));
        assert!(env_problem(&[row("A", "1"), row("A", "2")]).unwrap().contains("twice"));
    }

    #[test]
    fn the_saved_line_names_the_files() {
        let one = [("game".to_string(), ".orbit/run.local.json")];
        assert_eq!(saved_message(&one, 0).unwrap(), "Saved 'game' to .orbit/run.local.json");
        let two = [("a".to_string(), ".orbit/run.json"), ("b".to_string(), ".orbit/run.local.json")];
        assert_eq!(saved_message(&two, 1).unwrap(), "Saved 2 configurations to .orbit/run.json and .orbit/run.local.json; removed 1 configuration");
        assert_eq!(saved_message(&[], 2).unwrap(), "Removed 2 configurations");
        assert_eq!(saved_message(&[], 0), None);
    }
}
