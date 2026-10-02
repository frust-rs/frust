//! The `PanZoomView`/`PanZoomWidget` container: one child laid out at its
//! intrinsic size and placed under a scale-then-translate transform the user
//! drives — drag to pan, pinch (touch) or ctrl/⌘+wheel and trackpad pinch
//! (desktop) to zoom.
//!
//! The child sits in a [`ChildPod`] whose
//! [`set_transform`](ChildPod::set_transform) is
//! `Affine::translate(offset) * Affine::scale(scale)` ([`PanZoomTransform`]),
//! so the framework paints it transformed and inverse-maps hit tests and every
//! positioned event it routes down — a child sees its own unscaled local
//! coordinates at any zoom, with no help from this widget.
//!
//! # Input
//!
//! - **Pan.** A primary `Down` is offered to the child first. If the child
//!   handles it (a draggable node, a button) the gesture is the child's and the
//!   view does not pan; if the child ignores it (empty canvas, a press outside
//!   the content) the claimant contact's moves translate the content. When the
//!   view is about to pan it also joins `scroll.rs`'s innermost-wins
//!   nested-scroll claim seam (the same one `ListView` uses), reporting itself
//!   into an enclosing `ScrollView`/`ListView`'s ambient claim cell so that
//!   surface defers to the pan past touch-slop instead of taking the gesture
//!   over — see [`PanZoomWidget::begin_gesture`].
//! - **Zoom.** An [`InputEvent::Scale`] — the desktop ctrl/⌘+wheel and
//!   trackpad-pinch mapping — is offered to the child first and applied here
//!   when the child ignores it. A touch pinch is recognised in-widget by a
//!   [`PinchRecognizer`] fed from the gesture's contacts (the view calls
//!   [`EventCtx::capture_contacts`] on every primary `Down`, so a second finger
//!   routes here even when the child owns the first). Both produce the same
//!   [`ScaleEvent`] stream and the same zoom: the content point under the
//!   focal stays under it, and while a bracketed gesture's focal moves the
//!   content follows it (two-finger pan). The scale clamps to
//!   [`min_scale`](PanZoomView::min_scale)/[`max_scale`](PanZoomView::max_scale).
//!   A pinch that begins over a child-owned gesture **steals** it: the child
//!   receives a synthesized `Cancel` on the claimant's next event (the
//!   [`crate::pinch`] wrapper's rule) and the rest of the gesture belongs to the
//!   view. When the child owns the gesture this view reports nothing into the
//!   nested-scroll claim above (the child's press, not a pan, owns it), so a
//!   second finger forming a pair here while nested in a `ScrollView`/`ListView`
//!   raises that surface's live multi-contact veto instead
//!   ([`crate::scroll::ambient_scroll_veto`], `crate::scroll`'s module docs'
//!   *Multi-contact veto*) — the same seam `pinch_detector` raises — so the
//!   enclosing surface does not steal the claimant's finger out from under the
//!   nascent pinch.
//! - **Wheel.** A plain [`InputEvent::Scroll`] (no modifier — the shell maps a
//!   modified wheel to `Scale` instead) is routed to the child, so a scrollable
//!   inside still scrolls; the view never pans on it.
//! - **Inertia.** With [`inertia(true)`](PanZoomView::inertia) a pan released
//!   faster than [`MIN_FLING_VELOCITY`] glides to rest on a per-axis
//!   [`FrictionSimulation`], pumped from the paint clock. Any new press, scale
//!   or controller command stops it.
//!
//! Panning is unbounded: the content may be dragged fully out of view (the
//! graph-editor convention); [`PanZoomController::fit_to_bounds`] brings it
//! back.
//!
//! # Notification and control
//!
//! [`on_transform`](PanZoomView::on_transform) runs with the new
//! [`PanZoomTransform`] after every change. A change made during the event pass
//! (a drag, a scale) notifies at once; one made where no `&mut State` exists (an
//! inertia frame, a controller command applied at layout/paint) is delivered by
//! the next [`InputEvent::Housekeeping`] flush, which the widget requests with
//! [`frust_core::mark_pending_result_flush`] — ordinarily the next frame. A
//! `Cancel` never notifies (the Cancel-never-mutates-state convention,
//! `docs/CODE_STANDARDS.md`).
//!
//! A [`PanZoomController`] attached with [`controller`](PanZoomView::controller)
//! is a cloneable, reactive-free handle: [`jump_to`](PanZoomController::jump_to)
//! / [`fit_to_bounds`](PanZoomController::fit_to_bounds) /
//! [`fit_rect`](PanZoomController::fit_rect) record a command the widget
//! applies at its next layout or paint (whichever comes first), and
//! [`transform`](PanZoomController::transform) reads the transform the widget
//! last published.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::event::{PointerId, ScaleEvent, ScalePhase};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerEvent, PointerPhase, SemanticsCtx,
    VelocityTracker, View, Widget, any,
};
use kurbo::{Affine, Point, Rect, Size, Vec2};

use crate::authoring::{presses, route_event_single};
use crate::physics::simulation::FrictionSimulation;
use crate::physics::{MAX_FLING_VELOCITY, MIN_FLING_VELOCITY, Simulation, Tolerance};
use crate::pinch::PinchRecognizer;
use crate::scroll::{InnerScrollState, ambient_scroll_claim, ambient_scroll_veto};

/// The smallest scale a [`PanZoomView`] allows unless
/// [`min_scale`](PanZoomView::min_scale) says otherwise.
pub const DEFAULT_MIN_SCALE: f64 = 0.25;

/// The largest scale a [`PanZoomView`] allows unless
/// [`max_scale`](PanZoomView::max_scale) says otherwise.
pub const DEFAULT_MAX_SCALE: f64 = 8.0;

/// The fraction of a pan glide's velocity surviving one second — the
/// [`FrictionSimulation`] drag the inertia runs on.
///
/// **Community-approximate**: no platform publishes a pan/zoom glide curve;
/// this is the drag Flutter's `InteractiveViewer` uses for its pan inertia
/// (`_kDrag`), which settles a typical flick within roughly a third of a
/// second.
const GLIDE_DRAG: f64 = 0.000_013_5;

/// The scroll-surface fling threshold doubles as the glide's settle speed, so
/// the input constants keep their single source in `frust_core::input`.
const GLIDE_TOLERANCE: Tolerance = Tolerance {
    velocity: FLING_STOP,
    distance: 0.5,
};

/// Where the content sits: `view = offset + scale · content`.
///
/// The child's pod carries exactly [`affine`](Self::affine); a content-space
/// point (the child's local coordinates) maps to the view's local space
/// through it and back through [`to_content`](Self::to_content).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanZoomTransform {
    /// The zoom factor: `1.0` is the child's natural size.
    pub scale: f64,
    /// Where the child's local origin sits in the view, in view-local px.
    pub offset: Vec2,
}

impl PanZoomTransform {
    /// Unscaled, unpanned: the child at the view's top-left at natural size.
    pub const IDENTITY: PanZoomTransform = PanZoomTransform {
        scale: 1.0,
        offset: Vec2::ZERO,
    };

    /// The child pod's transform: `translate(offset) · scale(scale)`.
    pub fn affine(&self) -> Affine {
        Affine::translate(self.offset) * Affine::scale(self.scale)
    }

    /// The content-space point drawn at view-local `point`.
    pub fn to_content(&self, point: Point) -> Point {
        ((point.to_vec2() - self.offset) / self.scale).to_point()
    }

