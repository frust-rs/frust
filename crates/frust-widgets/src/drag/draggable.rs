//! [`draggable()`]: the drag *source* — a transparent wrapper that turns a press
//! on its child into a [`DragCoordinator`] session, floats a ghost that follows
//! the pointer, and marks the source while the drag is in flight.
//!
//! # Initiation
//!
//! A primary `Down` **arms** the coordinator ([`DragCoordinator::arm`]) and
//! captures the pointer; the press then becomes a drag one of two ways, chosen
//! per press by [`DragPolicy`]:
//!
//! * **distance** ([`DragPolicy::Immediate`]) — the press travels further than
//!   the threshold ([`DraggableView::threshold`], default [`DRAG_THRESHOLD`]);
//! * **hold** ([`DragPolicy::LongPress`]) — the press stays inside
//!   [`TOUCH_SLOP`] for the long-press duration
//!   ([`DraggableView::hold_threshold_ms`], default the same 500 ms every
//!   `GestureDetector` holds for). Moving past the slop first gives the
//!   gesture up — it was a scroll or a swipe, not a drag — and an enclosing
//!   scroll surface is free to take it over.
//!
//! [`DragPolicy::Auto`] (the default) picks by the device the press came from,
//! read off [`EventCtx::pointer_id`] at the `Down`: a
//! [`PointerSource::Mouse`] press (a bare `InputEvent::Pointer`, which is what
//! the desktop shells report for the mouse, the trackpad and a pen) uses the
//! distance threshold, and a [`PointerSource::Touch`] press (an
//! `InputEvent::PointerContact`, which is what the Android, iOS and web shells
//! report for a finger) the long-press — a finger dragging a list item would
//! otherwise steal every scroll.
//!
//! The hold is timed on the paint clock by the same timer the gesture detector
//! uses (`crate::gesture`'s `HoldTracker`), so it fires the way a long-press
//! does: the paint that sees the threshold crossed raises the pending-result
//! flush, and the `Housekeeping` broadcast the next rebuild dispatches carries
//! the `EventCtx` that starts the drag — or a live pointer event does, if one
//! arrives first. See `crate::gesture`'s *Long-press firing semantics* for the
//! full race.
//!
//! Until the drag begins the child sees the whole gesture, so a press that never
//! becomes a drag — a click, a tap, a short touch — still reaches it exactly as
//! it would without the wrapper. When the drag begins the child is handed a
//! `Cancel` and hears nothing more of the gesture.
//!
//! # The session
//!
//! On begin the payload builder runs against the application state and the
//! typed payload is handed to [`DragCoordinator::begin`]. The pointer capture
//! taken on the arming `Down` — the only phase the root mirrors a capture on —
//! keeps every `Move` and the final `Up`/`Cancel` coming here wherever the
//! pointer goes; each `Move` is lifted into window space and reported with
//! [`DragCoordinator::update_pointer`], which also resolves the drop target
//! under it (see the [module docs](super#target-resolution)). An `Up` reports
//! the release point the same way and calls [`DragCoordinator::drop`] — a
//! release outside every target is therefore a cancel — and a `Cancel` calls
//! [`DragCoordinator::cancel`]; both end in [`DraggableWidget`]'s single
//! teardown (`on_session_end`), and the root releases the capture on that same
//! physical `Up`/`Cancel`. A session ended from elsewhere (cancelled, or
//! superseded by an OS file drag) is noticed on the next event or rebuild and
//! torn down the same way, the rest of the gesture swallowed.
//!
//! # Keyboard
//!
//! `Escape` ([`NamedKey::Escape`]) while this source drags — by pointer or by
//! keyboard — cancels the session ([`DragCoordinator::cancel`]) and runs the
//! same teardown; for a pointer drag the pointer is still down, so the rest of
//! the gesture — its `Move`s and its `Up` — is swallowed, never reaching the
//! child as a stray release. For a keyboard drag (below) there is no pending
//! gesture to swallow, so the widget lands back on its idle state at once,
//! ready for another lift.
//!
//! A keyboard session can also end from outside this source entirely — a
//! target completing the drop on its own click, a cancel raised elsewhere —
//! noticed on the next event or rebuild rather than through `Escape` or this
//! source's own drop chord. With no pointer gesture pending either way, that
//! also lands the widget back on idle at once (never the pointer case's
//! swallowing wait for a stray `Up`/`Cancel`), the focus claim left exactly
//! as it is — still focused, ready for another lift.
//!
//! Key events are focus-routed, never routed to a pointer captor, so the source
//! holds the keyboard focus for the length of each press: the arming `Down`
//! requests focus ([`EventCtx::request_focus`]) unless the child took it on
//! that same `Down` (a field inside the source keeps its own focus, and Escape
//! still passes through this wrapper on its way down the focus chain). A `Down`
//! is the only phase on which the root honours a focus claim — a drag begins on
//! a `Move` or a housekeeping broadcast, too late to take it then. Whatever
//! held focus elsewhere before the press is not displaced by this: a `Down`
//! that claims nothing blurs the tree anyway. The claim is released
//! ([`EventCtx::release_focus`]) when a drag ends — by drop, cancel or Escape —
//! restoring the unfocused state the press would otherwise have left; **a
//! plain tap that never became a drag instead keeps the claim**, the one
//! deliberate exception, so a mouse/touch user can click an item and then
//! reach for the keyboard to lift it — the same "click focuses, Space/Enter
//! activates" contract an HTML control honours. An `Up`/`Cancel` release
//! clears the recorded focus path at once, but the root's own focus flag is
//! reconciled only by its next `Down` (the root adjusts its focus bookkeeping
//! on `Down` and keyboard passes, not on a release).
//!
//! While a pointer drags, the enclosing scroll surface's live takeover veto
//! (the one a pinch raises, see `crate::scroll`'s *Multi-contact veto*) is
//! held raised, so a drag inside a `ScrollView`/`ListView` is not stolen once
//! the finger travels past the scroll's own slop. A keyboard drag has no
//! finger to steal from, so this does not apply to it.
//!
//! ## Lift, cycle, drop
//!
//! While this source holds its own focus (not a descendant's —
//! [`ChildPod::holds_live_focus`] on the child reads `false`) and no gesture
//! is in flight, `Enter` or Space (`Key::Character(" ")`, there being no
//! [`NamedKey`] for it) **lifts**: [`DragCoordinator::lift`] then
//! [`DragCoordinator::begin`] with the same payload a pointer press would
//! hand over, the ghost anchored at this source's own window-space bounds
//! (there is no pointer to anchor it to yet). Once lifted, `ArrowRight`/
//! `ArrowDown` and `ArrowLeft`/`ArrowUp` cycle the hovered target forward and
//! backward through [`DragCoordinator::move_to_next_target`]/
//! [`DragCoordinator::move_to_previous_target`], and the ghost's
//! [`OverlayAnchor::Window`] point follows: every `Enter` the cycle raises
//! repositions it to the newly hovered target's own bounds
//! ([`DragCoordinator::target_bounds`]) rather than a pointer position that
//! does not exist. `Enter`/Space while lifted **drops**
//! ([`DragCoordinator::drop`]) on whatever is currently hovered, exactly like
//! a pointer's `Up` — released with nothing hovered, a cancel, like the
//! pointer case. `Escape` cancels from either phase (above).
//!
//! A key this source does not otherwise react to — including every key while
//! its own focus claim is the child's instead — falls through to the child
//! untouched, the same pass-through every other phase gives it.
//!
//! # Ghost and source feedback
//!
//! The ghost floats through an [`OverlaySlot`] — band `Tooltip`, input
//! `Transparent` (it never takes a hit, so what is under the pointer stays
//! reachable), anchored with [`OverlayAnchor::Window`] at the pointer plus the
//! press point's offset into the child, so the grabbed point stays under the
//! finger. Its content is [`DraggableView::ghost`]'s view built from the state
//! at begin, or by default a **snapshot of the child itself**: the child's
//! retained subtree painted a second time, at the ghost's position and the
//! child's size. Either is painted under a [`GHOST_OPACITY`] layer
//! ([`DraggableView::ghost_opacity`]). The pod is mounted by the rebuild that
//! follows begin (begin raises the pending-result flush, so that rebuild runs
//! in the same frame) and torn down by the one after the session ends; it stops
//! being registered — and so painted — the moment the session ends.
//!
//! [`SourceFeedback`] decides what the child looks like in place meanwhile.
//!
//! # Semantics
//!
//! [`Widget::semantics`] pushes a `Role::Button`-equivalent node — accesskit
//! names no drag-specific role — wrapping the child as its one accesskit
//! child, labelled `"Drag"`/`"Drop"` for the two states a session can be in.
//! The richer surface the lift/cycle/drop verbs above would ideally advertise
//! — `accesskit::Action::CustomAction` for `lift`/`drop`/`cancel` individually
//! — is not wired to anything: `frust-core`'s `perform_accessibility_action`
//! (the one seam a shell's `ActionRequest` reaches a widget through) matches
//! only `Action::Click`/`Action::Focus`, dropping every other action and the
//! `ActionRequest::data` a custom action's id would ride in (the same gap
//! `TextInput`'s own advertised-but-uninvocable custom actions document — see
//! `docs/CODE_STANDARDS.md`'s *Text Selection and the Clipboard*). The node
//! still advertises `Action::Click`, as any `Role::Button` does, but it is
//! **a plain activation here, not a lift/drop toggle**: `Action::Click`
//! reaches this widget as a synthesized `Down`+`Up` pair at the node's own
//! bounds center, routed through the normal [`event`] path exactly like a
//! real press, with no marker distinguishing it from one — [`PointerEvent`]
//! and [`EventCtx`] carry no synthetic-origin field to key an honest
//! lift-on-idle/drop-on-live-session reading off (see
//! `perform_accessibility_action`'s doc comment in `frust-core`'s `app.rs`).
//! Routed through that path, the synthesized press-and-release is simply too
//! short to cross the drag threshold — a tap — on an idle source, which
//! reaches the child and (per *Keyboard*, above) leaves the source focused,
//! same as any other plain tap. Arriving while this source drags, either by
//! pointer or by keyboard, it is indistinguishable from an unrelated stray
//! press and so is handled exactly the same way every other press interrupts
//! a session in flight: it cancels the session rather than committing a
//! drop. The keyboard chord — `Enter`/Space to lift and drop,
//! `ArrowRight`/`ArrowDown`/`ArrowLeft`/`ArrowUp` to cycle the hovered
//! target, `Escape` to cancel — is the assistive-technology path through the
//! whole lift-cycle-drop walk; `Action::Click` only ever activates.
//!
//! # Limits
//!
//! * Window-space positions are the child's paint origin plus an owner-local
//!   pointer position, so a source under a transformed ancestor (a `pan_zoom`)
//!   reports — and floats its ghost at — an untransformed position, like any
//!   other [`OverlayAnchor`] consumer.
//! * The default snapshot paints the child's subtree twice a frame unless
//!   [`SourceFeedback::Placeholder`] leaves its rest position empty. Paint-time
//!   side effects a widget inside the child keeps (a nested floated surface, a
//!   published platform view, a recorded window position) see that second
//!   paint too; a child carrying any of those should name a
//!   [`ghost`](DraggableView::ghost) of its own.
//! * Nested draggables are unsupported: both arm on the same `Down` and the
//!   outer arm cancels the inner one.
//! * Every press takes keyboard focus for its length (see *Keyboard*), and a
//!   plain tap that never became a drag keeps it afterward rather than
//!   releasing it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::event::PointerSource;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx, EventResult,
    InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, OverlayBand, OverlayInput, PaintCtx,
    PaintScene, PointerButton, PointerEvent, PointerPhase, SemanticsCtx, TOUCH_SLOP, View, Widget,
    any,
};
use frust_theme::Theme;
use kurbo::{Point, Size, Vec2};
use peniko::Color;

