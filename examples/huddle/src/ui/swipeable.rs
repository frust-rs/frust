//! `swipeable_row` — a reusable horizontal swipe-to-action row (Phase C, task 11).
//!
//! A small hand-rolled [`View`]/[`Widget`] pair built directly against
//! `forgekit-core` — the facade's documented "low-level escape hatch" pattern
//! (`forgekit::App::new`, see `crates/forgekit/src/lib.rs`) applied one layer
//! down, the same precedent the pre-skeleton theme/motion screens used for
//! their `ColorBoxView` (`git show d02ad77~1:examples/huddle/src/screens/theme.rs`).
//! No facade widget reveals action areas under a horizontally dragged child, so
//! this widget captures the pointer, tracks a horizontal drag, and paints a
//! left/right action strip behind its child.
//!
//! # Gesture contract
//!
//! It mirrors `ScrollView`'s capture/takeover machinery (see that widget's
//! module docs), rotated to the horizontal axis: it captures the pointer on
//! `Down` and forwards events so a tappable child stays interactive; once the
//! accumulated drag passes [`TOUCH_SLOP`] *horizontally* (and horizontal
//! dominates vertical, so a vertical scroll drag is left to an enclosing
//! `ScrollView`) it sends the child a synthetic `Cancel`, stops forwarding, and
//! consumes the drag itself. On release past [`COMMIT_FRACTION`] it fires the
//! matching action callback (an `Up`, so mutating state is allowed); either way
//! the row springs closed. A `Cancel` (gesture steal) snaps closed and clears
//! its flags without firing — the Cancel-clears-flags-only contract
//! (`docs/CODE_STANDARDS.md`).
//!
//! # Generic API
//!
//! Actions are passed in ([`SwipeableRow::on_swipe_right`]/[`on_swipe_left`]),
//! each an accent color plus an `Fn(&mut State)` callback, so Phase D can reuse
//! the same widget for swipe-to-reply.

use std::rc::Rc;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase,
    SemanticsCtx, TOUCH_SLOP, View, Widget, any,
};
use forgekit_theme::Theme;
use kurbo::{Affine, Point, Size};
use peniko::Color;

/// Fraction of the row width the drag must pass, on release, to commit the
/// action (fire the callback). Below it, the row just springs closed.
const COMMIT_FRACTION: f64 = 0.35;

/// Fraction of the row width past which the reveal is clamped, so the child is
/// never dragged fully off-screen.
const MAX_FRACTION: f64 = 0.75;

/// Per-millisecond retain factor for the spring-closed settle animation: after
/// `dt` ms the remaining distance to closed is scaled by `SETTLE_DECAY.powf(dt)`.
/// Mirrors `ScrollView::SETTLE_DECAY` — an iOS-like snap with no published
/// spring spec to match. **Community-approximate.**
const SETTLE_DECAY: f64 = 0.988;

/// Distance (logical px) below which the settle snaps exactly closed and stops.
const SETTLE_STOP_PX: f64 = 0.5;

/// Side length (logical px) of the white action-marker painted in the revealed
/// strip (a generic icon stand-in — a real vector glyph is a later polish).
const MARKER_PX: f64 = 22.0;

/// Unthemed-fallback marker color (a theme resolves this from `colors.on_surface`,
/// providing contrast on the swipe action strips).
const MARKER_COLOR: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// Resolve the marker color from the theme (defaults to [`MARKER_COLOR`] if unthemed).
fn resolve_marker_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => MARKER_COLOR,
    }
}

/// The affordance glyph painted in a revealed swipe strip. A real Material
/// icon glyph (`icons::REPLY`) can't be shaped from this facade-only escape-hatch
/// widget (glyph shaping lives in `forgekit-text`, unreachable from a raw
/// `forgekit-core` `Widget` — see the module docs), so the reply affordance is a
/// hand-drawn left-pointing arrow rendered with `stroke_line`, delivering the
/// "later polish" the base marker's doc comment deferred.
#[derive(Clone, Copy)]
pub enum SwipeMarker {
    /// A plain rounded-square marker — the generic archive/mute default
    /// (unchanged from the original widget).
    Square,
    /// A left-pointing reply arrow — the swipe-to-reply affordance (task 22).
    Reply,
}

