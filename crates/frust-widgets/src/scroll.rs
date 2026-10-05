//! The `ScrollView` widget: a vertical scroll surface with drag,
//! fling, and wheel support and clipped, offset content.
//!
//! [`scroll_view`] wraps a child that is laid out with unbounded height; the
//! view itself takes the incoming constraints and paints the child offset by
//! `-scroll_offset` inside a clip. Offsets settle within `[0, content −
//! viewport]`; what a pointer *drag* past an edge does is the installed
//! physics' call (wheel stays hard-clamped whatever it says). See
//! [`ScrollView::on_scroll`] for scroll observation and
//! [`ScrollView::on_refresh_release`] for the pull-to-refresh trigger.
//!
//! # Physics seam
//!
//! The feel is not wired in directly: the drag mapping, the
//! boundary-rejection rule, and post-release ballistic motion are asked of a
//! [`ScrollPhysics`] ([`crate::physics`]) the widget holds. The installed
//! default is the platform-adaptive pairing
//! ([`crate::physics::default_physics`]): Android gets clamping-plus-stretch,
//! every other platform bouncing-plus-translate.
//! [`RubberBand`](crate::RubberBand) — the pre-seam feel, a flat
//! [`OVERSCROLL_RESISTANCE`] rubber band with the legacy fling — is an opt-in
//! via [`ScrollView::physics`]. Two things stay widget-side on purpose: the
//! **legacy fling/settle path** (a physics whose
//! `create_ballistic_simulation` returns `None` — `RubberBand` always does —
//! leaves post-release motion to [`ScrollWidget::tick`]/
//! [`ScrollWidget::settle_tick`]), and the **wheel path**, which is a
//! physics-independent hard clamp. A physics that *does* hand back a
//! [`Simulation`] gets driven by the generic ballistic driver in
//! [`ScrollWidget::pump_fling`] instead.
//!
//! ## Drag convention: a per-move delta against the live position
//!
//! Every drag `Move` hands [`ScrollPhysics::apply_physics_to_user_offset`]
//! *that move's* raw finger delta, with metrics reporting the surface's real
//! current position — past-edge displacement and all
//! ([`ScrollWidget::apply_drag_offset`]). This is Flutter's own convention,
//! and it is what makes a depth-aware friction curve work: a physics whose
//! resistance tightens with overscroll depth (`Bouncing`) reads a real depth
//! rather than a permanent zero. The consequence is that the mapping is
//! **path-dependent** — the same total pull delivered in two moves and in four
//! need not land on the same pixel for a non-linear physics — which is
//! inherent to progressive tension, not a defect of this seam.
//!
//! # Overscroll visuals
//!
//! *What* a past-edge pull looks like is a separate axis from the physics that
//! computes it: [`OverscrollEffect`] selects between moving the content with
//! the pull ([`OverscrollEffect::Translate`], the default and this module's
//! long-standing behavior), the Material-3-Expressive
//! [`OverscrollEffect::Stretch`], and no visual at all. The offset itself
//! evolves identically under all three — only the paint changes.
//!
//! Stretch is a **paint-only** vertical scale about the held edge
//! ([`stretch_about_edge`], driven by [`ScrollWidget::edge_pull`] so a
//! clamping physics stretches too): the content origin stays where an in-range
//! offset would put it, and no layout pass reads the pull or the intensity
//! derived from it. It is an affine approximation of Android 12's overscroll
//! *shader* — the same approximation Flutter's non-Impeller
//! `StretchingOverscrollIndicator` makes — so roughly 60–70% of the real
//! effect: a whole-viewport scale cannot reproduce the shader's per-pixel
//! falloff, and Android's own release spring (ω = 24.657, ζ = 0.98) is not
//! ported either — the stretch decays on the same release settle the
//! displacement rides. Both are accepted approximations, for
//! `docs/LIMITATIONS.md`'s register rather than a fix here.
//!
//! # Gesture takeover
//!
//! ScrollView captures the pointer on `Down` and forwards events to the child
//! so descendant widgets stay interactive. It *observes* `Move` deltas before
//! forwarding: once the accumulated drag passes [`TOUCH_SLOP`] it enters
//! scrolling mode — it sends the child a synthetic `Cancel` (disarming any
//! armed descendant tap/press), stops forwarding, and consumes the drag itself.
//! This is how a scroll can be *taken* from a child after the slop, matching
//! masonry.
//!
//! ## Nested scrolling: innermost wins
//!
//! Dispatch is strictly parent-first, so an outer surface always reaches that
//! takeover site before any nested one sees the `Move` — left alone, a
//! scrollable inside a scrollable could never win a drag. Both surfaces
//! therefore run an **ambient claim** ([`InnerScrollState`],
//! [`with_scroll_claim`]), the shape the navigator's edge-swipe claim
//! (`R-B3-inner`, `nav::ambient`) established: a scrollable pushes a fresh
//! claim cell around the `Down` it forwards, any scrollable reached underneath
//! reports what it could do with the gesture into it, and the outer reads that
//! answer back ([`ScrollWidget::inner_at_down`]) before deciding at the slop.
//! When the nested surface can consume the drag's *direction*, the outer
//! **defers**: it takes nothing over, sends no `Cancel`, and keeps forwarding
//! the real events for the rest of the gesture, so the inner's own slop
//! machinery takes the drag (and cancels its own child). Otherwise the takeover
//! below runs exactly as it always has — with no nested scrollable present the
//! claim never registers and not one byte of this changes.
//!
//! The claim itself requires real capacity
//! (`max_scroll_extent > min_scroll_extent`), on top of whatever the physics'
//! own drag gate says: a bouncing-family physics accepts a user offset
//! unconditionally, so without this a nested surface whose content exactly
//! fills its viewport would still claim (and hold) every drag forever, with
//! nothing to show for it. This parts from Flutter, whose bouncing physics
//! bounces a fits-viewport scrollable too, toward UIKit's own default
//! (`alwaysBounceVertical == false`): a scrollable with nothing to scroll
//! does not intercept the gesture.
//!
//! ## Multi-contact veto: a live counterpart to the `Down`-time claim
//!
//! The ambient claim above is a snapshot taken once, synchronously, right
//! after the `Down` is forwarded — too early for a multi-contact recognizer
//! nested inside (`pinch_detector`, [`crate::pan_zoom`]'s child-owned-press
//! branch) to report anything about a second contact that has not arrived
//! yet. Those recognizers instead capture the ambient **multi-contact veto**
//! cell ([`ambient_scroll_veto`]) on the claimant's `Down` and keep writing to
//! it for the rest of the gesture: `true` while they are tracking more than
//! the claimant's own contact, `false` once back down to one. The takeover
//! site checks it live, on every `Move`, alongside `inner_at_down` — while it
//! reads `true` this surface takes nothing over and keeps forwarding the
//! claimant's events, exactly as it does for a deferred nested scrollable, so
//! a two-finger pinch or pan-zoom beginning over a single-finger drag is never
//! stolen out from under it. Clearing the veto does not retroactively replay
//! the slop check that a live pinch suppressed — the very next claimant
//! `Move` is measured against the gesture's original `down_start` as usual,
//! so a drag that already travelled well past [`TOUCH_SLOP`] while the second
//! finger was down takes over immediately once the veto lifts.
//!
//! # Fling driver (v1)
//!
//! On release with sufficient velocity a fling begins, integrated
//! frame-by-frame with [`ScrollWidget::tick`] (pure, unit-tested). [`paint`]
//! pumps the fling from the shared shell frame clock ([`PaintCtx::frame_time`]
//! — no wall-clock reads in widget code) so it animates for free on the
//! continuous-loop mobile shells, and calls [`PaintCtx::request_frame`] while the
//! fling is still in flight so the desktop shell (event-driven
//! `ControlFlow::Wait`) keeps scheduling frames via `window.request_redraw()`;
//! the signal stops once the fling reaches rest. The first paint after the
//! release seeds the fling clock from `frame_time` (a zero-delta frame), and each
//! subsequent paint advances it by the inter-frame delta.
//!
//! # Programmatic scrolling
//!
//! A [`ScrollController`] attached with [`ScrollView::controller`] is the one
//! external write seam: its recorded commands are drained at the start of the
//! widget's next layout or paint ([`ScrollWidget::drain_controller`]), after
//! the content has been measured, and every pass that moves the position
//! publishes the new [`ScrollInfo`] back to the handle. A jump
//! ([`ScrollWidget::apply_jump`]) stops any fling, settle or ballistic
//! simulation in flight; its `on_scroll` notification has no `EventCtx` to
//! fire through, so — like a fling frame's — it is owed to the next event,
//! and a paint that applied one raises
//! [`frust_core::mark_pending_result_flush`] so that event is the very next
//! frame's `Housekeeping` flush. `ScrollController::animate_to` eases to its
//! target the same way, through a [`TweenSimulation`] driven by the same
//! [`ScrollWidget::ballistic`] field and [`ScrollWidget::drive_ballistic`]
//! pump a release fling runs through
//! ([`ScrollWidget::apply_animate_to`]) — so a jump, a fresh animate, or any
//! user pointer `Down`/wheel on the surface all end it the same way
//! ([`ScrollWidget::stop_ballistic`]), and the app's `motion.reduce_motion`
//! theme flag collapses it to an immediate jump. See
//! [`crate::scroll_controller`] for the handle's side of the contract.
//!
//! **A command ends a live drag** (Flutter's `animateTo`/`jumpTo` semantics):
//! a jump or animate applied while a finger is down replaces the drag instead
//! of running alongside it ([`ScrollWidget::end_live_drag`]). The rest of that
//! gesture's `Move`s fall through to the child/hover path and its `Up` to the
//! non-scrolling branch, so the finger can no longer write the offset the
//! command now owns, and the release starts no fling or settle of its own.
//! Pointer capture is not touched — the drain runs in layout/paint, which
//! carry no `EventCtx`, and the shell releases capture on the physical
//! `Up`/`Cancel` regardless. Every post-release start (`Up`'s simulation,
//! settle or fling) tears the previous motion down through
//! [`ScrollWidget::stop_ballistic`] first, so a fling, a settle and a
//! simulation are never live together and `is_animating` cannot outlive a
//! tween.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta, SemanticsCtx, TOUCH_SLOP, VelocityTracker, View, WHEEL_LINE_PX,
    Widget, any, fling_decay, fling_displacement,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size};

use crate::authoring::{ErasedArgCallback, ErasedCallback, presses};
use crate::physics::effect::OverscrollEffect;
use crate::physics::simulation::TweenSimulation;
use crate::physics::{
    MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR, ScrollMetrics, ScrollPhysics, Simulation,
    default_overscroll_effect, default_physics,
};
use crate::scroll_controller::{AnimateTo, ScrollBinding, ScrollCommand, ScrollController};

/// iOS-style rubber-band resistance applied to the past-edge portion of a drag
/// under [`RubberBand`](crate::RubberBand): the visible out-of-range
/// displacement is `raw_excess * OVERSCROLL_RESISTANCE`. No longer any
/// platform's default — the platform-parity physics carry their own curves —
/// but still the constant the opt-in pre-seam feel is defined by.
///
/// **Community-approximate**: UIScrollView's rubber-banding is a
/// diminishing-returns curve (roughly `d·(1 − 1/(1 + d/dim·c))`), not a
/// published constant. A flat `0.5` factor is the common community linear
/// approximation — half the raw finger travel shows past the edge, giving the
/// pull a heavier feel the further it is dragged in *raw* terms while staying
/// cheap and deterministic to reason about. Tunable in one place if a
/// diminishing curve is wanted later.
///
/// `pub(crate)`, not `pub`: [`crate::list_view::ListViewWidget`] shares this
/// exact value (and the three constants below) for overscroll feel parity
/// (lazy-list 06) — never redeclare a second copy there.
pub(crate) const OVERSCROLL_RESISTANCE: f64 = 0.5;

/// Pull-past-top distance (logical px, measured on [`ScrollWidget::edge_pull`]
/// — the physics-mapped pull, so a resisting physics needs proportionally more
/// raw finger travel to reach it and a clamping one exactly this much) beyond
/// which releasing fires [`ScrollView::on_refresh_release`] (and
/// `ListView::on_refresh_release`, which shares this constant and
/// [`crossed_refresh_trigger`]) — the pull-to-refresh trigger.
///
/// **Community-approximate**: iOS's `UIRefreshControl` trigger distance is not a
/// published constant; ~64pt is the value community reimplementations converge
/// on for a comfortable pull.
pub(crate) const REFRESH_TRIGGER_PX: f64 = 64.0;

/// Per-millisecond retain factor for the release-settle animation that returns
/// an overscrolled surface to its clamped edge: after `dt` ms the remaining
/// distance to the edge is scaled by `SETTLE_DECAY.powf(dt)`.
///
/// **Community-approximate**: `0.988` settles ~95% of the way in ≈250 ms, an
/// iOS-like snap-back with no published spring spec to match.
pub(crate) const SETTLE_DECAY: f64 = 0.988;

/// Distance (logical px) below which the settle animation snaps exactly to the
/// edge and stops, so it terminates instead of asymptotically approaching.
pub(crate) const SETTLE_STOP_PX: f64 = 0.5;

/// The [`ScrollMetrics::device_pixel_ratio`] both scroll surfaces report.
///
/// **No context exposes a real one**: density is resolved at the shell's FFI
/// boundary and everything above it speaks logical pixels
/// (`docs/CODE_STANDARDS.md`'s Interaction Semantics), so neither `EventCtx`
/// nor `PaintCtx` carries a scale factor a widget could thread in. `1.0` is
/// therefore reported rather than guessed. Nothing in the shipped feel reads
/// it — every constant above is dpr-independent, and it feeds only
/// [`crate::physics::Tolerance::for_device_pixel_ratio`] for a physics that
/// builds a [`Simulation`]. Shared with [`crate::list_view::ListViewWidget`]
/// so the two surfaces cannot report different densities.
pub(crate) const METRICS_FALLBACK_DPR: f64 = 1.0;

/// Whether a past-top overscroll displacement crossed [`REFRESH_TRIGGER_PX`] —
/// the pull-to-refresh release condition, shared with
/// [`crate::list_view::ListViewWidget`] so both surfaces trigger at exactly the
/// same pull distance (lazy-list 06) rather than two independently-typed
/// comparisons drifting apart.
pub(crate) fn crossed_refresh_trigger(overscroll: f64) -> bool {
    overscroll < -REFRESH_TRIGGER_PX
}

/// The scale [`OverscrollEffect::Stretch`] adds per unit of normalized pull —
/// the linear term's slope and, equally, the exponential term's ceiling, so a
/// full-viewport pull stretches by at most `2 · STRETCH_INTENSITY`.
///
/// **Published value**, not a guess: Flutter's `_StretchController`
/// (`widgets/overscroll_indicator.dart`) uses exactly this in its port of
/// Android 12's overscroll effect, and the port here is term-for-term the
/// same curve. Shared with [`crate::list_view::ListViewWidget`] like every
/// constant above.
pub(crate) const STRETCH_INTENSITY: f64 = 0.016;

/// How fast [`stretch_intensity`]'s exponential term saturates. Flutter's
/// `_StretchController.exponentialScalar` verbatim (`e / 0.33`): the pull is
/// ~95% of the way to the term's ceiling at a third of a viewport.
pub(crate) const STRETCH_EXP_SCALAR: f64 = std::f64::consts::E / 0.33;

/// The added scale [`OverscrollEffect::Stretch`] paints for a pull of
/// `edge_pull` (signed, in the [`ScrollWidget::edge_pull`] sense) against a
/// viewport `viewport_dimension` px along the scroll axis: a linear plus a
/// saturating-exponential term over the normalized pull
/// `x = |edge_pull| / viewport_dimension`, clamped to `[0, 1]`.
///
/// Magnitude-only — which edge is held decides the anchor
/// ([`stretch_about_edge`]), never the amount. `0.0` for an unpulled surface
/// or a degenerate (zero-height) viewport, and never above
/// `2 · STRETCH_INTENSITY`.
pub(crate) fn stretch_intensity(edge_pull: f64, viewport_dimension: f64) -> f64 {
    if viewport_dimension <= 0.0 {
        return 0.0;
    }
    let x = (edge_pull.abs() / viewport_dimension).clamp(0.0, 1.0);
    STRETCH_INTENSITY * x + STRETCH_INTENSITY * (1.0 - (-x * STRETCH_EXP_SCALAR).exp())
}

/// The paint-side affine [`OverscrollEffect::Stretch`] wraps a viewport's
/// content in: a scroll-axis-only scale of `1 + stretch_intensity(…)` about
/// the **held** edge — the viewport's top for a pull past the top (negative
/// `edge_pull`), its bottom edge otherwise — so the content grows away from
/// the finger while the edge under it stays pinned.
///
/// `origin`/`size` are the viewport's absolute paint geometry
/// ([`PaintCtx::origin`]/[`PaintCtx::size`]), which is the space
/// [`PaintScene::push_transform`] composes in. `None` when there is nothing to
/// paint (no pull, or a degenerate viewport), so a caller pushes no transform
/// at all rather than an identity one.
pub(crate) fn stretch_about_edge(origin: Point, size: Size, edge_pull: f64) -> Option<Affine> {
    let intensity = stretch_intensity(edge_pull, size.height);
    if intensity == 0.0 {
        return None;
    }
    let anchor = if edge_pull < 0.0 {
        origin.y
    } else {
        origin.y + size.height
    };
    // The standard scale-about-a-point sandwich (`motion::animated`'s
    // `scale_about`, `nav::transition`'s `rect_to_rect`), non-uniform so only
    // the scroll axis stretches.
    Some(
        Affine::translate((0.0, anchor))
            * Affine::scale_non_uniform(1.0, 1.0 + intensity)
            * Affine::translate((0.0, -anchor)),
    )
}

// --- Nested-scroll arbitration: the ambient inner-scroll claim ---------------

/// How far past an edge [`inner_claim_state`] probes
/// [`ScrollPhysics::apply_boundary_conditions`] to ask "would a pull past this
/// edge be rejected?" (logical px).
///
/// The question is categorical — *does this physics hold an out-of-range
/// position at all* — not metric, and every physics in [`crate::physics`]
/// answers it the same way at any depth, so one pixel is enough; it is far
/// enough past the extent that no rounding step can land back inside it.
const CLAIM_PROBE_PX: f64 = 1.0;

/// A `Down`-time snapshot of what a nested scroll surface could do with the
/// gesture, written by that surface into its nearest enclosing one's ambient
/// claim cell and read back there at the takeover site (the module docs'
/// *Nested scrolling*).
///
/// "Down"/"up" name the **finger's** direction, never the offset's: a
/// finger-moving-down drag reveals content *above* it (the offset falls toward
/// the leading edge), a finger-moving-up drag reveals content below.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InnerScrollState {
    /// Whether a nested scroll surface reported at all. `false` — the default
    /// a never-written cell reads back — is the no-nested-scrollable case
    /// every gesture took before arbitration existed.
    pub(crate) registered: bool,
    /// Whether the inner can consume a finger-moving-DOWN drag: it still holds
    /// content above (`pixels > min_scroll_extent`), or its physics allows
    /// past-leading-edge displacement, which lets a bouncing-family surface
    /// answer a pull with a rubber-band even pinned at the top.
    pub(crate) can_consume_down_drag: bool,
    /// Whether the inner can consume a finger-moving-UP drag: content below
    /// (`pixels < max_scroll_extent`), or past-trailing displacement allowed.
    pub(crate) can_consume_up_drag: bool,
}

impl InnerScrollState {
    /// Whether an outer surface must stand down at its takeover site for a
    /// cumulative drag of `dy` finger px (positive = the finger moved down) —
    /// innermost-wins: a registered inner that can consume *this* direction
    /// owns the gesture; one pinned against it does not, and the outer takes
    /// over as it always has.
    ///
    /// Only ever asked past [`TOUCH_SLOP`], so `dy` is never zero; a zero would
    /// read as an up-drag rather than warranting a third branch.
    pub(crate) fn defers(self, dy: f64) -> bool {
        if !self.registered {
            return false;
        }
        if dy > 0.0 {
            self.can_consume_down_drag
        } else {
            self.can_consume_up_drag
        }
    }
}

thread_local! {
    /// The stack of per-surface **inner-scroll claim** cells for the scroll
    /// surfaces currently forwarding a pointer `Down` — the seam a nested
    /// scrollable reports itself to its nearest enclosing one through
    /// (mirrors `nav::ambient`'s `SWIPE_CLAIM`/`with_swipe_claim`).
    ///
    /// A `Down` does not decide anything: the outer surface captures, forwards
    /// it, and only at the later `Move` slop does it choose between taking the
    /// drag over and deferring — by which point dispatch order has already put
    /// it upstream of the inner. So the outer pushes a fresh cell before
    /// forwarding the `Down` ([`with_scroll_claim`]), the nested surface writes
    /// its [`InnerScrollState`] into whatever cell is ambient
    /// ([`ambient_scroll_claim`]) as that `Down` reaches it, and the outer
    /// reads it back once forwarding returns.
    ///
    /// A **stack**, not a single slot, because the pairing must be
    /// *nearest*-inner: a surface writes its own state into the ambient cell
    /// **before** pushing its own cell for its own children, so its write lands
    /// in its enclosing surface's cell while everything deeper lands in its
    /// own. Three levels deep, the outermost therefore learns only about the
    /// middle surface and the middle only about the innermost — nothing
    /// propagates a grandchild's claim up past its own parent, which is what
    /// makes each layer arbitrate against the layer it actually contains.
    ///
    /// Reactive-free by construction (`frust-widgets` carries no
    /// `reactive_graph` dependency): a plain `Rc<Cell<_>>`, never a signal, and
    /// UI-thread-affine for the same reason `nav::ambient`'s cells are.
    static SCROLL_CLAIM: RefCell<Vec<Rc<Cell<InnerScrollState>>>> =
        const { RefCell::new(Vec::new()) };
}

/// Pops [`SCROLL_CLAIM`] on drop, so an unwinding child dispatch cannot leave a
/// stale scope behind for the rest of the thread's life (mirrors
/// `nav::ambient`'s `SwipeClaimGuard`).
struct ScrollClaimGuard;