use super::coordinator::{DragCoordinator, DragPhase, DragSourceId, DragState};
use crate::authoring::presses;
use crate::gesture::{HoldTracker, LONG_PRESS_MS};
use crate::overlay::{OverlayAnchor, OverlaySlot};
use crate::scroll::ambient_scroll_veto;

/// How far, in logical px, a press under the distance policy travels before
/// it becomes a drag ([`DraggableView::threshold`] overrides it).
///
/// **Community-approximate**: larger than [`frust_core::MOUSE_SLOP`] (3 px —
/// a click with a little hand tremor must stay a click, since beginning a drag
/// cancels the child's press) and inside the 4–8 px band desktop toolkits ship
/// as their drag threshold (Windows' `SM_CXDRAG` defaults to 4, GTK's
/// `gtk-dnd-drag-threshold` to 8). Distinct from [`TOUCH_SLOP`], which the
/// long-press policy uses as its give-up distance instead.
pub const DRAG_THRESHOLD: f64 = 6.0;

/// The default opacity a ghost is painted at
/// ([`DraggableView::ghost_opacity`] overrides it) — translucent enough that
/// what it hovers over stays legible.
pub const GHOST_OPACITY: f32 = 0.85;

/// The opacity [`SourceFeedback::Dim`] paints the source at in place. No theme
/// token carries a "dragged-away" opacity; this is the disabled-content opacity
/// the baseline button uses for the same "present but not interactive" read.
const DIM_OPACITY: f32 = 0.38;

/// [`SourceFeedback::Placeholder`]'s fill with no theme threaded; themed, it
/// fills with the scheme's `surface_container_highest`.
const PLACEHOLDER_FILL: Color = Color::from_rgba8(0x00, 0x00, 0x00, 0x14);

/// [`SourceFeedback::Placeholder`]'s corner radius with no theme threaded;
/// themed, it uses the shape scale's `small` radius.
const PLACEHOLDER_RADIUS: f64 = 8.0;

/// How a press on a [`draggable()`] becomes a drag. See the [module
/// docs](self#initiation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DragPolicy {
    /// Begin once the press travels past the threshold
    /// ([`DraggableView::threshold`]) — the desktop pointer behaviour.
    Immediate,
    /// Begin once the press is held still (inside [`TOUCH_SLOP`]) for the
    /// hold duration ([`DraggableView::hold_threshold_ms`]); moving past the
    /// slop first gives the gesture up — the touch behaviour.
    LongPress,
    /// [`Immediate`](Self::Immediate) for a mouse press,
    /// [`LongPress`](Self::LongPress) for a touch press, decided per press from
    /// [`EventCtx::pointer_id`]'s source.
    #[default]
    Auto,
}

/// What the source looks like in place while its drag is in flight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceFeedback {
    /// Paint the child dimmed, so the item reads as "being moved".
    #[default]
    Dim,
    /// Paint an empty, themed slot of the child's size instead of the child —
    /// the gap the item will leave.
    Placeholder,
    /// Leave the child exactly as it was.
    None,
}

/// Builds the ghost's content from the application state when a drag begins.
type GhostBuilder<State> = Rc<dyn Fn(&State) -> AnyView<State>>;

/// A declarative drag source. See the [module docs](self).
pub struct DraggableView<State: 'static, T: 'static> {
    child: AnyView<State>,
    coordinator: DragCoordinator,
    payload: Rc<dyn Fn(&State) -> T>,
    policy: DragPolicy,
    threshold: f64,
    hold_threshold_ms: Option<u64>,
    ghost: Option<GhostBuilder<State>>,
    ghost_opacity: f32,
    feedback: SourceFeedback,
}

/// Make `child` a drag source on `coordinator`: a press that becomes a drag
/// (see [`DragPolicy`]) begins a session carrying `payload(state)`.
///
/// The child keeps every press that does not become a drag. The ghost defaults
/// to a translucent snapshot of the child and the source dims in place; see
/// [`DraggableView`]'s builders.
pub fn draggable<State, V, T, F>(
    child: V,
    coordinator: DragCoordinator,
    payload: F,
) -> DraggableView<State, T>
where
    State: 'static,
    V: View<State>,
    T: 'static,
    F: Fn(&State) -> T + 'static,
{
    DraggableView {
        child: any(child),
        coordinator,
        payload: Rc::new(payload),
        policy: DragPolicy::Auto,
        threshold: DRAG_THRESHOLD,
        hold_threshold_ms: None,
        ghost: None,
        ghost_opacity: GHOST_OPACITY,
        feedback: SourceFeedback::Dim,
    }
}

impl<State: 'static, T: 'static> DraggableView<State, T> {
    /// How a press becomes a drag (default [`DragPolicy::Auto`]).
    pub fn policy(mut self, policy: DragPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The distance, in logical px, a press travels before it becomes a drag
    /// under the distance policy (default [`DRAG_THRESHOLD`]). Negative values
    /// clamp to zero.
    pub fn threshold(mut self, px: f64) -> Self {
        self.threshold = px.max(0.0);
        self
    }

    /// How long, in ms, a press is held before it becomes a drag under the
    /// long-press policy (default the gesture detector's long-press duration,
    /// 500 ms). Floored at 1 ms.
    pub fn hold_threshold_ms(mut self, ms: u64) -> Self {
        self.hold_threshold_ms = Some(ms);
        self
    }

    /// Build the ghost from the application state when the drag begins,
    /// instead of snapshotting the child. Sized by its own content, laid out
    /// loosely against the window, and painted at
    /// [`ghost_opacity`](Self::ghost_opacity).
    pub fn ghost<F>(mut self, ghost: F) -> Self
    where
        F: Fn(&State) -> AnyView<State> + 'static,
    {
        self.ghost = Some(Rc::new(ghost));
        self
    }

    /// The opacity the ghost is painted at (default [`GHOST_OPACITY`]),
    /// clamped to `0.0..=1.0`.
    pub fn ghost_opacity(mut self, opacity: f32) -> Self {
        self.ghost_opacity = opacity.clamp(0.0, 1.0);
        self
    }

    /// What the source looks like in place while it is dragged (default
    /// [`SourceFeedback::Dim`]).
    pub fn while_dragging(mut self, feedback: SourceFeedback) -> Self {
        self.feedback = feedback;
        self
    }

    fn resolved_hold_ms(&self) -> f64 {
        self.hold_threshold_ms
            .map(|ms| ms.max(1) as f64)
            .unwrap_or(LONG_PRESS_MS)
    }
}

/// Which way the current press becomes a drag — [`DragPolicy`] resolved
/// against the pressing device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Initiation {
    Distance,
    Hold,
}

/// The source's gesture state machine.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    /// No press in flight.
    Idle,
    /// A primary press armed the coordinator and may still become a drag.
    /// `down`/`last` are owner-local; `elapsed` flips on the paint that sees a
    /// hold cross its threshold (hold initiation only).
    Pressed {
        down: Point,
        last: Point,
        initiation: Initiation,
        elapsed: bool,
    },
    /// The press is the child's: it never armed, or gave up (moved past the
    /// slop before a hold, or lost its arm). The rest of the gesture is routed
    /// to the child as if the wrapper were not there.
    Declined,
    /// This source's session is in flight.
    Dragging,
    /// A *pointer* session ended out from under the gesture (cancelled or
    /// superseded elsewhere); the rest of the still-pressed gesture is
    /// swallowed. A *keyboard* session ended the same way has no pressed
    /// gesture to wait for, so it returns straight to [`Idle`](Self::Idle)
    /// instead — see [`end_elsewhere`](DraggableWidget::end_elsewhere).
    Ended,
}

