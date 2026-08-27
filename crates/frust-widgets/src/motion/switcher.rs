//! Keyed single-slot child-switch container: [`PatternSwitcher`], Flutter's
//! `PageTransitionSwitcher`/`AnimatedSwitcher` analog. It stages an
//! outgoing/incoming child pair under a
//! [`TransitionPattern`](super::patterns::TransitionPattern) whenever its
//! declared child changes *identity* (a [`ChildKey`](crate::ChildKey)).
//!
//! # Identity, not equality, drives the transition
//!
//! The switcher holds one child plus a caller-supplied `key`. On a rebuild
//! whose `key` is **unchanged**, the child reconciles in place like any
//! single-child wrapper — no transition (an every-frame rebuild is free). On a
//! **changed** `key`, the current child widget is *frozen* and retained as the
//! **exiting** child while a fresh **incoming** child is built in its place;
//! both then stage under the pattern, driven `0→1` off the frame clock
//! (advance-during-paint, `request_frame` — the crate's shared animation
//! contract). Same identity ⇒ same widget, state preserved; changed identity ⇒
//! a real subtree swap with an animated hand-off. This mirrors Flutter's
//! `PageTransitionSwitcher` "identity = type + key" rule.
//!
//! # Frozen exiting child: cancelled, never routed, dropped on settle
//!
//! While a transition runs the exiting child is inert: it keeps *painting* its
//! last state but is removed from the event path entirely — pointer routing
//! goes to the incoming child only ([`event`](PatternSwitcherWidget::event)).
//! At the instant it is frozen it receives the navigator's
//! **container-suppresses-routing** sequence verbatim (`docs/CODE_STANDARDS.md`):
//! a synthetic `Cancel` unwinds any in-flight capture, its focus flag is
//! cleared, and a cleared IME surface is published on the next paint — in that
//! order. It is retained until progress reaches `1.0`, then dropped on the next
//! rebuild; unlike the navigator (which retains a page *builder*), the switcher
//! holds no retained view for the frozen widget, so its teardown is a drop —
//! sound because [`frust_core::ComponentWidget`] disposes its reactive owner on
//! `Drop` defensively (`docs/CODE_STANDARDS.md`'s State & Reactivity rule).
//!
//! # Compositing: paint-only, route-to-incoming keeps input correct
//!
//! A resolved [`PatternLayer`](super::patterns::PatternLayer) is applied as a
//! pod-origin offset (so a slide's paint and hit-test move together), a
//! `push_layer` opacity, and a `push_transform` scale about the child's centre.
//! `push_layer`/`push_transform` are **paint-only** (no hit-test effect); it is
//! the route-to-incoming-only rule above — not the transforms — that keeps
//! input correct mid-transition.
//!
//! # Timing & reduced motion
//!
//! Timing defaults from the theme's [`MotionScheme`](frust_theme::MotionScheme)
//! (`durations.base` + `easing.effects`), overridable with
//! [`.timing(...)`](PatternSwitcherView::timing); with no theme threaded it
//! falls back to [`FALLBACK_DURATION`] + [`Curve::Emphasized`]. A
//! `reduce_motion` theme collapses *every* pattern to a fast
//! ([`REDUCE_MOTION_DURATION`]) linear [`FadeThrough`] crossfade — the same hard
//! accessibility rule `nav::transition::resolve_spec` enforces.
//!
//! # Scaffold contract
//!
//! This file owns its own contents only — it never edits `motion/mod.rs`'s
//! module list or re-export block (see that module's docs; the facade
//! re-exports `motion` wholesale, so these types ride along under
//! `frust::motion::*`).

use std::sync::atomic::Ordering;
use std::time::Duration;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, DiscardScene, EditingState,
    EventCtx, EventResult, ImeState, InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx,
    View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Point, Size};

use crate::ChildKey;
use crate::Timing;
use crate::motion::patterns::{FadeThrough, PatternLayer, TransitionPattern};
use crate::nav::navigator::NEXT_SNAPSHOT_KEY;
use crate::nav::transition::{TransitionDriver, make_driver};

/// The unthemed fallback transition duration (the Glyph `base` token, 220ms) —
/// used when no theme is threaded to resolve a `MotionScheme` from (bare-core
/// tests, pre-theme apps). See the [module docs](self)'s timing section.
pub const FALLBACK_DURATION: Duration = Duration::from_millis(220);

