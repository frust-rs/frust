//! The [`InputEvent`] enum the tree receives, plus its overlay and file-drop payloads.

use std::path::PathBuf;

use kurbo::{Affine, Point, Vec2};

use crate::overlay::OverlayKey;
use crate::shortcuts::ShortcutId;

use super::ime::ImeEvent;
use super::key::{EditCommand, KeyEvent};
use super::pointer::{PointerEvent, PointerId, ScaleEvent, ScalePhase, ScrollDelta};

/// What an [`InputEvent::Overlay`] carries: the owner's key and the input, in window space.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayEvent {
    pub key: OverlayKey,
    pub kind: OverlayEventKind,
}

/// An [`OverlayEvent`]'s input, in window space (the owner subtracts its rect origin itself).
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayEventKind {
    Pointer(PointerEvent),
    Scroll {
        position: Point,
        delta: ScrollDelta,
    },
    Scale {
        focal: Point,
        phase: ScalePhase,
        scale_delta: f64,
        velocity: f64,
    },
    /// A primary press outside every surface (light dismiss), sent only to `Notify` entries.
    OutsideDown,
    /// A shortcut the pod declared; the owner forwards it as [`InputEvent::Shortcut`].
    Shortcut(ShortcutId),
}

/// The phase of a desktop OS file drag over the window. A shell publishes `Hover`, `Drop` and
/// `Cancel`; [`Ended`](FileDropPhase::Ended) is the root's own broadcast follow-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDropPhase {
    Hover,
    Drop,
    Cancel,
    /// Broadcast-only, after every `Drop`/`Cancel`: the drag is over, so end any state it opened.
    Ended,
}

/// A desktop OS file drag/drop event, hit-tested and bubbling like [`InputEvent::Scroll`].
///
/// # Source
/// Only `frust-shell-desktop` publishes it, mapping winit's `HoveredFile`/`DroppedFile`/
/// `HoveredFileCancelled`; Android, iOS and web publish nothing (see `docs/LIMITATIONS.md`).
///
/// # Broadcast follow-up
/// After the hit-tested dispatch of every `Drop`/`Cancel`, handled or not, the root broadcasts
/// [`FileDropPhase::Ended`] to every widget in the same call; answering it must be idempotent.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDropEvent {
    pub phase: FileDropPhase,
    /// Where the drag is, in the receiving widget's local logical space.
    pub position: Point,
    /// The dropped files, populated only for `Drop`. Untrusted OS input: resolve symlinks, check
    /// type and size before opening, and sanitise a name before showing it.
    pub paths: Vec<PathBuf>,
}

/// An input event delivered to the widget tree. Pointer, scroll, scale and file-drop events are
/// hit-tested; key, IME and edit-command events are focus-routed; the rest are broadcasts.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// A pointer gesture event — the only pointer form a widget receives; from a shell, the mouse.
    Pointer(PointerEvent),
    /// One identified pointer contact from a shell. Widgets never receive it: the root dispatches
    /// `event` as [`InputEvent::Pointer`] and reports `pointer_id` via `EventCtx::pointer_id`.
    ///
    /// # Multi-contact contract
    /// * **(a)** With no live capture, slot `0` is hit-tested exactly like `Pointer`; a capture
    ///   taken on its `Down` latches this id as the gesture's **claimant**.
    /// * **(b)** With no live capture, slot `1` and above is dropped at the root.
    /// * **(c)** While a capture is live, the claimant's events take the captured path; any other
    ///   id reaches only the captor (enclosing containers forward it unhandled), and only if it
    ///   called `EventCtx::capture_contacts` on its capturing `Down`; otherwise it is dropped.
    ///   Only the claimant's `Up`/`Cancel` releases the capture, and a container takeover
    ///   (`EventCtx::release_captured_child`) ends the opt-in.
    PointerContact {
        pointer_id: PointerId,
        event: PointerEvent,
    },
    /// A scroll event at `position` carrying `delta`.
    Scroll {
        /// Where the scroll occurred, in the receiving widget's local logical space.
        position: Point,
        delta: ScrollDelta,
    },
    /// A pinch/zoom event, hit-tested by [`ScaleEvent::focal`] and bubbling until handled.
    Scale(ScaleEvent),
    /// A keyboard key event, routed down the focus path.
    Key(KeyEvent),
    /// An IME event, routed down the focus path.
    Ime(ImeEvent),
    /// A decoded clipboard/selection command, routed down the focus path; with nothing focused it
    /// is dropped, so a shell may answer a stale paste request unconditionally.
    EditCommand(EditCommand),
    /// Not user input: a broadcast, never consumed, that lets a widget run a callback it queued
    /// needing `&mut State` during a state-free build pass, on the frame that queued it.
    Housekeeping,
    /// A floated overlay surface's own input: a broadcast only the owner holding the matching
    /// [`OverlayKey`] acts on; every other widget ignores it.
    Overlay(OverlayEvent),
    /// A desktop OS file drag; see [`FileDropEvent`]. Not routed through the overlay pre-pass.
    FileDrop(FileDropEvent),
    /// A broadcast naming the shortcut a key pressed (`crate::shortcuts`); the declarer answers.
    Shortcut(ShortcutId),
}

impl InputEvent {
    /// The event's location in the receiving widget's local space; [`Point::ZERO`] for
    /// focus-routed events and broadcasts, which callers must never hit-test on.
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
            | InputEvent::Overlay(_)
            | InputEvent::Shortcut(_) => Point::ZERO,
        }
    }

    /// A copy with its position shifted by `offset` (a container passes `-child_origin`).
    /// Positionless events and the window-space `Overlay` payload are returned unchanged.
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
            | InputEvent::Overlay(_)
            | InputEvent::Shortcut(_) => self.clone(),
        }
    }

    /// A copy with exactly the positions `translated` shifts mapped through `affine` (the inverse
    /// of the child's transform); deltas and velocities are carried over as-is.
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
            | InputEvent::Overlay(_)
            | InputEvent::Shortcut(_) => self.clone(),
        }
    }

    /// Whether this event is focus-routed (key, IME, edit command) rather than hit-tested.
    pub fn is_focus_routed(&self) -> bool {
        matches!(
            self,
            InputEvent::Key(_) | InputEvent::Ime(_) | InputEvent::EditCommand(_)
        )
    }

    /// Whether this event is a broadcast — `Housekeeping`, `Overlay`, `Shortcut`, or a file drop's
    /// `Ended` — forwarded to every child before any capture/focus/hit-test branch.
    pub fn is_broadcast(&self) -> bool {
        matches!(
            self,
            InputEvent::Housekeeping
                | InputEvent::Overlay(_)
                | InputEvent::Shortcut(_)
                | InputEvent::FileDrop(FileDropEvent {
                    phase: FileDropPhase::Ended,
                    ..
                })
        )
    }
}