impl Drop for ScrollClaimGuard {
    fn drop(&mut self) {
        SCROLL_CLAIM.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Run `f` (a `Down` forward into this surface's own children) with `claim`
/// installed as the ambient inner-scroll claim cell, so the nearest scroll
/// surface reached underneath can report itself into it via
/// [`ambient_scroll_claim`].
///
/// The [`SCROLL_CLAIM`] borrow is released *before* `f` runs, so `f` may itself
/// nest another `with_scroll_claim` call (a third level of scroll nesting).
pub(crate) fn with_scroll_claim<R>(claim: &Rc<Cell<InnerScrollState>>, f: impl FnOnce() -> R) -> R {
    SCROLL_CLAIM.with(|stack| stack.borrow_mut().push(Rc::clone(claim)));
    let _guard = ScrollClaimGuard;
    f()
}

/// The claim cell of the scroll surface currently forwarding a `Down` — the
/// *nearest enclosing* one — or `None` if none is (a top-level surface's own
/// `Down`, or any non-`Down` event).
pub(crate) fn ambient_scroll_claim() -> Option<Rc<Cell<InnerScrollState>>> {
    SCROLL_CLAIM.with(|stack| stack.borrow().last().cloned())
}

thread_local! {
    /// The stack of per-surface **multi-contact veto** cells for the scroll
    /// surfaces currently forwarding a pointer `Down` — [`SCROLL_CLAIM`]'s
    /// live counterpart (the module docs' *Multi-contact veto*). A nested
    /// recognizer that opts into the gesture's other contacts
    /// (`pinch_detector`, [`crate::pan_zoom`]'s child-owned-press branch)
    /// captures the ambient cell on the claimant's `Down`, while it is still
    /// reachable, and keeps writing to it for the rest of the gesture — long
    /// after the `Down` forward that exposed it has returned, which is
    /// exactly what a `Down`-time snapshot like [`InnerScrollState`] cannot
    /// do (it is read back once, synchronously, before a second contact can
    /// possibly have arrived).
    ///
    /// A separate stack from [`SCROLL_CLAIM`] rather than a field folded into
    /// it: `InnerScrollState` stays `Copy` (and byte-identical) for
    /// [`crate::list_view::ListViewWidget`]'s existing consumption of it, and
    /// a plain `Rc<Cell<bool>>` is exactly as `Copy`-free a value as this
    /// seam needs.
    static MULTI_CONTACT_VETO: RefCell<Vec<Rc<Cell<bool>>>> = const { RefCell::new(Vec::new()) };
}

/// Pops [`MULTI_CONTACT_VETO`] on drop, so an unwinding child dispatch cannot
/// leave a stale scope behind for the rest of the thread's life (mirrors
/// [`ScrollClaimGuard`]).
struct MultiContactVetoGuard;

impl Drop for MultiContactVetoGuard {
    fn drop(&mut self) {
        MULTI_CONTACT_VETO.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Run `f` (a `Down` forward into this surface's own children) with `veto`
/// installed as the ambient multi-contact veto cell, so a recognizer reached
/// underneath that opts into the gesture's other contacts can capture it and
/// raise it later, once a second contact actually joins the gesture.
///
/// The [`MULTI_CONTACT_VETO`] borrow is released *before* `f` runs, mirroring
/// [`with_scroll_claim`].
pub(crate) fn with_scroll_veto<R>(veto: &Rc<Cell<bool>>, f: impl FnOnce() -> R) -> R {
    MULTI_CONTACT_VETO.with(|stack| stack.borrow_mut().push(Rc::clone(veto)));
    let _guard = MultiContactVetoGuard;
    f()
}

/// The multi-contact veto cell of the scroll surface currently forwarding a
/// `Down` — the *nearest enclosing* one — or `None` outside that forward (a
/// top-level surface's own `Down`, or any non-`Down` event).
pub(crate) fn ambient_scroll_veto() -> Option<Rc<Cell<bool>>> {
    MULTI_CONTACT_VETO.with(|stack| stack.borrow().last().cloned())
}

/// What a surface sitting at `metrics` under `physics` claims it could do with
/// a drag starting now — the value a nested scrollable writes into its host's
/// claim cell.
///
/// Both directions are answered the same way: *room* in that direction, or a
/// physics willing to hold a position past that edge. The second half is a
/// one-pixel [`CLAIM_PROBE_PX`] probe of
/// [`ScrollPhysics::apply_boundary_conditions`] — nothing rejected means the
/// surface can answer the drag with a rubber-band even pinned against the edge
/// (the `Bouncing`/[`RubberBand`](crate::RubberBand) family), while a clamping
/// physics rejects the probe and genuinely has nothing to give. `registered`
/// is the physics' own drag gate, the same one the takeover site consults —
/// **and, on top of it, real capacity** (`max_scroll_extent >
/// min_scroll_extent`): the bouncing family's `should_accept_user_offset` is
/// hardcoded `true` regardless of content, which without this conjunct would
/// make a content-fits inner claim (and defer to) every drag forever, even
/// though it has nothing to show for it. This deliberately parts from
/// Flutter, whose `BouncingScrollPhysics` would still bounce a fits-viewport
/// surface, in favor of UIKit's own default
/// (`UIScrollView.alwaysBounceVertical == false`): a scrollable with nothing
/// to scroll does not intercept the gesture. A `NeverScrollable` inner, or
/// one with real capacity but no physics willing to accept the drag, never
/// takes a drag from its host either way.
pub(crate) fn inner_claim_state(
    physics: &dyn ScrollPhysics,
    metrics: &ScrollMetrics,
) -> InnerScrollState {
    let leading_free = physics
        .apply_boundary_conditions(metrics, metrics.min_scroll_extent - CLAIM_PROBE_PX)
        == 0.0;
    let trailing_free = physics
        .apply_boundary_conditions(metrics, metrics.max_scroll_extent + CLAIM_PROBE_PX)
        == 0.0;
    let has_capacity = metrics.max_scroll_extent > metrics.min_scroll_extent;
    InnerScrollState {
        registered: has_capacity && physics.should_accept_user_offset(metrics),
        can_consume_down_drag: metrics.pixels > metrics.min_scroll_extent || leading_free,
        can_consume_up_drag: metrics.pixels < metrics.max_scroll_extent || trailing_free,
    }
}

/// A scroll observation snapshot handed to [`ScrollView::on_scroll`].
///
/// `offset` is the clamped scroll position in `[0, max_offset]`; `overscroll` is
/// the signed past-edge displacement (negative = pulled past the top, positive =
/// pulled past the bottom), zero while the surface rests in range. During a
/// drag past an edge, `offset` pins at the edge and `overscroll` carries the
/// (resisted) pull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollInfo {
    /// The clamped scroll offset in `[0, max_offset]` (px scrolled down).
    pub offset: f64,
    /// The maximum scroll offset (`content − viewport`, never negative).
    pub max_offset: f64,
    /// Signed past-edge displacement: negative past the top, positive past the
    /// bottom, `0.0` while in range.
    pub overscroll: f64,
}

/// A view-held scroll-observation callback (erased to [`ErasedArgCallback`] on
/// build).
type OnScroll<State> = Rc<dyn Fn(&mut State, ScrollInfo)>;

/// A view-held pull-to-refresh release callback (erased to [`ErasedCallback`] on
/// build).
type OnRefresh<State> = Rc<dyn Fn(&mut State)>;

/// A declarative vertical scroll surface. See the [module docs](self).
pub struct ScrollView<State: 'static> {
    child: AnyView<State>,
    /// Fired whenever the offset/overscroll changes (drag, wheel, or — one event
    /// late — fling/settle). See [`ScrollView::on_scroll`].
    on_scroll: Option<OnScroll<State>>,
    /// Fired on pointer `Up` when the past-top overscroll exceeded
    /// [`REFRESH_TRIGGER_PX`]. See [`ScrollView::on_refresh_release`].
    on_refresh_release: Option<OnRefresh<State>>,
    /// A custom [`ScrollPhysics`] installed via [`ScrollView::physics`], or
    /// `None` to leave whatever is already installed on the widget alone.
    /// `Rc`, not `Box`: [`ScrollWidget::physics`] is shared-immutable widget
    /// state, so a cheap `Rc::clone` is what a `rebuild` (which only ever sees
    /// `&self`) can hand across without a `Box<dyn ScrollPhysics>`-isn't-`Clone`
    /// reconstruction problem. See [`ScrollView::physics`] for the full
    /// build/rebuild contract.
    physics: Option<Rc<dyn ScrollPhysics>>,
    /// How past-edge pull is visualized, carried down to
    /// [`ScrollWidget::effect`] on every build/rebuild. See
    /// [`ScrollView::overscroll_effect`].
    pub(crate) effect: OverscrollEffect,
    /// The programmatic-scroll handle to attach, if any. See
    /// [`ScrollView::controller`].
    controller: Option<ScrollController>,
}

impl<State: 'static> ScrollView<State> {
    /// Wrap `child` in a vertical scroll view.
    pub fn new<V: View<State>>(child: V) -> Self {
        Self {
            child: any(child),
            on_scroll: None,
            on_refresh_release: None,
            physics: None,
            effect: default_overscroll_effect(),
            controller: None,
        }
    }

    /// Observe scroll position changes. The callback receives a [`ScrollInfo`]
    /// snapshot each time the offset or overscroll changes due to input (drag,
    /// wheel), and — one event late — for fling/settle motion driven at paint
    /// time (the paint pass carries no [`EventCtx`], so the notification is
    /// recorded and delivered on the next event, the same controlled-component
    /// convention the interactive widgets follow). A `Cancel` clears any pending
    /// notification without firing it.
    pub fn on_scroll<F: Fn(&mut State, ScrollInfo) + 'static>(mut self, callback: F) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    /// The pull-to-refresh trigger: fires on pointer `Up` when the surface was
    /// pulled past the top by more than [`REFRESH_TRIGGER_PX`] (post-resistance),
    /// so an app gets a single "release past threshold" signal without
    /// reimplementing overscroll thresholding. Never fires on a `Cancel`.
    pub fn on_refresh_release<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_refresh_release = Some(Rc::new(callback));
        self
    }

    /// Install a custom [`ScrollPhysics`] strategy — the pluggable
    /// drag-mapping/boundary-rejection/post-release-motion contract described
    /// in [`crate::physics`], with the platform-parity physics
    /// ([`crate::physics::parity`]'s `Bouncing`/`Clamping`/
    /// `AlwaysScrollable`/`NeverScrollable`) and the pre-seam
    /// [`RubberBand`](crate::RubberBand) as the built-in implementations.
    ///
    /// ```
    /// use frust_widgets::{NeverScrollable, ScrollView, scroll_view, text};
    /// let view: ScrollView<()> = scroll_view(text("hi")).physics(NeverScrollable::new());
    /// # let _ = view;
    /// ```
    ///
    /// # Build/rebuild semantics
    ///
    /// A view built (or rebuilt) *with* `.physics(...)` installs it on the
    /// widget every time — like [`ScrollView::on_scroll`]'s erased callback,
    /// a trait object isn't comparable, so this reinstalls unconditionally
    /// rather than diffing. A view built (or rebuilt) *without*
    /// `.physics(...)` leaves whatever the widget already has installed
    /// untouched: a fresh `build` still starts the widget at
    /// [`crate::physics::default_physics`] (the widget's own constructor
    /// default), but rebuilding *from* a `.physics(...)`-carrying view *to* a
    /// plain one does not revert it — there is no spelling for "go back to the
    /// default" versus "no opinion this rebuild", and this picks the latter,
    /// the same shape [`ScrollWidget::effect`] already followed before this
    /// method existed.
    ///
    /// Defaults to the platform-adaptive physics
    /// ([`crate::physics::default_physics`] — Android clamping, elsewhere
    /// bouncing) if never called; `.physics(RubberBand::new())` is how an app
    /// asks for the pre-seam rubber-band feel instead.
    pub fn physics(mut self, physics: impl ScrollPhysics + 'static) -> Self {
        self.physics = Some(Rc::new(physics));
        self
    }

    /// Select how past-edge pull is visualized. See [`OverscrollEffect`]
    /// ([`crate::physics::effect`]) for the full contract: move the content
    /// with the pull, paint a Material-3-Expressive edge stretch about the
    /// held edge, or show no visual at all.
    ///
    /// ```
    /// use frust_widgets::{OverscrollEffect, ScrollView, scroll_view, text};
    /// let view: ScrollView<()> = scroll_view(text("hi")).overscroll_effect(OverscrollEffect::Stretch);
    /// # let _ = view;
    /// ```
    ///
    /// Plain view-owned data, unlike [`ScrollView::physics`]: every
    /// build/rebuild carries the current value down to
    /// [`ScrollWidget::effect`] unconditionally.
    ///
    /// Defaults to the effect paired with the platform's default physics
    /// ([`crate::physics::default_overscroll_effect`]): the M3E
    /// [`OverscrollEffect::Stretch`] on Android, translate overscroll
    /// ([`OverscrollEffect::Translate`]) everywhere else.
    pub fn overscroll_effect(mut self, effect: OverscrollEffect) -> Self {
        self.effect = effect;
        self
    }

    /// Attach a [`ScrollController`] — the handle app code drives this surface
    /// with ([`ScrollController::jump_to`]) and reads it through
    /// ([`ScrollController::offset`]/[`ScrollController::max_offset`]/
    /// [`ScrollController::on_change`]) from outside any dispatch.
    ///
    /// ```
    /// use frust_widgets::{ScrollController, ScrollView, scroll_view, text};
    /// let controller = ScrollController::new();
    /// let view: ScrollView<()> = scroll_view(text("hi")).controller(controller.clone());
    /// # let _ = view;
    /// ```
    ///
    /// # Build/rebuild semantics
    ///
    /// A build binds the handle, taking it over from any other surface that
    /// held it. A rebuild with the same handle keeps the binding — and, when
    /// another surface has since taken the handle over, leaves it there rather
    /// than taking it back, so two live views naming one handle never trade it
    /// every frame (it re-binds only once the handle is unattached again). A
    /// rebuild with a different handle binds that one; a rebuild with none
    /// detaches. Dropping the widget detaches the handle it still holds.
    pub fn controller(mut self, controller: ScrollController) -> Self {
        self.controller = Some(controller);
        self
    }
}

/// Wrap `child` in a vertical [`ScrollView`] — the free-function spelling of
/// [`ScrollView::new`].
pub fn scroll_view<State: 'static, V: View<State>>(child: V) -> ScrollView<State> {
    ScrollView::new(child)
}

/// A running [`Simulation`] and the frame clock it is measured from — the
/// state behind the generic ballistic driver both scroll surfaces run when
/// their physics hands one back (`pub(crate)`: shared with
/// [`crate::list_view::ListViewWidget`]'s parallel pump rather than
/// hand-copied, like the constants above).
pub(crate) struct BallisticState {
    /// The physics-built curve, in **seconds** from its own start.
    pub(crate) sim: Box<dyn Simulation>,
    /// The frame time the first paint-time pump seeded, or `None` before it —
    /// the same zero-delta seeding convention `last_anim` uses, so the release
    /// itself (which carries no clock) never has to guess a start.
    pub(crate) start: Option<FrameTime>,
}

impl BallisticState {
    /// Seconds elapsed at frame time `now`; `0.0` until the clock is seeded.
    pub(crate) fn elapsed_secs(&self, now: FrameTime) -> f64 {
        match self.start {
            Some(start) => now.saturating_sub(start).as_secs_f64(),
            None => 0.0,
        }
    }
}

/// The retained widget for a [`ScrollView`].
pub struct ScrollWidget {
    child: ChildPod,
    /// Current *effective* scroll offset (px scrolled down). Normally in
    /// `[0, max_offset]`, but a drag past an edge lets it go out of range (with
    /// [`OVERSCROLL_RESISTANCE`] applied) until the release-settle brings it back;
    /// the fling/wheel/layout paths still hard-clamp via [`ScrollWidget::set_offset`].
    offset: f64,
    /// Resolved viewport size (this widget's own size).
    viewport: Size,
    /// The child's (content) size after an unbounded-height layout.
    content: Size,
    /// Whether we have taken the gesture over as a scroll drag.
    scrolling: bool,
    /// The physics-mapped drag position accumulated during an active scroll
    /// drag, **before** boundary rejection: seeded from `offset` at takeover
    /// and advanced by each move's mapped delta
    /// ([`ScrollWidget::apply_drag_offset`]). It equals `offset` under any
    /// physics that rejects nothing, and runs off past the edge under a
    /// clamping one — which is exactly what lets a pinned surface still report
    /// how hard the finger is pulling.
    drag_position: f64,
    /// Whether a release-settle animation is returning an overscrolled surface to
    /// its clamped edge (driven at paint via [`ScrollWidget::settle_tick`]).
    settling: bool,
    /// The installed scroll physics — [`crate::physics::default_physics`]'s
    /// platform-adaptive choice unless [`ScrollView::physics`] replaces it.
    /// `Rc`, not `Box`: every [`ScrollPhysics`] method takes `&self`, so a
    /// shared, immutable handle is both cheap to (re)install (a `Rc::clone`,
    /// not a fresh reconstruction — `Box<dyn ScrollPhysics>` isn't `Clone`)
    /// and sufficient, since nothing here ever needs `&mut` access to it.
    /// Survives a rebuild whose view carries no `.physics(...)` call
    /// untouched; see [`ScrollView::physics`] for the full contract and the
    /// [module docs](self)' *Physics seam*.
    pub(crate) physics: Rc<dyn ScrollPhysics>,
    /// How past-edge pull is visualized —
    /// [`crate::physics::default_overscroll_effect`]'s platform pairing unless
    /// [`ScrollView::overscroll_effect`] names one. Read at paint alone (see
    /// [`ScrollWidget::painted_offset`] and the module docs' *Overscroll
    /// visuals*), never by layout or by the physics.
    pub(crate) effect: OverscrollEffect,
    /// The signed pull past an edge, in the same sense as
    /// [`ScrollInfo::overscroll`]: **negative past the top**, positive past the
    /// bottom, `0.0` in range. Its two halves are the displacement the physics
    /// *allowed* (what [`ScrollInfo::overscroll`] reports) plus whatever
    /// [`ScrollPhysics::apply_boundary_conditions`] *rejected* while the
    /// position was pinned at the edge — so a clamping physics, whose position
    /// never leaves range, still reports how hard the finger is pulling
    /// (`physics`' design ruling). Under a physics that rejects nothing
    /// (`Bouncing`, [`RubberBand`](crate::RubberBand)) this is exactly the
    /// overscroll, which is why basing the [`REFRESH_TRIGGER_PX`] check on it
    /// changes no trigger distance for either of them.
    ///
    /// Re-derived from [`ScrollWidget::drag_position`] on every drag move
    /// (never summed across moves, which would strand a rejected pull the
    /// finger has since eased back), re-seeded from the live displacement on
    /// `Down`, re-derived again on every ballistic pump, decayed by the
    /// release-settle, and zeroed by the wheel/`Cancel` hard clamps. A
    /// ballistic that ends with a pull outstanding hands it to that same settle
    /// ([`ScrollWidget::settle_ballistic_residual`]) rather than leaving it
    /// standing — the settle is the only path back to `0.0` that animates.
    pub(crate) edge_pull: f64,
    /// A generic ballistic simulation handed back by
    /// [`ScrollPhysics::create_ballistic_simulation`] on release, or `None` —
    /// always `None` under [`RubberBand`](crate::RubberBand), which keeps the
    /// legacy [`ScrollWidget::fling`]/[`ScrollWidget::settling`] path instead.
    ballistic: Option<BallisticState>,
    /// Velocity (px/s of offset) of the motion a new `Down` interrupted, fed to
    /// [`ScrollPhysics::carried_momentum`] at the next fling start. `Down` is
    /// its only writer and always writes it (`0.0` when the press landed on a
    /// resting surface), so it can never carry a stale value into a later
    /// gesture.
    carried_velocity: f64,
    /// A scroll notification produced by the paint-time fling/settle pump (which
    /// carries no [`EventCtx`]); delivered to `on_scroll` on the next event and
    /// cleared by a `Cancel` without firing.
    pending_scroll_notify: bool,
    /// The scroll observation callback (`None` if the view set none).
    on_scroll: Option<ErasedArgCallback<ScrollInfo>>,
    /// The pull-to-refresh release callback (`None` if the view set none).
    on_refresh_release: Option<ErasedCallback>,
    /// Whether a `Down` has armed an active gesture (distinct from `scrolling`,
    /// which only becomes true after the drag passes the slop). Set on `Down`
    /// and cleared on `Up`/`Cancel`; the slop/takeover math runs only while it
    /// is true, so a hover `Move` (dispatched on every cursor motion) is never
    /// mistaken for a drag and can never take the gesture from a child.
    down_active: bool,
    /// What the nearest nested scroll surface claimed it could do with this
    /// gesture, read back out of the claim cell this widget pushed around the
    /// `Down`'s forward — the whole input to the innermost-wins decision at the
    /// takeover site (the module docs' *Nested scrolling*). Reset on `Down`
    /// (before that forward) and on `Up`/`Cancel`, so it can never carry a
    /// previous gesture's answer.
    ///
    /// **Accepted staleness**: this is a `Down`-time snapshot. An inner
    /// revealed (scrolled off the edge it was pinned against) or re-pinned
    /// *during* the gesture never re-registers, and the decision taken from it
    /// is never revisited — the same limitation the navigator's edge-swipe
    /// claim accepts for exactly the same reason (one arbitration point per
    /// gesture, decided once).
    pub(crate) inner_at_down: InnerScrollState,
    /// Whether this gesture was handed to the nested surface at the takeover
    /// site. A **sticky** decision for the rest of the gesture: every remaining
    /// `Move` is forwarded untouched and the takeover math never runs again, so
    /// a mid-gesture direction reversal cannot steal the drag back (v1 — the
    /// finger is already inside the inner's own drag by then, and taking over
    /// would mean cancelling a scroll in flight). Cleared with
    /// [`ScrollWidget::inner_at_down`].
    pub(crate) deferring: bool,
    /// A live veto a nested multi-contact recognizer (`pinch_detector`,
    /// [`crate::pan_zoom`]'s child-owned-press branch) can raise for the rest
    /// of this gesture once it is tracking more than the claimant's own
    /// contact — the live counterpart to `inner_at_down` for a case the
    /// `Down`-time snapshot cannot see (the module docs' *Multi-contact
    /// veto*). Checked on every `Move` at the takeover site, not read once:
    /// the recognizer flips it live as a second contact joins and leaves.
    /// Replaced with a fresh, unset cell on every `Down` (and cleared again
    /// on `Up`/`Cancel`) so a stale recognizer handle from a previous gesture
    /// can never veto this one.
    live_veto: Rc<Cell<bool>>,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    /// Active fling velocity, in px/s of *offset* (opposite the finger), or
    /// `None` when not flinging.
    fling: Option<f64>,
    /// The most recent frame time observed during [`Widget::paint`], reused as the
    /// event-pass timestamp for velocity tracking (the event pass carries no clock
    /// of its own — time is provided only at paint; the fling starts from the
    /// last paint clock, which is today's behavior too).
    last_frame_time: FrameTime,
    /// Last animation frame time for the paint-time fling pump; `None` seeds the
    /// clock (zero-delta) on the first paint after a release.
    last_anim: Option<FrameTime>,
    /// This surface's claim on an attached [`ScrollController`], or `None`.
    /// Dropping it (with the widget, or on a rebuild that names no handle)
    /// detaches the handle when this surface still holds it.
    controller: Option<ScrollBinding>,
    /// Whether a layout has run, so the extent a controller command clamps
    /// against (and the snapshot it publishes) is a measured one rather than
    /// the pre-layout zero.
    laid_out: bool,
    /// Whether a controller jump moved the position since the last paint —
    /// the paint that sees it requests the `Housekeeping` flush that delivers
    /// the owed `on_scroll` (the module docs' *Programmatic scrolling*).
    jump_notify_owed: bool,
    /// Whether [`ScrollWidget::ballistic`] is currently running a
    /// controller-driven [`ScrollController::animate_to`] tween rather than a
    /// release fling/spring — this surface's own mirror of the attached
    /// [`ScrollController::is_animating`] flag, read back at every tear-down
    /// site ([`ScrollWidget::stop_ballistic`]) so the controller is told the
    /// instant it actually ends, and at [`ScrollWidget::drive_ballistic`]'s
    /// own completion so a tween that finishes on its own is told too.
    controller_animating: bool,
}

impl ScrollWidget {
    fn new(child: ChildPod) -> Self {
        Self {
            child,
            offset: 0.0,
            viewport: Size::ZERO,
            content: Size::ZERO,
            scrolling: false,
            drag_position: 0.0,
            settling: false,
            physics: default_physics(),
            effect: default_overscroll_effect(),
            edge_pull: 0.0,
            ballistic: None,
            carried_velocity: 0.0,
            pending_scroll_notify: false,
            on_scroll: None,
            on_refresh_release: None,
            down_active: false,
            inner_at_down: InnerScrollState::default(),
            deferring: false,
            live_veto: Rc::new(Cell::new(false)),
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
            controller: None,
            laid_out: false,
            jump_notify_owed: false,
            controller_animating: false,
        }
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The maximum scroll offset (`content − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content.height - self.viewport.height).max(0.0)
    }

    /// Whether post-release motion is in flight — the legacy fling, or a
    /// physics-supplied [`Simulation`] the generic driver is running (never
    /// both, and never either one under [`RubberBand`](crate::RubberBand)'s
    /// legacy-only path).
    pub fn is_flinging(&self) -> bool {
        self.fling.is_some() || self.ballistic.is_some()
    }

    /// This surface's extent/position snapshot for the physics, reading
    /// `pixels` from a caller-supplied position rather than the live offset —
    /// the drag path asks about the *raw* (un-resisted) drag position, clamped
    /// into range, so resistance is derived from the accumulator instead of
    /// compounding across moves.
    fn metrics_at(&self, pixels: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: self.max_offset(),
            viewport_dimension: self.viewport.height,
            device_pixel_ratio: METRICS_FALLBACK_DPR,
        }
    }

    /// This surface's extent/position snapshot at its current effective offset.
    fn metrics(&self) -> ScrollMetrics {
        self.metrics_at(self.offset)
    }

    /// The signed distance the effective offset currently sits past an edge
    /// (negative past the top, positive past the bottom, `0.0` in range) — the
    /// value [`ScrollInfo::overscroll`] reports and the allowed half of
    /// [`ScrollWidget::edge_pull`].
    fn displacement(&self) -> f64 {
        self.offset - self.offset.clamp(0.0, self.max_offset())
    }

    /// The velocity (px/s of offset) of whatever post-release motion is live
    /// right now — the legacy fling's own, or a running simulation's at the
    /// last painted frame — and `0.0` when the surface is at rest.
    fn live_velocity(&self) -> f64 {
        if let Some(v) = self.fling {
            return v;
        }
        match self.ballistic.as_ref() {
            Some(state) => state.sim.dx(state.elapsed_secs(self.last_frame_time)),
            None => 0.0,
        }
    }

    /// A new fling's starting velocity: the `release` velocity plus whatever
    /// [`ScrollPhysics::carried_momentum`] carries over from the motion this
    /// gesture's `Down` interrupted. `Down` is the only writer of
    /// [`ScrollWidget::carried_velocity`] (and always writes it), so the
    /// remembered value is never stale; [`RubberBand`](crate::RubberBand)
    /// carries `0.0`, leaving `release` untouched.
    ///
    /// **Momentum is carried only onto a release that plainly continues the
    /// interrupted motion**: same sign, and faster than
    /// [`MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR`] of the physics' own
    /// **mapped** share of the interrupted velocity —
    /// `physics.carried_momentum(carried)`, the exact value the release is
    /// about to add, not the raw interrupted speed. Flutter's two
    /// `ScrollDragController.end` guards compare against `carriedMomentum`
    /// the same way; `Bouncing`'s fitted power curve sits below the raw speed
    /// under ~1563 px/s and above it beyond, so gating on the mapped value
    /// (rather than the raw one) changes where the threshold actually sits.
    /// Both guards are load-bearing, not polish — the carried term is
    /// comparable in magnitude to an ordinary release, so adding it to a
    /// flick back the other way cancels the finger's own velocity out or
    /// reverses it outright.
    ///
    /// **Accepted gap**: Flutter drops the carried velocity a third way, when
    /// the finger held still before letting go (`_maybeLoseMomentum`); that
    /// guard is not ported. A press that stalls live motion, pauses, then
    /// releases slowly in the same direction still carries momentum here.
    ///
    /// The single carry site for both release branches — the generic
    /// ballistic driver ([`ScrollWidget::release_simulation`]) and the legacy
    /// fling — so neither can grow a rule of its own.
    fn fling_start_velocity(&self, release: f64) -> f64 {
        let mapped = self.physics.carried_momentum(self.carried_velocity);
        let continues_it = release.signum() == mapped.signum()
            && release.abs() > MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped.abs();
        if continues_it {
            release + mapped
        } else {
            release
        }
    }

    /// Ask the installed physics for post-release motion at the release
    /// velocity its own bounds allow: under
    /// [`ScrollPhysics::min_fling_velocity`] the release is not a fling at all
    /// (the physics may still want to spring an overscrolled surface back,
    /// just from rest), and over [`ScrollPhysics::max_fling_velocity`] it
    /// clamps. **Those bounds govern the generic driver only** — the legacy
    /// fling path keeps its own pinned [`FLING_STOP`] threshold and no upper
    /// clamp, so installing a physics that returns `None` here (as
    /// [`RubberBand`](crate::RubberBand) does) cannot change a single shipped
    /// fling.
    fn release_simulation(&self) -> Option<Box<dyn Simulation>> {
        // The offset moves opposite the finger, like every other release path.
        let released = self.fling_start_velocity(-self.tracker.velocity());
        let max = self.physics.max_fling_velocity();
        let velocity = if released.abs() < self.physics.min_fling_velocity() {
            0.0
        } else {
            released.clamp(-max, max)
        };
        self.physics
            .create_ballistic_simulation(&self.metrics(), velocity)
    }

    /// The last painted frame time as milliseconds — the event-pass timestamp
    /// source for velocity tracking (see [`ScrollWidget::last_frame_time`]).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    /// The offset the content is actually **painted** at, which is the live
    /// `offset` — past-edge displacement and all — only under
    /// [`OverscrollEffect::Translate`]. [`OverscrollEffect::Stretch`] paints
    /// the pull as a scale about the held edge instead, and
    /// [`OverscrollEffect::None`] paints it not at all, so both leave the
    /// content exactly where an in-range offset would put it.
    ///
    /// Subtracted back off rather than never computed: the offset itself still
    /// moves precisely as the physics dictates under every effect (the module
    /// docs' *Overscroll visuals*), so only this one read differs.
    fn painted_offset(&self) -> f64 {
        match self.effect {
            OverscrollEffect::Translate => self.offset,
            OverscrollEffect::Stretch | OverscrollEffect::None => self.offset - self.displacement(),
        }
    }

    fn sync_child_origin(&mut self) {
        self.child
            .set_origin(Point::new(0.0, -self.painted_offset()));
    }

    /// A snapshot of the current scroll position for [`ScrollView::on_scroll`]:
    /// the clamped `offset`, the `max_offset`, and the signed past-edge
    /// `overscroll` (negative past the top). See [`ScrollInfo`].
    fn scroll_info(&self) -> ScrollInfo {
        let max = self.max_offset();
        ScrollInfo {
            offset: self.offset.clamp(0.0, max),
            max_offset: max,
            overscroll: self.displacement(),
        }
    }

    /// Fire `on_scroll` (if set) with the current [`ScrollInfo`]. Called from the
    /// event pass after an input-driven offset/overscroll change.
    fn notify_scroll(&mut self, ctx: &mut EventCtx) {
        let info = self.scroll_info();
        if let Some(cb) = self.on_scroll.as_mut() {
            cb(ctx, info);
        }
    }

    /// Deliver a fling/settle notification recorded at paint time (which had no
    /// [`EventCtx`]) on the next event — one event of latency, the same
    /// controlled-component convention the fling clock already relies on.
    fn deliver_pending_scroll(&mut self, ctx: &mut EventCtx) {
        if self.pending_scroll_notify {
            self.pending_scroll_notify = false;
            self.notify_scroll(ctx);
        }
    }

    /// Advance the drag by `delta` px of raw finger travel in offset space
    /// (positive = the content scrolls down), asking the physics what that
    /// delta is worth from where the surface actually sits — the module docs'
    /// *Drag convention*.
    ///
    /// Three steps, in order: the physics maps the raw delta at metrics
    /// reporting the **live** position (so a depth-aware curve reads a real
    /// overscroll depth); the mapped delta accumulates into
    /// [`ScrollWidget::drag_position`]; and the boundary rule decides how much
    /// of that accumulated position the surface may actually hold. What it
    /// rejects never reaches `offset` but is still reported through
    /// [`ScrollWidget::edge_pull`], which is what lets a clamping surface drive
    /// pull-to-refresh and a stretch effect.
    fn apply_drag_offset(&mut self, delta: f64) {
        let metrics = self.metrics_at(self.offset);
        let mapped = self.physics.apply_physics_to_user_offset(&metrics, delta);
        self.drag_position += mapped;
        let rejected = self
            .physics
            .apply_boundary_conditions(&metrics, self.drag_position);
        self.offset = self.drag_position - rejected;
        // Read the allowed half back off the offset rather than reusing the
        // term above, so this is the *same* number `scroll_info` reports and
        // the two can never disagree by a rounding step at the bottom edge.
        self.edge_pull = self.displacement() + rejected;
    }

    /// Advance a release-settle by `dt_ms`, easing the effective `offset` back to
    /// its clamped edge and returning whether it is still animating. Pure and
    /// deterministic (mirrors [`ScrollWidget::tick`]); the paint pump and the
    /// tests both drive it.
    pub fn settle_tick(&mut self, dt_ms: f64) -> bool {
        if !self.settling {
            return false;
        }
        let max = self.max_offset();
        let target = self.offset.clamp(0.0, max);
        let remaining = target - self.offset;
        // `edge_pull`'s boundary-rejected half (always `0.0` under
        // `RubberBand`, where the pull *is* the displacement) has no position
        // to ride back, so it decays on the same curve of its own — otherwise
        // a clamping physics' stretch would snap off at release.
        let rejected = self.edge_pull - self.displacement();
        if remaining.abs() <= SETTLE_STOP_PX && rejected.abs() <= SETTLE_STOP_PX {
            self.offset = target;
            self.settling = false;
            self.edge_pull = 0.0;
            self.sync_child_origin();
            return false;
        }
        let retained = SETTLE_DECAY.powf(dt_ms);
        self.offset = target - remaining * retained;
        self.edge_pull = self.displacement() + rejected * retained;
        self.sync_child_origin();
        true
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// animating. Pure and deterministic — the paint-time pump and the tests
    /// both drive it.
    pub fn tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        self.sync_child_origin();
        let next_v = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next_v.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next_v);
            true
        }
    }

    /// Whether the running simulation can no longer move anything on screen:
    /// the position it proposed sits outside the range, the physics rejected
    /// **all** of that excess (so the painted offset is already pinned at the
    /// edge), and the curve is still travelling further out. Nothing it reports
    /// after that can reach the offset, so the driver ends it here rather than
    /// asking the shell for the rest of the spline's worth of frames — an
    /// Android-style fling into an edge otherwise pumps a second of them with
    /// the surface stock-still.
    ///
    /// **Deliberately conservative.** A *partial* rejection (a physics holding
    /// some of the excess) and an inward velocity each keep the simulation
    /// running, because either can still bring the position back in range: an
    /// edge spring released outward crosses back within a few frames, and only
    /// the fully-rejected-and-still-outward case is one-way for every curve in
    /// [`crate::physics::simulation`]. The comparison against the raw excess is
    /// exact rather than tolerant for the same reason — a physics whose
    /// rejection merely rounds to the excess keeps the old pump-to-done
    /// behavior instead of being guessed at.
    fn ballistic_is_pinned_outward(&self, proposed: f64, rejected: f64, velocity: f64) -> bool {
        let excess = proposed - proposed.clamp(0.0, self.max_offset());
        excess != 0.0 && rejected == excess && velocity * excess > 0.0
    }

    /// Hand a residual [`ScrollWidget::edge_pull`] left behind by a finished
    /// ballistic to the release-settle, so it decays on [`SETTLE_DECAY`]
    /// exactly as a drag release's pull does instead of standing on screen
    /// until the next `Down`/wheel/`Cancel`. Only the *rejected* half can be
    /// left over — the offset is wherever the simulation put it — and
    /// [`ScrollWidget::settle_tick`] already decays that half on its own curve.
    ///
    /// A simulation that ends in range leaves nothing to settle and this is a
    /// no-op: the guard is [`SETTLE_STOP_PX`], the same distance `settle_tick`
    /// itself calls settled, so a bouncing spring's sub-pixel float residue
    /// never arms an animation that would stop on its first tick.
    fn settle_ballistic_residual(&mut self) {
        if self.edge_pull.abs() > SETTLE_STOP_PX {
            self.settling = true;
        }
    }

    /// End any live [`ScrollWidget::ballistic`] simulation — a release
    /// fling/spring, or a controller [`ScrollController::animate_to`] tween —
    /// telling the attached controller the animation stopped if it was one.
    /// The single tear-down path for every place user input or a jump/animate
    /// interrupts motion (`Down`, wheel, `Cancel`, [`ScrollWidget::apply_jump`],
    /// [`ScrollWidget::apply_animate_to`], and [`ScrollWidget::drive_ballistic`]'s
    /// own completion all call this instead of assigning `self.ballistic =
    /// None` directly). **Every start goes through it too**: the `Up` release
    /// clears the fling and settle and calls this before it starts a
    /// simulation, settle or fling of its own, and a controller tween is only
    /// ever installed by [`ScrollWidget::apply_animate_to`] after the same
    /// tear-down — so a fling, a settle and a simulation are never live at
    /// once, and [`ScrollController::is_animating`] cannot outlive the tween
    /// it reports. (A tween whose surface is dropped or displaced mid-flight
    /// is the handle's side of the contract, not this widget's.)
    fn stop_ballistic(&mut self) {
        self.ballistic = None;
        if std::mem::take(&mut self.controller_animating)
            && let Some(binding) = self.controller.as_ref()
        {
            binding.set_animating(false);
        }
    }

    /// End a live drag because a controller command just took the position
    /// over — Flutter's `animateTo`/`jumpTo` semantics, where a programmatic
    /// command replaces the user's drag rather than running alongside it (the
    /// module docs' *Programmatic scrolling*). Resets exactly the per-gesture
    /// state the `Down` arm resets (the drag/armed flags, the Down-time claim
    /// snapshot, the sticky defer decision and the live multi-contact veto
    /// cell), so the rest of the gesture's `Move`s take the unarmed
    /// child/hover path and its `Up` the non-scrolling child-forward branch —
    /// the finger can no longer write the offset and the release
    /// starts no fling or settle. A no-op when no gesture is armed.
    ///
    /// Pointer capture is deliberately left alone: the drain that calls this
    /// runs in layout/paint, which carry no [`EventCtx`], and the shell clears
    /// capture on the physical `Up`/`Cancel` whatever this flag says. The
    /// caller re-seeds [`ScrollWidget::drag_position`] from the position it
    /// applies.
    fn end_live_drag(&mut self) {
        if !self.scrolling && !self.down_active {
            return;
        }
        self.scrolling = false;
        self.down_active = false;
        self.inner_at_down = InnerScrollState::default();
        self.deferring = false;
        self.live_veto = Rc::new(Cell::new(false));
    }

    /// Advance a physics-supplied [`Simulation`] to frame time `now`: the
    /// position it reports, minus whatever
    /// [`ScrollPhysics::apply_boundary_conditions`] rejects of it. Subtracting
    /// the rejection is what keeps the driver honest for **any** physics — a
    /// clamping one can never paint an out-of-range offset even if its
    /// simulation overshoots, while a bouncing one (rejecting nothing) is free
    /// to run past the edge and back.
    ///
    /// Clears the simulation once it reports itself done — or once it is
    /// [pinned outward](ScrollWidget::ballistic_is_pinned_outward) and can
    /// never move the offset again — which is what stops the pump asking for
    /// frames, and hands any pull the rejection left behind to the settle
    /// ([`ScrollWidget::settle_ballistic_residual`]).
    fn drive_ballistic(&mut self, now: FrameTime) {
        let Some((proposed, velocity, done)) = self.ballistic.as_ref().map(|state| {
            let t = state.elapsed_secs(now);
            (state.sim.x(t), state.sim.dx(t), state.sim.is_done(t))
        }) else {
            return;
        };
        let rejected = self
            .physics
            .apply_boundary_conditions(&self.metrics(), proposed);
        self.offset = proposed - rejected;
        self.edge_pull = self.displacement() + rejected;
        self.sync_child_origin();
        if done || self.ballistic_is_pinned_outward(proposed, rejected, velocity) {
            self.stop_ballistic();
            self.settle_ballistic_residual();
        }
    }

    /// Advance the ballistic simulation, the legacy fling, *or* the
    /// release-settle by the delta since the last paint, and signal
    /// [`PaintCtx::request_frame`] while any of them is still running so the
    /// shell keeps scheduling frames (the desktop `ControlFlow::Wait` loop
    /// would otherwise idle). A fling stops once [`ScrollWidget::tick`] brings it
    /// to rest (`|velocity|` below [`FLING_STOP`], or a scroll bound reached); a
    /// settle stops once [`ScrollWidget::settle_tick`] reaches the edge; a
    /// simulation stops when it reports itself done or is pinned outward.
    /// Because this path carries no [`EventCtx`], an offset change here records
    /// a pending `on_scroll` notification delivered on the next event.
    ///
    /// The three are mutually exclusive at any one instant — a release picks
    /// one, after tearing every one of them down through
    /// [`ScrollWidget::stop_ballistic`], and a controller command ends the drag
    /// a release would come from — though a simulation ending against an edge
    /// hands the pull it left behind to the settle for the frames after it
    /// ([`ScrollWidget::settle_ballistic_residual`]). Under
    /// [`RubberBand`](crate::RubberBand) the simulation arm is never taken at
    /// all.
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if self.fling.is_none() && !self.settling && self.ballistic.is_none() {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        // A simulation measures time from its own start, so the first pump
        // after the release seeds it — the same zero-delta seeding frame
        // `last_anim` takes, so neither clock ever jumps on frame one.
        if let Some(state) = self.ballistic.as_mut() {
            state.start.get_or_insert(now);
        }
        if dt > 0.0 {
            if self.ballistic.is_some() {
                self.drive_ballistic(now);
            } else if self.fling.is_some() {
                self.tick(dt);
            } else {
                self.settle_tick(dt);
            }
            // The offset moved from a non-input source — record a notification the
            // next event delivers (the paint pass has no EventCtx to fire it now).
            self.pending_scroll_notify = true;
        }
        // While any animation is still in flight, ask the shell for another
        // frame to continue it.
        if self.fling.is_some() || self.settling || self.ballistic.is_some() {
            ctx.request_frame();
        }
    }

    /// Reconcile the attached [`ScrollController`] with the one a view names
    /// (see [`ScrollView::controller`]'s build/rebuild semantics), returning
    /// whether the binding changed.
    fn attach_controller(&mut self, requested: Option<&ScrollController>) -> bool {
        match (requested, self.controller.as_ref()) {
            (None, None) => false,
            (None, Some(_)) => {
                self.controller = None;
                true
            }
            (Some(handle), Some(binding)) if binding.controller().same(handle) => {
                if binding.is_current() || handle.is_attached() {
                    false
                } else {
                    self.controller = Some(handle.bind());
                    true
                }
            }
            (Some(handle), _) => {
                self.controller = Some(handle.bind());
                true
            }
        }
    }

    /// Apply every command queued on the attached [`ScrollController`], in
    /// recording order — once this surface has laid out (a command queued
    /// earlier waits for the first measured extent) and only while it still
    /// holds the handle.
    fn drain_controller(&mut self, reduce_motion: bool) {
        if !self.laid_out {
            return;
        }
        let commands = match self.controller.as_ref() {
            Some(binding) => binding.take_commands(),
            None => return,
        };
        for command in commands {
            match command {
                ScrollCommand::JumpTo(target) => self.apply_jump(target),
                ScrollCommand::AnimateTo(target, options) => {
                    self.apply_animate_to(target, options, reduce_motion);
                }
            }
        }
    }

    /// Move straight to `target`, clamped to `[0, max_offset]`, ending any
    /// fling, release-settle or ballistic simulation (and the pull it rode) in
    /// flight. A live drag ends here ([`ScrollWidget::end_live_drag`]) — the
    /// rest of its gesture never moves the content. `NaN` is ignored.
    fn apply_jump(&mut self, target: f64) {
        if target.is_nan() {
            return;
        }
        self.end_live_drag();
        let before = self.scroll_info();
        let was_moving = self.is_flinging() || self.settling;
        self.fling = None;
        self.stop_ballistic();
        self.settling = false;
        self.edge_pull = 0.0;
        self.last_anim = None;
        self.set_offset(target);
        self.drag_position = self.offset;
        self.sync_child_origin();
        if was_moving || self.scroll_info() != before {
            self.pending_scroll_notify = true;
            self.jump_notify_owed = true;
        }
    }

    /// Start (or replace) a controller-driven tween to `target`, clamped to
    /// `[0, max_offset]`, ending any fling, release-settle or ballistic
    /// simulation in flight first — the same replace semantics
    /// [`ScrollWidget::apply_jump`] gives a jump. Drives the tween through a
    /// [`TweenSimulation`] in [`ScrollWidget::ballistic`], the exact same
    /// field and pump ([`ScrollWidget::drive_ballistic`]) a release fling
    /// runs through, so paint cadence, `request_frame` and the installed
    /// physics' boundary handling are shared rather than duplicated.
    /// `reduce_motion` (or a non-positive/non-finite `duration_ms`) collapses
    /// this to an immediate [`ScrollWidget::apply_jump`]. A live drag ends
    /// here ([`ScrollWidget::end_live_drag`]), so the tween is the only writer
    /// of the offset until it finishes or user input interrupts it. `NaN` is
    /// ignored.
    fn apply_animate_to(&mut self, target: f64, options: AnimateTo, reduce_motion: bool) {
        if target.is_nan() {
            return;
        }
        self.end_live_drag();
        self.fling = None;
        self.stop_ballistic();
        self.settling = false;
        self.edge_pull = 0.0;
        self.last_anim = None;
        if reduce_motion || !options.duration_ms.is_finite() || options.duration_ms <= 0.0 {
            self.apply_jump(target);
            return;
        }
        self.drag_position = self.offset;
        let clamped = target.clamp(0.0, self.max_offset());
        let sim = TweenSimulation::new(
            self.offset,
            clamped,
            options.duration_ms / 1000.0,
            options.curve,
        );
        self.ballistic = Some(BallisticState {
            sim: Box::new(sim),
            start: None,
        });
        self.controller_animating = true;
        if let Some(binding) = self.controller.as_ref() {
            binding.set_animating(true);
        }
    }

    /// Publish the current position to the attached [`ScrollController`] (a
    /// no-op before the first layout, or once another surface took it over).
    fn publish_to_controller(&self) {
        if !self.laid_out {
            return;
        }
        if let Some(binding) = self.controller.as_ref() {
            binding.publish(self.scroll_info(), self.viewport.height);
        }
    }

    fn send_child_cancel(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        self.child.event_child(ctx, &cancel);
    }

    /// The event entry, parameterised on an explicit timestamp so velocity math
    /// is deterministic in tests; [`Widget::event`] supplies the real clock.
    /// Routes the event, then publishes whatever position it left behind to an
    /// attached [`ScrollController`].
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        let result = self.route_event_at(ctx, event, t_ms);
        self.publish_to_controller();
        result
    }

    /// The event body behind [`ScrollWidget::event_at`].
    fn route_event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // A fling/settle notification recorded at paint time is delivered on the
        // next event — except a Cancel, which clears it without firing (below).
        if !matches!(
            event,
            InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel
        ) {
            self.deliver_pending_scroll(ctx);
        }
        match event {
            // A broadcast is not user input: it bypasses the whole gesture
            // machinery, reaches the child whether or not it is focused or
            // captured, and is never consumed (`crate::authoring::route_event`'s
            // contract, applied to this widget's hand-rolled routing). A floated
            // surface's own input travels the same way — it has to reach an
            // overlay owner anywhere below this viewport, and a scroll gesture
            // must never swallow it.
            InputEvent::Housekeeping | InputEvent::Overlay(_) => {
                self.child.event_child(ctx, event);
                EventResult::Ignored
            }
            // Focus-routed events (Key/Ime, and the clipboard verbs an
            // `EditCommand` carries) bypass the scroll gesture machinery and go
            // straight to the child if it holds the recorded focus path.
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::EditCommand(_) => {
                if self.child.is_focused() {
                    self.child.event_child(ctx, event)
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Scroll { delta, .. } => {
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                // Wheel scrolling stays hard-clamped — no overscroll rubber-band on
                // desktop wheel input, and no physics consulted: the clamp is a
                // property of the input device, not of the installed feel, so
                // this arm is identical under every physics.
                self.fling = None;
                self.settling = false;
                self.stop_ballistic();
                self.edge_pull = 0.0;
                self.set_offset(self.offset + dy);
                self.sync_child_origin();
                self.notify_scroll(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    // Only a primary press arms a drag. A secondary press is a
                    // context gesture: it still reaches the child (a
                    // context-menu consumer inside the viewport must see it),
                    // but opens no capture and can never start a scroll. The
                    // wheel arm above is unaffected — it carries no button.
                    if !presses(p) {
                        return self.child.event_child(ctx, event);
                    }
                    self.scrolling = false;
                    self.down_active = true;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    // A fresh, unset cell for this gesture — never the
                    // previous one, which a since-torn-down recognizer may
                    // still hold a clone of.
                    self.live_veto = Rc::new(Cell::new(false));
                    // Remember what this press interrupted before killing it —
                    // the next fling asks the physics how much of it to carry
                    // forward (`0.0` under `RubberBand`, i.e. start cold).
                    self.carried_velocity = self.live_velocity();
                    self.fling = None;
                    self.settling = false;
                    self.stop_ballistic();
                    // A `Down` deliberately leaves a mid-bounce displacement on
                    // screen (the regrab continues from it), so the pull is
                    // re-seeded from that displacement rather than zeroed —
                    // what "reset" means here is "carries nothing stale from
                    // the previous gesture".
                    self.edge_pull = self.displacement();
                    self.last_anim = None;
                    self.down_start = p.position;
                    self.last_drag = p.position;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    // Innermost-wins arbitration, both halves in dispatch
                    // order. First report THIS surface into whatever cell is
                    // ambient — the nearest *enclosing* scrollable's, if any —
                    // while that is still the cell on top; only then push this
                    // surface's own cell for the forward below, so a nested
                    // scrollable's write lands here and never in the
                    // grandparent's.
                    if let Some(host) = ambient_scroll_claim() {
                        host.set(inner_claim_state(self.physics.as_ref(), &self.metrics()));
                    }
                    let claim = Rc::new(Cell::new(InnerScrollState::default()));
                    let veto = Rc::clone(&self.live_veto);
                    let child = &mut self.child;
                    with_scroll_claim(&claim, || {
                        with_scroll_veto(&veto, || child.event_child(ctx, event))
                    });
                    self.inner_at_down = claim.get();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    // Without an armed `Down`, this is a hover move (the desktop
                    // shell dispatches `Move` on every cursor motion): never run
                    // the slop/takeover math against a stale `down_start`, just
                    // forward it to the child.
                    if !self.down_active {
                        return self.child.event_child(ctx, event);
                    }
                    self.tracker.record(t_ms, p.position.y);
                    if self.scrolling {
                        let dy = p.position.y - self.last_drag.y;
                        self.last_drag = p.position;
                        // Hand the physics this move's raw delta (the offset
                        // moves opposite the finger) and let it decide what
                        // reaches the position past an edge.
                        self.apply_drag_offset(-dy);
                        self.sync_child_origin();
                        self.notify_scroll(ctx);
                        ctx.request_redraw();
                    } else if !self.deferring
                        && !self.live_veto.get()
                        && (p.position.y - self.down_start.y).abs() > TOUCH_SLOP
                        && self.physics.should_accept_user_offset(&self.metrics())
                    {
                        if self.inner_at_down.defers(p.position.y - self.down_start.y) {
                            // Innermost wins: a nested scrollable registered on
                            // this gesture's `Down` and can consume this
                            // direction, so take nothing over — no `Cancel`, no
                            // capture handover — and keep forwarding. The
                            // decision is sticky (`deferring` gates this whole
                            // branch), and the inner's own slop machinery
                            // cancels its own child from here.
                            self.deferring = true;
                            self.child.event_child(ctx, event);
                        } else {
                            // Take the gesture over: cancel the child, stop
                            // forwarding — unless the physics refuses drags
                            // outright, in which case the move keeps flowing to
                            // the child. `Bouncing`/`RubberBand` accept
                            // unconditionally (even content that fits
                            // rubber-bands), so that gate is inert on the
                            // bouncing-family default.
                            self.scrolling = true;
                            self.settling = false;
                            self.last_drag = p.position;
                            // Seed the drag accumulator from the live position
                            // — including a mid-bounce displacement, so a
                            // regrab continues from what is on screen.
                            self.drag_position = self.offset;
                            self.send_child_cancel(ctx, p.position);
                            // A takeover, not the gesture's end: releasing the
                            // child through the context also ends a contact
                            // opt-in held below it, so the root stops routing
                            // other fingers to a captor that was just cancelled.
                            ctx.release_captured_child(&mut self.child);
                            ctx.request_redraw();
                        }
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.scrolling {
                        // Pull-to-refresh: released past the top trigger fires the
                        // app hook (an Up, so mutating state is allowed). Measured
                        // on `edge_pull`, so a clamping physics — which never lets
                        // the position leave range — can still trigger it; under
                        // `RubberBand` this is bit-for-bit the overscroll the
                        // check has always read.
                        if crossed_refresh_trigger(self.edge_pull)
                            && let Some(cb) = self.on_refresh_release.as_mut()
                        {
                            cb(ctx);
                        }
                        // Whatever the release starts, it starts alone: tear
                        // every motion down first, through the single
                        // tear-down path, so no fling, settle or simulation
                        // (a controller tween included) survives alongside the
                        // one picked below and `is_animating` is never left
                        // set by a tween this release replaced.
                        self.fling = None;
                        self.settling = false;
                        self.stop_ballistic();
                        self.last_anim = None;
                        // Ask the physics for post-release motion first: one that
                        // hands back a simulation owns the release outright, and
                        // one that does not (`RubberBand`) falls through to the
                        // legacy settle/fling below untouched.
                        if let Some(sim) = self.release_simulation() {
                            self.ballistic = Some(BallisticState { sim, start: None });
                        } else if self.edge_pull != 0.0 {
                            // Released while overscrolled: settle back to the edge,
                            // never fling out of range.
                            self.settling = true;
                        } else {
                            // The legacy path keeps its own FLING_STOP threshold
                            // (the trait's min/max fling bounds govern the generic
                            // driver only) and takes carried momentum, which is
                            // `0.0` under `RubberBand`.
                            let finger_v = self.tracker.velocity();
                            if finger_v.abs() > FLING_STOP {
                                // Offset moves opposite the finger.
                                self.fling = Some(self.fling_start_velocity(-finger_v));
                            }
                        }
                        self.notify_scroll(ctx);
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    self.live_veto = Rc::new(Cell::new(false));
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    self.live_veto = Rc::new(Cell::new(false));
                    // Cancel never mutates state and never fires a callback: drop
                    // any pending notification and snap an overscrolled surface back
                    // into range (no settle animation, no on_scroll/on_refresh) —
                    // including any live simulation and the pull it was riding.
                    self.settling = false;
                    self.stop_ballistic();
                    self.edge_pull = 0.0;
                    self.pending_scroll_notify = false;
                    self.set_offset(self.offset);
                    self.sync_child_origin();
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            // Any other variant — a hit-tested `Scale` today — belongs to the
            // child, and so does its result: a child that handled it consumed
            // it, so an enclosing recognizer (a `pinch_detector` offered the
            // same gesture next) must not act on it a second time. Only a
            // broadcast is never consumed, whatever the child returned; the arm
            // above catches both today, and the check keeps that true for a
            // broadcast variant added later.
            _ => {
                let result = self.child.event_child(ctx, event);
                if event.is_broadcast() {
                    EventResult::Ignored
                } else {
                    result
                }
            }
        }
    }
}

impl<State: 'static> View<State> for ScrollView<State> {
    type Element = ScrollWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollWidget {
        let mut widget = ScrollWidget::new(crate::authoring::build_child(&self.child, ctx));
        widget.on_scroll = self
            .on_scroll
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        widget.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        if let Some(physics) = self.physics.clone() {
            widget.physics = physics;
        }
        widget.effect = self.effect;
        widget.attach_controller(self.controller.as_ref());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapters.
        element.on_scroll = self
            .on_scroll
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        element.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        // A `.physics(...)`-carrying view reinstalls it every rebuild, like the
        // erased callbacks above; a view with no opinion (`None`) leaves the
        // widget's currently-installed physics alone — see
        // `ScrollView::physics`'s doc for the full contract.
        if let Some(physics) = self.physics.clone() {
            element.physics = physics;
        }
        // The visual effect is plain data the view owns and is always carried
        // down unconditionally.
        element.effect = self.effect;
        let mut flags =
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        // A new binding publishes, and a queued command applies, at the next
        // layout or paint — make sure a frame-gated shell runs one.
        let rebound = element.attach_controller(self.controller.as_ref());
        if rebound
            || element
                .controller
                .as_ref()
                .is_some_and(ScrollBinding::has_pending)
        {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ScrollWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ScrollWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // The child is laid out at the viewport width with unbounded height.
        let child_bc = BoxConstraints::new(Size::new(vw, 0.0), Size::new(vw, f64::INFINITY));
        self.content = self.child.layout_child(ctx, &child_bc);
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            self.content.height
        };
        self.viewport = Size::new(vw, vh);
        // Invariant: an in-flight gesture/settle/simulation owns an
        // out-of-range offset; layout must not snap it. While `scrolling` (an
        // active past-slop drag), `settling` (the post-release decay back to
        // the edge), or a physics-driven ballistic simulation (which for a
        // bouncing physics legitimately runs past an edge and back) is live,
        // `self.offset` legitimately carries the resisted past-edge overscroll —
        // re-clamping it here would snap the child to rest mid-gesture, and the
        // next pointer `Move` (or settle tick) would re-apply the displacement,
        // producing a visible alternation between rest and dragged positions at
        // display rate on a page where something else requests layout every
        // frame (device-gate G6, the "phantom clone" bug). Only the clamp is
        // conditional: origin sync and the viewport/content bookkeeping above
        // still run unconditionally either way. A resize mid-drag (content or
        // viewport shrinking under an out-of-range offset) still resolves
        // correctly without an immediate clamp here: `Up`'s handler always
        // recomputes `scroll_info()`/settles/flings off the freshly-updated
        // `max_offset`, and any non-drag layout after the gesture ends clamps
        // normally on its own next pass.
        if !self.scrolling && !self.settling && self.ballistic.is_none() {
            self.set_offset(self.offset); // re-clamp against new content/viewport
        }
        // A controller command clamps against the extent just measured, so a
        // jump recorded alongside a content change lands in the new range.
        self.laid_out = true;
        let reduce_motion = Theme::from_layout_ctx(ctx)
            .map(|t| t.motion.reduce_motion)
            .unwrap_or(false);
        self.drain_controller(reduce_motion);
        self.sync_child_origin();
        self.publish_to_controller();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the shared frame clock so the between-frames event pass (which
        // carries no clock) has a timestamp for velocity tracking.
        self.last_frame_time = ctx.frame_time();
        // A command recorded since layout (or with layout skipped this frame)
        // applies before the pump, so a jump ends a fling before it advances.
        let reduce_motion = Theme::from_paint_ctx(ctx)
            .map(|t| t.motion.reduce_motion)
            .unwrap_or(false);
        self.drain_controller(reduce_motion);
        self.pump_fling(ctx);
        self.publish_to_controller();
        if std::mem::take(&mut self.jump_notify_owed) && self.on_scroll.is_some() {
            // Paint has no `&mut State`: owe the jump's `on_scroll` to the next
            // frame's Housekeeping flush (the `pan_zoom` precedent).
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
        if self
            .controller
            .as_ref()
            .is_some_and(ScrollBinding::has_pending)
        {
            // Recorded during this paint (a listener reacting to the publish
            // above): the next frame applies it.
            ctx.request_frame();
        }
        scene.push_clip(ctx.origin(), ctx.size());
        self.sync_child_origin();
        // Publish this viewport as the paint-time visible rect (absolute coords),
        // so a `Flex` in the scrolled content can cull children fully below/above
        // the fold — suppressing offscreen animators' frame requests. Intersects
        // (never widens) any rect an outer scroll surface already threaded down.
        ctx.constrain_visible_rect(Rect::from_origin_size(ctx.origin(), ctx.size()));
        // The stretch is PAINT-ONLY, and load-bearingly so: no layout pass
        // reads `edge_pull` or the intensity derived from it, and none may
        // start to. A layout-affecting animation must request a relayout on
        // every frame of its motion or the mobile shell's intra-frame layout
        // skip leaves it frozen (`docs/WIDGETS_CODE_STANDARDS.md`'s
        // animation-pacing rule) — keeping the stretch out of every layout
        // read is what makes that irrelevant here, and is why there is no
        // `request_layout` in this path either: the settle/ballistic pump
        // above already asks for every frame the decaying stretch needs.
        // Pushed INSIDE the viewport clip so stretched content can never
        // paint past the viewport's edges.
        let stretch = match self.effect {
            OverscrollEffect::Stretch => {
                stretch_about_edge(ctx.origin(), ctx.size(), self.edge_pull)
            }
            OverscrollEffect::Translate | OverscrollEffect::None => None,
        };
        if let Some(transform) = stretch {
            scene.push_transform(transform);
        }
        self.child.paint_child(ctx, scene);
        if stretch.is_some() {
            scene.pop_transform();
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A ScrollView node exposing the vertical scroll offset and its range,
        // wrapping its scrolled content: `semantics_child` translates by the
        // child's origin (which carries `-offset`), so descendant bounds reflect
        // the scrolled position.
        let max_offset = (self.content.height - self.viewport.height).max(0.0);
        ctx.push_container(
            Role::ScrollView,
            |node| {
                node.set_scroll_y(self.offset);
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(max_offset);
            },
            |ctx| self.child.semantics_child(ctx),
        );
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::parity::{Bouncing, Clamping, DecelerationRate, NeverScrollable};
    use crate::physics::rubber_band::RubberBand;
    use crate::test_support::leaf;
    use std::any::Any;

    /// Build and lay out a scroll widget over `()` state with a `content_h`-tall
    /// leaf child inside a `vw`×`vh` viewport.
    fn laid_out(vw: f64, vh: f64, content_h: f64) -> ScrollWidget {
        let view: ScrollView<()> = scroll_view(leaf(vw, content_h));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(vw, vh)));
        w
    }

    /// [`laid_out`] with the pre-seam [`RubberBand`] feel pinned explicitly —
    /// the fixture every *rubber-band* pin below builds from now that the
    /// widget's own default is the platform-adaptive physics (bouncing here,
    /// clamping on Android), each with curves of its own. A test whose
    /// assertions are a `0.5`-resisted displacement, a `SETTLE_DECAY` trace or
    /// a legacy-fling trace is pinning *this* physics, not the default.
    fn laid_out_rubber_band(vw: f64, vh: f64, content_h: f64) -> ScrollWidget {
        let mut w = laid_out(vw, vh, content_h);
        w.physics = Rc::new(RubberBand::new());
        w
    }

    fn scroll(y: f64, lines: bool, amount: f64) -> InputEvent {
        let delta = if lines {
            ScrollDelta::Lines(0.0, amount)
        } else {
            ScrollDelta::Pixels(0.0, amount)
        };
        InputEvent::Scroll {
            position: Point::new(10.0, y),
            delta,
        }
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ScrollWidget, event: &InputEvent, t_ms: f64) {
        let mut unit = ();
        let state_any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, event, t_ms);
    }

    #[test]
    fn max_offset_is_content_minus_viewport() {
        let w = laid_out(200.0, 100.0, 1000.0);
        assert_eq!(w.max_offset(), 900.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn wheel_scrolls_and_clamps_with_no_overscroll() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // 3 lines * 40 px = 120.
        dispatch(&mut w, &scroll(50.0, true, 3.0), 0.0);
        assert_eq!(w.offset(), 120.0);
        // A huge line delta clamps to max_offset (no overscroll past the end).
        dispatch(&mut w, &scroll(50.0, true, 100.0), 0.0);
        assert_eq!(w.offset(), 900.0);
        // Scrolling back past the top clamps to 0.
        dispatch(&mut w, &scroll(50.0, false, -5000.0), 0.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn drag_past_slop_scrolls_the_offset() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        // First move crosses the slop → takeover (no scroll on this move).
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(w.offset(), 0.0);
        assert!(w.scrolling);
        // Next move drags the finger up 30 px → content scrolls down 30 px.
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(w.offset(), 30.0);
    }

    #[test]
    fn takeover_sends_the_child_a_cancel() {
        // A child that records the pointer phases it receives.
        #[derive(Default)]
        struct Rec {
            downs: u32,
            cancels: u32,
        }
        struct Probe;
        struct ProbeW;
        impl View<Rec> for Probe {
            type Element = ProbeW;
            fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
                ProbeW
            }
            fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }
        impl Widget for ProbeW {
            fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(200.0, 1000.0))
            }
            fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
                if let InputEvent::Pointer(p) = e {
                    let rec = ctx.state_mut::<Rec>();
                    match p.phase {
                        PointerPhase::Down => rec.downs += 1,
                        PointerPhase::Cancel => rec.cancels += 1,
                        _ => {}
                    }
                }
                EventResult::Handled
            }
        }

        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);

        let mut state = Rec::default();
        let run = |w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64| {
            let sa: &mut dyn Any = state;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
            w.event_at(&mut ctx, e, t);
        };
        run(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        // Drag 30 px past the slop → takeover fires a Cancel at the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(state.downs, 1);
        assert_eq!(state.cancels, 1);
        // Subsequent scrolling moves are not forwarded to the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 50.0), 32.0);
        assert_eq!(state.cancels, 1);
    }

    /// A recording child probe, shared by the takeover/hover tests.
    #[derive(Default)]
    struct Rec {
        downs: u32,
        cancels: u32,
        moves: u32,
    }
    struct Probe;
    struct ProbeW;
    impl View<Rec> for Probe {
        type Element = ProbeW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
            ProbeW
        }
        fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 1000.0))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e {
                let rec = ctx.state_mut::<Rec>();
                match p.phase {
                    PointerPhase::Down => rec.downs += 1,
                    PointerPhase::Cancel => rec.cancels += 1,
                    PointerPhase::Move => rec.moves += 1,
                    _ => {}
                }
            }
            EventResult::Ignored
        }
    }

