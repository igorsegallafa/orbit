//! The overlay scrollbar of the custom-painted views (terminal, log views):
//! gpui-component's `Scrollbar` over any `ScrollbarHandle`, styled as App.css's
//! `::-webkit-scrollbar` (a thin rounded thumb, wider and brighter on hover)
//! and shown on scroll or hover. A click on the track pages, where
//! gpui-component's jumps there.
use crate::probe;
use crate::theme::{self, hsla};
use gpui::{canvas, div, point, prelude::*, px, Bounds, ElementId, Pixels, Point};
use gpui_component::scroll::{Scrollbar, ScrollbarHandle, ScrollbarMode, ScrollbarThumbStyle};
use std::sync::atomic::Ordering;

const TRACK: f32 = 14.;
const THUMB: f32 = 9.;
const THUMB_HOVER: f32 = 11.;
const INSET: f32 = 2.;
const MIN_THUMB: f32 = 20.;

/// The vertical scrollbar over `handle`'s viewport, for a `relative()` parent.
/// `name` is its id and, in self-tests, the probes `<name>-thumb` and `<name>-track`.
pub fn vertical<H: ScrollbarHandle + Clone>(name: &str, handle: &H) -> impl IntoElement {
    let rest = hsla(theme::BORDER_STRONG);
    let hover = hsla(theme::mix(theme::hex(theme::TEXT_FAINT), theme::hex(theme::BORDER_STRONG), 0.5));
    let active = hsla(theme::TEXT_FAINT);
    let thumb = |s: ScrollbarThumbStyle, w: f32, bg: gpui::Hsla| {
        s.width(px(w)).inset(px(INSET)).radius(px(w / 2.)).min_length(px(MIN_THUMB)).bg(bg)
    };
    let bar = Scrollbar::vertical(handle)
        .id(ElementId::Name(format!("{name}-scrollbar").into()))
        .mode(ScrollbarMode::Hover)
        .styles(|s| {
            s.track(|t| t.width(px(TRACK)).bg(gpui::transparent_black()))
                .track_hover(|t| t.bg(gpui::transparent_black()))
                .track_active(|t| t.bg(gpui::transparent_black()))
                .thumb(|t| thumb(t, THUMB, rest))
                .thumb_hover(|t| thumb(t, THUMB_HOVER, hover))
                .thumb_active(|t| thumb(t, THUMB_HOVER, active))
        });
    div().absolute().inset_0().child(bar).children(probe::ENABLED.load(Ordering::Relaxed).then(|| {
        let (handle, name) = (handle.clone(), name.to_string());
        canvas(
            move |_, _, _| {
                if let Some(t) = thumb_span(&handle) {
                    probe::record(format!("{name}-track"), t.track);
                    probe::record(format!("{name}-thumb"), Bounds::new(point(t.track.origin.x, t.start), gpui::size(px(TRACK), t.len)));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0()
    }))
}

struct Thumb {
    track: Bounds<Pixels>,
    start: Pixels,
    len: Pixels,
}

/// Where gpui-component draws the thumb (its `ThumbGeometry`), or `None` when
/// everything fits and the bar is hidden.
fn thumb_span(handle: &impl ScrollbarHandle) -> Option<Thumb> {
    let view = handle.viewport_bounds();
    let (height, content) = (view.size.height, handle.content_size().height);
    if height <= px(0.) || content <= height {
        return None;
    }
    let logical = (height * (height / content)).max(px(MIN_THUMB)).min(height);
    let inset = px(INSET).min(logical / 2.);
    let progress = (-handle.offset().y / (content - height)).clamp(0., 1.);
    Some(Thumb {
        track: Bounds::new(point(view.right() - px(TRACK), view.top()), gpui::size(px(TRACK), height)),
        start: view.top() + inset + (height - logical) * progress,
        len: logical - inset * 2.,
    })
}

/// A press on the track off the thumb scrolls a page towards it. Returns
/// whether it did, so the caller stops the press there (call it while capturing).
pub fn page(handle: &impl ScrollbarHandle, at: Point<Pixels>) -> bool {
    let Some(t) = thumb_span(handle) else { return false };
    if !t.track.contains(&at) || (t.start..=t.start + t.len).contains(&at.y) {
        return false;
    }
    let height = handle.viewport_bounds().size.height;
    let extent = handle.content_size().height - height;
    let offset = handle.offset();
    let y = if at.y < t.start { offset.y + height } else { offset.y - height };
    handle.set_offset(point(offset.x, y.clamp(-extent, px(0.))));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A 200 px viewport over `content` px, scrolled to `offset`.
    #[derive(Clone)]
    struct Fake {
        offset: Rc<Cell<Pixels>>,
        content: f32,
    }

    impl ScrollbarHandle for Fake {
        fn viewport_bounds(&self) -> Bounds<Pixels> {
            Bounds::new(point(px(0.), px(0.)), gpui::size(px(100.), px(200.)))
        }
        fn offset(&self) -> Point<Pixels> {
            point(px(0.), self.offset.get())
        }
        fn set_offset(&self, offset: Point<Pixels>) {
            self.offset.set(offset.y);
        }
        fn content_size(&self) -> gpui::Size<Pixels> {
            gpui::size(px(100.), px(self.content))
        }
    }

    #[test]
    fn track_clicks_page_towards_the_click() {
        let h = Fake { offset: Rc::new(Cell::new(px(-400.))), content: 1000. };
        // A 40 px thumb half way along 160 px of travel, less the 2 px insets.
        let t = thumb_span(&h).unwrap();
        assert_eq!((t.start, t.len), (px(82.), px(36.)));
        assert!(!page(&h, point(px(95.), px(100.))), "on the thumb");
        assert!(!page(&h, point(px(50.), px(10.))), "off the track");
        assert!(page(&h, point(px(95.), px(10.))));
        assert_eq!(h.offset.get(), px(-200.));
        for _ in 0..3 {
            assert!(page(&h, point(px(95.), px(190.))));
        }
        assert_eq!(h.offset.get(), px(-800.));
        assert!(!page(&h, point(px(95.), px(190.))), "the thumb reached the click");
    }

    #[test]
    fn no_thumb_when_everything_fits() {
        let h = Fake { offset: Rc::default(), content: 200. };
        assert!(thumb_span(&h).is_none());
        assert!(!page(&h, point(px(95.), px(10.))));
    }
}
