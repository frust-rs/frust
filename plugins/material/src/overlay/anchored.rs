// Ported from `frust-shadcn`'s `plugins/shadcn/src/overlay/anchored.rs`
// (in-repo sibling catalog; itself a port of shadcn/ui v4 rev
// `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, MIT © shadcn, whose own placement
// source is `@radix-ui/react-popper`). A sibling port, never a dependency — see
// `super`'s header.
// Porting decisions: the placement math (side/align/flip/clamp), the
// window-space anchor capture and the light-dismiss/Escape semantics are
// carried over verbatim. Two Material changes: the gap/motion constants resolve
// to `crate::tokens` presets (see *Motion* below), and the **host itself owns
// the enter/exit ramp** rather than delegating it to the content view the way
// the source does — which is what lets a closed, settled host paint nothing at
// all instead of relying on every panel component to ramp itself out.

//! The anchored (non-modal) overlay host: a full-area layer that positions its
//! content relative to a trigger's rect — the seam every Material menu,
//! dropdown and tooltip presents through.
//!
//! # The three pieces
//!
//! 1. [`place_anchored`] — the placement math, a pure function over
//!    `(anchor, content size, area, `[`OverlayPlacement`]`)`. It is public and
//!    unit-tested on its own; the host below is the only in-crate caller.
//! 2. [`OverlayAnchor`] + [`overlay_anchor`] — the trigger side. The anchor
//!    rect is **window space**, and the only place a widget learns its
//!    window-space position is
//!    [`PaintCtx::origin`](frust::authoring::PaintCtx::origin), which is
//!    absolute; so [`overlay_anchor`] wraps the trigger view in a transparent
//!    pod that writes `Rect::from_origin_size(ctx.origin(), ctx.size())` into a
//!    shared cell on every paint. The host reads the same cell.
//! 3. [`anchored_overlay`] — the host itself: a full-area widget holding one
//!    content child, positioned by [`place_anchored`], ramped in and out, and
//!    light-dismissing on a press outside it.
//!
//! # Mounting, and what an exit animation costs
//!
//! There are two ways to drive an anchored overlay, and they differ only in
//! what happens on the way *out*:
//!
//! * **Mount-on-open** (the simplest): the app mounts the host while its own
//!   flag is set and drops it when the flag clears. The overlay appears with
//!   its entrance ramp and disappears instantly, because the widget it would
//!   have ramped out is gone by the next frame.
//! * **Kept-mounted** (what an exit animation needs): the app mounts the host
//!   *unconditionally* and hands the flag down through
//!   [`AnchoredOverlayView::open`]. A closed host lays its content out and,
//!   once its exit ramp settles, **paints nothing at all** — no layer, no
//!   content, no chrome — while claiming no focus, dismissing on nothing and
//!   publishing no semantics. From then on it costs one layout of the content
//!   per frame and nothing else.
//!
//! The framework has no seam for keeping a conditionally-mounted view alive
//! past the rebuild that unmounts it, so an exit ramp is only possible for a
//! widget that **stays mounted with `open == false`** (the same gap registered
//! as `shadcn-anchored-exit-needs-kept-mounted` in `docs/LIMITATIONS.md`, hit
//! first by the sibling catalog this module is ported from). A rebuild that
//! unmounts the host mid-ramp simply truncates the exit; nothing breaks, the
//! panel just vanishes.
//!
//! Placement runs whether the host is open or not, so
//! [`AnchoredOverlayWidget::content_rect`] stays truthful for the whole ramp —
//! which is what makes the exit-ramp swallow below possible: a closing host
//! still knows exactly where its (still fading) content sits.
//!
//! # Motion
//!
//! The ramp drives both the composited alpha and a scale about the point of
//! the content nearest the trigger, so a menu grows out of the control that
//! opened it rather than out of its own centre. The source's Tailwind timings
//! map onto `crate::tokens`' M3 presets:
//!
//! | Source class | Material preset |
//! |---|---|
//! | `duration-200` (entrance) | [`MaterialMotion::SHORT_4`] (200ms) |
//! | `ease-out` (entrance) | [`MaterialMotion::EMPHASIZED_DECELERATE`] |
//! | `duration-150` (exit) | [`MaterialMotion::SHORT_3`] (150ms) |
//! | (exit easing) | [`MaterialMotion::STANDARD_ACCELERATE`] — M3's curve for elements leaving the screen |
//! | `zoom-in-95` | [`ANCHORED_ENTER_SCALE`], carried over verbatim (a visual constant, not a token) |
//!
//! `Theme.motion.reduce_motion` collapses either ramp to a jump: the driver is
//! stopped and progress snaps to its rest value on the pass that would have
//! started it.
//!
//! # Coordinate spaces
//!
//! The anchor is captured in absolute window coordinates; the host places
//! content in its own local space. The host learns its own absolute origin from
//! its paint pass (the same `PaintCtx::origin` read) and subtracts it, so the
//! two agree even when the host is not itself at the window origin — with a
//! one-frame lag on the pass where the host first paints or moves, which the
//! paint arm corrects by requesting a relayout. In the supported mounts (a
//! transparent navigator page, or the top child of a full-area
//! [`frust::Stack`]) the host *is* at the window origin, so the correction is
//! the identity and the lag is unobservable.
//!
//! # Dismissal
//!
//! - **Light dismiss**: a `Down` outside the placed content fires `on_dismiss`
//!   and is **consumed**, so it never reaches the app below (a click-through
//!   variant is not offered in v1). A `Down` *inside* the content routes to the
//!   content and, if nothing there takes it, is swallowed — pressing a panel's
//!   own background never dismisses.
//! - **Escape**: dismisses once the host holds focus. The host claims focus on
//!   every `Down` it sees (the [`mod@crate::dialog`]/[`crate::sheet`] opt-in) — so,
//!   exactly as for every modal widget in the workspace, a caller must complete
//!   one pointer interaction with the overlay before Escape does anything.
//!   There is still no auto-focus-on-appear hook in the framework.
//! - **Move/Scroll pass through**: only `Down` (and a focus-routed `Escape`) is
//!   consumed outside the content. A `Move` outside is reported
//!   [`EventResult::Ignored`] and the host claims no hover of its own, so a
//!   hover-driven overlay (a tooltip) can still see its trigger's hover end
//!   underneath the host.
//! - **Exit-ramp swallow**: while a kept-mounted host is closed but its content
//!   is still visually present, a `Down` landing inside
//!   [`AnchoredOverlayWidget::content_rect`] is **absorbed by the host** —
//!   forwarded to nothing and re-arming no dismissal — rather than falling
//!   through to whatever the page has underneath, and rather than reaching a
//!   panel the app has already dismissed. A `Down` outside the content, and
//!   every press once the ramp has fully settled, passes straight through: the
//!   click that closed the overlay already reached the page, and that stays
//!   true for anything outside the content's own bounds.

