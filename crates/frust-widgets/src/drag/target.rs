//! [`drag_target`]: the drag *target* — a transparent wrapper that registers a
//! [`DragTargetId`] with a [`DragCoordinator`] and turns that id's notifications
//! into typed callbacks plus a themed highlight. See the [module docs](super)
//! for the coordinator's session model and the registry
//! ([`DragCoordinator::register_target`]/[`DragCoordinator::set_target_bounds`]/
//! [`DragCoordinator::target_at`]) this builds on.
//!
//! # Lifecycle
//!
//! [`DragTargetView::build`] allocates a fresh [`DragTargetId`]
//! ([`DragCoordinator::new_target_id`]) and registers it
//! ([`DragCoordinator::register_target`]); [`DragTargetView::teardown`]
//! unregisters it. Every paint reports the widget's window-space bounds
//! ([`DragCoordinator::set_target_bounds`]) so cross-container resolution
//! ([`mod@super`]'s `c-05`) can find this target under the ghost point without
//! hit-testing through the ghost's own `Transparent` pod — `paint`, not layout,
//! because [`frust_core::LayoutCtx`] carries no window-space origin (only
//! [`frust_core::PaintCtx::origin`] does; see [`mod@super::draggable`]'s
//! `window_origin`, recorded the same way).
//!
//! # Driving callbacks from notifications
//!
//! This widget does not [`DragCoordinator::subscribe`] — like
//! [`mod@super::draggable`], it polls the coordinator's own state, here on every
//! [`frust_core::InputEvent::Housekeeping`] broadcast (the vehicle a hover
//! resolution pass's [`DragCoordinator::set_hovered`]/[`DragCoordinator::drop`]
//! reaches this widget through: every coordinator mutation that produces a
//! change raises [`frust_core::mark_pending_result_flush`], so the next rebuild
//! dispatches the broadcast with a real [`frust_core::EventCtx`] — see the
//! [module docs](super) and `crate::gesture`'s *Long-press firing semantics*).
//! Each poll compares the coordinator's hover/drop state against what this
//! target last saw:
//!
//! * **Enter**: hovered, and the held payload is a `T`
//!   ([`DragCoordinator::payload_is`]) — fires [`DragTargetView::on_enter`].
//!   A session whose payload is not `T` is never offered: no callback, no
//!   highlight, regardless of [`DragTargetView::accepts`].
//! * **Hover**: while entered, the latest pointer position
//!   ([`DragSession::pointer`]) converted to this widget's local space — fires
//!   [`DragTargetView::on_hover`] once per distinct position.
//! * **Leave**: entered and no longer hovered (the pointer moved elsewhere, or
//!   the session ended without dropping here) — fires
//!   [`DragTargetView::on_leave`].
//! * **Drop**: [`DragCoordinator::drop`] released over this target
//!   ([`DragState::Dropping`]). [`DragTargetView::accepts`] (default: accept
//!   every `T`) is evaluated against the held payload; accepted,
//!   [`DragCoordinator::take_payload`] then
//!   [`DragTargetView::on_drop`] then [`DragCoordinator::complete_drop`] — no
//!   `on_leave` (the coordinator's own contract: the drop target hears no
//!   `Leave`, it is the drop target). Rejected (wrong type, or `accepts`
//!   false), this target calls [`DragCoordinator::cancel`] itself instead —
//!   `on_drop` never fires — and reports `on_leave` once, since the engagement
//!   ends without a drop.
//!
//! A test driving [`DragCoordinator::set_hovered`]/[`DragCoordinator::drop`]
//! directly (simulating `c-05`'s per-frame resolution) sees exactly the same
//! callbacks a real resolution pass would produce, once its
//! `InputEvent::Housekeeping` is delivered.
//!
//! # Highlight
//!
//! [`DragTargetView::highlight`] overrides the paint seam design systems use to
//! show hover/accept/reject state ([`DragHighlight`]); the signature carries no
//! [`frust_core::PaintCtx`], so this widget paints it inside a
//! [`frust_core::PaintScene::push_transform`]/[`frust_core::PaintScene::push_clip`]
//! pair translated to its own window-space origin — the closure's `(0, 0)` is
//! this widget's top-left corner, mirroring `crate::canvas`'s local-space
//! contract. With no override, the default paints a 2px rounded inset stroke in
//! [`frust_theme::Theme::scheme`]'s `primary` (the `Theme::from_paint_ctx`
//! pattern `crate::slider` uses), or [`HIGHLIGHT_FALLBACK`] unthemed; a
//! [`DragHighlight::Reject`] paints nothing by default — a design system that
//! wants a reject treatment supplies its own closure.