    /// Where content-space `point` is drawn, in view-local px.
    pub fn to_view(&self, point: Point) -> Point {
        (self.offset + point.to_vec2() * self.scale).to_point()
    }
}

impl Default for PanZoomTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A command recorded on a [`PanZoomController`], applied by the widget.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    JumpTo { scale: f64, offset: Vec2 },
    FitContent,
    FitRect(Rect),
}

/// The state a controller shares with its widget.
#[derive(Debug, Default)]
struct Shared {
    transform: PanZoomTransform,
    viewport: Size,
    content: Size,
    pending: Option<Command>,
}

/// A cloneable handle onto a [`PanZoomView`]: drive it with
/// [`jump_to`](Self::jump_to)/[`fit_to_bounds`](Self::fit_to_bounds)/
/// [`fit_rect`](Self::fit_rect) and read where it stands with
/// [`transform`](Self::transform). Every clone shares one state, so the handle
/// an app keeps in its state and the one attached to the view are the same.
///
/// Commands are *recorded*, not applied: the widget applies the latest one at
/// its next layout or paint (a later command replaces an unapplied earlier
/// one), clamping the scale to the view's bounds. One controller drives one
/// view; attaching it to two makes them race for its commands.
#[derive(Clone, Debug, Default)]
pub struct PanZoomController {
    shared: Rc<RefCell<Shared>>,
}

impl PanZoomController {
    /// A controller attached to nothing yet, reading [`PanZoomTransform::IDENTITY`].
    pub fn new() -> Self {
        Self::default()
    }

    /// The transform the attached widget last published (identity until one
    /// attaches and lays out).
    pub fn transform(&self) -> PanZoomTransform {
        self.shared.borrow().transform
    }

    /// The attached widget's last laid-out size (zero until it lays out).
    pub fn viewport_size(&self) -> Size {
        self.shared.borrow().viewport
    }

    /// The child's last laid-out natural size (zero until it lays out).
    pub fn content_size(&self) -> Size {
        self.shared.borrow().content
    }

    /// Move to `scale` (clamped to the view's bounds) with the child's origin
    /// at view-local `offset`.
    pub fn jump_to(&self, scale: f64, offset: Vec2) {
        self.record(Command::JumpTo { scale, offset });
    }

    /// Fit the child's whole laid-out bounds into the view, centred, at the
    /// largest scale that shows all of it (clamped to the view's bounds).
    pub fn fit_to_bounds(&self) {
        self.record(Command::FitContent);
    }

    /// Fit `rect`, in the child's content space, into the view, centred, at the
    /// largest scale that shows all of it (clamped to the view's bounds).
    pub fn fit_rect(&self, rect: Rect) {
        self.record(Command::FitRect(rect));
    }

    /// Record `command`, then raise [`frust_core::mark_pending_result_flush`] so
    /// a frame runs to apply it even when the command came from outside any
    /// input path (the `NavigatorController` precedent).
    fn record(&self, command: Command) {
        self.shared.borrow_mut().pending = Some(command);
        frust_core::mark_pending_result_flush();
    }

    fn take_pending(&self) -> Option<Command> {
        self.shared.borrow_mut().pending.take()
    }

    fn has_pending(&self) -> bool {
        self.shared.borrow().pending.is_some()
    }

    fn publish(&self, transform: PanZoomTransform, viewport: Size, content: Size) {
        let mut shared = self.shared.borrow_mut();
        shared.transform = transform;
        shared.viewport = viewport;
        shared.content = content;
    }

    fn same(a: &Option<PanZoomController>, b: &Option<PanZoomController>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => Rc::ptr_eq(&a.shared, &b.shared),
            (None, None) => true,
            _ => false,
        }
    }
}

/// A declarative pan/zoom container. See the [module docs](self).
pub struct PanZoomView<State: 'static> {
    child: AnyView<State>,
    min_scale: f64,
    max_scale: f64,
    inertia: bool,
    on_transform: Option<crate::authoring::TypedArgCallback<State, PanZoomTransform>>,
    controller: Option<PanZoomController>,
}

/// Wrap `child` in a pan/zoom view: identity transform, scale bounds
/// [`DEFAULT_MIN_SCALE`]..=[`DEFAULT_MAX_SCALE`], no inertia.
pub fn pan_zoom<State: 'static, V: View<State>>(child: V) -> PanZoomView<State> {
    PanZoomView {
        child: any(child),
        min_scale: DEFAULT_MIN_SCALE,
        max_scale: DEFAULT_MAX_SCALE,
        inertia: false,
        on_transform: None,
        controller: None,
    }
}

impl<State: 'static> PanZoomView<State> {
    /// The smallest scale a zoom may reach (default [`DEFAULT_MIN_SCALE`]).
    /// Non-positive or non-finite values are ignored.
    pub fn min_scale(mut self, scale: f64) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.min_scale = scale;
        }
        self
    }

    /// The largest scale a zoom may reach (default [`DEFAULT_MAX_SCALE`]).
    /// Non-positive or non-finite values are ignored.
    pub fn max_scale(mut self, scale: f64) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.max_scale = scale;
        }
        self
    }

    /// Let a released pan glide to rest (default `false`).
    pub fn inertia(mut self, inertia: bool) -> Self {
        self.inertia = inertia;
        self
    }

    /// Run `on_transform(state, transform)` after every change to the
    /// transform (see the [module docs](self#notification-and-control)).
    pub fn on_transform<F: Fn(&mut State, PanZoomTransform) + 'static>(
        mut self,
        on_transform: F,
    ) -> Self {
        self.on_transform = Some(Rc::new(on_transform));
        self
    }

    /// Attach a [`PanZoomController`].
    pub fn controller(mut self, controller: PanZoomController) -> Self {
        self.controller = Some(controller);
        self
    }

    /// The scale bounds, ordered so an inverted pair cannot invert the clamp.
    fn bounds(&self) -> (f64, f64) {
        (
            self.min_scale.min(self.max_scale),
            self.max_scale.max(self.min_scale),
        )
    }
}

/// Who the claimant contact's gesture belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// No gesture.
    Idle,
    /// The child handled the claimant's `Down`; its events go to the child.
    Child,
    /// A pinch began over a child-owned gesture; the child is cancelled on the
    /// claimant's next event.
    StealPending,
    /// The claimant pans the content (suspended while a pinch is live).
    Pan { last: Point },
}

/// A pan glide in flight.
struct Glide {
    x: FrictionSimulation,
    y: FrictionSimulation,
    /// The first frame the glide was painted on (its `t = 0`).
    start: Option<frust_core::FrameTime>,
}

/// The retained widget for a [`PanZoomView`].
pub struct PanZoomWidget {
    child: ChildPod,
    transform: PanZoomTransform,
    min_scale: f64,
    max_scale: f64,
    inertia: bool,
    on_transform: Option<crate::authoring::ErasedArgCallback<PanZoomTransform>>,
    controller: Option<PanZoomController>,
    viewport: Size,
    content: Size,
    recognizer: PinchRecognizer,
    /// The contact whose primary `Down` opened the current gesture.
    claimant: Option<PointerId>,
    drag: Drag,
    /// The previous focal of an open scale bracket (`Begin` seen, `End` not).
    last_focal: Option<Point>,
    tracker_x: VelocityTracker,
    tracker_y: VelocityTracker,
    glide: Option<Glide>,
    /// The transform changed where `on_transform` could not run.
    notify_owed: bool,
    /// A Housekeeping flush was already requested for the owed notification.
    notify_requested: bool,
    /// The last painted frame time in ms — the event pass's timestamp source.
    last_frame_ms: f64,
    /// The enclosing scroll surface's live multi-contact veto, captured from
    /// [`ambient_scroll_veto`] on the claiming `Down` while it is still
    /// reachable — `None` outside a scroll surface. See the
    /// [module docs](self#input)'s child-owned-gesture paragraph.
    scroll_veto: Option<Rc<Cell<bool>>>,
}

