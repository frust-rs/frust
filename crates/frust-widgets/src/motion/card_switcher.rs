//! [`card_switcher`]: an app-switcher stage. The live page shrinks into a
//! rounded card over a scrim, with a label row, page dots and an action bar
//! around it, and peer cards beside it on a horizontal strip.
//!
//! # Composition
//!
//! One wrapper owns the whole stage. The live child keeps the full constraints
//! and is only *transformed*: [`ChildPod::set_transform`] scales it about its
//! centre, so paint, hit tests and event localisation follow the shrunken card
//! while the page keeps its layout and state. With open progress `p`
//! (`0` closed, `1` open) the scale is `1 → card_scale`, the on-screen corner
//! radius `0 → card_radius`, and the stage paints, in parent space: the scrim
//! over the whole stage, then for each card on the strip its drop shadow, a
//! rounded clip at its on-screen rect and the transformed card, then the
//! slots: a label row `label_gap` above each card, the dots `dots_gap` below
//! the strip, and the action bar flush with the bottom edge (it consumes its
//! own window inset: wrap it in `safe_area`). Labels and dots fade in over the
//! last 40 % of `p`, the bar over the first 40 %. Clip and shadow stay in
//! parent space because a clip radius is recorded in pre-transform space.
//! Closed, the child paints plainly: no transform, clip, shadow or scrim.
//!
//! # The strip
//!
//! The live card and the [`peers`](CardSwitcherView::peers) form one strip of
//! `N = peers + 1` cards, the live card at
//! [`live_index`](CardSwitcherView::live_index). Every card (the live child and
//! each peer face) is laid out at the full stage size and shown under the same
//! transform, `translate(c_k)·scale(s)·translate(−c)`, so neighbours peek in at
//! both edges. Open, the stride is `stage_w × card_scale + gap`, card `k`'s
//! centre sits at `stage_cx + k × stride − offset`, and the strip scrolls over
//! `[0, (N − 1) × stride]`. Over the open ramp the whole strip scales with the
//! live card (the stride is `stage_w × s + gap` at scale `s`), so the peers
//! slide in from beyond the edges and never overlap a card; they fade in on the
//! labels' window. Closing, the strip eases back to the live card, so the page
//! always grows back into place.
//!
//! The card nearest the centre is drawn at full opacity and the others fade
//! toward `peer_alpha` with their distance from the centre, in strides, so a
//! drag cross-fades continuously.
//!
//! The strip scrolls on the crate's scroll surface core (`scroll_core.rs`),
//! with [`PageSnap::extent`] at the stride over [`default_physics`](crate::physics::default_physics):
//! drags take over past `TOUCH_SLOP`, a release snaps to the nearest card
//! biased by the fling, and a drag past the first or last card overscrolls and
//! springs back under the platform physics. It reads the horizontal axis only;
//! a vertical drag on the strip moves nothing. Wheel input does not scroll it.
//! The strip registers nothing with an enclosing scroll surface's nested-claim
//! cell: the stage wraps the whole page, so there is nothing to nest in, and a
//! stage placed inside a scroll view does not make that view defer to it.
//!
//! # Input
//!
//! While the stage is open or animating no pointer event reaches the live
//! child, a peer face or a peer label. Settled open, a press on the card row
//! (the band the cards span, gaps included) goes to the strip:
//!
//! * a drag scrolls it and never selects anything; selection is tap-only;
//! * a tap on the centred live card calls `on_close`;
//! * a tap on any other card centres it on a tween, then calls `on_select`
//!   with its **peer** index (or `on_close`, for the live card) once the strip
//!   rests there; a card already centred answers at once;
//! * a tap in the row but on no card (a gap, the margins) calls `on_close`.
//!
//! A tap is a press and release the strip did not take over. A press that lands
//! while the strip is still moving (a release spring, an edge settle) catches
//! it; if it then releases without dragging, it is not a tap, and the strip
//! springs to the nearest card instead. **A tap during a tap's centring tween
//! is ignored**: from the tap until its callback fires, every pointer event in
//! the stage is swallowed, so neither a second tap nor a drag can redirect the
//! selection in flight.
//!
//! Outside the card row, a press calls `on_close` at once (the scrim), unless a
//! slot takes it; every other pointer event in the stage's bounds is
//! swallowed. During the open and close ramps the row is not live: a press and
//! release on the live card calls `on_close`, and a press elsewhere calls it at
//! once. Blocking covers only the stage's own bounds, so wrap the whole shell
//! page, not a body under app chrome. Key and IME routing is untouched. Opening
//! cancels the child's capture and clears its focus link; closing restores the
//! link when focus still runs through the stage.
//!
//! A callback that comes due during a paint (a centring tween landing) is
//! delivered by the next frame's `Housekeeping` event, the scroll core's own
//! route for a paint-time notification.
//!
//! # Expanding a card
//!
//! [`expanding(Some(i))`](CardSwitcherView::expanding) grows peer `i` into the
//! page. The strip first centres card `i` (on the centring tween, unless it
//! already rests there), then runs the `close_timing` ramp on that card alone:
//! its scale goes `card_scale → 1` about the stage centre, its radius
//! `card_radius → 0` and its shadow fades, while every other card, the labels,
//! the dots and the bar fade out as the ramp passes `0.8`
//! (`alpha = clamp((0.8 − p) / 0.4)`). Pointer input is swallowed from the
//! request until the stage closes or the request is withdrawn. When the ramp
//! completes the stage holds that pose and does nothing else: swapping the
//! live content is the app's move. The handshake:
//!
//! 1. the user taps peer `i`; the strip centres it and calls `on_select(i)`;
//! 2. the app records the choice and rebuilds with `.expanding(Some(i))`;
//! 3. the stage grows card `i` to full screen and holds;
//! 4. the app rebuilds with `open = false`, the chosen page as the live child,
//!    `.live_index` at that card's strip position (keep the strip order) and
//!    `.expanding(None)`.
//!
//! Step 4 may come at any time after step 2. The close continues from the
//! expansion's progress: after a completed expansion the stage is closed at
//! once and the new live child paints plainly where the face was; mid-ramp the
//! close finishes the grow from the current scale. Withdrawing the request
//! while still open (`.expanding(None)`) reverses the ramp.
//!
//! # Reduced motion
//!
//! Under `theme.motion.reduce_motion` the scale and radius jump to the target
//! pose on the first paint, and only the alphas (scrim, shadow, slots, peers)
//! run, as a 120 ms linear fade, whatever `open_timing`/`close_timing` say. The
//! peers appear at their final positions. The centring tween collapses to a
//! jump, a drag release still snaps, and an expansion jumps its geometry and
//! fades its alphas.
//!
//! # Flutter
//!
//! No direct analog; `OpenContainer` (the `animations` package) and `Hero` are
//! the nearest.

use std::rc::Rc;
use std::time::Duration;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerEvent, PointerPhase, ScrollAxis,
    SemanticsCtx, Tween, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size, Vec2};
use peniko::Color;

use crate::Timing;
use crate::authoring::{
    ErasedArgCallback, ErasedCallback, TypedArgCallback, VisitPods, build_child, erase_callback,
    erase_callback_arg, presses, rebuild_child, route_event_single, teardown_child,
};
use crate::nav::transition::{REDUCE_MOTION_DURATION, TransitionDriver, make_driver};
use crate::physics::page_snap::PageSnap;
use crate::physics::{ScrollPhysics, default_overscroll_effect};
use crate::scroll::{ScrollInfo, ambient_scroll_claim};
use crate::scroll_controller::AnimateTo;
use crate::scroll_core::{BallisticState, ScrollSurface, ScrollSurfaceCore};

// Defaults from the reference switcher design. No `Theme` token carries a
// card-stage scale, radius, gap or slot offset, so these stay local constants.
const CARD_SCALE: f64 = 0.62;
const CARD_RADIUS: f64 = 24.0;
const GAP: f64 = 16.0;
const PEER_ALPHA: f64 = 0.78;
const LABEL_GAP: f64 = 30.0;
const DOTS_GAP: f64 = 22.0;
/// Opacity of the default scrim and card shadow at full open.
const BACKDROP_ALPHA: f32 = 0.55;
/// Unthemed scrim, `#05070B` at 55 %; a theme resolves `colors.scrim` instead.
const FALLBACK_SCRIM: Color = Color::new([5.0 / 255.0, 7.0 / 255.0, 11.0 / 255.0, 0.55]);
const SHADOW_Y: f64 = 24.0;
/// The Gaussian standard deviation `draw_shadow` takes: a CSS blur radius of
/// 48 px is `48 / 2` (`Container::glow`'s convention).
const SHADOW_STD_DEV: f64 = 24.0;
/// Open: 360 ms emphasized-decelerate. Close: 320 ms emphasized-accelerate.
/// `MotionScheme` has neither duration, so these are the pattern's own.
const OPEN_TIMING: Timing = Timing::Duration(
    Duration::from_millis(360),
    Curve::Cubic(0.05, 0.7, 0.1, 1.0),
);
const CLOSE_TIMING: Timing = Timing::Duration(
    Duration::from_millis(320),
    Curve::Cubic(0.3, 0.0, 0.8, 0.15),
);
/// The slot fade windows: labels and dots over the last 40 % of `p`, the bar
/// over the first 40 %.
const LABEL_FADE_START: f64 = 0.6;
const FADE_SPAN: f64 = 0.4;
/// An expansion fades everything but its card out by this progress, over
/// [`FADE_SPAN`] before it.
const EXPAND_FADE_END: f64 = 0.8;
/// How close the ramp must sit to its target to count as landed while the
/// driver still formally runs (`motion::animated`'s tolerance).
const SETTLED_EPS: f64 = 1e-9;
/// The tween that centres a tapped or expanding card: 300 ms emphasized, the
/// M3 medium spatial motion. The strip has no theme in the event pass that
/// starts it, and `MotionScheme` names no scroll-to duration, so this is the
/// pattern's own; `reduce_motion` collapses it to a jump.
const CENTRE_TWEEN: AnimateTo = AnimateTo {
    duration_ms: 300.0,
    curve: Curve::Emphasized,
};
/// How far (px) the strip may sit from a card's slot and still count as
/// resting on it.
const ON_SLOT_PX: f64 = 0.5;
/// Strides from the centre below which a card counts as centred for the dim:
/// absorbs the last-ulp residue of a landed tween.
const CENTRED_EPS: f64 = 1e-6;

// --- Public API ---------------------------------------------------------------

/// The builder options, compared as a whole on rebuild.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StageOptions {
    card_scale: f64,
    card_radius: f64,
    gap: f64,
    peer_alpha: f64,
    scrim: Option<Color>,
    shadow: Option<(Color, f64, f64)>,
    label_gap: f64,
    dots_gap: f64,
    open_timing: Timing,
    close_timing: Timing,
}

type CloseCallback<State> = Rc<dyn Fn(&mut State)>;

/// A peer card for [`CardSwitcherView::peers`]: a face and an optional label.
pub struct PeerCard<State: 'static> {
    face: AnyView<State>,
    label: Option<AnyView<State>>,
}

/// A peer card showing `face`. See [`CardSwitcherView::peers`].
pub fn peer_card<State: 'static>(face: impl View<State>) -> PeerCard<State> {
    PeerCard {
        face: any(face),
        label: None,
    }
}

impl<State: 'static> PeerCard<State> {
    /// The label row drawn above this card.
    pub fn label(mut self, label: impl View<State>) -> Self {
        self.label = Some(any(label));
        self
    }
}

/// A card-switcher stage around a live page. See the [module docs](self).
pub struct CardSwitcherView<State: 'static> {
    open: bool,
    live: AnyView<State>,
    peers: Vec<PeerCard<State>>,
    live_index: usize,
    live_label: Option<AnyView<State>>,
    dots: Option<AnyView<State>>,
    action_bar: Option<AnyView<State>>,
    expanding: Option<usize>,
    on_select: Option<TypedArgCallback<State, usize>>,
    on_close: Option<CloseCallback<State>>,
    options: StageOptions,
}

