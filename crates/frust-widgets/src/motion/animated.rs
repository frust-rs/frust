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
//! [`AnimatedOpacityView::cached`] paints the child through a retained-page
//! boundary ([`repaint_boundary`](crate::repaint_boundary)) instead, the fade
//! riding the page's composite alpha, so a fade over an unchanged child does
//! not repaint it. `AnimatedScale` has no such option: a page is only ever
//! served at the scale it was recorded at.
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
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, Tween, View, Widget, any,
};
use frust_theme::{MotionExtras, MotionSpring, Theme};
use kurbo::{Affine, Point, Size};

use crate::nav::transition::{TransitionDriver, make_driver};
use crate::repaint_boundary::RetainedBoundary;
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

/// Whether the threaded theme asks for reduced motion — the collapse rule the
/// navigator and `PatternSwitcher` follow, read at paint so a flip lands on the
/// next paint. `false` unthemed, so an unthemed paint animates as before.
fn reduce_motion(theme: Option<&Theme>) -> bool {
    theme.is_some_and(|theme| theme.motion.reduce_motion)
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

/// How close `value()` must sit to its target to count as landed: far above
/// f64 lerp residue (≈1e-16), far below any visible opacity or scale step.
const SETTLED_EPS: f64 = 1e-9;

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
    /// Whether the current `driver` was set under the reduced-motion collapse
    /// (a flip re-drives an in-flight value under the other timing).
    driving_reduced: bool,
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
            driving_reduced: false,
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
        self.drive(now, timing, false)
    }

    /// Advance one frame under the reduced-motion collapse: a linear ramp of
    /// `fade` (the theme's reduced fade; 120 ms by default), regardless of any
    /// explicit timing. A zero `fade` lands on the target this paint.
    fn advance_collapsed(&mut self, now: FrameTime, fade: Duration) -> bool {
        self.drive(now, Timing::Duration(fade, Curve::Linear), true)
    }

    /// Whether the value has landed on the target with no retarget pending: the
    /// driver finished, or the value sits within [`SETTLED_EPS`] of the target
    /// (a fresh or snapped `(x, x)` tween not yet ticked). Never `==`: once the
    /// driver reports `1.0`, `begin + (end − begin)` need not return `end`
    /// (`0.03 + (0.3 − 0.03)` is `0.30000000000000004`), and an exact test would
    /// re-drive a settled fade on a `reduce_motion` flip for 120 ms of no-ops.
    fn settled(&self) -> bool {
        if self.driving_target != self.target {
            return false;
        }
        let driver_idle = match &self.driver {
            TransitionDriver::Auto(controller) => !controller.is_animating(),
            _ => false,
        };
        driver_idle || (self.value() - self.target).abs() <= SETTLED_EPS
    }

    /// Settle straight at the target, bypassing the ramp so an in-flight scale
    /// lands at this paint; [`AnimatedScaleWidget`] uses it under
    /// `reduce_motion`, which never runs a spatial ramp. Returns whether it
    /// rebuilt the driver: `false` when the value already sits on the target
    /// under the reduced collapse, so a settled reduced paint builds nothing.
    fn snap(&mut self) -> bool {
        if self.driving_reduced && self.settled() {
            return false;
        }
        self.tween = Tween::new(self.target, self.target);
        self.driving_target = self.target;
        self.driving_reduced = true;
        let (driver, _) = make_driver(Timing::Duration(Duration::ZERO, Curve::Linear));
        self.driver = driver;
        true
    }

    /// Shared body of [`Self::advance`] and [`Self::advance_collapsed`]: a
    /// pending target retargets from the current value; a collapse-flag flip
    /// re-drives only an in-flight value (a settled one reports not animating).
    fn drive(&mut self, now: FrameTime, timing: Timing, reduced: bool) -> bool {
        let retargeted = self.target != self.driving_target;
        let flipped = reduced != self.driving_reduced;
        self.driving_reduced = reduced;
        if !retargeted {
            if !flipped {
                return self.driver.advance(now).animating;
            }
            if self.settled() {
                return false;
            }
        }
        let from = self.value();
        self.tween = Tween::new(from, self.target);
        self.driving_target = self.target;
        let (driver, _) = make_driver(timing);
        self.driver = driver;
        self.driver.advance(now).animating
    }
}

// --- AnimatedOpacity ---------------------------------------------------------