/// The ghost pod's lifecycle across event and rebuild passes: begin (an event)
/// stages the view, the following rebuild mounts it, and the rebuild after the
/// session ends tears it down.
struct Ghost<State: 'static> {
    /// Built at begin, mounted by the next rebuild.
    pending: Option<AnyView<State>>,
    /// The view currently mounted in the slot — what a teardown diffs against.
    mounted: Option<AnyView<State>>,
    /// Whether the session it belongs to is still in flight: paint registers
    /// the slot only while this holds.
    live: bool,
}

/// The retained widget for a [`DraggableView`].
pub struct DraggableWidget<State: 'static, T: 'static> {
    /// Shared with the default ghost, which paints this same retained subtree.
    child: Rc<RefCell<ChildPod>>,
    coordinator: DragCoordinator,
    payload: Rc<dyn Fn(&State) -> T>,
    policy: DragPolicy,
    threshold: f64,
    ghost_builder: Option<GhostBuilder<State>>,
    ghost_opacity: f32,
    feedback: SourceFeedback,
    gesture: Gesture,
    /// The hold timer for long-press initiation, reset on every arming `Down`.
    hold: HoldTracker,
    /// The id this source armed the current press with — allocated per press
    /// from the coordinator it is armed on, so it is always that coordinator's.
    source: Option<DragSourceId>,
    slot: OverlaySlot<State>,
    ghost: Ghost<State>,
    /// This widget's absolute paint origin as of the last paint: what an
    /// owner-local pointer position is lifted into window space with.
    window_origin: Point,
    /// The enclosing scroll surface's live takeover veto, captured on the
    /// arming `Down` and held raised while dragging.
    scroll_veto: Option<Rc<Cell<bool>>>,
    /// Whether the current press requested keyboard focus for this source (so
    /// `Escape` reaches it) and has not released it yet.
    focus_claimed: bool,
    /// Whether the current [`Gesture::Dragging`] session was begun by
    /// [`keyboard_lift`](Self::keyboard_lift) rather than a pointer press —
    /// set there and by [`begin`](Self::begin), read when a session is
    /// noticed to have ended elsewhere (see [`end_elsewhere`](Self::end_elsewhere)).
    keyboard_drag: bool,
}

impl<State: 'static, T: 'static> View<State> for DraggableView<State, T> {
    type Element = DraggableWidget<State, T>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        let mut slot = OverlaySlot::new();
        slot.set_band(OverlayBand::Tooltip);
        slot.set_input(OverlayInput::Transparent);
        DraggableWidget {
            child: Rc::new(RefCell::new(crate::authoring::build_child(
                &self.child,
                ctx,
            ))),
            coordinator: self.coordinator.clone(),
            payload: Rc::clone(&self.payload),
            policy: self.policy,
            threshold: self.threshold,
            ghost_builder: self.ghost.clone(),
            ghost_opacity: self.ghost_opacity,
            feedback: self.feedback,
            gesture: Gesture::Idle,
            hold: HoldTracker::new(self.resolved_hold_ms()),
            source: None,
            slot,
            ghost: Ghost {
                pending: None,
                mounted: None,
                live: false,
            },
            window_origin: Point::ZERO,
            scroll_veto: None,
            focus_claimed: false,
            keyboard_drag: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // A press in flight keeps the coordinator it armed on; any other time
        // the latest handle is adopted.
        if matches!(element.gesture, Gesture::Idle) {
            element.coordinator = self.coordinator.clone();
        }
        element.payload = Rc::clone(&self.payload);
        element.policy = self.policy;
        element.threshold = self.threshold;
        element.hold.set_threshold_ms(self.resolved_hold_ms());
        element.ghost_builder = self.ghost.clone();
        if element.ghost_opacity != self.ghost_opacity || element.feedback != self.feedback {
            element.ghost_opacity = self.ghost_opacity;
            element.feedback = self.feedback;
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(
            &prev.child,
            &self.child,
            &mut element.child.borrow_mut(),
            ctx,
        );
        flags | element.sync_ghost(ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        // Nothing would ever drop or cancel a session whose source is gone.
        if matches!(element.gesture, Gesture::Pressed { .. } | Gesture::Dragging)
            && element.owns_session()
        {
            element.coordinator.cancel();
        }
        element.gesture = Gesture::Idle;
        element.source = None;
        // The pod is going away, and its focus link with it.
        element.focus_claimed = false;
        element.keyboard_drag = false;
        element.release_scroll_veto();
        element.ghost.live = false;
        element.ghost.pending = None;
        if let Some(mounted) = element.ghost.mounted.take() {
            element.slot.rebuild(Some(&mounted), None, ctx);
        }
        crate::authoring::teardown_child(&self.child, &mut element.child.borrow_mut(), ctx);
    }
}

impl<State: 'static, T: 'static> DraggableWidget<State, T> {
    /// Whether the coordinator's current session (armed or dragging) is the one
    /// this source armed.
    fn owns_session(&self) -> bool {
        let Some(source) = self.source else {
            return false;
        };
        match self.coordinator.state() {
            DragState::Armed { source: armed, .. } => armed == source,
            DragState::Dragging(session) => session.source == Some(source),
            DragState::Idle | DragState::Dropping { .. } => false,
        }
    }

    /// Whether this source's drag is in flight right now — the gesture says so
    /// and the coordinator agrees.
    fn is_dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging)
            && self.coordinator.phase() == DragPhase::Dragging
            && self.owns_session()
    }

    /// An owner-local position in window space.
    fn to_window(&self, local: Point) -> Point {
        local + self.window_origin.to_vec2()
    }

    fn set_scroll_veto(&self, raised: bool) {
        if let Some(veto) = &self.scroll_veto {
            veto.set(raised);
        }
    }

    fn release_scroll_veto(&mut self) {
        self.set_scroll_veto(false);
        self.scroll_veto = None;
    }

    /// A [`Gesture::Dragging`] session noticed to have ended out from under
    /// this source — cancelled or superseded elsewhere, not through this
    /// widget's own end-of-session call ([`on_session_end`](Self::on_session_end)).
    /// A pointer drag still has a physical gesture in flight (its `Move`s and
    /// its `Up`/`Cancel` are still coming): land on [`Gesture::Ended`] and
    /// swallow them, same as before. A keyboard drag has no physical gesture
    /// to wait for — nothing will ever deliver the `Up`/`Cancel` that would
    /// otherwise release it from `Ended` — so it returns straight to `Idle`,
    /// the focus claim it already holds left exactly as it is, ready for
    /// another lift.
    fn end_elsewhere(&mut self) {
        self.gesture = if self.keyboard_drag {
            Gesture::Idle
        } else {
            Gesture::Ended
        };
        self.keyboard_drag = false;
        self.ghost.live = false;
        self.release_scroll_veto();
    }

    /// Mount a staged ghost, or tear down one whose session ended — the
    /// rebuild half of the ghost's lifecycle (see [`Ghost`]).
    fn sync_ghost(&mut self, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        // A session cancelled or superseded elsewhere since the last event:
        // stop showing it now rather than on the next pointer event.
        if matches!(self.gesture, Gesture::Dragging) && !self.is_dragging() {
            self.end_elsewhere();
        }
        let mut flags = ChangeFlags::NONE;
        if let Some(next) = self.ghost.pending.take() {
            if let Some(prev) = self.ghost.mounted.take() {
                flags |= self.slot.rebuild(Some(&prev), None, ctx);
            }
            if self.ghost.live {
                flags |= self.slot.rebuild(None, Some(&next), ctx);
                self.ghost.mounted = Some(next);
            }
        } else if !self.ghost.live
            && let Some(prev) = self.ghost.mounted.take()
        {
            flags |= self.slot.rebuild(Some(&prev), None, ctx);
        }
        flags
    }

    /// Arm the coordinator for a primary `Down` at owner-local `at`.
    fn press(&mut self, ctx: &mut EventCtx<'_>, event: &InputEvent, at: Point) -> EventResult {
        let initiation = match self.policy {
            DragPolicy::Immediate => Initiation::Distance,
            DragPolicy::LongPress => Initiation::Hold,
            DragPolicy::Auto => match ctx.pointer_id().source {
                PointerSource::Mouse => Initiation::Distance,
                PointerSource::Touch => Initiation::Hold,
            },
        };
        let source = self.coordinator.new_source_id();
        if !self.coordinator.arm(source, self.to_window(at)) {
            // A session is already in flight elsewhere: this press is the
            // child's alone.
            self.gesture = Gesture::Declined;
            return crate::authoring::route_event_single(&mut self.child.borrow_mut(), ctx, event);
        }
        self.source = Some(source);
        self.gesture = Gesture::Pressed {
            down: at,
            last: at,
            initiation,
            elapsed: false,
        };
        self.hold.cancel();
        // Captured now, while the enclosing scroll surface's `Down` forward is
        // still on the stack (see the module docs' *The session*).
        self.scroll_veto = ambient_scroll_veto();
        ctx.capture_pointer();
        // Start the hold clock on the next paint.
        ctx.request_redraw();
        let child_took_focus = {
            let mut child = self.child.borrow_mut();
            child.event_child(ctx, event);
            child.holds_live_focus()
        };
        // Hold the keyboard focus for the press so `Escape` reaches this
        // source — unless the child took it, in which case `Escape` passes
        // through here anyway (see the module docs' *Escape*).
        if !child_took_focus {
            ctx.request_focus();
            self.focus_claimed = true;
        }
        EventResult::Handled
    }

    /// Release the keyboard focus the current press claimed, if it did.
    fn release_focus_claim(&mut self, ctx: &mut EventCtx<'_>) {
        if std::mem::take(&mut self.focus_claimed) {
            ctx.release_focus();
        }
    }

    /// Give a press up to the child: drop this source's arm and route the rest
    /// of the gesture through untouched.
    fn decline(&mut self) {
        if self.owns_session() {
            self.coordinator.cancel();
        }
        self.source = None;
        self.release_scroll_veto();
        self.gesture = Gesture::Declined;
    }