/// A view-held action callback (erased on build).
type Callback<State> = Rc<dyn Fn(&mut State)>;

/// The erased callback shape the widget invokes on commit (mirrors
/// `forgekit-widgets`' own `ErasedCallback`).
type Erased = Box<dyn FnMut(&mut EventCtx)>;

/// One swipe action: an accent color for the revealed strip plus the callback
/// fired when the swipe commits.
struct SwipeAction<State: 'static> {
    color: Color,
    marker: SwipeMarker,
    on_commit: Callback<State>,
}

/// A declarative swipeable row. See the [module docs](self).
pub struct SwipeableRow<State: 'static> {
    child: AnyView<State>,
    /// Revealed on a swipe *right* (finger→right, content shifts right), painted
    /// on the row's left edge.
    swipe_right: Option<SwipeAction<State>>,
    /// Revealed on a swipe *left* (finger→left, content shifts left), painted on
    /// the row's right edge.
    swipe_left: Option<SwipeAction<State>>,
}

/// Wrap `child` in a swipeable row (no actions until one is attached).
pub fn swipeable_row<State: 'static, V: View<State>>(child: V) -> SwipeableRow<State> {
    SwipeableRow {
        child: any(child),
        swipe_right: None,
        swipe_left: None,
    }
}

impl<State: 'static> SwipeableRow<State> {
    /// Attach the swipe-*right* action (revealed on the left edge). `color` fills
    /// the revealed strip; `on_commit` fires on release past [`COMMIT_FRACTION`].
    /// Uses the generic [`SwipeMarker::Square`] affordance.
    pub fn on_swipe_right<F: Fn(&mut State) + 'static>(self, color: Color, on_commit: F) -> Self {
        self.on_swipe_right_marked(color, SwipeMarker::Square, on_commit)
    }

    /// [`on_swipe_right`](Self::on_swipe_right) with an explicit
    /// [`SwipeMarker`] — the swipe-to-reply feed rows pass
    /// [`SwipeMarker::Reply`] (task 22).
    pub fn on_swipe_right_marked<F: Fn(&mut State) + 'static>(
        mut self,
        color: Color,
        marker: SwipeMarker,
        on_commit: F,
    ) -> Self {
        self.swipe_right = Some(SwipeAction {
            color,
            marker,
            on_commit: Rc::new(on_commit),
        });
        self
    }

    /// Attach the swipe-*left* action (revealed on the right edge). Uses the
    /// generic [`SwipeMarker::Square`] affordance.
    pub fn on_swipe_left<F: Fn(&mut State) + 'static>(self, color: Color, on_commit: F) -> Self {
        self.on_swipe_left_marked(color, SwipeMarker::Square, on_commit)
    }

    /// [`on_swipe_left`](Self::on_swipe_left) with an explicit [`SwipeMarker`].
    pub fn on_swipe_left_marked<F: Fn(&mut State) + 'static>(
        mut self,
        color: Color,
        marker: SwipeMarker,
        on_commit: F,
    ) -> Self {
        self.swipe_left = Some(SwipeAction {
            color,
            marker,
            on_commit: Rc::new(on_commit),
        });
        self
    }
}

/// Erase a typed action callback into the `EventCtx`-driven shape (mirrors
/// `forgekit-widgets::erase_callback`).
fn erase<State: 'static>(cb: &Callback<State>) -> Erased {
    let cb = cb.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        cb(state);
    })
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s (double-boxed) element —
/// the same shape `forgekit-widgets::build_child` produces, re-derived here
/// because that helper is crate-private.
fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