    fn probe_scroll() -> ScrollWidget {
        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);
        w
    }

    fn run_rec(w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64) -> bool {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
        w.event_at(&mut ctx, e, t);
        ctx.needs_redraw()
    }

    #[test]
    fn an_overlay_broadcast_reaches_the_child_and_is_never_consumed() {
        use frust_core::{OverlayEvent, OverlayEventKind, OverlayKey};

        /// What a floated surface's owner below this viewport would see.
        #[derive(Default)]
        struct Seen {
            overlays: u32,
            pointers: u32,
        }
        struct Owner;
        struct OwnerW;
        impl View<Seen> for Owner {
            type Element = OwnerW;
            fn build(&self, _c: &mut BuildCtx<'_>) -> OwnerW {
                OwnerW
            }
            fn rebuild(&self, _p: &Self, _e: &mut OwnerW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }
        impl Widget for OwnerW {
            fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(200.0, 1000.0))
            }
            fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
                match e {
                    InputEvent::Overlay(_) => {
                        ctx.state_mut::<Seen>().overlays += 1;
                        // Even a `Handled` must not be reported upward: a
                        // broadcast is never consumed.
                        EventResult::Handled
                    }
                    InputEvent::Pointer(_) => {
                        ctx.state_mut::<Seen>().pointers += 1;
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
        }

        let view: ScrollView<Seen> = scroll_view(Owner);
        let mut counter = 0u64;
        let mut w = View::<Seen>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);
        let mut state = Seen::default();
        let broadcast = InputEvent::Overlay(OverlayEvent {
            key: OverlayKey::next(),
            kind: OverlayEventKind::OutsideDown,
        });
        let result = {
            let sa: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
            w.event_at(&mut ctx, &broadcast, 0.0)
        };
        assert_eq!(
            state.overlays, 1,
            "a floated surface's own input reaches its owner through the viewport"
        );
        assert_eq!(
            result,
            EventResult::Ignored,
            "and is never consumed, whatever the child returned"
        );
        assert_eq!(state.pointers, 0, "it is not a pointer event");
        // The gesture machinery is untouched by it: no capture was opened and
        // no drag armed.
        assert!(!w.down_active && !w.scrolling);
    }

