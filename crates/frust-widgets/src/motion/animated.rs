//! Declarative single-child implicit-animation wrappers:
//! [`AnimatedOpacity`]/[`AnimatedScale`].
//!
//! # Shape
//!
//! [`animated_opacity(target, child)`](animated_opacity)/
//! [`animated_scale(target, child)`](animated_scale) are controlled,
//! transparent single-child wrappers (Flutter's `AnimatedOpacity`/
//! `AnimatedScale` analogs): on every rebuild a changed `target` retargets a
//! retained animation, and paint composites the child under
//! [`PaintScene::push_layer`] (opacity, `alpha`) or
//! [`PaintScene::push_transform`] (scale, about an [`Alignment`] point) —
//! advancing during paint and calling [`PaintCtx::request_frame`] while still
//! in flight, the crate's shared advance-during-paint contract (see
//! `docs/CODE_STANDARDS.md`'s Theming & Animation Conventions). Layout is a
//! transparent pass-through: this wrapper reports exactly its child's size.
//!
//! # Retargeting: value continuity, not velocity continuity
//!
//! [`ImplicitAnim`] is the shared retarget driver both wrappers use. A
//! target change captures the *exact currently-painted value* as the new
//! [`Tween`]'s `begin`, then launches a fresh `0..1` progress
//! [`crate::nav::transition::TransitionDriver`] (reusing the
//! duration/spring [`Timing`] progress driver — the same vocabulary a page
//! transition's progress is driven by, just mapped onto an arbitrary value
//! range via `Tween` instead of a page's own `0..1` transition progress) via
//! [`crate::nav::transition::make_driver`]. Because the fresh driver's
//! progress starts at exactly `0.0` and `Tween::lerp(0.0) == begin` by
//! construction, retargeting mid-flight is **value-continuous**: the painted
//! value never jumps at the instant of a retarget. It is *not*
//! velocity-continuous — a spring [`Timing::Spring`] retarget always
//! launches its canonical from-rest shape rather than preserving the
//! interrupted spring's exact instantaneous velocity (`AnimationController`
//! exposes no public velocity accessor to seed a fresh `Spring` with); this
//! is a documented simplification, not a bug, and still reads as smooth
//! motion since the value itself never discontinues.
//!
//! # Hit-testing: paint-only effects, unscaled event space (v1)
//!
//! Both `push_layer`/`push_transform` are **paint-only** compositing
//! primitives (`docs/CODE_STANDARDS.md`'s Compositing-primitive
//! constraints) — a scaled or faded subtree still hit-tests at its
//! *unscaled* layout geometry. This wrapper's `event`/layout never apply the
//! animation, so a child under [`AnimatedScale`] mid-shrink is still hit at
//! its full laid-out bounds — the same v1 rule huddle's `PressPop` precedent
//! established (see `push_transform`'s own doc). A future addition may add a
//! scaled-hit-test opt-in; this module deliberately does not.

use std::time::Duration;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, Tween, View, Widget, any,
};
use frust_theme::{MotionSpring, Theme};
use kurbo::{Affine, Point, Size};

use crate::nav::transition::{TransitionDriver, make_driver};
use crate::{Alignment, Timing};

// --- Unthemed fallbacks (CODE_STANDARDS' "resolve theme tokens with an
// unthemed-fallback constant per resolved value" rule) -----------------------

/// The unthemed fallback spring for opacity fades, matching
/// `MotionScheme::m3_expressive()`'s `default_effects` preset exactly
/// (critically damped, stiffness 1600) — see `frust-theme`'s `motion` module
/// docs for why "effects" springs (opacity/color) are critically damped
/// (never overshoot).
const FALLBACK_EFFECTS_SPRING: MotionSpring = MotionSpring {
    damping_ratio: 1.0,
    stiffness: 1600.0,
};

