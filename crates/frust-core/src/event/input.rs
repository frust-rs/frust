use std::path::PathBuf;

use kurbo::{Affine, Point, Vec2};

use crate::overlay::OverlayKey;

use super::ime::ImeEvent;
use super::key::{EditCommand, KeyEvent};
use super::pointer::{PointerEvent, PointerId, ScaleEvent, ScalePhase, ScrollDelta};

/// What an [`InputEvent::Overlay`] carries into a floated surface.
///
/// The key names the surface's owner (see [`OverlayKey`]); the kind is the input
/// itself, always in **absolute window space** rather than in anyone's local
/// space — see [`OverlayEventKind`].
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayEvent {
    /// The owner whose registered surface the root hit. Every other widget in
    /// the tree sees this broadcast and must ignore it.
    pub key: OverlayKey,
    /// What happened.
    pub kind: OverlayEventKind,
}

/// The input an [`OverlayEvent`] delivers.
///
/// # Window space, not local space
///
/// Every position here is absolute logical window space, deliberately: the
/// broadcast reaches the owner by travelling the *main* tree, so the translation
/// chain it passes through on the way (`ChildPod::event_child` subtracting each
/// container's origin) describes the owner's position, not the floated pod's.
/// Translating the payload would therefore corrupt it. The owner instead
/// subtracts its own registered
/// [`window_rect`](crate::overlay::OverlayEntry::window_rect) origin before
/// forwarding into the pod, which is the only offset that means anything — which
/// is also why [`InputEvent::translated`] returns an overlay event unchanged.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayEventKind {
    /// A pointer event inside the surface's rect, positioned in window space.
    Pointer(PointerEvent),
    /// A scroll inside the surface's rect, positioned in window space.
    Scroll {
        /// Where the scroll occurred, in absolute window space.
        position: Point,
        /// How much to scroll.
        delta: ScrollDelta,
    },
    /// A scale gesture inside the surface's rect, focal point in window
    /// space — the overlay mirror of [`InputEvent::Scale`], routed here on
    /// exactly the same hit-test terms as [`Scroll`](OverlayEventKind::Scroll).
    Scale {
        /// Where the gesture is centered, in absolute window space.
        focal: Point,
        /// The gesture phase.
        phase: ScalePhase,
        /// The multiplicative scale change this event represents.
        scale_delta: f64,
        /// The gesture's current rate of scale change, per second.
        velocity: f64,
    },
    /// A primary press landed outside **every** registered surface — the
    /// light-dismiss notification, delivered only to entries registered
    /// [`OutsideTap::Notify`](crate::overlay::OutsideTap::Notify). It carries no
    /// position: where the press landed is the main tree's business, and an
    /// owner that wants it can register `consume: false` and watch the press
    /// arrive there normally.
    OutsideDown,
}

/// The phase of a desktop OS file drag entering, hovering over, dropping on,
/// or leaving the window — see [`InputEvent::FileDrop`].
///
/// A shell publishes only the first three. [`Ended`](FileDropPhase::Ended) is
/// the root's own broadcast follow-up to every `Drop`/`Cancel`, handled or
/// not — see [`FileDropEvent`]'s *Broadcast follow-up* section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDropPhase {
    /// The drag is hovering the window — it just entered, or moved while
    /// still over it. Carries no paths; see [`FileDropEvent::paths`].
    Hover,
    /// The files were released over the window. Carries every dropped path.
    Drop,
    /// The drag left the window, or the OS otherwise called it off, without a
    /// drop. Carries no paths.
    Cancel,
    /// **Broadcast-only, never sent by a shell**: the OS drag is over — a
    /// `Drop` or `Cancel` was just dispatched hit-tested, whether or not a
    /// widget handled it — so [`crate::app::RenderRoot::event`] re-dispatches
    /// this phase to every widget. Carries no paths and no meaningful
    /// position ([`Point::ZERO`], never translated). A widget holding state
    /// the drag opened (a drop target's external drag session) ends whatever
    /// of it is still open here, so a release over a region nothing handles —
    /// or over a widget other than the one whose state the drag opened —
    /// still ends the drag everywhere.
    Ended,
}

