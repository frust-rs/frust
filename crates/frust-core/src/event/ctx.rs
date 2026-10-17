use std::any::Any;

use kurbo::{Point, Size};

use super::contact::{
    current_pointer_id, note_capture_released, note_contacts_requested, under_contact_captor,
};
use super::ime::ImeState;
use super::key::EditCommand;
use super::pass::{CLIPBOARD_WRITE, CURSOR_REQUEST, EDIT_COMMAND_QUEUE, PASTE_REQUEST};
use super::pointer::{CursorIcon, PointerId};

/// What a widget did with an event.
///
/// `Handled` stops the enclosing container from offering the event to further
/// siblings and marks the frame dirty; `Ignored` lets routing continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    /// The widget did not consume the event.
    Ignored,
    /// The widget consumed the event.
    Handled,
}

/// The result of a whole [`crate::app::RenderRoot::event`] pass.
///
/// `handled` is whether any widget consumed the event; `needs_redraw` is whether
/// the shell should schedule a repaint (a handled event or an explicit
/// [`EventCtx::request_redraw`]). The shell turns `needs_redraw` into a
/// `window.request_redraw()` — the event pass itself never rebuilds or repaints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventOutcome {
    /// Whether the event was consumed by the tree.
    pub handled: bool,
    /// Whether the shell should schedule a redraw as a result.
    pub needs_redraw: bool,
}

/// Context threaded into [`crate::widget::Widget::event`].
///
/// Gives an event handler three capabilities: mutate the (type-erased)
/// application state, request a redraw, and capture the pointer. It also carries
/// the receiving widget's own geometry ([`EventCtx::origin`]/[`EventCtx::size`])
/// so handlers can do local-coordinate math (the event `position` is already in
/// the widget's local space; origin/size describe where that widget sits in and
/// how big it is within its parent).
///
/// `State` is erased as `&mut dyn Any` — the same pattern
/// [`crate::widget::LayoutCtx`] uses for the text context — so `frust-core`
/// carries no knowledge of the concrete app state type; a handler recovers it
/// with [`EventCtx::state_mut`].
pub struct EventCtx<'a> {
    state: &'a mut dyn Any,
    needs_redraw: bool,
    /// Set by [`EventCtx::request_layout`]; bubbles up through every
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) exactly
    /// like [`crate::widget::PaintCtx::request_layout`]'s flag does through
    /// `paint_child`, and is folded into the render root's pending
    /// [`ChangeFlags::LAYOUT`](crate::view::ChangeFlags::LAYOUT).
    needs_layout: bool,
    /// Whether a pod below this context was marked layout-dirty by this
    /// dispatch (a consumed event, a redraw or layout request) — the event
    /// pass's path bit, so the pod above marks itself too (see
    /// [`ChildPod::layout_child`](crate::widget::ChildPod::layout_child)).
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) folds it into its
    /// pending [`ChangeFlags::LAYOUT`](crate::view::ChangeFlags::LAYOUT) as well:
    /// a dirtied pod is only laid out by a pass that runs, and a shell gating
    /// its layout pass on those flags runs one only when they ask.
    layout_path_dirty: bool,
    capture_requested: bool,
    /// Set by [`EventCtx::capture_contacts`]; bubbles up beside
    /// `capture_requested` so a container can see the opt-in.
    contacts_requested: bool,
    /// Set by [`EventCtx::release_captured_child`] when the release took the
    /// gesture's contact opt-in off the active path; bubbles up beside
    /// `capture_requested` ([`EventCtx::is_capture_released`]).
    capture_released: bool,
    /// Which contact this dispatch carries ([`EventCtx::pointer_id`]); threaded
    /// unchanged from parent to child.
    pointer_id: PointerId,
    /// Set by [`EventCtx::request_focus`]; read by the enclosing container to
    /// record which child holds the focus path (mirrors `capture_requested`).
    focus_requested: bool,
    /// Set by [`EventCtx::release_focus`]; drops the recorded focus path.
    focus_released: bool,
    /// Whether the receiving widget currently holds focus (threaded down from its
    /// pod's recorded focus flag; seeded from the root focus state at the root).
    has_focus: bool,
    /// Whether the receiving widget is on the recorded hover path — the pointer is
    /// over it or over a descendant of it, as of the last completed hover pass.
    /// Threaded down from its
    /// pod's recorded hover stamp (see `hover_epoch`), seeded from
    /// [`crate::app::RenderRoot`]'s own hover mirror at the root. The event-pass
    /// mirror of [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered).
    hovered: bool,
    /// Set by [`EventCtx::claim_hover`]; read by the enclosing container, which
    /// stamps the claim onto its child's pod and bubbles it further up (the hover
    /// mirror of `focus_requested`).
    hover_claimed: bool,
    /// Whether a [`EventCtx::claim_hover`] call in this (sub)dispatch records
    /// anything at all. `false` unless the root marked this pass an uncaptured
    /// [`PointerPhase::Move`], and narrowed further on the way down: a pod holding
    /// the capture path, or a pass in which a claim was already recorded, hands
    /// its child an ineligible context. This is what makes "a captured pointer
    /// never creates hover" and "at most one claimant per pass" true by
    /// construction rather than by a check the root has to remember.
    hover_eligible: bool,
    /// The hover epoch of the last **completed** hover pass — what a pod's
    /// recorded stamp must equal for its link to still count
    /// ([`EventCtx::hover_epoch`]). A claim made during *this* pass records
    /// [`EventCtx::hover_claim_epoch`] instead (one past this value), because the
    /// root advances its own epoch when the pass ends.
    hover_epoch: u64,
    /// The identity of the [`crate::app::RenderRoot`] running this pass, stamped
    /// onto a pod beside the claim epoch ([`EventCtx::hover_root`]) so a pod
    /// dropped later can tell its own root's live link from another root's
    /// identically-numbered epoch. `0` outside a root-driven pass.
    hover_root: u64,
    /// The IME surface the focused widget published this dispatch, if any; bubbles
    /// up the focus chain to [`crate::app::RenderRoot`].
    ime_state: Option<ImeState>,
    origin: Point,
    size: Size,
}

