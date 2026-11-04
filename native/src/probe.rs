//! Named element bounds for the self-test, which clicks and drags through
//! GPUI's own event dispatch: `probe("tab:2")` inside a `relative()` element
//! records where it was painted. Off unless a script runs.
use gpui::{canvas, div, prelude::*, AnyElement, Bounds, Pixels};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static BOUNDS: RefCell<HashMap<String, Bounds<Pixels>>> = RefCell::new(HashMap::new());
}

pub fn probe(name: impl Into<String>) -> AnyElement {
    if !ENABLED.load(Ordering::Relaxed) {
        return div().into_any_element();
    }
    let name = name.into();
    canvas(move |bounds, _, _| BOUNDS.with(|b| b.borrow_mut().insert(name, bounds)), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
}

pub fn bounds(name: &str) -> Option<Bounds<Pixels>> {
    BOUNDS.with(|b| b.borrow().get(name).copied())
}

/// Forgets everything, so the next frame's bounds are the only ones known.
pub fn clear() {
    BOUNDS.with(|b| b.borrow_mut().clear());
}