/// A desktop OS file drag/drop event — hit-tested and bubbling exactly like
/// [`InputEvent::Scroll`], by `position`, so a [`crate::widget::ChildPod`]
/// container routes it with no change of its own (see
/// [`InputEvent::translated`]/[`InputEvent::transformed`]).
///
/// # Source
///
/// Only a desktop shell publishes this today: `frust-shell-desktop` maps
/// winit's `HoveredFile`/`DroppedFile`/`HoveredFileCancelled` window events
/// onto the three [`FileDropPhase`]s (see that crate's `app_handler` module
/// for the exact mapping, including why `Drop`'s `paths` is a coalesced
/// batch rather than one event per file). Android, iOS and web publish
/// nothing — none of those embeddings surfaces an OS file-drag signal to this
/// framework, so a `frust-widgets` drop target never engages on those hosts
/// (see `docs/LIMITATIONS.md`).
///
/// # Broadcast follow-up
///
/// A shell resolves a `Drop` or `Cancel` at the last cursor position it
/// knows, which can sit over a region no widget handles a file drop in — and
/// on a platform that sends no cursor motion during an OS drag, that is the
/// last in-window position before the drag began, not the release point.
/// Even a `Drop` or `Cancel` some widget handles reaches only that one
/// widget, while the drag may have opened state in others along the way
/// (two independent drop-target groups, each hovered in turn). So after the
/// hit-tested dispatch of **every** `Drop` or `Cancel`, handled or
/// [`EventResult::Ignored`](crate::event::EventResult::Ignored), [`crate::app::RenderRoot::event`] immediately
/// dispatches a second event in the same call: this struct with
/// [`FileDropPhase::Ended`], which [`InputEvent::is_broadcast`] reports as a
/// broadcast, so every container forwards it to every child. A widget that
/// already finished the drag on the hit-tested event finds nothing left to
/// end in it, so answering `Ended` must be idempotent.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDropEvent {
    /// The gesture phase.
    pub phase: FileDropPhase,
    /// Where the drag is, in the receiving widget's local logical space —
    /// translated like [`InputEvent::Scroll`]'s `position` by the container
    /// chain that routes it.
    pub position: Point,
    /// The dropped files — populated only for [`FileDropPhase::Drop`].
    /// [`FileDropPhase::Hover`]/[`FileDropPhase::Cancel`]/
    /// [`FileDropPhase::Ended`] always carry an empty list: the platform
    /// source delivers one file per hover notification with no "this is the
    /// complete set" signal, so the full list is only ever knowable once
    /// files are actually released.
    ///
    /// **Untrusted input.** The paths arrive verbatim from the OS drag
    /// source — whatever application the user dragged from — and nothing
    /// between it and the receiving widget validates them: a path may be a
    /// symlink to somewhere else, a directory, a device node, a file of any
    /// size, or carry a name built to mislead when displayed. A consumer must
    /// treat them as untrusted: resolve symlinks (and re-check where they
    /// lead) before opening, check the type and size before reading, and
    /// sanitise a file name before showing it.
    pub paths: Vec<PathBuf>,
}

