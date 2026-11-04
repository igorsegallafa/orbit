//! Motion from App.css: its durations and easings, and the helpers that play
//! them (shimmering skeletons, entry animations, pulses, tweens). Repeating
//! animations only run while their element is on screen, and one-shots and
//! tweens stop requesting frames once they settle, so an idle app draws nothing.
use crate::theme::{self, c};
use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, relative, Animation, AnimationExt, AnyElement, App,
    BoxShadow, Bounds, DefiniteLength, Div, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, LayoutId, MouseMoveEvent, Pixels, Rgba, Window,
};
use std::{cell::Cell, rc::Rc, time::Duration};

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `--transition` (hover backgrounds, borders, chevrons).
pub const TRANSITION: Duration = ms(150);
/// `.sidebar-anim` / `.dock-anim`: hiding and showing a side pane.
pub const PANE: Duration = ms(220);
/// `page-in`.
pub const PAGE_IN: Duration = ms(180);
pub const CARD_IN: Duration = ms(200);
/// `.modal-overlay` (fade-in) and `.modal` (modal-in).
pub const OVERLAY_IN: Duration = ms(140);
pub const MODAL_IN: Duration = ms(160);
/// `.context-menu` / `.select-list` and `.search-modal` (modal-in).
pub const MENU_IN: Duration = ms(100);
pub const SEARCH_IN: Duration = ms(140);
pub const TOAST_IN: Duration = ms(180);
/// `.spinner`, and `.status-work` / `.spin`.
pub const SPIN: Duration = ms(700);
pub const STATUS_SPIN: Duration = ms(800);
pub const STATUS_PULSE: Duration = ms(1200);
/// `.cr-row-pending`.
pub const PENDING_PULSE: Duration = ms(1400);
pub const SHIMMER: Duration = ms(1300);
pub const RV_PULSE: Duration = ms(1600);
pub const RV_BLINK: Duration = ms(1400);
pub const PANEL_PULSE: Duration = ms(1000);
/// `.plan-progress-fill` and `.rv-progress > div`.
pub const PLAN_FILL: Duration = ms(300);
pub const RV_FILL: Duration = ms(250);
/// `.rv-seg` background.
pub const SEG: Duration = ms(300);

/// CSS `cubic-bezier(x1, y1, x2, y2)`.
pub fn bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 + Clone + 'static {
    let (cx, bx) = (3. * x1, 3. * (x2 - x1) - 3. * x1);
    let ax = 1. - cx - bx;
    let (cy, by) = (3. * y1, 3. * (y2 - y1) - 3. * y1);
    let ay = 1. - cy - by;
    move |t: f32| {
        let t = t.clamp(0., 1.);
        let sx = |u: f32| ((ax * u + bx) * u + cx) * u;
        let dx = |u: f32| (3. * ax * u + 2. * bx) * u + cx;
        // Newton, then bisection when the slope is flat.
        let mut u = t;
        for _ in 0..8 {
            let err = sx(u) - t;
            let d = dx(u);
            if err.abs() < 1e-5 || d.abs() < 1e-6 {
                break;
            }
            u -= err / d;
        }
        if (sx(u) - t).abs() > 1e-4 || !(0. ..=1.).contains(&u) {
            let (mut lo, mut hi) = (0f32, 1f32);
            u = t;
            for _ in 0..30 {
                if sx(u) < t { lo = u } else { hi = u }
                u = (lo + hi) / 2.;
            }
        }
        ((ay * u + by) * u + cy) * u
    }
}

