//! The catalog's shared pointer-admission tests, ramp-driven scalars, and
//! small paint helpers every interactive component reaches for.
//!
//! # Pointer and key admission
//!
//! [`presses`] and [`inside`]/[`inside_inclusive`] are this catalog's own copy
//! of the shared rule `docs/CODE_STANDARDS.md`'s Interaction Semantics
//! requires per catalog (`frust_widgets::authoring::presses`,
//! `frust_shadcn::hit::presses` are the siblings in the other two crates).
//! [`is_activation_key`] is the keyboard-activation counterpart every
//! `role="button"`-shaped control needs.
//!
//! `inside` and `inside_inclusive` differ only at the far edge (half-open vs.
//! closed): `button` (and its three siblings) and `theme_toggle` use the
//! half-open form, matching `kurbo::Rect::contains`; `switch` (and its five
//! form-control siblings) use the closed form. Both are preserved here
//! unmodified rather than collapsed into one, since changing either would
//! change a shipped component's boundary behavior.
//!
//! # Ramp-driven scalars
//!
//! [`SpringScalar`] and [`Lane`] both drive a value along a [`Ramp`] toward a
//! retargetable endpoint, and both are kept — rather than folded into one
//! type — because their contracts genuinely differ: `Lane` supports
//! [`Lane::retarget_with`] (swapping the ramp itself mid-life, for a control
//! whose entrance and exit are timed differently) and reports whether it is
//! still in flight from [`Lane::advance`]; `SpringScalar` supports
//! [`SpringScalar::jump_to`] (snapping to an arbitrary value, not just its
//! current target) and reports the value itself from
//! [`SpringScalar::advance`]. `button` (and its three siblings) use
//! `SpringScalar`; `switch` and its five form-control siblings use `Lane`.
//!
//! # Small paint helpers
//!
//! [`stroke_outline`] and [`draw_focus_ring`] paint a control's border and
//! focus ring; [`press_scale`] is the `whileTap`-style scale-toward-a-floor
//! formula `checkbox` and `radio` both apply to their press shrink;
//! [`lerp_color`]/[`keyframes_at`] are small numeric helpers a [`Lane`]-driven
//! color crossfade or literal keyframe timeline (the refusal shake) samples.

use frust::authoring::{
    Brush, Color, Key, KeyEvent, NamedKey, PaintScene, Point, PointerButton, PointerEvent, Rect,
    RoundedRect, Shape, Size,
};
use frust::{FrameTime, Tween};

use crate::motion::Ramp;
use crate::style;

/// Whether widget-local `pos` lies inside a `size`-shaped box, half-open on
/// both axes (matching `kurbo::Rect::contains`), so two adjacent boxes
/// sharing a seam never both claim the same pointer.
///
/// Used by `button` (and `expanding_arrow_button`/`expandable_control`/
/// `action_swap`) and `theme_toggle`.
pub(crate) fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Whether widget-local `pos` lies inside a `size`-shaped box, closed on both
/// axes — a pointer resting exactly on the far edge still counts.
///
/// Used by `switch` and its five form-control siblings (`checkbox`, `radio`,
/// `input`, `range_slider`, `wheel_picker`).
pub(crate) fn inside_inclusive(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x <= size.width && pos.y <= size.height
}

/// Whether `p` carries a button that may begin a press.
///
/// A press machine — pressed chrome, a pointer capture, an up-inside callback
/// — starts on the primary button alone (the left mouse button, or any
/// touch/pen contact, which every shell reports as `Primary`). A secondary
/// press is a context gesture and no component in this catalog activates on
/// one. Move arms never consult this: hover chrome and cursors are
/// position-driven.
pub(crate) fn presses(p: &PointerEvent) -> bool {
    p.button == PointerButton::Primary
}

/// Whether `key` activates a control: `Space` (arriving as typed text — there
/// is no `NamedKey::Space`) or `Enter`, the two keys the native `<button>`
/// upstream renders activates on.
pub(crate) fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
    }
}

/// One scalar following a target along a [`Ramp`] — the driver behind every
/// press scale, magnetic pull, expand width and thumb return in the `button`
/// family.
///
/// Retargeting mid-flight restarts the ramp from the **current value** toward
/// the new target: value-continuous, never velocity-continuous, the same
/// simplification [`Presence`](crate::motion::Presence) documents (nothing
/// exposes an in-flight ramp's instantaneous velocity to seed the next one
/// with). For a cursor-follow retargeted on every pointer sample this reads
/// as smooth lag rather than a series of restarts, because each restart
/// begins exactly where the last left off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpringScalar {
    value: f64,
    from: f64,
    target: f64,
    ramp: Ramp,
    started: Option<FrameTime>,
    animating: bool,
}