    #[test]
    fn hover_move_without_down_never_scrolls_or_cancels_child() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        // A cursor drifting over the list with no prior Down: no takeover, no
        // Cancel to the child, no offset change, no self redraw request.
        let redraw = run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 16.0);
        assert!(!w.scrolling, "hover must not enter scrolling");
        assert!(!w.down_active);
        assert_eq!(w.offset(), 0.0, "hover must not move the offset");
        assert_eq!(state.cancels, 0, "hover must not cancel the child");
        assert!(!redraw, "hover must not request a redraw");
        // The hover move is forwarded to the child (which ignores it).
        assert_eq!(state.moves, 1);
    }

    #[test]
    fn cancel_clears_down_active() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        run_rec(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(w.down_active);
        run_rec(&mut w, &mut state, &ev(PointerPhase::Cancel, 100.0), 16.0);
        assert!(!w.down_active, "Cancel disarms the gesture");
        assert!(!w.scrolling);
        // A subsequent hover Move must not run the takeover math.
        run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 20.0), 32.0);
        assert!(!w.scrolling, "hover after Cancel must not take over");
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn fling_after_release_decays_and_clamps() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // scroll, builds velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");
        // Integrate to completion.
        let mut steps = 0;
        while w.tick(16.0) {
            steps += 1;
            assert!(steps < 100_000, "fling failed to terminate");
        }
        assert!(!w.is_flinging());
        assert!(w.offset() >= 0.0 && w.offset() <= w.max_offset());
    }

    #[test]
    fn pump_fling_signals_needs_frame_until_at_rest() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        // Drive a release-with-velocity to start a fling (deterministic seam).
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // build velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");

        // A paint-time pump while flinging asks for another frame.
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx);
        assert!(
            ctx.needs_frame(),
            "an in-flight fling requests continuation"
        );
        assert!(w.is_flinging());

        // Integrate the fling to rest via the deterministic tick seam.
        while w.tick(16.0) {}
        assert!(!w.is_flinging());

        // At rest, the pump no longer signals — the shell can idle again.
        let mut ctx_rest = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx_rest);
        assert!(
            !ctx_rest.needs_frame(),
            "a fling at rest stops requesting frames"
        );
    }

    #[test]
    fn tick_without_a_fling_is_a_noop() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        assert!(!w.tick(16.0));
        assert_eq!(w.offset(), 0.0);
    }

    /// A no-op paint sink for the RenderRoot clock test.
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn scroll_widget(root: &frust_core::RenderRoot<(), ScrollView<()>>) -> &ScrollWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ScrollWidget>()
            .expect("root is a ScrollWidget")
    }

    #[test]
    fn fling_advances_from_injected_paint_frame_time() {
        // End-to-end through the real paint path: the
        // event pass reads the last painted frame time for velocity tracking, and
        // the fling pump advances off the injected `RenderRoot::paint` frame time
        // — no wall clock anywhere. Paints are interleaved with the drag so the
        // velocity tracker sees distinct (paint-clock) timestamps.
        use frust_core::{FrameTime, RenderRoot};

        fn logic(_: &mut ()) -> ScrollView<()> {
            scroll_view(leaf(200.0, 1000.0))
        }
        let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 100.0));

        let ft = |ms: f64| FrameTime::from_nanos((ms * 1_000_000.0) as u64);
        let mut sink = NullScene;
        root.paint(&mut sink, ft(0.0));
        root.event(&mut state, &ev(PointerPhase::Down, 100.0));
        root.paint(&mut sink, ft(16.0));
        root.event(&mut state, &ev(PointerPhase::Move, 75.0)); // crosses slop → takeover
        root.paint(&mut sink, ft(32.0));
        root.event(&mut state, &ev(PointerPhase::Move, 50.0)); // scroll, builds velocity
        root.event(&mut state, &ev(PointerPhase::Up, 50.0)); // release → fling

        assert!(
            scroll_widget(&root).is_flinging(),
            "release with paint-clock velocity starts a fling"
        );
        let before = scroll_widget(&root).offset();

        // Advancing frame times drive the fling: the first paint seeds the fling
        // clock (zero delta), the next advances the offset.
        root.paint(&mut sink, ft(48.0));
        root.paint(&mut sink, ft(64.0));
        assert!(
            scroll_widget(&root).offset() > before,
            "the fling advanced from the injected paint frame time"
        );
    }

    /// A perpetual animator: requests a continuation frame on every paint.
    /// Stands in for an offscreen shimmer/spinner whose frame requests
    /// paint-time culling must suppress.
    struct Ticker;
    struct TickerWidget;
    impl View<()> for Ticker {
        type Element = TickerWidget;
        fn build(&self, _c: &mut BuildCtx<'_>) -> TickerWidget {
            TickerWidget
        }
        fn rebuild(&self, _p: &Self, _e: &mut TickerWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for TickerWidget {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 100.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _s: &mut dyn PaintScene) {
            ctx.request_frame();
        }
    }

    #[test]
    fn offscreen_flex_animator_culled_until_scrolled_into_view() {
        // End-to-end frame suppression: a perpetual animator below
        // the fold in a Column inside a ScrollView is culled from paint, so its
        // request_frame never bubbles and the root PaintOutcome asks for no
        // continuation frame. Scroll it into view and the requests resume.
        use frust_core::RenderRoot;

        fn logic(_: &mut ()) -> ScrollView<()> {
            // A 1000px spacer, then a 100px perpetual animator (content 1100 tall).
            scroll_view(crate::Column(vec![any(leaf(100.0, 1000.0)), any(Ticker)]))
        }
        let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let mut sink = NullScene;
        // At rest (offset 0) the animator sits at y=1000, far below the warm band
        // (viewport 100 + one-viewport margin → y ∈ [-100, 200]); it is culled, so
        // no continuation frame is requested.
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            !outcome.needs_frame,
            "an offscreen animator's frame request is culled"
        );

        // Scroll to the bottom (wheel clamps to max_offset = 1000): the animator
        // comes into view and its request_frame bubbles out of paint again.
        root.event(&mut state, &scroll(50.0, false, 5000.0));
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            outcome.needs_frame,
            "scrolling the animator into view resumes its frame requests"
        );
    }

    #[test]
    fn offset_reclamps_when_content_shrinks() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &scroll(50.0, false, 800.0), 0.0);
        assert_eq!(w.offset(), 800.0);
        // Content shrinks to just above the viewport → max_offset drops to 50,
        // and a re-clamp (what layout does) pulls the stale offset back in range.
        w.content = Size::new(200.0, 150.0);
        w.set_offset(w.offset);
        assert_eq!(w.max_offset(), 50.0);
        assert_eq!(w.offset(), 50.0);
    }

    // --- Overscroll (pull-to-refresh seam) ---

    #[test]
    fn drag_past_top_overscrolls_with_resistance_then_settles_back() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        // Down, then a drag downward crossing the slop takes the gesture over.
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover
        assert!(w.scrolling);
        assert_eq!(w.offset(), 0.0, "the takeover move does not itself scroll");
        // Drag 20px further down past the already-at-top edge → resisted overscroll.
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        assert!(w.offset() < 0.0, "a drag past the top overscrolls negative");
        assert_eq!(
            w.offset(),
            -10.0,
            "overscroll is the raw excess (-20) * OVERSCROLL_RESISTANCE (0.5)"
        );
        // Release → a settle animation, not a fling; it returns to the edge.
        dispatch(&mut w, &ev(PointerPhase::Up, 110.0), 48.0);
        assert!(
            !w.is_flinging(),
            "an overscrolled release settles, never flings"
        );
        let mut steps = 0;
        while w.settle_tick(16.0) {
            steps += 1;
            assert!(steps < 10_000, "settle failed to terminate");
        }
        assert_eq!(
            w.offset(),
            0.0,
            "the surface settles back to the clamped edge"
        );
    }

    #[test]
    fn wheel_never_overscrolls_past_top() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // A large negative wheel delta at the top stays hard-clamped at 0 — no
        // rubber-band on wheel input.
        dispatch(&mut w, &scroll(50.0, false, -5000.0), 0.0);
        assert_eq!(w.offset(), 0.0);
        assert!(!w.settling, "wheel input starts no settle animation");
    }

    /// A state that records every `ScrollInfo` its `on_scroll` observes.
    #[derive(Default)]
    struct ScrollLog {
        infos: Vec<ScrollInfo>,
        refreshes: u32,
    }

    /// A fixed-size content view generic over the state type (the shared `leaf`
    /// fixture is `View<()>` only), so a scroll view can wrap it over `ScrollLog`.
    struct Content(Size);
    struct ContentW(Size);
    impl<S: 'static> View<S> for Content {
        type Element = ContentW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ContentW {
            ContentW(self.0)
        }
        fn rebuild(&self, _p: &Self, _e: &mut ContentW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ContentW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
    }

    /// Build+lay out a scroll widget over `ScrollLog` state with the given
    /// callbacks installed.
    fn observed(with_refresh: bool) -> ScrollWidget {
        let mut view: ScrollView<ScrollLog> = scroll_view(Content(Size::new(200.0, 1000.0)))
            .on_scroll(|s: &mut ScrollLog, info| s.infos.push(info));
        if with_refresh {
            view = view.on_refresh_release(|s: &mut ScrollLog| s.refreshes += 1);
        }
        let mut counter = 0u64;
        let mut w = View::<ScrollLog>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        w
    }

    /// [`observed`] with the pre-seam [`RubberBand`] feel pinned explicitly,
    /// for the same reason [`laid_out_rubber_band`] exists.
    fn observed_rubber_band(with_refresh: bool) -> ScrollWidget {
        let mut w = observed(with_refresh);
        w.physics = Rc::new(RubberBand::new());
        w
    }

    fn run_log(w: &mut ScrollWidget, state: &mut ScrollLog, e: &InputEvent, t: f64) {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, e, t);
    }

    #[test]
    fn on_scroll_observes_drag_deltas() {
        let mut w = observed(false);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0); // takeover
        // Two scrolling drags upward move the content down.
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 32.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 10.0), 48.0);
        assert!(!state.infos.is_empty(), "on_scroll fired on the drag");
        let last = state.infos.last().unwrap();
        assert!(
            last.offset > 0.0,
            "the observed offset grew as content scrolled"
        );
        assert_eq!(last.overscroll, 0.0, "an in-range drag has no overscroll");
        assert_eq!(last.max_offset, 900.0);
    }

    #[test]
    fn on_refresh_release_fires_only_past_trigger_and_only_on_release() {
        // A small pull (under the trigger) does not fire on release.
        let mut w = observed_rubber_band(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 180.0), 32.0); // raw -90 → -45
        assert_eq!(w.offset(), -45.0, "under the trigger (|-45| < 64)");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 180.0), 48.0);
        assert_eq!(
            state.refreshes, 0,
            "release under the trigger does not refresh"
        );

        // A large pull past the trigger fires exactly once, on release.
        let mut w = observed_rubber_band(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 290.0), 32.0); // raw -200 → -100
        assert_eq!(w.offset(), -100.0, "past the trigger (|-100| > 64)");
        assert_eq!(state.refreshes, 0, "no fire before release");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 290.0), 48.0);
        assert_eq!(state.refreshes, 1, "release past the trigger fires once");
    }

    #[test]
    fn cancel_during_overscroll_never_fires_refresh_and_snaps_back() {
        let mut w = observed_rubber_band(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 290.0), 32.0); // past trigger
        assert_eq!(w.offset(), -100.0);
        // A Cancel (gesture steal) must not fire on_refresh_release and snaps the
        // overscroll away with no settle animation.
        run_log(&mut w, &mut state, &ev(PointerPhase::Cancel, 290.0), 48.0);
        assert_eq!(
            state.refreshes, 0,
            "Cancel never fires the refresh callback"
        );
        assert_eq!(w.offset(), 0.0, "Cancel snaps the surface back into range");
        assert!(!w.settling);
    }

    // --- Device-gate G6 (the "phantom clone" bug): a layout pass mid-gesture
    // must not snap an out-of-range offset back into range. ---

    #[test]
    fn layout_mid_drag_preserves_top_overscroll() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover
        assert!(w.scrolling);
        // Drag 20px further down past the already-at-top edge → resisted overscroll.
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        assert_eq!(
            w.offset(),
            -10.0,
            "resisted overscroll before the layout pass"
        );

        // A layout pass fires mid-drag (e.g. a sibling requesting relayout every
        // frame, like a wavy progress indicator). Without the fix this snaps the
        // child back to rest (offset 0.0) — the phantom-clone bug: the next
        // pointer Move re-applies the displacement, so the presented frame
        // alternates between rest and dragged at display rate.
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(
            w.offset(),
            -10.0,
            "a layout pass mid-drag must not snap the overscroll back into range"
        );
        assert!(w.scrolling, "still an active drag after the layout pass");
    }

    #[test]
    fn layout_mid_settle_preserves_decaying_overscroll() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 290.0), 32.0); // raw -200 → -100
        assert_eq!(w.offset(), -100.0, "past the refresh trigger (|-100| > 64)");
        dispatch(&mut w, &ev(PointerPhase::Up, 290.0), 48.0);
        assert!(
            w.settling,
            "an overscrolled release enters the settle animation"
        );
        assert!(!w.is_flinging());

        // Advance one settle tick: the offset has eased toward the edge but has
        // not arrived yet.
        let still_settling = w.settle_tick(16.0);
        assert!(still_settling);
        let after_tick = w.offset();
        assert!(
            after_tick < 0.0,
            "one settle tick eases toward the edge but is still out of range"
        );

        // A layout pass fires mid-settle. Without the fix this snaps the
        // decaying offset straight to 0.0, visibly skipping the rest of the
        // settle animation (the same bug class as the mid-drag case above).
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(
            w.offset(),
            after_tick,
            "a layout pass mid-settle must not snap the decaying overscroll back into range"
        );
        assert!(w.settling, "still settling after the layout pass");
    }

    // --- The physics seam: the default `RubberBand` install, the generic
    //     ballistic driver, and the carried-momentum hook. ---

    #[test]
    fn edge_pull_equals_overscroll_under_rubber_band() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover
        // 20px past the already-at-top edge: resistance halves it, and nothing
        // is boundary-rejected, so the pull *is* the displacement.
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        assert_eq!(w.offset(), -10.0);
        assert_eq!(w.edge_pull, -10.0, "negative past the top, like overscroll");
        assert_eq!(
            w.edge_pull,
            w.scroll_info().overscroll,
            "nothing rejected → the two are the same number"
        );

        // The settle decays both together, and both land exactly on zero.
        dispatch(&mut w, &ev(PointerPhase::Up, 110.0), 48.0);
        assert!(w.settling);
        assert!(w.settle_tick(16.0));
        assert!(
            w.edge_pull < 0.0 && w.edge_pull > -10.0,
            "one settle tick eases the pull toward the edge: {}",
            w.edge_pull
        );
        assert_eq!(w.edge_pull, w.scroll_info().overscroll);
        while w.settle_tick(16.0) {}
        assert_eq!(w.edge_pull, 0.0, "a completed settle leaves no pull");
        assert_eq!(w.scroll_info().overscroll, 0.0);

        // The bottom edge is the same story with the opposite sign.
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        dispatch(&mut w, &scroll(50.0, false, 5000.0), 0.0); // clamp to max_offset
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 60.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 0.0), 32.0); // 60px past the bottom
        assert_eq!(w.edge_pull, 30.0, "positive past the bottom");
        assert_eq!(w.edge_pull, w.scroll_info().overscroll);
    }

    /// A scripted ballistic curve: a straight 100 px/s ramp from where the
    /// release left the position, done after 100ms.
    struct Ramp {
        from: f64,
    }
    impl Simulation for Ramp {
        fn x(&self, time: f64) -> f64 {
            self.from + 100.0 * time
        }
        fn dx(&self, _time: f64) -> f64 {
            100.0
        }
        fn is_done(&self, time: f64) -> bool {
            time >= 0.1
        }
    }

    /// A toy physics that *does* hand back a simulation — the counterpart of
    /// `RubberBand`'s `None`, exercising the generic driver.
    #[derive(Debug)]
    struct RampPhysics;
    impl ScrollPhysics for RampPhysics {
        fn create_ballistic_simulation(
            &self,
            metrics: &ScrollMetrics,
            _velocity: f64,
        ) -> Option<Box<dyn Simulation>> {
            Some(Box::new(Ramp {
                from: metrics.pixels,
            }))
        }
    }

    fn frame_time(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[test]
    fn ballistic_driver_runs_generic_simulation() {
        let mut w = observed(false);
        w.physics = Rc::new(RampPhysics);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 50.0), 32.0); // in-range drag
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(
            w.ballistic.is_some(),
            "a physics handing back a simulation owns the release"
        );
        assert!(w.fling.is_none(), "…and the legacy fling never starts");
        assert!(!w.settling);
        let start = w.offset();
        let observed_before = state.infos.len();

        // The first pump seeds the simulation clock: zero delta, nothing moves,
        // nothing recorded — the same seeding frame the legacy pump takes.
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(100.0));
        w.pump_fling(&mut ctx);
        assert!(ctx.needs_frame(), "a live simulation asks for continuation");
        assert_eq!(w.offset(), start, "the seeding frame moves nothing");
        assert!(!w.pending_scroll_notify);

        // 16ms on, the offset is exactly the curve's own position.
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(116.0));
        w.pump_fling(&mut ctx);
        assert!(
            (w.offset() - (start + 1.6)).abs() < 1e-9,
            "the offset follows sim.x(t): {}",
            w.offset()
        );
        assert!(w.pending_scroll_notify, "recorded at paint, not fired");
        assert_eq!(
            state.infos.len(),
            observed_before,
            "the notification stays one event late"
        );

        // …and the next event delivers it (a hover move, the gesture is over).
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 50.0), 132.0);
        assert!(!w.pending_scroll_notify);
        assert_eq!(state.infos.len(), observed_before + 1);

        // Past the curve's own end the driver drops it and the pump goes quiet.
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(300.0));
        w.pump_fling(&mut ctx);
        assert!((w.offset() - (start + 20.0)).abs() < 1e-9);
        assert!(w.ballistic.is_none(), "a done simulation is dropped");
        assert!(!w.is_flinging());
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(316.0));
        w.pump_fling(&mut ctx);
        assert!(!ctx.needs_frame(), "at rest the shell can idle again");
    }

    /// A toy physics carrying a fixed +100 px/s out of *interrupted* motion
    /// (nothing to carry from a press onto a resting surface), leaving
    /// post-release motion to the legacy path like `RubberBand` does.
    #[derive(Debug)]
    struct CarryPhysics;
    impl ScrollPhysics for CarryPhysics {
        fn carried_momentum(&self, existing_velocity: f64) -> f64 {
            if existing_velocity == 0.0 { 0.0 } else { 100.0 }
        }
    }

    /// Drag-release twice, the second press landing on the live fling of the
    /// first, and report the two fling velocities.
    fn fling_then_refling(w: &mut ScrollWidget) -> (f64, f64) {
        dispatch(w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(w, &ev(PointerPhase::Move, 50.0), 32.0); // builds velocity
        dispatch(w, &ev(PointerPhase::Up, 50.0), 32.0);
        let first = w.fling.expect("release with velocity flings");
        // The second press interrupts that fling, and releases identically.
        dispatch(w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(w, &ev(PointerPhase::Move, 75.0), 64.0);
        dispatch(w, &ev(PointerPhase::Move, 50.0), 80.0);
        dispatch(w, &ev(PointerPhase::Up, 50.0), 80.0);
        (first, w.fling.expect("the second release flings too"))
    }

    #[test]
    fn carried_momentum_hook_feeds_new_fling() {
        let mut w = laid_out(200.0, 100.0, 5000.0);
        w.physics = Rc::new(CarryPhysics);
        let (first, second) = fling_then_refling(&mut w);
        assert_eq!(
            second,
            first + 100.0,
            "a fling started during live motion carries the physics' momentum"
        );

        // `RubberBand` carries nothing, so the identical sequence produces
        // the identical velocity twice (and keeps the legacy fling the helper
        // above reads — the bouncing default hands back a simulation instead,
        // pinned by `default_carried_momentum_compounds_a_refling`).
        let mut w = laid_out_rubber_band(200.0, 100.0, 5000.0);
        let (first, second) = fling_then_refling(&mut w);
        assert_eq!(second, first, "RubberBand starts every fling cold");
    }

    // --- The platform default (`physics::default_physics`): bouncing on this
    //     host, clamping on Android. What a surface that names no physics of
    //     its own actually does — the depth-aware drag curve, the spring-back
    //     release, carried momentum, and the fling gate. ---

    fn assert_close(actual: f64, expected: f64, epsilon: f64, what: &str) {
        assert!(
            (actual - expected).abs() < epsilon,
            "{what}: {actual} is not within {epsilon} of {expected}"
        );
    }

    /// The host default, named once so every test below reads as "the default"
    /// rather than "Bouncing" — and so the pairing itself is asserted.
    #[test]
    fn a_fresh_surface_installs_the_platform_default() {
        let w = laid_out(200.0, 100.0, 1000.0);
        assert_eq!(
            format!("{:?}", w.physics),
            format!("{:?}", crate::physics::default_physics())
        );
        assert_eq!(w.effect, crate::physics::default_overscroll_effect());
    }

    #[test]
    fn default_drag_tension_tightens_with_depth() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover

        // 20px past the already-at-top edge, from zero depth: the friction
        // factor is 0.52·(1 − 0)² = 0.52, so 20 · 0.52 = 10.4 shows.
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        let first = w.offset();
        assert_close(
            first,
            -20.0 * DecelerationRate::NORMAL_FRICTION,
            1e-12,
            "the first past-edge move, at zero depth",
        );

        // 20px more. The surface now sits 10.4px out of a 100px viewport, so
        // the factor has tightened to 0.52·(1 − 0.104)² = 0.41746432 and this
        // move only adds 20 · 0.41746432 = 8.3492864 — a total of 18.7492864.
        dispatch(&mut w, &ev(PointerPhase::Move, 130.0), 48.0);
        let second = w.offset() - first;
        assert_close(second, -8.349_286_4, 1e-9, "the second, deeper move");
        assert_close(w.offset(), -18.749_286_4, 1e-9, "the accumulated pull");
        assert!(
            second.abs() < first.abs(),
            "the deeper pull must displace LESS per raw px: {second} vs {first}"
        );
        // The whole pull is displacement, none of it boundary-rejected.
        assert_eq!(w.edge_pull, w.scroll_info().overscroll);
    }

    #[test]
    fn default_release_past_the_edge_springs_back_to_the_boundary() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 110.0), 32.0);
        assert!(w.offset() < 0.0, "the drag left the surface past its top");

        dispatch(&mut w, &ev(PointerPhase::Up, 110.0), 48.0);
        assert!(
            w.ballistic.is_some(),
            "the default physics owns the release with a spring"
        );
        assert!(w.fling.is_none() && !w.settling, "…so no legacy path runs");
        assert!(w.is_flinging(), "post-release motion is live");

        // Pump the shared frame clock until the spring reports itself done.
        let mut ms = 100.0;
        let mut steps = 0;
        let mut moved = false;
        while w.is_flinging() {
            let before = w.offset();
            let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(ms));
            w.pump_fling(&mut ctx);
            moved |= w.offset() != before;
            ms += 16.0;
            steps += 1;
            assert!(steps < 2_000, "the bounce-back never settled");
        }
        assert!(moved, "the spring must actually animate, not snap");
        assert!(
            w.offset().abs() < 1e-6,
            "the spring converges onto the boundary: {}",
            w.offset()
        );
        assert!(
            w.edge_pull.abs() < 1e-6,
            "…and leaves no pull behind: {}",
            w.edge_pull
        );
    }

    #[test]
    fn default_carried_momentum_compounds_a_refling() {
        let mut w = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // builds velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        let first = w
            .ballistic
            .as_ref()
            .expect("a release above the fling minimum is ballistic")
            .sim
            .dx(0.0);
        // 50px of finger travel over 32ms, and the offset moves opposite it.
        assert_close(first, 1562.5, 1e-9, "the first release velocity");

        // The second press lands on that live curve, so its release carries
        // the physics' own fitted share of it forward.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 64.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 80.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 80.0);
        let second = w
            .ballistic
            .as_ref()
            .expect("the second release is ballistic too")
            .sim
            .dx(0.0);
        assert_close(
            second,
            first + Bouncing::new().carried_momentum(first),
            1e-9,
            "the re-fling carries Bouncing::carried_momentum(first)",
        );
        assert!(second > first, "…which is a genuine speed-up");
    }

    /// The signed start velocity of whatever post-release motion the last `Up`
    /// produced — the physics-supplied curve's own, the legacy fling's, or
    /// `0.0` for a release that started no motion at all. Reads all three
    /// outcomes through one number so a direction assertion does not depend on
    /// which release path caught the gesture.
    fn release_velocity(w: &ScrollWidget) -> f64 {
        match w.ballistic.as_ref() {
            Some(state) => state.sim.dx(0.0),
            None => w.fling.unwrap_or(0.0),
        }
    }

    #[test]
    fn reverse_refling_keeps_the_fingers_velocity() {
        // Parked mid-content, so both releases are judged on velocity alone
        // with no edge spring in play.
        let mut w = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut w, &scroll(50.0, false, 1000.0), 0.0);

        // A downward fling: 50px of finger travel up over 32ms.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert_close(release_velocity(&w), 1562.5, 1e-9, "the first release");

        // The finger lands on that live curve and flicks back the other way,
        // just as fast. The interrupted motion's momentum must not be added to
        // a release pointing the other way — it would cancel the flick out (or
        // reverse it), and the surface would ignore the finger entirely.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 125.0), 64.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 150.0), 80.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 150.0), 80.0);
        assert_close(
            release_velocity(&w),
            -1562.5,
            1e-9,
            "the reverse re-fling runs at the finger's own velocity",
        );
        assert!(
            w.ballistic.is_some(),
            "…as a real ballistic curve, not a stalled remnant"
        );
    }

    #[test]
    fn small_reverse_flick_is_not_inverted() {
        let mut w = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut w, &scroll(50.0, false, 1000.0), 0.0);

        // Live downward motion at 1000 px/s: 32px of finger travel over 32ms.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 78.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the live motion");

        // A *modest* drag back the other way — 24px down over 60ms, 400 px/s,
        // well under the interrupted motion's own speed. Slower than what it
        // interrupted, but still unambiguously the other way: the surface must
        // never answer it by accelerating onward in the old direction.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 108.0), 68.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 116.0), 88.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 124.0), 108.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Up, 124.0), 108.0);
        let released = release_velocity(&w);
        assert!(
            released <= 0.0,
            "a reverse flick must never relaunch the surface the way it was \
             already going: {released}"
        );
        assert_close(
            released,
            -400.0,
            1e-9,
            "…it runs at the finger's own velocity instead",
        );
    }

    /// The retain gate compares a release against the physics' **mapped**
    /// share of the interrupted velocity, not the raw interrupted speed —
    /// pinned here because the two diverge (`Bouncing`'s power curve sits
    /// below the raw value under ~1563 px/s). Interrupted at 1000 px/s,
    /// `Bouncing::new().carried_momentum(1000.0)` maps to ~649.7, putting the
    /// retain threshold at ~324.8 — well under the raw-carried threshold
    /// (500) the pre-fix gate used.
    #[test]
    fn momentum_retain_threshold_refuses_a_weak_refling() {
        // Parked mid-content, so the release is judged on velocity alone.
        let mut w = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut w, &scroll(50.0, false, 1000.0), 0.0);

        // Interrupted motion at 1000 px/s (the same drag
        // `small_reverse_flick_is_not_inverted` uses to establish it).
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 78.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the interrupted motion");

        let mapped = Bouncing::new().carried_momentum(1000.0);
        let threshold = MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped;
        assert!(
            (300.0..350.0).contains(&threshold),
            "the fixture's release values must straddle the threshold: {threshold}"
        );

        // Same-direction re-flick at 300 px/s — under the mapped threshold.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 80.0), 64.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 148.0); // 30px / 100ms → 300 px/s
        dispatch(&mut w, &ev(PointerPhase::Up, 70.0), 148.0);
        assert_close(
            release_velocity(&w),
            300.0,
            1e-9,
            "a release under the mapped threshold carries nothing forward",
        );
    }

    /// The strong-side twin of `momentum_retain_threshold_refuses_a_weak_refling`:
    /// a release over the same mapped threshold carries `mapped` forward
    /// exactly, pre-clamp.
    #[test]
    fn momentum_retain_threshold_carries_a_strong_refling() {
        let mut w = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut w, &scroll(50.0, false, 1000.0), 0.0);

        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 78.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the interrupted motion");

        let mapped = Bouncing::new().carried_momentum(1000.0);
        let threshold = MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped;
        assert!(
            (300.0..350.0).contains(&threshold),
            "the fixture's release values must straddle the threshold: {threshold}"
        );

        // Same-direction re-flick at 350 px/s — over the mapped threshold.
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 80.0), 64.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 65.0), 148.0); // 35px / 100ms → 350 px/s
        dispatch(&mut w, &ev(PointerPhase::Up, 65.0), 148.0);
        assert_close(
            release_velocity(&w),
            350.0 + mapped,
            1e-9,
            "a release over the mapped threshold carries `mapped` forward exactly",
        );
    }

    #[test]
    fn default_min_fling_gate_is_one_hundred() {
        // Both halves start parked mid-content, so the release is judged on
        // velocity alone — an out-of-range release always gets its spring back
        // whatever the speed (`default_release_past_the_edge_springs_back…`).
        let mut slow = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut slow, &scroll(50.0, false, 1000.0), 0.0);
        dispatch(&mut slow, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut slow, &ev(PointerPhase::Move, 80.0), 16.0); // takeover
        // 6px of finger travel over the tracker's whole 100ms window: 60 px/s.
        dispatch(&mut slow, &ev(PointerPhase::Move, 94.0), 100.0);
        dispatch(&mut slow, &ev(PointerPhase::Up, 94.0), 100.0);
        assert!(
            slow.ballistic.is_none(),
            "60 px/s is under the bouncing minimum (100), so no ballistic"
        );
        // The physics declining leaves the release on the widget's own legacy
        // path, whose threshold is the pinned FLING_STOP (30 px/s) instead —
        // the one place the two ladders disagree, pinned so it cannot drift
        // unnoticed.
        assert_close(
            slow.fling.expect("the legacy fling catches it instead"),
            60.0,
            1e-9,
            "the legacy fallback runs at the raw release velocity",
        );

        let mut fast = laid_out(200.0, 100.0, 5000.0);
        dispatch(&mut fast, &scroll(50.0, false, 1000.0), 0.0);
        dispatch(&mut fast, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut fast, &ev(PointerPhase::Move, 80.0), 16.0); // takeover
        // 15px over the same window: 150 px/s, over the minimum.
        dispatch(&mut fast, &ev(PointerPhase::Move, 85.0), 100.0);
        dispatch(&mut fast, &ev(PointerPhase::Up, 85.0), 100.0);
        let sim = fast
            .ballistic
            .as_ref()
            .expect("150 px/s clears the bouncing minimum");
        assert_close(sim.sim.dx(0.0), 150.0, 1e-9, "…at the release velocity");
        assert!(fast.fling.is_none(), "and the legacy fling stays out of it");
        assert_eq!(Bouncing::new().min_fling_velocity(), 100.0);
    }

    // --- Pull-to-refresh under both shipped defaults: the trigger is measured
    //     on `edge_pull`, which a bouncing surface fills with displacement and
    //     a clamping one with boundary-rejected pull. ---

    #[test]
    fn refresh_trigger_under_the_bouncing_default() {
        // 110px of raw pull at zero depth maps to 110 · 0.52 = 57.2, under the
        // 64px trigger.
        let mut w = observed(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 200.0), 32.0);
        assert_close(w.edge_pull, -57.2, 1e-9, "under the trigger");
        assert_eq!(
            w.edge_pull,
            w.scroll_info().overscroll,
            "a bouncing surface rejects nothing, so the pull IS the displacement"
        );
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 200.0), 48.0);
        assert_eq!(state.refreshes, 0, "release under the trigger never fires");

        // 150px of raw pull maps to 78.0 — past it, so the release fires once.
        let mut w = observed(true);
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 240.0), 32.0);
        assert_close(w.edge_pull, -78.0, 1e-9, "past the trigger");
        assert!(crossed_refresh_trigger(w.edge_pull));
        assert_eq!(state.refreshes, 0, "no fire before release");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 240.0), 48.0);
        assert_eq!(state.refreshes, 1, "release past the trigger fires once");
    }

    #[test]
    fn refresh_trigger_under_a_clamping_physics() {
        // Android's default, simulated on the host: the position never leaves
        // range, so the trigger is reached at the RAW pull distance.
        let mut w = observed(true);
        w.physics = Rc::new(Clamping::new());
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 140.0), 32.0);
        assert_eq!(w.offset(), 0.0, "a clamping surface never displaces");
        assert_eq!(w.scroll_info().overscroll, 0.0);
        assert_eq!(w.edge_pull, -50.0, "…but reports the whole rejected pull");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 140.0), 48.0);
        assert_eq!(state.refreshes, 0, "50px of raw pull is under the trigger");

        let mut w = observed(true);
        w.physics = Rc::new(Clamping::new());
        let mut state = ScrollLog::default();
        run_log(&mut w, &mut state, &ev(PointerPhase::Down, 50.0), 0.0);
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 90.0), 16.0); // takeover
        run_log(&mut w, &mut state, &ev(PointerPhase::Move, 190.0), 32.0);
        assert_eq!(w.offset(), 0.0);
        assert_eq!(w.edge_pull, -100.0, "100px of raw pull, none of it shown");
        run_log(&mut w, &mut state, &ev(PointerPhase::Up, 190.0), 48.0);
        assert_eq!(state.refreshes, 1, "past 64px of raw pull, it fires once");
        // Nothing to spring back (the position never moved), so the rejected
        // pull decays on the settle instead — which is what keeps a stretch
        // effect from snapping off at release.
        assert!(w.settling, "the rejected pull settles rather than springs");
        while w.settle_tick(16.0) {}
        assert_eq!(w.edge_pull, 0.0);
    }

    // --- The M3E stretch effect: a paint-side affine about the pulled edge,
    //     with the content origin left where an in-range offset puts it. See
    //     the module docs' *Overscroll visuals*. ---

    /// `Down`, a 40px past-slop takeover drag, then 20px further past the
    /// already-at-top edge — the same gesture the overscroll tests above use,
    /// so every effect below sees byte-identical input.
    fn drag_20px_past_top(w: &mut ScrollWidget) {
        dispatch(w, &ev(PointerPhase::Down, 50.0), 0.0);
        dispatch(w, &ev(PointerPhase::Move, 90.0), 16.0); // 40px > slop → takeover
        dispatch(w, &ev(PointerPhase::Move, 110.0), 32.0);
    }

    /// Scroll to the bottom, then drag 60px past that edge.
    fn drag_60px_past_bottom(w: &mut ScrollWidget) {
        dispatch(w, &scroll(50.0, false, 5000.0), 0.0); // clamp to max_offset
        dispatch(w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(w, &ev(PointerPhase::Move, 60.0), 16.0); // takeover
        dispatch(w, &ev(PointerPhase::Move, 0.0), 32.0);
    }

    /// Paint `w` into a recording scene and read back the one transform the
    /// stretch pushed, as `(anchor_y, scale_y)` in absolute paint space —
    /// painted-scene inspection, no test-only accessor. The affine is
    /// `translate(anchor)·scale(1, s)·translate(−anchor)`, whose coefficients
    /// are `[1, 0, 0, s, 0, anchor·(1 − s)]`, so both terms read straight back
    /// off it. `None` when the paint pushed no transform at all.
    fn painted_stretch(w: &mut ScrollWidget) -> Option<(f64, f64)> {
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        let mut scene = crate::test_support::RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(
            scene.transforms.len(),
            scene.transform_pops as usize,
            "every pushed transform must be popped in the same paint"
        );
        assert!(
            scene.transforms.len() <= 1,
            "the stretch pushes at most one transform"
        );
        assert_eq!(scene.rects.len(), 1, "the child paints either way");
        let [a, b, c, d, e, f] = scene.transforms.first()?.as_coeffs();
        assert_eq!(
            [a, b, c, e],
            [1.0, 0.0, 0.0, 0.0],
            "a scroll-axis-only scale: no x scale, no skew, no x translation"
        );
        Some((f / (1.0 - d), d))
    }

    #[test]
    fn stretch_intensity_curve_pins() {
        // An unpulled surface stretches not at all.
        assert_eq!(stretch_intensity(0.0, 100.0), 0.0);

        // x = 0.1, hand-computed from the two constants:
        //   0.016·0.1 + 0.016·(1 − e^(−0.1·e/0.33))
        // = 0.0016   + 0.016·(1 − e^−0.8237217661997105)
        // = 0.01057927172144907
        assert!(
            (stretch_intensity(-10.0, 100.0) - 0.010_579_271_721_449_07).abs() < 1e-9,
            "the curve drifted from its pinned constants: {}",
            stretch_intensity(-10.0, 100.0)
        );
        assert_eq!(
            stretch_intensity(10.0, 100.0),
            stretch_intensity(-10.0, 100.0),
            "magnitude-only: the sign picks the anchor, never the amount"
        );

        // Strictly increasing across the whole normalized range.
        let mut previous = 0.0;
        for step in 1..=100 {
            let intensity = stretch_intensity(step as f64, 100.0);
            assert!(
                intensity > previous,
                "the curve must increase monotonically (step {step}): {intensity} <= {previous}"
            );
            previous = intensity;
        }

        // Bounded by the sum of the two terms' ceilings, and clamped past a
        // full-viewport pull rather than growing without limit.
        assert!(stretch_intensity(100.0, 100.0) <= 2.0 * STRETCH_INTENSITY);
        assert_eq!(
            stretch_intensity(500.0, 100.0),
            stretch_intensity(100.0, 100.0),
            "the normalized pull clamps at 1.0"
        );
        assert_eq!(
            stretch_intensity(-10.0, 0.0),
            0.0,
            "a degenerate viewport stretches nothing"
        );
    }

    #[test]
    fn stretch_keeps_child_origin_fixed() {
        // The identical drag under each effect. The offset is the physics'
        // answer and must not vary; only what paint does with it does.
        let mut translate = laid_out_rubber_band(200.0, 100.0, 1000.0);
        drag_20px_past_top(&mut translate);
        let mut stretch = laid_out_rubber_band(200.0, 100.0, 1000.0);
        stretch.effect = OverscrollEffect::Stretch;
        drag_20px_past_top(&mut stretch);
        let mut none = laid_out_rubber_band(200.0, 100.0, 1000.0);
        none.effect = OverscrollEffect::None;
        drag_20px_past_top(&mut none);

        assert_eq!(stretch.offset(), -10.0, "the resisted overscroll, as ever");
        assert_eq!(translate.offset(), stretch.offset());
        assert_eq!(none.offset(), stretch.offset());
        assert_eq!(translate.edge_pull, stretch.edge_pull);

        // Translate paints the displacement into the child origin; Stretch and
        // None leave it exactly where an in-range offset would put it.
        assert_eq!(translate.child.origin().y, 10.0);
        assert_eq!(stretch.child.origin().y, 0.0);
        assert_eq!(none.child.origin().y, 0.0);
        assert_eq!(
            translate.child.origin().y - stretch.child.origin().y,
            -stretch.displacement(),
            "the two fixtures differ by exactly the overscroll displacement"
        );

        // …and only Stretch paints a transform for it.
        assert_eq!(painted_stretch(&mut translate), None);
        assert_eq!(painted_stretch(&mut none), None);
        assert!(painted_stretch(&mut stretch).is_some());
    }

    #[test]
    fn stretch_anchor_follows_pulled_edge() {
        let mut top = laid_out_rubber_band(200.0, 100.0, 1000.0);
        top.effect = OverscrollEffect::Stretch;
        drag_20px_past_top(&mut top);
        assert_eq!(top.edge_pull, -10.0, "pulled past the top");
        let (anchor, scale) = painted_stretch(&mut top).expect("a held pull stretches");
        assert!(
            anchor.abs() < 1e-9,
            "a top pull scales about the viewport's top edge: {anchor}"
        );
        assert!(
            (scale - (1.0 + stretch_intensity(-10.0, 100.0))).abs() < 1e-12,
            "scale is 1 + the curve's intensity: {scale}"
        );
        assert!(scale > 1.0, "the content grows, never shrinks");

        let mut bottom = laid_out_rubber_band(200.0, 100.0, 1000.0);
        bottom.effect = OverscrollEffect::Stretch;
        drag_60px_past_bottom(&mut bottom);
        assert_eq!(bottom.edge_pull, 30.0, "pulled past the bottom");
        let (anchor, scale) = painted_stretch(&mut bottom).expect("a held pull stretches");
        assert!(
            (anchor - 100.0).abs() < 1e-9,
            "a bottom pull scales about the viewport's bottom edge: {anchor}"
        );
        assert!((scale - (1.0 + stretch_intensity(30.0, 100.0))).abs() < 1e-12);
    }

    #[test]
    fn stretch_settles_back_to_identity() {
        let mut w = laid_out_rubber_band(200.0, 100.0, 1000.0);
        w.effect = OverscrollEffect::Stretch;
        drag_20px_past_top(&mut w);
        let (_, held) = painted_stretch(&mut w).expect("the held pull stretches");

        dispatch(&mut w, &ev(PointerPhase::Up, 110.0), 48.0);
        assert!(w.settling, "an overscrolled release settles, effect or not");
        let (_, releasing) = painted_stretch(&mut w).expect("the settle still stretches");
        assert!(
            releasing <= held,
            "the stretch decays with the pull, never grows: {releasing} > {held}"
        );

        let mut steps = 0;
        while w.settle_tick(16.0) {
            steps += 1;
            assert!(steps < 10_000, "settle failed to terminate");
        }
        assert_eq!(w.edge_pull, 0.0, "a completed settle leaves no pull");
        assert_eq!(
            painted_stretch(&mut w),
            None,
            "…so paint pushes no transform at all — back to identity"
        );

        // And with nothing left to animate, the pump stops asking for frames.
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        let mut scene = crate::test_support::RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(
            !ctx.needs_frame(),
            "a settled stretch stops requesting frames"
        );
    }

    /// A toy clamping physics: it rejects 100% of any past-edge proposal, so
    /// the position never leaves range and the entire pull is reported as
    /// boundary rejection instead — the Android-style clamping-plus-stretch
    /// pairing, exercised here without depending on any composed default.
    #[derive(Debug)]
    struct RejectPastEdge;
    impl ScrollPhysics for RejectPastEdge {
        fn apply_boundary_conditions(&self, metrics: &ScrollMetrics, value: f64) -> f64 {
            value - value.clamp(metrics.min_scroll_extent, metrics.max_scroll_extent)
        }
    }

    #[test]
    fn stretch_under_boundary_rejection_uses_edge_pull() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        w.effect = OverscrollEffect::Stretch;
        w.physics = Rc::new(RejectPastEdge);
        drag_20px_past_top(&mut w);

        assert_eq!(
            w.offset(),
            0.0,
            "a clamping physics never lets the position leave range"
        );
        assert_eq!(
            w.scroll_info().overscroll,
            0.0,
            "…so there is no displacement for Translate to have shown"
        );
        // The whole raw 20px is rejected excess (this physics maps the drag
        // itself with the trait's identity default — no rubber-band halving).
        assert_eq!(w.edge_pull, -20.0, "the pull is still reported in full");

        let (anchor, scale) = painted_stretch(&mut w).expect("a rejected pull still stretches");
        assert!(
            anchor.abs() < 1e-9,
            "anchored at the pulled (top) edge: {anchor}"
        );
        assert!((scale - (1.0 + stretch_intensity(-20.0, 100.0))).abs() < 1e-12);
        assert!(scale > 1.0, "clamping + stretch is a visible effect");
        assert_eq!(
            w.child.origin().y,
            0.0,
            "and the content itself never moves"
        );
    }

    #[test]
    fn clamping_fling_into_the_edge_settles_the_stretch() {
        use crate::physics::Tolerance;
        use crate::physics::simulation::ClampingScrollSimulation;

        // The shipped Android pairing, run on the host: parked 100px short of
        // the bottom, released at 1000 px/s straight into it. The clamping
        // curve is unbounded (`physics::parity`'s *Why Clamping needs no
        // clamped simulation adapter*), so the driver pins the offset and
        // routes the whole overshoot into `edge_pull` — the release must not
        // leave that pull, and the stretch it paints, standing.
        let mut w = laid_out(200.0, 100.0, 1000.0);
        w.physics = Rc::new(Clamping::new());
        w.effect = OverscrollEffect::Stretch;
        dispatch(&mut w, &scroll(50.0, false, 800.0), 0.0);
        assert_eq!(w.offset(), 800.0, "parked 100px short of the 900px bottom");

        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 68.0), 16.0); // 32px > slop → takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        let released = w.ballistic.as_ref().expect("a clamping release flings");
        assert_close(released.sim.dx(0.0), 1000.0, 1e-9, "the release velocity");

        // The same curve, spelled out: it runs far past the extent, which is
        // what makes the pull it feeds `edge_pull` a real one.
        let curve = ClampingScrollSimulation::new(
            800.0,
            1000.0,
            ClampingScrollSimulation::DEFAULT_FRICTION,
            Tolerance::for_device_pixel_ratio(METRICS_FALLBACK_DPR),
        );
        assert!(
            curve.final_x() > 1000.0,
            "the spline must overshoot the 900px extent by >100px: {}",
            curve.final_x()
        );
        let spline_frames = (curve.duration() * 1000.0 / 16.0).ceil();

        // Pump the shared frame clock until nothing asks for another frame.
        let mut ms = 100.0;
        let mut frames = 0.0;
        let mut ballistic_frames = 0.0;
        let mut peak = 0.0f64;
        loop {
            let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(ms));
            w.pump_fling(&mut ctx);
            peak = peak.max(w.edge_pull.abs());
            if w.ballistic.is_some() {
                ballistic_frames += 1.0;
            }
            ms += 16.0;
            frames += 1.0;
            assert!(
                frames < 2_000.0,
                "the release never came to rest: edge_pull {}",
                w.edge_pull
            );
            if !ctx.needs_frame() {
                break;
            }
        }

        assert!(
            peak > 1.0,
            "the fling must actually reach the edge for this to mean anything: {peak}"
        );
        assert_close(
            w.offset(),
            900.0,
            1e-9,
            "the offset ends pinned at the edge",
        );
        assert_eq!(w.edge_pull, 0.0, "a finished fling leaves no pull standing");
        assert_eq!(
            painted_stretch(&mut w),
            None,
            "…so paint pushes no transform at all — back to identity"
        );

        // And the simulation itself stops the moment it is pinned outward,
        // rather than pumping dead frames for the rest of the spline.
        assert!(
            ballistic_frames < spline_frames / 2.0,
            "the pinned curve ran {ballistic_frames} frames of a {spline_frames}-frame spline"
        );
    }

    // --- Nested scrolling: innermost-wins arbitration. See the module docs'
    //     *Nested scrolling*. ---

    /// Which layer of a nested fixture an observation came from — an index into
    /// [`Nest`]'s per-layer logs, so one builder serves every depth.
    const OUTER: usize = 0;
    /// The middle layer of the three-deep fixture.
    const MIDDLE: usize = 1;
    /// The innermost scroll surface of a nested fixture.
    const INNER: usize = 2;

    /// What each layer of a nested fixture observed, by layer index.
    #[derive(Default)]
    struct Nest {
        /// Every `ScrollInfo` a layer reported through `on_scroll`.
        scrolls: [Vec<ScrollInfo>; 3],
        /// How many times a layer's pull-to-refresh fired.
        refreshes: [u32; 3],
    }

    /// The pointer phases the deepest, non-scrollable content saw — how a
    /// `Cancel` is attributed to whichever surface sent it.
    #[derive(Clone, Copy, Default)]
    struct ContentSeen {
        downs: u32,
        cancels: u32,
    }

    /// 1000px of ordinary, non-scrollable content tallying what reaches it.
    ///
    /// Counted through an `Rc<Cell<_>>` rather than `EventCtx::state_mut`
    /// because a `Cancel` arm never touches state
    /// (`docs/CODE_STANDARDS.md`'s Interaction Semantics) — and `Cancel`s are
    /// exactly what this probe exists to count.
    struct NestContent(Rc<Cell<ContentSeen>>);
    /// Retained widget for [`NestContent`].
    struct NestContentW(Rc<Cell<ContentSeen>>);

    /// A [`NestContent`] tallying into `seen`.
    fn nest_content(seen: &Rc<Cell<ContentSeen>>) -> NestContent {
        NestContent(Rc::clone(seen))
    }

    /// A [`NestContent`] whose tally nobody reads — the placeholder child a
    /// layer is built with before [`nest`] wires the real nested surface into
    /// its place. Its 1000px height is what gives that layer its content
    /// extent, so the placeholder is load-bearing even after the swap.
    fn spacer_content() -> NestContent {
        NestContent(Rc::new(Cell::new(ContentSeen::default())))
    }

    impl View<Nest> for NestContent {
        type Element = NestContentW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> NestContentW {
            NestContentW(Rc::clone(&self.0))
        }
        fn rebuild(&self, _p: &Self, _e: &mut NestContentW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for NestContentW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 1000.0))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e {
                let mut seen = self.0.get();
                match p.phase {
                    PointerPhase::Down => seen.downs += 1,
                    PointerPhase::Cancel => seen.cancels += 1,
                    _ => {}
                }
                self.0.set(seen);
            }
            EventResult::Ignored
        }
    }

    /// Build and lay out one layer of a nested fixture: a `viewport_h`-tall
    /// viewport over `child`, reporting scrolls and refreshes under `layer`.
    ///
    /// Laid out **tight** rather than by an enclosing surface, because a
    /// `ScrollView` hands its child *unbounded* height — a nested scroll
    /// surface laid out that way sizes its viewport to its own content and has
    /// nothing left to scroll. A real tree bounds it (a `SizedBox`, a list
    /// row's own extent); the fixture states the resulting geometry directly
    /// rather than threading a third widget through every assertion.
    fn nest_surface(child: impl View<Nest>, viewport_h: f64, layer: usize) -> ScrollWidget {
        let view: ScrollView<Nest> = scroll_view(child)
            .on_scroll(move |s: &mut Nest, info| s.scrolls[layer].push(info))
            .on_refresh_release(move |s: &mut Nest| s.refreshes[layer] += 1);
        let mut counter = 0u64;
        let mut w = View::<Nest>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(
            &mut lctx,
            &BoxConstraints::tight(Size::new(200.0, viewport_h)),
        );
        w
    }

    /// [`nest_surface`] with the pre-seam [`RubberBand`] feel pinned
    /// explicitly — the variant the nested tests that assert a `0.5`-resisted
    /// number build their feel-carrying layer from (see
    /// [`laid_out_rubber_band`]); the arbitration itself is physics-agnostic,
    /// so every other layer stays on the platform default.
    fn nest_surface_rubber_band(
        child: impl View<Nest>,
        viewport_h: f64,
        layer: usize,
    ) -> ScrollWidget {
        let mut w = nest_surface(child, viewport_h, layer);
        w.physics = Rc::new(RubberBand::new());
        w
    }

    /// Wire `inner` in as `outer`'s single child — the nesting a real tree
    /// builds through a bounded-height wrapper (see [`nest_surface`]).
    fn nest(outer: &mut ScrollWidget, inner: ScrollWidget) {
        outer.child = ChildPod::new(Box::new(inner));
    }

    /// The nested surface [`nest`] wired under `outer` (single-boxed, unlike an
    /// `AnyView`-erased pod).
    fn nested_of(outer: &ScrollWidget) -> &ScrollWidget {
        (outer.child.widget() as &dyn Any)
            .downcast_ref::<ScrollWidget>()
            .expect("the fixture wired a ScrollWidget child")
    }

    /// The nested `ListView` wired under `outer`.
    fn nested_list_of(outer: &ScrollWidget) -> &crate::list_view::ListViewWidget {
        (outer.child.widget() as &dyn Any)
            .downcast_ref::<crate::list_view::ListViewWidget>()
            .expect("the fixture wired a ListViewWidget child")
    }

    /// Build and lay out a nested `ListView` — 10 rows of 100px in a
    /// `viewport_h`-tall viewport, tight for the same reason
    /// [`nest_surface`] is.
    fn nested_list(viewport_h: f64) -> crate::list_view::ListViewWidget {
        let view: crate::list_view::ListView<Nest> =
            crate::list_view::list_view(10, 100.0, |_| any(spacer_content()));
        let mut counter = 0u64;
        let mut w = View::<Nest>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(
            &mut lctx,
            &BoxConstraints::tight(Size::new(200.0, viewport_h)),
        );
        w
    }

    fn run_nest(w: &mut ScrollWidget, state: &mut Nest, e: &InputEvent, t: f64) {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, e, t);
    }

    /// Park a surface at `offset` px with a wheel scroll (hard-clamped, no
    /// overscroll and no gesture state) — how a real surface reaches a
    /// mid-content position.
    fn park(w: &mut dyn Widget, viewport_h: f64, offset: f64) {
        let mut throwaway = Nest::default();
        let sa: &mut dyn Any = &mut throwaway;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, viewport_h));
        w.event(&mut ctx, &scroll(50.0, false, offset));
    }

    #[test]
    fn outer_defers_when_inner_can_consume() {
        let mut outer = nest_surface(spacer_content(), 200.0, OUTER);
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        let mut inner = nest_surface(nest_content(&seen), 120.0, INNER);
        // Mid-content, at neither edge: the inner's claim comes from actual
        // room, not from a displacement-allowing physics.
        park(&mut inner, 120.0, 400.0);
        assert_eq!(inner.offset(), 400.0);
        nest(&mut outer, inner);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(
            outer.inner_at_down.registered,
            "the nested surface reported itself on the forwarded Down"
        );
        assert!(outer.inner_at_down.can_consume_up_drag);
        assert_eq!(seen.get().downs, 1, "the Down still reached the content");

        // 50px of finger-up drag, past the slop: the outer stands down.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 50.0), 16.0);
        assert!(outer.deferring, "the outer deferred to the nested surface");
        assert!(!outer.scrolling);
        assert_eq!(outer.offset(), 0.0, "…and never moved");
        assert!(state.scrolls[OUTER].is_empty(), "…nor reported a scroll");
        // The one Cancel the content saw came from the INNER's own takeover —
        // the outer sent none, and it is the inner that is now scrolling.
        assert!(nested_of(&outer).scrolling, "the inner took the gesture");
        assert_eq!(seen.get().cancels, 1);

        // The rest of the drag lands in the inner, still never in the outer.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 10.0), 32.0);
        assert_eq!(
            nested_of(&outer).offset(),
            440.0,
            "the inner consumed the 40px"
        );
        assert_eq!(outer.offset(), 0.0);
        assert!(state.scrolls[OUTER].is_empty());
        assert!(!state.scrolls[INNER].is_empty(), "the inner reported it");
        assert_eq!(seen.get().cancels, 1, "no second Cancel from anywhere");
    }

    #[test]
    fn outer_takes_over_when_inner_pinned() {
        let mut outer = nest_surface_rubber_band(spacer_content(), 200.0, OUTER);
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        let mut inner = nest_surface(nest_content(&seen), 120.0, INNER);
        // At its top under a physics that rejects every past-edge proposal:
        // there is nothing a downward drag can do here.
        inner.physics = Rc::new(Clamping::new());
        nest(&mut outer, inner);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        assert!(
            outer.inner_at_down.registered,
            "the inner still reports itself…"
        );
        assert!(
            !outer.inner_at_down.can_consume_down_drag,
            "…pinned against a downward drag"
        );
        assert!(
            outer.inner_at_down.can_consume_up_drag,
            "…though not against an upward one"
        );

        // 40px down, past the slop: the outer takes over exactly as ever.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(outer.scrolling);
        assert!(!outer.deferring);
        assert_eq!(
            seen.get().cancels,
            1,
            "the outer's takeover Cancel reached the content through the inner"
        );
        assert_eq!(
            outer.offset(),
            0.0,
            "the takeover move does not itself scroll"
        );

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 80.0), 32.0);
        assert_eq!(
            outer.offset(),
            -10.0,
            "the resisted 20px past-top overscroll, as ever"
        );
        assert_eq!(nested_of(&outer).offset(), 0.0, "the inner never moved");
        assert!(state.scrolls[INNER].is_empty());
    }

    #[test]
    fn bouncing_inner_wins_even_at_edge() {
        let mut outer = nest_surface(spacer_content(), 200.0, OUTER);
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        // A `RubberBand` inner at the very top: like the bouncing default it
        // rejects nothing, so it can still answer a downward pull with a
        // rubber-band — and its resisted number is the one pinned below.
        let inner = nest_surface_rubber_band(nest_content(&seen), 120.0, INNER);
        nest(&mut outer, inner);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        assert!(
            outer.inner_at_down.can_consume_down_drag,
            "a displacement-allowing physics claims even pinned at the top"
        );

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(
            outer.deferring,
            "the outer defers even though the inner sits at offset 0"
        );
        assert!(nested_of(&outer).scrolling);
        assert_eq!(seen.get().cancels, 1, "the inner's own takeover Cancel");

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 80.0), 32.0);
        assert_eq!(
            nested_of(&outer).offset(),
            -10.0,
            "the inner rubber-bands past its own top"
        );
        assert_eq!(outer.offset(), 0.0, "and the outer stays exactly put");
        assert!(state.scrolls[OUTER].is_empty());
    }

    #[test]
    fn content_fits_inner_does_not_steal_the_drag() {
        // A bouncing-family inner whose content exactly fills its viewport —
        // `max_scroll_extent == min_scroll_extent`, nothing to scroll either
        // way — under the platform default (no `.physics(...)` override).
        // `should_accept_user_offset` is hardcoded `true` for the whole
        // bouncing family, so without a capacity conjunct in
        // `inner_claim_state` this would register and defer forever; the
        // outer must still win the drag (`inner_claim_state`'s doc comment,
        // the module docs' *Nested scrolling*). The outer runs `RubberBand`
        // (like `outer_takes_over_when_inner_pinned`) so its resisted number
        // is the deterministic one pinned below rather than the default's
        // progressive depth curve.
        let mut outer = nest_surface_rubber_band(spacer_content(), 200.0, OUTER);
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        // 1000px viewport over the fixture's fixed 1000px content: an exact
        // fit, so `max_offset() == 0.0`.
        let inner = nest_surface(nest_content(&seen), 1000.0, INNER);
        assert_eq!(
            inner.max_offset(),
            0.0,
            "the fixture's content exactly fits"
        );
        nest(&mut outer, inner);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        assert!(
            !outer.inner_at_down.registered,
            "no real capacity to scroll, so the claim never registers"
        );

        // 40px down, past the slop: the outer takes over exactly as the
        // no-nested-scrollable case always has.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(outer.scrolling, "the outer takes the drag over");
        assert!(!outer.deferring);
        assert_eq!(
            seen.get().cancels,
            1,
            "the outer's takeover Cancel reached the content through the inner"
        );

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 80.0), 32.0);
        assert_eq!(
            outer.offset(),
            -10.0,
            "the resisted 20px past-top overscroll, same as a pinned-inner takeover"
        );
        assert_eq!(nested_of(&outer).offset(), 0.0, "the inner never moved");
        assert_eq!(nested_of(&outer).edge_pull, 0.0, "…nor accrued any pull");
        assert!(state.scrolls[INNER].is_empty(), "the inner saw nothing");
    }

    #[test]
    fn no_inner_behavior_identical() {
        // The pre-existing takeover, unchanged with arbitration in place: a
        // plain non-scrollable child registers nothing, so nothing defers.
        // Viewport 100 over 1000px of content — `laid_out(200, 100, 1000)`'s
        // geometry, so the numbers below are the shipped ones.
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        let mut w = nest_surface(nest_content(&seen), 100.0, OUTER);
        assert_eq!(w.max_offset(), 900.0);
        let mut state = Nest::default();

        run_nest(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(!w.inner_at_down.registered, "a plain child claims nothing");
        assert_eq!(seen.get().downs, 1);

        run_nest(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0);
        assert!(w.scrolling, "the slop still takes the gesture over");
        assert!(!w.deferring);
        assert_eq!(seen.get().cancels, 1, "exactly one child Cancel, as before");
        assert_eq!(w.offset(), 0.0, "the takeover move does not itself scroll");

        run_nest(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(
            w.offset(),
            30.0,
            "the 30px `drag_past_slop_scrolls_the_offset` pins"
        );
        assert_eq!(seen.get().cancels, 1, "no further move reaches the child");
    }

    #[test]
    fn three_deep_nesting_pairs_nearest() {
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        let innermost = nest_surface(nest_content(&seen), 80.0, INNER);
        let mut middle = nest_surface(spacer_content(), 140.0, MIDDLE);
        nest(&mut middle, innermost);
        let mut outer = nest_surface(spacer_content(), 200.0, OUTER);
        nest(&mut outer, middle);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 75.0), 0.0);
        // Each level learns about the level it actually contains and no
        // further: the innermost's own child is plain content, and nothing
        // propagated its (absent) claim up past the middle.
        assert!(outer.inner_at_down.registered, "outer sees the middle");
        assert!(
            nested_of(&outer).inner_at_down.registered,
            "middle sees the innermost"
        );
        assert!(
            !nested_of(nested_of(&outer)).inner_at_down.registered,
            "the innermost sees no scrollable below it"
        );

        // A finger-up drag every layer could consume: the innermost gets it.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 35.0), 16.0);
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 5.0), 32.0);
        assert!(outer.deferring && nested_of(&outer).deferring);
        assert!(!nested_of(nested_of(&outer)).deferring);
        assert!(nested_of(nested_of(&outer)).scrolling);
        assert_eq!(outer.offset(), 0.0, "the outermost never moved");
        assert_eq!(nested_of(&outer).offset(), 0.0, "nor the middle");
        assert_eq!(
            nested_of(nested_of(&outer)).offset(),
            30.0,
            "only the innermost took the drag"
        );
        assert!(state.scrolls[OUTER].is_empty() && state.scrolls[MIDDLE].is_empty());
        assert!(!state.scrolls[INNER].is_empty());
    }

    #[test]
    fn up_and_cancel_still_reach_child_when_deferring() {
        // The device-gate case end to end: a refresh surface owning its own
        // scroll, under a page-level scroll. The outer must forward the whole
        // gesture — including the release that fires the refresh.
        let mut outer = nest_surface(spacer_content(), 200.0, OUTER);
        let seen = Rc::new(Cell::new(ContentSeen::default()));
        let inner = nest_surface_rubber_band(nest_content(&seen), 120.0, INNER);
        nest(&mut outer, inner);

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(
            outer.deferring,
            "the inner can rubber-band, so the outer defers"
        );
        // Pull the inner well past its own refresh trigger (150px raw, halved
        // by the rubber-band resistance to 75 > REFRESH_TRIGGER_PX).
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 210.0), 32.0);
        assert_eq!(nested_of(&outer).offset(), -75.0);
        assert!(crossed_refresh_trigger(nested_of(&outer).edge_pull));

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Up, 210.0), 48.0);
        assert_eq!(
            state.refreshes[INNER], 1,
            "the forwarded Up fired the INNER's refresh exactly once"
        );
        assert_eq!(state.refreshes[OUTER], 0, "and never the outer's");
        assert_eq!(outer.offset(), 0.0, "the outer never scrolled at all");
        assert!(state.scrolls[OUTER].is_empty());
        // The Up clears the arbitration state, so the next gesture arbitrates
        // from scratch rather than inheriting this one's answer.
        assert!(!outer.deferring);
        assert!(!outer.inner_at_down.registered);
        assert!(!outer.scrolling && !outer.down_active);

        // A second gesture, cancelled mid-drag: the Cancel is forwarded too.
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 64.0);
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 80.0);
        assert!(outer.deferring, "still the inner's gesture");
        run_nest(
            &mut outer,
            &mut state,
            &ev(PointerPhase::Cancel, 60.0),
            96.0,
        );
        // Three Cancels all told: the inner's takeover in each of the two
        // gestures, plus this forwarded one reaching the content through it.
        assert_eq!(seen.get().cancels, 3);
        assert_eq!(state.refreshes[INNER], 1, "a Cancel never fires a refresh");
        assert!(
            !outer.deferring,
            "the Cancel clears the arbitration state too"
        );
        assert!(!outer.inner_at_down.registered);
    }

    #[test]
    fn a_nested_list_view_wins_the_drag_from_a_scroll_view() {
        let mut outer = nest_surface(spacer_content(), 200.0, OUTER);
        let mut list = nested_list(150.0);
        park(&mut list, 150.0, 300.0);
        assert_eq!(list.offset(), 300.0);
        outer.child = ChildPod::new(Box::new(list));

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(
            outer.inner_at_down.registered,
            "a nested ListView reports itself on the same seam"
        );
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 50.0), 16.0);
        assert!(outer.deferring);
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 10.0), 32.0);
        assert_eq!(outer.offset(), 0.0, "the outer never moved");
        assert!(state.scrolls[OUTER].is_empty());
        assert_eq!(
            nested_list_of(&outer).offset(),
            340.0,
            "the nested list took the 40px"
        );
    }

    #[test]
    fn a_pinned_nested_list_view_hands_the_drag_back_to_the_scroll_view() {
        let mut outer = nest_surface_rubber_band(spacer_content(), 200.0, OUTER);
        let mut list = nested_list(150.0);
        list.physics = Rc::new(Clamping::new());
        outer.child = ChildPod::new(Box::new(list));

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        assert!(
            !outer.inner_at_down.can_consume_down_drag,
            "the nested list is pinned at its own top"
        );
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(outer.scrolling, "so the outer takes over as it always has");
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 80.0), 32.0);
        assert_eq!(
            outer.offset(),
            -10.0,
            "the resisted 20px past-top overscroll"
        );
        assert_eq!(
            nested_list_of(&outer).offset(),
            0.0,
            "the nested list never moved"
        );
    }

    #[test]
    fn a_content_fits_nested_list_view_does_not_steal_the_drag() {
        // The `ListView` twin of `content_fits_inner_does_not_steal_the_drag`:
        // both widgets route the claim through the same shared
        // `inner_claim_state`, so this pins that the capacity conjunct
        // applies here too rather than being a `ScrollWidget`-only fix.
        let mut outer = nest_surface_rubber_band(spacer_content(), 200.0, OUTER);
        // 10 rows of 100px in a 1000px viewport: an exact fit.
        let list = nested_list(1000.0);
        assert_eq!(list.max_offset(), 0.0, "the fixture's content exactly fits");
        outer.child = ChildPod::new(Box::new(list));

        let mut state = Nest::default();
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Down, 20.0), 0.0);
        assert!(
            !outer.inner_at_down.registered,
            "no real capacity to scroll, so the claim never registers"
        );

        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 60.0), 16.0);
        assert!(outer.scrolling, "the outer takes the drag over");
        run_nest(&mut outer, &mut state, &ev(PointerPhase::Move, 80.0), 32.0);
        assert_eq!(
            outer.offset(),
            -10.0,
            "the resisted 20px past-top overscroll, same as a pinned-inner takeover"
        );
        assert_eq!(
            nested_list_of(&outer).offset(),
            0.0,
            "the nested list never moved"
        );
    }

    // --- (07) The public builder surface: `.physics(...)`/`.overscroll_effect(...)` ---

    #[test]
    fn physics_builder_installs_custom_physics() {
        let view: ScrollView<()> = scroll_view(leaf(200.0, 1000.0)).physics(NeverScrollable::new());
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));

        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        // Same slop-crossing shape as `drag_past_slop_scrolls_the_offset`, but
        // `NeverScrollable` refuses the drag outright.
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(w.offset(), 0.0, "NeverScrollable must refuse the drag");
        assert!(
            !w.scrolling,
            "NeverScrollable must never take the gesture over"
        );

        // A default-built twin (no `.physics(...)` call) still scrolls normally
        // under `RubberBand` — proving the builder, not some global default
        // change, is what reached the widget above.
        let mut default_w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut default_w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut default_w, &ev(PointerPhase::Move, 70.0), 16.0);
        dispatch(&mut default_w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(
            default_w.offset(),
            30.0,
            "the default twin scrolls normally"
        );
    }

    #[test]
    fn effect_builder_reaches_widget() {
        let view: ScrollView<()> =
            scroll_view(leaf(200.0, 1000.0)).overscroll_effect(OverscrollEffect::Stretch);
        let mut counter = 0u64;
        let w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.effect, OverscrollEffect::Stretch);

        // A default-built twin takes the platform pairing instead — asserted
        // against the selector rather than a literal, so this reads the same
        // on the Android arm (Stretch) as on this one (Translate).
        let default_view: ScrollView<()> = scroll_view(leaf(200.0, 1000.0));
        let default_w = View::<()>::build(&default_view, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            default_w.effect,
            crate::physics::default_overscroll_effect()
        );
        // …which on this host is translate overscroll; the Android arm is
        // pinned beside the selector itself, in `physics`' own tests.
        #[cfg(not(target_os = "android"))]
        assert_eq!(default_w.effect, OverscrollEffect::Translate);
    }

    #[test]
    fn rebuild_preserves_builder_physics() {
        let physics_view: ScrollView<()> =
            scroll_view(leaf(200.0, 1000.0)).physics(NeverScrollable::new());
        let mut counter = 0u64;
        let mut w = View::<()>::build(&physics_view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));

        // Rebuilding against an identical `.physics(...)`-carrying view
        // reinstalls it (unconditionally, like the erased callbacks) — still
        // refuses the drag.
        View::<()>::rebuild(
            &physics_view,
            &physics_view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(
            w.offset(),
            0.0,
            "still installed after a re-asserting rebuild"
        );
        assert!(!w.scrolling);
        // Clear the armed gesture before the next rebuild's own probe.
        dispatch(&mut w, &ev(PointerPhase::Cancel, 40.0), 48.0);

        // Rebuilding against a view with no `.physics(...)` call at all leaves
        // the widget's currently-installed physics untouched — it does not
        // revert to the `RubberBand` default.
        let plain_view: ScrollView<()> = scroll_view(leaf(200.0, 1000.0));
        View::<()>::rebuild(
            &plain_view,
            &physics_view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(
            w.offset(),
            0.0,
            "a rebuild whose view carries no .physics(...) leaves the widget's physics untouched"
        );
        assert!(!w.scrolling);
    }

    /// A focused editable inside the viewport must see a clipboard verb: an
    /// `EditCommand` is focus-routed, so it takes the same bypass `Key`/`Ime`
    /// take rather than the gesture machinery (and never a hit test).
    #[test]
    fn an_edit_command_reaches_the_focused_child() {
        use crate::text_input;
        use frust_core::{EditCommand, RenderRoot};

        struct Field {
            value: String,
        }
        fn logic(state: &mut Field) -> ScrollView<Field> {
            scroll_view(text_input(
                state.value.clone(),
                |s: &mut Field, v: String| {
                    s.value = v;
                },
            ))
        }

        let mut state = Field {
            value: "hello".to_string(),
        };
        let mut root: RenderRoot<Field, ScrollView<Field>> = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 100.0));

        // Tap the field through the viewport so it holds the recorded focus path.
        root.event(&mut state, &ev(PointerPhase::Down, 10.0));
        root.event(&mut state, &ev(PointerPhase::Up, 10.0));
        assert!(root.is_focus_active(), "the tap focused the child field");

        root.event(&mut state, &InputEvent::EditCommand(EditCommand::SelectAll));
        root.event(&mut state, &InputEvent::EditCommand(EditCommand::Copy));

        assert_eq!(
            root.take_clipboard_write().as_deref(),
            Some("hello"),
            "the copy was answered by the child, through this router"
        );
        assert_eq!(state.value, "hello", "a copy edits nothing");
    }
}