    /// Start the drag with the pointer at owner-local `at`, the press having
    /// gone down at `down`. Returns whether a session began.
    fn begin(&mut self, ctx: &mut EventCtx<'_>, at: Point, down: Point) -> bool {
        if !self.owns_session() {
            return false;
        }
        let (payload, custom) = {
            let state: &State = ctx.state_mut::<State>();
            (
                (self.payload)(state),
                self.ghost_builder.as_ref().map(|build| build(state)),
            )
        };
        if !self.coordinator.begin(payload) {
            return false;
        }
        // From the pointer to the child's top-left, so the grabbed point stays
        // under the pointer.
        let offset = Point::ZERO - down;
        let pointer = self.to_window(at);
        self.coordinator.set_ghost_offset(offset);
        self.coordinator.update_pointer(pointer);
        let content = custom.unwrap_or_else(|| {
            any(SnapshotView {
                source: Rc::clone(&self.child),
            })
        });
        self.ghost.pending = Some(any(GhostFrame {
            content,
            opacity: self.ghost_opacity,
        }));
        self.ghost.live = true;
        self.slot.set_anchor(OverlayAnchor::Window {
            point: pointer,
            offset,
        });
        // The gesture is the drag's now: cancel whatever the child made of it.
        {
            let mut child = self.child.borrow_mut();
            child.event_child(ctx, &pointer_event(PointerPhase::Cancel, at));
            ctx.release_captured_child(&mut child);
        }
        self.set_scroll_veto(true);
        self.gesture = Gesture::Dragging;
        self.keyboard_drag = false;
        ctx.request_redraw();
        true
    }

    /// The local teardown every end of a session runs, whichever way it ended
    /// (an `Up`'s drop, a `Cancel`, an `Escape`, a keyboard drop/cancel, or a
    /// session ended elsewhere): the gesture resets, the ghost stops
    /// registering at once and is unmounted by the next rebuild, and the
    /// scroll veto drops. `release_focus` is the caller's call on whether the
    /// press's focus claim goes with it — every ending but a plain tap's
    /// releases it (see the module docs' *Keyboard* section for the one
    /// exception). The coordinator call that ended the session, if any, is
    /// the caller's.
    fn on_session_end(&mut self, ctx: &mut EventCtx<'_>, release_focus: bool) {
        self.gesture = Gesture::Idle;
        self.source = None;
        self.keyboard_drag = false;
        self.hold.cancel();
        self.release_scroll_veto();
        if release_focus {
            self.release_focus_claim(ctx);
        }
        if self.ghost.live || self.ghost.mounted.is_some() {
            self.ghost.live = false;
            frust_core::mark_pending_result_flush();
        }
        ctx.request_redraw();
    }

    fn pointer(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
        p: &PointerEvent,
    ) -> EventResult {
        match (p.phase, self.gesture) {
            (PointerPhase::Down, _) => {
                // A gesture the shell never ended: abandon it before the new
                // press, rather than leave a session nobody will drop.
                if !matches!(self.gesture, Gesture::Idle | Gesture::Declined) {
                    if self.owns_session() {
                        self.coordinator.cancel();
                    }
                    self.on_session_end(ctx, true);
                }
                self.gesture = Gesture::Idle;
                if !presses(p) {
                    return crate::authoring::route_event_single(
                        &mut self.child.borrow_mut(),
                        ctx,
                        event,
                    );
                }
                self.press(ctx, event, p.position)
            }
            (
                PointerPhase::Move,
                Gesture::Pressed {
                    down,
                    initiation,
                    elapsed,
                    ..
                },
            ) => {
                let travel = (p.position - down).hypot();
                let begins = match initiation {
                    Initiation::Distance => travel > self.threshold,
                    Initiation::Hold if travel > TOUCH_SLOP => {
                        self.decline();
                        return self.child.borrow_mut().event_child(ctx, event);
                    }
                    Initiation::Hold => elapsed,
                };
                if begins && self.begin(ctx, p.position, down) {
                    return self.drag_move(ctx, p.position);
                }
                if begins {
                    // The arm was lost before the drag could start.
                    self.decline();
                } else if let Gesture::Pressed { last, .. } = &mut self.gesture {
                    *last = p.position;
                }
                self.child.borrow_mut().event_child(ctx, event);
                EventResult::Handled
            }
            (PointerPhase::Move, Gesture::Dragging) => self.drag_move(ctx, p.position),
            (PointerPhase::Up, Gesture::Pressed { elapsed, .. }) => {
                // Released before it became a drag: a click or tap, the child's.
                // A hold that crossed its threshold but had not started yet
                // (the release beat the deferred start) was not a tap, though.
                if self.owns_session() {
                    self.coordinator.cancel();
                }
                let phase = if elapsed {
                    PointerPhase::Cancel
                } else {
                    PointerPhase::Up
                };
                {
                    let mut child = self.child.borrow_mut();
                    child.event_child(ctx, &pointer_event(phase, p.position));
                    child.set_active(false);
                }
                // A genuine tap (not a hold that almost began) keeps the
                // focus claim, so a following keyboard chord can lift it
                // (the module docs' *Keyboard* section).
                self.on_session_end(ctx, elapsed);
                EventResult::Handled
            }
            (PointerPhase::Cancel, Gesture::Pressed { .. }) => {
                if self.owns_session() {
                    self.coordinator.cancel();
                }
                {
                    let mut child = self.child.borrow_mut();
                    child.event_child(ctx, event);
                    child.set_active(false);
                }
                self.on_session_end(ctx, true);
                EventResult::Handled
            }
            (PointerPhase::Up, Gesture::Dragging) => {
                if self.owns_session() {
                    self.coordinator.update_pointer(self.to_window(p.position));
                    self.coordinator.drop();
                }
                self.on_session_end(ctx, true);
                EventResult::Handled
            }
            (PointerPhase::Cancel, Gesture::Dragging) => {
                if self.owns_session() {
                    self.coordinator.cancel();
                }
                self.on_session_end(ctx, true);
                EventResult::Handled
            }
            (PointerPhase::Up | PointerPhase::Cancel, Gesture::Ended) => {
                self.on_session_end(ctx, true);
                EventResult::Handled
            }
            (PointerPhase::Move, Gesture::Ended) => EventResult::Handled,
            (PointerPhase::Up | PointerPhase::Cancel, Gesture::Declined) => {
                self.gesture = Gesture::Idle;
                self.release_focus_claim(ctx);
                crate::authoring::route_event_single(&mut self.child.borrow_mut(), ctx, event)
            }
            // Hover moves, stray releases, and a declined press's moves.
            _ => crate::authoring::route_event_single(&mut self.child.borrow_mut(), ctx, event),
        }
    }

    /// `Escape` while a *pointer* drags this source: cancel the session and
    /// tear it down, swallowing the rest of the still-pressed gesture.
    fn escape(&mut self, ctx: &mut EventCtx<'_>) -> EventResult {
        if self.owns_session() {
            self.coordinator.cancel();
        }
        self.on_session_end(ctx, true);
        // The pointer is still down: its `Move`s and `Up` are this gesture's
        // and must not reach the child as a fresh one.
        self.gesture = Gesture::Ended;
        EventResult::Handled
    }

    /// Whether this source currently drags a session it began by keyboard
    /// ([`keyboard_lift`](Self::keyboard_lift)) — the gate the lift/cycle/
    /// drop/cancel chords below use to tell a keyboard drag from a pointer
    /// one in flight.
    fn is_keyboard_dragging(&self) -> bool {
        matches!(self.gesture, Gesture::Dragging)
            && self.owns_session()
            && matches!(self.coordinator.state(), DragState::Dragging(session) if session.keyboard)
    }

    /// `Escape` while a *keyboard* drag is live: cancel and tear down. Unlike
    /// [`escape`](Self::escape) there is no still-pressed pointer gesture to
    /// swallow, so the widget lands directly back on `Idle`, ready for
    /// another lift.
    fn keyboard_cancel(&mut self, ctx: &mut EventCtx<'_>) -> EventResult {
        if self.owns_session() {
            self.coordinator.cancel();
        }
        self.on_session_end(ctx, true);
        EventResult::Handled
    }

    /// `Enter`/Space while a keyboard drag is live: drop on whatever
    /// [`move_to_next_target`](DragCoordinator::move_to_next_target)/
    /// [`move_to_previous_target`](DragCoordinator::move_to_previous_target)
    /// left hovered — a cancel if nothing is, exactly like a pointer's `Up`.
    fn keyboard_drop(&mut self, ctx: &mut EventCtx<'_>) -> EventResult {
        if self.owns_session() {
            self.coordinator.drop();
        }
        self.on_session_end(ctx, true);
        EventResult::Handled
    }

    /// `Enter`/Space while this source holds its own focus and nothing is in
    /// flight: lift it (see the module docs' *Lift, cycle, drop*). Returns
    /// `Ignored` without effect when a session is already in flight
    /// elsewhere (nothing here claims it).
    fn keyboard_lift(&mut self, ctx: &mut EventCtx<'_>) -> EventResult {
        let source = self.coordinator.new_source_id();
        if !self.coordinator.lift(source) {
            return EventResult::Ignored;
        }
        self.source = Some(source);
        let (payload, custom) = {
            let state: &State = ctx.state_mut::<State>();
            (
                (self.payload)(state),
                self.ghost_builder.as_ref().map(|build| build(state)),
            )
        };
        if !self.coordinator.begin(payload) {
            self.source = None;
            return EventResult::Ignored;
        }
        let content = custom.unwrap_or_else(|| {
            any(SnapshotView {
                source: Rc::clone(&self.child),
            })
        });
        self.ghost.pending = Some(any(GhostFrame {
            content,
            opacity: self.ghost_opacity,
        }));
        self.ghost.live = true;
        // No pointer yet: the ghost starts over this source's own bounds.
        self.slot.set_anchor(OverlayAnchor::Window {
            point: self.window_origin,
            offset: Vec2::ZERO,
        });
        self.gesture = Gesture::Dragging;
        self.keyboard_drag = true;
        ctx.request_focus();
        self.focus_claimed = true;
        ctx.request_redraw();
        EventResult::Handled
    }