use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::coordinator::{DragCoordinator, DragState, DragTargetId};
use crate::authoring::{ErasedArgCallback, ErasedCallback, TypedArgCallback};

/// Flattening tolerance for the highlight's rounded-rect stroke path (mirrors
/// `crate::container`'s/`crate::button`'s identical precedent).
const BORDER_TOLERANCE: f64 = 0.1;

/// The default highlight's stroke width, in logical px.
const HIGHLIGHT_STROKE_WIDTH: f64 = 2.0;

/// The default highlight's corner radius, in logical px — matches
/// `crate::drag::draggable`'s `PLACEHOLDER_RADIUS` community-approximate value,
/// there being no kit-mined drop-target metric either.
const HIGHLIGHT_RADIUS: f64 = 8.0;

/// The default highlight's stroke color with no theme threaded; themed, it uses
/// [`frust_theme::ColorScheme::primary`] — mirrors `crate::slider`'s
/// `THUMB_FILL`/`FILL` unthemed fallbacks (same token, same role).
pub const HIGHLIGHT_FALLBACK: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);

/// What a [`DragTargetView::highlight`] paint seam is asked to show for a
/// session currently over the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragHighlight {
    /// Hovered by a session whose payload is a `T` and that
    /// [`DragTargetView::accepts`] would accept.
    Hover,
    /// The pointer was released here and the payload is accepted — shown for
    /// the drop itself, the instant before `on_drop` runs.
    Accept,
    /// Hovered (or dropped) by a session [`DragTargetView::accepts`] rejects.
    /// A session whose payload is not `T` at all never reaches the highlight
    /// seam — this variant is for a *type-matching* payload the predicate
    /// still turns down.
    Reject,
}

/// The paint seam a design system overrides with [`DragTargetView::highlight`].
/// See the [module docs](self#highlight) for the local-space contract.
type HighlightPaint = Rc<dyn Fn(&mut dyn PaintScene, Size, DragHighlight)>;

/// A view-held, no-argument app-state callback (`on_enter`/`on_leave`), erased
/// to [`ErasedCallback`] on build — mirrors `crate::list_view`'s
/// `OnNearStart<State>`.
type ChangeCallback<State> = Rc<dyn Fn(&mut State)>;

/// A declarative drop target. See the [module docs](self).
pub struct DragTargetView<State: 'static, T: 'static> {
    child: AnyView<State>,
    coordinator: DragCoordinator,
    accepts: Rc<dyn Fn(&T) -> bool>,
    on_enter: Option<ChangeCallback<State>>,
    on_leave: Option<ChangeCallback<State>>,
    on_hover: Option<TypedArgCallback<State, Point>>,
    on_drop: Option<TypedArgCallback<State, T>>,
    highlight: Option<HighlightPaint>,
}

/// Make `child` a drop target on `coordinator`, accepting payloads of type
/// `T` (specify `T` explicitly — nothing else names it): `drag_target::<Item>(child,
/// coordinator)`. See the [module docs](self) for the registration/notification
/// contract.
pub fn drag_target<T, State, V>(child: V, coordinator: DragCoordinator) -> DragTargetView<State, T>
where
    T: 'static,
    State: 'static,
    V: View<State>,
{
    DragTargetView {
        child: any(child),
        coordinator,
        accepts: Rc::new(|_: &T| true),
        on_enter: None,
        on_leave: None,
        on_hover: None,
        on_drop: None,
        highlight: None,
    }
}