/// An input event delivered to the widget tree.
///
/// Pointer gestures, scroll, scale, and a file drop are **hit-tested** (routed
/// by position); keyboard, IME, and edit-command events are **focus-routed**
/// — delivered straight down the recorded focus chain with no hit test and no
/// meaningful position (see [`crate::widget::ChildPod`]'s focus bookkeeping
/// and `frust-widgets`' `route_event`). [`InputEvent::Housekeeping`] and
/// [`InputEvent::Overlay`] are neither: they are **broadcasts** that reach
/// every child unconditionally.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// A pointer (mouse/touch/pen) gesture event.
    ///
    /// From a shell it means the same as
    /// [`PointerContact`](InputEvent::PointerContact) with [`PointerId::MOUSE`].
    /// It is also the **only** pointer form a widget ever receives: the root
    /// unwraps a `PointerContact` into this variant and reports the contact's
    /// identity through [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id).
    Pointer(PointerEvent),
    /// One identified pointer contact — the shell-facing carrier for a touch
    /// contact (or any pointer that is not [`PointerId::MOUSE`]).
    ///
    /// **Widgets never receive this variant.**
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) unwraps it and
    /// dispatches [`InputEvent::Pointer`]`(event)` with
    /// [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id) reporting `pointer_id`, so every existing
    /// `match` on `InputEvent::Pointer` keeps working unchanged and a widget
    /// that does not care which contact it is seeing never has to ask.
    ///
    /// # Multi-contact contract
    ///
    /// The root decides what a contact does from its id and the capture latch
    /// it holds (the latch records the **claimant**: the id whose `Down` took the
    /// capture).
    ///
    /// * **(a) Slot 0 with no live capture is hit-tested exactly like
    ///   [`InputEvent::Pointer`].** The overlay pre-pass, hover, focus and
    ///   blur-on-outside-tap bookkeeping are the same code path, so a
    ///   single-finger gesture behaves identically whichever carrier delivered
    ///   it. A capture taken on its `Down` latches `pointer_id` as the claimant.
    /// * **(b) Slot 1 and above with no live capture is dropped at the root.**
    ///   Additional contacts exist only inside a captured gesture; one that
    ///   arrives while nothing holds the pointer reaches no widget and moves no
    ///   root state.
    /// * **(c) While a capture is live, routing is keyed on the claimant.** The
    ///   claimant's own events take the captured path exactly as before. An
    ///   event from **any other id** is delivered down the same captured path
    ///   to the captor — as `InputEvent::Pointer`, with
    ///   [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id) reporting that id — only if the captor called
    ///   [`EventCtx::capture_contacts`](crate::event::EventCtx::capture_contacts) on the `Down` it captured with;
    ///   otherwise it is dropped at the root. **Only the claimant's
    ///   `Up`/`Cancel` releases the capture.** Another contact's `Up`/`Cancel`
    ///   never does — at the root or in any container's recorded active path —
    ///   so a finger lifting elsewhere cannot break a mouse drag, nor a mouse
    ///   release a touch drag.
    ///
    ///   **The captor is the only widget that sees another contact.** The
    ///   delivery walks the recorded active path *forward-only*: every
    ///   container between the root and the captor hands it on without its own
    ///   pointer handling running (see
    ///   [`ChildPod::event_child`](crate::widget::ChildPod::event_child) for the
    ///   mechanism), so a scroll view or gesture detector enclosing a pinch
    ///   recognizer never sees the second finger as a `Down` of its own; the
    ///   captor's own handler, and whatever it routes below itself, run as
    ///   usual. If the walk cannot reach the captor through a container (an
    ///   overlay owner whose captured pod is a floated surface), that container
    ///   alone is handed the event the ordinary way.
    ///
    ///   **A takeover ends the opt-in.** A container that cancels the captor
    ///   and keeps the gesture for itself releases it with
    ///   [`EventCtx::release_captured_child`](crate::event::EventCtx::release_captured_child); the root then stops routing the
    ///   other contacts (they fall under rule (c)'s drop branch), while the
    ///   claimant keeps the capture — now held by that container — until its own
    ///   `Up`/`Cancel`.
    ///
    /// A delivered non-claimant contact is not a gesture of its own: it opens or
    /// moves no capture, takes no hover pass, resolves no cursor, and blurs
    /// nothing (an explicit [`EventCtx::request_focus`](crate::event::EventCtx::request_focus)/[`EventCtx::release_focus`](crate::event::EventCtx::release_focus)
    /// from its handler is still honoured, as it is for a scroll). When the
    /// claimant's `Up`/`Cancel` ends the capture, the captor must treat every
    /// other contact it was tracking as ended too: their later events fall under
    /// rule (b) and never reach it.
    ///
    /// A bare [`InputEvent::Pointer`] from a shell is this variant with
    /// [`PointerId::MOUSE`], so a host that emits only `Pointer` (desktop, web)
    /// sees exactly the single-pointer behaviour it always had.
    PointerContact {
        /// Which contact this is.
        pointer_id: PointerId,
        /// The contact's event, positioned like any [`InputEvent::Pointer`].
        event: PointerEvent,
    },
    /// A scroll event at `position` (local logical space) carrying `delta`.
    Scroll {
        /// Where the scroll occurred, in the receiving widget's local space.
        position: Point,
        /// How much to scroll.
        delta: ScrollDelta,
    },
    /// A scale (pinch/zoom) gesture event — hit-tested exactly like
    /// [`Scroll`](InputEvent::Scroll), by its [`ScaleEvent::focal`] point, and
    /// bubbles up the tree until a widget reports [`EventResult::Handled`](crate::event::EventResult::Handled).
    /// See [`ScaleEvent`] for the field contract.
    Scale(ScaleEvent),
    /// A keyboard key event, routed down the focus path (no hit test).
    Key(KeyEvent),
    /// An IME event, routed down the focus path (no hit test).
    Ime(ImeEvent),
    /// A decoded clipboard / selection command, routed down the focus path (no
    /// hit test) exactly like [`Key`](InputEvent::Key) and [`Ime`](InputEvent::Ime).
    ///
    /// Focus-routed rather than hit-tested because a clipboard verb is *about
    /// the selection*, and the selection lives wherever focus is — a `Cmd+V`
    /// carries no pointer position, and an edit-menu tap's position is the
    /// menu's, not the field's. Focus routing is also what makes a paste with
    /// nothing focused a harmless no-op: the event reaches no widget and is
    /// dropped, so a shell may answer a stale paste request unconditionally
    /// (see [`EventCtx::request_paste`](crate::event::EventCtx::request_paste)).
    EditCommand(EditCommand),
    /// **Not user input**: a state-bearing housekeeping pass, broadcast to the
    /// whole tree so a widget that queued a callback needing `&mut State` during
    /// a state-free `BuildCtx` pass can run it.
    ///
    /// # Why it exists
    ///
    /// [`crate::app::RenderRoot::rebuild`] is the only unconditional per-frame
    /// pass holding `&mut State`, and it hands that state to the build closure alone —
    /// the view diff itself (and therefore every `View::rebuild`, where a
    /// navigator applies its queued push/pop ops) is state-free. A widget that
    /// needs to call back into app state from there had, before this variant, no
    /// pass to run in except the *next event*, which on a touch device may be
    /// seconds away or may never reach that widget at all (a pop-result
    /// callback measured 3.2s late on device, and was lost entirely when the
    /// next tap was consumed by chrome outside the navigator).
    /// `rebuild` now dispatches this variant instead, so the deferred callback
    /// runs on the very frame that queued it.
    ///
    /// # Routing contract
    ///
    /// **Broadcast, never consumed.** It carries no position, is not hit-tested,
    /// and is not focus-routed: a container forwards it to *every* child
    /// unconditionally (before any capture/focus/hit-test branch) and reports
    /// [`EventResult::Ignored`](crate::event::EventResult::Ignored) regardless of what the children returned, so no
    /// "first handler wins" short-circuit can hide a subtree from it. A leaf
    /// widget with nothing deferred simply ignores it — the fall-through is
    /// harmless by construction. It never opens or releases a capture, never
    /// moves focus, and never blurs.
    ///
    /// # Naming
    ///
    /// Deliberately *not* `Tick`: `Tick` already means frame pacing in this
    /// codebase ([`crate::widget::TickClass`]), and this variant has nothing to
    /// do with the frame gate.
    Housekeeping,
    /// **Not user input either**: one floated overlay surface's own input,
    /// broadcast to the whole tree so it reaches the owner that registered the
    /// surface, wherever in the tree that owner sits.
    ///
    /// # Why a broadcast
    ///
    /// The owner of a floated surface is an ordinary widget somewhere in the
    /// tree, and the pointer that hit its surface is nowhere near its own bounds
    /// — that is the entire point of floating. Hit-testing the event would
    /// therefore deliver it to whatever the main tree has under the pointer, and
    /// focus-routing it would deliver it to a text field that has nothing to do
    /// with the surface. Broadcasting is the only route that reaches the owner
    /// without knowing where it is, so this is the **second** broadcast variant
    /// (see [`InputEvent::is_broadcast`]), and every routing helper's existing
    /// broadcast-first branch already forwards it correctly with no change.
    ///
    /// # Routing contract
    ///
    /// **Only the owner whose [`OverlayKey`] matches acts on it; every other
    /// widget ignores it.** A container forwards it to every child
    /// unconditionally — no hit test, no capture fast path, no focus gate — and
    /// reports [`EventResult::Ignored`](crate::event::EventResult::Ignored) regardless, exactly like
    /// [`Housekeeping`](InputEvent::Housekeeping). A widget that is not an
    /// overlay owner, or whose key differs, must fall through: the key
    /// comparison is the whole addressing mechanism.
    ///
    /// At the root it is inert in the ways a broadcast must be — it advances no
    /// hover epoch and never blurs — but, unlike `Housekeeping`, it *is* a real
    /// user gesture underneath, so a focus request or a pointer capture bubbled
    /// from inside the surface is honoured (see
    /// [`crate::app::RenderRoot::event`]).
    Overlay(OverlayEvent),
    /// A desktop OS file drag entering, hovering over, dropping on, or
    /// leaving the window — hit-tested and bubbling exactly like
    /// [`InputEvent::Scroll`], by [`FileDropEvent::position`]. See
    /// [`FileDropEvent`] for the phase/path contract and which shells
    /// publish it.
    ///
    /// One phase travels differently: [`FileDropPhase::Ended`] is a
    /// **broadcast** ([`InputEvent::is_broadcast`]), the root's own follow-up
    /// to every `Drop`/`Cancel`, handled or not, so a widget holding state
    /// the drag opened hears that the drag is over even when the release
    /// missed it (see [`FileDropEvent`]'s *Broadcast follow-up*). It carries
    /// no position and is never translated, like the other broadcasts.
    ///
    /// Deliberately **not** routed through the overlay pre-pass
    /// ([`crate::app::RenderRoot::event`]'s `route_overlay`): a native OS
    /// drag is a window-level signal with no floated-surface concept on the
    /// platform side, so it is hit-tested straight against the main tree —
    /// a drop target living inside a popover is a gap this variant does not
    /// close (see `docs/LIMITATIONS.md`).
    FileDrop(FileDropEvent),
}