/// The reduced-motion collapse duration: `reduce_motion` flattens every pattern
/// to a `≤120ms` linear [`FadeThrough`] crossfade — the Glyph design system's
/// hard accessibility rule, matching `nav::transition`'s
/// [`REDUCE_MOTION_DURATION`](crate::nav::transition) collapse.
pub const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Resolve the effective [`Timing`] for a switch: an explicit builder value
/// always wins; otherwise the theme's `base` duration + `effects` easing,
/// falling back to [`FALLBACK_DURATION`] + [`Curve::Emphasized`] when no theme
/// is threaded.
fn resolve_switch_timing(theme: Option<&Theme>, explicit: Option<Timing>) -> Timing {
    explicit.unwrap_or_else(|| match theme {
        Some(theme) => Timing::Duration(
            Duration::from_secs_f64(theme.motion.durations.base / 1000.0),
            theme.motion.easing.effects,
        ),
        None => Timing::Duration(FALLBACK_DURATION, Curve::Emphasized),
    })
}

/// Paint one staged child: offset its pod origin by the layer's `dx`/`dy` (so
/// paint and hit-testing move together), then bracket its paint with a
/// snapshot bracket, keyed by `snapshot`, using `layer`'s alpha/scale — no
/// `push_transform`/`push_layer` of its own. Both the live and frozen exiting
/// child are static during a switch (only their position/alpha/scale move,
/// never their content), the same "ideal cache candidate" reasoning
/// `nav::navigator`'s `paint_page_layer` applies to a static transition page —
/// see [`PatternSwitcherWidget::snapshot_base`] for the two keys this bracket
/// draws from.
///
/// When `alpha == 0` the child is fully invisible this frame: rather than
/// rasterize it under a zero-opacity layer, it paints into a [`DiscardScene`]
/// sink instead — the pass still has to run for its side effects (animating
/// descendants advancing), but nothing it records is ever composited, so no
/// bracket is needed at all. Mirrors `nav::navigator`'s `paint_page_layer`,
/// whose `DiscardScene` check also runs first, ahead of its own bracket.
fn paint_staged_child(
    pod: &mut ChildPod,
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    layer: PatternLayer,
    snapshot: u64,
) {
    pod.set_origin(Point::new(layer.dx, layer.dy));
    let alpha = layer.alpha.clamp(0.0, 1.0);

    if alpha <= 0.0 {
        let mut sink = DiscardScene;
        pod.paint_child(ctx, &mut sink);
        return;
    }

    scene.push_snapshot(snapshot, ctx.origin(), ctx.size(), alpha, layer.scale);
    pod.paint_child(ctx, scene);
    scene.pop_snapshot();
}

/// The cleared/inactive IME surface published after a switch so the platform
/// keyboard hides deterministically (mirrors `nav::navigator`'s
/// `cleared_ime_state`).
fn cleared_ime_state() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
        content_type: Default::default(),
    }
}

/// A keyed single-slot child-switch container. See the [module docs](self).
pub struct PatternSwitcherView<State: 'static, P: TransitionPattern + Clone + 'static> {
    key: ChildKey,
    pattern: P,
    timing: Option<Timing>,
    reverse: bool,
    child: AnyView<State>,
}

/// Stage a keyed child switch under `pattern`: whenever `key` changes identity,
/// the previous child animates out and `child` animates in. See the
/// [module docs](self).
pub fn pattern_switcher<State, P, V>(
    key: impl Into<ChildKey>,
    pattern: P,
    child: V,
) -> PatternSwitcherView<State, P>
where
    State: 'static,
    P: TransitionPattern + Clone + 'static,
    V: View<State>,
{
    PatternSwitcherView {
        key: key.into(),
        pattern,
        timing: None,
        reverse: false,
        child: any(child),
    }
}

/// PascalCase alias for [`pattern_switcher`] (mirrors the `AnimatedOpacity`/
/// `animated_opacity` dual naming).
#[allow(non_snake_case)]
pub fn PatternSwitcher<State, P, V>(
    key: impl Into<ChildKey>,
    pattern: P,
    child: V,
) -> PatternSwitcherView<State, P>
where
    State: 'static,
    P: TransitionPattern + Clone + 'static,
    V: View<State>,
{
    pattern_switcher(key, pattern, child)
}