impl<State: 'static> View<State> for PanZoomView<State> {
    type Element = PanZoomWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PanZoomWidget {
        let (min_scale, max_scale) = self.bounds();
        PanZoomWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            transform: PanZoomTransform::IDENTITY,
            min_scale,
            max_scale,
            inertia: self.inertia,
            on_transform: self
                .on_transform
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            controller: self.controller.clone(),
            viewport: Size::ZERO,
            content: Size::ZERO,
            recognizer: PinchRecognizer::new(),
            claimant: None,
            drag: Drag::Idle,
            last_focal: None,
            tracker_x: VelocityTracker::new(),
            tracker_y: VelocityTracker::new(),
            glide: None,
            notify_owed: false,
            notify_requested: false,
            last_frame_ms: 0.0,
            scroll_veto: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PanZoomWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.on_transform = self
            .on_transform
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        element.inertia = self.inertia;
        if !self.inertia {
            element.glide = None;
        }
        let (min_scale, max_scale) = self.bounds();
        if (min_scale, max_scale) != (element.min_scale, element.max_scale) {
            element.min_scale = min_scale;
            element.max_scale = max_scale;
            // Re-clamp about the viewport centre so the visible middle stays put.
            let centre = element.viewport.to_rect().center();
            if element.zoom(centre, centre, 1.0) {
                element.owe_notify();
                flags |= ChangeFlags::PAINT;
            }
        }
        if !PanZoomController::same(&element.controller, &self.controller) {
            element.controller = self.controller.clone();
            element.publish();
        }
        if element.controller.as_ref().is_some_and(|c| c.has_pending()) {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut PanZoomWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl PanZoomWidget {
    /// The transform the child is currently placed under.
    pub fn transform(&self) -> PanZoomTransform {
        self.transform
    }

    /// Map the content point under view-local `from` to `to`, multiplying the
    /// scale by `delta` (clamped). Returns whether the transform changed.
    fn zoom(&mut self, from: Point, to: Point, delta: f64) -> bool {
        let delta = if delta.is_finite() && delta > 0.0 {
            delta
        } else {
            1.0
        };
        let content = self.transform.to_content(from);
        let scale = (self.transform.scale * delta).clamp(self.min_scale, self.max_scale);
        let next = PanZoomTransform {
            scale,
            offset: to.to_vec2() - content.to_vec2() * scale,
        };
        self.set(next)
    }

    /// Install `next` if it differs and is finite. Returns whether it changed.
    fn set(&mut self, next: PanZoomTransform) -> bool {
        let finite =
            next.scale.is_finite() && next.offset.x.is_finite() && next.offset.y.is_finite();
        if !finite || next == self.transform {
            return false;
        }
        self.transform = next;
        self.publish();
        true
    }

    /// Publish the current geometry to the attached controller.
    fn publish(&self) {
        if let Some(controller) = &self.controller {
            controller.publish(self.transform, self.viewport, self.content);
        }
    }

    /// Record a change no `EventCtx` was present for.
    fn owe_notify(&mut self) {
        self.notify_owed = true;
    }

    /// A change made during the event pass: notify now and repaint.
    fn changed(&mut self, ctx: &mut EventCtx) {
        self.notify_owed = false;
        self.notify_requested = false;
        if let Some(cb) = self.on_transform.as_mut() {
            cb(ctx, self.transform);
        }
        ctx.request_redraw();
    }

    /// Deliver a notification owed from a layout/paint-time change.
    fn deliver_owed(&mut self, ctx: &mut EventCtx) {
        if self.notify_owed {
            self.changed(ctx);
        }
    }

    /// Apply the controller's pending command, if any, against the current
    /// viewport and content sizes. Returns whether the transform changed.
    fn apply_pending(&mut self) -> bool {
        let Some(command) = self.controller.as_ref().and_then(|c| c.take_pending()) else {
            return false;
        };
        self.glide = None;
        self.last_focal = None;
        let next = match command {
            Command::JumpTo { scale, offset } => PanZoomTransform {
                scale: if scale.is_finite() && scale > 0.0 {
                    scale.clamp(self.min_scale, self.max_scale)
                } else {
                    self.transform.scale
                },
                offset,
            },
            Command::FitContent => match self.fit(self.content.to_rect()) {
                Some(next) => next,
                None => return false,
            },
            Command::FitRect(rect) => match self.fit(rect) {
                Some(next) => next,
                None => return false,
            },
        };
        let changed = self.set(next);
        if changed {
            self.owe_notify();
        }
        changed
    }

    /// The transform that centres content-space `rect` in the viewport at the
    /// largest in-bounds scale showing all of it; `None` for an empty rect or
    /// viewport.
    fn fit(&self, rect: Rect) -> Option<PanZoomTransform> {
        let rect = rect.abs();
        if rect.width() <= 0.0
            || rect.height() <= 0.0
            || self.viewport.width <= 0.0
            || self.viewport.height <= 0.0
        {
            return None;
        }
        let scale = (self.viewport.width / rect.width())
            .min(self.viewport.height / rect.height())
            .clamp(self.min_scale, self.max_scale);
        let centre = self.viewport.to_rect().center();
        Some(PanZoomTransform {
            scale,
            offset: centre.to_vec2() - rect.center().to_vec2() * scale,
        })
    }

    /// Apply one scale event (either source). Returns whether the transform
    /// changed. A bracketed gesture's moving focal pans the content with it.
    fn apply_scale(&mut self, scale: &ScaleEvent) -> bool {
        self.glide = None;
        match scale.phase {
            ScalePhase::Begin => {
                self.last_focal = Some(scale.focal);
                self.zoom(scale.focal, scale.focal, scale.scale_delta)
            }
            ScalePhase::Update => {
                let from = match self.last_focal.as_mut() {
                    Some(last) => std::mem::replace(last, scale.focal),
                    // A lone update (a wheel notch) has no bracket to track.
                    None => scale.focal,
                };
                self.zoom(from, scale.focal, scale.scale_delta)
            }
            ScalePhase::End => {
                let anchor = self.last_focal.take().unwrap_or(scale.focal);
                self.zoom(anchor, anchor, scale.scale_delta)
            }
        }
    }

    /// A recognised touch-pinch event from contact event `p`.
    fn on_pinch(&mut self, ctx: &mut EventCtx, scale: ScaleEvent, p: &PointerEvent) {
        if scale.phase == ScalePhase::Begin {
            if self.drag == Drag::Child {
                self.drag = Drag::StealPending;
            }
            // A pinch's motion is not a pan's: no glide from it.
            self.tracker_x.clear();
            self.tracker_y.clear();
        }
        if p.phase == PointerPhase::Cancel {
            // Never notify from a Cancel; just close the bracket.
            self.last_focal = None;
            return;
        }
        if self.apply_scale(&scale) {
            self.changed(ctx);
        }
    }

    /// Start a glide from the claimant's release, if inertia is on and the
    /// release was fast enough.
    fn maybe_glide(&mut self) {
        if !self.inertia {
            return;
        }
        let mut velocity = Vec2::new(self.tracker_x.velocity(), self.tracker_y.velocity());
        let speed = velocity.hypot();
        if !speed.is_finite() || speed < MIN_FLING_VELOCITY {
            return;
        }
        if speed > MAX_FLING_VELOCITY {
            velocity *= MAX_FLING_VELOCITY / speed;
        }
        let offset = self.transform.offset;
        self.glide = Some(Glide {
            x: FrictionSimulation::new(GLIDE_DRAG, offset.x, velocity.x, GLIDE_TOLERANCE, 0.0),
            y: FrictionSimulation::new(GLIDE_DRAG, offset.y, velocity.y, GLIDE_TOLERANCE, 0.0),
            start: None,
        });
    }

    /// Advance a glide to this frame. Paint-only: the pod transform moves, the
    /// layout does not.
    fn pump_glide(&mut self, ctx: &mut PaintCtx) {
        let Some(glide) = self.glide.as_mut() else {
            return;
        };
        let now = ctx.frame_time();
        let start = *glide.start.get_or_insert(now);
        let t = now.saturating_sub(start).as_secs_f64();
        let offset = Vec2::new(glide.x.x(t), glide.y.x(t));
        let done = glide.x.is_done(t) && glide.y.is_done(t);
        if done {
            self.glide = None;
        } else {
            ctx.request_frame();
        }
        let next = PanZoomTransform {
            offset,
            ..self.transform
        };
        if self.set(next) {
            self.owe_notify();
        }
    }

    /// The opening primary `Down` of a gesture: offer it to the child, then
    /// claim the gesture (and its other contacts) either way.
    fn begin_gesture(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        id: PointerId,
        p: &PointerEvent,
    ) {
        self.glide = None;
        self.recognizer.reset();
        self.last_focal = None;
        self.tracker_x.clear();
        self.tracker_y.clear();
        self.claimant = Some(id);
        // Capture the enclosing scroll surface's live multi-contact veto now,
        // while the ambient cell from its `Down` forward is still reachable —
        // needed below only for the child-owned branch, but captured
        // unconditionally since this is the one point in the gesture the
        // ambient cell is visible from (`crate::scroll`'s module docs'
        // *Multi-contact veto*).
        self.scroll_veto = ambient_scroll_veto();
        self.recognizer.handle(id, p, self.last_frame_ms);
        self.sync_scroll_veto();
        let inside = self.child.contains(p.position);
        let result = if inside {
            self.child.event_child(ctx, event)
        } else {
            EventResult::Ignored
        };
        if !inside && self.child.is_focused() {
            // Blur-on-outside-tap, as `route_event_single` does.
            self.child.set_focused(false);
        }
        if result == EventResult::Handled {
            self.drag = Drag::Child;
            // The child owns the gesture, not this view — publish nothing.
            // An enclosing `ScrollView`/`ListView` already wrote its own
            // default (unregistered) `InnerScrollState` into this cell
            // before forwarding the `Down` that reached here, and that
            // default is exactly "nothing claims this drag", so leaving it
            // untouched is the correct report.
        } else {
            self.drag = Drag::Pan { last: p.position };
            self.tracker_x.record(self.last_frame_ms, p.position.x);
            self.tracker_y.record(self.last_frame_ms, p.position.y);
            // Innermost-wins nested-scroll arbitration (`scroll.rs` owns the
            // seam; `ListView` is the other consumer): the child ignored the
            // `Down`, so this view is about to pan it, and reports that into
            // whichever cell is ambient — the nearest enclosing scroll
            // surface's own fresh cell for *this* `Down`
            // (`with_scroll_claim`), pushed before the forward that reached
            // this `begin_gesture` and read back synchronously the moment
            // that forward returns. A single write at `Down` is sufficient
            // and needs no later reset: the host never reuses a cell across
            // gestures (a fresh `Rc<Cell<_>>` is made for every `Down`), so
            // there is nothing stale to clear between gestures. Both
            // directions are claimed unconditionally — unlike a scrollable,
            // whose claim depends on remaining content/physics, this view
            // pans freely on either axis the moment it owns the gesture.
            if let Some(host) = ambient_scroll_claim() {
                host.set(InnerScrollState {
                    registered: true,
                    can_consume_down_drag: true,
                    can_consume_up_drag: true,
                });
            }
        }
        ctx.capture_pointer();
        ctx.capture_contacts();
    }

    /// Raise or clear the captured [`PanZoomWidget::scroll_veto`] to match
    /// whether [`PanZoomWidget::recognizer`] is tracking more than one
    /// contact right now — called after every [`PinchRecognizer::handle`], so
    /// an enclosing scroll surface sees the flip before its own next `Move`
    /// decides whether to take the claimant's finger over. Live-only: the
    /// `Drag::Pan` branch already claims unconditionally at `Down`
    /// (`begin_gesture`'s nested-scroll report), so this matters for
    /// `Drag::Child`, where that report is deliberately left unregistered.
    fn sync_scroll_veto(&self) {
        if let Some(veto) = &self.scroll_veto {
            veto.set(self.recognizer.contact_count() >= 2);
        }
    }

    /// A later event of the claimant contact.
    fn claimant_event(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        id: PointerId,
        p: &PointerEvent,
    ) {
        if let Some(scale) = self.recognizer.handle(id, p, self.last_frame_ms) {
            self.on_pinch(ctx, scale, p);
        }
        self.sync_scroll_veto();
        let ends = matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel);
        match self.drag {
            Drag::Idle => {}
            Drag::Child => {
                route_event_single(&mut self.child, ctx, event);
            }
            Drag::StealPending => {
                let cancel = InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Cancel,
                    ..*p
                });
                self.child.event_child(ctx, &cancel);
                self.child.set_active(false);
                self.drag = Drag::Pan { last: p.position };
            }
            Drag::Pan { last } => {
                if p.phase == PointerPhase::Move {
                    if !self.recognizer.is_pinching() {
                        let next = PanZoomTransform {
                            offset: self.transform.offset + (p.position - last),
                            ..self.transform
                        };
                        if self.set(next) {
                            self.changed(ctx);
                        }
                        self.tracker_x.record(self.last_frame_ms, p.position.x);
                        self.tracker_y.record(self.last_frame_ms, p.position.y);
                    }
                    self.drag = Drag::Pan { last: p.position };
                } else if p.phase == PointerPhase::Up {
                    self.tracker_x.record(self.last_frame_ms, p.position.x);
                    self.tracker_y.record(self.last_frame_ms, p.position.y);
                    self.maybe_glide();
                    if self.glide.is_some() {
                        ctx.request_redraw();
                    }
                }
            }
        }
        if ends {
            self.claimant = None;
            self.drag = Drag::Idle;
            self.recognizer.reset();
            self.last_focal = None;
            self.scroll_veto = None;
        }
    }
}