impl SpringScalar {
    /// A scalar resting at `value`, following `ramp` when retargeted.
    pub(crate) const fn new(value: f64, ramp: Ramp) -> Self {
        Self {
            value,
            from: value,
            target: value,
            ramp,
            started: None,
            animating: false,
        }
    }

    /// The value as of the last [`advance`](Self::advance).
    pub(crate) const fn value(&self) -> f64 {
        self.value
    }

    /// The value it is heading for.
    pub(crate) const fn target(&self) -> f64 {
        self.target
    }

    /// Whether a ramp is in flight, so the stepping widget owes another
    /// frame.
    pub(crate) const fn is_animating(&self) -> bool {
        self.animating
    }

    /// Aim at `target`, restarting the ramp from the current value. Returns
    /// whether anything changed.
    pub(crate) fn set_target(&mut self, target: f64) -> bool {
        if self.target == target {
            return false;
        }
        self.from = self.value;
        self.target = target;
        self.started = None;
        self.animating = true;
        true
    }

    /// Snap to `target` with no motion — the `reduce_motion` collapse, and
    /// what a widget calls when it wants a value placed rather than
    /// animated.
    pub(crate) fn jump_to(&mut self, target: f64) {
        self.value = target;
        self.from = target;
        self.target = target;
        self.started = None;
        self.animating = false;
    }

    /// Step to `now` and return the current value. The first call after a
    /// retarget latches its start time, so a ramp is timed from the frame it
    /// first painted rather than from the rebuild that staged it.
    pub(crate) fn advance(&mut self, now: FrameTime) -> f64 {
        if !self.animating {
            return self.value;
        }
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if self.ramp.is_settled(elapsed) {
            let target = self.target;
            self.jump_to(target);
            return self.value;
        }
        let progress = self.ramp.progress(elapsed);
        self.value = self.from + (self.target - self.from) * progress;
        self.value
    }
}

/// One `from → to` animation lane: a [`Ramp`], the endpoints it interpolates
/// between, and the frame the current run started on.
///
/// The endpoint pair (rather than a bare progress value) is what lets a
/// freshly-built control rest at its confirmed state with no entry
/// animation, and a mid-flight reversal start from what is actually on
/// screen. Used by `switch` and its five form-control siblings.
pub(crate) struct Lane {
    ramp: Ramp,
    from: f64,
    to: f64,
    /// The frame the current run started on; `None` while at rest.
    start: Option<FrameTime>,
    /// What [`Lane::advance`] last computed — the value paint reads.
    displayed: f64,
}

impl Lane {
    /// An idle lane resting at `value`.
    pub(crate) fn at_rest(ramp: Ramp, value: f64) -> Self {
        Self {
            ramp,
            from: value,
            to: value,
            start: None,
            displayed: value,
        }
    }

    /// Re-aim at `to`, starting from whatever is on screen now. A no-op when
    /// the lane already rests on that target, so a redundant `rebuild`
    /// cannot restart a settled run.
    pub(crate) fn retarget(&mut self, to: f64) {
        if self.to == to && self.start.is_none() {
            return;
        }
        self.from = self.displayed;
        self.to = to;
        self.start = None;
        if self.from == to {
            self.displayed = to;
        }
    }

    /// Re-aim at `to` **on a different ramp** — for a control whose entrance
    /// and exit are timed differently, which is the ordinary shape upstream
    /// (`animate` a spring, `exit` a short tween).
    pub(crate) fn retarget_with(&mut self, ramp: Ramp, to: f64) {
        if self.ramp == ramp && self.to == to && self.start.is_none() {
            return;
        }
        self.from = self.displayed;
        self.ramp = ramp;
        self.to = to;
        self.start = None;
        if self.from == to {
            self.displayed = to;
        }
    }

    /// Land on the target immediately, cancelling any flight — the
    /// `reduce_motion` path.
    pub(crate) fn snap(&mut self) {
        self.from = self.to;
        self.displayed = self.to;
        self.start = None;
    }

    /// Advance to frame time `now`, returning whether the lane is still in
    /// flight (in which case the caller owes another frame).
    pub(crate) fn advance(&mut self, now: FrameTime) -> bool {
        if self.from == self.to {
            self.displayed = self.to;
            self.start = None;
            return false;
        }
        let start = *self.start.get_or_insert(now);
        let elapsed = now.saturating_sub(start);
        if self.ramp.is_settled(elapsed) {
            self.from = self.to;
            self.displayed = self.to;
            self.start = None;
            return false;
        }
        self.displayed = self.from + (self.to - self.from) * self.ramp.progress(elapsed);
        true
    }

    /// The current value. A spring lane may pass its endpoints, which is
    /// what makes it read as a spring; a caller driving a bounded quantity
    /// clamps at the point of use.
    pub(crate) fn value(&self) -> f64 {
        self.displayed
    }

    /// The value this lane is heading for.
    pub(crate) fn target(&self) -> f64 {
        self.to
    }
}