/// The `--transition` curve, `cubic-bezier(0.25, 0.25, 0, 1)`.
pub fn standard() -> impl Fn(f32) -> f32 + Clone {
    bezier(0.25, 0.25, 0., 1.)
}
pub fn ease() -> impl Fn(f32) -> f32 + Clone {
    bezier(0.25, 0.1, 0.25, 1.)
}
pub fn ease_in_out() -> impl Fn(f32) -> f32 + Clone {
    bezier(0.42, 0., 0.58, 1.)
}
pub fn ease_out() -> impl Fn(f32) -> f32 + Clone {
    bezier(0., 0., 0.58, 1.)
}
/// `toast-in`'s `cubic-bezier(0.2, 0.8, 0.2, 1)`.
pub fn toast_curve() -> impl Fn(f32) -> f32 + Clone {
    bezier(0.2, 0.8, 0.2, 1.)
}

/// A `0%, 100% { a } 50% { b }` keyframe pair eased per half: 0 → 1 → 0.
pub fn there_and_back(t: f32, easing: &impl Fn(f32) -> f32) -> f32 {
    if t < 0.5 { easing(t * 2.) } else { easing((1. - t) * 2.) }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: lerp(a.r, b.r, t), g: lerp(a.g, b.g, t), b: lerp(a.b, b.b, t), a: lerp(a.a, b.a, t) }
}

// ---------- skeletons ----------

/// `.skeleton` (`Skeleton.tsx`): a bar with the shimmer sweeping across it.
#[derive(IntoElement)]
pub struct Skeleton {
    w: DefiniteLength,
    h: f32,
    radius: f32,
}

/// `<Skeleton w h />` (6 px corners, as React's default).
pub fn skeleton(w: impl Into<DefiniteLength>, h: f32) -> Skeleton {
    Skeleton { w: w.into(), h, radius: 6. }
}

impl Skeleton {
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    /// `rounded={999}`.
    pub fn pill(self) -> Self {
        let h = self.h;
        self.radius(h / 2.)
    }
}

impl RenderOnce for Skeleton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let r = self.radius.min(self.h / 2.);
        let base = div().w(self.w).h(px(self.h)).flex().flex_none().rounded(px(r)).bg(c(theme::PANEL_RAISED));
        // Synced: every skeleton sweeps in phase, as they mount together in React.
        let anim = Animation::new(SHIMMER).repeat_synced().with_easing(ease_in_out());
        base.with_animation("skeleton", anim, move |d, t| d.children(shimmer(t, r)))
    }
}

/// `skeleton-shimmer`: `linear-gradient(raised 40%, hover 50%, raised 60%)` at
/// `background-size: 200%`, moved from `120%` to `-80%`. In the bar's width,
/// the highlight peaks at `1 - pos` and fades over 0.2 on each side, so it
/// sweeps from -0.2 to 1.8 (the last 30% of a cycle shows plain bars). The
/// bar is cut into gradient segments so its rounded ends stay clipped; the
/// CSS's 10° tilt (100deg) is dropped.
fn shimmer(t: f32, r: f32) -> Vec<Div> {
    const HALF: f32 = 0.2;
    let peak = 1. - (1.2 - 2. * t);
    let (base, hi) = (c(theme::PANEL_RAISED), c(theme::PANEL_HOVER));
    let at = |x: f32| mix(base, hi, (1. - (x - peak).abs() / HALF).max(0.));
    let mut cuts = vec![0f32];
    cuts.extend([peak - HALF, peak, peak + HALF].into_iter().filter(|x| *x > 0. && *x < 1.));
    cuts.push(1.);
    if cuts.len() == 2 {
        return Vec::new(); // the highlight is off the bar
    }
    let n = cuts.len() - 1;
    (0..n)
        .map(|i| {
            let (a, b) = (cuts[i], cuts[i + 1]);
            let (ca, cb) = (at(a), at(b));
            let seg = div().h_full().flex_none().w(relative(b - a));
            let seg = if ca == cb { seg.bg(ca) } else { seg.bg(linear_gradient(90., linear_color_stop(ca, 0.), linear_color_stop(cb, 1.))) };
            let seg = if i == 0 { seg.rounded_l(px(r)) } else { seg };
            if i == n - 1 { seg.rounded_r(px(r)) } else { seg }
        })
        .collect()
}