/// Show `live` as a full-size page, or (`open`) shrunk into a card on the
/// switcher stage. A changed `open` animates on rebuild, like
/// [`AnimatedScale`](super::AnimatedScale); mounting already open does not.
pub fn card_switcher<State: 'static>(
    open: bool,
    live: impl View<State>,
) -> CardSwitcherView<State> {
    CardSwitcherView {
        open,
        live: any(live),
        peers: Vec::new(),
        live_index: 0,
        live_label: None,
        dots: None,
        action_bar: None,
        expanding: None,
        on_select: None,
        on_close: None,
        options: StageOptions {
            card_scale: CARD_SCALE,
            card_radius: CARD_RADIUS,
            gap: GAP,
            peer_alpha: PEER_ALPHA,
            scrim: None,
            shadow: None,
            label_gap: LABEL_GAP,
            dots_gap: DOTS_GAP,
            open_timing: OPEN_TIMING,
            close_timing: CLOSE_TIMING,
        },
    }
}

/// PascalCase alias for [`card_switcher`].
#[allow(non_snake_case)]
pub fn CardSwitcher<State: 'static>(open: bool, live: impl View<State>) -> CardSwitcherView<State> {
    card_switcher(open, live)
}

impl<State: 'static> CardSwitcherView<State> {
    /// The other cards on the strip, in strip order without the live card.
    pub fn peers(mut self, peers: impl IntoIterator<Item = PeerCard<State>>) -> Self {
        self.peers = peers.into_iter().collect();
        self
    }

    /// The live card's position on the strip (default `0`).
    pub fn live_index(mut self, index: usize) -> Self {
        self.live_index = index;
        self
    }

    /// The label row drawn above the live card.
    pub fn live_label(mut self, label: impl View<State>) -> Self {
        self.live_label = Some(any(label));
        self
    }

    /// Called with the **peer** index of a tapped peer card, once the strip
    /// has centred it (the [module docs](self)' *Input*). A drag never
    /// selects.
    pub fn on_select(mut self, on_select: impl Fn(&mut State, usize) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Called when the open stage asks to close (a tap on the centred live
    /// card, the card row's gaps or the scrim).
    /// The app sets `open` to `false`; the stage never closes itself.
    pub fn on_close(mut self, on_close: impl Fn(&mut State) + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// The action bar, flush with the stage's bottom edge.
    pub fn action_bar(mut self, bar: impl View<State>) -> Self {
        self.action_bar = Some(any(bar));
        self
    }

    /// A page indicator centred `dots_gap` below the card; the stage only
    /// positions and fades it.
    pub fn dots(mut self, dots: impl View<State>) -> Self {
        self.dots = Some(any(dots));
        self
    }

    /// The peer card expanding into the live page, if any: the strip centres
    /// it and grows it to full screen, then holds for the app's swap (the
    /// [module docs](self)' *Expanding a card*).
    pub fn expanding(mut self, peer: Option<usize>) -> Self {
        self.expanding = peer;
        self
    }

    /// The card's scale at full open (default `0.62`).
    pub fn card_scale(mut self, scale: f64) -> Self {
        self.options.card_scale = scale;
        self
    }

    /// The card's on-screen corner radius at full open (default `24`).
    pub fn card_radius(mut self, radius: f64) -> Self {
        self.options.card_radius = radius;
        self
    }

    /// The on-screen gap between neighbouring cards (default `16`).
    pub fn gap(mut self, gap: f64) -> Self {
        self.options.gap = gap;
        self
    }

    /// The opacity of a card away from the centre (default `0.78`).
    pub fn peer_alpha(mut self, alpha: f64) -> Self {
        self.options.peer_alpha = alpha;
        self
    }

    /// The scrim at full open. Defaults to the theme's `colors.scrim` at 55 %,
    /// or `#05070B` at 55 % unthemed.
    pub fn scrim(mut self, color: Color) -> Self {
        self.options.scrim = Some(color);
        self
    }

    /// The card's drop shadow at full open: `color`, a downward `y_offset` and
    /// the Gaussian `blur_std_dev` (half a CSS blur radius). Defaults to the
    /// theme's `colors.shadow` (black unthemed) at 55 %, offset 24, std-dev 24.
    pub fn shadow(mut self, color: Color, y_offset: f64, blur_std_dev: f64) -> Self {
        self.options.shadow = Some((color, y_offset, blur_std_dev));
        self
    }

    /// The clear space between a label row and its card's top (default `30`).
    pub fn label_gap(mut self, gap: f64) -> Self {
        self.options.label_gap = gap;
        self
    }

    /// The clear space between the card's bottom and the dots (default `22`).
    pub fn dots_gap(mut self, gap: f64) -> Self {
        self.options.dots_gap = gap;
        self
    }

    /// The open ramp (default 360 ms, `Cubic(0.05, 0.7, 0.1, 1.0)`). Ignored
    /// under `reduce_motion`.
    pub fn open_timing(mut self, timing: Timing) -> Self {
        self.options.open_timing = timing;
        self
    }

    /// The close ramp (default 320 ms, `Cubic(0.3, 0, 0.8, 0.15)`). Ignored
    /// under `reduce_motion`.
    pub fn close_timing(mut self, timing: Timing) -> Self {
        self.options.close_timing = timing;
        self
    }
}

// --- Open ramp -----------------------------------------------------------------

/// The `0..1` open progress, on `make_driver` + [`Tween`] with
/// `motion::animated`'s retarget contract: a target change re-drives from the
/// current value (value-continuous), and a `reduce_motion` flip re-drives only
/// an in-flight value.
struct StageRamp {
    target: f64,
    driving_target: f64,
    tween: Tween<f64>,
    driver: TransitionDriver,
    driving_reduced: bool,
}

impl StageRamp {
    /// Settled at the mount state: mounting open does not animate in.
    fn new(open: bool) -> Self {
        let p = if open { 1.0 } else { 0.0 };
        let (driver, _) = make_driver(Timing::Duration(Duration::ZERO, Curve::Linear));
        Self {
            target: p,
            driving_target: p,
            tween: Tween::new(p, p),
            driver,
            driving_reduced: false,
        }
    }

    fn set_open(&mut self, open: bool) {
        self.target = if open { 1.0 } else { 0.0 };
    }

    fn is_open(&self) -> bool {
        self.target > 0.5
    }

    /// The progress the alphas follow.
    fn value(&self) -> f64 {
        self.tween.lerp(self.driver.value())
    }

    /// The progress the scale and radius follow: the target itself under the
    /// reduced-motion collapse, which never ramps geometry.
    fn geometry(&self) -> f64 {
        if self.driving_reduced {
            self.target
        } else {
            self.value()
        }
    }

    fn settled(&self) -> bool {
        if self.driving_target != self.target {
            return false;
        }
        let idle = match &self.driver {
            TransitionDriver::Auto(controller) => !controller.is_animating(),
            _ => false,
        };
        idle || (self.value() - self.target).abs() <= SETTLED_EPS
    }

    /// Advance to `now`, launching a fresh driver under `timing` on a pending
    /// retarget or an in-flight collapse flip. Returns whether still running.
    fn advance(&mut self, now: FrameTime, timing: Timing, reduced: bool) -> bool {
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
        self.tween = Tween::new(self.value(), self.target);
        self.driving_target = self.target;
        self.driver = make_driver(timing).0;
        self.driver.advance(now).animating
    }

    /// Close from `value` rather than from the ramp's own progress: the close
    /// that follows an expansion picks up where the grow stands (`0` lands it
    /// closed at once). The next [`StageRamp::advance`] re-drives from
    /// `value`.
    fn close_from(&mut self, value: f64) {
        self.tween = Tween::new(value, value);
        self.driving_target = value;
        self.target = 0.0;
    }
}

// --- Geometry -------------------------------------------------------------------

/// Where the strip's cards sit at one instant, in the stage's local space.
#[derive(Clone, Copy, Debug)]
struct StripGeometry {
    stage: Size,
    /// The cards' common scale.
    scale: f64,
    /// The stride at full open, in strip px (`stage_w × card_scale + gap`).
    stride: f64,
    /// On-screen px per strip px at this instant: `1` at full open, more while
    /// the cards are larger.
    ratio: f64,
    /// The strip position the cards are drawn at, in strip px.
    offset: f64,
}

impl StripGeometry {
    /// Card `k`'s signed distance from the centre, in strip px.
    fn delta(&self, k: usize) -> f64 {
        k as f64 * self.stride - self.offset
    }

    fn centre(&self, k: usize) -> Point {
        Point::new(
            self.stage.width / 2.0 + self.delta(k) * self.ratio,
            self.stage.height / 2.0,
        )
    }

    /// Card `k`'s on-screen rect.
    fn rect(&self, k: usize) -> Rect {
        Rect::from_center_size(self.centre(k), self.stage * self.scale)
    }

    /// The transform that takes a full-size card to [`StripGeometry::rect`]:
    /// scale about the card's own centre, then move that centre to card `k`'s.
    fn transform(&self, k: usize) -> Affine {
        let own = Point::new(self.stage.width / 2.0, self.stage.height / 2.0).to_vec2();
        Affine::translate(self.centre(k).to_vec2())
            * Affine::scale(self.scale)
            * Affine::translate(-own)
    }

    /// Card `k`'s distance from the centre in strides (`0` centred).
    fn distance(&self, k: usize) -> f64 {
        if self.stride > 0.0 {
            self.delta(k).abs() / self.stride
        } else {
            0.0
        }
    }
}

/// The whole stage's pose at one instant: the strip geometry plus every alpha
/// the paint and the slot routing read.
struct Frame {
    geometry: StripGeometry,
    /// The live card's strip position.
    live: usize,
    radius: f64,
    /// Open progress for the scrim and shadows, clamped to `0..=1`.
    alpha: f64,
    /// The label/dots window of the open ramp (peers fade on it too).
    label: f64,
    /// The bar's window of the open ramp.
    bar: f64,
    /// An expansion's fade for everything but its card (`1` without one).
    others: f64,
    /// The expanding card's strip position.
    expanding: Option<usize>,
    /// The expanding card's shadow, `1 → 0` over its ramp.
    shadow_fade: f64,
    peer_alpha: f64,
}

impl Frame {
    /// Card `k`'s opacity: the centre dim, a peer's fade-in, and an
    /// expansion's fade-out.
    fn card_alpha(&self, k: usize) -> f64 {
        if self.expanding == Some(k) {
            return 1.0;
        }
        let distance = self.geometry.distance(k);
        let dim = if distance <= CENTRED_EPS {
            1.0
        } else {
            1.0 - (1.0 - self.peer_alpha) * distance.min(1.0)
        };
        let fade = if k == self.live { 1.0 } else { self.label };
        dim * fade * self.others
    }

    fn shadow_alpha(&self, k: usize) -> f64 {
        let fade = if self.expanding == Some(k) {
            self.shadow_fade
        } else {
            1.0
        };
        self.alpha * self.card_alpha(k) * fade
    }

    fn label_alpha(&self) -> f64 {
        self.label * self.others
    }

    fn bar_alpha(&self) -> f64 {
        self.bar * self.others
    }
}

// --- Peers strip ---------------------------------------------------------------

/// One peer card's retained pods.
struct PeerPods {
    face: ChildPod,
    label: Option<ChildPod>,
}

impl VisitPods for PeerPods {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.face.visit_pods(visitor);
        self.label.visit_pods(visitor);
    }
}

/// What a press on the card row landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    /// The card at this strip position.
    Card(usize),
    /// The row, but no card: a gap or a margin.
    Gap,
}

/// A tapped card the strip is centring; its callback fires when it lands.
#[derive(Clone, Copy, Debug)]
struct Pending {
    /// The card's strip position.
    card: usize,
    /// Whether the centring tween has been started (at the first paint after
    /// the tap, which knows `reduce_motion`).
    started: bool,
}

/// A callback that came due during a paint, delivered by the next event.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Owed {
    Select(usize),
    Close,
}

/// A peer card growing into the page (see the module docs' *Expanding a
/// card*).
struct Expansion {
    peer: usize,
    /// Its strip position.
    card: usize,
    /// Whether the app still asks for it; a withdrawn request reverses the
    /// ramp and drops the expansion once it lands back at `0`.
    requested: bool,
    /// Whether the centring step has been issued.
    centring: bool,
    /// The grow, `0 → 1`, on `close_timing`.
    ramp: StageRamp,
}