/// A scroll view enclosing a multi-contact recognizer, driven through a real
/// `RenderRoot`: the second finger reaches only the widget that opted into it,
/// a takeover ends that opt-in, and a hit-tested `Scale` keeps its result.
#[cfg(test)]
mod contact_tests {
    use super::*;
    use crate::{PanZoomTransform, pan_zoom, pinch_detector};
    use frust_core::RenderRoot;
    use frust_core::event::{PointerId, ScaleEvent, ScalePhase};
    use std::any::Any;

    #[derive(Default)]
    struct App {
        transforms: Vec<PanZoomTransform>,
        scales: Vec<ScaleEvent>,
    }

    type Seen = Rc<RefCell<Vec<(PointerId, PointerPhase)>>>;

    /// A 1000-px-tall content leaf. Logs every pointer event it receives (into
    /// a shared log, never app state, so its `Cancel` arm stays state-free).
    /// With `grabs` it captures every primary `Down` and handles it — opting
    /// into the gesture's other contacts too with `opt_in` — and otherwise
    /// ignores pointers. With `consumes` it reports `Handled` for a `Scale`
    /// and for a broadcast; otherwise it ignores both.
    #[derive(Clone)]
    struct Tall {
        seen: Seen,
        grabs: bool,
        opt_in: bool,
        consumes: bool,
    }
    struct TallWidget(Tall);
    impl View<App> for Tall {
        type Element = TallWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TallWidget {
            TallWidget(self.clone())
        }
        fn rebuild(&self, _p: &Self, _e: &mut TallWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for TallWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(400.0, 1000.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) => {
                    self.0.seen.borrow_mut().push((ctx.pointer_id(), p.phase));
                    if !self.0.grabs {
                        return EventResult::Ignored;
                    }
                    if p.phase == PointerPhase::Down {
                        ctx.capture_pointer();
                        if self.0.opt_in {
                            ctx.capture_contacts();
                        }
                    }
                    EventResult::Handled
                }
                InputEvent::Scale(_) | InputEvent::Housekeeping if self.0.consumes => {
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    fn tall(grabs: bool, opt_in: bool, consumes: bool) -> (Tall, Seen) {
        let seen = Seen::default();
        let view = Tall {
            seen: seen.clone(),
            grabs,
            opt_in,
            consumes,
        };
        (view, seen)
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn touch(slot: u32, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(slot),
            event: PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            },
        }
    }

    /// A root over `logic`, laid out in a 400 × 300 window.
    fn root_over<V: View<App>>(logic: impl Fn() -> V + 'static) -> (RenderRoot<App, V>, App) {
        let mut root: RenderRoot<App, V> = RenderRoot::new();
        let mut state = App::default();
        root.rebuild(&mut move |_: &mut App| logic(), &mut state);
        root.layout(Size::new(400.0, 300.0));
        (root, state)
    }

    fn viewport(root: &RenderRoot<App, ScrollView<App>>) -> &ScrollWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ScrollWidget>()
            .expect("root is a ScrollWidget")
    }