impl Widget for PanZoomWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let unbounded = BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, f64::INFINITY));
        let mut content = self.child.layout_child(ctx, &unbounded);
        let axis = |limit: f64, natural: f64| {
            if limit.is_finite() {
                limit
            } else if natural.is_finite() {
                natural
            } else {
                0.0
            }
        };
        let viewport = bc.constrain(Size::new(
            axis(max.width, content.width),
            axis(max.height, content.height),
        ));
        if !(content.width.is_finite() && content.height.is_finite()) {
            // An expanding child (a default `canvas`) has no natural size of its
            // own: give it the viewport rather than an infinite transform.
            content = self
                .child
                .layout_child(ctx, &BoxConstraints::loose(viewport));
        }
        self.viewport = viewport;
        self.content = content;
        self.child.set_origin(Point::ZERO);
        self.apply_pending();
        self.publish();
        self.child.set_transform(Some(self.transform.affine()));
        viewport
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The event pass carries no clock; stamp velocity samples with the last
        // painted frame instead (the `ScrollView` precedent).
        self.last_frame_ms = ctx.frame_time().as_secs_f64() * 1000.0;
        self.apply_pending();
        self.pump_glide(ctx);
        if self.on_transform.is_none() {
            self.notify_owed = false;
        } else if self.notify_owed && !self.notify_requested {
            // Paint has no `&mut State`: owe the notification to the next
            // Housekeeping flush (the `GestureDetector` long-press precedent).
            self.notify_requested = true;
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
        self.child.set_transform(Some(self.transform.affine()));
        scene.push_clip(ctx.origin(), ctx.size());
        ctx.constrain_visible_rect(Rect::from_origin_size(ctx.origin(), ctx.size()));
        self.child.paint_child(ctx, scene);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let cancel = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel);
        if !cancel {
            self.deliver_owed(ctx);
        }
        if event.is_broadcast() {
            self.child.event_child(ctx, event);
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Pointer(p) => {
                let id = ctx.pointer_id();
                let starts = p.phase == PointerPhase::Down && presses(p);
                if self.claimant.is_none() || (starts && self.claimant == Some(id)) {
                    if !starts {
                        // Hover moves, non-primary presses, stray releases.
                        return route_event_single(&mut self.child, ctx, event);
                    }
                    self.begin_gesture(ctx, event, id, p);
                    return EventResult::Handled;
                }
                if self.claimant == Some(id) {
                    self.claimant_event(ctx, event, id, p);
                } else {
                    let scale = self.recognizer.handle(id, p, self.last_frame_ms);
                    self.sync_scroll_veto();
                    if let Some(scale) = scale {
                        // Another contact of the gesture is the recogniser's alone.
                        self.on_pinch(ctx, scale, p);
                    }
                }
                EventResult::Handled
            }
            InputEvent::Scale(scale) => {
                if self.child.contains(scale.focal)
                    && self.child.event_child(ctx, event) == EventResult::Handled
                {
                    return EventResult::Handled;
                }
                if self.apply_scale(scale) {
                    self.changed(ctx);
                }
                EventResult::Handled
            }
            InputEvent::Scroll { .. } => {
                // An open `Scale` bracket (`Begin` seen, no `End` yet) can have
                // its rest arrive as a plain, unmodified `Scroll` instead — a
                // macOS trackpad pinch whose ⌘ is released mid-gesture finishes
                // as wheel events, with no `End` ever delivered. Left set, a
                // later lone `Update` (an unrelated wheel notch) would read
                // `last_focal` as that stale bracket's continuation and anchor
                // its zoom there instead of at its own focal. A `Scroll` can
                // only reach a view with no bracket genuinely still open —
                // this widget's own pinch recogniser and the desktop
                // modified-wheel mapping both route a live bracket's events as
                // `Scale`, never `Scroll` — so clearing here never cuts off an
                // in-progress bracket.
                self.last_focal = None;
                route_event_single(&mut self.child, ctx, event)
            }
            // Focus-routed and any other remaining event belongs to the child.
            _ => route_event_single(&mut self.child, ctx, event),
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent container: forward to the single child.
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, PointerButton, RenderRoot, ScrollDelta};
    use std::cell::RefCell;

    /// What the test content recorded, in its own local space.
    #[derive(Default)]
    struct Log {
        events: Vec<InputEvent>,
    }

    #[derive(Default)]
    struct App {
        transforms: Vec<PanZoomTransform>,
        /// Every `ScrollInfo::offset` an enclosing `scroll_view` in the
        /// nested-claim fixtures reported through `on_scroll` — empty means
        /// that outer surface never scrolled.
        scroll_offsets: Vec<f64>,
    }

    /// A 1000 × 800 content leaf with one "node" at (100, 100)–(200, 200) that
    /// claims a primary press (and captures), and that handles a scroll only
    /// when `scrolls`. Logs every event into a shared log, never app state, so
    /// its `Cancel` arm stays state-free.
    struct Content {
        log: Rc<RefCell<Log>>,
        scrolls: bool,
        size: Option<Size>,
    }
    struct ContentWidget {
        log: Rc<RefCell<Log>>,
        scrolls: bool,
        size: Option<Size>,
        captured: bool,
    }
    const NODE: Rect = Rect::new(100.0, 100.0, 200.0, 200.0);
    const CONTENT: Size = Size::new(1000.0, 800.0);

    impl View<App> for Content {
        type Element = ContentWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ContentWidget {
            ContentWidget {
                log: self.log.clone(),
                scrolls: self.scrolls,
                size: self.size,
                captured: false,
            }
        }
        fn rebuild(&self, _p: &Self, _e: &mut ContentWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ContentWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            match self.size {
                Some(size) => bc.constrain(size),
                None => bc.max(),
            }
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            self.log.borrow_mut().events.push(event.clone());
            match event {
                InputEvent::Pointer(p) => match p.phase {
                    PointerPhase::Down if NODE.contains(p.position) => {
                        self.captured = true;
                        ctx.capture_pointer();
                        EventResult::Handled
                    }
                    PointerPhase::Move | PointerPhase::Up | PointerPhase::Cancel
                        if self.captured =>
                    {
                        if p.phase != PointerPhase::Move {
                            self.captured = false;
                        }
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                },
                InputEvent::Scroll { .. } if self.scrolls => EventResult::Handled,
                _ => EventResult::Ignored,
            }
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    type Root = RenderRoot<App, PanZoomView<App>>;
    type Logic = Box<dyn FnMut(&mut App) -> PanZoomView<App>>;

    struct Harness {
        root: Root,
        state: App,
        logic: Logic,
        log: Rc<RefCell<Log>>,
        controller: PanZoomController,
        now_ms: u64,
    }

    impl Harness {
        fn new(configure: impl Fn(PanZoomView<App>) -> PanZoomView<App> + 'static) -> Self {
            Self::with_content(false, Some(CONTENT), configure)
        }

        fn with_content(
            scrolls: bool,
            size: Option<Size>,
            configure: impl Fn(PanZoomView<App>) -> PanZoomView<App> + 'static,
        ) -> Self {
            let log = Rc::new(RefCell::new(Log::default()));
            let controller = PanZoomController::new();
            let (child_log, child_controller) = (log.clone(), controller.clone());
            let logic = Box::new(move |_: &mut App| {
                configure(
                    pan_zoom(Content {
                        log: child_log.clone(),
                        scrolls,
                        size,
                    })
                    .controller(child_controller.clone())
                    .on_transform(|s: &mut App, t| s.transforms.push(t)),
                )
            });
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                logic,
                log,
                controller,
                now_ms: 0,
            };
            h.frame();
            h
        }

        /// One shell frame: rebuild (draining any Housekeeping flush), layout,
        /// paint at the current clock; then advance the clock by 16 ms.
        fn frame(&mut self) -> frust_core::PaintOutcome {
            self.root.rebuild(&mut self.logic, &mut self.state);
            self.root.layout(Size::new(400.0, 300.0));
            let outcome = self.root.paint(
                &mut NullScene,
                FrameTime::from_nanos(self.now_ms * 1_000_000),
            );
            self.now_ms += 16;
            outcome
        }

        fn send(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn transform(&self) -> PanZoomTransform {
            self.controller.transform()
        }
    }

    fn pe(phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
        PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        }
    }

    fn mouse(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(pe(phase, x, y))
    }

    fn touch(slot: u32, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(slot),
            event: pe(phase, x, y),
        }
    }

    fn scale(phase: ScalePhase, delta: f64, x: f64, y: f64) -> InputEvent {
        InputEvent::Scale(ScaleEvent {
            phase,
            scale_delta: delta,
            focal: Point::new(x, y),
            velocity: 0.0,
        })
    }

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    fn same_transform(a: PanZoomTransform, b: PanZoomTransform) -> bool {
        (a.scale - b.scale).abs() < 1e-9 && (a.offset - b.offset).hypot() < 1e-9
    }

    #[test]
    fn transform_round_trips_through_its_affine() {
        let t = PanZoomTransform {
            scale: 2.5,
            offset: Vec2::new(-30.0, 12.0),
        };
        let content = Point::new(17.0, -4.0);
        assert!(close(t.affine() * content, t.to_view(content)));
        assert!(close(t.to_content(t.to_view(content)), content));
        assert!(close(
            t.affine().inverse() * Point::new(5.0, 6.0),
            t.to_content(Point::new(5.0, 6.0))
        ));
    }

    #[test]
    fn zoom_about_a_focal_keeps_the_focal_content_point_fixed() {
        let mut h = Harness::new(|v| v);
        // Pan first so the transform is not the identity.
        h.send(mouse(PointerPhase::Down, 300.0, 50.0));
        h.send(mouse(PointerPhase::Move, 280.0, 70.0));
        h.send(mouse(PointerPhase::Up, 280.0, 70.0));
        let focal = Point::new(150.0, 120.0);
        let before = h.transform();
        let content = before.affine().inverse() * focal;
        for (phase, delta) in [
            (ScalePhase::Begin, 1.0),
            (ScalePhase::Update, 1.5),
            (ScalePhase::Update, 1.2),
            (ScalePhase::End, 1.0),
        ] {
            h.send(scale(phase, delta, focal.x, focal.y));
        }
        let after = h.transform();
        assert!((after.scale - 1.8).abs() < 1e-12, "{after:?}");
        assert!(
            close(after.affine() * content, focal),
            "focal content point moved"
        );
        h.frame();
        // The child pod carries the same affine, so the framework maps the
        // focal back to the same content point.
        h.send(mouse(PointerPhase::Move, focal.x, focal.y));
        let last = h.log.borrow().events.last().cloned().unwrap();
        assert!(close(last.position(), content), "{last:?}");
    }

    #[test]
    fn scale_clamps_to_the_bounds_and_stays_focal_correct() {
        let mut h = Harness::new(|v| v);
        let focal = Point::new(200.0, 150.0);
        for _ in 0..40 {
            h.send(scale(ScalePhase::Update, 1.5, focal.x, focal.y));
        }
        assert_eq!(h.transform().scale, DEFAULT_MAX_SCALE);
        for _ in 0..80 {
            h.send(scale(ScalePhase::Update, 0.5, focal.x, focal.y));
        }
        assert_eq!(h.transform().scale, DEFAULT_MIN_SCALE);
        // The focal's content point survives the clamped steps too.
        let content = h.transform().to_content(focal);
        h.send(scale(ScalePhase::Update, 0.5, focal.x, focal.y));
        assert!(close(h.transform().to_view(content), focal));

        let mut h = Harness::new(|v| v.min_scale(0.5).max_scale(2.0));
        h.send(scale(ScalePhase::Update, 10.0, 0.0, 0.0));
        assert_eq!(h.transform().scale, 2.0);
        h.send(scale(ScalePhase::Update, 0.01, 0.0, 0.0));
        assert_eq!(h.transform().scale, 0.5);
    }

    #[test]
    fn primary_drag_on_empty_content_pans() {
        let mut h = Harness::new(|v| v);
        h.send(mouse(PointerPhase::Down, 300.0, 250.0));
        h.send(mouse(PointerPhase::Move, 320.0, 240.0));
        h.send(mouse(PointerPhase::Move, 350.0, 200.0));
        h.send(mouse(PointerPhase::Up, 350.0, 200.0));
        assert_eq!(h.transform().offset, Vec2::new(50.0, -50.0));
        assert_eq!(h.transform().scale, 1.0);
        assert_eq!(
            h.state.transforms.len(),
            2,
            "one notification per moving event"
        );
        assert_eq!(h.state.transforms[1], h.transform());
        assert!(!h.root.is_pointer_captured());
    }

    #[test]
    fn a_child_press_claims_the_gesture_and_suppresses_pan() {
        let mut h = Harness::new(|v| v);
        h.send(mouse(PointerPhase::Down, 150.0, 150.0)); // on the node
        h.send(mouse(PointerPhase::Move, 250.0, 250.0));
        h.send(mouse(PointerPhase::Up, 250.0, 250.0));
        assert_eq!(h.transform(), PanZoomTransform::IDENTITY);
        assert!(h.state.transforms.is_empty());
        let phases: Vec<_> = h
            .log
            .borrow()
            .events
            .iter()
            .filter_map(|e| match e {
                InputEvent::Pointer(p) => Some(p.phase),
                _ => None,
            })
            .collect();
        assert_eq!(
            phases,
            [PointerPhase::Down, PointerPhase::Move, PointerPhase::Up]
        );
    }

    #[test]
    fn child_events_are_inverse_mapped_under_zoom() {
        let mut h = Harness::new(|v| v);
        h.controller.jump_to(2.0, Vec2::new(-100.0, -100.0));
        h.frame();
        // Content (150, 150) — the node — is drawn at (200, 200).
        h.send(mouse(PointerPhase::Down, 200.0, 200.0));
        h.send(mouse(PointerPhase::Move, 260.0, 200.0));
        h.send(mouse(PointerPhase::Up, 260.0, 200.0));
        let events: Vec<_> = h
            .log
            .borrow()
            .events
            .iter()
            .filter(|e| matches!(e, InputEvent::Pointer(_)))
            .cloned()
            .collect();
        assert!(close(events[0].position(), Point::new(150.0, 150.0)));
        assert!(close(events[1].position(), Point::new(180.0, 150.0)));
        assert_eq!(h.transform().offset, Vec2::new(-100.0, -100.0), "no pan");
    }

    #[test]
    fn touch_pinch_and_desktop_scale_produce_the_same_transform() {
        use PointerPhase::{Down, Move, Up};
        // Touch: two fingers on empty content, 20 px apart about (300, 250),
        // spread to 140 px with the focal wandering and ending back at (300, 250).
        let mut touch_h = Harness::new(|v| v);
        touch_h.send(touch(0, Down, 290.0, 250.0));
        touch_h.send(touch(1, Down, 310.0, 250.0));
        touch_h.send(touch(1, Move, 330.0, 250.0)); // 20 → 40: Begin, focal (310, 250)
        touch_h.send(touch(0, Move, 270.0, 250.0)); // 40 → 60, focal (300, 250)
        touch_h.send(touch(1, Move, 350.0, 250.0)); // 60 → 80, focal (310, 250)
        touch_h.send(touch(0, Move, 210.0, 250.0)); // 80 → 140, focal (280, 250)
        touch_h.send(touch(1, Move, 370.0, 250.0)); // 140 → 160, focal (290, 250)
        touch_h.send(touch(0, Move, 230.0, 250.0)); // 160 → 140, focal (300, 250)
        touch_h.send(touch(1, Up, 370.0, 250.0));
        touch_h.send(touch(0, Up, 230.0, 250.0));
        let touch_t = touch_h.transform();
        assert!((touch_t.scale - 140.0 / 40.0).abs() < 1e-9, "{touch_t:?}");

        // Desktop: one bracket with the same net factor, opened at the same
        // focal and closed at the same focal.
        let mut desk_h = Harness::new(|v| v);
        desk_h.send(scale(ScalePhase::Begin, 1.0, 310.0, 250.0));
        desk_h.send(scale(ScalePhase::Update, 140.0 / 40.0, 300.0, 250.0));
        desk_h.send(scale(ScalePhase::End, 1.0, 300.0, 250.0));
        let desk_t = desk_h.transform();
        assert!(
            same_transform(touch_t, desk_t),
            "touch {touch_t:?} vs desktop {desk_t:?}"
        );
        assert!(!touch_h.root.is_pointer_captured());
    }

    #[test]
    fn a_pinch_over_a_child_owned_press_steals_it() {
        use PointerPhase::{Cancel, Down, Move, Up};
        let mut h = Harness::new(|v| v);
        h.send(touch(0, Down, 150.0, 150.0)); // the node claims it
        h.send(touch(1, Down, 170.0, 150.0));
        h.send(touch(1, Move, 210.0, 150.0)); // 20 → 60: Begin
        h.send(touch(0, Move, 140.0, 150.0)); // carries the steal
        h.send(touch(1, Move, 230.0, 150.0)); // zooms
        h.send(touch(1, Up, 230.0, 150.0));
        h.send(touch(0, Up, 140.0, 150.0));
        let phases: Vec<_> = h
            .log
            .borrow()
            .events
            .iter()
            .filter_map(|e| match e {
                InputEvent::Pointer(p) => Some(p.phase),
                _ => None,
            })
            .collect();
        assert_eq!(
            phases,
            [Down, Cancel],
            "the second finger never reaches the child"
        );
        assert!(h.transform().scale > 1.0);
        assert!(!h.root.is_pointer_captured());
    }

    #[test]
    fn a_child_handled_scale_is_not_applied_and_plain_wheel_reaches_the_child() {
        let mut h = Harness::with_content(true, Some(CONTENT), |v| v);
        h.send(InputEvent::Scroll {
            position: Point::new(50.0, 50.0),
            delta: ScrollDelta::Lines(0.0, 3.0),
        });
        assert!(matches!(
            h.log.borrow().events.last(),
            Some(InputEvent::Scroll { .. })
        ));
        assert_eq!(
            h.transform(),
            PanZoomTransform::IDENTITY,
            "a wheel never pans"
        );

        // The content ignores Scale, so the view zooms; the child saw it first.
        h.send(scale(ScalePhase::Update, 2.0, 50.0, 50.0));
        assert!(matches!(
            h.log.borrow().events.last(),
            Some(InputEvent::Scale(_))
        ));
        assert_eq!(h.transform().scale, 2.0);
    }

    #[test]
    fn controller_jump_and_fit() {
        let mut h = Harness::new(|v| v);
        h.controller.jump_to(3.0, Vec2::new(10.0, 20.0));
        h.frame();
        assert_eq!(
            h.transform(),
            PanZoomTransform {
                scale: 3.0,
                offset: Vec2::new(10.0, 20.0)
            }
        );
        assert_eq!(h.controller.viewport_size(), Size::new(400.0, 300.0));
        assert_eq!(h.controller.content_size(), CONTENT);
        // The owed notification arrives through the next frame's flush.
        h.frame();
        assert_eq!(h.state.transforms.last(), Some(&h.transform()));

        // An out-of-bounds jump clamps.
        h.controller.jump_to(100.0, Vec2::ZERO);
        h.frame();
        assert_eq!(h.transform().scale, DEFAULT_MAX_SCALE);

        // Fit 1000 × 800 into 400 × 300: limited by height, 0.375, centred.
        h.controller.fit_to_bounds();
        h.frame();
        let fit = h.transform();
        assert!((fit.scale - 0.375).abs() < 1e-12);
        assert!(close(
            fit.to_view(Point::new(500.0, 400.0)),
            Point::new(200.0, 150.0)
        ));

        // Fit the node: 100 × 100 into 400 × 300 → 3.0, node centre at view centre.
        h.controller.fit_rect(NODE);
        h.frame();
        let fit = h.transform();
        assert!((fit.scale - 3.0).abs() < 1e-12);
        assert!(close(fit.to_view(NODE.center()), Point::new(200.0, 150.0)));
    }

    #[test]
    fn inertia_glides_then_settles() {
        let mut h = Harness::new(|v| v.inertia(true));
        h.send(mouse(PointerPhase::Down, 300.0, 250.0));
        for step in 1..=4 {
            h.frame();
            h.send(mouse(PointerPhase::Move, 300.0 - 20.0 * step as f64, 250.0));
        }
        h.send(mouse(PointerPhase::Up, 220.0, 250.0));
        let released = h.transform().offset;
        assert_eq!(released, Vec2::new(-80.0, 0.0));
        let mut frames = 0;
        loop {
            let outcome = h.frame();
            frames += 1;
            if !outcome.needs_frame {
                break;
            }
            assert!(frames < 200, "the glide never settled");
        }
        h.frame(); // deliver the final owed notification
        let rest = h.transform().offset;
        assert!(rest.x < released.x - 50.0, "glided on: {rest:?}");
        assert_eq!(rest.y, 0.0);
        assert!(frames > 3, "the glide took several frames");
        assert_eq!(h.state.transforms.last().map(|t| t.offset), Some(rest));
        // Settled: further frames move nothing.
        h.frame();
        assert_eq!(h.transform().offset, rest);

        // Without inertia the same release stops dead.
        let mut h = Harness::new(|v| v);
        h.send(mouse(PointerPhase::Down, 300.0, 250.0));
        for step in 1..=4 {
            h.frame();
            h.send(mouse(PointerPhase::Move, 300.0 - 20.0 * step as f64, 250.0));
        }
        h.send(mouse(PointerPhase::Up, 220.0, 250.0));
        assert!(!h.frame().needs_frame);
        assert_eq!(h.transform().offset, Vec2::new(-80.0, 0.0));
    }

    #[test]
    fn a_press_stops_a_glide() {
        let mut h = Harness::new(|v| v.inertia(true));
        h.send(mouse(PointerPhase::Down, 300.0, 250.0));
        for step in 1..=4 {
            h.frame();
            h.send(mouse(PointerPhase::Move, 300.0 - 20.0 * step as f64, 250.0));
        }
        h.send(mouse(PointerPhase::Up, 220.0, 250.0));
        h.frame();
        h.frame();
        h.send(mouse(PointerPhase::Down, 300.0, 250.0));
        let held = h.transform();
        assert!(!h.frame().needs_frame);
        assert_eq!(h.transform(), held);
    }

    #[test]
    fn an_expanding_child_is_laid_out_at_the_viewport() {
        let h = Harness::with_content(false, None, |v| v);
        assert_eq!(h.controller.content_size(), Size::new(400.0, 300.0));
        assert_eq!(h.transform(), PanZoomTransform::IDENTITY);
    }

    #[test]
    fn rebuilt_bounds_reclamp_about_the_viewport_centre() {
        let max = Rc::new(std::cell::Cell::new(8.0));
        let bound = max.clone();
        let mut h = Harness::new(move |v| v.max_scale(bound.get()));
        h.send(scale(ScalePhase::Update, 4.0, 0.0, 0.0));
        assert_eq!(h.transform().scale, 4.0);
        let centre = Point::new(200.0, 150.0);
        let content = h.transform().to_content(centre);
        max.set(2.0);
        h.frame();
        assert_eq!(h.transform().scale, 2.0);
        assert!(close(h.transform().to_view(content), centre));
    }

    // --- Nested-scroll claim: this view joins `scroll.rs`'s innermost-wins
    //     seam when it is about to pan. See `begin_gesture` and the module
    //     docs' *Pan* bullet. ---

    use crate::flex::Column;
    use crate::scroll::{ScrollView, scroll_view};
    use crate::sized::SizedBox;

    /// The fixed height of the pan_zoom row in the nested-claim fixture.
    const PAN_H: f64 = 150.0;
    /// The fixed height of the plain sibling row — tall enough that, added to
    /// [`PAN_H`], the column exceeds the 300px viewport (so the outer
    /// `scroll_view` genuinely has somewhere to scroll).
    const SIBLING_H: f64 = 300.0;

    type NestLogic = Box<dyn FnMut(&mut App) -> ScrollView<App>>;

    /// `scroll_view(Column([pan_zoom(Content), plain sibling]))` — a real tree
    /// shape (`scroll_view(pan_zoom(canvas))` on a page with other content),
    /// laid out at the same 400×300 window every other fixture in this module
    /// uses. Exercises `scroll.rs`'s claim seam from the outside rather than
    /// reimplementing it.
    struct NestFixture {
        root: RenderRoot<App, ScrollView<App>>,
        state: App,
        logic: NestLogic,
        controller: PanZoomController,
    }

    impl NestFixture {
        fn new() -> Self {
            let log = Rc::new(RefCell::new(Log::default()));
            let controller = PanZoomController::new();
            let (child_log, child_controller) = (log, controller.clone());
            let logic: NestLogic = Box::new(move |_: &mut App| {
                scroll_view(Column(vec![
                    any(SizedBox(None, Some(PAN_H)).child(
                        pan_zoom(Content {
                            log: child_log.clone(),
                            scrolls: false,
                            size: Some(Size::new(400.0, PAN_H)),
                        })
                        .controller(child_controller.clone()),
                    )),
                    any(SizedBox(None, Some(SIBLING_H)).child(Content {
                        log: child_log.clone(),
                        scrolls: false,
                        size: Some(Size::new(400.0, SIBLING_H)),
                    })),
                ]))
                .on_scroll(|s: &mut App, info| s.scroll_offsets.push(info.offset))
            });
            let mut fixture = NestFixture {
                root: RenderRoot::new(),
                state: App::default(),
                logic,
                controller,
            };
            fixture.frame();
            fixture
        }

        fn frame(&mut self) {
            self.root.rebuild(&mut self.logic, &mut self.state);
            self.root.layout(Size::new(400.0, 300.0));
            self.root.paint(&mut NullScene, FrameTime::from_nanos(0));
        }

        fn send(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn transform(&self) -> PanZoomTransform {
            self.controller.transform()
        }
    }

    #[test]
    fn a_pan_inside_a_scroll_view_claims_the_vertical_drag_so_the_outer_defers() {
        let mut h = NestFixture::new();
        // Down at (50, 50): inside the pan_zoom row (0–150), outside the
        // node (x 100–200), so the content ignores it and the view pans.
        h.send(mouse(PointerPhase::Down, 50.0, 50.0));
        // 30px down, past TOUCH_SLOP (18).
        h.send(mouse(PointerPhase::Move, 50.0, 80.0));
        assert_eq!(
            h.transform().offset,
            Vec2::new(0.0, 30.0),
            "the pan applied the drag"
        );
        assert!(
            h.state.scroll_offsets.is_empty(),
            "the outer scroll_view never scrolled: {:?}",
            h.state.scroll_offsets
        );
        h.send(mouse(PointerPhase::Up, 50.0, 80.0));
    }

    #[test]
    fn a_drag_over_a_sibling_outside_pan_zoom_still_scrolls_the_outer() {
        let mut h = NestFixture::new();
        // Down at (50, 200): inside the plain sibling row (150–450 in the
        // column, i.e. 150–300 of the visible viewport at rest), well clear
        // of the pan_zoom row entirely.
        h.send(mouse(PointerPhase::Down, 50.0, 200.0));
        h.send(mouse(PointerPhase::Move, 50.0, 170.0)); // 30px up, past slop: arms the takeover
        h.send(mouse(PointerPhase::Move, 50.0, 160.0)); // the move that actually scrolls
        assert!(
            !h.state.scroll_offsets.is_empty(),
            "the outer scroll_view took the drag as it always has"
        );
        assert_eq!(
            h.transform(),
            PanZoomTransform::IDENTITY,
            "the pan_zoom row was never touched"
        );
        h.send(mouse(PointerPhase::Up, 50.0, 170.0));
    }

    #[test]
    fn a_child_claimed_press_inside_pan_zoom_registers_no_claim_and_the_outer_still_takes_over() {
        let mut h = NestFixture::new();
        // Down at (150, 120): inside the pan_zoom row and inside the node
        // (100–200, 100–200) — the content claims it, so the view does not
        // publish a claim into the outer's cell.
        h.send(mouse(PointerPhase::Down, 150.0, 120.0));
        h.send(mouse(PointerPhase::Move, 150.0, 150.0)); // 30px down, past slop: arms the takeover
        h.send(mouse(PointerPhase::Move, 150.0, 160.0)); // the move that actually scrolls
        assert!(
            !h.state.scroll_offsets.is_empty(),
            "an unregistered claim leaves the outer free to take over, as before"
        );
        assert_eq!(
            h.transform(),
            PanZoomTransform::IDENTITY,
            "the pan_zoom row never panned — its child owned (then lost) the gesture"
        );
        h.send(mouse(PointerPhase::Up, 150.0, 150.0));
    }

    // --- Stale `last_focal`: a dead `Scale` bracket must not leak into a
    //     later lone `Update`. See `apply_scale`'s module doc and the `Scroll`
    //     arm of `Widget::event`. ---

    #[test]
    fn a_scroll_between_scale_events_clears_the_stale_focal_so_a_later_update_anchors_at_itself() {
        let mut h = Harness::new(|v| v);
        // Open a bracket…
        h.send(scale(ScalePhase::Begin, 1.0, 50.0, 50.0));
        // …and let the rest of it arrive as a plain, unmodified wheel Scroll
        // instead of an `End` — the macOS trackpad-pinch-with-released-⌘
        // case — reaching the child since nothing here handles `Scroll`.
        h.send(InputEvent::Scroll {
            position: Point::new(10.0, 10.0),
            delta: ScrollDelta::Lines(0.0, 1.0),
        });
        // A later lone Update — an ordinary, unrelated wheel notch — must
        // anchor at its own focal, not read `last_focal` as the dead
        // bracket's continuation.
        h.send(scale(ScalePhase::Update, 2.0, 300.0, 200.0));
        assert_eq!(h.transform().scale, 2.0);
        assert!(
            close(
                h.transform().to_content(Point::new(300.0, 200.0)),
                Point::new(300.0, 200.0)
            ),
            "the update's own focal content point must stay fixed: {:?}",
            h.transform()
        );
    }
}