use std::rc::Rc;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, SemanticsCtx, Size, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, Theme};

use super::{finite_or_zero, reduce_motion};
use crate::tokens::{MaterialMotion, MaterialSpacing};

/// The side of the anchor an overlay opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlaySide {
    /// Above the anchor.
    Top,
    /// To the trailing side of the anchor.
    Right,
    /// Below the anchor — the default.
    #[default]
    Bottom,
    /// To the leading side of the anchor.
    Left,
}

impl OverlaySide {
    /// The side a collision flip lands on.
    pub fn opposite(self) -> Self {
        match self {
            OverlaySide::Top => OverlaySide::Bottom,
            OverlaySide::Bottom => OverlaySide::Top,
            OverlaySide::Left => OverlaySide::Right,
            OverlaySide::Right => OverlaySide::Left,
        }
    }

    /// Whether this side stacks the overlay vertically (`Top`/`Bottom`).
    pub fn is_vertical(self) -> bool {
        matches!(self, OverlaySide::Top | OverlaySide::Bottom)
    }
}

/// How the overlay lines up with the anchor on the cross axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayAlign {
    /// Leading edges flush (top edges for a left/right side, left edges for a
    /// top/bottom one).
    Start,
    /// Centers flush — the default.
    #[default]
    Center,
    /// Trailing edges flush.
    End,
}

/// A resolved placement request: which side, how it lines up, how far off the
/// anchor it sits, and whether it may flip or shift to stay inside the area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayPlacement {
    /// The preferred side.
    pub side: OverlaySide,
    /// Cross-axis alignment.
    pub align: OverlayAlign,
    /// Gap between the anchor and the overlay, in logical px.
    pub offset: f64,
    /// Flip to [`OverlaySide::opposite`] when the preferred side does not fit
    /// and the opposite one does.
    pub flip: bool,
    /// Shift the placed rect back inside the area when it overflows.
    pub clamp: bool,
}

/// The default gap between a trigger and its anchored panel, in logical px —
/// [`MaterialSpacing::XS`] (4dp), the Material spacing token the source's own
/// `sideOffset` of `4` lands on exactly.
pub const OVERLAY_ANCHOR_GAP: f64 = MaterialSpacing::XS;

/// The scale an anchored panel's entrance starts from (the source's
/// `zoom-in-95`, carried over verbatim — a visual constant with no Material
/// token to resolve against).
pub const ANCHORED_ENTER_SCALE: f64 = 0.95;

/// Progress difference below which a ramp counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

/// How far outside its own bounds a panel's chrome (an elevation shadow) may
/// reach, in logical px — the bound the fade layer is inflated by so a shadow
/// is not clipped out of it.
const SHADOW_SPILL: f64 = 48.0;

impl Default for OverlayPlacement {
    fn default() -> Self {
        OverlayPlacement {
            side: OverlaySide::default(),
            align: OverlayAlign::default(),
            offset: OVERLAY_ANCHOR_GAP,
            flip: true,
            clamp: true,
        }
    }
}

impl OverlayPlacement {
    /// A placement on `side`, everything else defaulted.
    pub fn on(side: OverlaySide) -> Self {
        OverlayPlacement {
            side,
            ..Self::default()
        }
    }

    /// Set the cross-axis alignment.
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.align = align;
        self
    }

    /// Set the anchor gap, in logical px.
    pub fn offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    /// Enable or disable the collision flip.
    pub fn flip(mut self, flip: bool) -> Self {
        self.flip = flip;
        self
    }

    /// Enable or disable the shift-back-inside clamp.
    pub fn clamp(mut self, clamp: bool) -> Self {
        self.clamp = clamp;
        self
    }
}

/// Whether a `content`-sized overlay fits on `side` of `anchor` inside `area`.
fn fits(side: OverlaySide, anchor: Rect, content: Size, area: Rect, offset: f64) -> bool {
    match side {
        OverlaySide::Top => anchor.y0 - offset - content.height >= area.y0,
        OverlaySide::Bottom => anchor.y1 + offset + content.height <= area.y1,
        OverlaySide::Left => anchor.x0 - offset - content.width >= area.x0,
        OverlaySide::Right => anchor.x1 + offset + content.width <= area.x1,
    }
}

/// The cross-axis start coordinate for `align`, given the anchor's own span
/// `[a0, a1]` and the overlay's `extent` along that axis.
fn align_start(align: OverlayAlign, a0: f64, a1: f64, extent: f64) -> f64 {
    match align {
        OverlayAlign::Start => a0,
        OverlayAlign::Center => (a0 + a1) / 2.0 - extent / 2.0,
        OverlayAlign::End => a1 - extent,
    }
}

/// Shift `rect` back inside `area`, keeping its size.
///
/// The start edge wins when the overlay is larger than the area (`min` before
/// `max`): a too-wide panel hangs off the trailing edge rather than the leading
/// one, which is where its content starts.
fn clamp_into(rect: Rect, area: Rect) -> Rect {
    let x = rect.x0.min(area.x1 - rect.width()).max(area.x0);
    let y = rect.y0.min(area.y1 - rect.height()).max(area.y0);
    Rect::from_origin_size(Point::new(x, y), rect.size())
}