    #[test]
    fn a_second_finger_never_rearms_the_viewport_around_a_pan_zoom() {
        use PointerPhase::{Down, Move};
        let (content, seen) = tall(false, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(
                pan_zoom(content.clone()).on_transform(|s: &mut App, t| s.transforms.push(t)),
            )
        });
        let t0 = PointerId::touch(0);
        // Two fingers stacked vertically, then the first one pans sideways —
        // no vertical travel at all for the claimant. A viewport that took the
        // second finger's `Down` as its own would measure this move against
        // *that* finger's position, cross its slop, and steal the gesture.
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        assert_eq!(root.pointer_capture_claimant(), Some(t0));
        assert!(root.pointer_capture_contacts(), "pan_zoom opted in");
        root.event(&mut state, &touch(1, Down, 100.0, 250.0));
        root.event(&mut state, &touch(0, Move, 110.0, 100.0));
        root.event(&mut state, &touch(0, Move, 120.0, 100.0));

        let scroll = viewport(&root);
        assert_eq!(scroll.offset(), 0.0, "the viewport never scrolled");
        assert!(!scroll.scrolling, "and never took the gesture over");
        assert_eq!(
            scroll.down_start,
            Point::new(100.0, 100.0),
            "its drag is still anchored on the claimant"
        );
        assert_eq!(
            state.transforms.last().map(|t| t.offset),
            Some(kurbo::Vec2::new(20.0, 0.0)),
            "pan_zoom kept receiving the claimant's moves — it was never cancelled"
        );
        assert!(
            !seen
                .borrow()
                .iter()
                .any(|(_, phase)| *phase == PointerPhase::Cancel)
        );
        assert_eq!(root.pointer_capture_claimant(), Some(t0));
        assert!(
            root.pointer_capture_contacts(),
            "still routing the second finger to pan_zoom"
        );
    }