/// Reconcile the child through its `ChildPod` (mirrors
/// `forgekit-widgets::rebuild_child`).
fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("swipeable child element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

/// Tear the child down (mirrors `forgekit-widgets::teardown_child`).
fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

/// The retained widget for a [`SwipeableRow`].
pub struct SwipeableWidget {
    child: ChildPod,
    /// Horizontal content offset (px). `> 0` = swiped right, `< 0` = swiped left.
    offset: f64,
    /// This widget's own resolved size.
    size: Size,
    /// A `Down` has armed a gesture (distinct from `dragging`, which only becomes
    /// true once the horizontal slop is crossed).
    down_active: bool,
    /// We have taken the gesture over as a horizontal swipe.
    dragging: bool,
    down_start: Point,
    last: Point,
    /// A spring-closed settle is returning the offset to 0 (driven at paint).
    settling: bool,
    /// Last animation frame time for the settle pump; `None` seeds the clock.
    last_anim: Option<FrameTime>,
    /// Swipe-right (left-edge) action: fill color + erased commit callback.
    right_color: Option<Color>,
    right_marker: SwipeMarker,
    on_swipe_right: Option<Erased>,
    /// Swipe-left (right-edge) action.
    left_color: Option<Color>,
    left_marker: SwipeMarker,
    on_swipe_left: Option<Erased>,
}

impl SwipeableWidget {
    fn new(child: ChildPod) -> Self {
        Self {
            child,
            offset: 0.0,
            size: Size::ZERO,
            down_active: false,
            dragging: false,
            down_start: Point::ZERO,
            last: Point::ZERO,
            settling: false,
            last_anim: None,
            right_color: None,
            right_marker: SwipeMarker::Square,
            on_swipe_right: None,
            left_color: None,
            left_marker: SwipeMarker::Square,
            on_swipe_left: None,
        }
    }

    fn sync_child_origin(&mut self) {
        self.child.set_origin(Point::new(self.offset, 0.0));
    }

    /// The commit distance for the current row width.
    fn commit_px(&self) -> f64 {
        self.size.width * COMMIT_FRACTION
    }

    /// Clamp `offset` into the allowed reveal range, zeroing a direction whose
    /// action is not configured.
    fn clamp_offset(&mut self) {
        let max = self.size.width * MAX_FRACTION;
        self.offset = self.offset.clamp(-max, max);
        if self.offset > 0.0 && self.on_swipe_right.is_none() {
            self.offset = 0.0;
        }
        if self.offset < 0.0 && self.on_swipe_left.is_none() {
            self.offset = 0.0;
        }
    }

    /// Advance the spring-closed settle by the delta since the last paint,
    /// requesting another frame while it is still animating.
    fn pump_settle(&mut self, ctx: &mut PaintCtx) {
        if !self.settling {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        if dt > 0.0 {
            if self.offset.abs() <= SETTLE_STOP_PX {
                self.offset = 0.0;
                self.settling = false;
            } else {
                self.offset *= SETTLE_DECAY.powf(dt);
            }
            self.sync_child_origin();
        }
        if self.settling {
            ctx.request_frame();
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

    /// Paint one revealed action strip and a centered marker.
    fn paint_action(
        scene: &mut dyn PaintScene,
        strip_origin: Point,
        strip: Size,
        color: Color,
        marker: SwipeMarker,
        marker_color: Color,
    ) {
        if strip.width <= 0.0 {
            return;
        }
        scene.fill_rect(strip_origin, strip, color);
        let m = MARKER_PX.min(strip.width).min(strip.height);
        if m <= 0.0 {
            return;
        }
        let marker_origin = Point::new(
            strip_origin.x + (strip.width - m) / 2.0,
            strip_origin.y + (strip.height - m) / 2.0,
        );
        match marker {
            SwipeMarker::Square => {
                scene.fill_rounded_rect(marker_origin, Size::new(m, m), 4.0, marker_color);
            }
            SwipeMarker::Reply => Self::paint_reply_arrow(scene, marker_origin, m, marker_color),
        }
    }

    /// A hand-drawn, left-pointing reply arrow inside the `m`×`m` marker box —
    /// the swipe-to-reply affordance (a real `icons::REPLY` glyph can't be
    /// shaped here, see [`SwipeMarker`]).
    fn paint_reply_arrow(scene: &mut dyn PaintScene, o: Point, m: f64, color: Color) {
        let cy = o.y + m / 2.0;
        let tip = Point::new(o.x + m * 0.22, cy);
        let tail = Point::new(o.x + m * 0.82, cy);
        let head = m * 0.24;
        let w = (m * 0.09).max(1.5);
        // Shaft, then the two arrowhead barbs meeting at the tip.
        scene.stroke_line(tip, tail, w, color);
        scene.stroke_line(tip, Point::new(tip.x + head, tip.y - head), w, color);
        scene.stroke_line(tip, Point::new(tip.x + head, tip.y + head), w, color);
    }
}

impl<State: 'static> View<State> for SwipeableRow<State> {
    type Element = SwipeableWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SwipeableWidget {
        let mut widget = SwipeableWidget::new(build_child(&self.child, ctx));
        widget.right_color = self.swipe_right.as_ref().map(|a| a.color);
        widget.right_marker = self
            .swipe_right
            .as_ref()
            .map_or(SwipeMarker::Square, |a| a.marker);
        widget.on_swipe_right = self.swipe_right.as_ref().map(|a| erase(&a.on_commit));
        widget.left_color = self.swipe_left.as_ref().map(|a| a.color);
        widget.left_marker = self
            .swipe_left
            .as_ref()
            .map_or(SwipeMarker::Square, |a| a.marker);
        widget.on_swipe_left = self.swipe_left.as_ref().map(|a| erase(&a.on_commit));
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SwipeableWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable — reinstall the erased adapters and colors.
        element.right_color = self.swipe_right.as_ref().map(|a| a.color);
        element.right_marker = self
            .swipe_right
            .as_ref()
            .map_or(SwipeMarker::Square, |a| a.marker);
        element.on_swipe_right = self.swipe_right.as_ref().map(|a| erase(&a.on_commit));
        element.left_color = self.swipe_left.as_ref().map(|a| a.color);
        element.left_marker = self
            .swipe_left
            .as_ref()
            .map_or(SwipeMarker::Square, |a| a.marker);
        element.on_swipe_left = self.swipe_left.as_ref().map(|a| erase(&a.on_commit));
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut SwipeableWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SwipeableWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let w = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // Full-width row; the child chooses its own height.
        let child_bc = BoxConstraints::new(Size::new(w, 0.0), Size::new(w, f64::INFINITY));
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.size = Size::new(w, child_size.height);
        self.sync_child_origin();
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.pump_settle(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        let theme = Theme::from_paint_ctx(ctx);
        let marker_color = resolve_marker_color(theme);
        scene.push_clip(origin, size);

        // Reveal the matching action strip behind the (about-to-be-offset) child.
        if self.offset > 0.0
            && let Some(color) = self.right_color
        {
            let strip = Size::new(self.offset.min(size.width), size.height);
            Self::paint_action(scene, origin, strip, color, self.right_marker, marker_color);
        } else if self.offset < 0.0
            && let Some(color) = self.left_color
        {
            let w = (-self.offset).min(size.width);
            let strip_origin = Point::new(origin.x + size.width - w, origin.y);
            Self::paint_action(
                scene,
                strip_origin,
                Size::new(w, size.height),
                color,
                self.left_marker,
                marker_color,
            );
        }

        self.sync_child_origin();
        self.child.paint_child(ctx, scene);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            // Key/Ime/scroll: forward to the child if it holds a recorded path.
            return self.child.event_child(ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                self.down_active = true;
                self.dragging = false;
                self.settling = false;
                self.last_anim = None;
                self.down_start = p.position;
                self.last = p.position;
                ctx.capture_pointer();
                self.child.event_child(ctx, event);
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.down_active {
                    return self.child.event_child(ctx, event);
                }
                if self.dragging {
                    let dx = p.position.x - self.last.x;
                    self.last = p.position;
                    self.offset += dx;
                    self.clamp_offset();
                    self.sync_child_origin();
                    ctx.request_redraw();
                } else {
                    let dx = p.position.x - self.down_start.x;
                    let dy = p.position.y - self.down_start.y;
                    if dx.abs() > TOUCH_SLOP && dx.abs() > dy.abs() {
                        // Horizontal takeover: cancel the child, stop forwarding.
                        self.dragging = true;
                        self.settling = false;
                        self.last = p.position;
                        self.send_child_cancel(ctx, p.position);
                        self.child.set_active(false);
                        ctx.request_redraw();
                    } else {
                        self.child.event_child(ctx, event);
                    }
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.dragging {
                    // Commit past the threshold (an Up — state mutation is allowed).
                    if self.offset > self.commit_px()
                        && let Some(cb) = self.on_swipe_right.as_mut()
                    {
                        cb(ctx);
                    } else if self.offset < -self.commit_px()
                        && let Some(cb) = self.on_swipe_left.as_mut()
                    {
                        cb(ctx);
                    }
                    // Spring closed either way.
                    self.settling = true;
                    self.last_anim = None;
                } else {
                    self.child.event_child(ctx, event);
                }
                self.child.set_active(false);
                self.dragging = false;
                self.down_active = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.child.event_child(ctx, event);
                self.child.set_active(false);
                // Cancel clears flags only: snap closed, fire nothing, touch no state.
                self.dragging = false;
                self.down_active = false;
                self.settling = false;
                self.last_anim = None;
                self.offset = 0.0;
                self.sync_child_origin();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent container: contribute no node of its own, just recurse.
        self.child.semantics_child(ctx);
    }
}

// ---------------------------------------------------------------------------
// press_pop — pressed-state scale-dip micro-interaction (task 22)
// ---------------------------------------------------------------------------

/// The scale a pressed child dips to — a subtle ~0.92 "pop" (spec-less tactile
/// feedback value). **Community-approximate**: no published constant; the
/// small-dip figure most Material/iOS button-press reimplementations converge on.
const PRESS_SCALE: f64 = 0.92;

/// A transparent wrapper that scales its child down to [`PRESS_SCALE`] about its
/// center while the pointer is pressed on it, then springs back on release — the
/// composer send button / emoji-cell micro-interaction (task 22).
///
/// It never captures the pointer or consumes an event: it forwards every phase
/// to the child (so the wrapped [`GestureDetector`](forgekit::GestureDetector)'s
/// own tap still fires, and its capture propagates up unchanged — see
/// `docs/ARCHITECTURE.md`'s Event pipeline), reading the `Down`/`Up`/`Cancel`
/// phases only to toggle the pressed visual. The dip is applied with
/// `PaintScene::push_transform` (the one seam carrying scale, not just a
/// translation), the same escape-hatch capability `hero` morphs paint through.
pub struct PressPop<State: 'static> {
    child: AnyView<State>,
}

/// Wrap `child` so a press scales it to [`PRESS_SCALE`] and a release restores it.
pub fn press_pop<State: 'static, V: View<State>>(child: V) -> PressPop<State> {
    PressPop { child: any(child) }
}

/// The retained widget for a [`PressPop`].
pub struct PressPopWidget {
    child: ChildPod,
    size: Size,
    pressed: bool,
}

impl PressPopWidget {
    fn new(child: ChildPod) -> Self {
        Self {
            child,
            size: Size::ZERO,
            pressed: false,
        }
    }
}

impl<State: 'static> View<State> for PressPop<State> {
    type Element = PressPopWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PressPopWidget {
        PressPopWidget::new(build_child(&self.child, ctx))
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PressPopWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut PressPopWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for PressPopWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.size = size;
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.pressed {
            let origin = ctx.origin();
            let center = Point::new(
                origin.x + self.size.width / 2.0,
                origin.y + self.size.height / 2.0,
            );
            let transform = Affine::translate(center.to_vec2())
                * Affine::scale(PRESS_SCALE)
                * Affine::translate(-center.to_vec2());
            scene.push_transform(transform);
            self.child.paint_child(ctx, scene);
            scene.pop_transform();
        } else {
            self.child.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    self.pressed = true;
                    ctx.request_redraw();
                }
                // Cancel clears the flag only — never touches state (the
                // Cancel-clears-flags-only contract, `docs/CODE_STANDARDS.md`).
                PointerPhase::Up | PointerPhase::Cancel => {
                    if self.pressed {
                        self.pressed = false;
                        ctx.request_redraw();
                    }
                }
                PointerPhase::Move => {}
            }
        }
        // Forward unconditionally so the child's tap fires and its capture
        // propagates up (this wrapper never captures itself).
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A leaf child that records the pointer phases it receives, over `()` state.
    struct Probe {
        cancels: Rc<Cell<u32>>,
    }
    struct ProbeW {
        cancels: Rc<Cell<u32>>,
    }
    impl View<()> for Probe {
        type Element = ProbeW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
            ProbeW {
                cancels: self.cancels.clone(),
            }
        }
        fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 60.0))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e
                && p.phase == PointerPhase::Cancel
            {
                self.cancels.set(self.cancels.get() + 1);
            }
            EventResult::Ignored
        }
    }

    fn laid_out(fired: Rc<Cell<u32>>, cancels: Rc<Cell<u32>>) -> SwipeableWidget {
        let view: SwipeableRow<()> = swipeable_row(Probe { cancels })
            .on_swipe_right(Color::from_rgb8(0, 128, 0), move |_s: &mut ()| {
                fired.set(fired.get() + 1)
            });
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 60.0)));
        w
    }

    fn ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, 30.0),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut SwipeableWidget, event: &InputEvent) {
        let mut unit = ();
        let sa: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.size);
        w.event(&mut ctx, event);
    }

    #[test]
    fn horizontal_drag_past_slop_takes_over_and_cancels_child() {
        let fired = Rc::new(Cell::new(0));
        let cancels = Rc::new(Cell::new(0));
        let mut w = laid_out(fired, cancels.clone());
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0));
        // A horizontal move past the slop takes the gesture over and cancels the
        // child so its own tap/press machinery unwinds.
        dispatch(&mut w, &ev(PointerPhase::Move, 60.0));
        assert!(w.dragging, "a horizontal drag past the slop takes over");
        assert_eq!(cancels.get(), 1, "takeover cancels the child once");
    }

    #[test]
    fn release_past_commit_fires_the_action_then_springs_closed() {
        let fired = Rc::new(Cell::new(0));
        let cancels = Rc::new(Cell::new(0));
        let mut w = laid_out(fired.clone(), cancels);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0)); // takeover
        // Drag well past COMMIT_FRACTION * 200 = 70 px.
        dispatch(&mut w, &ev(PointerPhase::Move, 130.0));
        assert!(w.offset > w.commit_px(), "dragged past the commit distance");
        dispatch(&mut w, &ev(PointerPhase::Up, 130.0));
        assert_eq!(
            fired.get(),
            1,
            "release past the threshold commits the action"
        );
        assert!(w.settling, "a committed row springs closed");
    }

    #[test]
    fn release_below_commit_does_not_fire() {
        let fired = Rc::new(Cell::new(0));
        let cancels = Rc::new(Cell::new(0));
        let mut w = laid_out(fired.clone(), cancels);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0)); // takeover (30 px > slop)
        // Small reveal, under the commit distance.
        dispatch(&mut w, &ev(PointerPhase::Up, 40.0));
        assert_eq!(fired.get(), 0, "a short swipe does not commit");
    }

    #[test]
    fn cancel_snaps_closed_and_fires_nothing() {
        let fired = Rc::new(Cell::new(0));
        let cancels = Rc::new(Cell::new(0));
        let mut w = laid_out(fired.clone(), cancels);
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0)); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 130.0)); // past commit
        dispatch(&mut w, &ev(PointerPhase::Cancel, 130.0));
        assert_eq!(fired.get(), 0, "Cancel never commits");
        assert_eq!(w.offset, 0.0, "Cancel snaps the row closed");
        assert!(!w.dragging && !w.down_active);
    }
}