/// `SkeletonTable`: a faded header row and `rows` rows of `cols` bars.
pub fn skeleton_table(rows: usize, cols: usize) -> Div {
    let row = |head: bool| {
        div()
            .flex()
            .items_center()
            .gap(px(24.))
            .children((0..cols).map(move |c| {
                let w = if head { 0.6 } else if c == 0 { 0.8 } else { 0.55 };
                div().flex_1().min_w_0().child(skeleton(relative(w), 10.))
            }))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(20.))
        .py(px(18.))
        .child(row(true).opacity(0.5).pb(px(6.)).border_b_1().border_color(c(theme::BORDER)))
        .children((0..rows).map(move |_| row(false)))
}

/// `SkeletonCards`' card: `.skeleton-card` with a title and a line.
pub fn skeleton_card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(16.))
        .border_1()
        .border_color(c(theme::BORDER))
        .rounded(px(theme::size::RADIUS))
        .bg(c(theme::PANEL))
        .child(skeleton(relative(0.55), 14.))
        .child(skeleton(relative(0.75), 10.))
}

/// `.modal-row` of skeleton bars: a short one, then a long one.
pub fn skeleton_line(first: f32, second: f32, h: f32) -> Div {
    div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(6.)).child(skeleton(px(first), h)).child(skeleton(relative(second), h))
}

// ---------- entry animations ----------

/// Fades and rises `el` by `dy` px as it mounts (`page-in`, `card-in`,
/// `toast-in`, `modal-in` without its 0.97 scale: GPUI can't scale a div's
/// subtree, so the dialog only fades and rises), after `delay`.
pub fn rise_in<E>(el: E, id: impl Into<ElementId>, dur: Duration, dy: f32, delay: Duration, easing: impl Fn(f32) -> f32 + 'static) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    let el = el.relative();
    let main = Animation::new(dur).with_easing(easing);
    let animate = move |d: E, t: f32| d.opacity(t.clamp(0., 1.)).top(px(dy * (1. - t)));
    if delay.is_zero() {
        el.with_animation(id, main, animate).into_any_element()
    } else {
        // `animation-fill-mode: backwards`: held at the start during the delay.
        el.with_animations(id, vec![Animation::new(delay), main], move |d, ix, t| animate(d, if ix == 0 { 0. } else { t }))
            .into_any_element()
    }
}

/// `fade-in`: opacity only.
pub fn fade_in<E>(el: E, id: impl Into<ElementId>, dur: Duration, easing: impl Fn(f32) -> f32 + 'static) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    el.with_animation(id, Animation::new(dur).with_easing(easing), |d, t| d.opacity(t.clamp(0., 1.))).into_any_element()
}

/// `.page`: `page-in 0.18s`.
pub fn page_in<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>) -> AnyElement {
    rise_in(el, id, PAGE_IN, 6., Duration::ZERO, standard())
}

/// `.card` / `.dash-card` / `.cr-row`: `card-in 0.2s`, the `ix`-th one delayed by `stagger`.
pub fn card_in<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>, delay: Duration) -> AnyElement {
    rise_in(el, id, CARD_IN, 5., delay, standard())
}

/// `.modal`: `modal-in 0.16s`.
pub fn modal_in<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>) -> AnyElement {
    rise_in(el, id, MODAL_IN, 8., Duration::ZERO, standard())
}

/// `.modal-overlay`: `fade-in 0.14s ease-out`.
pub fn overlay_in<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>) -> AnyElement {
    fade_in(el, id, OVERLAY_IN, ease_out())
}

/// Popovers in flow: `modal-in` at `dur` (`SEARCH_IN`).
pub fn popover_in<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>, dur: Duration) -> AnyElement {
    rise_in(el, id, dur, 8., Duration::ZERO, standard())
}