    #[test]
    fn a_pinch_inside_a_viewport_reports_its_scale_and_leaves_the_viewport_alone() {
        use PointerPhase::{Down, Move, Up};
        let (content, _) = tall(true, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(pinch_detector(content.clone()).on_scale(|s: &mut App, e| s.scales.push(e)))
        });
        let mut sink = NullScene;
        let ms = |ms: u64| FrameTime::from_nanos(ms * 1_000_000);
        root.paint(&mut sink, ms(0));
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        root.paint(&mut sink, ms(16));
        // The second finger spreads straight down: vertical travel a viewport
        // that saw it would read as a scroll drag.
        root.event(&mut state, &touch(1, Move, 120.0, 160.0));
        root.paint(&mut sink, ms(32));
        root.event(&mut state, &touch(1, Move, 120.0, 250.0));
        root.event(&mut state, &touch(1, Up, 120.0, 250.0));

        let phases: Vec<ScalePhase> = state.scales.iter().map(|e| e.phase).collect();
        assert_eq!(
            phases,
            [ScalePhase::Begin, ScalePhase::Update, ScalePhase::End],
            "the pinch recognizer saw the whole spread"
        );
        let scroll = viewport(&root);
        assert_eq!(scroll.offset(), 0.0);
        assert!(!scroll.scrolling);
        assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
        root.event(&mut state, &touch(0, Up, 100.0, 100.0));
        assert!(!root.is_pointer_captured());
    }

    #[test]
    fn a_vertical_drag_over_a_non_panning_child_still_scrolls_and_ends_its_opt_in() {
        use PointerPhase::{Cancel, Down, Move, Up};
        let (content, seen) = tall(true, true, false);
        let (mut root, mut state) = root_over(move || scroll_view(content.clone()));
        let t0 = PointerId::touch(0);
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        assert!(root.pointer_capture_contacts(), "the child opted in");

        // Past the slop: the viewport takes over, cancels the child and
        // releases it — and the root hears about it.
        root.event(&mut state, &touch(0, Move, 100.0, 60.0));
        assert!(viewport(&root).scrolling);
        assert_eq!(*seen.borrow(), [(t0, Down), (t0, Cancel)]);
        assert_eq!(
            root.pointer_capture_claimant(),
            Some(t0),
            "the drag is still the first finger's"
        );
        assert!(
            !root.pointer_capture_contacts(),
            "but the cancelled child no longer gets other fingers"
        );

        // A second finger now reaches nothing.
        let outcome = root.event(&mut state, &touch(1, Down, 100.0, 200.0));
        assert!(!outcome.handled);
        assert_eq!(seen.borrow().len(), 2);

        // The claimant keeps scrolling, and its release ends the gesture.
        root.event(&mut state, &touch(0, Move, 100.0, 30.0));
        assert_eq!(viewport(&root).offset(), 30.0);
        root.event(&mut state, &touch(0, Up, 100.0, 30.0));
        assert!(!root.is_pointer_captured());
        assert_eq!(
            seen.borrow().len(),
            2,
            "the child heard nothing after its Cancel"
        );
    }

    /// Build a bare scroll widget over `child` in a 400 × 300 viewport.
    fn bare(child: Tall) -> ScrollWidget {
        let view: ScrollView<App> = scroll_view(child);
        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(400.0, 300.0)),
        );
        w
    }

    /// Dispatch into a bare scroll widget: its result and whether a capture
    /// release bubbled out of it.
    fn run(w: &mut ScrollWidget, event: &InputEvent, t_ms: f64) -> (EventResult, bool) {
        let mut state = App::default();
        let sa: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(400.0, 300.0));
        let result = w.event_at(&mut ctx, event, t_ms);
        (result, ctx.is_capture_released())
    }

    fn mouse(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(100.0, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn a_takeover_raises_the_release_only_for_an_opted_in_child() {
        for opt_in in [true, false] {
            let (content, _) = tall(true, opt_in, false);
            let mut w = bare(content);
            assert!(!run(&mut w, &mouse(PointerPhase::Down, 100.0), 0.0).1);
            let (_, released) = run(&mut w, &mouse(PointerPhase::Move, 60.0), 16.0);
            assert!(w.scrolling, "took over");
            assert!(!w.child.is_active(), "and released the child");
            assert_eq!(released, opt_in, "signalled only when the child opted in");
        }
    }

    #[test]
    fn a_scale_keeps_the_childs_result_and_a_broadcast_is_never_consumed() {
        let scale = InputEvent::Scale(ScaleEvent {
            phase: ScalePhase::Update,
            scale_delta: 1.1,
            focal: Point::new(100.0, 100.0),
            velocity: 0.0,
        });
        let (consumer, _) = tall(false, false, true);
        let mut w = bare(consumer);
        assert_eq!(run(&mut w, &scale, 0.0).0, EventResult::Handled);
        assert_eq!(
            run(&mut w, &InputEvent::Housekeeping, 0.0).0,
            EventResult::Ignored,
            "a broadcast is never consumed, whatever the child returned"
        );
        let (bystander, _) = tall(false, false, false);
        let mut w = bare(bystander);
        assert_eq!(run(&mut w, &scale, 0.0).0, EventResult::Ignored);
    }

    #[test]
    fn a_wheel_zoom_handled_inside_the_viewport_is_not_applied_twice() {
        let (content, _) = tall(false, false, false);
        let (mut root, mut state) = root_over(move || {
            pinch_detector(scroll_view(
                pan_zoom(content.clone()).on_transform(|s: &mut App, t| s.transforms.push(t)),
            ))
            .on_scale(|s: &mut App, e| s.scales.push(e))
        });
        let outcome = root.event(
            &mut state,
            &InputEvent::Scale(ScaleEvent {
                phase: ScalePhase::Update,
                scale_delta: 1.5,
                focal: Point::new(100.0, 100.0),
                velocity: 0.0,
            }),
        );
        assert!(outcome.handled);
        assert_eq!(state.transforms.len(), 1, "pan_zoom zoomed once");
        assert!(
            state.scales.is_empty(),
            "and the enclosing recognizer did not zoom again"
        );
    }

    // --- The multi-contact veto: surviving the claimant's own travel -------

    #[test]
    fn a_pinch_survives_the_claimants_own_travel_past_slop() {
        use PointerPhase::{Down, Move};
        let (content, _seen) = tall(false, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(pinch_detector(content.clone()).on_scale(|s: &mut App, e| s.scales.push(e)))
        });
        let mut sink = NullScene;
        let ms = |ms: u64| FrameTime::from_nanos(ms * 1_000_000);
        root.paint(&mut sink, ms(0));
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        root.paint(&mut sink, ms(16));
        // The CLAIMANT's own finger spreads the pair vertically, past the
        // viewport's `TOUCH_SLOP` — a viewport reading only the `Down`-time
        // claim snapshot (always unregistered here; `pinch_detector` is not a
        // nested scrollable) would steal this as an ordinary scroll drag
        // (the counterexample this card fixes).
        root.event(&mut state, &touch(0, Move, 100.0, 40.0));
        root.paint(&mut sink, ms(32));
        root.event(&mut state, &touch(0, Move, 100.0, 10.0));

        let phases: Vec<ScalePhase> = state.scales.iter().map(|e| e.phase).collect();
        assert_eq!(
            phases,
            [ScalePhase::Begin, ScalePhase::Update],
            "the pinch recognizer saw the whole spread"
        );
        let scroll = viewport(&root);
        assert_eq!(scroll.offset(), 0.0, "the viewport never scrolled");
        assert!(!scroll.scrolling, "and never took the gesture over");
        assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
        assert!(
            root.pointer_capture_contacts(),
            "the viewport never cancelled/released the detector's opt-in"
        );
    }

    #[test]
    fn a_pinch_over_a_child_owned_press_survives_the_claimants_own_travel() {
        use PointerPhase::{Down, Move};
        // `grabs = true`: the content captures the primary `Down` itself, so
        // `PanZoomWidget::begin_gesture` takes the child-owned branch, which
        // publishes nothing into the nested-scroll claim.
        let (content, _seen) = tall(true, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(
                pan_zoom(content.clone()).on_transform(|s: &mut App, t| s.transforms.push(t)),
            )
        });
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        // The claimant's finger travels well past TOUCH_SLOP vertically while
        // a second contact is tracked — exactly the counterexample this card
        // fixes for `PanZoomView`'s child-owned-press branch.
        root.event(&mut state, &touch(0, Move, 100.0, 40.0));
        root.event(&mut state, &touch(1, Move, 180.0, 40.0));

        let scroll = viewport(&root);
        assert_eq!(scroll.offset(), 0.0, "the viewport never scrolled");
        assert!(!scroll.scrolling, "and never took the gesture over");
        assert!(
            !state.transforms.is_empty(),
            "pan_zoom's own pinch zoomed the view instead"
        );
    }

    #[test]
    fn a_single_finger_drag_still_scrolls_through_a_pinch_detector_with_no_second_finger() {
        use PointerPhase::{Down, Move};
        let (content, _seen) = tall(false, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(pinch_detector(content.clone()).on_scale(|s: &mut App, e| s.scales.push(e)))
        });
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(0, Move, 100.0, 40.0));
        assert!(
            viewport(&root).scrolling,
            "no second finger ever arrived to veto the takeover"
        );
        root.event(&mut state, &touch(0, Move, 100.0, 10.0));
        assert_eq!(viewport(&root).offset(), 30.0);
        assert!(state.scales.is_empty(), "never a pinch with one finger");
    }

    #[test]
    fn releasing_the_second_finger_clears_the_veto_and_scrolling_resumes() {
        use PointerPhase::{Down, Move, Up};
        let (content, _seen) = tall(true, false, false);
        let (mut root, mut state) = root_over(move || {
            scroll_view(pinch_detector(content.clone()).on_scale(|s: &mut App, e| s.scales.push(e)))
        });
        let mut sink = NullScene;
        let ms = |ms: u64| FrameTime::from_nanos(ms * 1_000_000);
        root.paint(&mut sink, ms(0));
        root.event(&mut state, &touch(0, Down, 100.0, 100.0));
        root.event(&mut state, &touch(1, Down, 120.0, 100.0));
        root.paint(&mut sink, ms(16));
        root.event(&mut state, &touch(0, Move, 100.0, 40.0)); // past slop while paired: no takeover
        assert!(
            !viewport(&root).scrolling,
            "the pinch still owns the gesture"
        );

        root.event(&mut state, &touch(1, Up, 120.0, 40.0)); // the second finger lifts: pinch ends

        // The claimant's very next `Move` is measured against its original
        // `down_start` as usual (the veto does not replay the suppressed slop
        // check) — already well past `TOUCH_SLOP`, so the viewport takes the
        // drag over immediately once the veto clears, per the existing
        // single-finger scroll contract.
        root.event(&mut state, &touch(0, Move, 100.0, 10.0));
        assert!(
            viewport(&root).scrolling,
            "the viewport resumed scrolling once the pinch ended"
        );
    }
}

