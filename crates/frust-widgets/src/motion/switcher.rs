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
//! # Frozen exiting child: cancelled, torn down at the switch, only pixels survive
//!
//! While a transition runs the exiting child is inert: it keeps *painting* its
//! last state but is removed from the event path entirely — pointer routing
//! goes to the incoming child only ([`event`](PatternSwitcherWidget::event)).
//! At the instant it is frozen it receives the navigator's
//! **container-suppresses-routing** sequence verbatim (`docs/CODE_STANDARDS.md`):
//! a synthetic `Cancel` unwinds any in-flight capture, its focus flag is
//! cleared, and a cleared IME surface is published on the next paint — in that
//! order.
//!
//! **Then, in the same rebuild, the outgoing subtree is torn down**
//! (`View::teardown` through the view that last built it). From that point it
//! is logically out of the retained tree: every navigator under it reports
//! `is_mounted() == false` (and so claims no back press), every back
//! registrant's liveness count drops, and every
//! [`Component`](frust_core::Component) disposes its reactive owner — on the
//! very frame the key changes, not when the exit animation ends. What the
//! switcher keeps is only the torn-down widget tree's **pixels**: the pod is
//! laid out and painted until progress reaches `1.0`, never rebuilt and never
//! routed, then dropped on the next rebuild (or when a newer switch supersedes
//! it, or in the switcher's own teardown) — a plain drop each time, never a
//! second teardown.
//!
//! The teardown has to happen at the freeze because that is the only point at
//! which both halves are in hand: the old view (`prev.child`, gone after this
//! rebuild — an [`AnyView`] is not `Clone`, and the widget is not generic over
//! `State`, so it cannot be stashed until settle) and the old pod. A bare drop
//! at settle would skip `View::teardown` entirely, so a navigator or a back
//! fallback under a switched-away section would stay "mounted" forever and
//! keep swallowing back presses everywhere else in the app.
//!
//! Painting after teardown is sound for the widgets a switcher hosts: a
//! navigator's teardown finalizes any in-flight page transition and tears its
//! pages down but keeps their pods, which it paints like any static stack; a
//! [`frust_core::ComponentWidget`]'s paint is a pass-through to its pod that
//! never touches its (now disposed) owner, and its defensive `Drop` disposal is
//! idempotent; a plain leaf's teardown is a no-op. The focus contract above has
//! already cleared the pod's focus link before the teardown runs, so the
//! teardown's own focus-orphan gate sees no live link and releases nothing a
//! second time.
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
use kurbo::{Point, Size, Vec2};

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

    let bracket_origin = ctx.origin() + Vec2::new(layer.dx, layer.dy);
    scene.push_retained(snapshot, bracket_origin, ctx.size(), alpha, layer.scale);
    pod.paint_child(ctx, scene);
    scene.pop_retained();
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
        suppress_soft_keyboard: false,
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
/// [`ChildPod`](frust_core::ChildPod), so no `State` is captured — which is why
/// it is torn down at the switch; see the [module docs](self)'s frozen-child
/// section).
pub struct PatternSwitcherWidget<P: TransitionPattern + Clone + 'static> {
    key: ChildKey,
    child: ChildPod,
    /// The frozen outgoing child: already torn down at the switch, retained
    /// only to paint its last state until the transition settles, then
    /// dropped (never torn down a second time).
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
    /// This switcher's pair of [`PaintScene::push_retained`] cache keys,
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
        // requested this frame): drop the frozen exiting child. It was already
        // torn down at the switch, so this is a plain drop — never a second
        // teardown (a `ComponentWidget`'s `Drop` disposal is idempotent).
        if element.settled {
            element.exiting = None;
            element.driver = None;
            element.settled = false;
        }

        if self.key != element.key {
            // --- Identity change: stage a transition. ---
            // A new switch supersedes any still-running one: drop its exiting
            // pod, which was torn down when IT was frozen — a plain drop here.
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

            // Tear the outgoing subtree down NOW, through the view that last
            // built it — the only point where `prev.child` and the old pod are
            // both in hand (module docs). Navigators under it unmount and stop
            // claiming back, back registrants drop their liveness, components
            // dispose their owners. The pod is kept only to paint its pixels
            // until settle. The capture/focus steps above already ran, so
            // `teardown_child` finds the pod neither active (no second
            // `Cancel`) nor focused (its orphan gate sees no live link, and the
            // descent runs under a cleared chain, so nothing below re-releases
            // the session the IME clear above already ends).
            crate::authoring::teardown_child(&prev.child, &mut old, ctx);

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
        // The frozen exiting child (if a transition is still running) was torn
        // down at the switch that froze it; only its pixels were left, so a
        // plain drop finishes it — never a second teardown (module docs).
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
    use crate::{FlexView, column};
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

    /// A recording scene that separates `push_retained`/`pop_retained` calls
    /// from `push_layer`/`push_transform` ones. `crate::test_support`'s shared
    /// `RecordingScene` doesn't override `push_retained`, so it would fold a
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
        fn push_retained(&mut self, key: u64, origin: Point, size: Size, alpha: f32, scale: f64) {
            self.snapshots.push((key, origin, size, alpha, scale));
        }
        fn pop_retained(&mut self) {
            self.snapshot_pops += 1;
        }
    }

    /// An interactive child that captures the pointer + takes focus on `Down`,
    /// records a synthetic `Cancel` into a shared cell, records its widget's
    /// drop into `torn`, and its `View::teardown` into `teardowns` — the
    /// recording interactive child the tests below assert against (mirrors the
    /// navigator's recording tests). The two counters are kept apart on
    /// purpose: a frozen exiting child is torn down at the switch but dropped
    /// only at settle.
    struct Recorder {
        cancelled: Rc<Cell<bool>>,
        torn: Rc<Cell<u32>>,
        teardowns: Rc<Cell<u32>>,
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
        fn teardown(&self, _element: &mut RecorderWidget, _ctx: &mut BuildCtx<'_>) {
            self.teardowns.set(self.teardowns.get() + 1);
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
            teardowns: Rc::new(Cell::new(0)),
        }
    }

    /// A [`Recorder`] whose `View::teardown` count is observable too.
    fn counting_recorder(torn: &Rc<Cell<u32>>, teardowns: &Rc<Cell<u32>>) -> Recorder {
        Recorder {
            cancelled: Rc::new(Cell::new(false)),
            torn: torn.clone(),
            teardowns: teardowns.clone(),
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
            pattern_switcher(1u32, FadeThrough, recorder(&cancelled, &torn));
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

    // --- The freeze-time teardown releases no session a second time: the
    //     focus link is cleared before `teardown_child` runs, so its orphan
    //     gate sees nothing live; the IME clear stays the one release. ---

    #[test]
    fn freeze_teardown_of_a_focused_child_raises_no_second_focus_release() {
        let cancelled = Rc::new(Cell::new(false));
        let torn = Rc::new(Cell::new(0u32));
        let teardowns = Rc::new(Cell::new(0u32));
        let v1: PatternSwitcherView<(), _> = pattern_switcher(
            1u32,
            FadeThrough,
            Recorder {
                cancelled: cancelled.clone(),
                torn: torn.clone(),
                teardowns: teardowns.clone(),
            },
        );
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());
        let mut st = ();
        let mut ectx = EventCtx::new(&mut st, Point::ZERO, size);
        w.event(&mut ectx, &down(5.0, 5.0));
        assert!(w.child.is_active() && w.child.is_focused());

        let _ = frust_core::take_focus_orphaned();
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0));
        let mut counter = 0u64;
        // `BuildCtx::new` seeds a live chain: the focused pod WAS on it.
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        assert_eq!(
            teardowns.get(),
            1,
            "the focused child was torn down at the switch"
        );
        assert!(cancelled.get(), "cancelled exactly as before");
        assert!(w.needs_ime_clear, "the IME clear is the session release");
        assert!(
            !frust_core::take_focus_orphaned(),
            "the teardown must not raise a second (orphan) release"
        );
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
            suppress_soft_keyboard: false,
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
                column()
                    .child(PersistentField {
                        size: Size::new(100.0, 40.0),
                    })
                    .child(pattern_switcher(
                        key.get(),
                        FadeThrough,
                        leaf_any(100.0, 40.0),
                    ))
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

    // --- Exiting torn down at the switch, its pod retained (painting) until
    //     progress 1.0, then dropped — never torn down a second time. ---

    #[test]
    fn exiting_child_is_torn_down_at_the_switch_and_dropped_after_settle() {
        let torn = Rc::new(Cell::new(0u32));
        let teardowns = Rc::new(Cell::new(0u32));
        let short = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, counting_recorder(&torn, &teardowns)).timing(short);
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        // Switch identity -> transition begins. The outgoing subtree is torn
        // down on this very rebuild; its pod is retained (not dropped).
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_some());
        assert_eq!(teardowns.get(), 1, "torn down at the switch, exactly once");
        assert_eq!(
            torn.get(),
            0,
            "its pod is not dropped while the transition runs"
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

        // The finalize rebuild (same identity, key 2) drops the exiting pod —
        // a plain drop, no second teardown.
        let v3: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_none(), "exiting dropped after settle");
        assert_eq!(
            torn.get(),
            1,
            "exiting dropped exactly once, after progress 1.0"
        );
        assert_eq!(
            teardowns.get(),
            1,
            "the settle drop is not a second teardown"
        );
        assert!(!w.settled);
        assert!(w.driver.is_none());

        // And the switcher's own teardown touches nothing already finished.
        v3.teardown(&mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!((teardowns.get(), torn.get()), (1, 1));
    }

    // --- A navigator under the outgoing child leaves the tree AT the switch:
    //     unmounted and claiming no back press on that very rebuild, while its
    //     frozen pod keeps painting until settle. ---

    #[test]
    fn a_navigator_in_the_outgoing_child_unmounts_on_the_switch_but_still_paints() {
        use crate::test_support::leaf;
        use crate::{NavigatorController, navigator};

        let ctrl: NavigatorController<()> = NavigatorController::new();
        let short = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let section = |key: u32| -> PatternSwitcherView<(), FadeThrough> {
            let child = if key == 1 {
                any(navigator(&ctrl, || leaf(10.0, 10.0)))
            } else {
                leaf_any(10.0, 10.0)
            };
            pattern_switcher(key, FadeThrough, child).timing(short)
        };
        let mut counter = 0u64;
        let v1 = section(1);
        let mut w = build(&v1);
        assert!(ctrl.is_mounted());

        // Depth 2 through a same-key rebuild: the navigator claims back.
        ctrl.push(|| leaf(40.0, 40.0));
        let v2 = section(1);
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(ctrl.depth(), 2);
        assert!(
            ctrl.back_interest(),
            "a mounted depth-2 navigator claims back"
        );

        // Switch the key: the very same rebuild unmounts it and silences it.
        let v3 = section(2);
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!ctrl.is_mounted(), "torn down at the switch, not at settle");
        assert!(
            !ctrl.back_interest(),
            "a torn-down navigator claims nothing, with no re-wire in between"
        );

        // ...while its frozen pod keeps painting its top page (40x40) until
        // settle. FadeThrough at p=0: the exiting child is fully opaque.
        let size = w.layout(&mut LayoutCtx::new(), &loose());
        let mut scene = RecordingScene::default();
        w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut scene);
        assert!(
            scene.rects.iter().any(|(_, s)| *s == Size::new(40.0, 40.0)),
            "the torn-down navigator's top page still paints: {:?}",
            scene.rects
        );

        // Settle and finalize: dropped, and still unmounted (no double count).
        w.driver
            .as_mut()
            .expect("driver created on first paint")
            .advance(ft_ms(10_000.0));
        w.paint(
            &mut PaintCtx::new(Point::ZERO, size),
            &mut RecordingScene::default(),
        );
        assert!(w.settled);
        let v4 = section(2);
        v4.rebuild(&v3, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_none());
        assert!(!ctrl.is_mounted());
        assert!(!ctrl.back_interest());
    }

    // --- A Component in the outgoing child is torn down (its owner disposed)
    //     exactly once across the freeze, the settle drop and the switcher's
    //     own teardown. Its child view's `View::teardown` only runs from the
    //     component's teardown (never from its `Drop`), so its count is the
    //     component's teardown count; `State` drops with the widget. ---

    struct ProbeComp {
        teardowns: Rc<Cell<u32>>,
        state_drops: Rc<Cell<u32>>,
    }
    struct ProbeState {
        drops: Rc<Cell<u32>>,
    }
    impl Drop for ProbeState {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    impl frust_core::Component for ProbeComp {
        type State = ProbeState;
        fn init(&self) -> ProbeState {
            ProbeState {
                drops: self.state_drops.clone(),
            }
        }
        fn build(&self, _state: &mut ProbeState) -> impl View<ProbeState> {
            TeardownLeaf {
                teardowns: self.teardowns.clone(),
            }
        }
    }
    /// A leaf over any state that counts its `View::teardown`s.
    struct TeardownLeaf {
        teardowns: Rc<Cell<u32>>,
    }
    impl<S: 'static> View<S> for TeardownLeaf {
        type Element = crate::test_support::LeafWidget;
        fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
            View::<()>::build(&crate::test_support::leaf(30.0, 30.0), ctx)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut Self::Element,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
        fn teardown(&self, _element: &mut Self::Element, _ctx: &mut BuildCtx<'_>) {
            self.teardowns.set(self.teardowns.get() + 1);
        }
    }

    #[test]
    fn a_component_in_the_outgoing_child_is_torn_down_exactly_once() {
        let teardowns = Rc::new(Cell::new(0u32));
        let state_drops = Rc::new(Cell::new(0u32));
        let short = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let comp = || {
            any(frust_core::component(ProbeComp {
                teardowns: teardowns.clone(),
                state_drops: state_drops.clone(),
            }))
        };
        let mut counter = 0u64;

        // (1) Freeze -> settle drop -> switcher teardown.
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, comp()).timing(short);
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            teardowns.get(),
            1,
            "the component is torn down at the freeze"
        );
        assert_eq!(
            state_drops.get(),
            0,
            "its widget (and state) is still painting"
        );

        // A torn-down component still paints its pod (pass-through).
        let mut scene = RecordingScene::default();
        w.layout(&mut LayoutCtx::new(), &loose());
        w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut scene);
        assert!(
            scene.rects.iter().any(|(_, s)| *s == Size::new(30.0, 30.0)),
            "the disposed component's subtree still paints: {:?}",
            scene.rects
        );

        w.driver
            .as_mut()
            .expect("driver created on first paint")
            .advance(ft_ms(10_000.0));
        w.paint(
            &mut PaintCtx::new(Point::ZERO, size),
            &mut RecordingScene::default(),
        );
        let v3: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        v3.rebuild(&v2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.exiting.is_none());
        assert_eq!(state_drops.get(), 1, "dropped once at settle");
        assert_eq!(
            teardowns.get(),
            1,
            "the settle drop is not a second teardown"
        );
        v3.teardown(&mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!((teardowns.get(), state_drops.get()), (1, 1));

        // (2) Freeze -> switcher teardown mid-transition (the exiting pod is
        //     still retained when the switcher itself leaves the tree).
        teardowns.set(0);
        state_drops.set(0);
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, comp()).timing(short);
        let mut w = build(&v1);
        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, leaf_any(10.0, 10.0)).timing(short);
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!((teardowns.get(), state_drops.get()), (1, 0));
        v2.teardown(&mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            w.exiting.is_none(),
            "the switcher's teardown drops the exiting pod"
        );
        assert_eq!(
            (teardowns.get(), state_drops.get()),
            (1, 1),
            "torn down once at the freeze, dropped once by the switcher's teardown"
        );
    }

    // --- Supersede mid-transition (A -> B -> C before settle): each outgoing
    //     child is torn down exactly once, at its own freeze. ---

    #[test]
    fn superseding_switches_tear_each_outgoing_child_down_exactly_once() {
        let counters = || (Rc::new(Cell::new(0u32)), Rc::new(Cell::new(0u32)));
        let (a_drop, a_td) = counters();
        let (b_drop, b_td) = counters();
        let (c_drop, c_td) = counters();
        let mut counter = 0u64;

        let va: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, FadeThrough, counting_recorder(&a_drop, &a_td));
        let mut w = build(&va);
        let vb: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, FadeThrough, counting_recorder(&b_drop, &b_td));
        vb.rebuild(&va, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            (a_td.get(), a_drop.get()),
            (1, 0),
            "A frozen: torn down, kept"
        );
        assert_eq!((b_td.get(), b_drop.get()), (0, 0), "B is live");

        // C supersedes before A's exit settles: A's pod is dropped (no second
        // teardown), B is frozen and torn down.
        let vc: PatternSwitcherView<(), _> =
            pattern_switcher(3u32, FadeThrough, counting_recorder(&c_drop, &c_td));
        vc.rebuild(&vb, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            (a_td.get(), a_drop.get()),
            (1, 1),
            "A superseded: dropped only"
        );
        assert_eq!(
            (b_td.get(), b_drop.get()),
            (1, 0),
            "B frozen: torn down, kept"
        );
        assert_eq!((c_td.get(), c_drop.get()), (0, 0), "C is live");

        // The switcher's own teardown: C (live) is torn down through its view,
        // B (frozen) is only dropped.
        vc.teardown(&mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!((a_td.get(), a_drop.get()), (1, 1));
        assert_eq!((b_td.get(), b_drop.get()), (1, 1), "B dropped, not re-torn");
        assert_eq!(
            c_td.get(),
            1,
            "the live child is torn down by the switcher's teardown"
        );
        drop(w);
        assert_eq!(
            (a_td.get(), b_td.get(), c_td.get()),
            (1, 1, 1),
            "each child torn down exactly once"
        );
        assert_eq!((a_drop.get(), b_drop.get(), c_drop.get()), (1, 1, 1));
    }

    // --- Same-key rebuilds reconcile in place: no transition, no teardown. ---

    #[test]
    fn same_key_rebuilds_reconcile_in_place_with_no_teardown() {
        let torn = Rc::new(Cell::new(0u32));
        let teardowns = Rc::new(Cell::new(0u32));
        let mut counter = 0u64;
        let make = || -> PatternSwitcherView<(), FadeThrough> {
            pattern_switcher(7u32, FadeThrough, counting_recorder(&torn, &teardowns))
        };
        let mut prev = make();
        let mut w = build(&prev);
        for _ in 0..3 {
            let next = make();
            next.rebuild(&prev, &mut w, &mut BuildCtx::new(&mut counter));
            prev = next;
        }
        assert!(w.exiting.is_none(), "no transition staged");
        assert_eq!(
            (teardowns.get(), torn.get()),
            (0, 0),
            "the live child was reconciled in place: never torn down, never dropped"
        );
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

    // --- Snapshot bracket: a staged child paints through push_retained/
    //     pop_retained keyed by snapshot_base (incoming) / snapshot_base + 1
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

    #[test]
    fn bracket_origin_follows_the_pod_dx_in_slide_patterns() {
        // SharedAxis::X slides children by 30dp (incoming from +30, exiting to -30).
        // The bracket origin must follow the pod's absolute paint origin —
        // ctx.origin() + (dx, dy) — so the cached texture is slide-invariant and
        // the body lands inside the texture without cropping. This test verifies
        // the bracket origin includes the dx offset during a slide transition.
        let timing = Timing::Duration(Duration::from_millis(100), Curve::Linear);
        let v1: PatternSwitcherView<(), _> =
            pattern_switcher(1u32, SharedAxis::X, leaf_any(10.0, 10.0)).timing(timing);
        let mut w = build(&v1);
        let size = w.layout(&mut LayoutCtx::new(), &loose());

        let v2: PatternSwitcherView<(), _> =
            pattern_switcher(2u32, SharedAxis::X, leaf_any(10.0, 10.0)).timing(timing);
        let mut counter = 0u64;
        v2.rebuild(&v1, &mut w, &mut BuildCtx::new(&mut counter));

        // Seed frame: the first `advance` only seeds the clock (progress 0).
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)),
            &mut SnapshotRecordingScene::default(),
        );

        // At p=0 (seed frame), SharedAxis::X has:
        // - Exiting child: dx=0.0, alpha=1.0 (visible)
        // - Incoming child: dx=30.0, alpha=0.0 (discarded, not visible)
        // So only the exiting child paints through a snapshot bracket.
        let mut scene = SnapshotRecordingScene::default();
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)),
            &mut scene,
        );

        assert_eq!(
            scene.snapshots.len(),
            1,
            "only the exiting child is visible at p=0"
        );

        // The exiting child has dx=0.0 at p=0, so its bracket origin should be
        // at ctx.origin() = (0.0, 0.0).
        let (_, origin, _, _, _) = scene.snapshots[0];
        assert_eq!(
            origin,
            Point::new(0.0, 0.0),
            "exiting child bracket origin at x=0.0 when dx=0 (was {:?})",
            origin
        );

        // At a mid-transition frame like p=0.5 (50ms into 100ms duration),
        // SharedAxis::X has:
        // - Exiting child: dx=-15.0 (sliding out), alpha < 1.0
        // - Incoming child: dx=15.0 (sliding in), alpha > 0.0
        // Both are potentially visible and should have bracket origins that
        // include their respective dx values. However, SharedAxis uses a hard
        // split, so only one is above alpha 0 at any instant. At p=0.5 (past
        // the ~0.35 split), the incoming child is visible.
        let mut scene_mid = SnapshotRecordingScene::default();
        w.paint(
            &mut PaintCtx::for_test(Point::ZERO, size, ft_ms(50.0)),
            &mut scene_mid,
        );

        // At p=0.5, the incoming child has dx = (1.0 - 0.5) * 30 = 15.0 and
        // is visible (past the split). Its bracket origin should reflect this.
        assert_eq!(
            scene_mid.snapshots.len(),
            1,
            "at mid-frame (p=0.5), only one child is visible through a snapshot"
        );
        let (_, origin_mid, _, _, _) = scene_mid.snapshots[0];
        assert_eq!(
            origin_mid,
            Point::new(15.0, 0.0),
            "incoming child bracket origin at x=15.0 when dx=15.0 (was {:?})",
            origin_mid
        );
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