/// Interpolate `begin` → `end` at `t`, snapping exactly to an endpoint at (or
/// past) `0.0`/`1.0` rather than routing it through [`Tween::lerp`]'s `f32`
/// arithmetic, which can land a few ULPs off `end`. Keeps a resting control
/// pixel-identical to its resting token.
pub(crate) fn lerp_color(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// Sample an evenly-spaced keyframe array at `t` in `0..=1`, linearly between
/// neighbours — Motion's own default for an array target.
pub(crate) fn keyframes_at(frames: &[f64], t: f64) -> f64 {
    match frames.len() {
        0 => 0.0,
        1 => frames[0],
        len => {
            let spans = (len - 1) as f64;
            let scaled = t.clamp(0.0, 1.0) * spans;
            let index = (scaled.floor() as usize).min(len - 2);
            let local = scaled - index as f64;
            frames[index] + (frames[index + 1] - frames[index]) * local
        }
    }
}

/// The `whileTap`-style scale a control's press shrink resolves to: `1.0` at
/// rest, `min_scale` at full press, linear between. `checkbox` and `radio`
/// both apply this to a [`Lane`]-driven `0..=1` press value.
pub(crate) fn press_scale(min_scale: f64, press: f64) -> f64 {
    1.0 - (1.0 - min_scale) * press.clamp(0.0, 1.0)
}

/// Stroke a control's 1px outline, inset by half the stroke so it lands
/// inside the box rather than straddling it. Shared by `button` and its
/// three siblings.
pub(crate) fn stroke_outline(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let half = style::BORDER_WIDTH / 2.0;
    let rect = RoundedRect::new(
        half,
        half,
        (size.width - half).max(half),
        (size.height - half).max(half),
        (radius - half).max(0.0),
    );
    scene.stroke_path(
        origin,
        &rect.to_path(style::PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

/// Paint a focus ring of [`style::FOCUS_RING_WIDTH`] around a box, `offset`
/// logical px clear of it — Tailwind's `ring-N ring-offset-N`, a pair of
/// non-inset box shadows and therefore wholly outside the border box. Shared
/// by `switch`, `checkbox` and `radio`.
pub(crate) fn draw_focus_ring(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    offset: f64,
    color: Color,
) {
    let out = offset + style::FOCUS_RING_WIDTH / 2.0;
    let ring = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, size).inset(out),
        radius + out,
    );
    scene.stroke_path(
        origin,
        &Shape::to_path(&ring, style::PATH_TOLERANCE),
        style::FOCUS_RING_WIDTH,
        &Brush::Solid(color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};
    use std::time::Duration;

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    // ---- Pointer / key admission -------------------------------------------

    #[test]
    fn inside_is_half_open_and_inside_inclusive_closes_the_far_edge() {
        let size = Size::new(40.0, 20.0);
        assert!(inside(Point::new(0.0, 0.0), size));
        assert!(inside(Point::new(39.9, 19.9), size));
        assert!(!inside(Point::new(40.0, 10.0), size));
        assert!(!inside(Point::new(-0.1, 10.0), size));

        assert!(inside_inclusive(Point::new(0.0, 0.0), size));
        assert!(
            inside_inclusive(Point::new(40.0, 20.0), size),
            "closed edge"
        );
        assert!(!inside_inclusive(Point::new(40.1, 10.0), size));
        assert!(!inside_inclusive(Point::new(-0.1, 10.0), size));
    }

    #[test]
    fn only_the_primary_button_presses() {
        let at = |button| PointerEvent {
            phase: frust::authoring::PointerPhase::Down,
            position: Point::ORIGIN,
            button,
        };
        assert!(presses(&at(PointerButton::Primary)));
        assert!(!presses(&at(PointerButton::Secondary)));
        assert!(!presses(&at(PointerButton::Middle)));
    }

    #[test]
    fn space_and_enter_activate_and_nothing_else_does() {
        let key = |k: Key| KeyEvent {
            key: k,
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        };
        assert!(is_activation_key(&key(Key::Character(" ".into()))));
        assert!(is_activation_key(&key(Key::Named(NamedKey::Enter))));
        assert!(!is_activation_key(&key(Key::Named(NamedKey::Escape))));
        assert!(!is_activation_key(&key(Key::Character("a".into()))));
    }

    // ---- The spring scalar --------------------------------------------------

    #[test]
    fn a_spring_scalar_runs_from_its_current_value_to_its_target_and_settles() {
        let mut scalar = SpringScalar::new(0.0, Ramp::spring(SPRING_PRESS));
        assert!(!scalar.is_animating());
        assert!(scalar.set_target(10.0));
        assert!(
            !scalar.set_target(10.0),
            "re-aiming at the same target is a no-op"
        );
        assert!(scalar.is_animating());

        assert_eq!(scalar.advance(at(0)), 0.0);
        let mid = scalar.advance(at(40));
        assert!(mid > 0.0 && mid < 10.0, "mid-flight: {mid}");
        let settled = scalar.advance(at(5_000));
        assert_eq!(settled, 10.0);
        assert!(!scalar.is_animating());

        // Retargeting mid-flight restarts from where it had reached.
        scalar.set_target(0.0);
        scalar.advance(at(5_000));
        let part = scalar.advance(at(5_040));
        assert!(part < 10.0 && part > 0.0);
        scalar.set_target(20.0);
        assert_eq!(
            scalar.value(),
            part,
            "the new ramp starts at the current value"
        );
        assert_eq!(scalar.target(), 20.0);
    }

    // ---- The shared lane ------------------------------------------------------

    #[test]
    fn a_lane_rests_where_it_was_built_and_settles_on_its_target() {
        let mut lane = Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0);
        assert_eq!(lane.value(), 0.0);
        assert!(!lane.advance(at(0)), "a resting lane owes no frame");

        lane.retarget(1.0);
        assert!(lane.advance(at(0)));
        assert!(lane.value() < 1.0, "the first frame seeds the clock");
        assert!(lane.advance(at(40)));
        let mid = lane.value();
        assert!(mid > 0.0 && mid < 1.0, "mid-flight: {mid}");
        assert!(!lane.advance(at(5_000)));
        assert_eq!(lane.value(), 1.0, "settles exactly on the target");
        assert_eq!(lane.target(), 1.0);
    }

    #[test]
    fn a_reversal_starts_from_what_is_on_screen_and_a_snap_lands_at_once() {
        let mut lane = Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0);
        lane.retarget(1.0);
        lane.advance(at(0));
        lane.advance(at(40));
        let caught = lane.value();
        lane.retarget(0.0);
        lane.advance(at(40));
        assert!(
            (lane.value() - caught).abs() < 1e-6,
            "a reversal starts from {caught}, not from 1.0"
        );

        lane.snap();
        assert_eq!(lane.value(), 0.0);
        assert!(!lane.advance(at(41)));
    }

    /// A lane can change ramps mid-life, which is what an entrance-spring /
    /// exit-tween pair needs.
    #[test]
    fn retarget_with_swaps_the_ramp_as_well_as_the_target() {
        let quick = Ramp::eased(Duration::from_millis(20), EASE_OUT);
        let mut lane = Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0);
        lane.retarget_with(quick, 1.0);
        lane.advance(at(0));
        assert!(
            !lane.advance(at(21)),
            "the 20ms exit ramp is done, not the spring's settle time"
        );
        assert_eq!(lane.value(), 1.0);
    }

    // ---- Small numeric / paint helpers -----------------------------------

    #[test]
    fn keyframes_interpolate_evenly_across_the_timeline() {
        let frames = [0.0, -2.0, 2.0, -1.0, 0.0];
        assert_eq!(keyframes_at(&frames, 0.0), 0.0);
        assert_eq!(keyframes_at(&frames, 0.25), -2.0);
        assert_eq!(keyframes_at(&frames, 0.5), 2.0);
        assert_eq!(keyframes_at(&frames, 1.0), 0.0);
        // Halfway between two keyframes is halfway between their values.
        assert!((keyframes_at(&frames, 0.125) + 1.0).abs() < 1e-9);
        // Out-of-range samples clamp rather than extrapolate.
        assert_eq!(keyframes_at(&frames, 5.0), 0.0);
        assert_eq!(keyframes_at(&[], 0.5), 0.0);
        assert_eq!(keyframes_at(&[7.0], 0.5), 7.0);
    }

    #[test]
    fn lerp_color_snaps_exactly_at_its_endpoints() {
        let begin = Color::from_rgb8(0, 0, 0);
        let end = Color::from_rgb8(255, 255, 255);
        assert_eq!(lerp_color(begin, end, 0.0), begin);
        assert_eq!(lerp_color(begin, end, -1.0), begin);
        assert_eq!(lerp_color(begin, end, 1.0), end);
        assert_eq!(lerp_color(begin, end, 2.0), end);
        let mid = lerp_color(begin, end, 0.5);
        assert_ne!(mid, begin);
        assert_ne!(mid, end);
    }

    #[test]
    fn press_scale_floors_at_min_scale_and_clamps_out_of_range_press() {
        assert_eq!(press_scale(0.92, 0.0), 1.0);
        assert_eq!(press_scale(0.92, 1.0), 0.92);
        assert_eq!(press_scale(0.92, 2.0), 0.92, "clamps above 1.0");
        assert_eq!(press_scale(0.92, -1.0), 1.0, "clamps below 0.0");
        let mid = press_scale(0.92, 0.5);
        assert!(mid > 0.92 && mid < 1.0);
    }
}
