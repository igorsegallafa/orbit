//! Tab scopes: every tab belongs to a workspace or to the global scope, and
//! the strip shows the active scope's tabs. Nav pages belong to none.
use std::collections::HashMap;
use std::path::Path;

/// A workspace's name, or None for the global scope (plain folders, files
/// outside any workspace).
pub type Scope = Option<String>;

/// The titlebar's name for the global scope.
pub const GLOBAL_LABEL: &str = "No workspace";

/// The workspace holding `path`, from `(name, dir)` pairs.
pub fn of_path<'a>(workspaces: impl IntoIterator<Item = (&'a str, &'a Path)>, path: &Path) -> Scope {
    workspaces.into_iter().find(|(_, dir)| path.starts_with(dir)).map(|(name, _)| name.to_string())
}

/// The workspace a page key names (`page:<kind>:<workspace>`), if any.
pub fn page_workspace(key: &str) -> Option<&str> {
    key.strip_prefix("page:")?.split_once(':').map(|(_, rest)| rest)
}

/// Each scope's last active tab, by tab key.
#[derive(Default)]
pub struct Memory {
    last: HashMap<Scope, String>,
}

impl Memory {
    pub fn record(&mut self, scope: Scope, key: String) {
        self.last.insert(scope, key);
    }

    pub fn last(&self, scope: &Scope) -> Option<&str> {
        self.last.get(scope).map(String::as_str)
    }

    pub fn clear(&mut self) {
        self.last.clear();
    }

    /// The tab to show when `scope` becomes active: its last one while it is
    /// still on the strip (`strip`: the scope's keys, in order), else the first.
    pub fn pick(&self, scope: &Scope, strip: &[String]) -> Option<usize> {
        self.last(scope).and_then(|k| strip.iter().position(|s| s == k)).or((!strip.is_empty()).then_some(0))
    }
}

/// The switcher's filter: `query`'s characters in order, any case.
pub fn matches(query: &str, name: &str) -> bool {
    let mut rest = name.chars().flat_map(char::to_lowercase);
    query.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).all(|q| rest.any(|c| c == q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_paths_and_pages() {
        let ws = [("a", Path::new("/ws/a")), ("ab", Path::new("/ws/ab"))];
        assert_eq!(of_path(ws, Path::new("/ws/a/repo/x.rs")).as_deref(), Some("a"));
        // A sibling folder with a longer name isn't inside `a`.
        assert_eq!(of_path(ws, Path::new("/ws/ab/repo/x.rs")).as_deref(), Some("ab"));
        assert_eq!(of_path(ws, Path::new("/home/u/notes.md")), None);
        assert_eq!(page_workspace("page:ws:feat-x"), Some("feat-x"));
        assert_eq!(page_workspace("page:ralph:feat-x"), Some("feat-x"));
        assert_eq!(page_workspace("page:dashboard"), None);
        assert_eq!(page_workspace("term:12"), None);
    }

    #[test]
    fn switching_restores_the_last_tab() {
        let mut m = Memory::default();
        let a: Scope = Some("a".into());
        let strip = |keys: &[&str]| keys.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(m.pick(&a, &strip(&[])), None);
        assert_eq!(m.pick(&a, &strip(&["ed:/x", "term:1"])), Some(0));
        m.record(a.clone(), "term:1".into());
        m.record(None, "ed:/home/n".into());
        assert_eq!(m.pick(&a, &strip(&["ed:/x", "term:1"])), Some(1));
        assert_eq!(m.pick(&None, &strip(&["term:9", "ed:/home/n"])), Some(1));
        // The last tab was closed: the scope's first.
        assert_eq!(m.pick(&a, &strip(&["ed:/x"])), Some(0));
    }

    #[test]
    fn filters_by_subsequence() {
        assert!(matches("", "anything"));
        assert!(matches("ais", "add-items-skills"));
        assert!(matches("ADD", "add-items-skills"));
        assert!(!matches("xyz", "add-items-skills"));
        assert!(matches("no ws", "No workspace"));
    }
}