impl InputEvent {
    /// The event's location, in the receiving widget's local coordinate space.
    ///
    /// [`InputEvent::Scale`] reports its [`ScaleEvent::focal`] point here, the
    /// same way [`InputEvent::Scroll`] reports `position`, and
    /// [`InputEvent::FileDrop`] reports its own [`FileDropEvent::position`]
    /// identically. Focus-routed events
    /// ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the broadcasts
    /// ([`Housekeeping`](InputEvent::Housekeeping),
    /// [`Overlay`](InputEvent::Overlay), and a file drop's
    /// [`FileDropPhase::Ended`] follow-up) have no spatial position — they are
    /// delivered down the focus chain, or to every child, not hit-tested — so
    /// this reports [`Point::ZERO`] for them; callers must never hit-test on it
    /// (routing helpers early-return both classes). An overlay event's *payload*
    /// does carry a position, but in window space rather than in the receiver's
    /// local space, which is precisely why it is not reported here (see
    /// [`OverlayEventKind`]).
    pub fn position(&self) -> Point {
        match self {
            InputEvent::Pointer(p) | InputEvent::PointerContact { event: p, .. } => p.position,
            InputEvent::Scroll { position, .. } => *position,
            InputEvent::Scale(scale) => scale.focal,
            InputEvent::FileDrop(drop) if drop.phase == FileDropPhase::Ended => Point::ZERO,
            InputEvent::FileDrop(drop) => drop.position,
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => Point::ZERO,
        }
    }

