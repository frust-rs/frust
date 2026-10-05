//! Drag-and-drop session model: the [`DragCoordinator`] handle every
//! draggable source and drop target in one scope shares.
//!
//! The coordinator is pure logic — no widget, no paint, no event routing. It
//! is the single source of truth for "is a drag in flight, what does it
//! carry, where is the pointer, and which target is under it", and the
//! widgets built on it (a draggable source, a drop target, cross-container
//! resolution, OS file drops) read and drive it rather than each keeping a
//! copy.
//!
//! # Ambient, not global
//!
//! An app creates one [`DragCoordinator`] per scope that should share drags
//! (usually one per window) and hands a clone to every source and target in
//! it — the same cloneable-handle shape as
//! [`ScrollController`](crate::ScrollController). Every clone shares one
//! state; two coordinators never see each other's sessions, so a source can
//! only ever land on a target holding the same handle. Source and target ids
//! ([`DragSourceId`]/[`DragTargetId`]) are allocated by the coordinator
//! ([`DragCoordinator::new_source_id`]/[`DragCoordinator::new_target_id`]) and
//! are unique within it, not across coordinators.
//!
//! # The state machine
//!
//! ```text
//! Idle  ──arm──▶  Armed  ──begin──▶  Dragging  ──drop (hovered)──▶  Dropping  ──complete_drop──▶  Idle
//! Idle  ──begin_external──────────▶  Dragging
//! Armed | Dragging | Dropping  ──cancel──▶  Cancelled  ──▶  Idle
//! ```
//!
//! - **Idle → Armed** ([`DragCoordinator::arm`]): a source saw a press that may
//!   become a drag. Re-arming while armed (or while an unclaimed drop is
//!   still `Dropping`) cancels the earlier one first; arming while a drag is
//!   already in flight is ignored (debug-build log) — the session in flight
//!   owns the pointer.
//! - **Armed → Dragging** ([`DragCoordinator::begin`]): the source crossed its
//!   initiation policy and hands over the typed payload. `begin` without an
//!   arm is an error: it is ignored with a debug-build log. A second `begin`
//!   while `Dragging` cancels the first session and restarts from the same
//!   source with the new payload, at the first session's current pointer.
//! - **Idle → Dragging** ([`DragCoordinator::begin_external`]): an
//!   OS-originated drag ([`DragKind::ExternalFiles`]) has no source and skips
//!   `Armed`; its payload is the `Vec<PathBuf>` itself. It supersedes any
//!   session in flight (cancelling it first).
//! - **Dragging → Dropping** ([`DragCoordinator::drop`]): released over a
//!   hovered target. The session stays `Dropping` — payload still held —
//!   until the target claims it with [`DragCoordinator::take_payload`] and
//!   calls [`DragCoordinator::complete_drop`] (→ `Idle`). A drop with nothing
//!   hovered behaves exactly as a cancel.
//! - **→ Cancelled → Idle** ([`DragCoordinator::cancel`]): legal from every
//!   non-`Idle` phase and always lands on `Idle`; a target still hovered (or
//!   the target of an unclaimed drop) hears a `Leave` first. `Cancelled` is a
//!   transient phase: it is reported as the
//!   `Dragging → Cancelled`, `Cancelled → Idle` notification pair, but
//!   [`DragCoordinator::state`] already reads `Idle` by the time any
//!   subscriber hears either.
//!
//! Pointer moves ([`DragCoordinator::update_pointer`]) and hover changes
//! ([`DragCoordinator::set_hovered`]) apply only while `Dragging` and are
//! silent no-ops otherwise, so a per-frame resolution pass may call them
//! unconditionally.
//!
//! # Payload
//!
//! The payload is a `Box<dyn Any>` with its `TypeId` recorded, read through
//! [`DragCoordinator::payload_is`], [`DragCoordinator::payload_ref`] (a
//! [`Ref`](std::cell::Ref) guard into the shared state — do not hold it across
//! a call that mutates the coordinator, or that call panics on the borrow),
//! [`DragCoordinator::with_payload`] (the closure form, borrow scoped to the
//! closure) and moved out with [`DragCoordinator::take_payload`] while
//! `Dragging` or `Dropping`.
//!
//! # Notifications
//!
//! [`DragCoordinator::subscribe`] registers a listener for every
//! [`DragStateChange`] and returns a [`DragSubscription`] guard that
//! unsubscribes on drop. Every change is delivered to every live subscriber,
//! in subscription order, and every subscriber sees every change in the
//! order the changes happened: a listener may call back into the coordinator
//! (the borrow is released before any listener runs), and the changes that
//! re-entrant call produces are queued behind the ones still being delivered
//! rather than delivered nested. Every mutation that produces a change also
//! raises [`frust_core::mark_pending_result_flush`] so a frame-gated shell
//! runs the frame that repaints it.
//!
//! # Sources
//!
//! [`draggable()`] is the in-app source built on the coordinator: it arms on a
//! primary press, begins the session once the press crosses its [`DragPolicy`]
//! (a distance threshold for a mouse, a stationary long-press for a finger),
//! floats a ghost through the overlay portal at the pointer for the session's
//! length, and drops or cancels on the gesture's `Up`/`Cancel`. A press that
//! never becomes a drag reaches the wrapped child untouched. See the
//! [`mod@draggable`] module docs for the initiation, ghost and feedback contract.

mod coordinator;
pub mod draggable;

pub use coordinator::{
    DragCoordinator, DragKind, DragPhase, DragSession, DragSourceId, DragState, DragStateChange,
    DragSubscription, DragTargetId,
};
pub use draggable::{
    DRAG_THRESHOLD, DragPolicy, DraggableView, DraggableWidget, GHOST_OPACITY, SourceFeedback,
    draggable,
};