/// The peers strip: the peer pods, and the scroll surface the cards ride on.
/// It owns the strip position (`offset`, strip px, past an edge during a
/// rubber-band) and embeds a horizontal [`ScrollSurfaceCore`] as a
/// [`ScrollSurface`] owner, `viewport.rs`'s shape: the core runs the gesture,
/// the takeover, the release physics and the tweens, and reads and writes the
/// position through the trait's required methods. The live card is the
/// widget's own child; the strip only places it.
struct Strip {
    peers: Vec<PeerPods>,
    live_index: usize,
    /// The requested expansion, a peer index (the view's value).
    expanding: Option<usize>,
    gap: f64,
    peer_alpha: f64,
    on_select: Option<ErasedArgCallback<usize>>,
    core: ScrollSurfaceCore,
    /// The strip position in strip px: card `k` is centred at `k × stride`.
    offset: f64,
    /// The stride at full open, `0` before the first layout.
    stride: f64,
    stage: Size,
    card_scale: f64,
    /// Put the live card back in the centre at the next layout or paint.
    recentre: bool,
    /// The card under this gesture's press, until a takeover or the release.
    tap: Option<Hit>,
    /// Whether this gesture's press found the strip at rest (a press that
    /// catches a moving strip is never a tap).
    tap_eligible: bool,
    /// A tap the gesture just completed, for the widget to resolve.
    tap_outcome: Option<Hit>,
    pending: Option<Pending>,
    owed: Option<Owed>,
    expansion: Option<Expansion>,
}

impl Strip {
    fn build<State: 'static>(view: &CardSwitcherView<State>, ctx: &mut BuildCtx<'_>) -> Self {
        Self {
            peers: view
                .peers
                .iter()
                .map(|peer| build_peer(peer, ctx))
                .collect(),
            live_index: view.live_index,
            expanding: view.expanding,
            gap: view.options.gap,
            peer_alpha: view.options.peer_alpha,
            on_select: view.on_select.as_ref().map(erase_callback_arg),
            core: ScrollSurfaceCore::with_axis(ScrollAxis::Horizontal),
            offset: 0.0,
            stride: 0.0,
            stage: Size::ZERO,
            card_scale: view.options.card_scale,
            recentre: true,
            tap: None,
            tap_eligible: false,
            tap_outcome: None,
            pending: None,
            owed: None,
            expansion: None,
        }
    }

    /// Positional reconcile of the peer list, then the plain values.
    fn rebuild<State: 'static>(
        &mut self,
        prev: &CardSwitcherView<State>,
        next: &CardSwitcherView<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let keep = prev.peers.len().min(next.peers.len());
        for (view, mut pods) in prev.peers[keep..].iter().zip(self.peers.drain(keep..)) {
            teardown_peer(view, &mut pods, ctx);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        for ((old, new), pods) in prev.peers.iter().zip(&next.peers).zip(&mut self.peers) {
            flags |= rebuild_child(&old.face, &new.face, &mut pods.face, ctx);
            flags |= rebuild_slot(&old.label, &new.label, &mut pods.label, ctx);
        }
        for view in &next.peers[keep..] {
            self.peers.push(build_peer(view, ctx));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if self.live_index != next.live_index || self.expanding != next.expanding {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        self.live_index = next.live_index;
        self.expanding = next.expanding;
        self.gap = next.options.gap;
        self.peer_alpha = next.options.peer_alpha;
        self.on_select = next.on_select.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown<State: 'static>(&mut self, view: &CardSwitcherView<State>, ctx: &mut BuildCtx<'_>) {
        for (peer, pods) in view.peers.iter().zip(&mut self.peers) {
            teardown_peer(peer, pods, ctx);
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, label_bc: &BoxConstraints) {
        for pods in &mut self.peers {
            pods.face.layout_child(ctx, bc);
            pods.face.set_origin(Point::ZERO);
            if let Some(label) = &mut pods.label {
                label.layout_child(ctx, label_bc);
            }
        }
    }

    /// Hand a non-`Shortcut` broadcast to every peer pod: a card answers no
    /// chord, but a queued callback still runs.
    fn broadcast(&mut self, ctx: &mut EventCtx, event: &InputEvent) {
        if matches!(event, InputEvent::Shortcut(_)) {
            return;
        }
        for pods in &mut self.peers {
            pods.face.event_child(ctx, event);
            if let Some(label) = &mut pods.label {
                label.event_child(ctx, event);
            }
        }
    }

    // --- Strip positions ---

    fn card_count(&self) -> usize {
        self.peers.len() + 1
    }

    /// The live card's strip position (the view's index, clamped).
    fn live(&self) -> usize {
        self.live_index.min(self.peers.len())
    }

    /// The peer at strip position `card`, or `None` for the live card.
    fn peer_of(&self, card: usize) -> Option<usize> {
        let live = self.live();
        match card.cmp(&live) {
            std::cmp::Ordering::Less => Some(card),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(card - 1),
        }
    }

    /// Whether a fling, spring, settle, tween or drag is moving the strip.
    fn in_motion(&self) -> bool {
        self.core.scrolling || self.core.settling || self.core.is_flinging()
    }

    /// Whether the strip rests on card `card`'s slot.
    fn on_slot(&self, card: usize) -> bool {
        !self.in_motion() && (self.offset - card as f64 * self.stride).abs() <= ON_SLOT_PX
    }

    /// A tap's centring or an expansion owns the strip; pointer input waits.
    fn busy(&self) -> bool {
        self.pending.is_some() || self.expansion.is_some()
    }

    /// The geometry at full open, which every settled-open event reads.
    fn rest_geometry(&self) -> StripGeometry {
        StripGeometry {
            stage: self.stage,
            scale: self.card_scale,
            stride: self.stride,
            ratio: 1.0,
            offset: self.offset,
        }
    }

    /// Whether `position` lies in the card row: the band the cards span,
    /// across the whole stage.
    fn in_band(&self, position: Point) -> bool {
        let row = self.rest_geometry().rect(0);
        position.y >= row.y0 && position.y < row.y1
    }

    fn hit(&self, position: Point) -> Hit {
        let geometry = self.rest_geometry();
        (0..self.card_count())
            .find(|&card| geometry.rect(card).contains(position))
            .map_or(Hit::Gap, Hit::Card)
    }

    /// Follow the stage size, card scale and gap: a new stride rescales the
    /// position (keeping the card it shows), installs the snap at that stride
    /// and restarts whatever motion was measured in the old one. Then apply an
    /// owed recentre and, at rest, keep the position in range. Runs at every
    /// layout and paint.
    fn prepare(&mut self, stage: Size, card_scale: f64) {
        self.stage = stage;
        self.card_scale = card_scale;
        let stride = stage.width * card_scale + self.gap;
        let stride = if stride.is_finite() && stride > 0.0 {
            stride
        } else {
            0.0
        };
        if stride != self.stride {
            let page = if self.stride > 0.0 {
                self.offset / self.stride
            } else {
                0.0
            };
            let page = if (page - page.round()).abs() <= CENTRED_EPS {
                page.round()
            } else {
                page
            };
            self.stride = stride;
            self.offset = page * stride;
            if stride > 0.0 {
                let snap: Rc<dyn ScrollPhysics> = Rc::new(PageSnap::extent(stride));
                self.core.install(Some(&snap), default_overscroll_effect());
            }
            self.core.stop_ballistic();
            self.core.fling = None;
            self.core.settling = false;
            if let Some(pending) = &mut self.pending {
                pending.started = false;
            }
            if let Some(expansion) = &mut self.expansion
                && !expansion.ramp.is_open()
            {
                expansion.centring = false;
            }
        }
        if self.recentre && stride > 0.0 {
            self.offset = self.live() as f64 * stride;
            self.recentre = false;
        }
        if !self.in_motion() {
            self.offset = self.offset.clamp(0.0, self.max_scroll_extent());
        }
    }

    /// The open flip: the strip starts centred on the live card, at rest, with
    /// nothing in flight.
    fn reset_for_open(&mut self) {
        self.end_gesture();
        self.core.stop_ballistic();
        self.core.fling = None;
        self.core.settling = false;
        self.core.edge_pull = 0.0;
        self.recentre = true;
        self.tap = None;
        self.tap_outcome = None;
        self.pending = None;
        self.owed = None;
        self.expansion = None;
    }

    /// The close flip: drop the tap state, and hand back an expansion's grow
    /// progress for the close to continue from.
    fn take_for_close(&mut self) -> Option<f64> {
        self.end_gesture();
        self.tap = None;
        self.tap_outcome = None;
        self.pending = None;
        self.owed = None;
        self.expansion
            .take()
            .map(|expansion| expansion.ramp.value().clamp(0.0, 1.0))
    }

    /// Disarm a gesture an open flip cut short: its remaining events go to the
    /// stage's ramp-time path, so the core must not keep a drag (and the
    /// motion flag it implies) alive until the next press.
    fn end_gesture(&mut self) {
        self.core.scrolling = false;
        self.core.down_active = false;
    }

    /// After a press released without a tap: a strip left between slots (a
    /// caught spring, a cancelled drag) springs to the nearest card, on the
    /// installed snap at zero velocity. Returns whether it started one.
    fn resettle(&mut self) -> bool {
        if self.in_motion() || self.stride <= 0.0 {
            return false;
        }
        let nearest = (self.offset / self.stride)
            .round()
            .clamp(0.0, self.peers.len() as f64);
        if (self.offset - nearest * self.stride).abs() <= ON_SLOT_PX {
            return false;
        }
        let Some(sim) = self
            .core
            .physics
            .create_ballistic_simulation(&self.metrics(), 0.0)
        else {
            return false;
        };
        self.core.ballistic = Some(BallisticState { sim, start: None });
        true
    }

    /// Match the expansion to the request; returns whether one began.
    fn sync_expansion(&mut self, open: bool) -> bool {
        let requested = self
            .expanding
            .filter(|&peer| open && peer < self.peers.len());
        match (requested, self.expansion.as_mut()) {
            (Some(peer), Some(expansion)) if expansion.peer == peer => {
                expansion.requested = true;
                false
            }
            (Some(peer), _) => {
                let card = if peer < self.live() { peer } else { peer + 1 };
                self.expansion = Some(Expansion {
                    peer,
                    card,
                    requested: true,
                    centring: false,
                    ramp: StageRamp::new(false),
                });
                true
            }
            (None, Some(expansion)) => {
                if expansion.requested {
                    expansion.requested = false;
                    expansion.ramp.set_open(false);
                }
                false
            }
            (None, None) => false,
        }
    }

    /// The paint-side half of the strip: start an expansion's centring or a
    /// tap's tween, pump the core, land a tap that came to rest (owing its
    /// callback to the next event), and run an expansion's grow. Returns
    /// whether an expansion began this frame.
    fn advance(
        &mut self,
        ctx: &mut PaintCtx,
        reduced: bool,
        expand_timing: Timing,
        open: bool,
    ) -> bool {
        let began = self.sync_expansion(open);
        let centre_on = self
            .expansion
            .as_ref()
            .filter(|expansion| !expansion.centring)
            .map(|expansion| expansion.card);
        if let Some(card) = centre_on {
            self.pending = None;
            self.tap = None;
            if !self.on_slot(card) {
                let target = card as f64 * self.stride;
                self.apply_animate_to(target, CENTRE_TWEEN, reduced);
            }
            if let Some(expansion) = &mut self.expansion {
                expansion.centring = true;
            }
        }
        let start = self
            .pending
            .as_mut()
            .filter(|pending| !pending.started)
            .map(|pending| {
                pending.started = true;
                pending.card
            });
        if let Some(card) = start {
            let target = card as f64 * self.stride;
            self.apply_animate_to(target, CENTRE_TWEEN, reduced);
        }
        self.paint_motion(ctx, reduced);
        let resting = !self.in_motion();
        if resting && let Some(pending) = self.pending.take_if(|pending| pending.started) {
            self.owed = Some(match self.peer_of(pending.card) {
                Some(peer) => Owed::Select(peer),
                None => Owed::Close,
            });
        }
        if self.owed.is_some() {
            // Paint has no `&mut State`: owe the callback to the next frame's
            // Housekeeping broadcast.
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
        if let Some(expansion) = &mut self.expansion {
            if expansion.centring && resting && expansion.requested && !expansion.ramp.is_open() {
                expansion.ramp.set_open(true);
            }
            if expansion
                .ramp
                .advance(ctx.frame_time(), expand_timing, reduced)
            {
                ctx.request_frame();
            }
            if !expansion.requested && !expansion.ramp.is_open() && expansion.ramp.settled() {
                self.expansion = None;
            }
        }
        began
    }
}

/// The strip's side of the scroll core: one position, past an edge during a
/// rubber-band, exactly the viewport's storage shape. The "content" the core
/// forwards to is the tap tracker: no card ever receives a pointer event.
impl ScrollSurface for Strip {
    fn core(&self) -> &ScrollSurfaceCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut ScrollSurfaceCore {
        &mut self.core
    }

    fn max_scroll_extent(&self) -> f64 {
        self.peers.len() as f64 * self.stride
    }

    fn viewport_extent(&self) -> f64 {
        self.stage.width
    }

    fn base_offset(&self) -> f64 {
        self.offset
    }

    fn set_base_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_scroll_extent());
    }

    fn pixels(&self) -> f64 {
        self.offset
    }

    fn displacement(&self) -> f64 {
        self.offset - self.offset.clamp(0.0, self.max_scroll_extent())
    }

    fn set_allowed_position(&mut self, allowed: f64) {
        self.offset = allowed;
    }

    fn decay_displacement(&mut self, retained: f64) {
        let target = self.offset.clamp(0.0, self.max_scroll_extent());
        let remaining = target - self.offset;
        self.offset = target - remaining * retained;
    }

    fn settle_to_edge(&mut self) {
        self.offset = self.offset.clamp(0.0, self.max_scroll_extent());
    }

    /// Nothing to place: paint reads the position afresh every frame.
    fn sync_origins(&mut self) {}

    fn scroll_info(&self) -> ScrollInfo {
        let max_offset = self.max_scroll_extent();
        ScrollInfo {
            offset: self.offset.clamp(0.0, max_offset),
            max_offset,
            overscroll: self.displacement(),
        }
    }

    /// The tap tracker. The core forwards a gesture's `Down`, every move it
    /// has not taken over, and its `Up` only when it never took the drag: so
    /// an `Up` that arrives here with the press's card still recorded is a
    /// tap, by the core's own claim outcome.
    fn forward_pointer(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(pointer) = event else {
            return EventResult::Ignored;
        };
        match pointer.phase {
            PointerPhase::Down if presses(pointer) => self.tap = Some(self.hit(pointer.position)),
            PointerPhase::Up => {
                if let Some(hit) = self.tap.take()
                    && self.tap_eligible
                {
                    self.tap_outcome = Some(hit);
                }
            }
            PointerPhase::Cancel => self.tap = None,
            _ => {}
        }
        EventResult::Handled
    }

    fn forward_wheel(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
        EventResult::Ignored
    }

    /// The drag crossed the slop: the press is no longer a tap.
    fn take_over_from_content(&mut self, _ctx: &mut EventCtx, _position: Point) {
        self.tap = None;
    }
}

impl VisitPods for Strip {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.peers.visit_pods(visitor);
    }
}