    /// Reposition the ghost over whatever is now hovered, after a keyboard
    /// cycle step — the keyboard counterpart of [`drag_move`](Self::drag_move)
    /// moving it to the pointer. A no-op with nothing hovered (every target
    /// cycled past, or none registered).
    fn follow_hovered_target(&mut self, ctx: &mut EventCtx<'_>) {
        let DragState::Dragging(session) = self.coordinator.state() else {
            return;
        };
        let Some(target) = session.hovered else {
            return;
        };
        let Some(bounds) = self.coordinator.target_bounds(target) else {
            return;
        };
        self.slot.set_window_anchor(ctx, bounds.origin());
    }

    /// The lift/cycle/drop chord this source answers while it — not a
    /// descendant — holds the live focus (see the module docs' *Lift, cycle,
    /// drop*). `None` falls through to the child: not our chord, a session
    /// (pointer or keyboard) owned elsewhere, or the child's own focus claim
    /// takes keys first.
    fn keyboard_chord(&mut self, ctx: &mut EventCtx<'_>, key: &KeyEvent) -> Option<EventResult> {
        if !ctx.has_focus() || self.child.borrow().holds_live_focus() {
            return None;
        }
        let activates = key.key == Key::Named(NamedKey::Enter)
            || matches!(&key.key, Key::Character(s) if s == " ");
        if matches!(self.gesture, Gesture::Idle) {
            return activates.then(|| self.keyboard_lift(ctx));
        }
        if !self.is_keyboard_dragging() {
            return None;
        }
        match &key.key {
            Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowDown) => {
                self.coordinator.move_to_next_target();
                self.follow_hovered_target(ctx);
                ctx.request_redraw();
                Some(EventResult::Handled)
            }
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowUp) => {
                self.coordinator.move_to_previous_target();
                self.follow_hovered_target(ctx);
                ctx.request_redraw();
                Some(EventResult::Handled)
            }
            _ if activates => Some(self.keyboard_drop(ctx)),
            _ => None,
        }
    }

    /// A `Move` while this source drags: report it and move the ghost.
    fn drag_move(&mut self, ctx: &mut EventCtx<'_>, at: Point) -> EventResult {
        if !self.is_dragging() {
            // Ended elsewhere since the last event.
            self.end_elsewhere();
            frust_core::mark_pending_result_flush();
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let pointer = self.to_window(at);
        self.coordinator.update_pointer(pointer);
        self.slot.set_window_anchor(ctx, pointer);
        ctx.set_cursor(CursorIcon::Grabbing);
        EventResult::Handled
    }

    /// Paint the child in place, under whatever [`SourceFeedback`] the
    /// dragging state calls for.
    fn paint_source(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let feedback = if self.is_dragging() {
            self.feedback
        } else {
            SourceFeedback::None
        };
        let mut child = self.child.borrow_mut();
        match feedback {
            SourceFeedback::None => child.paint_child(ctx, scene),
            SourceFeedback::Dim => {
                scene.push_layer(ctx.origin(), ctx.size(), DIM_OPACITY);
                child.paint_child(ctx, scene);
                scene.pop_layer();
            }
            SourceFeedback::Placeholder => {
                let (fill, radius) = match Theme::from_paint_ctx(ctx) {
                    Some(theme) => (theme.scheme().surface_container_highest, theme.shape.small),
                    None => (PLACEHOLDER_FILL, PLACEHOLDER_RADIUS),
                };
                scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
            }
        }
    }
}

impl<State: 'static, T: 'static> Widget for DraggableWidget<State, T> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = {
            let mut child = self.child.borrow_mut();
            let size = child.layout_child(ctx, bc);
            child.set_origin(Point::ZERO);
            size
        };
        // After the child, so a snapshot ghost reads the child's fresh size.
        self.slot.layout(ctx);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.window_origin = ctx.origin();
        // The hold clock (see `crate::gesture`'s long-press firing semantics):
        // the paint that sees the threshold crossed latches the flush whose
        // `Housekeeping` broadcast starts the drag, and asks for the frame that
        // reaches it; until then, keep frames coming so the clock advances.
        if let Gesture::Pressed {
            initiation: Initiation::Hold,
            elapsed,
            ..
        } = &mut self.gesture
            && !*elapsed
        {
            if self.hold.fired(ctx.frame_time()) {
                *elapsed = true;
                frust_core::mark_pending_result_flush();
            }
            ctx.request_frame();
        }
        self.paint_source(ctx, scene);
        if self.ghost.live {
            let size = ctx.size();
            self.slot.paint(ctx, size);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            // A hold that crossed its threshold starts here when no pointer
            // event beat the deferred flush to it.
            if let Gesture::Pressed {
                initiation: Initiation::Hold,
                elapsed: true,
                down,
                last,
            } = self.gesture
                && !self.begin(ctx, last, down)
            {
                self.decline();
            }
            self.child.borrow_mut().event_child(ctx, event);
            return EventResult::Ignored;
        }
        if let InputEvent::Pointer(p) = event {
            return self.pointer(ctx, event, p);
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape) && matches!(self.gesture, Gesture::Dragging)
            {
                return if self.is_keyboard_dragging() {
                    self.keyboard_cancel(ctx)
                } else {
                    self.escape(ctx)
                };
            }
            if let Some(result) = self.keyboard_chord(ctx, key) {
                return result;
            }
        }
        crate::authoring::route_event_single(&mut self.child.borrow_mut(), ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // No accesskit role names a drag source; `Role::Button` is the
        // closest activatable analog (see the module docs' *Semantics*
        // section for why `Click` — not a custom `lift`/`drop`/`cancel`
        // action — is what this node advertises).
        let dragging = self.is_dragging();
        ctx.push_container(
            Role::Button,
            |node| {
                node.add_action(Action::Click);
                node.set_label(if dragging { "Drop" } else { "Drag" });
            },
            |ctx| self.child.borrow().semantics_child(ctx),
        );
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.child.borrow());
    }
}

/// A synthesized pointer event at owner-local `at`.
fn pointer_event(phase: PointerPhase, at: Point) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: at,
        button: PointerButton::Primary,
    })
}

// ---------------------------------------------------------------------------
// The ghost
// ---------------------------------------------------------------------------

/// The ghost pod's root: its content painted under one opacity layer.
struct GhostFrame<State: 'static> {
    content: AnyView<State>,
    opacity: f32,
}

struct GhostFrameWidget {
    content: ChildPod,
    opacity: f32,
}

impl<State: 'static> View<State> for GhostFrame<State> {
    type Element = GhostFrameWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GhostFrameWidget {
        GhostFrameWidget {
            content: crate::authoring::build_child(&self.content, ctx),
            opacity: self.opacity,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GhostFrameWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.opacity = self.opacity;
        crate::authoring::rebuild_child(&prev.content, &self.content, &mut element.content, ctx)
    }

    fn teardown(&self, element: &mut GhostFrameWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for GhostFrameWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.content.layout_child(ctx, bc);
        self.content.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.opacity >= 1.0 {
            self.content.paint_child(ctx, scene);
            return;
        }
        scene.push_layer(ctx.origin(), ctx.size(), self.opacity);
        self.content.paint_child(ctx, scene);
        scene.pop_layer();
    }

    crate::authoring::visit_children!(content);
}

/// The default ghost content: the source's own retained child, painted again at
/// the ghost's position and the child's laid-out size.
struct SnapshotView {
    source: Rc<RefCell<ChildPod>>,
}

struct SnapshotWidget {
    source: Rc<RefCell<ChildPod>>,
}

impl<State: 'static> View<State> for SnapshotView {
    type Element = SnapshotWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SnapshotWidget {
        SnapshotWidget {
            source: Rc::clone(&self.source),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut SnapshotWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.source = Rc::clone(&self.source);
        ChangeFlags::NONE
    }
}