impl<State: 'static, T: 'static> DragTargetView<State, T> {
    /// Only accept a payload `f` returns `true` for (default: accept every
    /// `T`). A payload that fails this shows [`DragHighlight::Reject`] and is
    /// never taken — see the [module docs](self#driving-callbacks-from-notifications).
    pub fn accepts<F: Fn(&T) -> bool + 'static>(mut self, f: F) -> Self {
        self.accepts = Rc::new(f);
        self
    }

    /// Run `f` against the app state when a session carrying a `T` first hovers
    /// this target.
    pub fn on_enter<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_enter = Some(Rc::new(f));
        self
    }

    /// Run `f` against the app state when a hovering `T` session stops hovering
    /// this target without dropping on it.
    pub fn on_leave<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_leave = Some(Rc::new(f));
        self
    }

    /// Run `f` against the app state, with the pointer converted to this
    /// widget's local space, on every distinct pointer position seen while a
    /// `T` session hovers this target.
    pub fn on_hover<F: Fn(&mut State, Point) + 'static>(mut self, f: F) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    /// Run `f` against the app state with the dropped payload when an accepted
    /// `T` session is released on this target.
    pub fn on_drop<F: Fn(&mut State, T) + 'static>(mut self, f: F) -> Self {
        self.on_drop = Some(Rc::new(f));
        self
    }

    /// Override the highlight paint seam (default: a 2px rounded inset stroke
    /// in the theme's primary color). See the [module docs](self#highlight).
    pub fn highlight<F: Fn(&mut dyn PaintScene, Size, DragHighlight) + 'static>(
        mut self,
        f: F,
    ) -> Self {
        self.highlight = Some(Rc::new(f));
        self
    }
}

/// The retained widget for a [`DragTargetView`].
pub struct DragTargetWidget<State: 'static, T: 'static> {
    child: ChildPod,
    coordinator: DragCoordinator,
    id: DragTargetId,
    accepts: Rc<dyn Fn(&T) -> bool>,
    on_enter: Option<ErasedCallback>,
    on_leave: Option<ErasedCallback>,
    on_hover: Option<ErasedArgCallback<Point>>,
    on_drop: Option<ErasedArgCallback<T>>,
    highlight: Option<HighlightPaint>,
    /// This widget's absolute paint origin as of the last paint — what a
    /// coordinator-reported window-space pointer position is converted to
    /// local space with (mirrors `DraggableWidget::window_origin`).
    window_origin: Point,
    /// Whether this target currently considers itself entered: hovered by a
    /// session whose payload is a `T`, regardless of what [`Self::accepts`]
    /// would say — the gate `on_enter`/`on_leave`/`on_hover` are driven by.
    engaged: bool,
    /// The last pointer position delivered to `on_hover`, so an unrelated
    /// notification does not re-fire it for an unchanged position.
    last_hover_pointer: Option<Point>,
    /// Every `State`-carrying field is erased to an `EventCtx`-driven callback
    /// (see [`crate::authoring::erase_callback`]), so nothing else on this
    /// struct names `State` — this is the marker that keeps it a real type
    /// parameter rather than an error.
    _state: std::marker::PhantomData<State>,
}