fn build_peer<State: 'static>(peer: &PeerCard<State>, ctx: &mut BuildCtx<'_>) -> PeerPods {
    PeerPods {
        face: build_child(&peer.face, ctx),
        label: peer.label.as_ref().map(|label| build_child(label, ctx)),
    }
}

fn teardown_peer<State: 'static>(
    peer: &PeerCard<State>,
    pods: &mut PeerPods,
    ctx: &mut BuildCtx<'_>,
) {
    teardown_child(&peer.face, &mut pods.face, ctx);
    if let (Some(view), Some(pod)) = (&peer.label, &mut pods.label) {
        teardown_child(view, pod, ctx);
    }
}

/// Reconcile an optional slot: rebuild in place, or build/tear down the side
/// that appeared/vanished.
fn rebuild_slot<State: 'static>(
    prev: &Option<AnyView<State>>,
    next: &Option<AnyView<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    if let (Some(old), Some(new), Some(existing)) = (prev, next, pod.as_mut()) {
        return rebuild_child(old, new, existing, ctx);
    }
    let mut flags = ChangeFlags::NONE;
    if let (Some(old), Some(mut existing)) = (prev, pod.take()) {
        teardown_child(old, &mut existing, ctx);
        flags = ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    if let Some(new) = next {
        *pod = Some(build_child(new, ctx));
        flags = ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    flags
}

fn teardown_slot<State: 'static>(
    view: &Option<AnyView<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) {
    if let (Some(view), Some(pod)) = (view, pod) {
        teardown_child(view, pod, ctx);
    }
}

/// Place a label row `gap` above `card`, flush with its left edge.
fn place_label(label: &mut ChildPod, card: Rect, gap: f64) {
    label.set_origin(Point::new(card.x0, card.y0 - gap - label.size().height));
}

// --- Widget ----------------------------------------------------------------------

/// The retained widget for a [`CardSwitcherView`]. See the [module docs](self).
pub struct CardSwitcherWidget {
    child: ChildPod,
    live_label: Option<ChildPod>,
    dots: Option<ChildPod>,
    action_bar: Option<ChildPod>,
    strip: Strip,
    on_close: Option<ErasedCallback>,
    options: StageOptions,
    ramp: StageRamp,
    /// A primary press landed on the card during a ramp; its release inside
    /// closes.
    close_armed: bool,
    /// The current gesture belongs to the strip (its press landed on the card
    /// row while the stage sat open).
    strip_gesture: bool,
    /// The child held a focus link when the stage opened.
    saved_focus: bool,
}