impl Widget for SnapshotWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The source lays itself out in the main tree; the snapshot only takes
        // its size, never re-runs its layout against the window.
        bc.constrain(self.source.borrow().size())
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The source pod sits at its owner's origin, so painting it from here
        // lands it at the ghost's. Skipped rather than panicking should the
        // source ever be mid-paint itself.
        if let Ok(mut source) = self.source.try_borrow_mut() {
            source.paint_child(ctx, scene);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingScene;
    use crate::{EdgeInsets, Padding, Stack, StackView};
    use frust_core::event::PointerId;
    use frust_core::{FrameTime, RenderRoot};
    use kurbo::Rect;

    use crate::drag::{DragStateChange, DragTargetId};

    const WINDOW: Size = Size::new(400.0, 600.0);
    /// Where the draggable sits in the window.
    const AT: Vec2 = Vec2::new(10.0, 100.0);
    const CHILD: Size = Size::new(100.0, 40.0);

    type Log = Rc<RefCell<Vec<String>>>;

    #[derive(Clone, Copy)]
    struct Cfg {
        policy: DragPolicy,
        feedback: SourceFeedback,
        custom_ghost: bool,
        /// Host the source inside a tall `ScrollView`.
        in_scroll: bool,
        /// The child requests keyboard focus on its own primary `Down`.
        focusable_child: bool,
    }

    impl Default for Cfg {
        fn default() -> Self {
            Cfg {
                policy: DragPolicy::Auto,
                feedback: SourceFeedback::Dim,
                custom_ghost: false,
                in_scroll: false,
                focusable_child: false,
            }
        }
    }

    struct App {
        cfg: Cfg,
        coordinator: DragCoordinator,
        log: Log,
        taps: u32,
        item: u32,
    }

    /// A button-like leaf: paints its rect, records every pointer event in
    /// its own space (and every key it is routed), captures on a primary
    /// `Down` — optionally taking focus with it — and counts an in-bounds `Up`
    /// as a tap.
    struct Probe {
        size: Size,
        log: Log,
        focusable: bool,
    }

    struct ProbeWidget {
        size: Size,
        log: Log,
        focusable: bool,
    }

    impl View<App> for Probe {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget {
                size: self.size,
                log: Rc::clone(&self.log),
                focusable: self.focusable,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.size = self.size;
            element.log = Rc::clone(&self.log);
            element.focusable = self.focusable;
            ChangeFlags::NONE
        }
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Key(key) = event {
                self.log.borrow_mut().push(format!("Key {:?}", key.key));
                return EventResult::Handled;
            }
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            self.log
                .borrow_mut()
                .push(format!("{:?}@{},{}", p.phase, p.position.x, p.position.y));
            match p.phase {
                PointerPhase::Down => {
                    ctx.capture_pointer();
                    if self.focusable && presses(p) {
                        ctx.request_focus();
                    }
                }
                PointerPhase::Up => {
                    let inside =
                        Rect::from_origin_size(Point::ZERO, ctx.size()).contains(p.position);
                    if inside {
                        ctx.state_mut::<App>().taps += 1;
                    }
                }
                PointerPhase::Move | PointerPhase::Cancel => {}
            }
            EventResult::Handled
        }
    }

    fn logic(state: &mut App) -> StackView<App> {
        let cfg = state.cfg;
        let mut source = draggable(
            Probe {
                size: CHILD,
                log: Rc::clone(&state.log),
                focusable: cfg.focusable_child,
            },
            state.coordinator.clone(),
            |state: &App| state.item,
        )
        .policy(cfg.policy)
        .while_dragging(cfg.feedback);
        if cfg.custom_ghost {
            source = source.ghost(|_: &App| any(crate::SizedBox(Some(30.0), Some(20.0))));
        }
        let placed = any(Padding(
            EdgeInsets {
                left: AT.x,
                top: AT.y,
                right: 0.0,
                bottom: 0.0,
            },
            source,
        ));
        if cfg.in_scroll {
            return Stack(vec![any(crate::scroll_view(Stack(vec![
                any(crate::SizedBox(Some(WINDOW.width), Some(2_000.0))),
                placed,
            ])))]);
        }
        Stack(vec![placed])
    }

    struct Harness {
        root: RenderRoot<App, StackView<App>>,
        state: App,
        clock_ms: f64,
        changes: Rc<RefCell<Vec<DragStateChange>>>,
        _subscription: crate::drag::DragSubscription,
    }

    impl Harness {
        fn new(cfg: Cfg) -> Self {
            let coordinator = DragCoordinator::new();
            let changes = Rc::new(RefCell::new(Vec::new()));
            let sink = Rc::clone(&changes);
            let subscription = coordinator.subscribe(move |change| sink.borrow_mut().push(change));
            let mut harness = Harness {
                root: RenderRoot::new(),
                state: App {
                    cfg,
                    coordinator,
                    log: Rc::new(RefCell::new(Vec::new())),
                    taps: 0,
                    item: 7,
                },
                clock_ms: 0.0,
                changes,
                _subscription: subscription,
            };
            harness.frame();
            harness
        }

        /// One whole frame — rebuild, layout, paint — returning the scene.
        fn frame(&mut self) -> RecordingScene {
            let mut build: fn(&mut App) -> StackView<App> = logic;
            self.root.rebuild(&mut build, &mut self.state);
            self.root.layout(WINDOW);
            self.clock_ms += 16.0;
            let mut scene = RecordingScene::default();
            self.root.paint(
                &mut scene,
                FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
            );
            scene
        }

        fn mouse(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &pointer_event(phase, Point::new(x, y)));
        }

        fn touch(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::PointerContact {
                    pointer_id: PointerId::touch(0),
                    event: PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: PointerButton::Primary,
                    },
                },
            );
        }

        fn phase(&self) -> DragPhase {
            self.state.coordinator.phase()
        }

        fn key(&mut self, key: Key) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(frust_core::KeyEvent {
                    key,
                    modifiers: frust_core::Modifiers::default(),
                    repeat: false,
                }),
            );
        }

        fn escape(&mut self) {
            self.key(Key::Named(NamedKey::Escape));
        }

        /// Register a drop target with window-space `bounds`, as a painted
        /// target would.
        fn target(&mut self, bounds: Rect) -> DragTargetId {
            let drag = &self.state.coordinator;
            let id = drag.new_target_id();
            drag.register_target(id);
            drag.set_target_bounds(id, bounds);
            id
        }

        fn log(&self) -> Vec<String> {
            self.state.log.borrow().clone()
        }

        /// Press at window `(30, 120)` — `(20, 20)` inside the child — and
        /// drag past the threshold to `(60, 150)`.
        fn mouse_drag(&mut self) {
            self.mouse(PointerPhase::Down, 30.0, 120.0);
            self.mouse(PointerPhase::Move, 60.0, 150.0);
        }
    }

    fn rects(scene: &RecordingScene) -> Vec<Rect> {
        scene
            .rects
            .iter()
            .map(|(origin, size)| Rect::from_origin_size(*origin, *size))
            .collect()
    }

    fn rest() -> Rect {
        Rect::from_origin_size(Point::ZERO + AT, CHILD)
    }

    #[test]
    fn a_mouse_drag_begins_past_the_threshold_and_not_before() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        assert_eq!(h.phase(), DragPhase::Armed, "a primary press arms");
        h.mouse(PointerPhase::Move, 34.0, 122.0);
        assert_eq!(
            h.phase(),
            DragPhase::Armed,
            "4.5 px is inside the 6 px threshold"
        );
        h.mouse(PointerPhase::Move, 37.0, 120.0);
        assert_eq!(h.phase(), DragPhase::Dragging, "7 px crosses it");
        assert_eq!(
            h.state.coordinator.with_payload(|item: &u32| *item),
            Some(7),
            "the payload builder ran against the state at begin"
        );
        let session = h.state.coordinator.state();
        let session = session.session().expect("a session is in flight");
        assert_eq!(session.pointer, Point::new(37.0, 120.0));
        assert_eq!(session.ghost_offset, Vec2::new(-20.0, -20.0));
    }

    #[test]
    fn a_short_click_passes_through_to_the_child() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Move, 32.0, 121.0);
        h.mouse(PointerPhase::Up, 32.0, 121.0);
        assert_eq!(h.state.taps, 1, "the child saw the whole click");
        assert_eq!(h.log(), vec!["Down@20,20", "Move@22,21", "Up@22,21"]);
        assert_eq!(h.phase(), DragPhase::Idle, "the arm was released");
        assert!(!h.root.is_pointer_captured());
    }

    #[test]
    fn a_short_tap_on_touch_passes_through_to_the_child() {
        let mut h = Harness::new(Cfg::default());
        h.touch(PointerPhase::Down, 30.0, 120.0);
        h.frame();
        h.touch(PointerPhase::Up, 30.0, 120.0);
        assert_eq!(h.state.taps, 1);
        assert_eq!(h.phase(), DragPhase::Idle);
    }

    #[test]
    fn beginning_a_drag_cancels_the_childs_press() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        h.mouse(PointerPhase::Up, 60.0, 150.0);
        assert_eq!(h.state.taps, 0, "a drag is never also a tap");
        assert_eq!(h.log(), vec!["Down@20,20", "Cancel@50,50"]);
    }

    #[test]
    fn a_touch_long_press_begins_the_drag() {
        let mut h = Harness::new(Cfg::default());
        h.touch(PointerPhase::Down, 30.0, 120.0);
        // Drift inside the touch slop doesn't count as travel: a 10 px move
        // would have begun a mouse drag.
        h.touch(PointerPhase::Move, 40.0, 120.0);
        assert_eq!(h.phase(), DragPhase::Armed);
        let mut frames = 0;
        while h.phase() != DragPhase::Dragging {
            h.frame();
            frames += 1;
            assert!(frames < 60, "the hold never began a drag");
        }
        // 16 ms frames: the threshold is crossed on the paint ~500 ms in, and
        // the next frame's Housekeeping flush starts the drag.
        assert!(
            frames >= 31,
            "began after {frames} frames — before the hold elapsed"
        );
        assert_eq!(
            h.state.coordinator.state().session().map(|s| s.pointer),
            Some(Point::new(40.0, 120.0)),
            "the drag starts where the finger is held"
        );
        assert_eq!(h.log().last().map(String::as_str), Some("Cancel@30,20"));
    }

    #[test]
    fn an_enclosing_scroll_view_does_not_steal_a_drag_in_flight() {
        let mut h = Harness::new(Cfg {
            in_scroll: true,
            ..Cfg::default()
        });
        h.touch(PointerPhase::Down, 30.0, 120.0);
        let mut frames = 0;
        while h.phase() != DragPhase::Dragging {
            h.frame();
            frames += 1;
            assert!(frames < 60, "the hold never began a drag");
        }
        // Well past the scroll view's own touch slop.
        h.touch(PointerPhase::Move, 30.0, 220.0);
        h.touch(PointerPhase::Move, 30.0, 320.0);
        assert_eq!(
            h.phase(),
            DragPhase::Dragging,
            "the scroll view took nothing over"
        );
        assert_eq!(
            h.state.coordinator.state().session().map(|s| s.pointer),
            Some(Point::new(30.0, 320.0))
        );
        let scene = h.frame();
        assert_eq!(
            rects(&scene).first(),
            Some(&rest()),
            "the content did not scroll under the drag"
        );
        h.touch(PointerPhase::Up, 30.0, 320.0);
        assert_eq!(h.phase(), DragPhase::Idle);
    }

    #[test]
    fn a_touch_press_that_moves_before_the_hold_is_the_childs() {
        let mut h = Harness::new(Cfg::default());
        h.touch(PointerPhase::Down, 30.0, 120.0);
        h.frame();
        h.touch(PointerPhase::Move, 30.0, 160.0);
        assert_eq!(
            h.phase(),
            DragPhase::Idle,
            "travel past the slop gives it up"
        );
        for _ in 0..40 {
            h.frame();
        }
        assert_eq!(h.phase(), DragPhase::Idle, "no later hold revives it");
        h.touch(PointerPhase::Up, 30.0, 160.0);
        assert_eq!(h.log(), vec!["Down@20,20", "Move@20,60", "Up@20,60"]);
    }

    #[test]
    fn an_explicit_policy_overrides_the_device() {
        let mut h = Harness::new(Cfg {
            policy: DragPolicy::LongPress,
            ..Cfg::default()
        });
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Move, 40.0, 120.0);
        assert_eq!(h.phase(), DragPhase::Armed, "a long-press mouse drag waits");

        let mut h = Harness::new(Cfg {
            policy: DragPolicy::Immediate,
            ..Cfg::default()
        });
        h.touch(PointerPhase::Down, 30.0, 120.0);
        h.touch(PointerPhase::Move, 40.0, 120.0);
        assert_eq!(
            h.phase(),
            DragPhase::Dragging,
            "an immediate touch drag doesn't"
        );
    }

    #[test]
    fn the_ghost_opens_and_closes_with_the_session() {
        let mut h = Harness::new(Cfg::default());
        let idle = h.frame();
        assert_eq!(rects(&idle), vec![rest()], "no ghost before a drag");

        h.mouse_drag();
        let dragging = h.frame();
        // Pointer (60, 150) minus the (20, 20) grab point: the snapshot of the
        // child, at the child's size, under the ghost's opacity layer.
        let ghost = Rect::from_origin_size(Point::new(40.0, 130.0), CHILD);
        assert_eq!(rects(&dragging), vec![rest(), ghost]);
        assert!(
            dragging
                .layers
                .contains(&(ghost.origin(), CHILD, GHOST_OPACITY)),
            "{:?}",
            dragging.layers
        );

        h.mouse(PointerPhase::Move, 200.0, 400.0);
        let moved = h.frame();
        assert_eq!(
            rects(&moved).last().copied(),
            Some(Rect::from_origin_size(Point::new(180.0, 380.0), CHILD)),
            "the ghost follows the pointer"
        );

        h.mouse(PointerPhase::Up, 200.0, 400.0);
        let ended = h.frame();
        assert_eq!(
            rects(&ended),
            vec![rest()],
            "the ghost closed with the session"
        );
        assert!(ended.layers.is_empty());
    }

    #[test]
    fn the_pointer_stays_captured_for_the_whole_drag() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        h.frame();
        assert!(h.root.is_pointer_captured());
        // Far outside the source: the capture still delivers it here.
        h.mouse(PointerPhase::Move, 390.0, 590.0);
        assert_eq!(
            h.state.coordinator.state().session().map(|s| s.pointer),
            Some(Point::new(390.0, 590.0))
        );
        assert!(h.root.is_pointer_captured());
        h.mouse(PointerPhase::Up, 390.0, 590.0);
        assert!(!h.root.is_pointer_captured(), "released on the physical Up");
    }

    #[test]
    fn releasing_over_a_target_drops_on_it() {
        let mut h = Harness::new(Cfg::default());
        let target = h.target(Rect::new(70.0, 150.0, 170.0, 250.0));
        h.mouse_drag();
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            None,
            "(60, 150) is outside the target"
        );
        h.mouse(PointerPhase::Move, 75.0, 155.0);
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            Some(target),
            "the move resolved the target under the pointer"
        );
        h.mouse(PointerPhase::Up, 80.0, 160.0);
        match h.state.coordinator.state() {
            DragState::Dropping { session, target: t } => {
                assert_eq!(t, target);
                assert_eq!(session.pointer, Point::new(80.0, 160.0));
            }
            other => panic!("expected a drop, got {other:?}"),
        }
    }

    #[test]
    fn releasing_outside_every_target_cancels() {
        let mut h = Harness::new(Cfg::default());
        let target = h.target(Rect::new(70.0, 150.0, 170.0, 250.0));
        h.mouse_drag();
        h.mouse(PointerPhase::Move, 75.0, 155.0);
        // Released just outside the target, off the hover it had.
        h.mouse(PointerPhase::Up, 200.0, 160.0);
        assert_eq!(h.phase(), DragPhase::Idle);
        let changes = h.changes.borrow();
        let tail: Vec<_> = changes.iter().rev().take(4).rev().copied().collect();
        assert_eq!(
            tail,
            vec![
                DragStateChange::Leave { target },
                DragStateChange::Move {
                    pointer: Point::new(200.0, 160.0)
                },
                DragStateChange::Phase {
                    previous: DragPhase::Dragging,
                    next: DragPhase::Cancelled,
                },
                DragStateChange::Phase {
                    previous: DragPhase::Cancelled,
                    next: DragPhase::Idle,
                },
            ]
        );
    }

    #[test]
    fn escape_cancels_the_drag_and_restores_focus() {
        let mut h = Harness::new(Cfg::default());
        assert!(
            !h.root.is_focus_active(),
            "nothing focused before the press"
        );
        let target = h.target(Rect::new(0.0, 0.0, 400.0, 600.0));
        h.mouse_drag();
        h.frame();
        assert!(
            h.root.is_focus_active(),
            "the press holds focus for the source"
        );
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            Some(target)
        );
        h.escape();
        assert_eq!(h.phase(), DragPhase::Idle, "Escape cancelled the session");
        assert!(h.changes.borrow().contains(&DragStateChange::Phase {
            previous: DragPhase::Dragging,
            next: DragPhase::Cancelled,
        }));
        assert!(
            h.changes
                .borrow()
                .contains(&DragStateChange::Leave { target }),
            "the hovered target heard Leave, not a drop"
        );
        assert!(
            !h.root.is_focus_active(),
            "focus is back to what it was before the press"
        );
        let after = h.frame();
        assert_eq!(rects(&after), vec![rest()], "the ghost closed");

        // The rest of the still-pressed gesture is swallowed.
        h.state.log.borrow_mut().clear();
        h.mouse(PointerPhase::Move, 32.0, 122.0);
        h.mouse(PointerPhase::Up, 32.0, 122.0);
        assert!(h.log().is_empty(), "{:?}", h.log());
        assert_eq!(h.state.taps, 0);
        assert!(!h.root.is_pointer_captured());

        // And the next press works normally.
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert_eq!(h.state.taps, 1);
    }

    #[test]
    fn escape_cancels_through_a_child_that_holds_focus_and_leaves_it_focused() {
        let mut h = Harness::new(Cfg {
            focusable_child: true,
            ..Cfg::default()
        });
        h.mouse_drag();
        assert!(h.root.is_focus_active());
        h.escape();
        assert_eq!(h.phase(), DragPhase::Idle);
        assert!(
            !h.log().iter().any(|line| line.starts_with("Key")),
            "the source consumed Escape: {:?}",
            h.log()
        );
        h.mouse(PointerPhase::Up, 60.0, 150.0);
        // The child's own focus was never the source's to release.
        assert!(h.root.is_focus_active(), "the child is still focused");
        h.key(Key::Character("a".into()));
        assert_eq!(
            h.log().last().map(String::as_str),
            Some("Key Character(\"a\")"),
            "keys still reach the focused child"
        );
    }

    #[test]
    fn escape_outside_a_drag_reaches_the_child() {
        let mut h = Harness::new(Cfg {
            focusable_child: true,
            ..Cfg::default()
        });
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.escape();
        assert_eq!(
            h.log().last().map(String::as_str),
            Some("Key Named(Escape)")
        );
    }

    #[test]
    fn a_drop_releases_the_focus_path_the_press_claimed() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        h.mouse(PointerPhase::Up, 60.0, 150.0);
        assert_eq!(h.phase(), DragPhase::Idle);
        // The focus path is gone: a key routes to nothing, and a later Escape
        // (no session) is inert.
        h.escape();
        assert!(!h.log().iter().any(|line| line.starts_with("Key")));
        // The next press elsewhere settles the root's own flag.
        h.mouse(PointerPhase::Down, 300.0, 500.0);
        h.mouse(PointerPhase::Up, 300.0, 500.0);
        assert!(!h.root.is_focus_active());
    }

    #[test]
    fn a_pointer_cancel_abandons_the_session() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        h.frame();
        h.mouse(PointerPhase::Cancel, 60.0, 150.0);
        assert_eq!(h.phase(), DragPhase::Idle);
        assert!(h.changes.borrow().contains(&DragStateChange::Phase {
            previous: DragPhase::Dragging,
            next: DragPhase::Cancelled,
        }));
        assert!(!h.root.is_pointer_captured());
        let after = h.frame();
        assert_eq!(rects(&after), vec![rest()], "the ghost closed");
        assert_eq!(h.state.taps, 0);
    }

    #[test]
    fn a_session_cancelled_elsewhere_closes_the_ghost_and_swallows_the_rest() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        h.frame();
        h.state.coordinator.cancel();
        let after = h.frame();
        assert_eq!(rects(&after), vec![rest()], "closed on the next frame");
        h.state.log.borrow_mut().clear();
        h.mouse(PointerPhase::Move, 90.0, 160.0);
        h.mouse(PointerPhase::Up, 90.0, 160.0);
        assert!(
            h.log().is_empty(),
            "the child hears nothing more: {:?}",
            h.log()
        );
        assert_eq!(h.phase(), DragPhase::Idle);

        // And the next press works normally.
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert_eq!(h.state.taps, 1);
    }

    #[test]
    fn dim_feedback_paints_the_source_under_a_dim_layer() {
        let mut h = Harness::new(Cfg::default());
        h.mouse_drag();
        let scene = h.frame();
        assert_eq!(
            scene.layers.first(),
            Some(&(rest().origin(), CHILD, DIM_OPACITY))
        );
        assert_eq!(rects(&scene).first(), Some(&rest()));
    }

    #[test]
    fn placeholder_feedback_leaves_the_rest_position_empty() {
        let mut h = Harness::new(Cfg {
            feedback: SourceFeedback::Placeholder,
            ..Cfg::default()
        });
        h.mouse_drag();
        let scene = h.frame();
        assert_eq!(
            rects(&scene),
            vec![Rect::from_origin_size(Point::new(40.0, 130.0), CHILD)],
            "only the ghost paints the child"
        );
    }

    #[test]
    fn no_feedback_leaves_the_source_untouched() {
        let mut h = Harness::new(Cfg {
            feedback: SourceFeedback::None,
            ..Cfg::default()
        });
        h.mouse_drag();
        let scene = h.frame();
        assert_eq!(
            scene.layers.len(),
            1,
            "only the ghost's layer: {:?}",
            scene.layers
        );
        assert_eq!(rects(&scene).first(), Some(&rest()));
    }

    #[test]
    fn a_ghost_builder_replaces_the_snapshot() {
        let mut h = Harness::new(Cfg {
            custom_ghost: true,
            ..Cfg::default()
        });
        h.mouse_drag();
        let scene = h.frame();
        assert_eq!(rects(&scene), vec![rest()], "the child is not snapshotted");
        assert!(
            scene.layers.contains(&(
                Point::new(40.0, 130.0),
                Size::new(30.0, 20.0),
                GHOST_OPACITY
            )),
            "the built ghost floats at the pointer, at its own size: {:?}",
            scene.layers
        );
    }

    #[test]
    fn a_secondary_press_never_arms() {
        let mut h = Harness::new(Cfg::default());
        h.root.event(
            &mut h.state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(30.0, 120.0),
                button: PointerButton::Secondary,
            }),
        );
        assert_eq!(h.phase(), DragPhase::Idle);
        assert_eq!(h.log(), vec!["Down@20,20"], "it still reaches the child");
    }

    #[test]
    fn a_plain_tap_leaves_the_source_focused_for_a_later_keyboard_lift() {
        let mut h = Harness::new(Cfg::default());
        assert!(!h.root.is_focus_active(), "nothing focused yet");
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert_eq!(h.state.taps, 1, "the tap still reaches the child");
        assert!(
            h.root.is_focus_active(),
            "a plain tap keeps the focus claim, unlike a drag's end"
        );
    }

    #[test]
    fn keyboard_lift_cycle_and_drop_deliver_the_payload_to_the_hovered_target() {
        let mut h = Harness::new(Cfg::default());
        let first = h.target(Rect::new(0.0, 0.0, 100.0, 100.0));
        let second = h.target(Rect::new(100.0, 0.0, 200.0, 100.0));

        // Focus the source with a plain tap — no drag, no pointer capture.
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert!(h.root.is_focus_active());

        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Dragging, "Enter lifts");
        assert_eq!(
            h.state.coordinator.with_payload(|item: &u32| *item),
            Some(7),
            "the payload builder ran against the state"
        );
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            None,
            "nothing hovered until a cycle step"
        );

        h.key(Key::Named(NamedKey::ArrowRight));
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            Some(first),
            "the first registered target is hovered after one step"
        );

        h.key(Key::Named(NamedKey::ArrowRight));
        assert_eq!(
            h.state
                .coordinator
                .state()
                .session()
                .and_then(|s| s.hovered),
            Some(second)
        );

        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.phase(),
            DragPhase::Dropping,
            "Enter drops on the hovered target"
        );
        match h.state.coordinator.state() {
            DragState::Dropping { target, .. } => assert_eq!(target, second),
            other => panic!("expected Dropping, got {other:?}"),
        }
        assert_eq!(
            h.state.coordinator.take_payload::<u32>().map(|p| *p),
            Some(7),
            "the payload the lift carried reaches the dropped-on target"
        );
        h.state.coordinator.complete_drop();
        assert_eq!(h.phase(), DragPhase::Idle);
        assert!(
            !h.root.is_focus_active(),
            "a completed drop releases focus, like a pointer drop"
        );
    }

    #[test]
    fn keyboard_drop_with_nothing_hovered_cancels() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Dragging);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Idle, "dropping on nothing cancels");
    }

    #[test]
    fn escape_cancels_a_keyboard_lift_and_a_later_lift_still_works() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Dragging);
        h.escape();
        assert_eq!(h.phase(), DragPhase::Idle);
        assert!(
            !h.root.is_focus_active(),
            "Escape ends the focus session outright, even for a keyboard lift"
        );

        // Not stuck in a swallowing state: another tap, then another lift.
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Dragging, "a later lift still works");
    }

    #[test]
    fn space_also_lifts_and_drops() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Character(" ".into()));
        assert_eq!(h.phase(), DragPhase::Dragging, "Space lifts");
        h.key(Key::Character(" ".into()));
        assert_eq!(
            h.phase(),
            DragPhase::Idle,
            "Space drops (on nothing: a cancel)"
        );
    }

    #[test]
    fn a_focused_child_keeps_its_own_keys_the_lift_chord_included() {
        let mut h = Harness::new(Cfg {
            focusable_child: true,
            ..Cfg::default()
        });
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert!(h.root.is_focus_active(), "the child holds its own focus");
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.phase(),
            DragPhase::Idle,
            "the child's own focus claim takes the key first — no lift"
        );
        assert_eq!(h.log().last().map(String::as_str), Some("Key Named(Enter)"));
    }

    #[test]
    fn semantics_reports_a_button_role_with_click_and_toggles_its_label() {
        let mut h = Harness::new(Cfg::default());
        let update = h.root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a Role::Button node was pushed");
        assert_eq!(node.label(), Some("Drag"));
        assert!(node.supports_action(Action::Click));

        h.mouse_drag();
        h.frame();
        let update = h.root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a Role::Button node was pushed");
        assert_eq!(node.label(), Some("Drop"), "dragging flips the label");
    }

    /// Where `perform_accessibility_action`'s `Action::Click` synthesizes its
    /// `Down`+`Up` pair: this node's own bounds center, in window space.
    fn node_center() -> Point {
        Point::new(AT.x + CHILD.width / 2.0, AT.y + CHILD.height / 2.0)
    }

    #[test]
    fn an_accessibility_click_on_an_idle_focused_source_is_a_plain_tap() {
        let mut h = Harness::new(Cfg::default());
        // Focus the source with a plain tap first, same as a real click would.
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        assert!(h.root.is_focus_active());
        h.state.taps = 0;
        h.state.log.borrow_mut().clear();

        // `Action::Click` reaches the widget as a synthesized `Down` then
        // `Up` at the node's own center — reproduced here through the
        // widget's own event path, not the accessibility seam itself.
        let center = node_center();
        h.mouse(PointerPhase::Down, center.x, center.y);
        h.mouse(PointerPhase::Up, center.x, center.y);

        assert_eq!(h.state.taps, 1, "Click reaches the child as a plain tap");
        assert_eq!(h.phase(), DragPhase::Idle, "Click never lifts a drag");
    }

    #[test]
    fn an_accessibility_click_during_a_live_keyboard_session_cancels_it_not_a_drop() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.phase(),
            DragPhase::Dragging,
            "Enter lifts a keyboard session"
        );

        // `Action::Click` arriving mid-session: the same synthesized `Down`
        // then `Up` at the node's own center. Click is a plain activation
        // (see the module docs), not the drop verb, so an unrelated press
        // arriving here cancels the live session exactly as any other stray
        // press would, rather than committing a drop.
        let center = node_center();
        h.mouse(PointerPhase::Down, center.x, center.y);
        h.mouse(PointerPhase::Up, center.x, center.y);

        assert_eq!(h.phase(), DragPhase::Idle, "the session ended, not dropped");
        assert!(
            h.changes.borrow().iter().any(|c| matches!(
                c,
                DragStateChange::Phase {
                    next: DragPhase::Cancelled,
                    ..
                }
            )),
            "cancelled, never a drop: {:?}",
            h.changes.borrow()
        );
        assert!(
            !h.changes.borrow().iter().any(|c| matches!(
                c,
                DragStateChange::Phase {
                    next: DragPhase::Dropping,
                    ..
                }
            )),
            "never a drop: {:?}",
            h.changes.borrow()
        );
    }

    #[test]
    fn a_keyboard_session_ended_elsewhere_returns_to_idle_ready_for_another_lift() {
        let mut h = Harness::new(Cfg::default());
        h.mouse(PointerPhase::Down, 30.0, 120.0);
        h.mouse(PointerPhase::Up, 30.0, 120.0);
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(h.phase(), DragPhase::Dragging, "Enter lifts");

        // Ended from outside this source entirely — a target's own
        // click-to-drop handling, or a cancel raised elsewhere — never
        // through this widget's own key or pointer paths.
        h.state.coordinator.cancel();
        let after = h.frame();
        assert_eq!(rects(&after), vec![rest()], "the ghost closed");
        assert!(
            h.root.is_focus_active(),
            "the focus claim is left exactly as it was, not released"
        );

        // Not stuck swallowing a pointer gesture that will never arrive: a
        // later lift still works.
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.phase(),
            DragPhase::Dragging,
            "the source landed back on Idle, ready for another lift"
        );
    }
}