/// The unthemed fallback spring for scale, matching
/// `MotionScheme::m3_expressive()`'s `default_spatial` preset exactly (a
/// slight overshoot, ζ 0.9) — "spatial" springs (position/size) allow a
/// small bounce past the target.
const FALLBACK_SPATIAL_SPRING: MotionSpring = MotionSpring {
    damping_ratio: 0.9,
    stiffness: 700.0,
};

/// Resolve the effective [`Timing`] for opacity: an explicit builder value
/// always wins; otherwise the theme's `default_effects` spring
/// (`MotionScheme`'s "effects slot" — see `frust-theme::motion`'s module
/// docs: "'Effects' springs (opacity/color) are critically damped"), falling
/// back to [`FALLBACK_EFFECTS_SPRING`] when no theme is threaded.
fn resolve_opacity_timing(theme: Option<&Theme>, explicit: Option<Timing>) -> Timing {
    explicit.unwrap_or_else(|| {
        let spring = match theme {
            Some(theme) => theme.motion.default_effects,
            None => FALLBACK_EFFECTS_SPRING,
        };
        Timing::Spring(spring)
    })
}

/// Resolve the effective [`Timing`] for scale: an explicit builder value
/// always wins; otherwise the theme's `default_spatial` spring
/// (`MotionScheme`'s "spatial slot" — see `frust-theme::motion`'s module
/// docs: "'spatial' springs (position/size) are `0.9`, allowing a small
/// overshoot"), falling back to [`FALLBACK_SPATIAL_SPRING`] when no theme is
/// threaded.
fn resolve_scale_timing(theme: Option<&Theme>, explicit: Option<Timing>) -> Timing {
    explicit.unwrap_or_else(|| {
        let spring = match theme {
            Some(theme) => theme.motion.default_spatial,
            None => FALLBACK_SPATIAL_SPRING,
        };
        Timing::Spring(spring)
    })
}

/// Map an [`Alignment`] component in `-1.0..=1.0` to a `0.0..=1.0` fraction —
/// duplicated from `align.rs`'s private `Alignment::fraction` (same math,
/// kept local since that helper isn't `pub`) so [`AnimatedScaleWidget`] can
/// locate its scale pivot the same way `Align` locates a child.
fn alignment_fraction(component: f64) -> f64 {
    (component + 1.0) / 2.0
}

/// Build the affine that scales uniformly by `scale` about the absolute
/// point `pivot` — a translate/scale/translate-back sandwich, the standard
/// "scale about a point" construction (mirrors
/// `nav::transition::rect_to_rect`'s translate-scale-translate shape).
fn scale_about(pivot: Point, scale: f64) -> Affine {
    Affine::translate((pivot.x, pivot.y))
        * Affine::scale(scale)
        * Affine::translate((-pivot.x, -pivot.y))
}

// --- Shared retarget driver --------------------------------------------------

/// Shared implicit-animation driver for [`AnimatedOpacityWidget`]/
/// [`AnimatedScaleWidget`] — see the [module docs](self)'s Retargeting
/// section for the full value-continuity contract.
struct ImplicitAnim {
    /// The app-confirmed value, updated by `rebuild` (a `BuildCtx` pass has
    /// no theme to resolve a default [`Timing`] from, so retargeting itself
    /// is deferred to the next `paint` — mirrors `SwitchWidget`'s
    /// `checked`/`anim_target` split).
    target: f64,
    /// The target the current `driver` is actually animating toward.
    /// Compared against `target` on every [`Self::advance`] call to detect a
    /// pending retarget.
    driving_target: f64,
    /// Maps the driver's `0..1` progress onto the actual value range:
    /// `begin` is the value at the instant the current retarget started,
    /// `end` is `driving_target`.
    tween: Tween<f64>,
    /// The `0..1` progress driver (the page-transition driver, reused here —
    /// see the [module docs](self)).
    driver: TransitionDriver,
}