/// An absolutely placed popover at `top`: `modal-in` at `dur`, rising from `top + 8`.
pub fn drop_in<E>(el: E, id: impl Into<ElementId>, top: Pixels, dur: Duration) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    el.with_animation(id, Animation::new(dur).with_easing(standard()), move |d, t| d.top(top + px(8. * (1. - t))).opacity(t.clamp(0., 1.)))
        .into_any_element()
}

// ---------- repeating ----------

/// `status-pulse`: opacity 1 → .35 and scale 1 → .8 at the half, ease-in-out.
/// The dot (`size` px) shrinks about its centre inside a box of its full size.
pub fn status_pulse(id: impl Into<ElementId>, size: f32, color: Rgba, period: Duration) -> AnyElement {
    let ease = ease_in_out();
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .with_animation(id, Animation::new(period).repeat_synced(), move |d, t| {
            let k = there_and_back(t, &ease);
            let s = size * lerp(1., 0.8, k);
            d.opacity(lerp(1., 0.35, k)).child(div().rounded_full().bg(color).size(px(s)))
        })
        .into_any_element()
}

/// `rv-blink` / `panel-pulse`: opacity 1 → `low` at the half.
pub fn blink<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>, period: Duration, low: f32) -> AnyElement {
    let ease = ease_in_out();
    el.with_animation(id, Animation::new(period).repeat_synced(), move |d, t| d.opacity(lerp(1., low, there_and_back(t, &ease))))
        .into_any_element()
}

/// `rv-pulse`: a `size` px dot whose `ring` px halo (`accent-soft`) widens to
/// 7 px and fades out at the half, over 1.6 s.
pub fn rv_pulse(id: impl Into<ElementId>, size: f32, ring: f32, color: Rgba, halo: Rgba) -> AnyElement {
    let ease = ease_in_out();
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(color)
        .with_animation(id, Animation::new(RV_PULSE).repeat_synced(), move |d, t| {
            let k = there_and_back(t, &ease);
            let mut col = halo;
            col.a *= 1. - k;
            d.shadow(vec![BoxShadow {
                color: col.into(),
                offset: gpui::point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(lerp(ring, 7., k)),
                inset: false,
            }])
        })
        .into_any_element()
}

// ---------- tweens ----------

/// Animates a value toward `target` whenever it changes (a CSS `transition`),
/// or toward 1 while hovered and 0 otherwise. Mounts at its target, so only
/// changes animate; it requests frames only while moving.
pub struct Tween<E> {
    id: ElementId,
    element: Option<E>,
    target: Option<f32>,
    from: Option<f32>,
    jump: bool,
    duration: Duration,
    easing: Rc<dyn Fn(f32) -> f32>,
    animator: Option<Box<dyn FnOnce(E, f32) -> E>>,
}

struct TweenState {
    from: f32,
    to: f32,
    start: std::time::Instant,
    hovered: Rc<Cell<bool>>,
}

pub trait TweenExt: Sized {
    /// `transition: <prop> <duration> <easing>` on a value.
    fn tween(self, id: impl Into<ElementId>, target: f32, duration: Duration, easing: impl Fn(f32) -> f32 + 'static, animator: impl FnOnce(Self, f32) -> Self + 'static) -> Tween<Self> {
        Tween { id: id.into(), element: Some(self), target: Some(target), from: None, jump: false, duration, easing: Rc::new(easing), animator: Some(Box::new(animator)) }
    }
    /// A `:hover` transition: the value goes 0 → 1 while the pointer is over the element.
    fn hover_tween(self, id: impl Into<ElementId>, duration: Duration, easing: impl Fn(f32) -> f32 + 'static, animator: impl FnOnce(Self, f32) -> Self + 'static) -> Tween<Self> {
        Tween { id: id.into(), element: Some(self), target: None, from: None, jump: false, duration, easing: Rc::new(easing), animator: Some(Box::new(animator)) }
    }
}

impl<E: IntoElement + 'static> TweenExt for E {}

