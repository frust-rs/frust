//! [`card_switcher`]: an app-switcher stage. The live page shrinks into a
//! rounded card over a scrim, with a label row, page dots and an action bar
//! around it, and peer cards beside it.
//!
//! # Composition
//!
//! One wrapper owns the whole stage. The live child keeps the full constraints
//! and is only *transformed*: [`ChildPod::set_transform`] scales it about its
//! centre, so paint, hit tests and event localisation follow the shrunken card
//! while the page keeps its layout and state. With open progress `p`
//! (`0` closed, `1` open) the scale is `1 → card_scale`, the on-screen corner
//! radius `0 → card_radius`, and the stage paints, in parent space: the scrim
//! over the whole stage, the card's drop shadow, a rounded clip at the card's
//! on-screen rect, the transformed child, then the slots — the label row
//! `label_gap` above the card, the dots `dots_gap` below it, and the action bar
//! flush with the bottom edge (it consumes its own window inset: wrap it in
//! `safe_area`). Labels and dots fade in over the last 40 % of `p`, the bar over
//! the first 40 %. Clip and shadow stay in parent space because a clip radius
//! is recorded in pre-transform space. Closed, the child paints plainly: no
//! transform, clip, shadow or scrim.
//!
//! Peers ([`peer_card`]) and [`expanding`](CardSwitcherView::expanding) are
//! accepted and retained, and their pods are laid out at the full stage size,
//! but the stage draws and hit-tests only the live card for now.
//!
//! # Input blocking
//!
//! While the stage is open or animating no pointer event reaches the live
//! child: a press and release on the card calls `on_close`, a press anywhere
//! else in the stage (the scrim) calls it at once, and every other pointer
//! event in the stage's bounds is swallowed. Blocking covers only the stage's
//! own bounds, so wrap the whole shell page, not a body under app chrome. Key
//! and IME routing is untouched. Opening cancels the child's capture and clears
//! its focus link; closing restores the link when focus still runs through the
//! stage.
//!
//! # Reduced motion
//!
//! Under `theme.motion.reduce_motion` the scale and radius jump to the target
//! pose on the first paint, and only the alphas (scrim, shadow, slots) run, as
//! a 120 ms linear fade, whatever `open_timing`/`close_timing` say.
//!
//! # Flutter
//!
//! No direct analog; `OpenContainer` (the `animations` package) and `Hero` are
//! the nearest.

use std::rc::Rc;
use std::time::Duration;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, Tween,
    View, Widget, any,
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
/// How close the ramp must sit to its target to count as landed while the
/// driver still formally runs (`motion::animated`'s tolerance).
const SETTLED_EPS: f64 = 1e-9;

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

    /// Called with the **peer** index of a selected peer card.
    pub fn on_select(mut self, on_select: impl Fn(&mut State, usize) + 'static) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Called when the open stage asks to close (a tap on the card or scrim).
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

    /// The peer card expanding into the live page, if any.
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
}

/// The stage geometry and alphas at the ramp's current progress, in the
/// stage's local space.
struct Pose {
    /// The live child's transform about its centre.
    transform: Affine,
    /// The card's on-screen rect.
    card: Rect,
    radius: f64,
    /// Open progress for the scrim and shadow, clamped to `0..=1`.
    alpha: f64,
}

impl Pose {
    fn new(ramp: &StageRamp, options: &StageOptions, size: Size) -> Self {
        let g = ramp.geometry();
        let scale = 1.0 + (options.card_scale - 1.0) * g;
        let centre = Point::new(size.width / 2.0, size.height / 2.0).to_vec2();
        Self {
            transform: Affine::translate(centre)
                * Affine::scale(scale)
                * Affine::translate(-centre),
            card: Rect::from_center_size(centre.to_point(), size * scale),
            radius: (options.card_radius * g.clamp(0.0, 1.0)).max(0.0),
            alpha: ramp.value().clamp(0.0, 1.0),
        }
    }

    fn label_alpha(&self) -> f64 {
        ((self.alpha - LABEL_FADE_START) / FADE_SPAN).clamp(0.0, 1.0)
    }