impl<State: 'static> View<State> for CardSwitcherView<State> {
    type Element = CardSwitcherWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardSwitcherWidget {
        CardSwitcherWidget {
            child: build_child(&self.live, ctx),
            live_label: self.live_label.as_ref().map(|v| build_child(v, ctx)),
            dots: self.dots.as_ref().map(|v| build_child(v, ctx)),
            action_bar: self.action_bar.as_ref().map(|v| build_child(v, ctx)),
            strip: Strip::build(self, ctx),
            on_close: self.on_close.as_ref().map(erase_callback),
            options: self.options,
            ramp: StageRamp::new(self.open),
            close_armed: false,
            strip_gesture: false,
            saved_focus: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardSwitcherWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.ramp.is_open() != self.open {
            element.strip_gesture = false;
            if self.open {
                element.ramp.set_open(true);
                element.strip.reset_for_open();
                element.suspend_child();
            } else {
                match element.strip.take_for_close() {
                    Some(grown) => element.ramp.close_from(1.0 - grown),
                    None => element.ramp.set_open(false),
                }
                element.resume_child(ctx.has_focus());
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.options != self.options {
            element.options = self.options;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_close = self.on_close.as_ref().map(erase_callback);
        flags |= rebuild_child(&prev.live, &self.live, &mut element.child, ctx);
        flags |= rebuild_slot(
            &prev.live_label,
            &self.live_label,
            &mut element.live_label,
            ctx,
        );
        flags |= rebuild_slot(&prev.dots, &self.dots, &mut element.dots, ctx);
        flags |= rebuild_slot(
            &prev.action_bar,
            &self.action_bar,
            &mut element.action_bar,
            ctx,
        );
        flags |= element.strip.rebuild(prev, self, ctx);
        flags
    }

    fn teardown(&self, element: &mut CardSwitcherWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.live, &mut element.child, ctx);
        teardown_slot(&self.live_label, &mut element.live_label, ctx);
        teardown_slot(&self.dots, &mut element.dots, ctx);
        teardown_slot(&self.action_bar, &mut element.action_bar, ctx);
        element.strip.teardown(self, ctx);
    }
}

impl CardSwitcherWidget {
    /// Open or still animating: the stage draws and input is blocked.
    fn engaged(&self) -> bool {
        self.ramp.is_open() || !self.ramp.settled()
    }

    /// Settled open with nothing in flight: a press on the card row is the
    /// strip's.
    fn strip_ready(&self) -> bool {
        self.ramp.is_open() && self.ramp.settled() && !self.strip.busy() && self.strip.stride > 0.0
    }

    /// The open flip: unwind the child's capture, then save and clear its focus
    /// link so keys stop reaching a page the user cannot see whole.
    fn suspend_child(&mut self) {
        self.close_armed = false;
        if self.child.is_active() {
            crate::authoring::cancel_pod(&mut self.child);
            self.child.set_active(false);
        }
        if self.child.is_focused() {
            self.saved_focus = true;
            self.child.set_focused(false);
        }
    }

    /// The close flip: drop the slots' gestures and focus, and restore the
    /// child's saved link when focus still runs down to this stage.
    fn resume_child(&mut self, chain_live: bool) {
        self.close_armed = false;
        for pod in [&mut self.live_label, &mut self.dots, &mut self.action_bar]
            .into_iter()
            .flatten()
        {
            if pod.is_active() {
                crate::authoring::cancel_pod(pod);
                pod.set_active(false);
            }
            pod.set_focused(false);
        }
        if std::mem::take(&mut self.saved_focus) && chain_live {
            self.child.set_focused(true);
        }
    }

    /// An expansion began: end any slot gesture, since no pointer input
    /// reaches a child until it is over.
    fn cancel_slot_gestures(&mut self) {
        self.close_armed = false;
        self.strip_gesture = false;
        for pod in [&mut self.live_label, &mut self.dots, &mut self.action_bar]
            .into_iter()
            .flatten()
        {
            if pod.is_active() {
                crate::authoring::cancel_pod(pod);
                pod.set_active(false);
            }
        }
    }

    /// The stage's pose at the ramps' current progress.
    fn frame(&self) -> Frame {
        let stage = self.child.size();
        let options = &self.options;
        let strip = &self.strip;
        let stage_g = self.ramp.geometry();
        let (grow_g, grow_p, expanding) = match &strip.expansion {
            Some(expansion) => (
                expansion.ramp.geometry(),
                expansion.ramp.value().clamp(0.0, 1.0),
                Some(expansion.card),
            ),
            None => (0.0, 0.0, None),
        };
        let g = stage_g * (1.0 - grow_g);
        let scale = 1.0 + (options.card_scale - 1.0) * g;
        let ratio = if strip.stride > 0.0 {
            (stage.width * scale + strip.gap) / strip.stride
        } else {
            1.0
        };
        // Closing, the strip eases back to the live card with the ramp.
        let live = strip.live();
        let home = live as f64 * strip.stride;
        let offset = home + (strip.offset - home) * stage_g;
        let alpha = self.ramp.value().clamp(0.0, 1.0);
        Frame {
            geometry: StripGeometry {
                stage,
                scale,
                stride: strip.stride,
                ratio,
                offset,
            },
            live,
            radius: (options.card_radius * g.clamp(0.0, 1.0)).max(0.0),
            alpha,
            label: ((alpha - LABEL_FADE_START) / FADE_SPAN).clamp(0.0, 1.0),
            bar: (alpha / FADE_SPAN).clamp(0.0, 1.0),
            others: ((EXPAND_FADE_END - grow_p) / FADE_SPAN).clamp(0.0, 1.0),
            expanding,
            shadow_fade: 1.0 - grow_p,
            peer_alpha: strip.peer_alpha.clamp(0.0, 1.0),
        }
    }

    /// Card `card`'s pod: the live child or a peer face.
    fn card_pod(&mut self, card: usize) -> &mut ChildPod {
        match self.strip.peer_of(card) {
            Some(peer) => &mut self.strip.peers[peer].face,
            None => &mut self.child,
        }
    }

    /// Re-set every card's transform and the slot origins from `frame`.
    fn apply_frame(&mut self, frame: &Frame) {
        let engaged = self.engaged();
        let geometry = frame.geometry;
        for card in 0..self.strip.card_count() {
            self.card_pod(card)
                .set_transform(engaged.then(|| geometry.transform(card)));
        }
        let label_gap = self.options.label_gap;
        for peer in 0..self.strip.peers.len() {
            let card = if peer < frame.live { peer } else { peer + 1 };
            if let Some(label) = &mut self.strip.peers[peer].label {
                place_label(label, geometry.rect(card), label_gap);
            }
        }
        let row = geometry.rect(frame.live);
        if let Some(label) = &mut self.live_label {
            place_label(label, row, label_gap);
        }
        let stage = geometry.stage;
        if let Some(dots) = &mut self.dots {
            let x = (stage.width - dots.size().width) / 2.0;
            dots.set_origin(Point::new(x, row.y1 + self.options.dots_gap));
        }
        if let Some(bar) = &mut self.action_bar {
            let bar_size = bar.size();
            bar.set_origin(Point::new(
                (stage.width - bar_size.width) / 2.0,
                stage.height - bar_size.height,
            ));
        }
    }

    fn fire_close(&mut self, ctx: &mut EventCtx) {
        if let Some(on_close) = &mut self.on_close {
            on_close(ctx);
        }
        ctx.request_redraw();
    }

    fn fire_select(&mut self, ctx: &mut EventCtx, peer: usize) {
        if let Some(on_select) = &mut self.strip.on_select {
            on_select(ctx, peer);
        }
        ctx.request_redraw();
    }

    /// Deliver a callback a paint owed (a centring tween that landed).
    fn deliver_owed(&mut self, ctx: &mut EventCtx) {
        match self.strip.owed.take() {
            Some(Owed::Select(peer)) => self.fire_select(ctx, peer),
            Some(Owed::Close) => self.fire_close(ctx),
            None => {}
        }
    }

    /// Offer an uncaptured pointer event to the visible slots, top first; a
    /// captured slot keeps its gesture.
    fn route_slots(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> Option<EventResult> {
        let frame = self.frame();
        let slots = [
            (&mut self.action_bar, frame.bar_alpha()),
            (&mut self.dots, frame.label_alpha()),
            (&mut self.live_label, frame.label_alpha()),
        ];
        let mut visible: Vec<&mut ChildPod> = Vec::new();
        for (slot, alpha) in slots {
            if let Some(pod) = slot {
                if pod.is_active() {
                    return Some(route_event_single(pod, ctx, event));
                }
                if alpha > 0.0 {
                    visible.push(pod);
                }
            }
        }
        let position = event.position();
        for pod in visible {
            if pod.contains(position) && route_event_single(pod, ctx, event) == EventResult::Handled
            {
                return Some(EventResult::Handled);
            }
        }
        None
    }

    /// Hand one event of a strip gesture to the scroll core, then resolve a
    /// tap it completed or re-settle a strip a release left between cards.
    fn strip_pointer(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        pointer: &PointerEvent,
    ) -> EventResult {
        let strip = &mut self.strip;
        let press = pointer.phase == PointerPhase::Down;
        if press {
            strip.tap_eligible = !strip.in_motion();
        }
        // The strip is not nested (module docs): keep its registration out of
        // any enclosing surface's claim cell, as a cross-axis viewport does.
        let host = if press && presses(pointer) {
            ambient_scroll_claim().map(|host| (host.get(), host))
        } else {
            None
        };
        let t_ms = strip.core.event_time_ms();
        strip.handle_pointer(ctx, event, pointer, t_ms);
        if let Some((saved, host)) = host {
            host.set(saved);
        }
        let ended = matches!(pointer.phase, PointerPhase::Up | PointerPhase::Cancel);
        if ended {
            self.strip_gesture = false;
        }
        match self.strip.tap_outcome.take() {
            Some(hit) => self.resolve_tap(ctx, hit),
            None if ended && self.strip.resettle() => ctx.request_redraw(),
            None => {}
        }
        EventResult::Handled
    }

    /// A tap the strip completed: close, select at once, or centre first.
    fn resolve_tap(&mut self, ctx: &mut EventCtx, hit: Hit) {
        let Hit::Card(card) = hit else {
            self.fire_close(ctx);
            return;
        };
        if !self.strip.on_slot(card) {
            self.strip.pending = Some(Pending {
                card,
                started: false,
            });
            ctx.request_redraw();
            return;
        }
        match self.strip.peer_of(card) {
            Some(peer) => self.fire_select(ctx, peer),
            None => self.fire_close(ctx),
        }
    }

    /// The input block: no pointer event reaches a card.
    fn blocked_pointer(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let position = event.position();
        let size = ctx.size();
        let inside = position.x >= 0.0
            && position.y >= 0.0
            && position.x < size.width
            && position.y < size.height;
        if self.strip.busy() {
            self.strip_gesture = false;
            self.close_armed = false;
            return if inside {
                EventResult::Handled
            } else {
                EventResult::Ignored
            };
        }
        if self.strip_gesture
            && let InputEvent::Pointer(pointer) = event
        {
            return self.strip_pointer(ctx, event, pointer);
        }
        let open = self.ramp.is_open();
        if open && let Some(result) = self.route_slots(ctx, event) {
            return result;
        }
        let InputEvent::Pointer(pointer) = event else {
            return if inside {
                EventResult::Handled
            } else {
                EventResult::Ignored
            };
        };
        if pointer.phase == PointerPhase::Down
            && inside
            && presses(pointer)
            && self.strip_ready()
            && self.strip.in_band(position)
        {
            self.strip_gesture = true;
            return self.strip_pointer(ctx, event, pointer);
        }
        let armed = self.close_armed;
        match pointer.phase {
            PointerPhase::Down if inside => {
                if !self.child.contains(position) {
                    if open {
                        self.fire_close(ctx);
                    }
                } else if open && presses(pointer) {
                    self.close_armed = true;
                    ctx.capture_pointer();
                }
            }
            PointerPhase::Up => {
                self.close_armed = false;
                if armed && open && self.child.contains(position) {
                    self.fire_close(ctx);
                }
            }
            PointerPhase::Cancel => self.close_armed = false,
            _ => {}
        }
        if inside || armed {
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }

    fn paint_slot(
        pod: &mut Option<ChildPod>,
        alpha: f64,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
    ) {
        let Some(pod) = pod else { return };
        if alpha <= 0.0 {
            return;
        }
        if alpha >= 1.0 {
            pod.paint_child(ctx, scene);
            return;
        }
        scene.push_layer(
            ctx.origin() + pod.origin().to_vec2(),
            pod.size(),
            alpha as f32,
        );
        pod.paint_child(ctx, scene);
        scene.pop_layer();
    }
}

/// The scrim and shadow colours at full open: explicit > theme > fallback.
fn resolve_backdrop(theme: Option<&Theme>, options: &StageOptions) -> (Color, (Color, f64, f64)) {
    let scrim = options.scrim.unwrap_or_else(|| match theme {
        Some(theme) => theme.scheme().scrim.multiply_alpha(BACKDROP_ALPHA),
        None => FALLBACK_SCRIM,
    });
    let shadow = options.shadow.unwrap_or_else(|| {
        let ink = theme.map_or(Color::BLACK, |theme| theme.scheme().shadow);
        (ink.multiply_alpha(BACKDROP_ALPHA), SHADOW_Y, SHADOW_STD_DEV)
    });
    (scrim, shadow)
}

impl Widget for CardSwitcherWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        let card_width = (size.width * self.options.card_scale).max(0.0);
        let label_bc = BoxConstraints::new(
            Size::new(card_width, 0.0),
            Size::new(card_width, size.height),
        );
        let loose = BoxConstraints::loose(size);
        if let Some(label) = &mut self.live_label {
            label.layout_child(ctx, &label_bc);
        }
        for pod in [&mut self.dots, &mut self.action_bar].into_iter().flatten() {
            pod.layout_child(ctx, &loose);
        }
        self.strip.layout(ctx, bc, &label_bc);
        self.strip.prepare(size, self.options.card_scale);
        let frame = self.frame();
        self.apply_frame(&frame);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduced = theme.is_some_and(|theme| theme.motion.reduce_motion);
        let reduced_timing = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        let timing = if reduced {
            reduced_timing
        } else if self.ramp.is_open() {
            self.options.open_timing
        } else {
            self.options.close_timing
        };
        let (scrim, (shadow, shadow_y, shadow_std_dev)) = resolve_backdrop(theme, &self.options);
        // A transition with an endpoint: the unpaced class.
        if self.ramp.advance(ctx.frame_time(), timing, reduced) {
            ctx.request_frame();
        }
        self.strip
            .prepare(self.child.size(), self.options.card_scale);
        let expand_timing = if reduced {
            reduced_timing
        } else {
            self.options.close_timing
        };
        let open = self.ramp.is_open();
        if self.strip.advance(ctx, reduced, expand_timing, open) {
            self.cancel_slot_gestures();
        }
        let frame = self.frame();
        self.apply_frame(&frame);
        if !self.engaged() {
            self.child.paint_child(ctx, scene);
            return;
        }
        let alpha = frame.alpha;
        let origin = ctx.origin().to_vec2();
        let width = ctx.size().width;
        scene.fill_rect(ctx.origin(), ctx.size(), scrim.multiply_alpha(alpha as f32));
        for card in 0..self.strip.card_count() {
            let card_alpha = frame.card_alpha(card);
            let local = frame.geometry.rect(card);
            if card_alpha <= 0.0 || local.x1 <= 0.0 || local.x0 >= width {
                continue;
            }
            let rect = local + origin;
            scene.draw_shadow(
                rect.origin() + Vec2::new(0.0, shadow_y * alpha),
                rect.size(),
                frame.radius,
                shadow_std_dev * alpha,
                shadow.multiply_alpha(frame.shadow_alpha(card) as f32),
            );
            let layered = card_alpha < 1.0;
            if layered {
                scene.push_layer(rect.origin(), rect.size(), card_alpha as f32);
            }
            scene.push_clip_rounded(rect.origin(), rect.size(), frame.radius);
            self.card_pod(card).paint_child(ctx, scene);
            scene.pop_clip();
            if layered {
                scene.pop_layer();
            }
        }
        let (label_alpha, bar_alpha) = (frame.label_alpha(), frame.bar_alpha());
        Self::paint_slot(&mut self.live_label, label_alpha, ctx, scene);
        for pods in &mut self.strip.peers {
            Self::paint_slot(&mut pods.label, label_alpha, ctx, scene);
        }
        Self::paint_slot(&mut self.dots, label_alpha, ctx, scene);
        Self::paint_slot(&mut self.action_bar, bar_alpha, ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.deliver_owed(ctx);
        if event.is_broadcast() {
            let shown = self.engaged() || !matches!(event, InputEvent::Shortcut(_));
            self.child.event_child(ctx, event);
            if shown {
                for pod in [&mut self.live_label, &mut self.dots, &mut self.action_bar]
                    .into_iter()
                    .flatten()
                {
                    pod.event_child(ctx, event);
                }
            }
            self.strip.broadcast(ctx, event);
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            let pods = [&mut self.live_label, &mut self.dots, &mut self.action_bar];
            let focused = std::iter::once(&mut self.child)
                .chain(pods.into_iter().flatten())
                .find(|pod| pod.holds_live_focus());
            return match focused {
                Some(pod) => pod.event_child(ctx, event),
                None => EventResult::Ignored,
            };
        }
        if !self.engaged() {
            self.close_armed = false;
            self.strip_gesture = false;
            return route_event_single(&mut self.child, ctx, event);
        }
        self.blocked_pointer(ctx, event)
    }

    /// Closed, the child is the Tab stop; open, only the slots are.
    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if !self.engaged() {
            visitor(&self.child);
            return;
        }
        if self.ramp.is_open() {
            for pod in [&self.live_label, &self.dots, &self.action_bar]
                .into_iter()
                .flatten()
            {
                visitor(pod);
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
        if self.ramp.is_open() {
            for pod in [&self.live_label, &self.dots, &self.action_bar]
                .into_iter()
                .flatten()
            {
                pod.semantics_child(ctx);
            }
        }
    }

    crate::authoring::visit_children!(strip, child, live_label, dots, action_bar);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf, probe};
    use frust_core::PointerButton;
    use std::any::Any;
    use std::cell::RefCell;

    const PHONE: Size = Size::new(402.0, 874.0);
    const TABLET: Size = Size::new(1194.0, 834.0);

    /// One recorded scene call, in paint order.
    #[derive(Clone, Debug, PartialEq)]
    enum Op {
        Fill(Rect, f32),
        Shadow(Rect, f64, f64, f32),
        Clip(Rect, f64),
        PopClip,
        Transform(Affine),
        PopTransform,
        Layer(Rect, f32),
        PopLayer,
    }

    #[derive(Default)]
    struct Ops(Vec<Op>);

    impl PaintScene for Ops {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            let rect = Rect::from_origin_size(origin, size);
            self.0.push(Op::Fill(rect, color.components[3]));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            let rect = Rect::from_origin_size(o, s);
            self.0
                .push(Op::Shadow(rect, radius, std_dev, color.components[3]));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
            self.0
                .push(Op::Clip(Rect::from_origin_size(origin, size), radius));
        }
        fn pop_clip(&mut self) {
            self.0.push(Op::PopClip);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.0.push(Op::Transform(transform));
        }
        fn pop_transform(&mut self) {
            self.0.push(Op::PopTransform);
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            let rect = Rect::from_origin_size(origin, size);
            self.0.push(Op::Layer(rect, alpha));
        }
        fn pop_layer(&mut self) {
            self.0.push(Op::PopLayer);
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn rebuild<S: 'static>(
        prev: &CardSwitcherView<S>,
        next: &CardSwitcherView<S>,
        w: &mut CardSwitcherWidget,
    ) -> ChangeFlags {
        let mut counter = 0u64;
        next.rebuild(prev, w, &mut BuildCtx::new(&mut counter))
    }

    fn lay_out(w: &mut CardSwitcherWidget, size: Size) {
        let laid = w.layout(&mut LayoutCtx::new(), &BoxConstraints::tight(size));
        assert_eq!(laid, size, "the stage reports the live child's size");
    }

    /// Paint at `ms`, optionally themed; returns the ops and `needs_frame`.
    fn paint_at(
        w: &mut CardSwitcherWidget,
        size: Size,
        ms: f64,
        theme: Option<&Theme>,
    ) -> (Vec<Op>, bool) {
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut ops = Ops::default();
        w.paint(&mut ctx, &mut ops);
        (ops.0, ctx.needs_frame())
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Dispatch `events` against a `Vec<u32>` log; returns the log and results.
    fn send(
        w: &mut CardSwitcherWidget,
        size: Size,
        events: &[InputEvent],
    ) -> (Vec<u32>, Vec<EventResult>) {
        let mut log = Vec::new();
        let mut results = Vec::new();
        for event in events {
            let mut ctx = EventCtx::new(&mut log, Point::ZERO, size);
            results.push(w.event(&mut ctx, event));
        }
        (log, results)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn near_rect(a: Rect, b: Rect) -> bool {
        near(a.x0, b.x0) && near(a.y0, b.y0) && near(a.x1, b.x1) && near(a.y1, b.y1)
    }

    /// The card's on-screen rect at `scale`, centred in `size`.
    fn card_at(size: Size, scale: f64) -> Rect {
        Rect::from_center_size(size.to_rect().center(), size * scale)
    }

    /// The uniform scale and fixed point of a centre-pivot transform.
    fn scale_and_pivot(t: Affine) -> (f64, Point) {
        let [s, _, _, _, tx, ty] = t.as_coeffs();
        (s, Point::new(tx / (1.0 - s), ty / (1.0 - s)))
    }

    fn closing_view<S: 'static>(open: bool, live: impl View<S>) -> CardSwitcherView<S> {
        card_switcher(open, live)
            .open_timing(Timing::Duration(Duration::from_millis(100), Curve::Linear))
            .close_timing(Timing::Duration(Duration::from_millis(100), Curve::Linear))
    }

    #[test]
    fn closed_paints_the_child_plainly_and_forwards_a_down() {
        let view = card_switcher(false, leaf(10.0, 10.0));
        let mut w = build(&view);
        lay_out(&mut w, PHONE);
        let (ops, frame) = paint_at(&mut w, PHONE, 0.0, None);
        assert_eq!(
            ops,
            vec![Op::Fill(PHONE.to_rect(), 1.0)],
            "no scrim/shadow/clip"
        );
        assert!(!frame, "a closed stage requests no frame");
        assert!(w.child.transform().is_none());

        let view = card_switcher(false, probe(7));
        let mut w = build(&view);
        lay_out(&mut w, PHONE);
        let (log, _) = send(&mut w, PHONE, &[pointer(PointerPhase::Down, 10.0, 10.0)]);
        assert_eq!(
            log,
            vec![7],
            "a closed stage forwards input to the live child"
        );
    }

    #[test]
    fn open_settled_paint_order_and_centred_card_on_phone_and_tablet() {
        for (size, scale) in [(PHONE, CARD_SCALE), (TABLET, 0.58)] {
            let view = card_switcher(true, leaf(10.0, 10.0)).card_scale(scale);
            let mut w = build(&view);
            lay_out(&mut w, size);
            let (ops, frame) = paint_at(&mut w, size, 0.0, None);
            assert!(!frame, "mounting open does not animate in");
            let card = card_at(size, scale);
            let [scrim, shadow, clip, transform, child, pop_t, pop_c] = ops.as_slice() else {
                panic!("unexpected op sequence {ops:?}");
            };
            assert_eq!(*scrim, Op::Fill(size.to_rect(), 0.55));
            let Op::Shadow(rect, radius, std_dev, alpha) = *shadow else {
                panic!("expected the shadow second, got {shadow:?}");
            };
            assert!(near_rect(rect, card + Vec2::new(0.0, SHADOW_Y)));
            assert_eq!((radius, std_dev, alpha), (24.0, SHADOW_STD_DEV, 0.55));
            let Op::Clip(rect, radius) = *clip else {
                panic!("expected the rounded clip third, got {clip:?}");
            };
            assert!(near_rect(rect, card), "clip {rect:?} vs card {card:?}");
            assert_eq!(radius, 24.0);
            let Op::Transform(t) = *transform else {
                panic!("expected the child transform fourth, got {transform:?}");
            };
            let (s, pivot) = scale_and_pivot(t);
            assert!(near(s, scale));
            assert!(near(pivot.x, size.width / 2.0) && near(pivot.y, size.height / 2.0));
            assert!(near_rect(t.transform_rect_bbox(size.to_rect()), card));
            assert_eq!(
                *child,
                Op::Fill(size.to_rect(), 1.0),
                "the child paints inside"
            );
            assert_eq!((pop_t, pop_c), (&Op::PopTransform, &Op::PopClip));
        }
    }

    #[test]
    fn mid_ramp_geometry_is_linear_in_the_driver_value() {
        let closed = closing_view(false, leaf(10.0, 10.0));
        let open = closing_view(true, leaf(10.0, 10.0));
        let mut w = build(&closed);
        assert_eq!(rebuild(&closed, &open, &mut w), ChangeFlags::PAINT);
        lay_out(&mut w, PHONE);
        let (ops, frame) = paint_at(&mut w, PHONE, 0.0, None);
        assert!(frame, "the seed frame starts the ramp");
        assert_eq!(
            ops[0],
            Op::Fill(PHONE.to_rect(), 0.0),
            "the scrim starts clear"
        );

        let (ops, frame) = paint_at(&mut w, PHONE, 50.0, None);
        assert!(frame);
        let p = w.ramp.value();
        assert!(near(p, 0.5), "linear 100 ms at 50 ms, got {p}");
        let Op::Fill(_, scrim) = ops[0] else {
            panic!("{ops:?}")
        };
        assert!(
            (scrim - 0.55 * p as f32).abs() < 1e-6,
            "scrim alpha scales with p"
        );
        let Op::Clip(rect, radius) = ops[2] else {
            panic!("{ops:?}")
        };
        assert!(near(radius, 24.0 * p) && near(radius, 12.0));
        let scale = 1.0 + (CARD_SCALE - 1.0) * p;
        assert!(near(scale, 0.81));
        assert!(near_rect(rect, card_at(PHONE, scale)));
        let Op::Transform(t) = ops[3] else {
            panic!("{ops:?}")
        };
        assert!(near(scale_and_pivot(t).0, scale));

        let (_, frame) = paint_at(&mut w, PHONE, 120.0, None);
        assert!(!frame, "a landed ramp requests no frame");
        assert!(near(w.ramp.value(), 1.0));
    }

    #[test]
    fn slots_fade_on_their_windows_and_sit_around_the_card() {
        let slots = |open: bool| {
            closing_view(open, leaf(10.0, 10.0))
                .live_label(leaf(80.0, 20.0))
                .action_bar(leaf(200.0, 56.0))
        };
        let mut w = build(&slots(false));
        rebuild(&slots(false), &slots(true), &mut w);
        lay_out(&mut w, PHONE);
        paint_at(&mut w, PHONE, 0.0, None);

        // p = 0.5: the bar is fully in, the label has not started.
        let (ops, _) = paint_at(&mut w, PHONE, 50.0, None);
        let after_card = &ops[ops.iter().position(|op| *op == Op::PopClip).unwrap() + 1..];
        let bar = Rect::new(101.0, PHONE.height - 56.0, 301.0, PHONE.height);
        assert_eq!(after_card, &[Op::Fill(bar, 1.0)], "bar only, unlayered");

        // p = 0.8: the label is half in, LABEL_GAP above the card's top edge.
        let (ops, _) = paint_at(&mut w, PHONE, 80.0, None);
        let card = card_at(PHONE, 1.0 + (CARD_SCALE - 1.0) * w.ramp.value());
        let label_width = PHONE.width * CARD_SCALE;
        let label = Rect::new(
            card.x0,
            card.y0 - LABEL_GAP - 20.0,
            card.x0 + label_width,
            card.y0 - LABEL_GAP,
        );
        let after_card = &ops[ops.iter().position(|op| *op == Op::PopClip).unwrap() + 1..];
        let [
            Op::Layer(rect, alpha),
            Op::Fill(..),
            Op::PopLayer,
            Op::Fill(..),
        ] = after_card
        else {
            panic!("expected a layered label then the bar, got {after_card:?}");
        };
        assert!(near_rect(*rect, label), "label {rect:?} vs {label:?}");
        assert!((alpha - 0.5).abs() < 1e-4, "label alpha {alpha}");
    }

    #[test]
    fn open_blocks_the_child_closes_on_card_tap_or_scrim_and_restores_focus() {
        let view =
            |open: bool| card_switcher(open, probe(7)).on_close(|log: &mut Vec<u32>| log.push(99));
        let mut w = build(&view(false));
        w.child.set_focused(true);
        rebuild(&view(false), &view(true), &mut w);
        assert!(
            !w.child.is_focused(),
            "opening clears the child's focus link"
        );
        lay_out(&mut w, PHONE);
        paint_at(&mut w, PHONE, 0.0, None);
        let (_, frame) = paint_at(&mut w, PHONE, 1000.0, None);
        assert!(!frame, "the open ramp has landed");

        let (cx, cy) = (PHONE.width / 2.0, PHONE.height / 2.0);
        let tap = [
            pointer(PointerPhase::Down, cx, cy),
            pointer(PointerPhase::Move, cx + 1.0, cy),
            pointer(PointerPhase::Up, cx, cy),
        ];
        let (log, results) = send(&mut w, PHONE, &tap);
        assert_eq!(
            log,
            vec![99],
            "card tap closes once; the child receives nothing"
        );
        assert!(results.iter().all(|r| *r == EventResult::Handled));

        let (log, _) = send(&mut w, PHONE, &[pointer(PointerPhase::Down, 5.0, 5.0)]);
        assert_eq!(log, vec![99], "a Down on the scrim closes");
        let (log, results) = send(&mut w, PHONE, &[pointer(PointerPhase::Move, 5.0, 5.0)]);
        assert_eq!((log, results), (vec![], vec![EventResult::Handled]));

        rebuild(&view(true), &view(false), &mut w);
        assert!(
            w.child.is_focused(),
            "closing restores the saved focus link"
        );
        paint_at(&mut w, PHONE, 2000.0, None);
        let (log, results) = send(&mut w, PHONE, &[pointer(PointerPhase::Down, cx, cy)]);
        assert_eq!(
            log,
            Vec::<u32>::new(),
            "mid close ramp: still blocked, no re-close"
        );
        assert_eq!(results, vec![EventResult::Handled]);
        paint_at(&mut w, PHONE, 3000.0, None);
        let (log, _) = send(&mut w, PHONE, &[pointer(PointerPhase::Down, cx, cy)]);
        assert_eq!(log, vec![7], "once closed the child takes input again");
    }

    #[test]
    fn reduce_motion_jumps_geometry_and_fades_only_alphas_in_120ms() {
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let full = reduced.scheme().scrim.components[3] * BACKDROP_ALPHA;
        let mut w = build(&card_switcher(false, leaf(10.0, 10.0)));
        rebuild(
            &card_switcher(false, leaf(10.0, 10.0)),
            &card_switcher(true, leaf(10.0, 10.0)),
            &mut w,
        );
        lay_out(&mut w, PHONE);
        for (ms, scrim_alpha, expect_frame) in [
            (0.0, 0.0, true),
            (60.0, 0.5 * full, true),
            (130.0, full, false),
        ] {
            let (ops, frame) = paint_at(&mut w, PHONE, ms, Some(&reduced));
            let Op::Fill(_, scrim) = ops[0] else {
                panic!("{ops:?}")
            };
            assert!((scrim - scrim_alpha).abs() < 1e-3, "{ms} ms: scrim {scrim}");
            let Op::Clip(_, radius) = ops[2] else {
                panic!("{ops:?}")
            };
            let Op::Transform(t) = ops[3] else {
                panic!("{ops:?}")
            };
            assert_eq!(radius, CARD_RADIUS, "{ms} ms: no radius ramp");
            assert!(
                near(scale_and_pivot(t).0, CARD_SCALE),
                "{ms} ms: no scale ramp"
            );
            assert_eq!(frame, expect_frame, "{ms} ms");
        }
    }

    #[test]
    fn a_resize_recentres_the_transform_on_layout_and_paint() {
        let mut w = build(&card_switcher(true, leaf(10.0, 10.0)));
        for size in [PHONE, TABLET] {
            lay_out(&mut w, size);
            let centre = size.to_rect().center();
            let (_, pivot) = scale_and_pivot(w.child.transform().expect("open transform"));
            assert!(
                near(pivot.x, centre.x) && near(pivot.y, centre.y),
                "{size:?}"
            );
            let mut scene = RecordingScene::default();
            let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0));
            w.paint(&mut ctx, &mut scene);
            let (_, painted) = scale_and_pivot(scene.transforms[0]);
            assert!(
                near(painted.x, centre.x) && near(painted.y, centre.y),
                "{size:?}"
            );
        }
    }

    #[test]
    fn peers_are_retained_reconciled_and_laid_out_at_the_full_stage_size() {
        let view = |peers: usize| {
            CardSwitcher(true, leaf(10.0, 10.0))
                .peers((0..peers).map(|i| {
                    let face = peer_card(leaf(1.0, 1.0));
                    if i == 0 {
                        face.label(leaf(40.0, 12.0))
                    } else {
                        face
                    }
                }))
                .live_index(1)
                .live_label(leaf(40.0, 12.0))
                .dots(leaf(30.0, 8.0))
                .action_bar(leaf(200.0, 56.0))
                .on_select(|_: &mut (), _peer: usize| {})
                .gap(20.0)
                .peer_alpha(0.5)
                .scrim(Color::WHITE)
                .shadow(Color::BLACK, 8.0, 4.0)
                .label_gap(10.0)
                .dots_gap(10.0)
        };
        let mut w = build(&view(3));
        lay_out(&mut w, PHONE);
        assert_eq!(w.strip.peers.len(), 3);
        assert!(
            w.strip.peers.iter().all(|p| p.face.size() == PHONE),
            "full stage size"
        );
        paint_at(&mut w, PHONE, 0.0, None);

        for peers in [1, 4] {
            let prev = view(if peers == 1 { 3 } else { 1 });
            rebuild(&prev, &view(peers), &mut w);
            lay_out(&mut w, PHONE);
            paint_at(&mut w, PHONE, 0.0, None);
            assert_eq!(w.strip.peers.len(), peers);
        }
    }

    // --- The strip ---

    /// Every `on_select` (`Some(peer)`) and `on_close` (`None`) in order.
    type Log = Rc<RefCell<Vec<Option<usize>>>>;

    /// `peers` leaf peers around a leaf live card at `live`, logging callbacks.
    fn strip_view(open: bool, peers: usize, live: usize, log: &Log) -> CardSwitcherView<()> {
        let (select, close) = (Rc::clone(log), Rc::clone(log));
        card_switcher(open, leaf(1.0, 1.0))
            .peers((0..peers).map(|_| peer_card(leaf(1.0, 1.0))))
            .live_index(live)
            .on_select(move |_: &mut (), peer: usize| select.borrow_mut().push(Some(peer)))
            .on_close(move |_: &mut ()| close.borrow_mut().push(None))
    }

    /// A laid-out, painted stage driven by frame time and pointer events.
    struct Rig {
        w: CardSwitcherWidget,
        size: Size,
        t: f64,
        theme: Option<Theme>,
    }

    impl Rig {
        fn new(view: &CardSwitcherView<()>, size: Size) -> Self {
            Self::themed(view, size, None)
        }

        fn themed(view: &CardSwitcherView<()>, size: Size, theme: Option<Theme>) -> Self {
            let mut w = build(view);
            lay_out(&mut w, size);
            let mut rig = Self {
                w,
                size,
                t: 0.0,
                theme,
            };
            rig.frame(0.0);
            rig
        }

        /// Paint `dt_ms` after the last frame; returns the ops and
        /// `needs_frame`.
        fn frame(&mut self, dt_ms: f64) -> (Vec<Op>, bool) {
            self.t += dt_ms;
            paint_at(&mut self.w, self.size, self.t, self.theme.as_ref())
        }

        fn rebuild(&mut self, prev: &CardSwitcherView<()>, next: &CardSwitcherView<()>) {
            rebuild(prev, next, &mut self.w);
            lay_out(&mut self.w, self.size);
        }

        fn send(&mut self, event: &InputEvent) -> EventResult {
            let mut unit = ();
            let state: &mut dyn Any = &mut unit;
            let mut ctx = EventCtx::new(state, Point::ZERO, self.size);
            self.w.event(&mut ctx, event)
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64) -> EventResult {
            self.send(&pointer(phase, x, self.size.height / 2.0))
        }

        /// A press and release at `x` on the card row.
        fn tap(&mut self, x: f64) {
            self.pointer(PointerPhase::Down, x);
            self.frame(16.0);
            self.pointer(PointerPhase::Up, x);
        }

        /// Press at `from`, take the drag over past the slop, then move the
        /// finger by `travel` (positive = rightward) in `steps` moves `gap_ms`
        /// apart.
        fn drag(&mut self, from: f64, travel: f64, steps: u32, gap_ms: f64) {
            self.pointer(PointerPhase::Down, from);
            self.frame(gap_ms);
            // Crossing the slop only takes the gesture over; the position
            // follows the finger from the next move on.
            let taken = from + 20.0 * travel.signum();
            self.pointer(PointerPhase::Move, taken);
            self.frame(gap_ms);
            for step in 1..=steps {
                self.pointer(
                    PointerPhase::Move,
                    taken + travel * f64::from(step) / f64::from(steps),
                );
                self.frame(gap_ms);
            }
        }

        /// Hold the finger still long enough to zero the release velocity,
        /// then lift it.
        fn hold_and_release(&mut self) {
            let x = self.finger();
            self.frame(150.0);
            self.pointer(PointerPhase::Move, x);
            self.pointer(PointerPhase::Up, x);
        }

        fn finger(&self) -> f64 {
            self.w.strip.core.last_drag.x
        }

        /// Frames until every motion rests, then the Housekeeping event that
        /// delivers an owed callback.
        fn settle(&mut self) {
            for _ in 0..200 {
                self.frame(16.0);
            }
            self.send(&InputEvent::Housekeeping);
        }

        fn offset(&self) -> f64 {
            self.w.strip.offset
        }

        fn stride(&self) -> f64 {
            self.w.strip.stride
        }
    }

    /// The layer alphas a paint recorded, in paint order.
    fn layer_alphas(ops: &[Op]) -> Vec<f32> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Layer(_, alpha) => Some(*alpha),
                _ => None,
            })
            .collect()
    }

    fn clips(ops: &[Op]) -> Vec<(Rect, f64)> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Clip(rect, radius) => Some((*rect, *radius)),
                _ => None,
            })
            .collect()
    }

    /// The on-screen rect a pod's transform maps the full stage to.
    fn shown(pod: &ChildPod, size: Size) -> Rect {
        pod.transform()
            .expect("an open card is transformed")
            .transform_rect_bbox(size.to_rect())
    }

    #[test]
    fn strip_geometry_centres_the_live_card_and_peeks_its_neighbours() {
        for (size, scale) in [(PHONE, CARD_SCALE), (TABLET, 0.58)] {
            let log = Log::default();
            let view = strip_view(true, 3, 1, &log).card_scale(scale).gap(16.0);
            let mut rig = Rig::new(&view, size);
            let stride = size.width * scale + 16.0;
            if size == PHONE {
                assert!(near(stride, 265.24), "phone stride {stride}");
            }
            assert!(near(rig.stride(), stride), "{size:?}");
            assert!(near(rig.offset(), stride), "{size:?}: live card 1 centred");

            let w = &rig.w;
            let card = card_at(size, scale);
            assert!(
                near_rect(shown(&w.child, size), card),
                "{size:?}: live centred"
            );
            let left = shown(&w.strip.peers[0].face, size);
            assert!(near_rect(left, card - Vec2::new(stride, 0.0)), "{size:?}");
            assert!(
                left.x0 < 0.0 && left.x1 > 0.0,
                "{size:?}: left neighbour peeks"
            );
            let right = shown(&w.strip.peers[1].face, size);
            assert!(near_rect(right, card + Vec2::new(stride, 0.0)), "{size:?}");
            assert!(
                right.x0 < size.width && right.x1 > size.width,
                "{size:?}: right neighbour peeks"
            );
            let far = shown(&w.strip.peers[2].face, size);
            assert!(far.x0 >= size.width, "{size:?}: card 3 is off stage");

            let (ops, frame) = rig.frame(16.0);
            assert!(!frame, "{size:?}: a resting strip requests no frame");
            let drawn: Vec<Rect> = clips(&ops).into_iter().map(|(rect, _)| rect).collect();
            assert_eq!(drawn.len(), 3, "{size:?}: the off-stage card is culled");
            assert!(near_rect(drawn[0], left) && near_rect(drawn[1], card));
            assert!(near_rect(drawn[2], right));
        }
    }

    #[test]
    fn the_centred_card_is_opaque_and_a_drag_cross_fades_its_neighbours() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 2, 1, &log), PHONE);
        let (ops, _) = rig.frame(16.0);
        assert_eq!(
            layer_alphas(&ops),
            vec![PEER_ALPHA as f32, PEER_ALPHA as f32],
            "both neighbours dimmed, the centre unlayered"
        );

        let half = rig.stride() / 2.0;
        rig.drag(201.0, -half, 1, 16.0);
        assert!(near(rig.offset(), 3.0 * half), "half a stride along");
        let (ops, _) = rig.frame(0.0);
        // Card 0 has left the stage; the live card and card 2 sit half a
        // stride either side of the centre.
        let mid = (1.0 - (1.0 - PEER_ALPHA) * 0.5) as f32;
        let alphas = layer_alphas(&ops);
        assert_eq!(alphas.len(), 2, "{alphas:?}");
        assert!(alphas.iter().all(|a| (a - mid).abs() < 1e-6), "{alphas:?}");
    }

    #[test]
    fn a_slow_release_springs_back_and_a_fling_pages_without_selecting() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 3, 1, &log), PHONE);
        let stride = rig.stride();

        rig.drag(201.0, -0.4 * stride, 4, 100.0);
        assert!(near(rig.offset(), 1.4 * stride), "dragged 0.4 stride");
        rig.hold_and_release();
        rig.settle();
        assert!(near(rig.offset(), stride), "back on the live card");
        assert!(log.borrow().is_empty(), "a drag selects nothing");

        rig.drag(201.0, -120.0, 3, 16.0);
        rig.pointer(PointerPhase::Up, rig.finger());
        rig.settle();
        assert!(near(rig.offset(), 2.0 * stride), "the fling paged once");
        assert!(log.borrow().is_empty(), "selection is tap-only");
    }

    #[test]
    fn tapping_a_peek_card_centres_it_then_selects_once() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 3, 1, &log), PHONE);
        let stride = rig.stride();
        rig.tap(370.0);
        assert!(
            log.borrow().is_empty(),
            "nothing fires before the strip lands"
        );
        rig.frame(16.0);
        rig.frame(16.0);
        assert!(
            rig.offset() > stride && rig.offset() < 2.0 * stride,
            "tweening"
        );
        rig.settle();
        assert!(near(rig.offset(), 2.0 * stride));
        assert_eq!(*log.borrow(), vec![Some(1)], "card 2 is peer 1");
        rig.settle();
        assert_eq!(*log.borrow(), vec![Some(1)], "exactly once");

        // The now-centred peer answers at once; the live card off-centre
        // centres, then closes.
        rig.tap(201.0);
        assert_eq!(*log.borrow(), vec![Some(1), Some(1)]);
        rig.tap(30.0);
        rig.settle();
        assert!(near(rig.offset(), stride));
        assert_eq!(*log.borrow(), vec![Some(1), Some(1), None]);
    }

    #[test]
    fn tapping_the_centred_live_card_or_the_row_gap_closes() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 3, 1, &log), PHONE);
        rig.tap(201.0);
        assert_eq!(*log.borrow(), vec![None], "the centred live card closes");
        // The gap between the live card and its right neighbour.
        let gap = PHONE.width / 2.0 + PHONE.width * CARD_SCALE / 2.0 + GAP / 2.0;
        rig.tap(gap);
        assert_eq!(
            *log.borrow(),
            vec![None, None],
            "a tap in the row's gap closes"
        );
        assert!(near(rig.offset(), rig.stride()), "and moves nothing");
    }

    #[test]
    fn a_tap_during_a_centring_tween_is_ignored() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 3, 1, &log), PHONE);
        let stride = rig.stride();
        rig.tap(370.0);
        rig.frame(16.0);
        rig.frame(48.0);
        let before = rig.offset();
        assert!(before > stride && before < 2.0 * stride, "mid tween");
        // A second tap (on the live card, now off-centre) and a drag: both
        // swallowed, the tween untouched.
        assert_eq!(rig.pointer(PointerPhase::Down, 150.0), EventResult::Handled);
        assert_eq!(rig.pointer(PointerPhase::Up, 150.0), EventResult::Handled);
        rig.drag(201.0, 150.0, 2, 16.0);
        rig.pointer(PointerPhase::Up, rig.finger());
        assert!(rig.w.strip.core.ballistic.is_some(), "the tween still runs");
        rig.settle();
        assert!(near(rig.offset(), 2.0 * stride));
        assert_eq!(*log.borrow(), vec![Some(1)], "only the first tap answers");
    }

    #[test]
    fn a_press_that_catches_a_spring_is_not_a_tap() {
        let log = Log::default();
        let mut rig = Rig::new(&strip_view(true, 3, 1, &log), PHONE);
        let stride = rig.stride();
        rig.drag(201.0, -120.0, 3, 16.0);
        rig.pointer(PointerPhase::Up, rig.finger());
        rig.frame(16.0);
        rig.frame(16.0);
        assert!(rig.w.strip.in_motion(), "the release spring runs");
        rig.tap(201.0);
        assert!(log.borrow().is_empty(), "the catch is not a tap");
        rig.settle();
        assert!(log.borrow().is_empty());
        let page = rig.offset() / stride;
        assert!(near(page, page.round()), "re-settled on a card, at {page}");
    }

    #[test]
    fn a_drag_past_either_end_rubber_bands_and_springs_back() {
        for (live, travel) in [(0, 120.0), (2, -120.0)] {
            let log = Log::default();
            let mut rig = Rig::new(&strip_view(true, 2, live, &log), PHONE);
            let max = 2.0 * rig.stride();
            rig.drag(201.0, travel, 4, 16.0);
            let offset = rig.offset();
            if live == 0 {
                assert!(offset < 0.0, "pulled past the first card: {offset}");
            } else {
                assert!(offset > max, "pulled past the last card: {offset}");
            }
            rig.hold_and_release();
            rig.settle();
            let rest = if live == 0 { 0.0 } else { max };
            assert!((rig.offset() - rest).abs() < 1e-6, "sprang back to {rest}");
            assert!(log.borrow().is_empty());
        }
    }

    #[test]
    fn reduce_motion_shows_the_peers_in_place_and_fades_them_in_120ms() {
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let log = Log::default();
        let closed = strip_view(false, 2, 1, &log);
        let mut rig = Rig::themed(&closed, PHONE, Some(reduced));
        rig.rebuild(&closed, &strip_view(true, 2, 1, &log));
        let (ops, frame) = rig.frame(16.0);
        assert!(frame);
        let stride = rig.stride();
        let card = card_at(PHONE, CARD_SCALE);
        let peers = &rig.w.strip.peers;
        assert!(near_rect(
            shown(&peers[0].face, PHONE),
            card - Vec2::new(stride, 0.0)
        ));
        assert!(near_rect(
            shown(&peers[1].face, PHONE),
            card + Vec2::new(stride, 0.0)
        ));
        assert_eq!(clips(&ops).len(), 1, "the peers start transparent");

        let (ops, frame) = rig.frame(60.0);
        assert!(
            frame && clips(&ops).len() == 1,
            "still inside the label window"
        );
        let (ops, frame) = rig.frame(60.0);
        let alphas = layer_alphas(&ops);
        assert_eq!(alphas.len(), 2);
        assert!(alphas.iter().all(|a| (a - PEER_ALPHA as f32).abs() < 1e-6));
        assert!(!frame, "settled within 120 ms");
    }

    // --- Expanding ---

    /// Probe cards (the live child 7, peers 10..) and a full-stage probe bar
    /// that would take any pointer event that got through.
    fn probe_view(open: bool, expanding: Option<usize>) -> CardSwitcherView<Vec<u32>> {
        card_switcher(open, probe(7))
            .peers((0..3).map(|i| peer_card(probe(10 + i)).label(probe(20 + i))))
            .live_index(1)
            .action_bar(probe(30))
            .expanding(expanding)
            .on_select(|log: &mut Vec<u32>, peer: usize| log.push(100 + peer as u32))
            .on_close(|log: &mut Vec<u32>| log.push(99))
            .close_timing(Timing::Duration(Duration::from_millis(100), Curve::Linear))
    }

    /// Paint until the expansion's grow starts; returns the frame time of the
    /// paint that started it.
    fn run_until_growing(w: &mut CardSwitcherWidget, mut ms: f64) -> f64 {
        for _ in 0..100 {
            ms += 16.0;
            paint_at(w, PHONE, ms, None);
            if w.strip.expansion.as_ref().is_some_and(|e| e.ramp.is_open()) {
                return ms;
            }
        }
        panic!("the expansion never started growing");
    }

    #[test]
    fn expanding_centres_the_card_then_grows_it_while_the_rest_fades() {
        let mut w = build(&probe_view(true, None));
        lay_out(&mut w, PHONE);
        paint_at(&mut w, PHONE, 0.0, None);
        let stride = w.strip.stride;
        rebuild(&probe_view(true, None), &probe_view(true, Some(2)), &mut w);
        lay_out(&mut w, PHONE);

        // Centring card 3 (peer 2): every pointer event is swallowed.
        paint_at(&mut w, PHONE, 16.0, None);
        paint_at(&mut w, PHONE, 64.0, None);
        assert!(w.strip.offset > stride, "centring");
        let (cx, cy) = (PHONE.width / 2.0, PHONE.height / 2.0);
        let presses = [
            pointer(PointerPhase::Down, cx, cy),
            pointer(PointerPhase::Up, cx, cy),
            pointer(PointerPhase::Down, 5.0, 5.0),
            pointer(PointerPhase::Up, 5.0, 5.0),
        ];
        let (log, results) = send(&mut w, PHONE, &presses);
        assert_eq!(log, Vec::<u32>::new(), "no callback, no child");
        assert!(results.iter().all(|r| *r == EventResult::Handled));

        let start = run_until_growing(&mut w, 64.0);
        assert!(near(w.strip.offset, 3.0 * stride), "card 3 centred first");
        let (ops, frame) = paint_at(&mut w, PHONE, start + 50.0, None);
        assert!(frame);
        let face = &w.strip.peers[2].face;
        let (scale, pivot) = scale_and_pivot(face.transform().expect("transformed"));
        assert!(near(scale, 0.81), "scale {scale}");
        assert!(
            near(pivot.x, cx) && near(pivot.y, cy),
            "about the stage centre"
        );
        let grown = card_at(PHONE, 0.81);
        let (rect, radius) = *clips(&ops).last().expect("the growing card");
        assert!(near_rect(rect, grown) && near(radius, 12.0));
        let shadow = ops.iter().rev().find_map(|op| match op {
            Op::Shadow(_, _, _, alpha) => Some(*alpha),
            _ => None,
        });
        assert!((shadow.unwrap() - 0.55 * 0.5).abs() < 1e-6, "shadow fading");
        let others = ((EXPAND_FADE_END - 0.5) / FADE_SPAN) as f32;
        let alphas = layer_alphas(&ops);
        assert!(
            alphas
                .iter()
                .any(|a| (a - PEER_ALPHA as f32 * others).abs() < 1e-6),
            "the neighbour fades: {alphas:?}"
        );
        let (log, _) = send(&mut w, PHONE, &presses);
        assert_eq!(log, Vec::<u32>::new(), "still blocked mid-grow");

        let (ops, frame) = paint_at(&mut w, PHONE, start + 200.0, None);
        assert!(!frame, "the grow landed and holds");
        let [
            Op::Fill(_, _),
            Op::Shadow(_, _, _, shadow),
            Op::Clip(rect, radius),
            Op::Transform(t),
            Op::PopTransform,
            Op::PopClip,
        ] = ops.as_slice()
        else {
            panic!("only the grown card, no slot or other card: {ops:?}");
        };
        assert_eq!((*shadow, *radius), (0.0, 0.0));
        assert!(near_rect(*rect, PHONE.to_rect()));
        assert!(near_rect(
            t.transform_rect_bbox(PHONE.to_rect()),
            PHONE.to_rect()
        ));
        let (log, _) = send(&mut w, PHONE, &presses);
        assert_eq!(log, Vec::<u32>::new(), "blocked while it holds");

        // The app's swap: closed, peer 2's page live at card 3. Nothing moves.
        let swapped = card_switcher(false, probe(12))
            .peers((0..3).map(|i| peer_card(probe(10 + i))))
            .live_index(3);
        rebuild(&probe_view(true, Some(2)), &swapped, &mut w);
        lay_out(&mut w, PHONE);
        let (ops, frame) = paint_at(&mut w, PHONE, start + 216.0, None);
        assert!(!frame && ops.is_empty(), "closed at once: {ops:?}");
        assert!(w.child.transform().is_none());
        let (log, _) = send(&mut w, PHONE, &[pointer(PointerPhase::Down, cx, cy)]);
        // A probe rebuilt in place keeps its id: the live pod is the same one.
        assert_eq!(log, vec![7], "the live page takes input again");
    }

    #[test]
    fn closing_mid_grow_continues_from_the_grown_scale() {
        let mut w = build(&probe_view(true, Some(1)));
        lay_out(&mut w, PHONE);
        let start = run_until_growing(&mut w, 0.0);
        paint_at(&mut w, PHONE, start + 50.0, None);
        rebuild(&probe_view(true, Some(1)), &probe_view(false, None), &mut w);
        paint_at(&mut w, PHONE, start + 66.0, None);
        assert!(
            near(w.ramp.value(), 0.5),
            "the close picks up at the grow's 0.5"
        );
        let (scale, _) = scale_and_pivot(w.child.transform().expect("closing"));
        assert!(near(scale, 0.81), "scale {scale}");
    }
}