impl<E> Tween<E> {
    /// Starts at `from` on mount instead of at the target.
    pub fn from(mut self, from: f32) -> Self {
        self.from = Some(from);
        self
    }
    /// Follows the target without animating (a resizer drag).
    pub fn jump(mut self, jump: bool) -> Self {
        self.jump = jump;
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for Tween<E> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<E: IntoElement + 'static> Element for Tween<E> {
    type RequestLayoutState = (AnyElement, Option<Rc<Cell<bool>>>);
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState) {
        window.with_element_state(id.unwrap(), |state: Option<TweenState>, window| {
            let now = std::time::Instant::now();
            let mut state = state.unwrap_or_else(|| {
                let to = self.target.unwrap_or(0.);
                TweenState { from: self.from.unwrap_or(to), to, start: now, hovered: Rc::default() }
            });
            let target = self.target.unwrap_or(if state.hovered.get() { 1. } else { 0. });
            let dur = self.duration.as_secs_f32().max(1e-3);
            let progress = |s: &TweenState| (now - s.start).as_secs_f32() / dur;
            let value_of = |s: &TweenState, easing: &dyn Fn(f32) -> f32| {
                let p = progress(s);
                if p >= 1. { s.to } else { lerp(s.from, s.to, easing(p)) }
            };
            if self.jump {
                state.from = target;
                state.to = target;
            } else if target != state.to {
                // Retarget from wherever it is now.
                state.from = value_of(&state, &*self.easing);
                state.to = target;
                state.start = now;
            }
            let done = cx.reduce_motion() || progress(&state) >= 1.;
            let value = if done { state.to } else { value_of(&state, &*self.easing) };
            if !done {
                window.request_animation_frame();
            }
            let animator = self.animator.take().expect("rendered once");
            let mut el = animator(self.element.take().expect("rendered once"), value).into_any_element();
            let hovered = self.target.is_none().then(|| state.hovered.clone());
            ((el.request_layout(window, cx), (el, hovered)), state)
        })
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut Self::RequestLayoutState, window: &mut Window, cx: &mut App) -> Option<Hitbox> {
        let hitbox = state.1.is_some().then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
        state.0.prepaint(window, cx);
        hitbox
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, state: &mut Self::RequestLayoutState, hitbox: &mut Option<Hitbox>, window: &mut Window, cx: &mut App) {
        state.0.paint(window, cx);
        let (Some(hovered), Some(hitbox)) = (state.1.clone(), hitbox.clone()) else { return };
        let view = window.current_view();
        let now = hitbox.is_hovered(window);
        if now != hovered.get() {
            hovered.set(now);
            cx.notify(view);
        }
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            let now = hitbox.is_hovered(window);
            if phase == DispatchPhase::Capture && now != hovered.get() {
                hovered.set(now);
                cx.notify(view);
            }
        });
    }
}

/// An animated width as a fraction of the parent (`.plan-progress-fill`, `.rv-progress > div`).
pub fn fill(id: impl Into<ElementId>, fraction: f32, dur: Duration, easing: impl Fn(f32) -> f32 + 'static, bar: Div) -> impl IntoElement {
    bar.tween(id, fraction.clamp(0., 1.), dur, easing, |d, v| d.w(relative(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_matches_css() {
        let e = ease_in_out();
        assert!((e(0.) - 0.).abs() < 1e-4 && (e(1.) - 1.).abs() < 1e-4);
        assert!((e(0.5) - 0.5).abs() < 1e-3);
        // cubic-bezier(0.25, 0.25, 0, 1) is fast early: past 0.8 at mid-time.
        assert!(standard()(0.5) > 0.8);
    }

    #[test]
    fn shimmer_sweeps_and_rests() {
        assert_eq!(shimmer(0.3, 6.).len(), 4); // peak at 0.4: plain, rise, fall, plain
        assert!(shimmer(1., 6.).is_empty()); // peak at 1.8: off the bar
    }
}
