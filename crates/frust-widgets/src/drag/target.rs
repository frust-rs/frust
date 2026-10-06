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
//! ([`DragCoordinator::new_target_id`]) and registers it as accepting `T`
//! ([`DragCoordinator::register_target_for`]), so the coordinator's
//! resolution and keyboard cycling never pick it for a session carrying any
//! other payload type; [`DragTargetView::teardown`] unregisters it. Every
//! paint reports the widget's window-space bounds
//! ([`DragCoordinator::report_target_bounds`], stamped with the paint's
//! [`frust_core::PaintCtx::frame_time`]) so cross-container resolution can
//! find this target under the ghost point without hit-testing through the
//! ghost's own `Transparent` pod — `paint`, not layout, because
//! [`frust_core::LayoutCtx`] carries no window-space origin (only
//! [`frust_core::PaintCtx::origin`] does; see [`mod@super::draggable`]'s
//! `window_origin`, recorded the same way).
//!
//! The reported bounds are clipped to the visible region the ancestors
//! threaded down ([`frust_core::PaintCtx::visible_rect`] — the viewport a
//! scroll surface publishes, narrowed by every nested one), and a target
//! clipped to nothing reports no bounds at all, so it resolves nowhere. That
//! threaded rect is the only clip a paint context exposes: a container that
//! merely clips its scene ([`frust_core::PaintScene::push_clip`]) without
//! publishing a visible rect does not narrow what this target reports. A
//! target that does not paint in a pass (culled by its container) misses
//! that pass's stamp and stops resolving until it paints again.
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
//!   A session whose payload is not `T` is never offered: the coordinator
//!   does not resolve or cycle to this target for it, and should anything
//!   hover it anyway ([`DragCoordinator::set_hovered`]) it gets no callback
//!   and no highlight, regardless of [`DragTargetView::accepts`].
//! * **Hover**: while entered, the latest pointer position
//!   ([`DragSession::pointer`]) converted to this widget's local space — fires
//!   [`DragTargetView::on_hover`] once per distinct position.
//! * **Leave**: entered and no longer hovered (the pointer moved elsewhere, or
//!   the session ended without dropping here) — fires
//!   [`DragTargetView::on_leave`]. Only ever for a target that fired
//!   `on_enter`: every `on_leave` closes exactly one `on_enter`.
//! * **Drop**: [`DragCoordinator::drop`] released over this target
//!   ([`DragState::Dropping`]). [`DragTargetView::accepts`] (default: accept
//!   every `T`) is evaluated against the held payload; accepted,
//!   [`DragCoordinator::take_payload`] then
//!   [`DragTargetView::on_drop`] then [`DragCoordinator::complete_drop`] — no
//!   `on_leave` (the coordinator's own contract: the drop target hears no
//!   `Leave`, it is the drop target). Rejected (wrong type, or `accepts`
//!   false), this target calls [`DragCoordinator::cancel`] itself instead —
//!   `on_drop` never fires — and, if it had fired `on_enter` for this
//!   session, reports the matching `on_leave` once, since the engagement ends
//!   without a drop.
//!
//! A test driving [`DragCoordinator::set_hovered`]/[`DragCoordinator::drop`]
//! directly (simulating the per-frame hover resolution) sees exactly the same
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
//!
//! # Click-to-drop and semantics
//!
//! A primary `Up` landing on this target while a KEYBOARD session is live and
//! this target [`DragTargetView::accepts`] the held payload sets it hovered
//! ([`DragCoordinator::set_hovered`]) and drops
//! ([`DragCoordinator::drop`]) on the spot, ahead of the usual forward to the
//! child — the one way a target itself answers a press, where every other
//! phase stays the transparent wrapper the [module docs](super) describe.
//! A *pointer* session's `Up` is excluded on purpose: it belongs to whichever
//! source captured the gesture, and it still travels through every ancestor
//! on the capture chain — including an ancestor that is itself a drop target
//! (a kanban column wrapping its own draggable cards) — so answering it here
//! would let that ancestor claim a drop meant for its child. A keyboard
//! session never captures a pointer gesture, so it carries no such risk.
//! This is what makes [`Widget::semantics`]'s `Action::Click` meaningful
//! without a widget-side change to `frust-core`'s accessibility-action
//! routing: a shell's `ActionRequest(node_id, Action::Click)` already
//! synthesizes exactly a `Down` then an `Up` at this node's own bounds
//! center (see [`mod@super::draggable`]'s *Semantics* module docs for why a
//! richer per-target action is not wired to anything today), so an
//! assistive-technology user who lifted a keyboard session elsewhere can
//! activate a target directly to drop on it, with no pointer travel at all.
//! The node's label states acceptance — `"accepts drop"` while a live
//! keyboard session's payload type-matches and [`DragTargetView::accepts`]
//! it, `"drop target"` otherwise — and `Click` is advertised only in the
//! accepting case, so an adapter never offers an action that would be a
//! no-op.
//!
//! # OS file drops
//!
//! A desktop [`frust_core::InputEvent::FileDrop`] is hit-tested and bubbles
//! exactly like a pointer event, so it simply reaches whichever
//! `DragTargetWidget` (of any `T`) the position hit-tests into — there is no
//! separate registration for it. [`DragTargetWidget::handle_file_drop`]
//! drives the shared [`DragCoordinator`] directly from it, and only ever an
//! `ExternalFiles` session: `Hover` opens one when nothing is in flight (or
//! re-resolves the one already open), [`DragCoordinator::set_external_payload`]
//! fills in the real paths on `Drop` (a hover notification carries none — see
//! [`frust_core::event::FileDropEvent::paths`]) before dropping, and `Cancel`
//! abandons it. While an in-app session — pointer or keyboard, armed or
//! dragging — is live, all three are ignored (the event reports
//! [`EventResult::Ignored`]) and the in-app session is left exactly as it
//! was. Once an external session exists, every registered
//! `DragTargetWidget<State, Vec<PathBuf>>` sees Enter/Hover/Leave/Drop
//! through the same `Housekeeping`-polled path internal drags use,
//! regardless of which target's hit test happened to carry the triggering
//! event — resolution reads the registry, not the tree.
//!
//! **The drag always ends.** A shell resolves `Drop`/`Cancel` at its last
//! in-window cursor position, which can sit over a region with no
//! `DragTargetWidget` in its hit-test path; on a platform that sends no
//! cursor motion during an OS drag it is not even the release point. And a
//! drag can open sessions on more than one coordinator (two independent
//! target groups, hovered in turn), while its `Drop`/`Cancel` reaches only
//! the target it hit-tests into. So the root follows **every** hit-tested
//! `Drop`/`Cancel`, handled or not, with a [`FileDropPhase::Ended`]
//! broadcast (see [`frust_core::event::FileDropEvent`]'s *Broadcast
//! follow-up*), which every `DragTargetWidget` answers with
//! [`DragCoordinator::end_external`] — idempotent and narrow, so on each
//! coordinator exactly one target ends a session still `Dragging` (with the
//! `Leave` a hovered target is owed) and the rest find nothing to end, while
//! a coordinator whose target already handled the `Drop` (now `Dropping` or
//! done) or the `Cancel` (now `Idle`) is left alone. A `Drop` that missed
//! every target of a coordinator therefore cancels that coordinator's
//! session: there is no accepting target to take the paths.
//!
//! **Known limit**: a file drag hovering a window region with no
//! `DragTargetWidget` anywhere in its hit-test path never opens (or updates)
//! a session, since nothing calls `handle_file_drop` for it — see
//! `docs/LIMITATIONS.md`. Mobile and web shells publish no `FileDrop` at
//! all, so this section is desktop-only in every sense (see
//! [`frust_core::event::FileDropEvent`]'s Source section).
//!
//! **Dropped paths are untrusted input.** `on_drop`'s `Vec<PathBuf>` is
//! whatever the OS drag source supplied, passed through verbatim and
//! unvalidated: resolve symlinks before opening, check a file's type and
//! size before reading it, and sanitise a name before displaying it (see
//! [`frust_core::event::FileDropEvent::paths`]).

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::event::{FileDropEvent, FileDropPhase};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::coordinator::{DragCoordinator, DragPhase, DragState, DragTargetId};
use crate::authoring::{ErasedArgCallback, ErasedCallback, TypedArgCallback, presses};

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
    /// Hovered (or dropped) by a session whose payload is a `T` that
    /// [`DragTargetView::accepts`] rejects. A session whose payload is not `T`
    /// at all never reaches the highlight seam, hovered or dropped — this
    /// variant is only for a *type-matching* payload the predicate turns
    /// down.
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
        self.coordinator.register_target_for::<T>(id);
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
                } else if self.coordinator.payload_is::<T>() {
                    Some(DragHighlight::Reject)
                } else {
                    None
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

    /// The inverse of [`Self::to_local`]: a window-space position from one
    /// already hit-tested into this widget's local space — what
    /// [`Self::handle_file_drop`] needs, since [`InputEvent::FileDrop`]
    /// arrives localized by the ordinary hit-test chain
    /// ([`InputEvent::translated`]) while the coordinator's own session state
    /// (`DragSession::pointer`, a registered target's bounds) is window-space
    /// throughout.
    fn to_window(&self, local: Point) -> Point {
        local + self.window_origin.to_vec2()
    }

    /// Whether a session is live and this target would accept it right now —
    /// the gate [`Widget::event`]'s click-to-drop and [`Widget::semantics`]'s
    /// label/action both read (see the [module docs](self#click-to-drop-and-semantics)).
    ///
    /// Admits only a *keyboard* session. A pointer session's `Up` belongs to
    /// whichever source captured the gesture — the coordinator resolves hover
    /// from the pointer position, not the tree — and that captured `Up` still
    /// travels through every ancestor on the capture chain, including an
    /// ancestor that is itself a [`DragTargetWidget`] (a drop target wrapping
    /// its own draggable children, as a kanban column wraps its cards). Were
    /// this gate open for a pointer session, that ancestor would answer the
    /// child's own `Up` before the child ever saw it, re-hovering and
    /// dropping on itself. A keyboard session never captures a pointer
    /// gesture, so no ancestor's `Up` can collide with it this way.
    fn accepts_click_drop(&self) -> bool {
        matches!(self.coordinator.state(), DragState::Dragging(session) if session.keyboard)
            && self.payload_accepted()
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
                let was_engaged = std::mem::replace(&mut self.engaged, false);
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
                    if was_engaged && let Some(cb) = self.on_leave.as_mut() {
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

    /// Drive the coordinator from a hit-tested [`InputEvent::FileDrop`] that
    /// reached this target — see the [module docs](self#os-file-drops) for
    /// the design and its limits.
    ///
    /// Acts only on an `ExternalFiles` session
    /// ([`DragCoordinator::is_external`]). `Hover` opens one with an empty
    /// payload (the real list is not known yet — see
    /// [`DragCoordinator::set_external_payload`]) only from `Idle`, then
    /// resolves the pointer (`begin_external` itself leaves nothing hovered,
    /// so a fresh session still needs this to enter on the same event that
    /// opened it); `Drop` fills in the real paths, resolves once more at the
    /// final position, then drops; `Cancel` abandons it. With an in-app
    /// session in flight instead, every phase reports
    /// [`EventResult::Ignored`] and touches nothing — and the root's `Ended`
    /// follow-up to a `Drop`/`Cancel` ends no in-app session either. Every
    /// acting arm polls inline afterward, like the
    /// pointer click-to-drop arm, so `on_enter`/`on_hover`/`on_drop`/
    /// `on_leave` resolve within the event that caused them rather than
    /// waiting for the next `Housekeeping` broadcast.
    ///
    /// [`FileDropPhase::Ended`] is a broadcast, so [`Widget::event`] answers
    /// it with [`Self::end_os_drag`] and never routes it here; its arm below
    /// only keeps the match total, and does the same thing.
    fn handle_file_drop(&mut self, ctx: &mut EventCtx, drop: &FileDropEvent) -> EventResult {
        let window_pos = self.to_window(drop.position);
        if drop.phase == FileDropPhase::Hover && self.coordinator.phase() == DragPhase::Idle {
            self.coordinator.begin_external(Vec::new(), window_pos);
        }
        if !self.coordinator.is_external() {
            return EventResult::Ignored;
        }
        match drop.phase {
            FileDropPhase::Hover => {
                self.coordinator.update_pointer(window_pos);
            }
            FileDropPhase::Drop => {
                self.coordinator.set_external_payload(drop.paths.clone());
                self.coordinator.update_pointer(window_pos);
                self.coordinator.drop();
            }
            FileDropPhase::Cancel => {
                self.coordinator.cancel();
            }
            FileDropPhase::Ended => {
                self.end_os_drag(ctx);
            }
        }
        self.poll(ctx);
        ctx.request_redraw();
        EventResult::Handled
    }

    /// Answer a [`FileDropPhase::Ended`] broadcast: the OS drag is over, so
    /// end any `ExternalFiles` session still dragging — one whose
    /// `Drop`/`Cancel` reached none of this coordinator's targets
    /// ([`DragCoordinator::end_external`] — a no-op for every target after
    /// the first, for a session a target already dropped or cancelled, and
    /// for any in-app session).
    /// [`Widget::event`]'s broadcast branch polls right after, which is what
    /// fires the hovered target's `on_leave`.
    fn end_os_drag(&mut self, ctx: &mut EventCtx) {
        if self.coordinator.end_external() {
            ctx.request_redraw();
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

/// This widget's window-space bounds clipped to the visible region its
/// ancestors threaded down ([`PaintCtx::visible_rect`]), or `None` when that
/// clip leaves nothing of it — see the [module docs](self#lifecycle).
fn visible_bounds(ctx: &PaintCtx) -> Option<Rect> {
    let bounds = Rect::from_origin_size(ctx.origin(), ctx.size());
    let visible = match ctx.visible_rect() {
        Some(clip) => bounds.intersect(clip),
        None => bounds,
    };
    (visible.width() > 0.0 && visible.height() > 0.0).then_some(visible)
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
            .report_target_bounds(self.id, visible_bounds(ctx), ctx.frame_time());
        self.child.paint_child(ctx, scene);
        if let Some(state) = self.current_highlight() {
            self.paint_highlight(ctx, scene, state);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            if let InputEvent::FileDrop(drop) = event
                && drop.phase == FileDropPhase::Ended
            {
                self.end_os_drag(ctx);
            }
            self.poll(ctx);
        } else if let InputEvent::FileDrop(drop) = event {
            return self.handle_file_drop(ctx, drop);
        }
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Up
            && presses(p)
            && self.accepts_click_drop()
        {
            self.coordinator.set_hovered(Some(self.id));
            self.coordinator.drop();
            // `drop` only moves the coordinator to `Dropping`; the usual
            // `Housekeeping` poll is what claims the payload and fires
            // `on_drop` — run it inline rather than waiting for the next
            // broadcast, so the click resolves within the event that caused
            // it.
            self.poll(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // No accesskit role names a drop zone; `Role::Group` is the nearest
        // region-equivalent. See the [module docs](self#click-to-drop-and-semantics)
        // for why `Click` is advertised only while this target would accept
        // the live session.
        let accepting = self.accepts_click_drop();
        ctx.push_container(
            Role::Group,
            |node| {
                node.set_label(if accepting {
                    "accepts drop"
                } else {
                    "drop target"
                });
                if accepting {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.child.semantics_child(ctx),
        );
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

    fn build<S: 'static, T: 'static>(view: &DragTargetView<S, T>) -> DragTargetWidget<S, T> {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn housekeeping<S: 'static, T: 'static>(
        w: &mut DragTargetWidget<S, T>,
        state: &mut S,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        w.event(&mut ctx, &InputEvent::Housekeeping)
    }

    /// A primary `Up` at the target's own local origin — the click-to-drop
    /// route a shell's `ActionRequest(Action::Click)` would synthesize.
    fn click<S: 'static, T: 'static>(w: &mut DragTargetWidget<S, T>, state: &mut S) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        let event = InputEvent::Pointer(frust_core::PointerEvent {
            phase: PointerPhase::Up,
            position: Point::ZERO,
            button: frust_core::PointerButton::Primary,
        });
        w.event(&mut ctx, &event)
    }

    /// Dispatch an [`InputEvent::FileDrop`] at `position` (already local to
    /// `w`, as a real hit test would deliver it) straight into `w`.
    fn file_drop<S: 'static, T: 'static>(
        w: &mut DragTargetWidget<S, T>,
        state: &mut S,
        phase: FileDropPhase,
        position: Point,
        paths: Vec<std::path::PathBuf>,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        let event = InputEvent::FileDrop(FileDropEvent {
            phase,
            position,
            paths,
        });
        w.event(&mut ctx, &event)
    }

    fn laid_out<S: 'static, T: 'static>(w: &mut DragTargetWidget<S, T>, at: Point, size: Size) {
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

    #[test]
    fn external_files_session_fires_callbacks_and_completes_drop() {
        use std::path::PathBuf;

        let coordinator = DragCoordinator::new();
        let entered = Rc::new(RefCell::new(0u32));
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let e2 = Rc::clone(&entered);
        let d2 = Rc::clone(&dropped);

        let view: DragTargetView<App, Vec<PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_enter(move |_s: &mut App| *e2.borrow_mut() += 1)
                .on_drop(move |_s: &mut App, paths: Vec<PathBuf>| *d2.borrow_mut() = paths);

        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let paths = vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")];
        coordinator.begin_external(paths.clone(), Point::ZERO);
        coordinator.set_hovered(Some(w.id));

        housekeeping(&mut w, &mut state);
        assert_eq!(
            *entered.borrow(),
            1,
            "on_enter fires once for an external files session"
        );
        assert_eq!(
            w.current_highlight(),
            Some(DragHighlight::Hover),
            "external files session shows Hover highlight when hovered"
        );

        coordinator.drop();
        assert_eq!(
            w.current_highlight(),
            Some(DragHighlight::Accept),
            "external files session shows Accept highlight during drop"
        );
        housekeeping(&mut w, &mut state);
        assert_eq!(
            *dropped.borrow(),
            paths,
            "on_drop receives exactly the external file paths"
        );
        assert_eq!(
            coordinator.phase(),
            DragPhase::Idle,
            "phase returns to Idle after drop completes"
        );
    }

    #[test]
    fn a_file_drop_hover_then_drop_delivers_the_dropped_paths() {
        use std::path::PathBuf;

        let coordinator = DragCoordinator::new();
        let entered = Rc::new(RefCell::new(0u32));
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let e2 = Rc::clone(&entered);
        let d2 = Rc::clone(&dropped);

        let view: DragTargetView<App, Vec<PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_enter(move |_s: &mut App| *e2.borrow_mut() += 1)
                .on_drop(move |_s: &mut App, paths: Vec<PathBuf>| *d2.borrow_mut() = paths);

        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        laid_out(&mut w, Point::new(5.0, 5.0), Size::new(10.0, 10.0));
        let mut state = App;

        // Hover carries no paths — the real list is unknown until the drop.
        let result = file_drop(
            &mut w,
            &mut state,
            FileDropPhase::Hover,
            Point::ZERO,
            vec![],
        );
        assert_eq!(result, EventResult::Handled);
        assert_eq!(
            coordinator.phase(),
            DragPhase::Dragging,
            "hovering a target opens an external session"
        );
        assert_eq!(
            *entered.borrow(),
            1,
            "on_enter fires on the same hover that opened the session"
        );

        let paths = vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")];
        let result = file_drop(
            &mut w,
            &mut state,
            FileDropPhase::Drop,
            Point::ZERO,
            paths.clone(),
        );
        assert_eq!(result, EventResult::Handled);
        assert_eq!(
            *dropped.borrow(),
            paths,
            "on_drop receives the dropped paths, not the empty hover list"
        );
        assert_eq!(coordinator.phase(), DragPhase::Idle);
    }

    #[test]
    fn a_file_drop_cancel_fires_leave_and_ends_the_session() {
        use std::path::PathBuf;

        let coordinator = DragCoordinator::new();
        let left = Rc::new(RefCell::new(0u32));
        let l2 = Rc::clone(&left);

        let view: DragTargetView<App, Vec<PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1);

        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        file_drop(
            &mut w,
            &mut state,
            FileDropPhase::Hover,
            Point::ZERO,
            vec![],
        );
        assert_eq!(coordinator.phase(), DragPhase::Dragging);

        file_drop(
            &mut w,
            &mut state,
            FileDropPhase::Cancel,
            Point::ZERO,
            vec![],
        );
        assert_eq!(
            *left.borrow(),
            1,
            "cancelling a hovered external session fires on_leave once"
        );
        assert_eq!(coordinator.phase(), DragPhase::Idle);
    }

    #[test]
    fn a_click_drops_an_accepted_session_with_no_pointer_travel() {
        let coordinator = DragCoordinator::new();
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let d2 = Rc::clone(&dropped);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_drop(move |_s: &mut App, v: u32| d2.borrow_mut().push(v));
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        // Lifted elsewhere (a keyboard session, say); never moved near this
        // target's bounds.
        let source = coordinator.new_source_id();
        coordinator.lift(source);
        coordinator.begin(7u32);

        let result = click(&mut w, &mut state);
        assert_eq!(result, EventResult::Handled);
        assert_eq!(*dropped.borrow(), vec![7], "the click delivered the drop");
        assert_eq!(coordinator.phase(), DragPhase::Idle);
    }

    #[test]
    fn a_click_with_no_live_session_falls_through_to_the_child() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone());
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;
        assert_eq!(click(&mut w, &mut state), EventResult::Ignored);
    }

    #[test]
    fn a_click_with_a_rejected_payload_does_not_drop() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .accepts(|v: &u32| *v < 10);
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(99u32);

        click(&mut w, &mut state);
        assert_eq!(
            coordinator.phase(),
            DragPhase::Dragging,
            "a rejected click leaves the session exactly as it was"
        );
    }

    #[test]
    fn semantics_label_and_action_track_acceptance() {
        use frust_core::accesskit::{Action, Role};

        let coordinator = DragCoordinator::new();
        let logic_coordinator = coordinator.clone();
        let mut logic = move |_s: &mut App| {
            drag_target::<u32, App, _>(
                SizedBox::<App>(Some(10.0), Some(10.0)),
                logic_coordinator.clone(),
            )
        };
        let mut root: frust_core::RenderRoot<App, DragTargetView<App, u32>> =
            frust_core::RenderRoot::new();
        let mut state = App;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group node was pushed");
        assert_eq!(node.label(), Some("drop target"));
        assert!(
            !node.supports_action(Action::Click),
            "no session: no action"
        );

        let source = coordinator.new_source_id();
        coordinator.lift(source);
        coordinator.begin(7u32);
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group node was pushed");
        assert_eq!(node.label(), Some("accepts drop"));
        assert!(
            node.supports_action(Action::Click),
            "accepting: Click is offered"
        );
    }

    /// Lay `w` out at `size` and paint it at window-space `at` in a paint
    /// context carrying `frame` and (optionally) an ancestor's visible rect.
    fn painted_at<S: 'static, T: 'static>(
        w: &mut DragTargetWidget<S, T>,
        at: Point,
        size: Size,
        frame: frust_core::FrameTime,
        visible: Option<Rect>,
    ) {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(size));
        let mut scene = crate::test_support::RecordingScene::default();
        let mut pctx = PaintCtx::for_test(at, size, frame);
        if let Some(visible) = visible {
            pctx.constrain_visible_rect(visible);
        }
        w.paint(&mut pctx, &mut scene);
    }

    fn frame(n: u64) -> frust_core::FrameTime {
        frust_core::FrameTime::from_nanos(n * 16_000_000)
    }

    #[test]
    fn paint_reports_bounds_clipped_to_the_visible_rect() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let mut w = build(&view);
        painted_at(
            &mut w,
            Point::new(0.0, 0.0),
            Size::new(100.0, 100.0),
            frame(1),
            Some(Rect::new(0.0, 60.0, 400.0, 400.0)),
        );
        assert_eq!(
            coordinator.target_bounds(w.id),
            Some(Rect::new(0.0, 60.0, 100.0, 100.0)),
            "only the part inside the ancestor's visible rect is reported"
        );
        assert_eq!(
            coordinator.target_at(Point::new(50.0, 30.0)),
            None,
            "the clipped-away part does not resolve"
        );
        assert_eq!(coordinator.target_at(Point::new(50.0, 80.0)), Some(w.id));
    }

    #[test]
    fn a_target_clipped_to_nothing_reports_no_bounds_and_is_not_resolved() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, u32> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let mut w = build(&view);
        painted_at(&mut w, Point::ZERO, Size::new(100.0, 100.0), frame(1), None);
        assert!(coordinator.target_bounds(w.id).is_some());

        // Scrolled entirely out of its scroll ancestor's viewport.
        painted_at(
            &mut w,
            Point::ZERO,
            Size::new(100.0, 100.0),
            frame(2),
            Some(Rect::new(0.0, 200.0, 400.0, 400.0)),
        );
        assert_eq!(coordinator.target_bounds(w.id), None);
        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(1u32);
        coordinator.update_pointer(Point::new(50.0, 50.0));
        assert_eq!(coordinator.state().session().and_then(|s| s.hovered), None);
    }

    #[test]
    fn a_target_that_did_not_paint_this_frame_is_not_resolved() {
        let coordinator = DragCoordinator::new();
        let first_view: DragTargetView<App, u32> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let second_view: DragTargetView<App, u32> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let mut first = build(&first_view);
        let mut second = build(&second_view);
        let size = Size::new(100.0, 100.0);
        painted_at(&mut first, Point::ZERO, size, frame(1), None);
        painted_at(&mut second, Point::new(200.0, 0.0), size, frame(1), None);
        assert_eq!(
            coordinator.target_at(Point::new(50.0, 50.0)),
            Some(first.id)
        );

        // Frame 2 culls the first target: only the second paints.
        painted_at(&mut second, Point::new(200.0, 0.0), size, frame(2), None);
        assert_eq!(coordinator.target_at(Point::new(50.0, 50.0)), None);
        assert_eq!(
            coordinator.target_at(Point::new(250.0, 50.0)),
            Some(second.id)
        );
    }

    #[test]
    fn overlapping_targets_of_different_types_resolve_by_the_payload_type() {
        let coordinator = DragCoordinator::new();
        let numbers: DragTargetView<App, u32> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let words: DragTargetView<App, String> = drag_target(
            SizedBox::<App>(Some(100.0), Some(100.0)),
            coordinator.clone(),
        );
        let mut numbers = build(&numbers);
        // Built (registered) later: it would win the overlap on order alone.
        let mut words = build(&words);
        let size = Size::new(100.0, 100.0);
        painted_at(&mut numbers, Point::ZERO, size, frame(1), None);
        painted_at(&mut words, Point::ZERO, size, frame(1), None);

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(5u32);
        coordinator.update_pointer(Point::new(50.0, 50.0));
        assert_eq!(
            coordinator.state().session().and_then(|s| s.hovered),
            Some(numbers.id)
        );
        coordinator.cancel();

        coordinator.lift(source);
        coordinator.begin(String::from("word"));
        coordinator.move_to_next_target();
        coordinator.move_to_next_target();
        assert_eq!(
            coordinator.state().session().and_then(|s| s.hovered),
            Some(words.id),
            "keyboard cycling only ever lands on the String target"
        );
    }

    #[test]
    fn a_rejected_drop_on_a_never_entered_target_fires_no_leave() {
        let coordinator = DragCoordinator::new();
        let left = Rc::new(RefCell::new(0u32));
        let l2 = Rc::clone(&left);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .accepts(|v: &u32| *v < 10)
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1);
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(99u32);
        // Hovered and dropped before any poll saw the hover: never entered.
        coordinator.set_hovered(Some(w.id));
        coordinator.drop();
        housekeeping(&mut w, &mut state);
        assert_eq!(coordinator.phase(), DragPhase::Idle, "the drop is refused");
        assert_eq!(*left.borrow(), 0, "no on_leave without an on_enter");
    }

    #[test]
    fn a_rejected_drop_on_an_entered_target_fires_exactly_one_leave() {
        let coordinator = DragCoordinator::new();
        let entered = Rc::new(RefCell::new(0u32));
        let left = Rc::new(RefCell::new(0u32));
        let e2 = Rc::clone(&entered);
        let l2 = Rc::clone(&left);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .accepts(|v: &u32| *v < 10)
                .on_enter(move |_s: &mut App| *e2.borrow_mut() += 1)
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1);
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin(99u32);
        coordinator.set_hovered(Some(w.id));
        housekeeping(&mut w, &mut state);
        assert_eq!(*entered.borrow(), 1);
        coordinator.drop();
        housekeeping(&mut w, &mut state);
        housekeeping(&mut w, &mut state);
        assert_eq!(coordinator.phase(), DragPhase::Idle);
        assert_eq!(*left.borrow(), 1, "one on_leave closes the one on_enter");
    }

    #[test]
    fn a_non_t_payload_dropped_here_shows_no_reject_and_fires_no_leave() {
        let coordinator = DragCoordinator::new();
        let left = Rc::new(RefCell::new(0u32));
        let l2 = Rc::clone(&left);
        let view: DragTargetView<App, u32> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1);
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;

        let source = coordinator.new_source_id();
        coordinator.arm(source, Point::ZERO);
        coordinator.begin("not-a-u32");
        // Only an explicit override can put a non-`u32` session here.
        coordinator.set_hovered(Some(w.id));
        housekeeping(&mut w, &mut state);
        coordinator.drop();
        assert_eq!(
            w.current_highlight(),
            None,
            "a non-T payload never reaches the highlight seam, dropped or not"
        );
        housekeeping(&mut w, &mut state);
        assert_eq!(coordinator.phase(), DragPhase::Idle);
        assert_eq!(*left.borrow(), 0);
    }

    /// Deliver the root's [`FileDropPhase::Ended`] broadcast straight into
    /// `w`, as a container forwarding it would.
    fn ended<S: 'static, T: 'static>(w: &mut DragTargetWidget<S, T>, state: &mut S) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        let event = InputEvent::FileDrop(FileDropEvent {
            phase: FileDropPhase::Ended,
            position: Point::ZERO,
            paths: Vec::new(),
        });
        w.event(&mut ctx, &event)
    }

    #[test]
    fn the_ended_broadcast_ends_a_hovering_external_session_exactly_once() {
        use std::path::PathBuf;

        let coordinator = DragCoordinator::new();
        let left = Rc::new(RefCell::new(0u32));
        let dropped = Rc::new(RefCell::new(0u32));
        let (l2, d2) = (Rc::clone(&left), Rc::clone(&dropped));
        let hovered_view: DragTargetView<App, Vec<PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone())
                .on_leave(move |_s: &mut App| *l2.borrow_mut() += 1)
                .on_drop(move |_s: &mut App, _paths: Vec<PathBuf>| *d2.borrow_mut() += 1);
        let other_view: DragTargetView<App, Vec<PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone());
        let mut hovered = build(&hovered_view);
        let mut other = build(&other_view);
        laid_out(&mut hovered, Point::ZERO, Size::new(10.0, 10.0));
        laid_out(&mut other, Point::new(50.0, 0.0), Size::new(10.0, 10.0));
        let mut state = App;

        file_drop(
            &mut hovered,
            &mut state,
            FileDropPhase::Hover,
            Point::new(5.0, 5.0),
            vec![],
        );
        assert_eq!(
            coordinator.state().session().and_then(|s| s.hovered),
            Some(hovered.id)
        );

        // The hit-tested `Drop`/`Cancel` landed on neither target; the root's
        // follow-up reaches both, in tree order, and the session ends once.
        assert_eq!(ended(&mut other, &mut state), EventResult::Ignored);
        assert_eq!(
            coordinator.phase(),
            DragPhase::Idle,
            "the first target ended it"
        );
        assert_eq!(ended(&mut hovered, &mut state), EventResult::Ignored);
        assert_eq!(coordinator.phase(), DragPhase::Idle);
        assert_eq!(
            *left.borrow(),
            1,
            "the hovered target's on_leave fired once"
        );
        assert_eq!(*dropped.borrow(), 0, "nothing accepted the drop");
    }

    #[test]
    fn file_drop_events_leave_an_in_app_session_untouched() {
        let coordinator = DragCoordinator::new();
        let view: DragTargetView<App, Vec<std::path::PathBuf>> =
            drag_target(SizedBox::<App>(Some(10.0), Some(10.0)), coordinator.clone());
        let mut w = build(&view);
        laid_out(&mut w, Point::ZERO, Size::new(10.0, 10.0));
        let mut state = App;
        let source = coordinator.new_source_id();

        // A pointer press that has not started dragging, a pointer drag, and
        // a keyboard drag.
        let sessions: [&dyn Fn(); 3] = [
            &|| {
                coordinator.arm(source, Point::new(1.0, 1.0));
            },
            &|| {
                coordinator.arm(source, Point::new(1.0, 1.0));
                coordinator.begin(3_u32);
            },
            &|| {
                coordinator.lift(source);
                coordinator.begin(4_u32);
            },
        ];
        for start in sessions {
            start();
            let before = coordinator.state();
            for phase in [
                FileDropPhase::Hover,
                FileDropPhase::Drop,
                FileDropPhase::Cancel,
            ] {
                let result = file_drop(
                    &mut w,
                    &mut state,
                    phase,
                    Point::new(5.0, 5.0),
                    vec![std::path::PathBuf::from("/tmp/a.txt")],
                );
                assert_eq!(result, EventResult::Ignored, "{phase:?} is not ours");
                assert_eq!(coordinator.state(), before, "{phase:?} touched nothing");
            }
            ended(&mut w, &mut state);
            assert_eq!(coordinator.state(), before, "nor did the Ended follow-up");
            coordinator.cancel();
        }
    }

    /// An app whose whole window is a 400x400 backdrop with one 100x100
    /// path-list drop target in its top-left corner, driven through a real
    /// [`frust_core::RenderRoot`] so the root's `Ended` follow-up is the
    /// one under test.
    struct FileApp {
        coordinator: DragCoordinator,
        entered: u32,
        left: u32,
        dropped: Vec<std::path::PathBuf>,
    }

    fn file_app_logic(state: &mut FileApp) -> crate::StackView<FileApp> {
        crate::Stack(vec![
            any(SizedBox::<FileApp>(Some(400.0), Some(400.0))),
            any(drag_target::<Vec<std::path::PathBuf>, FileApp, _>(
                SizedBox::<FileApp>(Some(100.0), Some(100.0)),
                state.coordinator.clone(),
            )
            .on_enter(|s: &mut FileApp| s.entered += 1)
            .on_leave(|s: &mut FileApp| s.left += 1)
            .on_drop(|s: &mut FileApp, paths| s.dropped = paths)),
        ])
    }

    #[test]
    fn an_os_drag_released_away_from_every_target_ends_its_session() {
        let (mut root, mut state) = file_app_root();
        let event = |phase, x, y, paths| {
            InputEvent::FileDrop(FileDropEvent {
                phase,
                position: Point::new(x, y),
                paths,
            })
        };
        for (round, end) in [FileDropPhase::Cancel, FileDropPhase::Drop]
            .into_iter()
            .enumerate()
        {
            root.event(&mut state, &event(FileDropPhase::Hover, 50.0, 50.0, vec![]));
            assert_eq!(state.coordinator.phase(), DragPhase::Dragging);
            assert_eq!(state.entered, round as u32 + 1);

            let paths = vec![std::path::PathBuf::from("/tmp/a.txt")];
            let outcome = root.event(&mut state, &event(end, 300.0, 300.0, paths));
            assert!(!outcome.handled, "{end:?} missed the only target");
            assert_eq!(
                state.coordinator.phase(),
                DragPhase::Idle,
                "a {end:?} away from every target still ends the session"
            );
            assert_eq!(state.left, round as u32 + 1, "and the target hears Leave");
            assert!(state.dropped.is_empty(), "nothing accepted the paths");
        }

        // The ended session no longer blocks an in-app drag.
        let source = state.coordinator.new_source_id();
        assert!(state.coordinator.arm(source, Point::new(10.0, 10.0)));
    }

    /// [`FileApp`] built, laid out at 400x400 and painted once, so its target
    /// has reported the bounds resolution reads.
    fn file_app_root() -> (
        frust_core::RenderRoot<FileApp, crate::StackView<FileApp>>,
        FileApp,
    ) {
        let mut root: frust_core::RenderRoot<FileApp, crate::StackView<FileApp>> =
            frust_core::RenderRoot::new();
        let mut state = FileApp {
            coordinator: DragCoordinator::new(),
            entered: 0,
            left: 0,
            dropped: Vec::new(),
        };
        let mut logic: fn(&mut FileApp) -> crate::StackView<FileApp> = file_app_logic;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(400.0, 400.0));
        let mut scene = crate::test_support::RecordingScene::default();
        root.paint(&mut scene, frust_core::FrameTime::from_nanos(16_000_000));
        (root, state)
    }

    fn os_drag(phase: FileDropPhase, at: Point) -> InputEvent {
        InputEvent::FileDrop(FileDropEvent {
            phase,
            position: at,
            paths: Vec::new(),
        })
    }

    fn primary(phase: PointerPhase, at: Point) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: at,
            button: frust_core::PointerButton::Primary,
        })
    }

    /// The tree half of an OS file drag that the desktop shell sees end with
    /// `CursorLeft` or a real button event rather than a drop: the shell
    /// dispatches `Hover` when the drag enters (and again on each cursor
    /// move), then exactly one `Cancel` at the last cursor position when the
    /// hover ends that way — before the button's own `Down`/`Up` — and
    /// nothing for a `HoveredFileCancelled` that may still follow. Fed those
    /// exact events, the session ends and in-app drags work again, whether
    /// the cursor last stood over the target or away from it.
    #[test]
    fn an_os_drag_whose_hover_ends_without_a_drop_ends_its_session() {
        let over = Point::new(50.0, 50.0);
        let away = Point::new(300.0, 300.0);
        for cursor in [over, away] {
            for with_button in [false, true] {
                let (mut root, mut state) = file_app_root();
                root.event(&mut state, &os_drag(FileDropPhase::Hover, over));
                root.event(&mut state, &os_drag(FileDropPhase::Hover, cursor));
                assert_eq!(state.coordinator.phase(), DragPhase::Dragging);
                assert_eq!(state.entered, 1);

                // `CursorLeft`, or the `MouseInput` that leads with the same
                // `Cancel`; a later `HoveredFileCancelled` dispatches nothing.
                root.event(&mut state, &os_drag(FileDropPhase::Cancel, cursor));
                if with_button {
                    root.event(&mut state, &primary(PointerPhase::Down, cursor));
                    root.event(&mut state, &primary(PointerPhase::Up, cursor));
                }
                let case = format!("cursor at {cursor:?}, button event: {with_button}");
                assert_eq!(state.coordinator.phase(), DragPhase::Idle, "{case}");
                assert_eq!(state.left, 1, "{case}: the entered target hears Leave");
                assert!(state.dropped.is_empty(), "{case}: nothing was dropped");

                let source = state.coordinator.new_source_id();
                assert!(
                    state.coordinator.arm(source, Point::new(10.0, 10.0)),
                    "{case}: an in-app drag can start again"
                );
            }
        }
    }

    /// Without the shell's `Cancel` the session the `Hover` opened stands
    /// through the button events, blocking in-app drags, and a release over
    /// the target click-drops the hover's empty path list — the outcome the
    /// shell's `Cancel` exists to prevent.
    #[test]
    fn an_os_drag_hover_left_standing_blocks_in_app_drags() {
        let (mut root, mut state) = file_app_root();
        let away = Point::new(300.0, 300.0);
        root.event(
            &mut state,
            &os_drag(FileDropPhase::Hover, Point::new(50.0, 50.0)),
        );
        root.event(&mut state, &primary(PointerPhase::Down, away));
        root.event(&mut state, &primary(PointerPhase::Up, away));
        assert_eq!(state.coordinator.phase(), DragPhase::Dragging);
        let source = state.coordinator.new_source_id();
        assert!(!state.coordinator.arm(source, Point::new(10.0, 10.0)));
    }

    /// Two independent path-list drop targets, each on its own
    /// [`DragCoordinator`], side by side at the top-left of a 400x400 window:
    /// `a` spans `0..100` and `b` `100..200` horizontally.
    struct TwoGroups {
        a: DragCoordinator,
        b: DragCoordinator,
        a_left: u32,
        a_dropped: Option<Vec<std::path::PathBuf>>,
        b_dropped: Option<Vec<std::path::PathBuf>>,
    }

    fn two_groups_logic(state: &mut TwoGroups) -> crate::StackView<TwoGroups> {
        crate::Stack(vec![
            any(SizedBox::<TwoGroups>(Some(400.0), Some(400.0))),
            any(crate::Row(vec![
                any(drag_target::<Vec<std::path::PathBuf>, TwoGroups, _>(
                    SizedBox::<TwoGroups>(Some(100.0), Some(100.0)),
                    state.a.clone(),
                )
                .on_leave(|s: &mut TwoGroups| s.a_left += 1)
                .on_drop(|s: &mut TwoGroups, paths| s.a_dropped = Some(paths))),
                any(drag_target::<Vec<std::path::PathBuf>, TwoGroups, _>(
                    SizedBox::<TwoGroups>(Some(100.0), Some(100.0)),
                    state.b.clone(),
                )
                .on_drop(|s: &mut TwoGroups, paths| s.b_dropped = Some(paths))),
            ])),
        ])
    }

    /// A drag that hovers group `a` (opening an external session on its
    /// coordinator) and is then released over group `b`: `b` handles the
    /// `Drop` and completes it, and the root's `Ended` follow-up — sent after
    /// every `Drop`, handled or not — still ends `a`'s session.
    #[test]
    fn a_drop_handled_by_one_coordinator_still_ends_anothers_session() {
        let mut root: frust_core::RenderRoot<TwoGroups, crate::StackView<TwoGroups>> =
            frust_core::RenderRoot::new();
        let mut state = TwoGroups {
            a: DragCoordinator::new(),
            b: DragCoordinator::new(),
            a_left: 0,
            a_dropped: None,
            b_dropped: None,
        };
        let mut logic: fn(&mut TwoGroups) -> crate::StackView<TwoGroups> = two_groups_logic;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(400.0, 400.0));
        let mut scene = crate::test_support::RecordingScene::default();
        root.paint(&mut scene, frust_core::FrameTime::from_nanos(16_000_000));

        root.event(
            &mut state,
            &os_drag(FileDropPhase::Hover, Point::new(50.0, 50.0)),
        );
        assert_eq!(state.a.phase(), DragPhase::Dragging, "a opened a session");
        assert_eq!(state.b.phase(), DragPhase::Idle);
        root.event(
            &mut state,
            &os_drag(FileDropPhase::Hover, Point::new(150.0, 50.0)),
        );
        assert_eq!(state.b.phase(), DragPhase::Dragging, "and so did b");

        let paths = vec![std::path::PathBuf::from("/tmp/a.txt")];
        let outcome = root.event(
            &mut state,
            &InputEvent::FileDrop(FileDropEvent {
                phase: FileDropPhase::Drop,
                position: Point::new(150.0, 50.0),
                paths: paths.clone(),
            }),
        );
        assert!(outcome.handled, "b's target handled the Drop");
        assert_eq!(state.b_dropped, Some(paths), "b completed its drop");
        assert_eq!(state.b.phase(), DragPhase::Idle);
        assert_eq!(state.a.phase(), DragPhase::Idle, "a's session ended too");
        assert_eq!(state.a_left, 1, "with the Leave a's hovered target is owed");
        assert_eq!(state.a_dropped, None, "a was not dropped on");

        for coordinator in [&state.a, &state.b] {
            let source = coordinator.new_source_id();
            assert!(coordinator.arm(source, Point::new(10.0, 10.0)));
        }
    }

    /// A kanban-shaped tree on one coordinator: drop target `a` wraps a
    /// column holding one draggable card, and a sibling drop target `b` sits
    /// beside it — the shape that exposed a drop target's click-to-drop gate
    /// admitting a *pointer* session (see [`DragTargetWidget::accepts_click_drop`]):
    /// the dragged card's own captured `Up` travels through `a`, its
    /// wrapping ancestor, on its way to the card, and `a` must let it pass
    /// through rather than answering it itself.
    struct Kanban {
        coordinator: DragCoordinator,
        a_entered: u32,
        a_dropped: Option<u32>,
        b_entered: u32,
        b_dropped: Option<u32>,
    }

    fn kanban_logic(state: &mut Kanban) -> crate::StackView<Kanban> {
        let card = super::super::draggable::draggable(
            SizedBox::<Kanban>(Some(80.0), Some(40.0)),
            state.coordinator.clone(),
            |_: &Kanban| 7u32,
        );
        let a = drag_target::<u32, Kanban, _>(
            SizedBox::<Kanban>(Some(100.0), Some(100.0)).child(crate::Column(vec![any(card)])),
            state.coordinator.clone(),
        )
        .on_enter(|s: &mut Kanban| s.a_entered += 1)
        .on_drop(|s: &mut Kanban, v: u32| s.a_dropped = Some(v));
        let b = drag_target::<u32, Kanban, _>(
            SizedBox::<Kanban>(Some(100.0), Some(100.0)),
            state.coordinator.clone(),
        )
        .on_enter(|s: &mut Kanban| s.b_entered += 1)
        .on_drop(|s: &mut Kanban, v: u32| s.b_dropped = Some(v));
        crate::Stack(vec![
            any(SizedBox::<Kanban>(Some(400.0), Some(400.0))),
            any(crate::Row(vec![any(a), any(b)])),
        ])
    }

    /// Drives [`kanban_logic`] through a real [`frust_core::RenderRoot`];
    /// `frame` is the rebuild/layout/paint a shell runs between events,
    /// which is what dispatches the `Housekeeping` broadcast a completed
    /// drop needs to fire `on_drop` (mirrors `reorderable`'s own test
    /// harness).
    struct KanbanHarness {
        root: frust_core::RenderRoot<Kanban, crate::StackView<Kanban>>,
        state: Kanban,
        clock_ms: f64,
    }

    impl KanbanHarness {
        fn new() -> Self {
            let mut h = KanbanHarness {
                root: frust_core::RenderRoot::new(),
                state: Kanban {
                    coordinator: DragCoordinator::new(),
                    a_entered: 0,
                    a_dropped: None,
                    b_entered: 0,
                    b_dropped: None,
                },
                clock_ms: 0.0,
            };
            h.frame();
            h
        }

        fn frame(&mut self) {
            let mut build: fn(&mut Kanban) -> crate::StackView<Kanban> = kanban_logic;
            self.root.rebuild(&mut build, &mut self.state);
            self.root.layout(Size::new(400.0, 400.0));
            self.clock_ms += 16.0;
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(
                &mut scene,
                frust_core::FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
            );
        }

        fn mouse(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &primary(phase, Point::new(x, y)));
        }

        fn touch(&mut self, phase: PointerPhase, x: f64, y: f64) {
            use frust_core::event::PointerId;
            self.root.event(
                &mut self.state,
                &InputEvent::PointerContact {
                    pointer_id: PointerId::touch(0),
                    event: frust_core::PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: frust_core::PointerButton::Primary,
                    },
                },
            );
        }
    }

    #[test]
    fn a_mouse_drag_dropped_on_the_sibling_column_fires_only_its_on_drop() {
        let mut h = KanbanHarness::new();
        // Press the card, inside column `a` (window 0..100, 0..100).
        h.mouse(PointerPhase::Down, 20.0, 20.0);
        // Cross the distance threshold without leaving `a`.
        h.mouse(PointerPhase::Move, 30.0, 20.0);
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);
        // Into column `b` (window 100..200, 0..100).
        h.mouse(PointerPhase::Move, 150.0, 50.0);
        let a_entered_before_release = h.state.a_entered;
        h.mouse(PointerPhase::Up, 150.0, 50.0);
        h.frame();
        assert_eq!(
            h.state.coordinator.phase(),
            DragPhase::Idle,
            "b claimed the drop"
        );
        assert_eq!(h.state.b_dropped, Some(7), "b's on_drop fired exactly once");
        assert_eq!(
            h.state.a_dropped, None,
            "a's on_drop never fires — the card left it before release"
        );
        assert_eq!(
            h.state.a_entered, a_entered_before_release,
            "the Up must not re-enter the source column (the old click-to-drop bug)"
        );
    }

    #[test]
    fn a_touch_long_press_dropped_on_the_sibling_column_fires_only_its_on_drop() {
        let mut h = KanbanHarness::new();
        h.touch(PointerPhase::Down, 20.0, 20.0);
        let mut frames = 0;
        while h.state.coordinator.phase() != DragPhase::Dragging {
            h.frame();
            frames += 1;
            assert!(frames < 60, "the hold never began a drag");
        }
        h.touch(PointerPhase::Move, 150.0, 50.0);
        h.frame();
        let a_entered_before_release = h.state.a_entered;
        h.touch(PointerPhase::Up, 150.0, 50.0);
        h.frame();
        assert_eq!(
            h.state.coordinator.phase(),
            DragPhase::Idle,
            "b claimed the drop"
        );
        assert_eq!(h.state.b_dropped, Some(7), "b's on_drop fired exactly once");
        assert_eq!(
            h.state.a_dropped, None,
            "a's on_drop never fires — the card left it before release"
        );
        assert_eq!(
            h.state.a_entered, a_entered_before_release,
            "the Up must not re-enter the source column (the old click-to-drop bug)"
        );
    }

    #[test]
    fn a_keyboard_sessions_click_to_drop_still_drops_on_the_cycled_target() {
        let mut h = KanbanHarness::new();
        let source = h.state.coordinator.new_source_id();
        h.state.coordinator.lift(source);
        h.state.coordinator.begin(7u32);
        // Cycle from nothing hovered onto `a`, then onto `b`.
        h.state.coordinator.move_to_next_target();
        h.state.coordinator.move_to_next_target();
        h.frame();
        assert_eq!(h.state.b_entered, 1, "cycling landed the hover on b");

        // A synthesized click lands on `b` directly — the same `Down`-then-`Up`
        // a shell's `ActionRequest(Action::Click)` delivers — and still drops,
        // because `b`'s click-to-drop gate stays open for a keyboard session.
        h.mouse(PointerPhase::Up, 150.0, 50.0);
        assert_eq!(h.state.b_dropped, Some(7), "the click delivered the drop");
        assert_eq!(h.state.coordinator.phase(), DragPhase::Idle);
    }
}
