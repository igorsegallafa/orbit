//! Back/forward through where you've been (mouse side buttons, Alt+←/→, the
//! titlebar arrows), like VS Code and `src/lib/navHistory.ts`: tab switches,
//! plus jumps between files, with the cursor each file had.

#[derive(Clone, Debug, PartialEq)]
pub struct NavEntry<T> {
    /// Tab key: two entries with the same key are one place.
    pub key: String,
    pub target: T,
    /// 1-based cursor to restore when coming back.
    pub position: Option<(u32, u32)>,
}

const LIMIT: usize = 60;

pub struct NavHistory<T> {
    back: Vec<NavEntry<T>>,
    forward: Vec<NavEntry<T>>,
    pub current: Option<NavEntry<T>>,
}

impl<T> Default for NavHistory<T> {
    fn default() -> Self {
        Self { back: vec![], forward: vec![], current: None }
    }
}

impl<T: Clone> NavHistory<T> {
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    fn push(&mut self, entry: NavEntry<T>) {
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        if self.back.len() > LIMIT {
            self.back.remove(0);
        }
        self.current = Some(entry);
        self.forward.clear();
    }

    /// Something else is shown now (a tab switch).
    pub fn visit(&mut self, key: String, target: T) {
        if self.current.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        self.push(NavEntry { key, target, position: None });
    }

    /// The shown file's cursor, remembered for when you come back.
    pub fn set_position(&mut self, key: &str, position: (u32, u32)) {
        if let Some(cur) = self.current.as_mut().filter(|c| c.key == key) {
            cur.position = Some(position);
        }
    }

    /// Steps back (or forward), skipping places `usable` rejects.
    pub fn step(&mut self, back: bool, usable: impl Fn(&NavEntry<T>) -> bool) -> Option<NavEntry<T>> {
        let (from, to) = if back { (&mut self.back, &mut self.forward) } else { (&mut self.forward, &mut self.back) };
        while let Some(e) = from.pop() {
            // Skipped places can leave the shown one next to itself.
            if !usable(&e) || self.current.as_ref().is_some_and(|c| c.key == e.key) {
                continue;
            }
            if let Some(cur) = self.current.take() {
                to.push(cur);
            }
            self.current = Some(e.clone());
            return Some(e);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_and_steps() {
        let mut h = NavHistory::<u8>::default();
        h.visit("a".into(), 1);
        h.visit("a".into(), 1);
        h.visit("b".into(), 2);
        h.set_position("b", (10, 2));
        h.visit("c".into(), 3);
        assert_eq!(h.depth(), (2, 0));
        let e = h.step(true, |_| true).unwrap();
        assert_eq!((e.key.as_str(), e.position), ("b", Some((10, 2))));
        assert!(h.can_go_forward());
        assert_eq!(h.step(false, |_| true).unwrap().key, "c");
        // A new visit after going back drops the forward list.
        h.step(true, |_| true);
        h.visit("d".into(), 4);
        assert!(!h.can_go_forward());
    }

    #[test]
    fn skips_unusable() {
        let mut h = NavHistory::<u8>::default();
        for (k, t) in [("a", 1), ("b", 2), ("c", 3)] {
            h.visit(k.into(), t);
        }
        assert_eq!(h.step(true, |e| e.key != "b").unwrap().key, "a");
        assert!(h.step(true, |_| true).is_none());
        // The skipped entry is gone, not moved to the forward list.
        assert_eq!(h.depth(), (0, 1));
    }

    #[test]
    fn never_steps_to_the_shown_place() {
        let mut h = NavHistory::<u8>::default();
        for (k, t) in [("a", 1), ("c", 3), ("t", 0), ("c", 3)] {
            h.visit(k.into(), t);
        }
        // "t" is unusable, and the "c" before it is where we are.
        assert_eq!(h.step(true, |e| e.key != "t").unwrap().key, "a");
    }

    #[test]
    fn bounded() {
        let mut h = NavHistory::<usize>::default();
        for i in 0..100 {
            h.visit(i.to_string(), i);
        }
        assert_eq!(h.depth().0, LIMIT);
    }
}