impl ImplicitAnim {
    /// A fresh driver settled at `initial` — no animate-in on first mount
    /// (Flutter's implicit-animation convention: only a *later* value change
    /// animates, never the initial one).
    fn new(initial: f64) -> Self {
        // A degenerate zero-duration Timing settles a driver immediately, but
        // it's moot here regardless: `tween`'s begin/end are both `initial`,
        // so `value()` reads `initial` no matter what progress the
        // placeholder driver reports.
        let (driver, _) = make_driver(Timing::Duration(Duration::ZERO, frust_core::Curve::Linear));
        Self {
            target: initial,
            driving_target: initial,
            tween: Tween::new(initial, initial),
            driver,
        }
    }

    /// The current interpolated value.
    fn value(&self) -> f64 {
        self.tween.lerp(self.driver.value())
    }

    /// Record a new app-confirmed target (called from `rebuild`). The actual
    /// retarget (capturing `value()` as the new tween's `begin`, launching a
    /// fresh driver) happens lazily in [`Self::advance`], the next time a
    /// theme is in scope to resolve a default `timing` from.
    fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Advance one frame at `now`. If `target` disagrees with
    /// `driving_target` (a retarget is pending), captures the current value
    /// as the new tween's `begin` and launches a fresh driver under
    /// `timing`. Returns whether still animating (the caller should
    /// [`PaintCtx::request_frame`]).
    fn advance(&mut self, now: FrameTime, timing: Timing) -> bool {
        if self.target != self.driving_target {
            let from = self.value();
            self.tween = Tween::new(from, self.target);
            self.driving_target = self.target;
            let (driver, _) = make_driver(timing);
            self.driver = driver;
        }
        self.driver.advance(now).animating
    }
}

// --- AnimatedOpacity ---------------------------------------------------------

/// A declarative opacity wrapper. See the [module docs](self).
pub struct AnimatedOpacityView<State: 'static> {
    target: f64,
    timing: Option<Timing>,
    child: AnyView<State>,
}

/// Fade `child` toward `target` opacity (clamped to `0.0..=1.0`) — see the
/// [module docs](self). Defaults its [`Timing`] from the theme's
/// `default_effects` spring; override with
/// [`.timing(...)`](AnimatedOpacityView::timing).
pub fn animated_opacity<State: 'static, V: View<State>>(
    target: f64,
    child: V,
) -> AnimatedOpacityView<State> {
    AnimatedOpacityView {
        target: target.clamp(0.0, 1.0),
        timing: None,
        child: any(child),
    }
}

/// PascalCase alias for [`animated_opacity`] (mirrors `Switch`/`switch`'s
/// dual naming).
#[allow(non_snake_case)]
pub fn AnimatedOpacity<State: 'static, V: View<State>>(
    target: f64,
    child: V,
) -> AnimatedOpacityView<State> {
    animated_opacity(target, child)
}

impl<State: 'static> AnimatedOpacityView<State> {
    /// Override the default (theme-resolved) fade timing.
    pub fn timing(mut self, timing: Timing) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// The retained widget for an [`AnimatedOpacityView`].
pub struct AnimatedOpacityWidget {
    child: ChildPod,
    explicit_timing: Option<Timing>,
    anim: ImplicitAnim,
}