    /// Return a copy of this event with its position shifted by `offset`.
    ///
    /// Containers use this (with `offset = -child_origin`) to translate an event
    /// from their own coordinate space into a child's local space before
    /// forwarding it — see [`crate::widget::ChildPod::event_child`].
    /// [`InputEvent::Scale`] shifts its [`ScaleEvent::focal`] point the same way
    /// [`InputEvent::Scroll`] shifts its `position`, and
    /// [`InputEvent::FileDrop`] shifts [`FileDropEvent::position`] identically.
    /// Focus-routed
    /// events ([`InputEvent::Key`]/[`InputEvent::Ime`]/
    /// [`InputEvent::EditCommand`]) and the
    /// [`Housekeeping`](InputEvent::Housekeeping) broadcast carry no position, so
    /// they are returned unchanged (cloned). An
    /// [`Overlay`](InputEvent::Overlay) event is returned unchanged for the
    /// opposite reason — its payload carries a **window-space** position that the
    /// container chain between the root and the owner must not shift, since that
    /// chain describes where the *owner* sits and not where the floated surface
    /// does (see [`OverlayEventKind`]).
    pub fn translated(&self, offset: Vec2) -> InputEvent {
        match self {
            InputEvent::Pointer(p) => InputEvent::Pointer(PointerEvent {
                position: p.position + offset,
                ..*p
            }),
            InputEvent::PointerContact { pointer_id, event } => InputEvent::PointerContact {
                pointer_id: *pointer_id,
                event: PointerEvent {
                    position: event.position + offset,
                    ..*event
                },
            },
            InputEvent::Scroll { position, delta } => InputEvent::Scroll {
                position: *position + offset,
                delta: *delta,
            },
            InputEvent::Scale(scale) => InputEvent::Scale(ScaleEvent {
                focal: scale.focal + offset,
                ..*scale
            }),
            InputEvent::FileDrop(drop) if drop.phase == FileDropPhase::Ended => self.clone(),
            InputEvent::FileDrop(drop) => InputEvent::FileDrop(FileDropEvent {
                position: drop.position + offset,
                phase: drop.phase,
                paths: drop.paths.clone(),
            }),
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => self.clone(),
        }
    }