impl<State: 'static, T: 'static> View<State> for DragTargetView<State, T> {
    type Element = DragTargetWidget<State, T>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        let id = self.coordinator.new_target_id();
        self.coordinator.register_target(id);
        DragTargetWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            coordinator: self.coordinator.clone(),
            id,
            accepts: Rc::clone(&self.accepts),
            on_enter: self.on_enter.as_ref().map(crate::authoring::erase_callback),
            on_leave: self.on_leave.as_ref().map(crate::authoring::erase_callback),
            on_hover: self
                .on_hover
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            on_drop: self
                .on_drop
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            highlight: self.highlight.clone(),
            window_origin: Point::ZERO,
            engaged: false,
            last_hover_pointer: None,
            _state: std::marker::PhantomData,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // The registered id is this pod's for its whole life (allocated once in
        // `build`, released in `teardown`); an app hands every source/target the
        // same coordinator clone for a scope's whole lifetime (see the [module
        // docs](super)), so adopting the latest clone unconditionally here never
        // moves the id to a different registry in practice.
        element.coordinator = self.coordinator.clone();
        element.accepts = Rc::clone(&self.accepts);
        element.on_enter = self.on_enter.as_ref().map(crate::authoring::erase_callback);
        element.on_leave = self.on_leave.as_ref().map(crate::authoring::erase_callback);
        element.on_hover = self
            .on_hover
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        element.on_drop = self
            .on_drop
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        element.highlight = self.highlight.clone();
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        element.coordinator.unregister_target(element.id);
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl<State: 'static, T: 'static> DragTargetWidget<State, T> {
    /// What [`DragHighlight`] to show right now, read purely from the
    /// coordinator's current state (safe at paint — no mutation).
    fn current_highlight(&self) -> Option<DragHighlight> {
        match self.coordinator.state() {
            DragState::Dragging(session) if session.hovered == Some(self.id) => {
                if self.payload_accepted() {
                    Some(DragHighlight::Hover)
                } else if self.coordinator.payload_is::<T>() {
                    Some(DragHighlight::Reject)
                } else {
                    None
                }
            }
            DragState::Dropping { target, .. } if target == self.id => {
                if self.payload_accepted() {
                    Some(DragHighlight::Accept)
                } else {
                    Some(DragHighlight::Reject)
                }
            }
            _ => None,
        }
    }

    /// Whether the coordinator's currently held payload is a `T` this
    /// target's `accepts` predicate accepts.
    fn payload_accepted(&self) -> bool {
        if !self.coordinator.payload_is::<T>() {
            return false;
        }
        let accepts = Rc::clone(&self.accepts);
        self.coordinator
            .with_payload::<T, _>(move |value| accepts(value))
            .unwrap_or(false)
    }

    /// An owner-local position from a coordinator-reported window-space one.
    fn to_local(&self, window: Point) -> Point {
        window - self.window_origin.to_vec2()
    }

    /// Poll the coordinator for this target's id and fire whatever callbacks
    /// its notifications imply since the last poll — see the [module
    /// docs](self#driving-callbacks-from-notifications).
    fn poll(&mut self, ctx: &mut EventCtx<'_>) {
        match self.coordinator.state() {
            DragState::Dragging(session) => {
                let matches =
                    session.hovered == Some(self.id) && self.coordinator.payload_is::<T>();
                if matches && !self.engaged {
                    self.engaged = true;
                    if let Some(cb) = self.on_enter.as_mut() {
                        cb(ctx);
                    }
                } else if !matches && self.engaged {
                    self.engaged = false;
                    self.last_hover_pointer = None;
                    if let Some(cb) = self.on_leave.as_mut() {
                        cb(ctx);
                    }
                }
                if matches {
                    let pointer = session.pointer;
                    if self.last_hover_pointer != Some(pointer) {
                        self.last_hover_pointer = Some(pointer);
                        let local = self.to_local(pointer);
                        if let Some(cb) = self.on_hover.as_mut() {
                            cb(ctx, local);
                        }
                    }
                }
            }
            DragState::Dropping { target, .. } if target == self.id => {
                self.engaged = false;
                self.last_hover_pointer = None;
                if self.payload_accepted() {
                    if let Some(payload) = self.coordinator.take_payload::<T>() {
                        if let Some(cb) = self.on_drop.as_mut() {
                            cb(ctx, *payload);
                        }
                        self.coordinator.complete_drop();
                    } else {
                        // Payload already gone (should not happen — nothing else
                        // takes it while `Dropping`) — leave nothing claimed.
                        self.coordinator.cancel();
                    }
                } else {
                    self.coordinator.cancel();
                    if let Some(cb) = self.on_leave.as_mut() {
                        cb(ctx);
                    }
                }
            }
            _ => {
                if self.engaged {
                    self.engaged = false;
                    self.last_hover_pointer = None;
                    if let Some(cb) = self.on_leave.as_mut() {
                        cb(ctx);
                    }
                }
            }
        }
    }

    /// Paint the current highlight (if any), in the custom seam's local-space
    /// contract — see the [module docs](self#highlight).
    fn paint_highlight(
        &self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        state: DragHighlight,
    ) {
        let origin = ctx.origin();
        let size = ctx.size();
        let theme = Theme::from_paint_ctx(ctx);
        scene.push_transform(Affine::translate(origin.to_vec2()));
        scene.push_clip(Point::ZERO, size);
        match &self.highlight {
            Some(custom) => custom(scene, size, state),
            None => default_highlight(scene, size, state, theme),
        }
        scene.pop_clip();
        scene.pop_transform();
    }
}

/// The highlight seam's default: a 2px rounded inset stroke in the theme's
/// primary color, or [`HIGHLIGHT_FALLBACK`] unthemed. Paints nothing for
/// [`DragHighlight::Reject`] — see the [module docs](self#highlight).
fn default_highlight(
    scene: &mut dyn PaintScene,
    size: Size,
    state: DragHighlight,
    theme: Option<&Theme>,
) {
    if matches!(state, DragHighlight::Reject) {
        return;
    }
    let color = match theme {
        Some(theme) => theme.scheme().primary,
        None => HIGHLIGHT_FALLBACK,
    };
    let half = HIGHLIGHT_STROKE_WIDTH / 2.0;
    let rr = RoundedRect::new(
        half,
        half,
        size.width - half,
        size.height - half,
        (HIGHLIGHT_RADIUS - half).max(0.0),
    );
    let path = rr.to_path(BORDER_TOLERANCE);
    scene.stroke_path(
        Point::ZERO,
        &path,
        HIGHLIGHT_STROKE_WIDTH,
        &Brush::Solid(color),
    );
}

impl<State: 'static, T: 'static> Widget for DragTargetWidget<State, T> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.window_origin = ctx.origin();
        self.coordinator
            .set_target_bounds(self.id, Rect::from_origin_size(ctx.origin(), ctx.size()));
        self.child.paint_child(ctx, scene);
        if let Some(state) = self.current_highlight() {
            self.paint_highlight(ctx, scene, state);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.poll(ctx);
        }
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent wrapper: forward to the single child. The highlight is
        // decorative chrome, not semantic content.
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::super::coordinator::DragPhase;
    use super::*;
    use crate::SizedBox;
    use frust_core::BuildCtx;
    use std::any::Any;
    use std::cell::RefCell;