impl<'a> EventCtx<'a> {
    /// Build a root event context over the erased application `state` for a
    /// widget placed at `origin` with `size`.
    pub fn new(state: &'a mut dyn Any, origin: Point, size: Size) -> Self {
        Self {
            state,
            needs_redraw: false,
            needs_layout: false,
            layout_path_dirty: false,
            capture_requested: false,
            contacts_requested: false,
            capture_released: false,
            // The contact the running root pass is dispatching, so a context
            // built mid-pass (a component's inner one) reports the same id its
            // enclosing context does; the mouse outside any pass.
            pointer_id: current_pointer_id(),
            focus_requested: false,
            focus_released: false,
            has_focus: false,
            hovered: false,
            hover_claimed: false,
            hover_eligible: false,
            hover_epoch: 0,
            hover_root: 0,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Recover the application state as `&mut T`.
    ///
    /// Panics if `T` is not the concrete state type the render root erased — a
    /// shell/wiring bug, not a runtime-data condition (mirrors
    /// [`crate::widget::LayoutCtx::text_context`]).
    pub fn state_mut<T: Any>(&mut self) -> &mut T {
        self.state
            .downcast_mut::<T>()
            .expect("event state is not the expected application-state type")
    }

    /// Request that the shell schedule a repaint after this event pass.
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Whether a redraw was requested during this (sub)dispatch.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Request that layout re-run on the next pass because this event changed
    /// layout-affecting state the widget holds itself (not through a rebuild).
    ///
    /// The event-pass mirror of [`crate::widget::PaintCtx::request_layout`]: the
    /// flag bubbles up through every
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child), marking
    /// each pod on the way layout-dirty (so the layout skip lays the whole path
    /// out again — see
    /// [`ChildPod::layout_child`](crate::widget::ChildPod::layout_child)), and
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) folds it into its
    /// pending [`ChangeFlags::LAYOUT`](crate::view::ChangeFlags::LAYOUT), so a
    /// shell gating layout on that seam runs it. It also implies
    /// [`EventCtx::request_redraw`], as the paint one implies a frame request: a
    /// layout change has to be shown.
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        self.needs_redraw = true;
        self.layout_path_dirty = true;
    }

    /// Whether a layout was requested during this (sub)dispatch
    /// ([`EventCtx::request_layout`]).
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// Whether this (sub)dispatch dirtied a pod below this context (see the
    /// `layout_path_dirty` field).
    pub(crate) fn is_layout_path_dirty(&self) -> bool {
        self.layout_path_dirty
    }

    /// Fold a child dispatch's layout request and layout-dirty path bit back
    /// into this context — the layout half of [`EventCtx::absorb_child`], kept
    /// separate so the existing fold's arity is untouched. Also the seam a
    /// context substituted across a state boundary
    /// ([`crate::component::ComponentWidget`]) mirrors its inner context through.
    pub(crate) fn absorb_layout(&mut self, child_needs_layout: bool, child_path_dirty: bool) {
        if child_needs_layout {
            self.needs_layout = true;
            self.needs_redraw = true;
        }
        self.layout_path_dirty |= child_path_dirty || child_needs_layout;
    }

    /// Capture the pointer: subsequent moves/releases should route back to this
    /// widget. The [`ChildPod::event_child`](crate::widget::ChildPod::event_child)
    /// call that delivered the event reads [`EventCtx::is_pointer_captured`]
    /// after the dispatch returns and records the active path on its pod; the
    /// container clears it on `Up`/`Cancel`.
    ///
    /// **Capture is a `Down`-time concept here.** For a hit-tested pointer event,
    /// only the `Down` arm of [`RenderRoot::event`](crate::app::RenderRoot::event)
    /// folds a request into the root's own capture mirror, so a capture opened
    /// from a `Move` records the pod's active path (routing works) while the root
    /// still reads uncaptured. (A floated overlay surface's own input is the one
    /// exception: the root mirrors a capture claimed through it on any phase —
    /// see [`InputEvent::Overlay`](crate::event::InputEvent::Overlay).) For hover that means a `Move` that both
    /// captures and [`claim_hover`](EventCtx::claim_hover)s records the claim — the pod's
    /// eligibility gate reads the active flag as it stood *before* this dispatch —
    /// and then lapses on the next `Move`, where the now-active pod is ineligible.
    /// A gesture that wants hover chrome for its whole drag keeps its own pressed
    /// flag rather than relying on the link.
    pub fn capture_pointer(&mut self) {
        self.capture_requested = true;
    }

    /// Whether the widget requested pointer capture during this (sub)dispatch.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_requested
    }

    /// Which pointer contact this event comes from.
    ///
    /// [`PointerId::MOUSE`] unless the dispatch says otherwise: a shell's bare
    /// [`InputEvent::Pointer`](crate::event::InputEvent::Pointer) is the mouse, and a touch contact arrives as
    /// [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact), which the root unwraps into
    /// `InputEvent::Pointer` while this reports its id. Meaningful for pointer
    /// events only; any other event reports whatever contact the pass carries
    /// (the mouse at the top level).
    ///
    /// A widget that tracks one gesture at a time never needs this — the root
    /// only ever delivers it the claimant's contact unless it opted in with
    /// [`EventCtx::capture_contacts`]. One that did opt in tells the contacts
    /// apart with it.
    pub fn pointer_id(&self) -> PointerId {
        self.pointer_id
    }

    /// Opt into the **other** contacts of the gesture this widget is capturing:
    /// call it on the same `Down` that calls [`EventCtx::capture_pointer`], and
    /// while that capture lives every additional contact's
    /// `Down`/`Move`/`Up`/`Cancel` is delivered here down the captured path, as
    /// [`InputEvent::Pointer`](crate::event::InputEvent::Pointer) with [`EventCtx::pointer_id`] naming the contact.
    /// A pinch or rotate recognizer is the intended caller.
    ///
    /// The opt-in is read with the capture it accompanies: on a `Down` that
    /// captures nothing it does nothing, and it ends with the capture.
    ///
    /// # Multi-contact contract
    ///
    /// * **(a)** With no live capture, a slot-`0` contact is hit-tested exactly
    ///   like a plain [`InputEvent::Pointer`](crate::event::InputEvent::Pointer); the `Down` this widget captures
    ///   on makes that contact the gesture's **claimant**.
    /// * **(b)** With no live capture, a contact on slot `1` or above is dropped
    ///   at the root — so an additional finger only ever reaches a widget
    ///   through this opt-in.
    /// * **(c)** While the capture lives, the claimant's events arrive as usual;
    ///   every other contact's events arrive only because of this call (a
    ///   captor that did not make it never sees them), and **only here**: the
    ///   containers between the root and this widget on the recorded active
    ///   path forward them without running their own pointer handling (see
    ///   [`ChildPod::event_child`](crate::widget::ChildPod::event_child)), so an
    ///   enclosing scroll view or gesture detector never mistakes a second
    ///   finger for its own. Only the claimant's `Up`/`Cancel` ends the
    ///   capture — another contact's `Up`/`Cancel` is delivered but releases
    ///   nothing, so the handler must not treat it as the end of the gesture.
    ///   When the claimant's `Up`/`Cancel` arrives, the captor must drop every
    ///   other contact it was tracking: their later events no longer reach it.
    ///   The opt-in also ends early if an enclosing container takes the gesture
    ///   over and releases this widget from the active path
    ///   ([`EventCtx::release_captured_child`]).
    ///
    /// See [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact) for the full contract.
    pub fn capture_contacts(&mut self) {
        self.contacts_requested = true;
        note_contacts_requested();
    }

    /// Whether a widget opted into the gesture's other contacts
    /// ([`EventCtx::capture_contacts`]) during this (sub)dispatch — the
    /// container-side read, bubbled by
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) exactly
    /// like [`EventCtx::is_pointer_captured`]. The root does not depend on the
    /// bubble: it reads the opt-in from the pass it opened, so a component
    /// boundary (whose fresh inner context this flag does not cross) cannot
    /// hide it.
    pub fn is_contact_capture_requested(&self) -> bool {
        self.contacts_requested
    }

    /// Release a captured child: the container-side half of a **takeover**, for
    /// a container that cancels the gesture its captured child was handling and
    /// keeps the gesture for itself (a scroll view crossing its drag slop).
    ///
    /// Clears `child`'s recorded active path exactly like
    /// [`ChildPod::set_active`](crate::widget::ChildPod::set_active)`(false)`
    /// (call it after delivering the child its `Cancel`), and when the released
    /// subtree held the widget that opted into the gesture's other contacts
    /// ([`EventCtx::capture_contacts`]) it also tells the root, which then stops
    /// routing those contacts — the widget that asked for them is no longer on
    /// the active path, and nothing else asked. The signal bubbles like
    /// [`EventCtx::is_pointer_captured`] ([`EventCtx::is_capture_released`]).
    ///
    /// The capture itself stays with the gesture's claimant: the container that
    /// took over is still on the active path (it was the released child's
    /// ancestor), so the claimant's later events keep reaching it and only the
    /// claimant's `Up`/`Cancel` ends the gesture. A container that wants the
    /// other contacts for itself calls [`EventCtx::capture_contacts`] after
    /// this, in the same dispatch.
    ///
    /// Nothing is released or signalled while the child holds no active path,
    /// while a non-claimant contact is being delivered (whose `Up`/`Cancel`
    /// must never break the claimant's gesture — see
    /// [`ChildPod::set_active`](crate::widget::ChildPod::set_active)), or when
    /// the opted-in widget is this container or one of its ancestors (it stays
    /// on the active path, so its opt-in stands).
    pub fn release_captured_child(&mut self, child: &mut crate::widget::ChildPod) {
        if !child.is_active() {
            return;
        }
        let held_opt_in = child.holds_contact_opt_in();
        child.set_active(false);
        if child.is_active() || !held_opt_in || under_contact_captor() {
            return;
        }
        self.capture_released = true;
        note_capture_released();
    }

    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this (sub)dispatch
    /// ([`EventCtx::release_captured_child`]) — bubbled by
    /// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) exactly
    /// like [`EventCtx::is_pointer_captured`]. As with the opt-in, the root reads
    /// the release from the pass it opened, so a component boundary cannot hide
    /// it.
    pub fn is_capture_released(&self) -> bool {
        self.capture_released
    }

    /// Request focus: subsequent keyboard/IME events should route to this widget.
    ///
    /// The [`ChildPod::event_child`](crate::widget::ChildPod::event_child) call
    /// that delivered the event reads the flag after the dispatch returns and
    /// records its pod as the focused path, stamped with the live focus session
    /// (the focus mirror of [`EventCtx::capture_pointer`]); the request bubbles,
    /// so every pod up to the root records it. Focus-routed events are delivered
    /// down that recorded chain with no hit test.
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Release focus: drop the recorded focus path (e.g. Escape / blur).
    pub fn release_focus(&mut self) {
        self.focus_released = true;
    }

    /// Whether the receiving widget currently holds the focus path.
    ///
    /// Threaded down from the widget's pod ([`crate::widget::ChildPod::is_focused`]);
    /// a keyboard/IME event only reaches a widget along this chain, so a widget
    /// handling such an event is by construction focused.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Claim the hover link: the pointer is over *this* widget, so the next paint
    /// pass reports [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered)
    /// for it — and, because the claim is recorded as a path, for every ancestor
    /// enclosing it as well (see the [module docs](crate::event)).
    ///
    /// # The consumer contract
    ///
    /// Three things together, all three required:
    ///
    /// 1. **Claim from the [`PointerPhase::Move`](crate::event::PointerPhase::Move) arm**, once the widget has
    ///    hit-tested the event's `position` inside its own bounds — the same local
    ///    test a press arm does on `Up`.
    /// 2. **Keep an internal hover flag**, updated from that same hit test, and
    ///    gate `request_redraw` on its *changed*-return. This call requests no
    ///    frame of its own (below), and the root manufactures one only when a hover
    ///    ends with nothing taking it — so a widget without this flag paints no
    ///    hover chrome on entry, and none when the link moves from a sibling to it.
    /// 3. **Read [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered) in
    ///    `paint` and self-correct the flag from it.** It is authoritative: the
    ///    flag can be stale (a pointer that left the widget never delivers it
    ///    another event; a container clearing or lapsing a link never tells the
    ///    widget either), and this read is what fixes it.
    ///
    /// ```ignore
    /// PointerPhase::Move => {
    ///     if !self.captured {
    ///         // Uncaptured move: this is the hover pass.
    ///         let over = inside(p.position, ctx.size());
    ///         if over { ctx.claim_hover(); }
    ///         if self.state_layer.set_hovered(over) { ctx.request_redraw(); }
    ///         return EventResult::Ignored;
    ///     }
    ///     // ... captured drag handling
    /// }
    ///
    /// fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
    ///     // Authoritative; corrects the flag above whenever it went stale.
    ///     self.state_layer.set_hovered(ctx.is_hovered());
    ///     // ... paint the overlay
    /// }
    /// ```
    ///
    /// **Claim on every qualifying `Move`, not just on entry.** The claim is
    /// per-pass, not sticky: a widget that stops claiming stops being hovered on
    /// the next hover pass. That is the mechanism, not a defect — it is what makes
    /// "the pointer moved somewhere else" self-clearing with no leave event to
    /// deliver.
    ///
    /// **A container claims *after* routing the `Move` to its children, never
    /// before.** One claim per pass is recorded and the first one recorded wins, so
    /// an ancestor that claims before it forwards makes every descendant ineligible
    /// for the pass: the child under the pointer reads
    /// [`is_hovered`](EventCtx::is_hovered) `== false` forever while step 2 above
    /// keeps flipping its flag and asking for a frame on every move — hover chrome
    /// that never appears, plus a repaint per event. Claiming after routing is
    /// correct in every case: a descendant's claim is recorded first and wins, the
    /// container's own late call is then a silent no-op yet it still reads hovered
    /// through the stamped path (below), and when no descendant claims, the
    /// container's claim is what records, so its own chrome still works. A
    /// container therefore never arbitrates — it orders.
    ///
    /// **Its sibling channel resolves the opposite way.** A claim is
    /// *first*-writer-wins; [`set_cursor`](EventCtx::set_cursor) is
    /// *last*-writer-wins. So the same "claim/ask after routing" placement means
    /// two different things in one handler: the container's claim is a **fallback**
    /// its child beats, while the container's cursor request is an **override**
    /// that beats its child's. A container that wants the child's cursor to win
    /// must ask *before* it routes — the mirror image of the ordering here.
    ///
    /// # When it does nothing
    ///
    /// A call is silently ignored unless the pass is hover-eligible: a captured
    /// pointer (anywhere on the path), any phase other than an uncaptured `Move`,
    /// and any claim after the first one in the same pass all record nothing. A
    /// **leaf** therefore never has to ask whether claiming is allowed — it claims
    /// whenever the pointer is over it and the pipeline decides. A **container**
    /// gets the same freedom only by claiming after it routes: it never asks
    /// either, but *when* it claims decides whether its children may, per the
    /// ordering rule above.
    ///
    /// A `Down`, `Up`, or `Cancel` *ends* whatever hover stood without opening a
    /// new one, so a consumer re-claims on the next `Move` rather than expecting
    /// its chrome to survive a click.
    ///
    /// # It does not request a redraw
    ///
    /// Deliberately: a pointer moving *within* one widget claims on every event,
    /// and repainting each time would be pure waste. The widget owns the change
    /// detection instead — which is what makes step 2 above part of the contract
    /// rather than an optimization.
    pub fn claim_hover(&mut self) {
        if self.hover_eligible {
            self.hover_claimed = true;
        }
    }

    /// Whether the receiving widget **or a descendant of it** holds the hover link
    /// — i.e. whether the last completed hover pass recorded a claim path running
    /// through this widget.
    ///
    /// So a container reads `true` while the pointer is over a claiming child
    /// (CSS `:hover` semantics), and a widget that never claims can still read
    /// `true` when a descendant does; a sibling or any other off-path widget reads
    /// `false`.
    ///
    /// Threaded down from the widget's pod
    /// ([`ChildPod::hover_epoch`](crate::widget::ChildPod::hover_epoch) against
    /// the live epoch) and seeded at the root from `RenderRoot`'s hover mirror, so
    /// it reflects state as of *before* this dispatch: a
    /// [`claim_hover`](EventCtx::claim_hover) made in this pass does not flip it.
    /// Mirrors [`EventCtx::has_focus`]; the paint-pass form is
    /// [`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered), which is the
    /// authoritative read for a widget's own hover chrome.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Ask the host to show `icon` while the pointer is where it is now.
    ///
    /// The request is per-pass and stateless, exactly like
    /// [`request_redraw`](EventCtx::request_redraw) and
    /// [`claim_hover`](EventCtx::claim_hover): it says what the cursor should be
    /// *for this pass*, and a widget that stops asking falls back to
    /// [`CursorIcon::Default`] with nothing to clear.
    ///
    /// # When to call it
    ///
    /// From a [`PointerPhase::Move`](crate::event::PointerPhase::Move) arm, on the same hit test a
    /// [`claim_hover`](EventCtx::claim_hover) rides — the two are siblings, and a
    /// widget that wants hover chrome usually wants a cursor too:
    ///
    /// ```ignore
    /// PointerPhase::Move => {
    ///     if !self.captured {
    ///         if inside(p.position, ctx.size()) {
    ///             ctx.claim_hover();
    ///             ctx.set_cursor(CursorIcon::Pointer);
    ///         }
    ///         return EventResult::Ignored;
    ///     }
    ///     // Captured drag: this widget owns the pass, so its request wins
    ///     // wherever the pointer has gone.
    ///     ctx.set_cursor(CursorIcon::Grabbing);
    ///     // ... drag handling
    /// }
    /// ```
    ///
    /// **Ask on every `Move`, not just on entry**, and ask from the captured
    /// `Move`s too if a drag should keep its own shape: a captured pass routes
    /// only to the capturing widget, so re-asking there is what keeps a
    /// `Grabbing` cursor alive while the pointer is dragged outside the widget's
    /// own bounds.
    ///
    /// # Which pass the root actually resolves
    ///
    /// Only a pointer [`PointerPhase::Move`](crate::event::PointerPhase::Move) — captured or not — re-resolves the
    /// cursor ([`crate::app::RenderRoot::cursor`]). A request made on any other
    /// pass records nothing, and, just as importantly, no other pass *resets* the
    /// cursor: a `Down`/`Up` whose handlers say nothing about the cursor leaves
    /// the standing shape alone rather than blinking it back to `Default` for the
    /// duration of a click. A widget wanting a press-specific cursor therefore
    /// keys it off its own pressed state from the `Move` arm rather than setting
    /// it on `Down`.
    ///
    /// # Last writer wins
    ///
    /// One value is resolved per pass, and the last `set_cursor` of the pass is
    /// it. Because a container routes to its child from the middle of its own
    /// handler, the innermost widget the route reaches normally speaks last and
    /// therefore wins — which is what makes a specific control override the
    /// generic surface behind it. A container that deliberately overrides its
    /// children sets the cursor *after* routing.
    ///
    /// **Note the asymmetry with [`claim_hover`](EventCtx::claim_hover)**, which
    /// is first-writer-wins: a container claiming after routing yields hover to
    /// its child, while a container asking for a cursor after routing overrides
    /// its child. Placing the two calls side by side in one `Move` arm — the
    /// example above — is correct precisely because a leaf has no child to order
    /// against; a *container* writing both has to place them separately.
    ///
    /// # It does not request a redraw
    ///
    /// Deliberately, for [`claim_hover`](EventCtx::claim_hover)'s reason: a
    /// pointer moving within one widget re-asks on every event, and the shell
    /// applies the resolved cursor whether or not a frame is painted.
    ///
    /// Takes `&mut self` like every other request on this context even though the
    /// pass's request slot is not a field of it (`CURSOR_REQUEST`, above): asking
    /// is something a widget does *through its context*, and keeping the signature
    /// honest about that leaves the storage free to move.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        CURSOR_REQUEST.with(|slot| slot.set(Some(icon)));
    }

    /// Ask the shell to put `text` on the host clipboard.
    ///
    /// The answer to an [`InputEvent::EditCommand`](crate::event::InputEvent::EditCommand)`(`[`EditCommand::Copy`]`)` or
    /// [`EditCommand::Cut`]: the widget owns the selection, so it is the only
    /// thing that can say what "copy" means, and the shell owns the host
    /// clipboard, so it is the only thing that can perform the write. A widget
    /// with nothing selected simply does not call this, and nothing is written.
    ///
    /// ```ignore
    /// InputEvent::EditCommand(EditCommand::Cut) => {
    ///     if let Some(sel) = self.selected_text() {
    ///         ctx.write_clipboard(sel);
    ///         self.delete_selection();
    ///         ctx.request_redraw();
    ///     }
    ///     EventResult::Handled
    /// }
    /// ```
    ///
    /// # Pass-scoped, last writer wins
    ///
    /// The request rides the same kind of per-pass slot as
    /// [`set_cursor`](EventCtx::set_cursor) (`CLIPBOARD_WRITE`, bracketed by the
    /// same [`RequestPass`](crate::event::RequestPass) guard), so exactly one write is resolved per dispatch
    /// and the pass's last caller is it — which, since a container routes to its
    /// child from the middle of its own handler, normally makes the innermost
    /// widget the route reaches the one that speaks. Nothing accumulates between
    /// passes and there is nothing to clear: a widget that stops copying stops
    /// writing.
    ///
    /// The root resolves the slot at the end of **every** pass (not just a
    /// clipboard one — a copy can be answered from a key chord a widget decoded
    /// itself), and a shell drains it with
    /// [`RenderRoot::take_clipboard_write`](crate::app::RenderRoot::take_clipboard_write)
    /// immediately after the dispatch, beside
    /// [`cursor()`](crate::app::RenderRoot::cursor) and
    /// [`ime_state()`](crate::app::RenderRoot::ime_state).
    ///
    /// # It does not request a redraw
    ///
    /// [`set_cursor`](EventCtx::set_cursor)'s reason: copying paints nothing. A
    /// `Cut` that mutates the document asks for its own redraw, for the mutation.
    ///
    /// Takes `&mut self` like every other request on this context even though the
    /// pass's slot is not a field of it: asking is something a widget does
    /// *through its context*, and keeping the signature honest about that leaves
    /// the storage free to move.
    pub fn write_clipboard(&mut self, text: String) {
        CLIPBOARD_WRITE.with(|slot| slot.set(Some(text)));
    }

    /// Ask the shell to read the host clipboard and deliver it back as an
    /// [`InputEvent::EditCommand`](crate::event::InputEvent::EditCommand)`(`[`EditCommand::Paste`]`)`.
    ///
    /// The inverse of [`write_clipboard`](EventCtx::write_clipboard), and the
    /// reason a paste arrives with its text already attached: only the shell can
    /// touch the host clipboard, so a widget that wants a paste it was not given
    /// — an in-widget context-menu item, a chord the widget decoded itself —
    /// raises this flag and receives the text on a *later* dispatch rather than
    /// inline.
    ///
    /// # Idempotent, pass-scoped, and answered out of band
    ///
    /// Data-free: two widgets asking in one pass owe exactly one clipboard read,
    /// because there is one host clipboard and one focused widget to deliver it
    /// to. The flag rides a per-pass slot bracketed by the same [`RequestPass`](crate::event::RequestPass)
    /// guard as the cursor, so an ask made outside any dispatch is dropped rather
    /// than leaking into the next pass; the root resolves it at the end of every
    /// pass and a shell drains it with
    /// [`RenderRoot::take_paste_request`](crate::app::RenderRoot::take_paste_request).
    ///
    /// The answer is a **new dispatch**, never a return value: the shell's read
    /// may be asynchronous (a permission prompt, a cross-process fetch), and by
    /// the time it lands the pass that asked is long over. The synthesized
    /// [`EditCommand::Paste`] carries text and no identity of its own, and focus
    /// routing hands it to whoever holds focus *at delivery*: a **release** does
    /// drop it — with nothing focused it reaches no widget — but a focus *move*
    /// lands it in the new field, not in the one that asked.
    ///
    /// A synchronous read has no in-flight window and needs no guard. An
    /// asynchronous one must bind its answer to the session that asked:
    /// snapshot [`RenderRoot::focus_epoch`](crate::app::RenderRoot::focus_epoch)
    /// (reached shell-side as `AppTree::focus_epoch`) when the request is
    /// drained, and drop an answer whose epoch no longer matches — never
    /// [`focus_ime_generation`](crate::app::RenderRoot::focus_ime_generation),
    /// which an edit or a caret move inside one session also moves.
    ///
    /// A `Cut` may write and ask in the same pass; the two slots are independent.
    pub fn request_paste(&mut self) {
        PASTE_REQUEST.with(|slot| slot.set(true));
    }

    /// Hand an [`EditCommand`] to whichever widget drains the queue later in
    /// **this** pass — the widget-to-widget half of the clipboard story.
    ///
    /// # Why this is not just an `InputEvent::EditCommand`
    ///
    /// A selection toolbar and the text input it acts on are two different
    /// widgets, and the toolbar is a pod its owner floats rather than contains
    /// (see [`crate::overlay`]), so there is no container path from the toolbar's
    /// "Copy" tap back down to the field. Re-entering
    /// [`crate::app::RenderRoot::event`] with a focus-routed
    /// [`InputEvent::EditCommand`](crate::event::InputEvent::EditCommand) would be the other option, and is worse: a
    /// dispatch may not re-enter the root (see that method's reentrancy
    /// contract), and the toolbar's tap is *already* being routed as an overlay
    /// broadcast when it decides. So the verb rides a pass-scoped FIFO the owner
    /// drains the instant its forward returns, and applies to the field itself —
    /// synchronously, inside the same dispatch.
    ///
    /// Order is preserved: commands drain in the order they were dispatched.
    ///
    /// A command nobody takes before the pass ends is **dropped** (with a
    /// debug-build diagnostic) rather than carried into the next pass, where it
    /// would apply to whatever happened to be selected by then.
    pub fn dispatch_edit_command(&mut self, cmd: EditCommand) {
        EDIT_COMMAND_QUEUE.with(|slot| {
            let mut queue = slot.take();
            queue.push(cmd);
            slot.set(queue);
        });
    }

    /// Drain everything [`EventCtx::dispatch_edit_command`] queued so far in this
    /// pass, in dispatch order, leaving the queue empty.
    ///
    /// An overlay owner calls this immediately after forwarding an event into its
    /// floated pod, and applies what comes back to itself. Draining the queue
    /// (rather than peeking) is what keeps a verb from being applied twice when
    /// two owners forward in the same pass.
    pub fn take_edit_commands(&mut self) -> Vec<EditCommand> {
        EDIT_COMMAND_QUEUE.with(|slot| slot.take())
    }

    /// Publish this widget's IME surface (editing state + caret) for the shell.
    ///
    /// The value bubbles up the focus chain to [`crate::app::RenderRoot`], where
    /// the shell reads it via [`crate::app::RenderRoot::ime_state`]. Called by the
    /// focused editable widget after any state change so the platform IME stays in
    /// sync.
    ///
    /// Core carries the whole [`ImeState`] — including its
    /// [`ImeContentType`](ImeState::content_type) hint — opaquely: nothing
    /// between here and the shell inspects or rewrites it.
    pub fn publish_ime_state(&mut self, state: ImeState) {
        self.ime_state = Some(state);
    }

    /// Whether this widget requested focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_requested(&self) -> bool {
        self.focus_requested
    }

    /// Whether this widget released focus during this (sub)dispatch (container-side).
    pub(crate) fn is_focus_released(&self) -> bool {
        self.focus_released
    }

    /// Take the IME surface published during this (sub)dispatch, leaving `None`.
    pub(crate) fn take_ime_state(&mut self) -> Option<ImeState> {
        self.ime_state.take()
    }

    /// Seed whether the receiving (root) widget holds focus — used by
    /// [`crate::app::RenderRoot::event`] when it dispatches straight to the root.
    pub(crate) fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// Whether a widget claimed hover during this (sub)dispatch (container-side).
    pub(crate) fn is_hover_claimed(&self) -> bool {
        self.hover_claimed
    }

    /// Whether a [`EventCtx::claim_hover`] call in this (sub)dispatch would record
    /// anything — read by [`crate::widget::ChildPod::event_child`], which narrows
    /// it further before handing it to a child.
    pub(crate) fn is_hover_eligible(&self) -> bool {
        self.hover_eligible
    }

    /// Seed whether the receiving (root) widget holds the hover link — the hover
    /// mirror of [`EventCtx::set_has_focus`].
    pub(crate) fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    /// Seed whether this pass may record a hover claim at all. Called by
    /// [`crate::app::RenderRoot::event`], which sets it only for an **uncaptured**
    /// [`PointerPhase::Move`].
    pub(crate) fn set_hover_eligible(&mut self, eligible: bool) {
        self.hover_eligible = eligible;
    }

    /// Seed the live hover epoch (the last completed hover pass's). Called by
    /// [`crate::app::RenderRoot::event`] at the root and threaded unchanged into
    /// every child by [`crate::widget::ChildPod::event_child`].
    pub(crate) fn set_hover_epoch(&mut self, epoch: u64) {
        self.hover_epoch = epoch;
    }

    /// The live hover epoch: a pod whose recorded stamp equals this still holds
    /// the hover link.
    pub(crate) fn hover_epoch(&self) -> u64 {
        self.hover_epoch
    }

    /// Seed the identity of the root running this pass. Called by
    /// [`crate::app::RenderRoot::event`] at the root and threaded unchanged into
    /// every child by [`crate::widget::ChildPod::event_child`], which stamps it
    /// beside the claim epoch.
    pub(crate) fn set_hover_root(&mut self, root: u64) {
        self.hover_root = root;
    }

    /// The identity of the root running this pass — stamped onto a claiming pod
    /// so its destructor can qualify its epoch (see [`mark_hover_orphaned`]).
    pub(crate) fn hover_root(&self) -> u64 {
        self.hover_root
    }

    /// The epoch a claim recorded during *this* pass takes — one past the live
    /// one, because [`crate::app::RenderRoot::event`] advances its epoch when the
    /// hover pass ends. Wrapping is deliberate and harmless: the stamp is only
    /// ever compared for equality, never ordered, and a wrap would need 2^64 hover
    /// passes to collide with a link recorded before it.
    pub(crate) fn hover_claim_epoch(&self) -> u64 {
        self.hover_epoch.wrapping_add(1)
    }

    /// The receiving widget's origin in its **parent's** coordinate space.
    ///
    /// Not the window-space origin, and **not** the same frame of reference as
    /// [`PaintCtx::origin`](crate::widget::PaintCtx::origin), which is absolute:
    /// the paint pass accumulates each child's parent-relative offset onto its
    /// parent's already-absolute origin, while the event pass instead translates
    /// the *event* into the child's local space
    /// ([`ChildPod::event_child`](crate::widget::ChildPod::event_child)) and hands
    /// down the pod's own offset unaccumulated. So an event position is already
    /// local (compare it against `Point::ZERO` and [`EventCtx::size`], never
    /// against this), and anything anchored in window space — an overlay, a
    /// popup, a reported rect — must be computed from `PaintCtx::origin` in
    /// `paint`, not from this value.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The receiving widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Create a fresh sub-context for a child at `origin`/`size`, reborrowing the
    /// same erased state. The child's `needs_redraw`/`capture_requested`/focus and
    /// hover-claim flags start clear; `has_focus` reflects the child pod's recorded
    /// focus flag, `hovered` its recorded hover link, and `hover_eligible` whether
    /// the child may claim hover at all (the caller narrows it — see
    /// [`crate::widget::ChildPod::event_child`]). The live hover epoch, the
    /// running root's identity and the dispatch's [`EventCtx::pointer_id`] are
    /// threaded down unchanged (a claim anywhere in the subtree is stamped with
    /// the first two). The parent folds the results back in with
    /// [`EventCtx::absorb_child`].
    pub(crate) fn child_ctx(
        &mut self,
        origin: Point,
        size: Size,
        focused: bool,
        hovered: bool,
        hover_eligible: bool,
    ) -> EventCtx<'_> {
        EventCtx {
            state: &mut *self.state,
            needs_redraw: false,
            needs_layout: false,
            layout_path_dirty: false,
            capture_requested: false,
            contacts_requested: false,
            capture_released: false,
            pointer_id: self.pointer_id,
            focus_requested: false,
            focus_released: false,
            has_focus: focused,
            hovered,
            hover_claimed: false,
            hover_eligible,
            hover_epoch: self.hover_epoch,
            hover_root: self.hover_root,
            ime_state: None,
            origin,
            size,
        }
    }

    /// Fold a child dispatch's redraw/capture/contact-opt-in/capture-release/
    /// hover-claim/focus flags (and any published IME surface) back into this
    /// context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn absorb_child(
        &mut self,
        child_needs_redraw: bool,
        child_captured: bool,
        child_contacts_requested: bool,
        child_capture_released: bool,
        child_hover_claimed: bool,
        child_focus_requested: bool,
        child_focus_released: bool,
        child_ime_state: Option<ImeState>,
    ) {
        self.needs_redraw |= child_needs_redraw;
        self.capture_requested |= child_captured;
        self.contacts_requested |= child_contacts_requested;
        self.capture_released |= child_capture_released;
        // A claim bubbles like a focus request: every pod between the claimant and
        // the root records it, so the whole path carries the same stamp.
        self.hover_claimed |= child_hover_claimed;
        self.focus_requested |= child_focus_requested;
        self.focus_released |= child_focus_released;
        if child_ime_state.is_some() {
            self.ime_state = child_ime_state;
        }
    }
}