/// A declarative opacity wrapper. See the [module docs](self). Under
/// `reduce_motion` the fade is a linear collapse running for the theme's
/// `MotionExtras::reduced_fade` (120 ms by default), whatever `.timing(...)` is set.
pub struct AnimatedOpacityView<State: 'static> {
    target: f64,
    timing: Option<Timing>,
    cached: bool,
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
        cached: false,
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

    /// Paint the child through a retained-page boundary
    /// ([`repaint_boundary`](crate::repaint_boundary)) and drive the fade
    /// through its composite alpha (default `false`): a running fade over an
    /// unchanged child composites the retained page instead of painting the
    /// child every frame. Worth it for an expensive child that stays still
    /// while it fades.
    pub fn cached(mut self, cached: bool) -> Self {
        self.cached = cached;
        self
    }
}

/// The retained widget for an [`AnimatedOpacityView`].
///
/// Keyboard input (`Shortcut`, `Key`, `Ime`, `EditCommand`) reaches the child
/// and the child is a Tab stop only while the fade is above opacity 0; an
/// invisible child answers no chord and takes no grant.
pub struct AnimatedOpacityWidget {
    child: ChildPod,
    explicit_timing: Option<Timing>,
    anim: ImplicitAnim,
    /// The retained-page boundary the child paints through, when
    /// [`cached`](AnimatedOpacityView::cached).
    boundary: Option<RetainedBoundary>,
}