/// Place a `content`-sized overlay against `anchor` inside `area`.
///
/// All three rects are in one coordinate space (the host's own; see the [module
/// docs](self)). The returned rect is the overlay's placed bounds:
///
/// 1. pick the side — the preferred one, or its opposite when `flip` is on, the
///    preferred one does not fit and the opposite does;
/// 2. offset off that edge of the anchor by `offset`, lined up on the cross
///    axis per `align`;
/// 3. shift back inside `area` when `clamp` is on.
///
/// Pure and total: it allocates nothing, reads no context, and is defined for a
/// degenerate anchor (a zero-size rect places against that point) and for
/// content larger than the area (which pins to the area's *start* edge, where
/// the content begins, rather than its trailing one).
pub fn place_anchored(
    anchor: Rect,
    content: Size,
    area: Rect,
    placement: OverlayPlacement,
) -> Rect {
    let mut side = placement.side;
    if placement.flip
        && !fits(side, anchor, content, area, placement.offset)
        && fits(side.opposite(), anchor, content, area, placement.offset)
    {
        side = side.opposite();
    }
    let origin = match side {
        OverlaySide::Top => Point::new(
            align_start(placement.align, anchor.x0, anchor.x1, content.width),
            anchor.y0 - placement.offset - content.height,
        ),
        OverlaySide::Bottom => Point::new(
            align_start(placement.align, anchor.x0, anchor.x1, content.width),
            anchor.y1 + placement.offset,
        ),
        OverlaySide::Left => Point::new(
            anchor.x0 - placement.offset - content.width,
            align_start(placement.align, anchor.y0, anchor.y1, content.height),
        ),
        OverlaySide::Right => Point::new(
            anchor.x1 + placement.offset,
            align_start(placement.align, anchor.y0, anchor.y1, content.height),
        ),
    };
    let rect = Rect::from_origin_size(origin, content);
    if placement.clamp {
        clamp_into(rect, area)
    } else {
        rect
    }
}

/// A shared cell holding a trigger's window-space rect: written by
/// [`overlay_anchor`] on every paint of the wrapped trigger, read by
/// [`anchored_overlay`] on every layout.
///
/// Clone it — the clone shares the same cell. An app keeps one per anchored
/// overlay in its `Component::State`, hands a clone to the trigger and another
/// to the host.
///
/// **Not reactive.** Writing a rect wakes no frame; it is read during the
/// layout pass the same frame anything that moved the trigger already
/// scheduled, and the host's paint arm requests a relayout when it sees the
/// rect change under it. An overlay whose trigger moves with nothing else
/// repainting is therefore one frame behind, never permanently stale.
#[derive(Clone, Debug, Default)]
pub struct OverlayAnchor(Rc<std::cell::Cell<Rect>>);

impl OverlayAnchor {
    /// A fresh anchor, holding the degenerate rect at the window origin until a
    /// trigger paints (see [`OverlayAnchor::rect`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// The captured trigger rect, in window space.
    ///
    /// Before the wrapped trigger's first paint this is `Rect::ZERO` — a
    /// zero-size anchor at the window origin, which places an overlay at the
    /// top-left. In practice a trigger always paints before it can be pressed,
    /// so an overlay opened by that press already has a real rect.
    pub fn rect(&self) -> Rect {
        self.0.get()
    }

    /// Overwrite the captured rect — for an app anchoring against something it
    /// positions itself (a caret, a canvas hit point) rather than a widget.
    pub fn set(&self, rect: Rect) {
        self.0.set(rect);
    }
}

/// Wrap `child` so its window-space rect is captured into `anchor` on every
/// paint — the trigger half of an anchored overlay.
///
/// Transparent in every other respect: it lays out, paints, routes events to
/// and publishes the semantics of `child` unchanged, and paints nothing of its
/// own.
pub fn overlay_anchor<State: 'static, V: View<State>>(
    anchor: &OverlayAnchor,
    child: V,
) -> OverlayAnchorView<State> {
    OverlayAnchorView {
        child: any(child),
        anchor: anchor.clone(),
    }
}

/// A declarative anchor-capturing wrapper. See [`overlay_anchor`].
pub struct OverlayAnchorView<State: 'static> {
    child: AnyView<State>,
    anchor: OverlayAnchor,
}

/// The retained widget for an [`OverlayAnchorView`].
pub struct OverlayAnchorWidget {
    child: ChildPod,
    anchor: OverlayAnchor,
}