    #[derive(Default)]
    struct App;

    fn build<S: 'static>(view: &DragTargetView<S, u32>) -> DragTargetWidget<S, u32> {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn housekeeping<S: 'static>(w: &mut DragTargetWidget<S, u32>, state: &mut S) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        w.event(&mut ctx, &InputEvent::Housekeeping)
    }

    fn laid_out<S: 'static>(w: &mut DragTargetWidget<S, u32>, at: Point, size: Size) {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(size));
        let mut scene = crate::test_support::RecordingScene::default();
        let mut pctx = PaintCtx::new(at, size);
        w.paint(&mut pctx, &mut scene);
    }

    #[test]
    fn build_registers_and_teardown_unregisters() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone());
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let mut w = view.build(&mut ctx);
        assert!(coordinator.registered_targets().contains(&w.id));

        view.teardown(&mut w, &mut BuildCtx::new(&mut counter));
        assert!(!coordinator.registered_targets().contains(&w.id));
    }

    #[test]
    fn enter_hover_leave_fire_for_a_matching_payload() {
        let coordinator = DragCoordinator::new();
        let entered = Rc::new(RefCell::new(0u32));
        let left = Rc::new(RefCell::new(0u32));
        let hovered = Rc::new(RefCell::new(Vec::new()));
        let e2 = Rc::clone(&entered);
        let l2 = Rc::clone(&left);
        let h2 = Rc::clone(&hovered);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_enter(move |_s: &mut App| *e2.borrow_mut() += 1)
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1)
                .on_hover(move |_s: &mut App, p: Point| h2.borrow_mut().push(p));
        let mut w = build(&view);
        laid_out(&mut w, Point::new(5.0, 5.0), Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::new(0.0, 0.0));
        coordinator.begin(42u32);
        coordinator.set_hovered(Some(w.id));
        coordinator.update_pointer(Point::new(8.0, 9.0));

        housekeeping(&mut w, &mut state);
        assert_eq!(
            *entered.borrow(),
            1,
            "on_enter fires once for the matching type"
        );
        assert_eq!(
            hovered.borrow().last(),
            Some(&Point::new(3.0, 4.0)),
            "on_hover converts the window-space pointer to local space"
        );

        coordinator.set_hovered(None);
        housekeeping(&mut w, &mut state);
        assert_eq!(*left.borrow(), 1, "on_leave fires once hover ends");
    }

    #[test]
    fn mismatched_payload_type_is_never_offered() {
        let coordinator = DragCoordinator::new();
        let entered = Rc::new(RefCell::new(0u32));
        let e2 = Rc::clone(&entered);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_enter(move |_s: &mut App| *e2.borrow_mut() += 1);
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        // A `&str` payload, not the `u32` this target accepts.
        coordinator.begin("not-a-u32");
        coordinator.set_hovered(Some(w.id));

        housekeeping(&mut w, &mut state);
        assert_eq!(
            *entered.borrow(),
            0,
            "a mismatched payload type must never fire on_enter"
        );
        assert_eq!(
            w.current_highlight(),
            None,
            "a mismatched payload type must never highlight either"
        );
    }

    #[test]
    fn accepts_false_rejects_the_drop() {
        let coordinator = DragCoordinator::new();
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let d2 = Rc::clone(&dropped);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .accepts(|v: &u32| *v < 10)
                .on_drop(move |_s: &mut App, v: u32| d2.borrow_mut().push(v));
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(99u32);
        coordinator.set_hovered(Some(w.id));
        assert_eq!(w.current_highlight(), Some(DragHighlight::Reject));

        coordinator.drop();
        housekeeping(&mut w, &mut state);
        assert!(
            dropped.borrow().is_empty(),
            "a rejected drop must not fire on_drop"
        );
        assert_eq!(coordinator.phase(), DragPhase::Idle);
    }

    #[test]
    fn accepts_true_completes_the_drop() {
        let coordinator = DragCoordinator::new();
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let d2 = Rc::clone(&dropped);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_drop(move |_s: &mut App, v: u32| d2.borrow_mut().push(v));
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(7u32);
        coordinator.set_hovered(Some(w.id));
        assert_eq!(w.current_highlight(), Some(DragHighlight::Hover));

        coordinator.drop();
        assert_eq!(w.current_highlight(), Some(DragHighlight::Accept));
        housekeeping(&mut w, &mut state);
        assert_eq!(*dropped.borrow(), vec![7]);
        assert_eq!(coordinator.phase(), DragPhase::Idle);
    }

    #[test]
    fn paint_reports_window_space_bounds() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(20.0), Some(15.0)), coordinator.clone());
        let mut w = build(&view);
        laid_out(&mut w, Point::new(30.0, 40.0), Size::new(20.0, 15.0));
        assert_eq!(
            coordinator.target_bounds(w.id),
            Some(Rect::from_origin_size(
                Point::new(30.0, 40.0),
                Size::new(20.0, 15.0)
            ))
        );
    }

    #[test]
    fn custom_highlight_overrides_the_default() {
        let coordinator = DragCoordinator::new();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen2 = Rc::clone(&seen);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .accepts(|v: &u32| *v < 10)
                .highlight(move |_scene, _size, state| seen2.borrow_mut().push(state));
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(99u32);
        coordinator.set_hovered(Some(w.id));
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        assert_eq!(*seen.borrow(), vec![DragHighlight::Reject]);
    }
}
