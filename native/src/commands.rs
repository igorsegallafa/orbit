//! Commands and the keymap: every action a user can run by name or shortcut.
//! The command palette (Ctrl+Shift+A) lists what is registered, with its keys.
//!
//! A stage adds its commands from an `init(cx)` called in `main.rs`:
//!
//! ```ignore
//! commands::register(cx, Command::new("app.toggleDock", "Toggle Files Panel")
//!     .run(|app, window, cx| app.toggle_dock(window, cx)));
//! // A key that only applies in a view: bind its own action in its key context.
//! commands::register(cx, Command::new("editor.save", "Save").keys(["ctrl-s"]).context("OrbitEditor").action(Save));
//! ```
//!
//! Keys use GPUI's syntax (`ctrl-shift-t`, `alt-left`, `shift shift` for a
//! double Shift). A command registered again under the same id replaces the
//! old one, so a later stage can take over a key (Ctrl+Shift+T: a new session
//! instead of a plain terminal).
use crate::app::OrbitApp;
use gpui::{Action, App, Context, Global, KeyBinding, KeyBindingContextPredicate, SharedString, Window};
use std::rc::Rc;

/// Runs an app command by id. `from_key`: pressed through its shortcut, so
/// `key_when` applies and an inapplicable key goes on to the focused view.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = orbit, no_json)]
pub struct RunCommand {
    pub id: SharedString,
    pub from_key: bool,
}

pub type Handler = Rc<dyn Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>)>;
pub type Predicate = Rc<dyn Fn(&OrbitApp, &App) -> bool>;

#[derive(Clone)]
pub enum Run {
    /// Runs on the root view.
    App(Handler),
    /// Dispatched to the focused view; the key binds the action itself.
    Action(Rc<dyn Action>),
}

#[derive(Clone)]
pub struct Command {
    pub id: SharedString,
    pub title: SharedString,
    /// GPUI keystroke specs.
    pub keys: Vec<SharedString>,
    /// Key context the bindings apply in (`OrbitEditor`, `FileTree`…).
    pub context: Option<SharedString>,
    pub run: Option<Run>,
    /// Can it run now? Hidden from the palette, and its keys pass through, when not.
    pub when: Option<Predicate>,
    /// Extra condition for the shortcut only (Ctrl+W leaves terminals alone).
    pub key_when: Option<Predicate>,
}

impl Command {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self { id: id.into(), title: title.into(), keys: vec![], context: None, run: None, when: None, key_when: None }
    }

    pub fn keys<S: Into<SharedString>>(mut self, keys: impl IntoIterator<Item = S>) -> Self {
        self.keys = keys.into_iter().map(Into::into).collect();
        self
    }

    pub fn context(mut self, context: impl Into<SharedString>) -> Self {
        self.context = Some(context.into());
        self
    }

    pub fn run(mut self, f: impl Fn(&mut OrbitApp, &mut Window, &mut Context<OrbitApp>) + 'static) -> Self {
        self.run = Some(Run::App(Rc::new(f)));
        self
    }

    pub fn action(mut self, action: impl Action) -> Self {
        self.run = Some(Run::Action(Rc::new(action)));
        self
    }

    pub fn when(mut self, f: impl Fn(&OrbitApp, &App) -> bool + 'static) -> Self {
        self.when = Some(Rc::new(f));
        self
    }

    pub fn key_when(mut self, f: impl Fn(&OrbitApp, &App) -> bool + 'static) -> Self {
        self.key_when = Some(Rc::new(f));
        self
    }

    pub fn enabled(&self, app: &OrbitApp, cx: &App) -> bool {
        self.when.as_ref().is_none_or(|w| w(app, cx))
    }

    /// The first shortcut as shown to people: `Ctrl+Shift+T`.
    pub fn shortcut(&self) -> Option<String> {
        self.keys.first().map(|k| display_keys(k))
    }
}

#[derive(Default)]
pub struct Registry {
    commands: Vec<Command>,
}

impl Global for Registry {}