#[cfg(test)]
mod controller_tests {
    //! The [`ScrollController`] seam: deferred application of recorded
    //! commands, clamping, fling interruption, publishing, and the
    //! one-surface-per-handle binding rules.

    use std::any::Any;

    use frust_core::{Curve, FrameTime, RenderRoot};

    use super::*;
    use crate::physics::parity::Bouncing;
    use crate::physics::rubber_band::RubberBand;
    use crate::test_support::{RecordingScene, leaf};

    const VIEWPORT: Size = Size::new(200.0, 100.0);
    const CONTENT_H: f64 = 1000.0;
    const MAX: f64 = CONTENT_H - 100.0;

    fn view(controller: &ScrollController) -> ScrollView<()> {
        scroll_view(leaf(200.0, CONTENT_H)).controller(controller.clone())
    }

    fn build(view: &ScrollView<()>) -> ScrollWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(view: &ScrollView<()>, prev: &ScrollView<()>, w: &mut ScrollWidget) -> ChangeFlags {
        let mut counter = 100u64;
        View::<()>::rebuild(view, prev, w, &mut BuildCtx::new(&mut counter))
    }

    fn lay(w: &mut ScrollWidget) {
        w.layout(&mut LayoutCtx::new(), &BoxConstraints::loose(VIEWPORT));
    }

    /// Paint once, returning whether the paint asked for another frame.
    fn paint(w: &mut ScrollWidget) -> bool {
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        w.paint(&mut ctx, &mut RecordingScene::default());
        ctx.needs_frame()
    }

    fn send(w: &mut ScrollWidget, event: &InputEvent, t_ms: f64) {
        let mut unit = ();
        let state: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(state, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, event, t_ms);
    }

    fn pointer(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    fn wheel(dy: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(10.0, 50.0),
            delta: ScrollDelta::Pixels(0.0, dy),
        }
    }

    /// Drag up past the slop with enough speed that the release flings.
    fn fling(w: &mut ScrollWidget) {
        send(w, &pointer(PointerPhase::Down, 100.0), 0.0);
        send(w, &pointer(PointerPhase::Move, 75.0), 16.0);
        send(w, &pointer(PointerPhase::Move, 50.0), 32.0);
        send(w, &pointer(PointerPhase::Up, 50.0), 32.0);
    }

    fn frame_time(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// Paint the full widget (not just the fling pump) at an explicit frame
    /// time, so `drain_controller`/`apply_animate_to` and the ballistic pump
    /// both see a real, advancing clock — what an `animate_to` tween needs to
    /// progress across more than one frame. Returns whether the paint asked
    /// for another one.
    fn paint_at(w: &mut ScrollWidget, ms: f64) -> bool {
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(ms));
        w.paint(&mut ctx, &mut RecordingScene::default());
        ctx.needs_frame()
    }

    /// A themed [`PaintCtx`] at `ms`, with `motion.reduce_motion` set —
    /// `animate_to`'s collapse-to-jump path reads it the same way
    /// [`button`](crate::button)'s loading spinner does.
    fn paint_at_with_reduce_motion(w: &mut ScrollWidget, ms: f64, theme: &Theme) -> bool {
        let mut ctx = PaintCtx::for_test(Point::ZERO, w.viewport, frame_time(ms)).with_theme(theme);
        w.paint(&mut ctx, &mut RecordingScene::default());
        ctx.needs_frame()
    }

    #[test]
    fn a_jump_recorded_before_layout_applies_after_the_first_layout() {
        let controller = ScrollController::new();
        controller.jump_to(400.0);
        let view = view(&controller);
        let mut w = build(&view);
        assert!(controller.is_attached());
        assert_eq!(w.offset(), 0.0, "nothing applies before a measured extent");
        // A paint before any layout must not clamp the jump against zero.
        paint(&mut w);
        assert_eq!(w.offset(), 0.0);
        lay(&mut w);
        assert_eq!(w.offset(), 400.0);
        assert_eq!(controller.offset(), 400.0);
        assert_eq!(controller.max_offset(), MAX);
        assert_eq!(controller.viewport_extent(), VIEWPORT.height);
    }

    #[test]
    fn a_jump_clamps_to_the_scroll_range() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        let mut jump = |target: f64| {
            controller.jump_to(target);
            paint(&mut w);
            w.offset()
        };
        assert_eq!(jump(5000.0), MAX);
        assert_eq!(jump(-50.0), 0.0);
        assert_eq!(jump(f64::INFINITY), MAX);
        assert_eq!(jump(f64::NAN), MAX, "NaN is ignored");
        assert_eq!(jump(f64::NEG_INFINITY), 0.0);
        assert_eq!(controller.offset(), 0.0);
    }

    #[test]
    fn a_jump_during_a_fling_stops_it() {
        // Both release paths: the legacy fling (`RubberBand`) and a
        // physics-supplied ballistic simulation (the bouncing family).
        let physics: [Rc<dyn ScrollPhysics>; 2] =
            [Rc::new(RubberBand::new()), Rc::new(Bouncing::new())];
        for physics in physics {
            let controller = ScrollController::new();
            let view = view(&controller);
            let mut w = build(&view);
            w.physics = physics;
            lay(&mut w);
            fling(&mut w);
            assert!(w.is_flinging(), "the release starts post-release motion");
            controller.jump_to(500.0);
            let needs_frame = paint(&mut w);
            assert!(!w.is_flinging(), "the jump ends the fling");
            assert_eq!(w.offset(), 500.0);
            assert!(!needs_frame, "nothing is left animating");
            // Later frames leave the jumped-to position alone.
            paint(&mut w);
            assert!(!w.tick(16.0));
            assert_eq!(w.offset(), 500.0);
            assert_eq!(controller.offset(), 500.0);
        }
    }

    #[test]
    fn on_change_hears_user_scrolls_and_jumps() {
        let controller = ScrollController::new();
        let heard = Rc::new(RefCell::new(Vec::<ScrollInfo>::new()));
        let sink = Rc::clone(&heard);
        let _subscription = controller.on_change(move |info| sink.borrow_mut().push(info));
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        assert!(
            heard.borrow().iter().all(|i| i.offset == 0.0),
            "the first layout publishes only the resting extent"
        );
        heard.borrow_mut().clear();

        send(&mut w, &wheel(30.0), 0.0);
        let at = |offset: f64| ScrollInfo {
            offset,
            max_offset: MAX,
            overscroll: 0.0,
        };
        assert_eq!(heard.borrow().as_slice(), &[at(30.0)]);

        // A drag: takeover at the slop, then a 25 px move.
        send(&mut w, &pointer(PointerPhase::Down, 100.0), 0.0);
        send(&mut w, &pointer(PointerPhase::Move, 75.0), 16.0);
        send(&mut w, &pointer(PointerPhase::Move, 50.0), 32.0);
        assert_eq!(heard.borrow().last().copied(), Some(at(55.0)));
        assert_eq!(controller.offset(), w.offset());
        send(&mut w, &pointer(PointerPhase::Cancel, 50.0), 48.0);
        heard.borrow_mut().clear();

        controller.jump_to(600.0);
        lay(&mut w);
        assert_eq!(heard.borrow().as_slice(), &[at(600.0)]);
        // Re-publishing an unchanged position stays silent.
        paint(&mut w);
        lay(&mut w);
        assert_eq!(heard.borrow().len(), 1);
    }

    #[test]
    fn dropping_a_subscription_unsubscribes_it() {
        let controller = ScrollController::new();
        let heard = Rc::new(Cell::new(0u32));
        let sink = Rc::clone(&heard);
        let subscription = controller.on_change(move |_| sink.set(sink.get() + 1));
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.jump_to(10.0);
        lay(&mut w);
        let before = heard.get();
        assert!(before > 0);
        drop(subscription);
        controller.jump_to(20.0);
        lay(&mut w);
        assert_eq!(heard.get(), before);
    }

    #[test]
    fn a_jump_wakes_a_frame_gated_shell() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        frust_core::take_pending_result_flush();
        controller.jump_to(42.0);
        assert!(
            frust_core::has_pending_result_flush(),
            "recording owes a frame even from outside any input path"
        );
        let same = self::view(&controller);
        assert!(rebuild(&same, &view, &mut w).needs_paint());
        lay(&mut w);
        assert_eq!(w.offset(), 42.0);
        let same_again = self::view(&controller);
        assert_eq!(rebuild(&same_again, &same, &mut w), ChangeFlags::NONE);
    }

    #[test]
    fn on_scroll_hears_a_jump_through_the_next_frames_flush() {
        let controller = ScrollController::new();
        let log = Rc::new(RefCell::new(Vec::<ScrollInfo>::new()));
        let (handle, sink) = (controller.clone(), Rc::clone(&log));
        let mut logic = move |_: &mut ()| {
            let sink = Rc::clone(&sink);
            scroll_view(leaf(200.0, CONTENT_H))
                .on_scroll(move |_: &mut (), info| sink.borrow_mut().push(info))
                .controller(handle.clone())
        };
        let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
        let mut state = ();
        let mut now = 0u64;
        let mut frame = |root: &mut RenderRoot<(), ScrollView<()>>| {
            root.rebuild(&mut logic, &mut state);
            root.layout(VIEWPORT);
            now += 16;
            root.paint(
                &mut RecordingScene::default(),
                FrameTime::from_nanos(now * 1_000_000),
            )
        };
        frame(&mut root);
        controller.jump_to(250.0);
        let outcome = frame(&mut root);
        assert_eq!(
            controller.offset(),
            250.0,
            "applied in the frame after the jump"
        );
        assert!(
            outcome.needs_frame,
            "the owed notification asks for a frame"
        );
        assert!(
            log.borrow().is_empty(),
            "paint has no state to notify through"
        );
        frame(&mut root);
        assert_eq!(
            log.borrow().as_slice(),
            &[ScrollInfo {
                offset: 250.0,
                max_offset: MAX,
                overscroll: 0.0,
            }]
        );
    }

    #[test]
    fn dropping_the_widget_detaches_the_handle() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.jump_to(120.0);
        lay(&mut w);
        assert!(controller.is_attached());
        drop(w);
        assert!(!controller.is_attached());
        assert_eq!(controller.offset(), 120.0, "the last published value stays");

        // A rebuild that names no handle detaches too.
        let mut w = build(&view);
        assert!(controller.is_attached());
        let bare: ScrollView<()> = scroll_view(leaf(200.0, CONTENT_H));
        assert!(rebuild(&bare, &view, &mut w).needs_paint());
        assert!(!controller.is_attached());
    }

    #[test]
    fn a_second_scroll_view_takes_the_handle_over() {
        let controller = ScrollController::new();
        let first_view = view(&controller);
        let mut first = build(&first_view);
        lay(&mut first);
        let second_view = view(&controller);
        let mut second = build(&second_view);
        lay(&mut second);

        controller.jump_to(300.0);
        lay(&mut first);
        lay(&mut second);
        assert_eq!(first.offset(), 0.0, "the displaced surface drains nothing");
        assert_eq!(second.offset(), 300.0);

        // The displaced surface neither publishes nor takes the handle back
        // on a rebuild naming the same handle.
        send(&mut first, &wheel(30.0), 0.0);
        assert_eq!(first.offset(), 30.0);
        assert_eq!(controller.offset(), 300.0);
        let first_again = view(&controller);
        rebuild(&first_again, &first_view, &mut first);
        controller.jump_to(100.0);
        lay(&mut first);
        lay(&mut second);
        assert_eq!((first.offset(), second.offset()), (30.0, 100.0));

        // Dropping the displaced surface leaves the holder attached; once the
        // holder goes too, the survivor's next rebuild binds the handle again.
        let mut survivor = first;
        drop(second);
        assert!(!controller.is_attached());
        let survivor_view = view(&controller);
        assert!(rebuild(&survivor_view, &first_again, &mut survivor).needs_paint());
        assert!(controller.is_attached());
        lay(&mut survivor);
        assert_eq!(controller.offset(), 30.0, "the new holder publishes itself");
        drop(survivor);
        assert!(!controller.is_attached());
    }

    #[test]
    fn an_animate_to_reaches_the_target_within_its_duration() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        assert!(!controller.is_animating(), "nothing is animating yet");

        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );
        // The seeding frame: the command drains and starts the tween, but
        // the zero-delta first pump moves nothing — the same convention a
        // release fling's first paint takes.
        assert!(paint_at(&mut w, 0.0), "a live tween asks for continuation");
        assert!(controller.is_animating());
        assert_eq!(w.offset(), 0.0, "the seeding frame moves nothing");

        // Halfway through the duration, a linear curve is halfway to the
        // target.
        assert!(paint_at(&mut w, 100.0));
        assert!((w.offset() - 200.0).abs() < 1e-6, "offset: {}", w.offset());
        assert!(controller.is_animating());

        // At its duration the tween lands exactly on the target and ends.
        assert!(!paint_at(&mut w, 200.0), "nothing is left animating");
        assert_eq!(w.offset(), 400.0);
        assert_eq!(controller.offset(), 400.0, "the completion publishes it");
        assert!(
            !controller.is_animating(),
            "completion resolves is_animating"
        );

        // A later frame leaves the arrived-at position alone.
        assert!(!paint_at(&mut w, 216.0));
        assert_eq!(w.offset(), 400.0);
    }

    #[test]
    fn a_down_mid_animation_stops_it_at_the_current_offset() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 100.0);
        assert!((w.offset() - 200.0).abs() < 1e-6);
        assert!(controller.is_animating());

        // A Down lands mid-animation: it ends exactly where the tween had
        // eased to, and the ordinary gesture machinery still arms normally —
        // user input always wins.
        send(&mut w, &pointer(PointerPhase::Down, 100.0), 116.0);
        assert!(w.ballistic.is_none(), "the Down ends the tween");
        assert!(!controller.is_animating());
        assert!(
            (w.offset() - 200.0).abs() < 1e-6,
            "frozen at the eased-to offset, offset: {}",
            w.offset()
        );
        assert!(w.down_active, "the Down still arms an ordinary gesture");

        // A later frame moves nothing on its own — the tween is really gone,
        // not just paused.
        assert!(!paint_at(&mut w, 300.0));
        assert!((w.offset() - 200.0).abs() < 1e-6);
    }

    #[test]
    fn a_wheel_mid_animation_stops_it() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 100.0);
        assert!(controller.is_animating());

        send(&mut w, &wheel(10.0), 116.0);
        assert!(w.ballistic.is_none());
        assert!(!controller.is_animating());
        assert!(
            (w.offset() - 210.0).abs() < 1e-6,
            "the wheel delta still applies"
        );
    }

    #[test]
    fn a_later_animate_to_replaces_a_running_one() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 100.0);
        let mid = w.offset();
        assert!((mid - 200.0).abs() < 1e-6);
        assert!(controller.is_animating());

        // A second `animate_to`, recorded before the first finishes, takes
        // over outright: the new tween starts from wherever the first had
        // eased to, not from the first's target.
        controller.animate_to(
            600.0,
            AnimateTo {
                duration_ms: 100.0,
                curve: Curve::Linear,
            },
        );
        assert!(paint_at(&mut w, 116.0), "the replacement keeps animating");
        assert!(controller.is_animating());
        assert_eq!(
            w.offset(),
            mid,
            "the seeding frame of the new tween moves nothing"
        );

        assert!(
            !paint_at(&mut w, 216.0),
            "the replacement's own duration ends it"
        );
        assert_eq!(w.offset(), 600.0);
        assert!(!controller.is_animating());
    }

    #[test]
    fn a_jump_recorded_after_an_animate_to_replaces_it() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 100.0);
        assert!(controller.is_animating());

        controller.jump_to(500.0);
        assert!(
            !paint_at(&mut w, 116.0),
            "the jump leaves nothing animating"
        );
        assert_eq!(w.offset(), 500.0);
        assert!(!controller.is_animating());
    }

    #[test]
    fn reduce_motion_collapses_animate_to_into_an_immediate_jump() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.animate_to(
            400.0,
            AnimateTo {
                duration_ms: 200.0,
                curve: Curve::Linear,
            },
        );

        let mut theme = frust_theme::Theme::neutral();
        theme.motion.reduce_motion = true;
        assert!(!paint_at_with_reduce_motion(&mut w, 0.0, &theme));
        assert_eq!(w.offset(), 400.0, "landed at once, no tween in flight");
        assert_eq!(controller.offset(), 400.0);
        assert!(!controller.is_animating(), "no animation ever started");
        assert!(w.ballistic.is_none());
    }

    #[test]
    fn an_animate_to_clamps_to_the_scroll_range() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);

        let mut animate_and_run = |target: f64| -> f64 {
            controller.animate_to(
                target,
                AnimateTo {
                    duration_ms: 50.0,
                    curve: Curve::Linear,
                },
            );
            paint_at(&mut w, 0.0);
            paint_at(&mut w, 116.0);
            w.offset()
        };
        assert_eq!(animate_and_run(5000.0), MAX);
        assert_eq!(animate_and_run(-50.0), 0.0);
        assert_eq!(animate_and_run(f64::INFINITY), MAX);
        assert_eq!(animate_and_run(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn an_animate_to_with_nan_offset_is_ignored() {
        let controller = ScrollController::new();
        let view = view(&controller);
        let mut w = build(&view);
        lay(&mut w);
        controller.jump_to(123.0);
        paint_at(&mut w, 0.0);

        controller.animate_to(
            f64::NAN,
            AnimateTo {
                duration_ms: 100.0,
                curve: Curve::Linear,
            },
        );
        assert!(!paint_at(&mut w, 116.0));
        assert_eq!(w.offset(), 123.0, "NaN is ignored, nothing moves");
        assert!(!controller.is_animating());
    }

    // --- A command applied mid-drag ends the drag. ---

    /// 1000 px of content logging every pointer phase that reaches it, so a
    /// test can see where a `Move`/`Up` was routed once the surface stopped
    /// scrolling. Logged through an `Rc` rather than `EventCtx::state_mut`
    /// because a `Cancel` arm never touches state.
    struct PhaseLog(Rc<RefCell<Vec<PointerPhase>>>);
    /// Retained widget for [`PhaseLog`].
    struct PhaseLogW(Rc<RefCell<Vec<PointerPhase>>>);

    impl View<()> for PhaseLog {
        type Element = PhaseLogW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> PhaseLogW {
            PhaseLogW(Rc::clone(&self.0))
        }
        fn rebuild(&self, _p: &Self, _e: &mut PhaseLogW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for PhaseLogW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, CONTENT_H))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e {
                self.0.borrow_mut().push(p.phase);
            }
            EventResult::Ignored
        }
    }

    /// A laid-out surface over a [`PhaseLog`] child running `physics`, with
    /// `controller` attached, plus the child's phase log.
    fn logged(
        controller: &ScrollController,
        physics: Rc<dyn ScrollPhysics>,
    ) -> (ScrollWidget, Rc<RefCell<Vec<PointerPhase>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let view = scroll_view(PhaseLog(Rc::clone(&log))).controller(controller.clone());
        let mut w = build(&view);
        w.physics = physics;
        lay(&mut w);
        (w, log)
    }

    /// Down, then two moves upward — the first crosses the slop and takes
    /// over, the second scrolls 25 px — leaving the drag live.
    fn drag_without_release(w: &mut ScrollWidget) {
        send(w, &pointer(PointerPhase::Down, 100.0), 0.0);
        send(w, &pointer(PointerPhase::Move, 75.0), 16.0);
        send(w, &pointer(PointerPhase::Move, 50.0), 32.0);
        assert!(w.scrolling && w.down_active, "the drag is live");
    }

    /// How many of the three post-release motions are live at once — the
    /// invariant is that this never exceeds one.
    fn live_motions(w: &ScrollWidget) -> usize {
        usize::from(w.fling.is_some())
            + usize::from(w.settling)
            + usize::from(w.ballistic.is_some())
    }

    fn both_physics() -> [Rc<dyn ScrollPhysics>; 2] {
        [Rc::new(RubberBand::new()), Rc::new(Bouncing::new())]
    }

    #[test]
    fn an_animate_to_mid_drag_ends_the_drag_and_owns_the_offset() {
        for physics in both_physics() {
            let controller = ScrollController::new();
            let (mut w, log) = logged(&controller, physics);
            drag_without_release(&mut w);
            let start = w.offset();
            assert!(start > 0.0, "the drag moved the content");
            let stale_veto = Rc::clone(&w.live_veto);

            controller.animate_to(
                start + 200.0,
                AnimateTo {
                    duration_ms: 200.0,
                    curve: Curve::Linear,
                },
            );
            assert!(paint_at(&mut w, 0.0), "the tween asks for frames");
            assert!(!w.scrolling, "the command ended the drag");
            assert!(!w.down_active, "and disarmed the gesture");
            assert!(!w.deferring);
            assert!(
                !Rc::ptr_eq(&w.live_veto, &stale_veto),
                "the gesture's veto cell is replaced, like on a Down"
            );
            assert!(controller.is_animating());

            assert!(paint_at(&mut w, 100.0));
            let mid = w.offset();
            assert!(
                (mid - (start + 100.0)).abs() < 1e-6,
                "the offset tracks the tween, offset: {mid}"
            );

            // The finger keeps going: the move takes the unarmed path to the
            // child and never touches the offset the tween owns.
            log.borrow_mut().clear();
            send(&mut w, &pointer(PointerPhase::Move, 20.0), 116.0);
            assert_eq!(w.offset(), mid, "a later Move does not move the content");
            assert_eq!(log.borrow().as_slice(), &[PointerPhase::Move]);
            assert!(!w.scrolling, "and never re-arms the drag");

            // The release takes the non-scrolling branch: forwarded to the
            // child, starting no fling or settle of its own.
            send(&mut w, &pointer(PointerPhase::Up, 20.0), 120.0);
            assert_eq!(
                log.borrow().as_slice(),
                &[PointerPhase::Move, PointerPhase::Up]
            );
            assert!(w.fling.is_none(), "no stray fling");
            assert!(!w.settling, "no stray settle");
            assert_eq!(live_motions(&w), 1, "only the tween is live");
            assert!(controller.is_animating(), "the tween runs on");

            assert!(!paint_at(&mut w, 200.0), "nothing is left animating");
            assert_eq!(w.offset(), start + 200.0);
            assert!(!controller.is_animating(), "completion resolves it");
            assert!(!w.is_flinging() && !w.settling);
            assert!(!paint_at(&mut w, 216.0));
            assert_eq!(w.offset(), start + 200.0);
        }
    }

    #[test]
    fn a_jump_mid_drag_ends_the_drag_and_its_release_starts_nothing() {
        for physics in both_physics() {
            let controller = ScrollController::new();
            let (mut w, log) = logged(&controller, physics);
            drag_without_release(&mut w);
            controller.jump_to(500.0);
            lay(&mut w);
            assert_eq!(w.offset(), 500.0);
            assert!(!w.scrolling && !w.down_active, "the jump ended the drag");

            log.borrow_mut().clear();
            send(&mut w, &pointer(PointerPhase::Move, 10.0), 48.0);
            assert_eq!(w.offset(), 500.0, "the finger no longer drags");
            // A fast upward flick that would have flung a live drag.
            send(&mut w, &pointer(PointerPhase::Up, 10.0), 50.0);
            assert_eq!(
                log.borrow().as_slice(),
                &[PointerPhase::Move, PointerPhase::Up],
                "both reached the child through the unarmed path"
            );
            assert_eq!(live_motions(&w), 0, "the release starts nothing");
            assert!(!controller.is_animating());
            assert!(!paint_at(&mut w, 64.0), "nothing animates");
            assert_eq!(w.offset(), 500.0);
            assert_eq!(controller.offset(), 500.0);
        }
    }

    #[test]
    fn a_release_tears_down_a_tween_before_starting_its_own_motion() {
        // Defence in depth behind the rule above: even with a tween somehow
        // live under a still-armed drag, the release replaces it through the
        // single tear-down path rather than writing over it — under the
        // legacy fling/settle path (`RubberBand`) and the simulation path.
        for physics in both_physics() {
            let controller = ScrollController::new();
            let (mut w, _log) = logged(&controller, physics);
            drag_without_release(&mut w);
            controller.animate_to(
                800.0,
                AnimateTo {
                    duration_ms: 400.0,
                    curve: Curve::Linear,
                },
            );
            paint_at(&mut w, 0.0);
            assert!(controller.is_animating());
            // Force the drag back on, as if the command had not ended it.
            w.scrolling = true;
            w.down_active = true;

            send(&mut w, &pointer(PointerPhase::Up, 50.0), 40.0);
            assert!(
                live_motions(&w) <= 1,
                "fling, settle and simulation are never live together"
            );
            assert!(
                !controller.is_animating(),
                "the replaced tween no longer reports itself"
            );
            assert!(!w.controller_animating);
            // Whatever the release started runs to rest on its own, never
            // resurrecting the tween's flag.
            let mut ms = 40.0;
            while paint_at(&mut w, ms) {
                assert!(live_motions(&w) <= 1);
                assert!(!controller.is_animating());
                ms += 16.0;
                assert!(ms < 10_000.0, "the release motion never settled");
            }
            assert_eq!(live_motions(&w), 0);
        }
    }

    #[test]
    fn a_nan_command_mid_drag_leaves_the_drag_alone() {
        let controller = ScrollController::new();
        let (mut w, _log) = logged(&controller, Rc::new(Bouncing::new()));
        drag_without_release(&mut w);
        let before = w.offset();
        controller.jump_to(f64::NAN);
        lay(&mut w);
        assert!(w.scrolling, "an ignored command does not end the drag");
        send(&mut w, &pointer(PointerPhase::Move, 40.0), 48.0);
        assert!(w.offset() > before, "the finger still drags");
    }
}