impl<State: 'static> View<State> for AnimatedOpacityView<State> {
    type Element = AnimatedOpacityWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnimatedOpacityWidget {
        AnimatedOpacityWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            explicit_timing: self.timing,
            anim: ImplicitAnim::new(self.target),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnimatedOpacityWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.target != self.target {
            element.anim.set_target(self.target);
            flags |= ChangeFlags::PAINT;
        }
        if prev.timing != self.timing {
            element.explicit_timing = self.timing;
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut AnimatedOpacityWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AnimatedOpacityWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Transparent wrapper: exactly the child's size (module docs).
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let timing = resolve_opacity_timing(theme, self.explicit_timing);
        if self.anim.advance(ctx.frame_time(), timing) {
            ctx.request_frame();
        }
        // Clamped: alpha is never meaningful outside [0, 1], even if a spring
        // `Timing` would otherwise overshoot past the target (see the module
        // docs' Retargeting section and `nav::transition`'s identical
        // opacity-clamps/position-doesn't precedent).
        let alpha = self.anim.value().clamp(0.0, 1.0) as f32;
        let origin = ctx.origin();
        let size = ctx.size();
        scene.push_layer(origin, size, alpha);
        self.child.paint_child(ctx, scene);
        scene.pop_layer();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

// --- AnimatedScale ------------------------------------------------------------

/// A declarative scale wrapper. See the [module docs](self).
pub struct AnimatedScaleView<State: 'static> {
    target: f64,
    alignment: Alignment,
    timing: Option<Timing>,
    child: AnyView<State>,
}

/// Scale `child` toward `target` (a scale factor — `1.0` is unscaled; no
/// range restriction, unlike opacity) about [`Alignment::CENTER`] by default —
/// see the [module docs](self). Defaults its [`Timing`] from the theme's
/// `default_spatial` spring; override with
/// [`.timing(...)`](AnimatedScaleView::timing); override the pivot with
/// [`.alignment(...)`](AnimatedScaleView::alignment).
pub fn animated_scale<State: 'static, V: View<State>>(
    target: f64,
    child: V,
) -> AnimatedScaleView<State> {
    AnimatedScaleView {
        target,
        alignment: Alignment::CENTER,
        timing: None,
        child: any(child),
    }
}

/// PascalCase alias for [`animated_scale`] (mirrors `Switch`/`switch`'s dual
/// naming).
#[allow(non_snake_case)]
pub fn AnimatedScale<State: 'static, V: View<State>>(
    target: f64,
    child: V,
) -> AnimatedScaleView<State> {
    animated_scale(target, child)
}

impl<State: 'static> AnimatedScaleView<State> {
    /// Override the pivot point the scale is applied about (default
    /// [`Alignment::CENTER`]).
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Override the default (theme-resolved) scale timing.
    pub fn timing(mut self, timing: Timing) -> Self {
        self.timing = Some(timing);
        self
    }
}

/// The retained widget for an [`AnimatedScaleView`].
pub struct AnimatedScaleWidget {
    child: ChildPod,
    alignment: Alignment,
    explicit_timing: Option<Timing>,
    anim: ImplicitAnim,
}