impl<State: 'static, P: TransitionPattern + Clone + 'static> PatternSwitcherView<State, P> {
    /// Override the default (theme-resolved) transition timing.
    pub fn timing(mut self, timing: Timing) -> Self {
        self.timing = Some(timing);
        self
    }

    /// Flip the pattern's directional motion (a "back"/reverse switch). Has no
    /// effect on the non-directional patterns ([`FadeThrough`]/`FadeScale`).
    pub fn reverse(mut self, reverse: bool) -> Self {
        self.reverse = reverse;
        self
    }
}

/// The retained widget for a [`PatternSwitcherView`]. Generic only over the
/// pattern `P` (the frozen exiting child is retained as a bare
/// [`ChildPod`](frust_core::ChildPod), so no `State` is captured — see the
/// [module docs](self)'s teardown note).
pub struct PatternSwitcherWidget<P: TransitionPattern + Clone + 'static> {
    key: ChildKey,
    child: ChildPod,
    /// The frozen outgoing child, retained (painting its last state) until the
    /// transition settles, then dropped.
    exiting: Option<ChildPod>,
    pattern: P,
    explicit_timing: Option<Timing>,
    reverse: bool,
    /// The `0→1` progress driver, created lazily on the first paint of a
    /// transition (a `BuildCtx` has no theme to resolve timing/`reduce_motion`
    /// from — mirrors `motion::animated`'s deferred retarget).
    driver: Option<TransitionDriver>,
    /// A transition was staged by the last rebuild; the next paint resolves
    /// timing and creates [`Self::driver`].
    pending_start: bool,
    /// The driver reached rest this paint; the next rebuild finalizes (drops the
    /// exiting child).
    settled: bool,
    /// The active transition collapsed to a `reduce_motion` crossfade (resolved
    /// once at start, stable for the whole transition).
    reduce_motion: bool,
    /// A cleared IME surface must be published on the next paint (deterministic
    /// keyboard hide after the switch).
    needs_ime_clear: bool,
    /// This switcher's pair of [`PaintScene::push_snapshot`] cache keys,
    /// reserved once from [`NEXT_SNAPSHOT_KEY`] at construction (`fetch_add(2)`)
    /// and stable for the widget's lifetime, across every switch it ever
    /// stages: `snapshot_base` for the live (incoming) child,
    /// `snapshot_base + 1` for the frozen exiting pod. A single-slot container
    /// has no slot index to key by (unlike the navigator's per-page keys), but
    /// still needs two distinct keys since both children can be on screen
    /// staged together — reusing one key for both would alias their cached
    /// rasters.
    snapshot_base: u64,
}