/// Adds (or replaces) a command and binds its keys.
pub fn register(cx: &mut App, command: Command) {
    let predicate = command.context.as_ref().map(|c| KeyBindingContextPredicate::parse(c).expect("valid key context"));
    let predicate = predicate.map(Rc::new);
    let bindings: Vec<KeyBinding> = command
        .keys
        .iter()
        .filter_map(|keys| {
            let action: Box<dyn gpui::Action> = match &command.run {
                Some(Run::Action(a)) => a.boxed_clone(),
                _ => Box::new(RunCommand { id: command.id.clone(), from_key: true }),
            };
            KeyBinding::load(keys, action, predicate.clone(), false, None, cx)
                .map_err(|e| eprintln!("command {}: bad keys {keys:?}: {e:?}", command.id))
                .ok()
        })
        .collect();
    cx.bind_keys(bindings);
    let registry = cx.default_global::<Registry>();
    registry.commands.retain(|c| c.id != command.id);
    registry.commands.push(command);
}

pub fn get(cx: &App, id: &str) -> Option<Command> {
    cx.try_global::<Registry>()?.commands.iter().find(|c| c.id == id).cloned()
}

/// Every command, by title.
pub fn all(cx: &App) -> Vec<Command> {
    let mut list = cx.try_global::<Registry>().map(|r| r.commands.clone()).unwrap_or_default();
    list.sort_by(|a, b| a.title.cmp(&b.title));
    list
}

/// Runs a command now (the palette, a menu, a script). False when it can't.
pub fn run(id: &str, app: &mut OrbitApp, window: &mut Window, cx: &mut Context<OrbitApp>) -> bool {
    let Some(command) = get(cx, id) else { return false };
    if !command.enabled(app, cx) {
        return false;
    }
    match command.run {
        Some(Run::App(f)) => f(app, window, cx),
        Some(Run::Action(a)) => window.dispatch_action(a.boxed_clone(), cx),
        None => return false,
    }
    true
}

/// The root view's handler for `RunCommand`. A shortcut that doesn't apply
/// now propagates, so the focused view gets the key (a shell's Ctrl+W).
pub fn on_run_command(app: &mut OrbitApp, action: &RunCommand, window: &mut Window, cx: &mut Context<OrbitApp>) {
    // An open dialog, menu or palette owns the keyboard.
    if app.dialog.is_some() || app.menu.is_some() || app.palette.is_some() {
        return;
    }
    let Some(command) = get(cx, &action.id) else { return cx.propagate() };
    let applies = command.enabled(app, cx) && (!action.from_key || command.key_when.as_ref().is_none_or(|w| w(app, cx)));
    if !applies {
        return cx.propagate();
    }
    run(&action.id, app, window, cx);
}

/// `ctrl-shift-t` → `Ctrl+Shift+T`, `alt-left` → `Alt+←`, `shift shift` → `Shift Shift`.
pub fn display_keys(spec: &str) -> String {
    spec.split_whitespace()
        .map(|stroke| {
            stroke
                .split('-')
                .map(|part| match part {
                    "ctrl" => "Ctrl".to_string(),
                    "shift" => "Shift".to_string(),
                    "alt" => "Alt".to_string(),
                    "cmd" | "super" | "secondary" => "Super".to_string(),
                    "left" => "←".to_string(),
                    "right" => "→".to_string(),
                    "up" => "↑".to_string(),
                    "down" => "↓".to_string(),
                    "enter" => "Enter".to_string(),
                    "escape" => "Esc".to_string(),
                    "tab" => "Tab".to_string(),
                    "space" => "Space".to_string(),
                    other if other.len() == 1 => other.to_uppercase(),
                    // f12, pageup…
                    other => {
                        let mut s = other.to_string();
                        s[..1].make_ascii_uppercase();
                        s
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fuzzy-enough palette match: every word of the query appears in the title.
pub fn matches(title: &str, query: &str) -> bool {
    let title = title.to_lowercase();
    query.split_whitespace().all(|w| title.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_keys() {
        assert_eq!(display_keys("ctrl-shift-t"), "Ctrl+Shift+T");
        assert_eq!(display_keys("alt-left"), "Alt+←");
        assert_eq!(display_keys("f12"), "F12");
        assert_eq!(display_keys("ctrl-f9"), "Ctrl+F9");
        assert_eq!(display_keys("shift shift"), "Shift Shift");
    }

    #[test]
    fn matches_words() {
        assert!(matches("Close Saved Files", "clo sav"));
        assert!(matches("Close Saved Files", ""));
        assert!(!matches("Close Saved Files", "open"));
    }
}