impl<State: 'static> View<State> for OverlayAnchorView<State> {
    type Element = OverlayAnchorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayAnchorWidget {
        OverlayAnchorWidget {
            child: build_child(&self.child, ctx),
            anchor: self.anchor.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayAnchorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.anchor = self.anchor.clone();
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut OverlayAnchorWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for OverlayAnchorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::origin` is absolute window space — the one read that can
        // answer "where is this trigger on screen", which is what an anchored
        // overlay needs.
        self.anchor
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// Build an anchored-overlay host showing `content`. See the [module
/// docs](self) for the mounting, motion, coordinate-space and dismissal
/// contracts.
///
/// Chain [`AnchoredOverlayView::anchor`] to point it at a trigger,
/// [`AnchoredOverlayView::placement`] to pick the side/align/offset,
/// [`AnchoredOverlayView::open`] for the kept-mounted exit ramp, and
/// [`AnchoredOverlayView::on_dismiss`] to handle a light dismiss or Escape.
pub fn anchored_overlay<State: 'static, V: View<State>>(content: V) -> AnchoredOverlayView<State> {
    AnchoredOverlayView {
        content: any(content),
        anchor: OverlayAnchor::new(),
        placement: OverlayPlacement::default(),
        open: true,
        on_dismiss: None,
    }
}

/// A declarative anchored-overlay host. See [`anchored_overlay`].
pub struct AnchoredOverlayView<State: 'static> {
    content: AnyView<State>,
    anchor: OverlayAnchor,
    placement: OverlayPlacement,
    open: bool,
    on_dismiss: Option<OnDismiss<State>>,
}

impl<State: 'static> AnchoredOverlayView<State> {
    /// Anchor the overlay to the rect `anchor` carries (see [`OverlayAnchor`]).
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Set the placement (side, align, offset, flip/clamp).
    pub fn placement(mut self, placement: OverlayPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Tell a kept-mounted host whether it is open (see the [module
    /// docs](self)). The default is `true`, which is the mount-on-open
    /// contract: a mounted host is an open one.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the dismiss callback: a press outside the placed content, or Escape
    /// once the host holds focus.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// The retained widget for an [`AnchoredOverlayView`].
pub struct AnchoredOverlayWidget {
    content: ChildPod,
    anchor: OverlayAnchor,
    placement: OverlayPlacement,
    /// Whether the overlay is open. `false` makes the host inert without
    /// unmounting it — the kept-mounted pattern the module docs describe.
    open: bool,
    on_dismiss: Option<ErasedCallback>,
    /// The placed content rect, in this host's own coordinate space.
    rect: Rect,
    /// The anchor rect the last layout placed against (window space) — compared
    /// at paint so a trigger that moved forces a relayout.
    anchor_used: Rect,
    /// This host's own absolute origin, learned at paint (see the module docs'
    /// coordinate-space note).
    host_origin: Point,
    /// The ramp driver, the endpoints it interpolates between, and the progress
    /// the last paint resolved: `progress = from + (to − from) · anim.value()`.
    anim: AnimationController,
    ramp: (f64, f64),
    progress: f64,
    /// Whether a paint has seeded the entrance yet (see the module docs'
    /// `reduce_motion` note).
    started: bool,
}

impl AnchoredOverlayWidget {
    /// The placed content rect in the host's own coordinate space — what the
    /// light-dismiss hit test compares against.
    ///
    /// Placement runs whether the host is open or not, so this stays truthful
    /// while a kept-mounted overlay plays its exit ramp.
    pub fn content_rect(&self) -> Rect {
        self.rect
    }

    /// Whether the host is open (see [`AnchoredOverlayView::open`]).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The ramp's progress: `0.0` gone, `1.0` fully shown.
    pub fn progress(&self) -> f64 {
        self.progress
    }

    /// Whether the content is still visually present — open, or closed with an
    /// exit ramp still running.
    fn visible(&self) -> bool {
        self.open || self.progress > PROGRESS_EPSILON
    }

    /// Start a ramp from the current progress to `to`, over the matching
    /// direction's own duration scaled by how much of the travel is left (a
    /// half-open panel closes in half the time) and eased by its own curve.
    fn begin_ramp(&mut self, to: f64) {
        let from = self.progress;
        self.ramp = (from, to);
        let entering = to > from;
        let fraction = (to - from).abs().clamp(0.0, 1.0);
        let base = if entering {
            MaterialMotion::SHORT_4
        } else {
            MaterialMotion::SHORT_3
        };
        let curve = if entering {
            MaterialMotion::EMPHASIZED_DECELERATE
        } else {
            MaterialMotion::STANDARD_ACCELERATE
        };
        self.anim = AnimationController::new(base.mul_f64(fraction)).with_curve(curve);
        self.anim.forward();
        self.started = true;
    }

    /// The progress the running ramp is at.
    fn ramp_value(&self) -> f64 {
        let (from, to) = self.ramp;
        from + (to - from) * self.anim.value_clamped()
    }

    /// Advance (or collapse) the running ramp for this paint, requesting the
    /// frames it still needs.
    ///
    /// `reduce` snaps straight to the ramp's *target*, which is why the target
    /// is seeded from `open` at build (`0.0` for a host mounted closed) rather
    /// than left at the entrance's `1.0`: a reduced-motion pass takes the
    /// endpoint as-is, and a closed host must land on nothing.
    fn advance(&mut self, ctx: &mut PaintCtx, reduce: bool) {
        if !self.started {
            self.started = true;
            if self.open && !reduce {
                self.anim.forward();
            }
        }
        let next = if reduce {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.ramp.1
        } else if self.anim.is_animating() {
            // `advance`'s first call after `forward()` only seeds the clock
            // (zero delta, but still truthy) — the continuation has to be
            // requested on every truthy advance, not just the ones that moved
            // `progress`, or the seeding paint never schedules the frame that
            // would carry it off zero.
            if self.anim.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
            self.ramp_value()
        } else {
            self.progress
        };
        if (next - self.progress).abs() > PROGRESS_EPSILON {
            self.progress = next;
            ctx.request_frame();
        }
    }

    /// The point the entrance scale pivots about, in this host's own space: the
    /// anchor's centre clamped into the placed content rect, i.e. the content's
    /// own edge or corner facing the trigger — so a panel grows out of the
    /// control that opened it.
    fn pivot(&self) -> Point {
        let anchor_local = self.anchor_used - self.host_origin.to_vec2();
        let center = anchor_local.center();
        Point::new(
            center.x.clamp(self.rect.x0, self.rect.x1),
            center.y.clamp(self.rect.y0, self.rect.y1),
        )
    }
}

impl<State: 'static> View<State> for AnchoredOverlayView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        AnchoredOverlayWidget {
            content: build_child(&self.content, ctx),
            anchor: self.anchor.clone(),
            placement: self.placement,
            open: self.open,
            on_dismiss: self.on_dismiss.as_ref().map(erase_callback),
            rect: Rect::ZERO,
            anchor_used: Rect::ZERO,
            host_origin: Point::ORIGIN,
            anim: AnimationController::new(MaterialMotion::SHORT_4)
                .with_curve(MaterialMotion::EMPHASIZED_DECELERATE),
            // The endpoint a reduced-motion pass snaps to — see `advance`.
            ramp: (0.0, if self.open { 1.0 } else { 0.0 }),
            progress: 0.0,
            started: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.placement != self.placement {
            element.placement = self.placement;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            // The paint is what runs the ramp this starts.
            element.begin_ramp(if self.open { 1.0 } else { 0.0 });
            flags |= ChangeFlags::PAINT;
        }
        element.anchor = self.anchor.clone();
        // Closures aren't comparable, so the dismiss adapter is reinstalled
        // unconditionally — cheap, and what every interactive widget does.
        element.on_dismiss = self.on_dismiss.as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for AnchoredOverlayWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let content = self.content.layout_child(ctx, &BoxConstraints::loose(area));
        self.anchor_used = self.anchor.rect();
        // Window space → this host's own space (the identity in the supported
        // mounts; see the module docs).
        let anchor_local = self.anchor_used - self.host_origin.to_vec2();
        self.rect = place_anchored(
            anchor_local,
            content,
            Rect::from_origin_size(Point::ORIGIN, area),
            self.placement,
        );
        self.content.set_origin(self.rect.origin());
        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Self-correction, the paint-time mirror of the layout above: this is
        // the only pass that knows the host's absolute origin, and the anchor
        // cell can move under a layout that already ran.
        if ctx.origin() != self.host_origin || self.anchor.rect() != self.anchor_used {
            self.host_origin = ctx.origin();
            ctx.request_layout();
        }
        let reduce = reduce_motion(Theme::from_paint_ctx(ctx));
        self.advance(ctx, reduce);
        // A closed host whose exit ramp has settled paints *nothing at all* —
        // the kept-mounted contract's whole point.
        if !self.visible() {
            self.progress = 0.0;
            return;
        }

        // The host has no chrome of its own — no scrim, no panel. Everything
        // visible is the content's; the host only composites it.
        let ramping = self.progress < 1.0 - PROGRESS_EPSILON;
        if ramping {
            let origin = ctx.origin();
            let placed = self.rect + origin.to_vec2();
            // The layer is inflated so a panel's own elevation shadow, which
            // reaches outside its box, is not clipped out of the fade.
            let layer = placed.inflate(SHADOW_SPILL, SHADOW_SPILL);
            scene.push_layer(layer.origin(), layer.size(), self.progress as f32);
            let pivot = self.pivot() + origin.to_vec2();
            let scale = ANCHORED_ENTER_SCALE + (1.0 - ANCHORED_ENTER_SCALE) * self.progress;
            scene.push_transform(
                Affine::translate(pivot.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-pivot.to_vec2()),
            );
        }
        self.content.paint_child(ctx, scene);
        if ramping {
            scene.pop_transform();
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A closed (or closing) kept-mounted host claims no focus and dismisses
        // on nothing: light dismiss and Escape are off for good the moment
        // `open` flips, and no press claims hover. A broadcast still reaches the
        // content, which is what keeps its pods live. A `Down` inside the still-
        // visible content is absorbed here — never forwarded, since the app has
        // already dismissed whatever it would activate, and never allowed to
        // fall through to the page underneath (the module docs' exit-ramp
        // swallow). Move, Up, Cancel and Key stay input-transparent.
        if !self.open {
            if event.is_broadcast() {
                self.content.event_child(ctx, event);
                return EventResult::Ignored;
            }
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
                && self.progress > PROGRESS_EPSILON
                && self.rect.contains(p.position)
            {
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        // Claim focus on every `Down`, before the routing below — the
        // `crate::dialog`/`crate::sheet` opt-in that makes Escape reachable at
        // all, and claimed even when the content consumes the press (a control
        // inside the panel claiming focus of its own simply wins the descent;
        // both pods sit on the same chain). Re-claiming while already focused is
        // a no-op — a claim-once guard would drop the root's focus session on
        // the second press.
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        // Content first, always: a broadcast reaches it unconsumed, a focused
        // child takes the key events, and a press inside it is its own. The host
        // claims nothing before this (the claim-ordering rule).
        if route_event_single(&mut self.content, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape)
                && let Some(on_dismiss) = self.on_dismiss.as_mut()
            {
                on_dismiss(ctx);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // Deliberately button-agnostic, unlike every press machine in
                // the catalog: a light dismiss is not a press. A right-click
                // outside an open menu closes it exactly as a left-click does,
                // and the host captures nothing here — it only fires the app's
                // dismiss callback — so the primary-only press rule has nothing
                // to protect.
                if !self.rect.contains(p.position)
                    && let Some(on_dismiss) = self.on_dismiss.as_mut()
                {
                    on_dismiss(ctx);
                }
                // Consumed either way: dismissing swallows the press, and a
                // press on the content's own background is the panel's.
                EventResult::Handled
            }
            // Everything else passes through, so a trigger under the host keeps
            // seeing its own hover/scroll (the module docs' pass-through note).
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The host contributes no node of its own — the content (a menu, a
        // tooltip) carries the role worth reporting. A closed host reports
        // nothing: a panel on its way out is not there to be read.
        if self.open {
            self.content.semantics_child(ctx);
        }
    }

    visit_children!(content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        Color, KeyEvent, Modifiers, PointerButton, PointerEvent, text::TextContext,
    };
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 600.0);
    const WINDOW: Size = Size::new(400.0, 600.0);
    const CONTENT: Size = Size::new(120.0, 60.0);
    const ANCHOR: Rect = Rect::new(100.0, 200.0, 180.0, 240.0);

    fn anchor_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::from_origin_size(Point::new(x, y), Size::new(w, h))
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    // ---- placement math ---------------------------------------------------

    #[test]
    fn the_default_placement_is_bottom_center_at_the_material_gap() {
        let p = OverlayPlacement::default();
        assert_eq!(p.side, OverlaySide::Bottom);
        assert_eq!(p.align, OverlayAlign::Center);
        assert_eq!(p.offset, OVERLAY_ANCHOR_GAP);
        assert_eq!(OVERLAY_ANCHOR_GAP, MaterialSpacing::XS);
        assert!(p.flip && p.clamp);
    }

    #[test]
    fn each_side_offsets_off_its_own_edge() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        // The raw side placement, with the collision passes off: `left` would
        // otherwise flip (a 120px panel does not fit in the 100px to the
        // anchor's left), which the flip test below covers on its own.
        let raw = |side| OverlayPlacement::on(side).flip(false).clamp(false);
        let bottom = place_anchored(a, content, AREA, raw(OverlaySide::Bottom));
        assert_eq!(bottom.y0, a.y1 + OVERLAY_ANCHOR_GAP);
        let top = place_anchored(a, content, AREA, raw(OverlaySide::Top));
        assert_eq!(top.y1, a.y0 - OVERLAY_ANCHOR_GAP);
        let right = place_anchored(a, content, AREA, raw(OverlaySide::Right));
        assert_eq!(right.x0, a.x1 + OVERLAY_ANCHOR_GAP);
        let left = place_anchored(a, content, AREA, raw(OverlaySide::Left));
        assert_eq!(left.x1, a.x0 - OVERLAY_ANCHOR_GAP);
        // Sizes are never altered by placement.
        for r in [bottom, top, right, left] {
            assert_eq!(r.size(), content);
        }
    }

    #[test]
    fn align_lines_up_leading_center_or_trailing_edges() {
        let a = anchor_rect(100.0, 200.0, 80.0, 40.0);
        let content = Size::new(120.0, 60.0);
        let at = |align| {
            place_anchored(
                a,
                content,
                AREA,
                OverlayPlacement::on(OverlaySide::Bottom).align(align),
            )
        };
        assert_eq!(at(OverlayAlign::Start).x0, a.x0);
        assert_eq!(at(OverlayAlign::End).x1, a.x1);
        assert_eq!(at(OverlayAlign::Center).center().x, a.center().x);

        // The same three on a horizontal side act on the vertical axis.
        let at = |align| {
            place_anchored(
                a,
                content,
                AREA,
                OverlayPlacement::on(OverlaySide::Right).align(align),
            )
        };
        assert_eq!(at(OverlayAlign::Start).y0, a.y0);
        assert_eq!(at(OverlayAlign::End).y1, a.y1);
        assert_eq!(at(OverlayAlign::Center).center().y, a.center().y);
    }

    #[test]
    fn a_side_that_does_not_fit_flips_to_the_opposite_one() {
        // An anchor near the bottom edge: `bottom` overflows, `top` fits.
        let a = anchor_rect(100.0, 560.0, 80.0, 20.0);
        let content = Size::new(120.0, 100.0);
        let flipped = place_anchored(a, content, AREA, OverlayPlacement::default());
        assert_eq!(
            flipped.y1,
            a.y0 - OVERLAY_ANCHOR_GAP,
            "flipped above the anchor"
        );

        // With the flip disabled it stays below and only the clamp moves it.
        let pinned = place_anchored(a, content, AREA, OverlayPlacement::default().flip(false));
        assert_eq!(pinned.y1, AREA.y1, "clamped, not flipped");

        // Neither side fits: the preferred one is kept (and clamped).
        let tall = Size::new(120.0, 590.0);
        let kept = place_anchored(a, tall, AREA, OverlayPlacement::default());
        assert_eq!(kept.y1, AREA.y1);

        assert_eq!(OverlaySide::Left.opposite(), OverlaySide::Right);
        assert!(OverlaySide::Top.is_vertical() && !OverlaySide::Right.is_vertical());
    }

    #[test]
    fn clamping_shifts_the_rect_back_inside_and_can_be_turned_off() {
        // An anchor at the right edge, centre-aligned: the panel overflows.
        let a = anchor_rect(380.0, 100.0, 20.0, 20.0);
        let content = Size::new(200.0, 50.0);
        let clamped = place_anchored(a, content, AREA, OverlayPlacement::default());
        assert_eq!(clamped.x1, AREA.x1);
        assert_eq!(
            clamped.y0,
            a.y1 + OVERLAY_ANCHOR_GAP,
            "only the cross axis moved"
        );

        let free = place_anchored(a, content, AREA, OverlayPlacement::default().clamp(false));
        assert!(free.x1 > AREA.x1, "unclamped placement may overflow");

        // Content wider than the area pins to the leading edge (the start-edge
        // rule), not the trailing one.
        let huge = Size::new(600.0, 50.0);
        let pinned = place_anchored(a, huge, AREA, OverlayPlacement::default());
        assert_eq!(pinned.x0, AREA.x0);
    }

    #[test]
    fn an_uncaptured_anchor_places_at_the_area_origin() {
        let content = Size::new(80.0, 40.0);
        let rect = place_anchored(Rect::ZERO, content, AREA, OverlayPlacement::default());
        assert_eq!(rect.x0, AREA.x0, "clamped in from the centred overhang");
        assert_eq!(rect.y0, OVERLAY_ANCHOR_GAP);
    }

    // ---- the host ---------------------------------------------------------

    #[derive(Default)]
    struct AppState {
        dismissed: u32,
        pressed: u32,
    }

    /// A fixed-size leaf that reports every press into the app state — enough to
    /// prove a press inside the content reached it rather than the host.
    struct Panel(Size);

    /// The retained half of [`Panel`].
    struct PanelWidget(Size);

    impl View<AppState> for Panel {
        type Element = PanelWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PanelWidget {
            PanelWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut PanelWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for PanelWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            if p.phase == PointerPhase::Down {
                ctx.state_mut::<AppState>().pressed += 1;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size)>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {}
    }

    /// The host mounted the way an app mounts it: as the top child of a
    /// full-area [`frust::Stack`] (see the module docs). The `Stack` is
    /// load-bearing for the Escape tests — it is the container that focus-routes
    /// a key event to whichever child holds the focus path, which is what the
    /// host's own focus claim buys.
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        clock_ms: f64,
    }

    impl Harness {
        fn new() -> Self {
            let anchor = OverlayAnchor::new();
            anchor.set(ANCHOR);
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor,
                clock_ms: 0.0,
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let mut logic = move |_s: &mut AppState| {
                frust::stack().child(
                    anchored_overlay(Panel(CONTENT))
                        .anchor(&anchor)
                        .on_dismiss(|s: &mut AppState| s.dismissed += 1),
                )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        /// Paint one frame, advancing the harness clock past any ramp.
        fn frame(&mut self, step_ms: f64) -> Recorder {
            self.clock_ms += step_ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(self.clock_ms));
            rec
        }

        /// Run the entrance ramp to rest.
        fn settle(&mut self) {
            self.frame(0.0);
            self.frame(1000.0);
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventResult {
            let outcome = self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
            if outcome.handled {
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }

        fn escape(&mut self) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key: Key::Named(NamedKey::Escape),
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    /// A standalone host widget plus the anchor cell it reads, for the ramp
    /// tests (which drive `paint` directly with synthetic frame times).
    fn host(open: bool) -> (AnchoredOverlayWidget, OverlayAnchor) {
        let cell = OverlayAnchor::new();
        cell.set(ANCHOR);
        let view = anchored_overlay(Panel(CONTENT)).anchor(&cell).open(open);
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        (w, cell)
    }

    fn paint_at(w: &mut AnchoredOverlayWidget, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    /// Close a kept-mounted host exactly as `View::rebuild` does when the app
    /// flips `open` — the flag *and* the exit ramp it starts.
    fn close(w: &mut AnchoredOverlayWidget) {
        w.open = false;
        w.begin_ramp(0.0);
    }

    fn dispatch(
        w: &mut AnchoredOverlayWidget,
        state: &mut AppState,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    #[test]
    fn the_host_fills_its_area_and_places_the_content_against_the_anchor() {
        let mut h = Harness::new();
        let rect = place_anchored(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        // Bottom/center of the anchor, `OVERLAY_ANCHOR_GAP` below it.
        assert_eq!(rect.y0, ANCHOR.y1 + OVERLAY_ANCHOR_GAP);
        assert_eq!(rect.center().x, ANCHOR.center().x);
        // Nothing painted by the host itself; the content lands at the placement.
        h.settle();
        let rec = h.frame(16.0);
        assert_eq!(rec.rects, vec![(rect.origin(), CONTENT)]);
        assert!(rec.layers.is_empty(), "a settled host composites plainly");
    }

    #[test]
    fn an_unbounded_height_collapses_the_host_area_and_pins_the_content_to_its_top() {
        // The documented mounts (a navigator page, a full-area `Stack`) are
        // bounded; a host put inside a scroll view is not, and this is what
        // that costs — pinned, not fixed here: the coercion cannot invent an
        // extent nobody offered, so the fix belongs at the mount site.
        let cell = OverlayAnchor::new();
        cell.set(ANCHOR);
        let view = anchored_overlay(Panel(CONTENT)).anchor(&cell);
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        // The constraints `frust::scroll_view` hands its child: the viewport
        // width, an infinite max height.
        let scrolled = BoxConstraints::new(
            Size::new(WINDOW.width, 0.0),
            Size::new(WINDOW.width, f64::INFINITY),
        );
        let size = w.layout(&mut lctx, &scrolled);
        assert_eq!(size.height, 0.0, "no vertical area to fill");
        assert_eq!(
            w.content_rect().y0,
            0.0,
            "the panel clamps to the top of a zero-height area, wherever its \
             anchor sits"
        );

        // The same host under the contract's own constraints places normally.
        let bounded = w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        assert_eq!(bounded, WINDOW);
        assert_eq!(w.content_rect().y0, ANCHOR.y1 + OVERLAY_ANCHOR_GAP);
    }

    #[test]
    fn a_press_outside_the_content_dismisses_and_is_swallowed() {
        let mut h = Harness::new();
        h.settle();
        assert_eq!(
            h.pointer(PointerPhase::Down, 5.0, 5.0),
            EventResult::Handled,
            "the light dismiss consumes the press"
        );
        assert_eq!(h.state.dismissed, 1);
        assert_eq!(h.state.pressed, 0, "the content never saw it");
    }

    #[test]
    fn a_press_inside_the_content_routes_to_it_and_never_dismisses() {
        let mut h = Harness::new();
        h.settle();
        let rect = place_anchored(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        let c = rect.center();
        assert_eq!(
            h.pointer(PointerPhase::Down, c.x, c.y),
            EventResult::Handled
        );
        assert_eq!(h.state.pressed, 1);
        assert_eq!(h.state.dismissed, 0);
    }

    #[test]
    fn moves_outside_the_content_pass_through_untouched() {
        let mut h = Harness::new();
        h.settle();
        assert_eq!(
            h.pointer(PointerPhase::Move, 5.0, 5.0),
            EventResult::Ignored,
            "a hover-driven trigger under the host keeps its own moves"
        );
        assert_eq!(h.state.dismissed, 0);
    }

    #[test]
    fn escape_dismisses_once_a_press_has_focused_the_host() {
        let mut h = Harness::new();
        h.settle();
        h.escape();
        assert_eq!(
            h.state.dismissed, 0,
            "no focus yet, so Escape has no chain to travel"
        );
        // A press on the content focuses the host without dismissing it.
        let rect = place_anchored(ANCHOR, CONTENT, AREA, OverlayPlacement::default());
        let c = rect.center();
        h.pointer(PointerPhase::Down, c.x, c.y);
        h.pointer(PointerPhase::Up, c.x, c.y);
        h.escape();
        assert_eq!(h.state.dismissed, 1);
    }

    #[test]
    fn a_moved_anchor_is_re_placed_on_the_next_layout() {
        let mut h = Harness::new();
        h.settle();
        h.anchor.set(anchor_rect(20.0, 20.0, 40.0, 20.0));
        h.pass();
        let rec = h.frame(16.0);
        let expected = place_anchored(h.anchor.rect(), CONTENT, AREA, OverlayPlacement::default());
        assert_eq!(rec.rects[0].0, expected.origin());
    }

    // ---- the ramp and the kept-mounted contract ---------------------------

    #[test]
    fn the_entrance_ramps_alpha_and_scale_then_settles() {
        let (mut w, _cell) = host(true);
        let first = paint_at(&mut w, 0.0);
        assert_eq!(first.layers, vec![0.0], "starts transparent");
        assert_eq!(first.transforms.len(), 1, "and scaled down");
        let mid = paint_at(&mut w, 100.0);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "mid-ramp: {alpha}");
        let done = paint_at(&mut w, 1000.0);
        assert!(
            done.layers.is_empty() && done.transforms.is_empty(),
            "a settled host composites plainly"
        );
        assert!((w.progress() - 1.0).abs() < 1e-9);
        assert_eq!(done.rects.len(), 1, "and still paints its content");
    }

    #[test]
    fn the_entrance_scale_pivots_on_the_edge_facing_the_trigger() {
        let (mut w, _cell) = host(true);
        paint_at(&mut w, 0.0);
        // The panel sits below the anchor, so it grows out of its own top edge,
        // horizontally centred on the anchor.
        let pivot = w.pivot();
        assert_eq!(pivot.y, w.content_rect().y0);
        assert!((pivot.x - ANCHOR.center().x).abs() < 1e-9);
    }

    #[test]
    fn a_closed_kept_mounted_host_paints_nothing_once_the_exit_settles() {
        let (mut w, _cell) = host(true);
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 1000.0);
        assert!((w.progress() - 1.0).abs() < 1e-9, "entered");

        // The kept-mounted close: `open` flips, the host stays mounted.
        close(&mut w);
        let mid = paint_at(&mut w, 1000.0);
        assert_eq!(mid.rects.len(), 1, "still painted mid-exit");
        let closing = paint_at(&mut w, 1075.0);
        assert!(w.progress() > 0.0 && w.progress() < 1.0, "{}", w.progress());
        assert_eq!(closing.rects.len(), 1, "and still visible");

        let settled = paint_at(&mut w, 2000.0);
        assert_eq!(w.progress(), 0.0);
        assert!(
            settled.rects.is_empty() && settled.layers.is_empty(),
            "a settled closed host paints nothing at all"
        );
        // …and stays silent on every later frame.
        assert!(paint_at(&mut w, 3000.0).rects.is_empty());
    }

    #[test]
    fn a_press_is_swallowed_mid_exit_and_passes_through_once_settled() {
        let (mut w, _cell) = host(true);
        paint_at(&mut w, 0.0);
        paint_at(&mut w, 1000.0);
        close(&mut w);
        paint_at(&mut w, 1000.0);
        paint_at(&mut w, 1075.0);
        assert!(w.progress() > PROGRESS_EPSILON, "mid-exit");

        let mut state = AppState::default();
        let inside = w.content_rect().center();
        let outside = Point::new(5.0, 5.0);
        assert_eq!(
            dispatch(&mut w, &mut state, &down(inside)),
            EventResult::Handled,
            "the fading panel absorbs the press"
        );
        assert_eq!(state.pressed, 0, "and never forwards it to the content");
        assert_eq!(state.dismissed, 0);
        assert_eq!(
            dispatch(&mut w, &mut state, &down(outside)),
            EventResult::Ignored,
            "a press outside it falls through to the page"
        );

        // Settled: even a press on the (now unpainted) content rect passes
        // through.
        paint_at(&mut w, 3000.0);
        assert_eq!(
            dispatch(&mut w, &mut state, &down(inside)),
            EventResult::Ignored
        );
    }

    fn down(p: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: p,
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn a_host_mounted_closed_never_paints_and_publishes_no_semantics() {
        let (mut w, _cell) = host(false);
        let rec = paint_at(&mut w, 0.0);
        assert!(rec.rects.is_empty() && rec.layers.is_empty());
        assert!(!w.is_open());
        assert_eq!(w.progress(), 0.0);
        // Its placement is still truthful — the content laid out.
        assert_eq!(w.content_rect().size(), CONTENT);
    }

    #[test]
    fn reduce_motion_collapses_the_ramp_to_a_jump() {
        let mut theme = crate::baseline().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let paint_reduced = |w: &mut AnchoredOverlayWidget| {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, FrameTime::ZERO)
                .with_theme(&theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
            rec
        };
        let (mut w, _cell) = host(true);
        let rec = paint_reduced(&mut w);
        assert!((w.progress() - 1.0).abs() < 1e-9, "settled on the spot");
        assert!(rec.layers.is_empty(), "no fade layer");
        assert_eq!(rec.rects.len(), 1, "the content is there whole");

        // …and the closed direction lands on nothing rather than on the
        // entrance's endpoint: a reduced-motion pass takes the ramp's target
        // as-is, so a host mounted closed must carry `0.0` as that target.
        let (mut closed_host, _cell) = host(false);
        let rec = paint_reduced(&mut closed_host);
        assert_eq!(closed_host.progress(), 0.0);
        assert!(
            rec.rects.is_empty() && rec.layers.is_empty(),
            "a closed host paints nothing under reduced motion either"
        );
    }

    #[test]
    fn semantics_are_published_only_while_open() {
        fn logic(_s: &mut AppState) -> AnchoredOverlayView<AppState> {
            anchored_overlay(frust::text("menu item")).open(true)
        }
        fn closed(_s: &mut AppState) -> AnchoredOverlayView<AppState> {
            anchored_overlay(frust::text("menu item")).open(false)
        }
        let mut root: RenderRoot<AppState, AnchoredOverlayView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let open_nodes = root.semantics().nodes.len();
        root.rebuild(&mut closed, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        assert!(
            root.semantics().nodes.len() < open_nodes,
            "a closed host contributes no subtree"
        );
    }

    // ---- the trigger-side capture -----------------------------------------

    #[test]
    fn the_anchor_wrapper_captures_the_triggers_window_rect_at_paint() {
        let cell = OverlayAnchor::new();
        assert_eq!(cell.rect(), Rect::ZERO, "nothing captured before a paint");

        let mut root: RenderRoot<AppState, frust::PaddingView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let captured = cell.clone();
        // Offset by a padding so the captured rect proves it is absolute, not
        // parent-relative.
        let mut logic = move |_s: &mut AppState| {
            frust::Padding(
                frust::EdgeInsets::symmetric(24.0, 12.0),
                overlay_anchor(&captured, Panel(CONTENT)),
            )
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), FrameTime::ZERO);

        assert_eq!(
            cell.rect(),
            Rect::from_origin_size(Point::new(24.0, 12.0), CONTENT)
        );
        // The wrapper itself paints nothing of its own.
        cell.set(Rect::ZERO);
        assert_eq!(cell.rect(), Rect::ZERO, "the cell is writable by hand too");
    }
}