impl<State: 'static, P: TransitionPattern + Clone + 'static> View<State>
    for PatternSwitcherView<State, P>
{
    type Element = PatternSwitcherWidget<P>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PatternSwitcherWidget<P> {
        PatternSwitcherWidget {
            key: self.key,
            child: crate::authoring::build_child(&self.child, ctx),
            exiting: None,
            pattern: self.pattern.clone(),
            explicit_timing: self.timing,
            reverse: self.reverse,
            driver: None,
            pending_start: false,
            settled: false,
            reduce_motion: false,
            needs_ime_clear: false,
            snapshot_base: NEXT_SNAPSHOT_KEY.fetch_add(2, Ordering::Relaxed),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PatternSwitcherWidget<P>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        // Pattern/timing/reverse are paint-time inputs; refresh them (a pattern
        // swap mid-flight is unusual but harmless — the running driver is
        // untouched). Only flag PAINT when a value actually changed.
        element.pattern = self.pattern.clone();
        if element.explicit_timing != self.timing {
            element.explicit_timing = self.timing;
            flags |= ChangeFlags::PAINT;
        }
        if element.reverse != self.reverse {
            element.reverse = self.reverse;
            flags |= ChangeFlags::PAINT;
        }

        // Finalize a just-settled transition (paint flagged `settled` and
        // requested this frame): drop the frozen exiting child. `Option::take`
        // via reassignment runs `ComponentWidget`'s defensive `Drop`.
        if element.settled {
            element.exiting = None;
            element.driver = None;
            element.settled = false;
        }

        if self.key != element.key {
            // --- Identity change: stage a transition. ---
            // A new switch supersedes any still-running one (drop its exiting).
            element.exiting = None;
            element.driver = None;
            element.reduce_motion = false;

            // Freeze the current incoming child as the exiting child; build the
            // new incoming child fresh in its place.
            let mut old = std::mem::replace(
                &mut element.child,
                crate::authoring::build_child(&self.child, ctx),
            );

            // Container-suppresses-routing contract (docs/CODE_STANDARDS.md):
            // synthetic Cancel -> focus clear -> cleared-IME publish, in order.
            if old.is_active() {
                crate::authoring::cancel_pod(&mut old);
                old.set_active(false);
            }
            if old.is_focused() {
                old.set_focused(false);
                // The cleared-IME publish is the third step of that contract, and
                // it belongs INSIDE this arm: publishing an inactive surface is a
                // full focus/IME **session release** at the root, not a value
                // update (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle), and
                // it bubbles last-write-wins. A switcher whose own child never
                // held focus has no session to end — raising the flag anyway
                // would release whatever *unrelated* subtree does own one (a
                // field painted earlier in the same frame), with no self-heal.
                //
                // Gated on the same composition every severing site in the crate
                // uses (`frust-widgets`' `mark_orphan_if_live`): the outgoing
                // pod's own link ANDed with the rebuild-pass chain down to this
                // switcher, so a *stale* flag under an already-blurred ancestor
                // publishes nothing either. `ctx.has_focus()` is unaffected by the
                // `build_child` above — that descent restores the chain on the
                // way out.
                if ctx.has_focus() {
                    element.needs_ime_clear = true;
                }
            }

            element.exiting = Some(old);
            element.key = self.key;
            element.pending_start = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // --- Same identity: an ordinary in-place reconcile, no transition. ---
            flags |=
                crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        }

        flags
    }

    fn teardown(&self, element: &mut PatternSwitcherWidget<P>, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
        // The frozen exiting child has no retained view to run `View::teardown`
        // against; dropping it disposes any Component owner via the defensive
        // `Drop` (module docs).
        element.exiting = None;
    }
}

impl<P: TransitionPattern + Clone + 'static> Widget for PatternSwitcherWidget<P> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        // Lay out the frozen exiting child too (under the same constraints) so it
        // has a valid size to paint at during the transition. Its origin is set
        // per-frame in `paint` from the pattern layer.
        if let Some(exiting) = &mut self.exiting {
            exiting.layout_child(ctx, bc);
            exiting.set_origin(Point::ZERO);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Lazily create the driver now a theme is in scope (reduce_motion +
        // timing resolution), mirroring `motion::animated`'s deferred retarget.
        if self.pending_start {
            let theme = Theme::from_paint_ctx(ctx);
            let reduce = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
            let timing = if reduce {
                Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
            } else {
                resolve_switch_timing(theme, self.explicit_timing)
            };
            self.driver = Some(make_driver(timing).0);
            self.reduce_motion = reduce;
            self.pending_start = false;
        }

        if let (Some(exiting), Some(driver)) = (self.exiting.as_mut(), self.driver.as_mut()) {
            let adv = driver.advance(ctx.frame_time());
            let size = ctx.size();
            // reduce_motion collapses any pattern to a FadeThrough crossfade.
            let (incoming_layer, exiting_layer) = if self.reduce_motion {
                FadeThrough.resolve(adv.value, self.reverse, size)
            } else {
                self.pattern.resolve(adv.value, self.reverse, size)
            };
            // Paint the exiting child (below) then the incoming child (on top).
            // `snapshot_base` keys the live child; `snapshot_base + 1` the
            // frozen exiting pod — see `Self::snapshot_base`.
            paint_staged_child(exiting, ctx, scene, exiting_layer, self.snapshot_base + 1);
            paint_staged_child(
                &mut self.child,
                ctx,
                scene,
                incoming_layer,
                self.snapshot_base,
            );

            if adv.animating {
                ctx.request_frame();
            }
            if adv.done {
                // Settle: finalize on the next rebuild; request one more frame so
                // that rebuild runs (mirrors the navigator's settle contract).
                self.settled = true;
                ctx.request_frame();
            }
        } else {
            self.child.paint_child(ctx, scene);
        }

        // Deterministic IME hide after a switch (navigator contract). Publishing
        // a cleared surface bubbles up through `ChildPod::paint_child`.
        if self.needs_ime_clear {
            ctx.publish_ime_state(cleared_ime_state());
            self.needs_ime_clear = false;
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Route only to the incoming child; the frozen exiting child left the
        // event path at transition start (synthetically cancelled) — module docs.
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Forward the incoming (destination) child; the transient exiting child
        // is mid-removal and is not reported.
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(exiting, child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::patterns::{FadeScale, SharedAxis};
    use crate::test_support::{RecordingScene, leaf_any};
    use crate::{Column, FlexView};
    use frust_core::{FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot};
    use kurbo::Rect;
    use std::cell::Cell;
    use std::rc::Rc;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn loose() -> BoxConstraints {
        BoxConstraints::loose(Size::new(100.0, 100.0))
    }

    /// A recording scene that separates `push_snapshot`/`pop_snapshot` calls
    /// from `push_layer`/`push_transform` ones. `crate::test_support`'s shared
    /// `RecordingScene` doesn't override `push_snapshot`, so it would fold a
    /// snapshot bracket into the trait's emulating default — a `push_transform`
    /// and `push_layer` pair — indistinguishable from a plain bracket. This one
    /// keeps them apart so a test can assert a staged child painted through
    /// the snapshot bracket specifically, and through no other bracket.
    #[derive(Default)]
    struct SnapshotRecordingScene {
        rects: Vec<(Point, Size)>,
        layers: Vec<(Point, Size, f32)>,
        transform_pushes: u32,
        snapshots: Vec<(u64, Point, Size, f32, f64)>,
        snapshot_pops: u32,
    }
    impl PaintScene for SnapshotRecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn push_transform(&mut self, _transform: kurbo::Affine) {
            self.transform_pushes += 1;
        }
        fn push_snapshot(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
            self.snapshots.push((key, origin, size, alpha, scale));
        }
        fn pop_snapshot(&mut self) {
            self.snapshot_pops += 1;
        }
    }

    /// An interactive child that captures the pointer + takes focus on `Down`,
    /// records a synthetic `Cancel` into a shared cell, and records its own
    /// teardown (a drop) into a shared counter — the recording interactive child
    /// the tests below assert against (mirrors the navigator's recording tests).
    struct Recorder {
        cancelled: Rc<Cell<bool>>,
        torn: Rc<Cell<u32>>,
    }
    struct RecorderWidget {
        cancelled: Rc<Cell<bool>>,
        torn: Rc<Cell<u32>>,
    }
    impl View<()> for Recorder {
        type Element = RecorderWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RecorderWidget {
            RecorderWidget {
                cancelled: self.cancelled.clone(),
                torn: self.torn.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut RecorderWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for RecorderWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        ctx.request_focus();
                        return EventResult::Handled;
                    }
                    // A Cancel arm only clears internal flags (never app state) —
                    // recording into an owned `Rc<Cell>` is an internal flag.
                    PointerPhase::Cancel => {
                        self.cancelled.set(true);
                        return EventResult::Handled;
                    }
                    _ => {}
                }
            }
            EventResult::Ignored
        }
    }
    impl Drop for RecorderWidget {
        fn drop(&mut self) {
            self.torn.set(self.torn.get() + 1);
        }
    }

    fn recorder(cancelled: &Rc<Cell<bool>>, torn: &Rc<Cell<u32>>) -> Recorder {
        Recorder {
            cancelled: cancelled.clone(),
            torn: torn.clone(),
        }
    }

    fn reduced_motion_theme() -> Theme {
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;
        theme
    }

    // --- Identity change transitions; same identity doesn't ---

    #[test]
    fn same_identity_rebuild_does_not_transition_but_changed_identity_does() {
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, leaf_any(10.0, 10.0));
        let mut w = build(&v1);
        assert!(w.exiting.is_none());

        // Same key: an ordinary reconcile, no transition staged.
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, leaf_any(20.0, 20.0));
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            w.exiting.is_none(),
            "same identity must not stage a transition"
        );
        assert!(!w.pending_start);

        // Changed key: the old child is frozen as the exiting child.
        let v3: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(30.0, 30.0));
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            w.exiting.is_some(),
            "an identity change must retain the exiting child"
        );
        assert!(w.pending_start, "the next paint starts the driver");
        assert_eq!(w.key, 2u32.into());
    }

    // --- Exiting child gets Cancel + focus/IME clear ---

    #[test]
    fn identity_change_cancels_focus_and_clears_ime_on_the_exiting_child() {
        let cancelled = Rc::new(Cell::new(false));
        let torn = Rc::new(Cell::new(0u32));
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, any(recorder(&cancelled, &torn)));
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        // Arm the child: a Down captures the pointer and takes focus.
        let mut st = ();
        let mut ectx = EventCtx::new(&mut st, Point::ZERO, size);
        w.event(&mut ectx, &down(5.0, 5.0));
        assert!(w.child.is_active(), "the child captured the pointer");
        assert!(w.child.is_focused(), "the child took focus");

        // Switch identity: the armed child becomes the exiting child and gets the
        // capture-cancel -> focus-clear -> IME-clear sequence.
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0));
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        assert!(
            cancelled.get(),
            "the exiting child received a synthetic Cancel"
        );
        let exiting = w.exiting.as_ref().expect("exiting child retained");
        assert!(
            !exiting.is_focused(),
            "the exiting child's focus was cleared"
        );
        assert!(!exiting.is_active(), "its capture path was released");
        assert!(
            w.needs_ime_clear,
            "an IME clear is queued for the next paint"
        );

        // The paint after the switch publishes the cleared IME surface.
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut pctx, &mut scene);
        let ime = pctx
            .take_ime_state()
            .expect("a cleared IME surface was published on the post-switch paint");
        assert!(!ime.active);
        assert!(ime.editing.text.is_empty());
        assert!(!w.needs_ime_clear, "the queued clear was consumed");
    }

    // --- Cross-subtree survival: a switcher whose OWN child never held focus
    //     must not release somebody else's live session (review-fix-3, FC). ---

    /// The IME surface the sibling field owns for the whole test below.
    fn field_surface() -> ImeState {
        ImeState {
            active: true,
            editing: EditingState {
                text: "query".to_string(),
                selection_base: 5,
                selection_extent: 5,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
            content_type: Default::default(),
        }
    }

    /// A **persistent** field-shaped leaf: it claims focus and publishes on
    /// `Down`, and republishes the same surface on every paint while it still
    /// holds focus — a real `TextInput`'s behavior, and the reason paint order
    /// (field first, switcher second) decides the last write. Bounded size so it
    /// can sit as an inflexible child on a `Column`'s unbounded main axis.
    struct PersistentField {
        size: Size,
    }
    struct PersistentFieldWidget {
        size: Size,
    }
    impl<S: 'static> View<S> for PersistentField {
        type Element = PersistentFieldWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PersistentFieldWidget {
            PersistentFieldWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PersistentFieldWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for PersistentFieldWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            if ctx.has_focus() {
                ctx.publish_ime_state(field_surface());
            }
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.request_focus();
                ctx.publish_ime_state(field_surface());
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn identity_change_in_an_unfocused_switcher_leaves_a_sibling_fields_session_alive() {
        // A persistent field as the EARLIER sibling of an unrelated switcher, so
        // the field paints FIRST and anything the switcher published would win
        // last-write-wins on the way to the root. Rows are 40 tall: the field
        // owns `y ∈ [0, 40)`, the switcher `y ∈ [40, 80)`.
        let key = Rc::new(Cell::new(1u32));
        let mut app = {
            let key = key.clone();
            move |_: &mut ()| {
                Column(vec![
                    any(PersistentField {
                        size: Size::new(100.0, 40.0),
                    }),
                    any(pattern_switcher(
                        key.get(),
                        FadeThrough,
                        leaf_any(100.0, 40.0),
                    )),
                ])
            }
        };
        let mut root: RenderRoot<(), FlexView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        // Tap inside the field's row — never the switcher's, which would blur the
        // field on the way in (blur-on-outside-tap) and hide the defect.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(
            root.is_focus_active(),
            "the sibling field owns the focus session"
        );
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("query".to_string()),
            "…and the shell-facing surface is the field's"
        );
        let generation = root.focus_ime_generation();

        // The switcher's identity changes. Its own child never held focus, so
        // there is no session here to end.
        key.set(2);
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        assert!(
            root.is_focus_active(),
            "an identity change in an unfocused switcher must not release the \
             sibling field's session"
        );
        let ime = root
            .ime_state()
            .expect("the identity change dropped the sibling field's IME surface entirely");
        assert!(ime.active, "the field's surface was left inactive");
        assert_eq!(
            ime.editing.text, "query",
            "the field's surface was replaced with the switcher's cleared one"
        );
        assert_eq!(
            root.focus_ime_generation(),
            generation,
            "an identity change in an unfocused switcher is not a focus/IME edge"
        );
    }

    // --- Exiting retained until progress 1.0, then torn down ---

    #[test]
    fn exiting_child_is_retained_until_settle_then_torn_down() {
        let cancelled = Rc::new(Cell::new(false));
        let torn = Rc::new(Cell::new(0u32));
        let short = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, any(recorder(&cancelled, &torn))).timing(short);
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        // Switch identity -> transition begins; exiting retained, not torn down.
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_some());
        assert_eq!(
            torn.get(),
            0,
            "exiting not torn down while the transition runs"
        );

        // First paint creates + seeds the driver (still mid-flight).
        w.paint(
            &mut PaintCtx::new(Point::ZERO, size),
            &mut RecordingScene::default(),
        );
        assert!(w.exiting.is_some(), "retained during the transition");
        assert_eq!(torn.get(), 0);
        assert!(!w.settled);

        // Drive the retained driver to rest directly (PaintCtx::set_frame_time is
        // crate-private to frust-core, mirroring the animated.rs tests).
        w.driver
            .as_mut()
            .expect("driver created on first paint")
            .advance(ft_ms(10_000.0));

        // The next paint observes the settled driver and flags finalize.
        w.paint(
            &mut PaintCtx::new(Point::ZERO, size),
            &mut RecordingScene::default(),
        );
        assert!(w.settled, "paint flags settle once the driver reaches rest");
        assert!(
            w.exiting.is_some(),
            "still retained until the finalize rebuild"
        );
        assert_eq!(torn.get(), 0);

        // The finalize rebuild (same identity, key 2) tears the exiting child down.
        let v3: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_none(), "exiting dropped after settle");
        assert_eq!(
            torn.get(),
            1,
            "exiting torn down exactly once, after progress 1.0"
        );
        assert!(!w.settled);
        assert!(w.driver.is_none());
    }

    // --- reduce_motion collapses to a fast crossfade ---

    #[test]
    fn reduce_motion_collapses_to_fast_crossfade() {
        let theme = reduced_motion_theme();
        // Configure a *directional* pattern (SharedAxis::X slides the incoming
        // child by 30dp) so the collapse to a non-directional crossfade is
        // observable in the child's pod origin.
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, SharedAxis::X, leaf_any(10.0, 10.0));
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, SharedAxis::X, leaf_any(10.0, 10.0));
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut RecordingScene::default());

        assert!(
            w.reduce_motion,
            "reduce_motion must be resolved from the threaded theme"
        );
        // FadeThrough collapse: the incoming child never slides (dx stays 0),
        // unlike the configured SharedAxis::X (which offsets it by 30dp at p=0).
        assert_eq!(
            w.child.origin(),
            Point::ZERO,
            "reduce_motion collapses to a non-directional crossfade (no slide)"
        );
    }

    #[test]
    fn without_reduce_motion_the_configured_directional_pattern_slides() {
        // The contrast case for the reduce_motion test above: an untheme'd (no
        // reduce_motion) SharedAxis::X switch DOES offset the incoming child.
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, SharedAxis::X, leaf_any(10.0, 10.0));
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, SharedAxis::X, leaf_any(10.0, 10.0));
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        w.paint(
            &mut PaintCtx::new(Point::ZERO, size),
            &mut RecordingScene::default(),
        );
        assert!(!w.reduce_motion);
        // At the seed frame (p=0) SharedAxis::X offsets the incoming child by the
        // full 30dp slide.
        assert_eq!(w.child.origin().x, 30.0);
    }

    // --- No-transition steady state paints the child once, plainly ---

    #[test]
    fn steady_state_paints_the_single_child_with_no_layer_or_transform() {
        let v: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, leaf_any(10.0, 10.0));
        let mut w = build(&v);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        let mut scene = RecordingScene::default();
        w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut scene);
        assert_eq!(scene.rects.len(), 1, "the single child paints its fill");
        assert!(
            scene.layers.is_empty(),
            "no crossfade layer at steady state"
        );
        assert!(
            scene.transforms.is_empty(),
            "no scale transform at steady state"
        );
    }

    // --- Discard: a staged child at alpha 0 paints into a DiscardScene sink,
    //     recording no ops and no zero-alpha layer (mirrors nav::navigator's
    //     paint_page_layer discard branch). ---

    #[test]
    fn alpha_zero_staged_child_records_no_ops_and_no_zero_alpha_layer() {
        let leaf = leaf_any(10.0, 10.0);
        let mut counter = 0u64;
        let mut pod = crate::authoring::build_child(&leaf, &mut BuildCtx::new(&mut counter));
        let size = pod.layout_child(&mut LayoutCtx::new(), &loose());
        pod.set_origin(Point::ZERO);

        let mut scene = RecordingScene::default();
        let layer = PatternLayer {
            dx: 0.0,
            dy: 0.0,
            alpha: 0.0,
            scale: 1.0,
        };
        paint_staged_child(
            &mut pod,
            &mut PaintCtx::new(Point::ZERO, size),
            &mut scene,
            layer,
            1,
        );

        assert!(scene.rects.is_empty(), "an alpha-0 child records no fill");
        assert!(
            scene.layers.is_empty(),
            "no zero-alpha push_layer is recorded either"
        );
    }

    // --- Snapshot bracket: a staged child paints through push_snapshot/
    //     pop_snapshot keyed by snapshot_base (incoming) / snapshot_base + 1
    //     (exiting), never a push_layer/push_transform bracket of its own —
    //     mirrors nav::navigator's paint_page_layer snapshot bracket. ---

    #[test]
    fn staged_children_paint_through_a_snapshot_bracket_keyed_by_snapshot_base() {
        // FadeScale (unlike FadeThrough/SharedAxis's hard split) fades both
        // children simultaneously over its first 30%, so a mid-window frame
        // has both the incoming and the frozen exiting child at alpha > 0 at
        // once — the case that proves each gets its OWN key.
        let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeScale, leaf_any(10.0, 10.0)).timing(timing);
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeScale, leaf_any(10.0, 10.0)).timing(timing);
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        // Seed frame: the first `advance` only seeds the clock (progress 0).
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)),
            &mut SnapshotRecordingScene::default(),
        );

        // 15ms into the 100ms linear duration: progress 0.15, inside
        // FadeScale's [0, 0.30] simultaneous-fade window.
        let base = w.snapshot_base;
        let mut scene = SnapshotRecordingScene::default();
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, size, ft_ms(15.0)),
            &mut scene,
        );

        assert_eq!(
            scene.snapshots.len(),
            2,
            "both the incoming and frozen exiting child paint through a \
             snapshot bracket: {:?}",
            scene.snapshots
        );
        assert_eq!(scene.snapshot_pops, 2, "each bracket pops exactly once");
        assert!(
            scene.layers.is_empty(),
            "no push_layer bracket of its own: {:?}",
            scene.layers
        );
        assert_eq!(
            scene.transform_pushes, 0,
            "no push_transform bracket of its own"
        );

        let keys: Vec<u64> = scene.snapshots.iter().map(|(k, ..)| *k).collect();
        assert!(
            keys.contains(&base),
            "the incoming child is keyed by snapshot_base: {keys:?}"
        );
        assert!(
            keys.contains(&(base + 1)),
            "the frozen exiting child is keyed by snapshot_base + 1: {keys:?}"
        );
        for (_, _, _, alpha, _) in &scene.snapshots {
            assert!(*alpha > 0.0, "an alpha-0 child never reaches the bracket");
        }
    }

    // --- snapshot_base is reserved once, at construction, and outlives every
    //     switch this widget instance stages. ---

    #[test]
    fn snapshot_base_is_stable_across_multiple_switches_on_the_same_widget() {
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, leaf_any(10.0, 10.0));
        let mut w = build(&v1);
        let base = w.snapshot_base;
        let mut counter = 0u64;

        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0));
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            w.snapshot_base, base,
            "a switch does not reassign the key pair"
        );

        let v3: PatternSwitcherView<(), _> =
            pattern_switcher(3u32, FadeThrough, leaf_any(10.0, 10.0));
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            w.snapshot_base, base,
            "a second switch on the same widget still keeps the same key pair"
        );
    }

    // --- Two switcher instances never alias each other's cache slot. ---

    #[test]
    fn two_switcher_instances_get_disjoint_snapshot_key_pairs() {
        let v: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, leaf_any(10.0, 10.0));
        let w1 = build(&v);
        let w2 = build(&v);
        assert_ne!(
            w1.snapshot_base, w2.snapshot_base,
            "two switcher instances never share a snapshot key pair"
        );
    }
}