    /// Return a copy of this event with its position mapped through `affine` —
    /// the general form of [`InputEvent::translated`], for a container that
    /// places a child under an arbitrary transform
    /// ([`crate::widget::ChildPod::set_transform`]).
    ///
    /// Maps exactly the positions `translated` shifts, and leaves alone exactly
    /// what it leaves alone: [`InputEvent::Pointer`]'s position, the inner event
    /// of an [`InputEvent::PointerContact`], [`InputEvent::Scroll`]'s `position`,
    /// [`InputEvent::Scale`]'s [`ScaleEvent::focal`], and
    /// [`InputEvent::FileDrop`]'s [`FileDropEvent::position`] are mapped; the
    /// focus-routed events, the [`Housekeeping`](InputEvent::Housekeeping)
    /// broadcast and the window-space [`Overlay`](InputEvent::Overlay) payload are
    /// returned unchanged (cloned), for the reasons `translated` gives.
    ///
    /// Only *positions* are mapped. A scroll `delta`, a scale's multiplicative
    /// `scale_delta` and its `velocity` are carried over as-is: they describe the
    /// gesture's magnitude in the input device's terms, not a point in the
    /// receiver's space.
    ///
    /// A container routing into a transformed child passes the **inverse** of the
    /// child's local→container mapping here; the caller owns checking that the
    /// inverse exists (see [`crate::hit::checked_inverse`]).
    pub fn transformed(&self, affine: &Affine) -> InputEvent {
        match self {
            InputEvent::Pointer(p) => InputEvent::Pointer(PointerEvent {
                position: *affine * p.position,
                ..*p
            }),
            InputEvent::PointerContact { pointer_id, event } => InputEvent::PointerContact {
                pointer_id: *pointer_id,
                event: PointerEvent {
                    position: *affine * event.position,
                    ..*event
                },
            },
            InputEvent::Scroll { position, delta } => InputEvent::Scroll {
                position: *affine * *position,
                delta: *delta,
            },
            InputEvent::Scale(scale) => InputEvent::Scale(ScaleEvent {
                focal: *affine * scale.focal,
                ..*scale
            }),
            InputEvent::FileDrop(drop) if drop.phase == FileDropPhase::Ended => self.clone(),
            InputEvent::FileDrop(drop) => InputEvent::FileDrop(FileDropEvent {
                position: *affine * drop.position,
                phase: drop.phase,
                paths: drop.paths.clone(),
            }),
            InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => self.clone(),
        }
    }

    /// Whether this event is focus-routed (delivered down the focus chain with no
    /// hit test) rather than hit-tested by position.
    ///
    /// [`Housekeeping`](InputEvent::Housekeeping) is **not** focus-routed — it
    /// reaches every child, focused or not; see
    /// [`is_broadcast`](InputEvent::is_broadcast).
    pub fn is_focus_routed(&self) -> bool {
        matches!(
            self,
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::EditCommand(_)
        )
    }

    /// Whether this event is a broadcast: forwarded to **every** child
    /// unconditionally, with no hit test, no capture fast-path, and no focus
    /// routing — [`InputEvent::Housekeeping`], [`InputEvent::Overlay`], and an
    /// [`InputEvent::FileDrop`] in its [`FileDropPhase::Ended`] phase.
    ///
    /// Every routing helper branches on this **first**, before its capture,
    /// focus, and hit-test branches (`frust-widgets`'
    /// `route_event`/`route_event_single`, and this crate's own
    /// [`crate::component`] mirror), so a broadcast can never be swallowed by a
    /// captured child or a `contains()` miss. That existing branch is exactly
    /// what carries an overlay event to its owner, and a file drag's `Ended`
    /// follow-up to every drop target, with no router change: the three differ
    /// in what they *mean* (a deferred callback flush, one floated surface's
    /// own input, an OS drag that is over), not in how they travel.
    pub fn is_broadcast(&self) -> bool {
        matches!(
            self,
            InputEvent::Housekeeping
                | InputEvent::Overlay(_)
                | InputEvent::FileDrop(FileDropEvent {
                    phase: FileDropPhase::Ended,
                    ..
                })
        )
    }
}