impl<State: 'static> View<State> for AnimatedOpacityView<State> {
    type Element = AnimatedOpacityWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnimatedOpacityWidget {
        AnimatedOpacityWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            explicit_timing: self.timing,
            anim: ImplicitAnim::new(self.target),
            boundary: self.cached.then(RetainedBoundary::new),
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
        if element.boundary.is_some() != self.cached {
            element.boundary = self.cached.then(RetainedBoundary::new);
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
        let animating = if reduce_motion(theme) {
            let fade = MotionExtras::of(theme).reduced_fade_duration();
            self.anim.advance_collapsed(ctx.frame_time(), fade)
        } else {
            let timing = resolve_opacity_timing(theme, self.explicit_timing);
            self.anim.advance(ctx.frame_time(), timing)
        };
        if animating {
            ctx.request_frame();
        }
        // Clamped: alpha is never meaningful outside [0, 1], even if a spring
        // `Timing` would otherwise overshoot past the target (see the module
        // docs' Retargeting section and `nav::transition`'s identical
        // opacity-clamps/position-doesn't precedent).
        let alpha = self.anim.value().clamp(0.0, 1.0) as f32;
        let origin = ctx.origin();
        let size = ctx.size();
        if let Some(boundary) = &mut self.boundary {
            boundary.paint(&mut self.child, ctx, scene, origin, size, alpha, true);
            return;
        }
        scene.push_layer(origin, size, alpha);
        self.child.paint_child(ctx, scene);
        scene.pop_layer();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Fully faded out the child still paints but is invisible: it must not
        // answer a chord or a key (see the type docs).
        if self.anim.value() <= 0.0
            && matches!(
                event,
                InputEvent::Shortcut(_)
                    | InputEvent::Key(_)
                    | InputEvent::Ime(_)
                    | InputEvent::EditCommand(_)
            )
        {
            return EventResult::Ignored;
        }
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        // An invisible child is no Tab stop either.
        if self.anim.value() > 0.0 {
            visitor(&self.child);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

// --- AnimatedScale ------------------------------------------------------------

/// A declarative scale wrapper. See the [module docs](self). Under
/// `reduce_motion` the scale snaps to the target on the first paint, whatever
/// `.timing(...)` is set.
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
        if reduce_motion(theme) {
            // Reduced motion snaps: no spatial ramp, no frame request.
            self.anim.snap();
        } else {
            let timing = resolve_scale_timing(theme, self.explicit_timing);
            if self.anim.advance(ctx.frame_time(), timing) {
                ctx.request_frame();
            }
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
    fn a_fully_faded_out_child_answers_no_shortcut_but_a_visible_one_does() {
        use crate::test_support::probe;
        let shortcut = InputEvent::Shortcut(frust_core::ShortcutId::next());
        let fire = |target: f64| {
            let view: AnimatedOpacityView<Vec<u32>> = animated_opacity(target, probe(7));
            let mut w = build(&view);
            let size = w.layout(
                &mut LayoutCtx::new(),
                &BoxConstraints::tight(Size::new(50.0, 50.0)),
            );
            let mut log: Vec<u32> = Vec::new();
            let mut ectx = EventCtx::new(&mut log, Point::ZERO, size);
            w.event(&mut ectx, &shortcut);
            log
        };
        assert!(fire(0.0).is_empty(), "an invisible child must not answer");
        assert_eq!(fire(1.0), vec![7], "a visible child still answers");
        assert_eq!(fire(0.4), vec![7], "a partly faded child still answers");
    }

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

    /// The fade's alpha after `raw_ms` of shell time under `scale`, driven
    /// through a real `RenderRoot` (whose clock applies the scale).
    fn fade_alpha_after(raw_ms: f64, scale: f64) -> f32 {
        frust_core::anim::with_thread_animation_time_scale(scale, || {
            let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
            let fade =
                move |target: f64| animated_opacity(target, leaf_any(10.0, 10.0)).timing(timing);
            let mut root = frust_core::RenderRoot::new();
            root.rebuild(&mut |_: &mut ()| fade(0.0), &mut ());
            root.layout(Size::new(50.0, 50.0));
            root.paint(&mut RecordingScene::default(), ft_ms(1000.0));
            root.rebuild(&mut |_: &mut ()| fade(1.0), &mut ());
            root.paint(&mut RecordingScene::default(), ft_ms(1000.0));
            let mut scene = RecordingScene::default();
            root.paint(&mut scene, ft_ms(1000.0 + raw_ms));
            scene.layers[0].2
        })
    }

    #[test]
    fn animated_opacity_runs_four_times_slower_at_quarter_scale() {
        let quarter = fade_alpha_after(200.0, 0.25);
        assert!(
            (quarter - 0.5).abs() < 1e-4,
            "200 ms raw at 0.25 is the midpoint: {quarter}"
        );
        assert!((fade_alpha_after(50.0, 1.0) - quarter).abs() < 1e-4);
        assert!(
            (fade_alpha_after(200.0, 1.0) - 1.0).abs() < 1e-6,
            "unscaled, 200 ms is done"
        );
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

    // --- Reduced motion ---

    /// A paint context at `ms`, themed with `theme`.
    fn themed_ctx(theme: &Theme, size: Size, ms: f64) -> PaintCtx<'_> {
        PaintCtx::for_test(Point::ZERO, size, ft_ms(ms)).with_theme(theme)
    }

    #[test]
    fn reduced_opacity_ignores_an_explicit_spring_and_fades_linearly_in_120ms() {
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let spring = Timing::Spring(MotionSpring {
            damping_ratio: 1.0,
            stiffness: 1600.0,
        });
        let view: AnimatedOpacityView<()> =
            animated_opacity(1.0, leaf_any(10.0, 10.0)).timing(spring);
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.0);

        // The first paint seeds the clock: the collapsed fade is now in flight.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame(), "the collapsed fade must be in flight");

        // Halfway through the 120 ms linear fade the alpha reads 0.5; the
        // explicit critically damped spring would read about 0.31 at this
        // point, so this also proves the spring was not used.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 60.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, alpha) = scene.layers[0];
        assert!(
            (alpha - 0.5).abs() < 1e-3,
            "expected 0.5 halfway through the linear collapse, got {alpha}"
        );
        assert!(ctx.needs_frame());

        // Past 120 ms: settled at the target, with no further frame.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 130.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, alpha) = scene.layers[0];
        assert!(
            alpha.abs() < 1e-6,
            "expected the target alpha 0, got {alpha}"
        );
        assert!(
            !ctx.needs_frame(),
            "a settled collapse must not request a frame"
        );
    }

    /// A reduce-motion theme carrying `reduced_fade` milliseconds.
    fn reduced_theme_with_fade(reduced_fade: f64) -> Theme {
        Theme::builder(Theme::neutral())
            .map_motion(|motion| frust_theme::MotionScheme {
                reduce_motion: true,
                ..motion
            })
            .extension(MotionExtras {
                reduced_fade,
                ..MotionExtras::neutral()
            })
            .build()
    }

    #[test]
    fn animated_opacity_collapse_uses_the_theme_reduced_fade() {
        let theme = reduced_theme_with_fade(40.0);
        let view: AnimatedOpacityView<()> = animated_opacity(1.0, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.0);

        let mut ctx = themed_ctx(&theme, size, 0.0);
        w.paint(&mut ctx, &mut RecordingScene::default());
        assert!(ctx.needs_frame());

        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&theme, size, 20.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, alpha) = scene.layers[0];
        assert!(
            (alpha - 0.5).abs() < 1e-3,
            "halfway through a 40 ms fade the alpha reads 0.5, got {alpha}"
        );

        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&theme, size, 45.0);
        w.paint(&mut ctx, &mut scene);
        assert!(scene.layers[0].2.abs() < 1e-6, "settled past 40 ms");
        assert!(!ctx.needs_frame());
    }

    #[test]
    fn zero_reduced_fade_snaps_without_requesting_a_frame() {
        let theme = reduced_theme_with_fade(0.0);
        let view: AnimatedOpacityView<()> = animated_opacity(1.0, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.0);

        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&theme, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        assert!(
            scene.layers[0].2.abs() < 1e-6,
            "one paint lands on the target, got {}",
            scene.layers[0].2
        );
        assert!(!ctx.needs_frame(), "a zero fade must not request a frame");
    }

    #[test]
    fn reduced_scale_snaps_to_target_on_the_first_paint_with_no_frame() {
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(20.0, 20.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.62);

        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 0.0);
        w.paint(&mut ctx, &mut scene);

        assert_eq!(
            scene.transforms.len(),
            1,
            "expected one push_transform call"
        );
        assert!(!ctx.needs_frame(), "the snap must not request a frame");
        let pivot_x = size.width / 2.0;
        let painted = scene.transforms[0] * Point::new(1.0, 0.0);
        let expected_x = pivot_x + (1.0 - pivot_x) * 0.62;
        assert!(
            (painted.x - expected_x).abs() < 1e-9,
            "expected the target scale 0.62 about the centre, got x {}",
            painted.x
        );
    }

    #[test]
    fn reduced_flip_mid_flight_lands_the_scale_on_its_target() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(20.0, 20.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(2.0);

        // Normal spring ramp: seed the clock, then step into flight.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 50.0);
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame(), "the spring ramp must still be in flight");

        // Flip reduce_motion on: the very next paint lands on the target.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 60.0);
        w.paint(&mut ctx, &mut scene);
        let pivot_x = size.width / 2.0;
        let painted = scene.transforms[0] * Point::new(1.0, 0.0);
        let expected_x = pivot_x + (1.0 - pivot_x) * 2.0;
        assert!(
            (painted.x - expected_x).abs() < 1e-9,
            "a mid-flight flip must land on the target 2.0, got x {}",
            painted.x
        );
        assert!(
            !ctx.needs_frame(),
            "the snapped scale must not request a frame"
        );
    }

    #[test]
    fn settled_opacity_flip_to_reduced_keeps_its_alpha_and_requests_no_frame() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let view: AnimatedOpacityView<()> = animated_opacity(0.4, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );

        // Settled at 0.4 from construction: no retarget is pending.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame(), "a settled opacity must not animate");

        // Flip reduce_motion on: the settled value holds and no frame is asked.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 10.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, alpha) = scene.layers[0];
        assert!(
            (alpha - 0.4).abs() < 1e-6,
            "the settled alpha must hold across the flip, got {alpha}"
        );
        assert!(
            !ctx.needs_frame(),
            "a flip on a settled opacity must not request a frame"
        );
        assert!(w.anim.driving_reduced, "the flip must still be recorded");

        // And it stays idle on later reduced paints.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 20.0);
        w.paint(&mut ctx, &mut scene);
        assert!(
            !ctx.needs_frame(),
            "a settled reduced opacity must stay idle"
        );
    }

    #[test]
    fn a_fade_settled_with_lerp_residue_stays_idle_across_a_reduce_flip() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        // 0.03 → 0.3: `0.03 + (0.3 − 0.03) × 1.0` is 0.30000000000000004, so
        // the landed value never `==` its target.
        let view: AnimatedOpacityView<()> = animated_opacity(0.03, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        let mut scene = RecordingScene::default();
        w.paint(&mut themed_ctx(&normal, size, 0.0), &mut scene);
        w.anim.set_target(0.3);
        let mut ms = 0.0;
        let mut rested = false;
        for _ in 0..600 {
            ms += 16.0;
            let mut scene = RecordingScene::default();
            let mut ctx = themed_ctx(&normal, size, ms);
            w.paint(&mut ctx, &mut scene);
            if !ctx.needs_frame() {
                rested = true;
                break;
            }
        }
        assert!(rested, "the fade must come to rest");
        assert_ne!(
            w.anim.value().to_bits(),
            0.3f64.to_bits(),
            "premise: the landed value carries lerp residue"
        );
        assert!(w.anim.settled(), "lerp residue must not count as in flight");

        // Flip reduce_motion on: nothing is in flight, so nothing re-drives.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, ms + 10.0);
        w.paint(&mut ctx, &mut scene);
        assert!(
            !ctx.needs_frame(),
            "a flip on a fade settled with lerp residue must not request a frame"
        );
        let (_, _, alpha) = scene.layers[0];
        assert!(
            (alpha - 0.3).abs() < 1e-6,
            "the landed alpha holds, got {alpha}"
        );
    }

    #[test]
    fn reduced_flip_mid_flight_redrives_the_opacity_from_its_current_value() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let view: AnimatedOpacityView<()> =
            animated_opacity(0.0, leaf_any(10.0, 10.0)).timing(timing);
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(1.0);

        // Normal 100 ms linear fade: seed at 0 ms, halfway at 50 ms.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 50.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, before) = scene.layers[0];
        assert!(
            (before - 0.5).abs() < 1e-3,
            "expected 0.5 halfway through the fade, got {before}"
        );

        // Flip reduce_motion on mid-fade: the value re-drives from 0.5 under the
        // 120 ms linear collapse, with no jump at the flip itself.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 50.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, seam) = scene.layers[0];
        assert!(
            (seam - before).abs() < 1e-6,
            "the flip must not jump the value: before {before}, after {seam}"
        );
        assert!(
            ctx.needs_frame(),
            "the re-driven collapse must be in flight"
        );

        // 60 ms into the 120 ms collapse the alpha is halfway from 0.5 to 1.0.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 110.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, mid) = scene.layers[0];
        assert!(
            (mid - 0.75).abs() < 1e-3,
            "expected 0.75 halfway through the re-driven collapse, got {mid}"
        );

        // Past 120 ms from the flip: settled on the target with no frame.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 180.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, end) = scene.layers[0];
        assert!(
            (end - 1.0).abs() < 1e-6,
            "expected the target 1.0, got {end}"
        );
        assert!(
            !ctx.needs_frame(),
            "the settled collapse must not request a frame"
        );
    }

    #[test]
    fn reduced_to_normal_flip_mid_collapse_continues_to_the_same_target() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let view: AnimatedOpacityView<()> = animated_opacity(1.0, leaf_any(10.0, 10.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.0);

        // The reduced 120 ms linear collapse: seed at 0 ms, halfway at 60 ms.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 60.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, before) = scene.layers[0];
        assert!(
            (before - 0.5).abs() < 1e-3,
            "expected 0.5 halfway through the collapse, got {before}"
        );

        // Flip reduce_motion off mid-collapse: the value continues from 0.5
        // with no jump, under the normal (spring) timing.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 60.0);
        w.paint(&mut ctx, &mut scene);
        let (_, _, seam) = scene.layers[0];
        assert!(
            (seam - before).abs() < 1e-6,
            "the flip must not jump the value: before {before}, after {seam}"
        );
        assert!(
            ctx.needs_frame(),
            "the normal-timing re-drive must be in flight"
        );

        // The spring runs on to the same target (0.0) and settles.
        let mut t = 60.0;
        let mut settled = false;
        for _ in 0..1000 {
            t += 1000.0 / 120.0;
            let mut scene = RecordingScene::default();
            let mut ctx = themed_ctx(&normal, size, t);
            w.paint(&mut ctx, &mut scene);
            if !ctx.needs_frame() {
                let (_, _, alpha) = scene.layers[0];
                assert!(
                    alpha.abs() < 1e-3,
                    "the re-driven fade must land on target 0.0, got {alpha}"
                );
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "the re-driven fade must settle within bounded frames"
        );
    }

    #[test]
    fn settled_reduced_scale_paints_build_no_driver_and_request_no_frame() {
        let normal = Theme::neutral();
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let view: AnimatedScaleView<()> = animated_scale(1.0, leaf_any(20.0, 20.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(50.0, 50.0)),
        );
        w.anim.set_target(0.62);

        // The first reduced paint lands on the target (the one rebuild).
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&reduced, size, 0.0);
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame(), "the snap must not request a frame");
        assert!(
            !w.anim.snap(),
            "a landed reduced scale has no driver to rebuild"
        );

        // Consecutive settled reduced paints keep the snapped transform and
        // rebuild nothing.
        for ms in [10.0, 20.0] {
            let mut scene = RecordingScene::default();
            let mut ctx = themed_ctx(&reduced, size, ms);
            w.paint(&mut ctx, &mut scene);
            assert!(!ctx.needs_frame(), "a settled reduced paint must be idle");
            assert!(!w.anim.snap(), "a settled reduced paint must not rebuild");
        }

        // A pending retarget still rebuilds.
        w.anim.set_target(0.5);
        assert!(w.anim.snap(), "a pending retarget must still rebuild");
        assert!((w.anim.value() - 0.5).abs() < 1e-9);

        // A flip back to normal while settled requests no frame either.
        let mut scene = RecordingScene::default();
        let mut ctx = themed_ctx(&normal, size, 30.0);
        w.paint(&mut ctx, &mut scene);
        assert!(
            !ctx.needs_frame(),
            "a flip on a settled scale must not request a frame"
        );
    }
}
