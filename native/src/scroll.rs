//! Nested scrolling. GPUI hands a wheel event to every scroll container under
//! the mouse, so a list inside a scrolling page moves together with the page.
//! `contain` wraps an inner scroller: while it has something to scroll, it
//! keeps the wheel and the page stays put.
use gpui::{div, prelude::*, Div, ScrollHandle};

/// Wraps `inner`, which must `track_scroll(handle)`. The wrapper's listener
/// runs after the inner scroller's own (children dispatch first), so the inner
/// list has already moved; this only stops the event reaching the page.
pub fn contain(handle: &ScrollHandle, inner: impl IntoElement) -> Div {
    let handle = handle.clone();
    div()
        .on_scroll(move |_, _, cx| {
            if handle.max_offset().y > gpui::px(0.) {
                cx.stop_propagation();
            }
        })
        .child(inner)
}