    fn bar_alpha(&self) -> f64 {
        (self.alpha / FADE_SPAN).clamp(0.0, 1.0)
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

/// The peers strip. Today it builds, rebuilds, tears down and lays out every
/// peer face at the full stage size (labels at the card's on-screen width),
/// forwards non-`Shortcut` broadcasts to them, and paints and hit-tests
/// nothing. Peer geometry, selection (`on_select` with the peer index) and the
/// `expanding` phase belong here; the stage keeps drawing the live card, its
/// ramp, scrim, shadow, slots and input block.
#[allow(dead_code)] // live_index, expanding, gap, peer_alpha and on_select wait on peer geometry
struct Strip {
    peers: Vec<PeerPods>,
    live_index: usize,
    expanding: Option<usize>,
    gap: f64,
    peer_alpha: f64,
    on_select: Option<ErasedArgCallback<usize>>,
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

    /// Hand a non-`Shortcut` broadcast to every peer pod: an unpainted card
    /// answers no chord, but a queued callback still runs.
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
    /// A primary press landed on the open card; its release inside closes.
    close_armed: bool,
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
            element.ramp.set_open(self.open);
            if self.open {
                element.suspend_child();
            } else {
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

    /// Re-set the child's transform and the slot origins from `pose`.
    fn apply_pose(&mut self, pose: &Pose, size: Size) {
        let engaged = self.engaged();
        self.child.set_transform(engaged.then_some(pose.transform));
        let card = pose.card;
        if let Some(label) = &mut self.live_label {
            let gap = self.options.label_gap;
            label.set_origin(Point::new(card.x0, card.y0 - gap - label.size().height));
        }
        if let Some(dots) = &mut self.dots {
            let x = card.center().x - dots.size().width / 2.0;
            dots.set_origin(Point::new(x, card.y1 + self.options.dots_gap));
        }
        if let Some(bar) = &mut self.action_bar {
            let bar_size = bar.size();
            bar.set_origin(Point::new(
                (size.width - bar_size.width) / 2.0,
                size.height - bar_size.height,
            ));
        }
    }

    fn fire_close(&mut self, ctx: &mut EventCtx) {
        if let Some(on_close) = &mut self.on_close {
            on_close(ctx);
        }
        ctx.request_redraw();
    }

    /// Offer an uncaptured pointer event to the visible slots, top first; a
    /// captured slot keeps its gesture.
    fn route_slots(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> Option<EventResult> {
        let pose = Pose::new(&self.ramp, &self.options, self.child.size());
        let slots = [
            (&mut self.action_bar, pose.bar_alpha()),
            (&mut self.dots, pose.label_alpha()),
            (&mut self.live_label, pose.label_alpha()),
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

    /// The input block: no pointer event reaches the live child.
    fn blocked_pointer(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let open = self.ramp.is_open();
        if open && let Some(result) = self.route_slots(ctx, event) {
            return result;
        }
        let position = event.position();
        let size = ctx.size();
        let inside = position.x >= 0.0
            && position.y >= 0.0
            && position.x < size.width
            && position.y < size.height;
        let InputEvent::Pointer(pointer) = event else {
            return if inside {
                EventResult::Handled
            } else {
                EventResult::Ignored
            };
        };
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
        let pose = Pose::new(&self.ramp, &self.options, size);
        self.apply_pose(&pose, size);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduced = theme.is_some_and(|theme| theme.motion.reduce_motion);
        let timing = if reduced {
            Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
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
        let size = self.child.size();
        let pose = Pose::new(&self.ramp, &self.options, size);
        self.apply_pose(&pose, size);
        if !self.engaged() {
            self.child.paint_child(ctx, scene);
            return;
        }
        let alpha = pose.alpha;
        let card = pose.card + ctx.origin().to_vec2();
        scene.fill_rect(ctx.origin(), ctx.size(), scrim.multiply_alpha(alpha as f32));
        scene.draw_shadow(
            card.origin() + Vec2::new(0.0, shadow_y * alpha),
            card.size(),
            pose.radius,
            shadow_std_dev * alpha,
            shadow.multiply_alpha(alpha as f32),
        );
        scene.push_clip_rounded(card.origin(), card.size(), pose.radius);
        self.child.paint_child(ctx, scene);
        scene.pop_clip();
        let (label_alpha, bar_alpha) = (pose.label_alpha(), pose.bar_alpha());
        Self::paint_slot(&mut self.live_label, label_alpha, ctx, scene);
        Self::paint_slot(&mut self.dots, label_alpha, ctx, scene);
        Self::paint_slot(&mut self.action_bar, bar_alpha, ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
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
    use frust_core::{PointerButton, PointerEvent};

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
    fn peers_and_expanding_are_retained_without_drawing() {
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
                .expanding(Some(0))
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
        let (ops, _) = paint_at(&mut w, PHONE, 0.0, None);
        let fills = ops.iter().filter(|op| matches!(op, Op::Fill(..))).count();
        assert_eq!(
            fills, 5,
            "scrim, live card and three slots; no peer is drawn"
        );

        for peers in [1, 4] {
            let prev = view(if peers == 1 { 3 } else { 1 });
            rebuild(&prev, &view(peers), &mut w);
            lay_out(&mut w, PHONE);
            paint_at(&mut w, PHONE, 0.0, None);
            assert_eq!(w.strip.peers.len(), peers);
        }
    }
}