impl<State: 'static> View<State> for AnimatedScaleView<State> {
    type Element = AnimatedScaleWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnimatedScaleWidget {
        AnimatedScaleWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            alignment: self.alignment,
            explicit_timing: self.timing,
            anim: ImplicitAnim::new(self.target),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnimatedScaleWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.target != self.target {
            element.anim.set_target(self.target);
            flags |= ChangeFlags::PAINT;
        }
        if prev.alignment != self.alignment {
            element.alignment = self.alignment;
            flags |= ChangeFlags::PAINT;
        }
        if prev.timing != self.timing {
            element.explicit_timing = self.timing;
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut AnimatedScaleWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AnimatedScaleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Transparent wrapper: exactly the child's size, unaffected by the
        // (paint-only) scale — see the module docs' hit-testing section.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let timing = resolve_scale_timing(theme, self.explicit_timing);
        if self.anim.advance(ctx.frame_time(), timing) {
            ctx.request_frame();
        }
        // Unclamped: a spatial spring's overshoot past the target is real,
        // intended motion (see the module docs' Retargeting section and
        // `AnimationController`'s own Overshoot docs).
        let scale = self.anim.value();

        let origin = ctx.origin();
        let size = ctx.size();
        let fx = alignment_fraction(self.alignment.x);
        let fy = alignment_fraction(self.alignment.y);
        let pivot = Point::new(origin.x + size.width * fx, origin.y + size.height * fy);

        scene.push_transform(scale_about(pivot, scale));
        self.child.paint_child(ctx, scene);
        scene.pop_transform();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Unscaled event space (v1) — see the module docs' hit-testing
        // section: `route_event_single` hit-tests the child's laid-out
        // (unscaled) bounds via `ChildPod::contains`, never the paint-time
        // transform.
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf_any};
    use frust_core::Curve;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    // Every test below advances the widget's retained `anim` field directly
    // (a private field, reachable here since `tests` is a descendant module
    // of `animated`) rather than through repeated `Widget::paint` calls with
    // different `PaintCtx` frame times -- `PaintCtx::set_frame_time` is
    // crate-private to `frust-core`, so `frust-widgets` cannot construct a
    // `PaintCtx` at an arbitrary frame time. This mirrors
    // `material::switch`'s own paint/animation tests exactly (see that
    // module's `animation_progresses_toward_and_settles_at_target` test and
    // its comment). A single `paint` call at the default `FrameTime::ZERO`
    // (to trigger the lazy retarget, or to read back the scene after
    // manually advancing `anim`) is still exercised in every test, so the
    // real `Widget::paint` compositing path (`push_layer`/`push_transform`)
    // is what's actually asserted against.

    // --- AnimatedOpacity ---

    #[test]
    fn opacity_mid_animation_paint_emits_expected_interpolated_alpha() {
        let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let view: AnimatedOpacityView<()> =
            animated_opacity(0.0, leaf_any(10.0, 10.0)).timing(timing);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));

        // Simulate what `rebuild` would do on a target change (mirrors
        // `SwitchWidget`'s test precedent of poking the confirmed-value
        // field directly rather than calling `View::rebuild`).
        w.anim.set_target(1.0);

        // First paint (frame_time == FrameTime::ZERO) starts the retarget:
        // seeds the driver's clock at a zero delta, alpha still 0.0.
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);
        assert_eq!(
            scene.layers[0].2, 0.0,
            "alpha must still be 0.0 on the seed frame"
        );

        // Directly advance to the 50ms midpoint of the 100ms linear fade.
        assert!(
            w.anim.advance(ft_ms(50.0), timing),
            "must still be animating at the midpoint"
        );

        // Re-paint at (still) FrameTime::ZERO: the retarget check is a no-op
        // (already retargeted above), and re-advancing the driver at ZERO
        // produces a zero -- never negative -- delta (`FrameTime::saturating_sub`),
        // so this just repaints the already-advanced midpoint value.
        let mut pctx2 = PaintCtx::new(Point::ZERO, size);
        let mut scene2 = RecordingScene::default();
        w.paint(&mut pctx2, &mut scene2);

        assert_eq!(scene2.layers.len(), 1, "expected one push_layer call");
        let (layer_origin, layer_size, alpha) = scene2.layers[0];
        assert_eq!(layer_origin, Point::ZERO);
        assert_eq!(layer_size, size);
        assert!(
            (alpha - 0.5).abs() < 1e-4,
            "expected ~0.5 alpha halfway through a linear fade, got {alpha}"
        );
        assert_eq!(
            scene2.layer_pops, 1,
            "push_layer must be popped exactly once"
        );
    }

    #[test]
    fn opacity_settles_at_target_and_stops_requesting_frames() {
        let timing = Timing::Duration(Duration::from_millis(50), Curve::Linear);
        let view: AnimatedOpacityView<()> =
            animated_opacity(0.0, leaf_any(10.0, 10.0)).timing(timing);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));

        w.anim.set_target(1.0);
        // Well past the 50ms duration: settles at the target.
        let mut running = true;
        let mut t = 0.0;
        for _ in 0..1000 {
            running = w.anim.advance(ft_ms(t), timing);
            if !running {
                break;
            }
            t += 1000.0 / 120.0;
        }
        assert!(
            !running,
            "the fade must settle within a bounded number of steps"
        );

        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);

        assert!(
            !pctx.needs_frame(),
            "a settled animation must not request a frame"
        );
        let (_, _, alpha) = scene.layers[0];
        assert!((alpha - 1.0).abs() < 1e-6);
    }

    // --- AnimatedScale ---

    #[test]
    fn scale_emits_push_transform_and_pop_transform_bracketing_the_child() {
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(20.0, 20.0))
            .timing(Timing::Duration(Duration::from_millis(50), Curve::Linear));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));

        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);

        assert_eq!(
            scene.transforms.len(),
            1,
            "expected one push_transform call"
        );
        assert_eq!(
            scene.transform_pops, 1,
            "push_transform must be popped exactly once"
        );
        assert_eq!(
            scene.rects.len(),
            1,
            "the child must paint (its fill_rect) between push/pop"
        );
    }

    #[test]
    fn scale_target_change_mid_flight_retargets_without_a_value_jump() {
        // Start at 1.0, retarget to 2.0, advance partway, read the
        // interpolated value, then retarget again to 0.5 at the SAME
        // instant (zero dt) -- the freshly-captured `from` must exactly
        // equal the last-painted value: no jump.
        let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(20.0, 20.0)).timing(timing);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));

        w.anim.set_target(2.0);
        // Seed (zero dt), then advance to the 50ms halfway point: value
        // should read 1.5 (halfway 1.0 -> 2.0 under a linear curve).
        assert!(w.anim.advance(ft_ms(0.0), timing));
        assert!(w.anim.advance(ft_ms(50.0), timing));
        let value_before_retarget = w.anim.value();
        assert!(
            (value_before_retarget - 1.5).abs() < 1e-6,
            "expected 1.5 halfway through 1.0 -> 2.0, got {value_before_retarget}"
        );

        // Retarget again, same instant (zero dt on the very next advance).
        w.anim.set_target(0.5);
        assert!(w.anim.advance(ft_ms(50.0), timing));
        let value_after_retarget = w.anim.value();

        assert!(
            (value_before_retarget - value_after_retarget).abs() < 1e-9,
            "a retarget must not jump the value: before {value_before_retarget},              after {value_after_retarget}"
        );

        // And the paint path reflects whatever `anim.value()` reports, end to end.
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);
        let painted = scene.transforms[0] * Point::new(1.0, 0.0);
        let pivot_x = size.width / 2.0;
        let expected_x = pivot_x + (1.0 - pivot_x) * value_after_retarget;
        assert!(
            (painted.x - expected_x).abs() < 1e-6,
            "the painted transform must reflect the just-verified continuous value"
        );
    }

    #[test]
    fn scale_default_timing_resolves_unthemed_fallback_and_animates() {
        // No explicit `.timing(...)` and no theme threaded: `paint` must
        // resolve the unthemed fallback spring, animate, and request a
        // continuation frame (not panic, not stall at the wrong value).
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));

        w.anim.set_target(2.0);
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);

        assert_eq!(scene.transforms.len(), 1);
        assert!(
            pctx.needs_frame(),
            "a freshly retargeted spring must request another frame"
        );
    }

    #[test]
    fn rebuild_forwards_a_target_change_into_the_retained_anim() {
        // Unlike the tests above (which poke `anim` directly, matching
        // `SwitchWidget`'s precedent), this one exercises the real
        // `View::rebuild` path end to end, proving the wiring itself (not
        // just `ImplicitAnim` in isolation).
        let view: AnimatedOpacityView<()> = animated_opacity(0.0, leaf_any(10.0, 10.0));
        let mut w = build(&view);

        let retarget: AnimatedOpacityView<()> = animated_opacity(1.0, leaf_any(10.0, 10.0));
        let mut counter = 0u64;
        let flags = retarget.rebuild(&view, &mut w, &mut BuildCtx::new(&mut counter));

        assert_eq!(flags, ChangeFlags::PAINT);
        assert_eq!(w.anim.target, 1.0, "rebuild must forward the new target");
    }
}
