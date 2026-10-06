//! [`DragCoordinator`]: the shared drag-session state machine. See the
//! [module docs](super) for the session model, transition rules and
//! notification contract.
//!
//! # Keyboard sessions
//!
//! [`DragCoordinator::lift`] begins a session the same way
//! [`DragCoordinator::arm`] + [`DragCoordinator::begin`] do, but with no
//! pointer driving it: [`DragSession::keyboard`] marks the result, and
//! [`DragCoordinator::update_pointer`]/[`DragCoordinator::resolve_hover`]
//! both no-op for one rather than resolving a registry point that was never
//! reported. Hover instead advances explicitly, through
//! [`DragCoordinator::move_to_next_target`]/
//! [`DragCoordinator::move_to_previous_target`] — cycling the registered
//! targets in registration ("tree") order, wrapping at either end, and
//! starting at the first (next) or last (previous) target when nothing is
//! hovered yet. Both raise the same `Leave`/`Enter` notification pair
//! pointer resolution does, so a target subscriber (or poller — see
//! [`mod@super::target`]) sees identical callbacks regardless of which drove
//! the session. [`DragCoordinator::drop`]/[`DragCoordinator::cancel`] are
//! unchanged: a keyboard session drops on whatever
//! [`move_to_next_target`](DragCoordinator::move_to_next_target)/
//! [`move_to_previous_target`](DragCoordinator::move_to_previous_target)
//! left hovered (or cancels, with nothing hovered) exactly like a pointer
//! release does. The ghost's placement for a keyboard session — anchored at
//! the source's own bounds on [`lift`](DragCoordinator::lift), then at the
//! hovered target's bounds on every `Enter` — is
//! [`mod@super::draggable`]'s concern, not the coordinator's; see its
//! module docs.

use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use kurbo::{Point, Rect, Vec2};

/// Identity of a draggable source, allocated by
/// [`DragCoordinator::new_source_id`] and unique within that coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DragSourceId(u64);

impl DragSourceId {
    /// The raw id.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Identity of a drop target, allocated by
/// [`DragCoordinator::new_target_id`] and unique within that coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DragTargetId(u64);

impl DragTargetId {
    /// The raw id.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Where a drag session came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragKind {
    /// Started in-app by a source through [`DragCoordinator::arm`] +
    /// [`DragCoordinator::begin`]; carries whatever payload the source handed
    /// over.
    Internal,
    /// An OS file drag entering the window, started by
    /// [`DragCoordinator::begin_external`]. Has no source; its payload is the
    /// same `Vec<PathBuf>` this variant lists.
    ExternalFiles(Vec<PathBuf>),
}

/// The phase tag of a drag session — what a [`DragStateChange::Phase`]
/// reports and [`DragCoordinator::phase`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DragPhase {
    /// No session.
    Idle,
    /// A source was pressed and may start a drag.
    Armed,
    /// A drag is in flight.
    Dragging,
    /// Released over a target; waiting for it to claim the payload and call
    /// [`DragCoordinator::complete_drop`].
    Dropping,
    /// Transient: the session was abandoned. Only ever reported between the
    /// phase it left and `Idle`; [`DragCoordinator::phase`] never reads it.
    Cancelled,
}

/// A snapshot of an in-flight (`Dragging`/`Dropping`) session, minus the
/// payload itself.
#[derive(Clone, Debug, PartialEq)]
pub struct DragSession {
    /// Where the session came from.
    pub kind: DragKind,
    /// The source that started it; `None` for [`DragKind::ExternalFiles`].
    pub source: Option<DragSourceId>,
    /// The point the source was pressed at (for an external session, the
    /// point the OS drag first entered at).
    pub press: Point,
    /// The latest pointer position ([`DragCoordinator::update_pointer`]).
    pub pointer: Point,
    /// Offset from the pointer to the ghost's origin
    /// ([`DragCoordinator::set_ghost_offset`]); zero until a source sets it.
    pub ghost_offset: Vec2,
    /// The target currently under the pointer, if any. While `Dropping` it
    /// is the drop target.
    pub hovered: Option<DragTargetId>,
    /// The payload's type, while the payload is still held — `None` once a
    /// target has moved it out with [`DragCoordinator::take_payload`].
    pub payload_type: Option<TypeId>,
    /// Whether this session was begun by [`DragCoordinator::lift`] rather
    /// than [`DragCoordinator::begin`]/[`DragCoordinator::begin_external`] —
    /// a keyboard session, with no pointer driving it.
    /// [`DragCoordinator::update_pointer`]/[`DragCoordinator::resolve_hover`]
    /// no-op for one; hover instead advances only through
    /// [`DragCoordinator::move_to_next_target`]/
    /// [`DragCoordinator::move_to_previous_target`] (see the [module
    /// docs](self#keyboard-sessions)).
    pub keyboard: bool,
}

impl DragSession {
    /// Where the ghost's origin sits: the pointer plus the ghost offset.
    pub fn ghost_origin(&self) -> Point {
        self.pointer + self.ghost_offset
    }
}

/// A snapshot of a coordinator's state ([`DragCoordinator::state`]).
#[derive(Clone, Debug, PartialEq)]
pub enum DragState {
    /// No session.
    Idle,
    /// `source` was pressed at `press` and may start a drag.
    Armed {
        /// The pressed source.
        source: DragSourceId,
        /// Where it was pressed.
        press: Point,
        /// Whether this arm came from [`DragCoordinator::lift`] rather than
        /// [`DragCoordinator::arm`] — see [`DragSession::keyboard`].
        keyboard: bool,
    },
    /// A drag is in flight.
    Dragging(DragSession),
    /// Released over `target`, awaiting [`DragCoordinator::complete_drop`].
    Dropping {
        /// The session being dropped.
        session: DragSession,
        /// The target it was released over.
        target: DragTargetId,
    },
}

impl DragState {
    /// This state's phase tag.
    pub fn phase(&self) -> DragPhase {
        match self {
            DragState::Idle => DragPhase::Idle,
            DragState::Armed { .. } => DragPhase::Armed,
            DragState::Dragging(_) => DragPhase::Dragging,
            DragState::Dropping { .. } => DragPhase::Dropping,
        }
    }

    /// The in-flight session, while `Dragging` or `Dropping`.
    pub fn session(&self) -> Option<&DragSession> {
        match self {
            DragState::Dragging(session) | DragState::Dropping { session, .. } => Some(session),
            DragState::Idle | DragState::Armed { .. } => None,
        }
    }
}

/// One change a [`DragCoordinator::subscribe`] listener hears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragStateChange {
    /// The session moved from `previous` to `next`.
    Phase {
        /// The phase left.
        previous: DragPhase,
        /// The phase entered.
        next: DragPhase,
    },
    /// The pointer moved onto `target` while dragging.
    Enter {
        /// The newly hovered target.
        target: DragTargetId,
    },
    /// The pointer left `target`, or the session hovering it (or about to
    /// drop on it) was cancelled.
    Leave {
        /// The target no longer hovered.
        target: DragTargetId,
    },
    /// The dragged pointer moved to `pointer`.
    Move {
        /// The new pointer position.
        pointer: Point,
    },
}

/// A subscribed [`DragCoordinator::subscribe`] listener.
type Listener = Rc<dyn Fn(DragStateChange)>;

/// An in-flight session's full state, payload included.
struct Session {
    kind: DragKind,
    source: Option<DragSourceId>,
    press: Point,
    pointer: Point,
    ghost_offset: Vec2,
    hovered: Option<DragTargetId>,
    payload: Option<Box<dyn Any>>,
    /// Recorded at `begin` rather than read back off the box, where
    /// `type_id()` would resolve on `Box<dyn Any>` itself.
    payload_type: TypeId,
    /// See [`DragSession::keyboard`].
    keyboard: bool,
}

impl Session {
    fn snapshot(&self) -> DragSession {
        DragSession {
            kind: self.kind.clone(),
            source: self.source,
            press: self.press,
            pointer: self.pointer,
            ghost_offset: self.ghost_offset,
            hovered: self.hovered,
            payload_type: self.payload.as_ref().map(|_| self.payload_type),
            keyboard: self.keyboard,
        }
    }
}

/// The coordinator's internal state machine.
enum Machine {
    Idle,
    Armed {
        source: DragSourceId,
        press: Point,
        keyboard: bool,
    },
    Dragging(Session),
    Dropping {
        session: Session,
        target: DragTargetId,
    },
}

impl Machine {
    fn phase(&self) -> DragPhase {
        match self {
            Machine::Idle => DragPhase::Idle,
            Machine::Armed { .. } => DragPhase::Armed,
            Machine::Dragging(_) => DragPhase::Dragging,
            Machine::Dropping { .. } => DragPhase::Dropping,
        }
    }

    fn session(&self) -> Option<&Session> {
        match self {
            Machine::Dragging(session) | Machine::Dropping { session, .. } => Some(session),
            Machine::Idle | Machine::Armed { .. } => None,
        }
    }

    fn session_mut(&mut self) -> Option<&mut Session> {
        match self {
            Machine::Dragging(session) | Machine::Dropping { session, .. } => Some(session),
            Machine::Idle | Machine::Armed { .. } => None,
        }
    }
}

/// The state every clone of a [`DragCoordinator`] shares.
struct Shared {
    machine: Machine,
    /// The next source/target id handed out (one counter for both kinds).
    next_id: u64,
    /// Subscribed listeners in subscription order, keyed by the id their
    /// guard removes them by.
    listeners: Vec<(u64, Listener)>,
    /// The id the next [`DragCoordinator::subscribe`] hands out.
    next_listener: u64,
    /// Changes produced and not yet delivered to every subscriber.
    queue: VecDeque<DragStateChange>,
    /// Whether a [`DragCoordinator::dispatch`] loop is running further up
    /// the stack — a re-entrant mutation queues behind it instead of
    /// delivering nested.
    dispatching: bool,
    /// Registered drop targets, keyed by id. An entry exists while the
    /// target is registered.
    targets: HashMap<DragTargetId, TargetEntry>,
    /// The registration stamp the next newly registered target takes.
    next_registration: u64,
}

/// One registered drop target's resolution record.
#[derive(Clone, Copy, Debug)]
struct TargetEntry {
    /// When it registered, relative to every other target: a later stamp wins
    /// an overlap in [`DragCoordinator::target_at`].
    registration: u64,
    /// Its window-space bounds; `None` until the target first reports them.
    bounds: Option<Rect>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            machine: Machine::Idle,
            next_id: 1,
            listeners: Vec::new(),
            next_listener: 0,
            queue: VecDeque::new(),
            dispatching: false,
            targets: HashMap::new(),
            next_registration: 0,
        }
    }
}

impl Shared {
    /// Abandon whatever is in flight, landing on `Idle`: a hovered target (or
    /// an unclaimed drop's target) hears `Leave`, then the
    /// `→ Cancelled → Idle` pair. Returns the abandoned session, if one was
    /// in flight (`Dragging`/`Dropping`). A no-op from `Idle`.
    fn cancel(&mut self, changes: &mut Vec<DragStateChange>) -> Option<Session> {
        let previous = self.machine.phase();
        let (leave, session) = match std::mem::replace(&mut self.machine, Machine::Idle) {
            Machine::Idle => return None,
            Machine::Armed { .. } => (None, None),
            Machine::Dragging(session) => (session.hovered, Some(session)),
            Machine::Dropping { session, target } => (Some(target), Some(session)),
        };
        if let Some(target) = leave {
            changes.push(DragStateChange::Leave { target });
        }
        changes.push(DragStateChange::Phase {
            previous,
            next: DragPhase::Cancelled,
        });
        changes.push(DragStateChange::Phase {
            previous: DragPhase::Cancelled,
            next: DragPhase::Idle,
        });
        session
    }

    /// The registered target whose bounds contain `point` — the latest
    /// registered among overlapping ones (see the [module docs](super)).
    fn target_at(&self, point: Point) -> Option<DragTargetId> {
        self.targets
            .iter()
            .filter(|(_, entry)| entry.bounds.is_some_and(|rect| rect.contains(point)))
            .max_by_key(|(_, entry)| entry.registration)
            .map(|(&id, _)| id)
    }

    /// While `Dragging`, make `target` the hovered one: the old one (if any)
    /// hears `Leave`, then the new one (if any) `Enter`. Nothing when
    /// unchanged or not `Dragging`.
    fn set_hovered(&mut self, target: Option<DragTargetId>, changes: &mut Vec<DragStateChange>) {
        if let Machine::Dragging(session) = &mut self.machine
            && session.hovered != target
        {
            if let Some(old) = session.hovered {
                changes.push(DragStateChange::Leave { target: old });
            }
            session.hovered = target;
            if let Some(target) = target {
                changes.push(DragStateChange::Enter { target });
            }
        }
    }

    /// While `Dragging`, re-resolve the hovered target under the session's
    /// current pointer against the registry's current bounds. A no-op for a
    /// keyboard session ([`DragSession::keyboard`]) — there is no pointer to
    /// resolve against, so this never overrides what
    /// [`Shared::move_target`] last set.
    fn resolve_hover(&mut self, changes: &mut Vec<DragStateChange>) {
        let Machine::Dragging(session) = &self.machine else {
            return;
        };
        if session.keyboard {
            return;
        }
        let target = self.target_at(session.pointer);
        self.set_hovered(target, changes);
    }

    /// `Idle → Armed`. The caller has already left `Idle` reachable.
    fn arm_from_idle(
        &mut self,
        source: DragSourceId,
        press: Point,
        keyboard: bool,
        changes: &mut Vec<DragStateChange>,
    ) {
        debug_assert!(matches!(self.machine, Machine::Idle));
        self.machine = Machine::Armed {
            source,
            press,
            keyboard,
        };
        changes.push(DragStateChange::Phase {
            previous: DragPhase::Idle,
            next: DragPhase::Armed,
        });
    }

    /// Cycle the hovered target by `step` (`1`/`-1`) through the registered
    /// targets in registration ("tree") order — the
    /// [`DragCoordinator::move_to_next_target`]/
    /// [`DragCoordinator::move_to_previous_target`] drive. Starts at the
    /// first (`step > 0`) or last (`step < 0`) target when nothing is
    /// hovered yet, and wraps around at either end. Raises the same
    /// `Leave`/`Enter` pair pointer resolution does
    /// ([`Shared::set_hovered`]). A no-op outside `Dragging` or with no
    /// registered targets.
    fn move_target(&mut self, step: i64, changes: &mut Vec<DragStateChange>) {
        let Machine::Dragging(session) = &self.machine else {
            return;
        };
        let mut ordered: Vec<(u64, DragTargetId)> = self
            .targets
            .iter()
            .map(|(&id, entry)| (entry.registration, id))
            .collect();
        if ordered.is_empty() {
            return;
        }
        ordered.sort_unstable();
        let ids: Vec<DragTargetId> = ordered.into_iter().map(|(_, id)| id).collect();
        let len = ids.len() as i64;
        let next = match session
            .hovered
            .and_then(|current| ids.iter().position(|&id| id == current))
        {
            Some(index) => ids[(index as i64 + step).rem_euclid(len) as usize],
            None if step >= 0 => ids[0],
            None => ids[ids.len() - 1],
        };
        self.set_hovered(Some(next), changes);
    }
}

/// A cloneable handle onto one drag scope's session state: sources
/// [`arm`](Self::arm)/[`begin`](Self::begin) a drag and report its pointer
/// with [`update_pointer`](Self::update_pointer), which resolves the hovered
/// target against the registry ([`set_hovered`](Self::set_hovered) overrides
/// it), and it ends in [`drop`](Self::drop) +
/// [`complete_drop`](Self::complete_drop) or [`cancel`](Self::cancel). Every
/// clone shares one state. See the [module docs](super) for the transition
/// rules.
///
/// ```
/// use frust_widgets::drag::{DragCoordinator, DragPhase};
/// use kurbo::{Point, Rect};
///
/// let drag = DragCoordinator::new();
/// let source = drag.new_source_id();
/// let target = drag.new_target_id();
/// drag.register_target(target);
/// drag.set_target_bounds(target, Rect::new(50.0, 0.0, 150.0, 100.0));
///
/// drag.arm(source, Point::new(10.0, 10.0));
/// drag.begin(42_u32);
/// // Resolution: the pointer is inside the target's reported bounds.
/// drag.update_pointer(Point::new(80.0, 40.0));
/// assert_eq!(drag.state().session().and_then(|s| s.hovered), Some(target));
/// assert_eq!(drag.drop(), Some(target));
///
/// // The target claims the payload, then completes the drop.
/// assert_eq!(drag.take_payload::<u32>().as_deref(), Some(&42));
/// drag.complete_drop();
/// assert_eq!(drag.phase(), DragPhase::Idle);
/// ```
#[derive(Clone, Default)]
pub struct DragCoordinator {
    shared: Rc<RefCell<Shared>>,
}

impl fmt::Debug for DragCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared = self.shared.borrow();
        f.debug_struct("DragCoordinator")
            .field("phase", &shared.machine.phase())
            .field("listeners", &shared.listeners.len())
            .finish()
    }
}

impl DragCoordinator {
    /// A coordinator with no session in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a source id, unique within this coordinator.
    pub fn new_source_id(&self) -> DragSourceId {
        DragSourceId(self.next_id())
    }

    /// Allocate a target id, unique within this coordinator.
    pub fn new_target_id(&self) -> DragTargetId {
        DragTargetId(self.next_id())
    }

    fn next_id(&self) -> u64 {
        let mut shared = self.shared.borrow_mut();
        let id = shared.next_id;
        shared.next_id += 1;
        id
    }

    /// A snapshot of the current state (payload excluded).
    pub fn state(&self) -> DragState {
        match &self.shared.borrow().machine {
            Machine::Idle => DragState::Idle,
            Machine::Armed {
                source,
                press,
                keyboard,
            } => DragState::Armed {
                source: *source,
                press: *press,
                keyboard: *keyboard,
            },
            Machine::Dragging(session) => DragState::Dragging(session.snapshot()),
            Machine::Dropping { session, target } => DragState::Dropping {
                session: session.snapshot(),
                target: *target,
            },
        }
    }

    /// The current phase — never [`DragPhase::Cancelled`], which is transient.
    pub fn phase(&self) -> DragPhase {
        self.shared.borrow().machine.phase()
    }

    /// `source` was pressed at `press`: `Idle → Armed`.
    ///
    /// Re-arming while `Armed`, or while an unclaimed drop is still
    /// `Dropping`, cancels that first (`→ Cancelled → Idle`, the drop target
    /// hearing `Leave`), then arms. Arming while `Dragging` is ignored with a
    /// debug-build log — the session in flight owns the pointer. Returns
    /// whether the coordinator is now armed by this call.
    pub fn arm(&self, source: DragSourceId, press: Point) -> bool {
        self.mutate(|shared, changes| {
            if matches!(shared.machine, Machine::Dragging(_)) {
                log_ignored("arm", "a drag is already in flight");
                return false;
            }
            shared.cancel(changes);
            shared.arm_from_idle(source, press, false, changes);
            true
        })
    }

    /// `source` is lifted by keyboard, with no pointer: `Idle → Armed`,
    /// marked as a keyboard session so the [`begin`](Self::begin) that
    /// follows carries [`DragSession::keyboard`] into `Dragging` — see the
    /// [module docs](self#keyboard-sessions). Same re-arm/ignore rules as
    /// [`arm`](Self::arm): re-arming while `Armed` (or while an unclaimed
    /// drop is still `Dropping`) cancels that first; lifting while
    /// `Dragging` is ignored (debug-build log). Returns whether the
    /// coordinator is now armed by this call.
    pub fn lift(&self, source: DragSourceId) -> bool {
        self.mutate(|shared, changes| {
            if matches!(shared.machine, Machine::Dragging(_)) {
                log_ignored("lift", "a drag is already in flight");
                return false;
            }
            shared.cancel(changes);
            shared.arm_from_idle(source, Point::ZERO, true, changes);
            true
        })
    }

    /// Move the hovered target to the next one registered after it
    /// (registration/tree order, wrapping to the first) — the keyboard
    /// `ArrowRight`/`ArrowDown` drive for a [`lift`](Self::lift)ed session.
    /// Starts at the first registered target when nothing is hovered yet.
    /// Raises the same `Leave`/`Enter` pair pointer resolution does. A
    /// no-op outside `Dragging` or with no registered targets — see the
    /// [module docs](self#keyboard-sessions).
    pub fn move_to_next_target(&self) {
        self.mutate(|shared, changes| shared.move_target(1, changes));
    }

    /// The mirror of
    /// [`move_to_next_target`](Self::move_to_next_target) — the keyboard
    /// `ArrowLeft`/`ArrowUp` drive, wrapping to the last registered target.
    pub fn move_to_previous_target(&self) {
        self.mutate(|shared, changes| shared.move_target(-1, changes));
    }

    /// The armed source starts dragging `payload`: `Armed → Dragging`, with
    /// the pointer at the press point and nothing hovered.
    ///
    /// `begin` without an arm is an error and is ignored (debug-build log),
    /// as is `begin` while `Dropping`. A second `begin` while `Dragging`
    /// cancels the first session (a hovered target hearing `Leave`) and
    /// restarts from the same source with this payload — re-armed at the
    /// first session's press point, then dragging from its current pointer
    /// and ghost offset, so subscribers hear the full
    /// `Dragging → Cancelled → Idle → Armed → Dragging` walk. An external
    /// session has no source to restart from: it is cancelled and this
    /// `begin` ignored. Returns whether a session is now dragging this
    /// payload.
    pub fn begin<T: Any>(&self, payload: T) -> bool {
        self.mutate(|shared, changes| {
            let (source, press, pointer, ghost_offset, keyboard) = match &shared.machine {
                Machine::Idle => {
                    log_ignored("begin", "nothing is armed");
                    return false;
                }
                Machine::Dropping { .. } => {
                    log_ignored("begin", "a drop is still being completed");
                    return false;
                }
                Machine::Armed {
                    source,
                    press,
                    keyboard,
                } => (*source, *press, *press, Vec2::ZERO, *keyboard),
                Machine::Dragging(_) => {
                    let first = shared
                        .cancel(changes)
                        .expect("a Dragging machine cancels to its session");
                    let Some(source) = first.source else {
                        log_ignored("begin", "the cancelled session had no source to restart");
                        return false;
                    };
                    shared.arm_from_idle(source, first.press, first.keyboard, changes);
                    (
                        source,
                        first.press,
                        first.pointer,
                        first.ghost_offset,
                        first.keyboard,
                    )
                }
            };
            shared.machine = Machine::Dragging(Session {
                kind: DragKind::Internal,
                source: Some(source),
                press,
                pointer,
                ghost_offset,
                hovered: None,
                payload: Some(Box::new(payload)),
                payload_type: TypeId::of::<T>(),
                keyboard,
            });
            changes.push(DragStateChange::Phase {
                previous: DragPhase::Armed,
                next: DragPhase::Dragging,
            });
            true
        })
    }

    /// An OS file drag entered at `pointer`: `Idle → Dragging` with
    /// [`DragKind::ExternalFiles`], no source, and `paths` as the payload (a
    /// `Vec<PathBuf>`, so `payload_is::<Vec<PathBuf>>()` holds). Any session
    /// already in flight — armed, dragging or dropping — is cancelled first.
    pub fn begin_external(&self, paths: Vec<PathBuf>, pointer: Point) {
        self.mutate(|shared, changes| {
            shared.cancel(changes);
            shared.machine = Machine::Dragging(Session {
                kind: DragKind::ExternalFiles(paths.clone()),
                source: None,
                press: pointer,
                pointer,
                ghost_offset: Vec2::ZERO,
                hovered: None,
                payload: Some(Box::new(paths)),
                payload_type: TypeId::of::<Vec<PathBuf>>(),
                keyboard: false,
            });
            changes.push(DragStateChange::Phase {
                previous: DragPhase::Idle,
                next: DragPhase::Dragging,
            });
        });
    }

    /// The dragged pointer is at `pointer` (window space): record it, then
    /// resolve the hovered target under it — the latest-registered target
    /// whose reported bounds contain it ([`target_at`](Self::target_at)), or
    /// none. A hover change is reported as `Leave` (the old target) then
    /// `Enter` (the new one), and a changed pointer then as a
    /// [`DragStateChange::Move`], so a newly entered target's first `Move` is
    /// already inside it. Called with an unchanged pointer it still
    /// re-resolves, which is how bounds that moved under a stationary pointer
    /// are picked up (see also [`resolve_hover`](Self::resolve_hover)). A
    /// silent no-op unless `Dragging`.
    ///
    /// Resolution reads the registry only: it never hit-tests the widget tree
    /// (the ghost floats in a transparent overlay pod above it), and a hover
    /// set explicitly with [`set_hovered`](Self::set_hovered) stands only
    /// until the next call.
    ///
    /// A silent no-op for a keyboard session ([`DragSession::keyboard`]) —
    /// nothing drives a real pointer during one, and a stray call must not
    /// fight the hover [`move_to_next_target`](Self::move_to_next_target)/
    /// [`move_to_previous_target`](Self::move_to_previous_target) last set
    /// (see the [module docs](self#keyboard-sessions)).
    pub fn update_pointer(&self, pointer: Point) {
        self.mutate(|shared, changes| {
            let Machine::Dragging(session) = &mut shared.machine else {
                return;
            };
            if session.keyboard {
                return;
            }
            let moved = session.pointer != pointer;
            session.pointer = pointer;
            shared.resolve_hover(changes);
            if moved {
                changes.push(DragStateChange::Move { pointer });
            }
        });
    }

    /// Re-resolve the hovered target under the current pointer against the
    /// registry's current bounds, without moving the pointer — for a pass
    /// that moved targets under a stationary pointer (an auto-scrolled list
    /// re-reporting its rows' bounds). Raises the same `Leave`/`Enter` pair
    /// [`update_pointer`](Self::update_pointer) does, and nothing when the
    /// hover is unchanged. A silent no-op unless `Dragging`.
    pub fn resolve_hover(&self) {
        self.mutate(|shared, changes| shared.resolve_hover(changes));
    }

    /// Set where the ghost's origin sits relative to the pointer (see
    /// [`DragSession::ghost_origin`]). Notifies nothing — a source sets it
    /// right after [`begin`](Self::begin), before the ghost first paints. A
    /// silent no-op unless `Dragging`.
    pub fn set_ghost_offset(&self, offset: Vec2) {
        if let Machine::Dragging(session) = &mut self.shared.borrow_mut().machine {
            session.ghost_offset = offset;
        }
    }

    /// Override the hovered target with `target`, bypassing resolution: the
    /// old one (if any) hears `Leave`, then the new one (if any) hears
    /// `Enter`. Nothing when unchanged; a silent no-op unless `Dragging`. The
    /// override stands until the next [`update_pointer`](Self::update_pointer)
    /// or [`resolve_hover`](Self::resolve_hover) re-resolves against the
    /// registry.
    pub fn set_hovered(&self, target: Option<DragTargetId>) {
        self.mutate(|shared, changes| shared.set_hovered(target, changes));
    }

    /// Register a drop target for bounds tracking and resolution, stamped
    /// after every target already registered — a later registration wins an
    /// overlap in [`target_at`](Self::target_at). Idempotent: re-registering
    /// an already-registered target is a no-op and keeps its original stamp.
    pub fn register_target(&self, id: DragTargetId) {
        let mut shared = self.shared.borrow_mut();
        if shared.targets.contains_key(&id) {
            return;
        }
        let registration = shared.next_registration;
        shared.next_registration += 1;
        shared.targets.insert(
            id,
            TargetEntry {
                registration,
                bounds: None,
            },
        );
    }

    /// Unregister a drop target. If the target is currently hovered or is the
    /// unclaimed drop target, the hover is cleared (emitting `Leave` to
    /// subscribers). A silent no-op for an unregistered target.
    pub fn unregister_target(&self, id: DragTargetId) {
        self.mutate(|shared, changes| {
            shared.targets.remove(&id);
            // If this target was hovered, clear the hover and emit Leave.
            match &mut shared.machine {
                Machine::Dragging(session) if session.hovered == Some(id) => {
                    session.hovered = None;
                    changes.push(DragStateChange::Leave { target: id });
                }
                Machine::Dropping {
                    session,
                    target: drop_target,
                } if *drop_target == id => {
                    // The drop target itself is being unregistered: clear hover and emit Leave.
                    changes.push(DragStateChange::Leave { target: id });
                }
                _ => {}
            }
        });
    }

    /// Record the window-space bounds of a registered target. Updates persist
    /// across drag sessions. Ignored (with a debug-build log) if the target
    /// is not registered.
    pub fn set_target_bounds(&self, id: DragTargetId, bounds: Rect) {
        let mut shared = self.shared.borrow_mut();
        match shared.targets.get_mut(&id) {
            Some(entry) => {
                entry.bounds = Some(bounds);
            }
            None => {
                log_ignored("set_target_bounds", "the target is not registered");
            }
        }
    }

    /// Query the window-space bounds of a registered target, or `None` if the
    /// target is not registered or bounds have not been set.
    pub fn target_bounds(&self, id: DragTargetId) -> Option<Rect> {
        self.shared
            .borrow()
            .targets
            .get(&id)
            .and_then(|entry| entry.bounds)
    }

    /// The registered target whose reported bounds contain `point` (window
    /// space); where several overlap, the one registered **last** wins — see
    /// the [module docs](super) for why registration order stands in for
    /// paint order. Targets that have not reported bounds yet are skipped.
    /// Returns `None` if no registered target contains the point.
    pub fn target_at(&self, point: Point) -> Option<DragTargetId> {
        self.shared.borrow().target_at(point)
    }

    /// All registered drop targets, in registration order.
    pub fn registered_targets(&self) -> Vec<DragTargetId> {
        let shared = self.shared.borrow();
        let mut targets: Vec<(u64, DragTargetId)> = shared
            .targets
            .iter()
            .map(|(&id, entry)| (entry.registration, id))
            .collect();
        targets.sort_unstable();
        targets.into_iter().map(|(_, id)| id).collect()
    }

    /// Release the drag: over a hovered target, `Dragging → Dropping` (the
    /// target hears no `Leave` — it is the drop target) and the target is
    /// returned; with nothing hovered, or from `Armed`, exactly
    /// [`cancel`](Self::cancel) and `None`. A no-op (`None`) from `Idle` or
    /// an already-`Dropping` session.
    pub fn drop(&self) -> Option<DragTargetId> {
        self.mutate(|shared, changes| match &shared.machine {
            Machine::Idle | Machine::Dropping { .. } => None,
            Machine::Armed { .. } => {
                shared.cancel(changes);
                None
            }
            Machine::Dragging(session) => {
                let Some(target) = session.hovered else {
                    shared.cancel(changes);
                    return None;
                };
                let Machine::Dragging(session) =
                    std::mem::replace(&mut shared.machine, Machine::Idle)
                else {
                    unreachable!("matched Dragging above");
                };
                shared.machine = Machine::Dropping { session, target };
                changes.push(DragStateChange::Phase {
                    previous: DragPhase::Dragging,
                    next: DragPhase::Dropping,
                });
                Some(target)
            }
        })
    }

    /// The drop target finished: `Dropping → Idle`, discarding the payload
    /// if it was never taken. A silent no-op unless `Dropping`.
    pub fn complete_drop(&self) {
        self.mutate(|shared, changes| {
            if matches!(shared.machine, Machine::Dropping { .. }) {
                shared.machine = Machine::Idle;
                changes.push(DragStateChange::Phase {
                    previous: DragPhase::Dropping,
                    next: DragPhase::Idle,
                });
            }
        });
    }

    /// Abandon whatever is in flight: from any non-`Idle` phase, a hovered
    /// target (or an unclaimed drop's target) hears `Leave`, then
    /// `→ Cancelled → Idle`; the payload is discarded. A no-op from `Idle`.
    pub fn cancel(&self) {
        self.mutate(|shared, changes| {
            shared.cancel(changes);
        });
    }

    /// Whether a payload of type `T` is held (`Dragging`/`Dropping`, not yet
    /// taken).
    pub fn payload_is<T: Any>(&self) -> bool {
        self.shared
            .borrow()
            .machine
            .session()
            .is_some_and(|session| {
                session.payload.is_some() && session.payload_type == TypeId::of::<T>()
            })
    }

    /// Borrow the held payload as a `T`, or `None` (no session, payload
    /// taken, or a different type).
    ///
    /// The returned guard borrows the coordinator's shared state: calling any
    /// method that mutates the coordinator (on this or any clone) while it is
    /// alive panics on the borrow. Prefer [`with_payload`](Self::with_payload)
    /// when the read can be scoped to a closure.
    pub fn payload_ref<T: Any>(&self) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.shared.borrow(), |shared| {
            shared
                .machine
                .session()?
                .payload
                .as_ref()?
                .downcast_ref::<T>()
        })
        .ok()
    }

    /// Run `f` over the held payload as a `T`, returning its result, or
    /// `None` (no session, payload taken, or a different type). The shared
    /// state stays borrowed only for the duration of `f`, which must not
    /// mutate the coordinator.
    pub fn with_payload<T: Any, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.payload_ref::<T>().map(|payload| f(&payload))
    }

    /// Move the held payload out as a `T` — only while `Dragging` or
    /// `Dropping`, and only when it is a `T`; otherwise `None` and the
    /// payload stays put. A second take returns `None`.
    pub fn take_payload<T: Any>(&self) -> Option<Box<T>> {
        let mut shared = self.shared.borrow_mut();
        let session = shared.machine.session_mut()?;
        if session.payload_type != TypeId::of::<T>() {
            return None;
        }
        session.payload.take()?.downcast::<T>().ok()
    }

    /// Hear every [`DragStateChange`], in order, until the returned guard is
    /// dropped. Listeners run in subscription order with the coordinator's
    /// state released, so a listener may read or drive the coordinator; see
    /// the [module docs](super) for the ordering guarantee under re-entrancy.
    pub fn subscribe<F: Fn(DragStateChange) + 'static>(&self, listener: F) -> DragSubscription {
        let mut shared = self.shared.borrow_mut();
        let id = shared.next_listener;
        shared.next_listener += 1;
        shared.listeners.push((id, Rc::new(listener)));
        DragSubscription {
            shared: Rc::downgrade(&self.shared),
            id,
        }
    }

    /// Run one mutation against the shared state, then deliver the changes
    /// it produced. The borrow ends before any listener runs.
    fn mutate<R>(&self, f: impl FnOnce(&mut Shared, &mut Vec<DragStateChange>) -> R) -> R {
        let mut changes = Vec::new();
        let result = {
            let mut shared = self.shared.borrow_mut();
            let result = f(&mut shared, &mut changes);
            shared.queue.extend(changes.iter().copied());
            result
        };
        if !changes.is_empty() {
            frust_core::mark_pending_result_flush();
            self.dispatch();
        }
        result
    }

    /// Deliver every queued change to every live listener, in order. A call
    /// made while an outer dispatch is running returns at once: the outer
    /// loop picks up whatever the re-entrant mutation queued, so every
    /// listener hears every change in the order it happened.
    fn dispatch(&self) {
        {
            let mut shared = self.shared.borrow_mut();
            if shared.dispatching {
                return;
            }
            shared.dispatching = true;
        }
        let _guard = DispatchGuard(&self.shared);
        loop {
            let (change, listeners) = {
                let mut shared = self.shared.borrow_mut();
                let Some(change) = shared.queue.pop_front() else {
                    break;
                };
                (change, shared.listeners.clone())
            };
            for (id, listener) in listeners {
                // A listener unsubscribed by an earlier one hears nothing more.
                let live = self
                    .shared
                    .borrow()
                    .listeners
                    .iter()
                    .any(|(other, _)| *other == id);
                if live {
                    listener(change);
                }
            }
        }
    }
}

/// Clears the dispatching flag when a [`DragCoordinator::dispatch`] loop
/// ends — including by a listener's panic, which also drops the undelivered
/// changes rather than replaying them late on the next mutation.
struct DispatchGuard<'a>(&'a RefCell<Shared>);

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.0.try_borrow_mut() {
            shared.dispatching = false;
            if std::thread::panicking() {
                shared.queue.clear();
            }
        }
    }
}

/// The guard [`DragCoordinator::subscribe`] returns: dropping it unsubscribes
/// the listener. Holds the coordinator weakly, so it never keeps it alive.
#[must_use = "dropping a DragSubscription unsubscribes its listener at once"]
pub struct DragSubscription {
    shared: Weak<RefCell<Shared>>,
    id: u64,
}

impl fmt::Debug for DragSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DragSubscription")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for DragSubscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared
                .borrow_mut()
                .listeners
                .retain(|(id, _)| *id != self.id);
        }
    }
}

/// Report an ignored call in debug builds; a no-op in release.
fn log_ignored(_call: &str, _why: &str) {
    #[cfg(debug_assertions)]
    eprintln!("frust-widgets: DragCoordinator::{_call} ignored: {_why}");
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const PRESS: Point = Point::new(10.0, 10.0);
    const MOVED: Point = Point::new(20.0, 30.0);
    const OTHER_PRESS: Point = Point::new(5.0, 5.0);
    const FILE_ENTRY: Point = Point::new(1.0, 2.0);

    /// A coordinator with two sources, two targets and a change log.
    struct Fixture {
        drag: DragCoordinator,
        source: DragSourceId,
        other_source: DragSourceId,
        first: DragTargetId,
        second: DragTargetId,
        log: Rc<RefCell<Vec<DragStateChange>>>,
        _subscription: DragSubscription,
    }

    impl Fixture {
        fn new() -> Self {
            let drag = DragCoordinator::new();
            let log = Rc::new(RefCell::new(Vec::new()));
            let sink = Rc::clone(&log);
            let subscription = drag.subscribe(move |change| sink.borrow_mut().push(change));
            Self {
                source: drag.new_source_id(),
                other_source: drag.new_source_id(),
                first: drag.new_target_id(),
                second: drag.new_target_id(),
                drag,
                log,
                _subscription: subscription,
            }
        }

        fn take_log(&self) -> Vec<DragStateChange> {
            std::mem::take(&mut *self.log.borrow_mut())
        }
    }

    /// The phase a table row starts from.
    #[derive(Clone, Copy, Debug)]
    enum Start {
        Idle,
        Armed,
        /// Dragging an internal payload, nothing hovered.
        Dragging,
        /// Dragging an internal payload over the first target.
        Hovering,
        /// Dropped on the first target, not yet completed.
        Dropping,
        /// Dragging an OS file drop, nothing hovered.
        External,
    }

    impl Start {
        fn reach(self, f: &Fixture) {
            match self {
                Start::Idle => {}
                Start::Armed => {
                    f.drag.arm(f.source, PRESS);
                }
                Start::Dragging => {
                    f.drag.arm(f.source, PRESS);
                    f.drag.begin(7_i32);
                }
                Start::Hovering => {
                    Start::Dragging.reach(f);
                    f.drag.set_hovered(Some(f.first));
                }
                Start::Dropping => {
                    Start::Hovering.reach(f);
                    f.drag.drop();
                }
                Start::External => {
                    f.drag
                        .begin_external(vec![PathBuf::from("/tmp/a.txt")], FILE_ENTRY);
                }
            }
        }
    }

    /// The operation a table row applies.
    #[derive(Clone, Copy, Debug)]
    enum Op {
        Arm,
        Begin,
        BeginExternal,
        Move,
        HoverSecond,
        HoverNone,
        Drop,
        Complete,
        Cancel,
    }

    impl Op {
        fn apply(self, f: &Fixture) {
            match self {
                Op::Arm => {
                    f.drag.arm(f.other_source, OTHER_PRESS);
                }
                Op::Begin => {
                    f.drag.begin("second");
                }
                Op::BeginExternal => {
                    f.drag
                        .begin_external(vec![PathBuf::from("/tmp/b.txt")], FILE_ENTRY);
                }
                Op::Move => f.drag.update_pointer(MOVED),
                Op::HoverSecond => f.drag.set_hovered(Some(f.second)),
                Op::HoverNone => f.drag.set_hovered(None),
                Op::Drop => {
                    f.drag.drop();
                }
                Op::Complete => f.drag.complete_drop(),
                Op::Cancel => f.drag.cancel(),
            }
        }
    }

    /// Symbolic expected changes, resolved against a fixture's ids.
    #[derive(Clone, Copy, Debug)]
    enum Expect {
        Phase(DragPhase, DragPhase),
        EnterSecond,
        LeaveFirst,
        Moved,
    }

    impl Expect {
        fn resolve(self, f: &Fixture) -> DragStateChange {
            match self {
                Expect::Phase(previous, next) => DragStateChange::Phase { previous, next },
                Expect::EnterSecond => DragStateChange::Enter { target: f.second },
                Expect::LeaveFirst => DragStateChange::Leave { target: f.first },
                Expect::Moved => DragStateChange::Move { pointer: MOVED },
            }
        }
    }

    use DragPhase::{Armed, Cancelled, Dragging, Dropping, Idle};

    const CANCEL_FROM_ARMED: &[Expect] = &[
        Expect::Phase(Armed, Cancelled),
        Expect::Phase(Cancelled, Idle),
    ];
    const CANCEL_FROM_DRAGGING: &[Expect] = &[
        Expect::Phase(Dragging, Cancelled),
        Expect::Phase(Cancelled, Idle),
    ];

    /// Every (start phase, operation) pair: the phase it lands on and the
    /// exact change sequence subscribers hear.
    fn table() -> Vec<(Start, Op, DragPhase, Vec<Expect>)> {
        use Expect::{EnterSecond, LeaveFirst, Moved, Phase};
        let cat = |parts: &[&[Expect]]| parts.concat();
        vec![
            // From Idle: only arm and begin_external leave it.
            (Start::Idle, Op::Arm, Armed, vec![Phase(Idle, Armed)]),
            (Start::Idle, Op::Begin, Idle, vec![]),
            (
                Start::Idle,
                Op::BeginExternal,
                Dragging,
                vec![Phase(Idle, Dragging)],
            ),
            (Start::Idle, Op::Move, Idle, vec![]),
            (Start::Idle, Op::HoverSecond, Idle, vec![]),
            (Start::Idle, Op::HoverNone, Idle, vec![]),
            (Start::Idle, Op::Drop, Idle, vec![]),
            (Start::Idle, Op::Complete, Idle, vec![]),
            (Start::Idle, Op::Cancel, Idle, vec![]),
            // From Armed.
            (
                Start::Armed,
                Op::Arm,
                Armed,
                cat(&[CANCEL_FROM_ARMED, &[Phase(Idle, Armed)]]),
            ),
            (
                Start::Armed,
                Op::Begin,
                Dragging,
                vec![Phase(Armed, Dragging)],
            ),
            (
                Start::Armed,
                Op::BeginExternal,
                Dragging,
                cat(&[CANCEL_FROM_ARMED, &[Phase(Idle, Dragging)]]),
            ),
            (Start::Armed, Op::Move, Armed, vec![]),
            (Start::Armed, Op::HoverSecond, Armed, vec![]),
            (Start::Armed, Op::HoverNone, Armed, vec![]),
            (Start::Armed, Op::Drop, Idle, CANCEL_FROM_ARMED.to_vec()),
            (Start::Armed, Op::Complete, Armed, vec![]),
            (Start::Armed, Op::Cancel, Idle, CANCEL_FROM_ARMED.to_vec()),
            // From Dragging with nothing hovered.
            (Start::Dragging, Op::Arm, Dragging, vec![]),
            (
                Start::Dragging,
                Op::Begin,
                Dragging,
                cat(&[
                    CANCEL_FROM_DRAGGING,
                    &[Phase(Idle, Armed), Phase(Armed, Dragging)],
                ]),
            ),
            (
                Start::Dragging,
                Op::BeginExternal,
                Dragging,
                cat(&[CANCEL_FROM_DRAGGING, &[Phase(Idle, Dragging)]]),
            ),
            (Start::Dragging, Op::Move, Dragging, vec![Moved]),
            (
                Start::Dragging,
                Op::HoverSecond,
                Dragging,
                vec![EnterSecond],
            ),
            (Start::Dragging, Op::HoverNone, Dragging, vec![]),
            (
                Start::Dragging,
                Op::Drop,
                Idle,
                CANCEL_FROM_DRAGGING.to_vec(),
            ),
            (Start::Dragging, Op::Complete, Dragging, vec![]),
            (
                Start::Dragging,
                Op::Cancel,
                Idle,
                CANCEL_FROM_DRAGGING.to_vec(),
            ),
            // From Dragging over the first target.
            (Start::Hovering, Op::Arm, Dragging, vec![]),
            (
                Start::Hovering,
                Op::Begin,
                Dragging,
                cat(&[
                    &[LeaveFirst],
                    CANCEL_FROM_DRAGGING,
                    &[Phase(Idle, Armed), Phase(Armed, Dragging)],
                ]),
            ),
            (
                Start::Hovering,
                Op::BeginExternal,
                Dragging,
                cat(&[
                    &[LeaveFirst],
                    CANCEL_FROM_DRAGGING,
                    &[Phase(Idle, Dragging)],
                ]),
            ),
            // The first target was hovered by override and never registered,
            // so resolving under the moved pointer finds nothing.
            (Start::Hovering, Op::Move, Dragging, vec![LeaveFirst, Moved]),
            (
                Start::Hovering,
                Op::HoverSecond,
                Dragging,
                vec![LeaveFirst, EnterSecond],
            ),
            (Start::Hovering, Op::HoverNone, Dragging, vec![LeaveFirst]),
            (
                Start::Hovering,
                Op::Drop,
                Dropping,
                vec![Phase(Dragging, Dropping)],
            ),
            (Start::Hovering, Op::Complete, Dragging, vec![]),
            (
                Start::Hovering,
                Op::Cancel,
                Idle,
                cat(&[&[LeaveFirst], CANCEL_FROM_DRAGGING]),
            ),
            // From Dropping on the first target.
            (
                Start::Dropping,
                Op::Arm,
                Armed,
                vec![
                    LeaveFirst,
                    Phase(Dropping, Cancelled),
                    Phase(Cancelled, Idle),
                    Phase(Idle, Armed),
                ],
            ),
            (Start::Dropping, Op::Begin, Dropping, vec![]),
            (
                Start::Dropping,
                Op::BeginExternal,
                Dragging,
                vec![
                    LeaveFirst,
                    Phase(Dropping, Cancelled),
                    Phase(Cancelled, Idle),
                    Phase(Idle, Dragging),
                ],
            ),
            (Start::Dropping, Op::Move, Dropping, vec![]),
            (Start::Dropping, Op::HoverSecond, Dropping, vec![]),
            (Start::Dropping, Op::HoverNone, Dropping, vec![]),
            (Start::Dropping, Op::Drop, Dropping, vec![]),
            (
                Start::Dropping,
                Op::Complete,
                Idle,
                vec![Phase(Dropping, Idle)],
            ),
            (
                Start::Dropping,
                Op::Cancel,
                Idle,
                vec![
                    LeaveFirst,
                    Phase(Dropping, Cancelled),
                    Phase(Cancelled, Idle),
                ],
            ),
            // From an external (OS file) drag with nothing hovered.
            (Start::External, Op::Arm, Dragging, vec![]),
            // The first is cancelled; with no source to restart from, the
            // second begin is ignored.
            (
                Start::External,
                Op::Begin,
                Idle,
                CANCEL_FROM_DRAGGING.to_vec(),
            ),
            (
                Start::External,
                Op::BeginExternal,
                Dragging,
                cat(&[CANCEL_FROM_DRAGGING, &[Phase(Idle, Dragging)]]),
            ),
            (Start::External, Op::Move, Dragging, vec![Moved]),
            (
                Start::External,
                Op::HoverSecond,
                Dragging,
                vec![EnterSecond],
            ),
            (Start::External, Op::HoverNone, Dragging, vec![]),
            (
                Start::External,
                Op::Drop,
                Idle,
                CANCEL_FROM_DRAGGING.to_vec(),
            ),
            (Start::External, Op::Complete, Dragging, vec![]),
            (
                Start::External,
                Op::Cancel,
                Idle,
                CANCEL_FROM_DRAGGING.to_vec(),
            ),
        ]
    }

    #[test]
    fn every_transition_lands_on_the_tabled_phase_with_the_tabled_changes() {
        let rows = table();
        assert_eq!(rows.len(), 6 * 9, "every start phase x every operation");
        for (start, op, phase, expect) in rows {
            let f = Fixture::new();
            start.reach(&f);
            f.take_log();
            op.apply(&f);
            let expected: Vec<DragStateChange> = expect.iter().map(|e| e.resolve(&f)).collect();
            assert_eq!(f.take_log(), expected, "{start:?} + {op:?}: changes");
            assert_eq!(f.drag.phase(), phase, "{start:?} + {op:?}: phase");
            assert_eq!(
                f.drag.state().phase(),
                phase,
                "{start:?} + {op:?}: state snapshot agrees with phase"
            );
            assert!(
                f.drag.phase() != Cancelled,
                "{start:?} + {op:?}: Cancelled is never a resting phase"
            );
        }
    }

    #[test]
    fn setup_paths_report_their_own_changes() {
        let f = Fixture::new();
        Start::Dropping.reach(&f);
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Phase {
                    previous: Idle,
                    next: Armed
                },
                DragStateChange::Phase {
                    previous: Armed,
                    next: Dragging
                },
                DragStateChange::Enter { target: f.first },
                DragStateChange::Phase {
                    previous: Dragging,
                    next: Dropping
                },
            ]
        );
    }

    #[test]
    fn a_session_snapshot_tracks_source_pointer_hover_and_drop_target() {
        let f = Fixture::new();
        f.drag.arm(f.source, PRESS);
        assert_eq!(
            f.drag.state(),
            DragState::Armed {
                source: f.source,
                press: PRESS,
                keyboard: false,
            }
        );
        f.drag.begin(7_i32);
        f.drag.set_ghost_offset(Vec2::new(-4.0, -6.0));
        f.drag.update_pointer(MOVED);
        f.drag.set_hovered(Some(f.first));
        let DragState::Dragging(session) = f.drag.state() else {
            panic!("expected Dragging");
        };
        assert_eq!(session.kind, DragKind::Internal);
        assert_eq!(session.source, Some(f.source));
        assert_eq!(session.press, PRESS);
        assert_eq!(session.pointer, MOVED);
        assert_eq!(session.ghost_origin(), Point::new(16.0, 24.0));
        assert_eq!(session.hovered, Some(f.first));
        assert_eq!(session.payload_type, Some(TypeId::of::<i32>()));

        assert_eq!(f.drag.drop(), Some(f.first));
        let DragState::Dropping { session, target } = f.drag.state() else {
            panic!("expected Dropping");
        };
        assert_eq!(target, f.first);
        assert_eq!(session.pointer, MOVED);
    }

    #[test]
    fn a_second_begin_restarts_from_the_same_source_at_the_current_pointer() {
        let f = Fixture::new();
        f.drag.arm(f.source, PRESS);
        assert!(f.drag.begin(1_u8));
        f.drag.set_ghost_offset(Vec2::new(-2.0, -2.0));
        f.drag.update_pointer(MOVED);
        assert!(f.drag.begin("replacement"));
        let DragState::Dragging(session) = f.drag.state() else {
            panic!("expected Dragging");
        };
        assert_eq!(session.source, Some(f.source));
        assert_eq!(session.press, PRESS);
        assert_eq!(session.pointer, MOVED);
        assert_eq!(session.ghost_offset, Vec2::new(-2.0, -2.0));
        assert_eq!(session.hovered, None);
        assert!(f.drag.payload_is::<&str>());
        assert!(
            !f.drag.payload_is::<u8>(),
            "the first payload was discarded"
        );
    }

    #[test]
    fn begin_without_arm_and_arm_while_dragging_report_failure() {
        let f = Fixture::new();
        assert!(!f.drag.begin(1_u8), "begin from Idle is ignored");
        assert!(f.drag.arm(f.source, PRESS));
        assert!(f.drag.begin(1_u8));
        assert!(
            !f.drag.arm(f.other_source, OTHER_PRESS),
            "arm while dragging is ignored"
        );
        assert_eq!(
            f.drag.state().session().and_then(|s| s.source),
            Some(f.source)
        );
    }

    #[test]
    fn a_typed_payload_round_trips_and_moves_out_once() {
        #[derive(Debug, PartialEq)]
        struct Card {
            id: u32,
            title: String,
        }
        let f = Fixture::new();
        assert!(!f.drag.payload_is::<Card>(), "no payload while Idle");
        assert!(f.drag.take_payload::<Card>().is_none());
        f.drag.arm(f.source, PRESS);
        assert!(
            f.drag.payload_ref::<Card>().is_none(),
            "no payload while Armed"
        );
        f.drag.begin(Card {
            id: 3,
            title: "three".into(),
        });

        assert!(f.drag.payload_is::<Card>());
        assert!(!f.drag.payload_is::<u32>());
        assert_eq!(f.drag.payload_ref::<Card>().map(|card| card.id), Some(3));
        assert!(
            f.drag.payload_ref::<String>().is_none(),
            "wrong type reads None"
        );
        assert_eq!(
            f.drag
                .with_payload(|card: &Card| card.title.clone())
                .as_deref(),
            Some("three")
        );
        assert_eq!(f.drag.with_payload(|n: &u32| *n), None);
        assert!(
            f.drag.take_payload::<u32>().is_none(),
            "a wrong-type take leaves the payload in place"
        );
        assert!(f.drag.payload_is::<Card>());

        // Still held into Dropping, and taken there.
        f.drag.set_hovered(Some(f.first));
        f.drag.drop();
        let taken = f
            .drag
            .take_payload::<Card>()
            .expect("the drop target claims it");
        assert_eq!(
            *taken,
            Card {
                id: 3,
                title: "three".into()
            }
        );
        assert!(
            f.drag.take_payload::<Card>().is_none(),
            "a second take is None"
        );
        assert!(!f.drag.payload_is::<Card>());
        assert_eq!(
            f.drag.state().session().and_then(|s| s.payload_type),
            None,
            "the snapshot reports the payload gone"
        );
        f.drag.complete_drop();
        assert_eq!(f.drag.phase(), Idle);
    }

    #[test]
    fn a_payload_is_discarded_by_cancel_and_by_an_unclaimed_completion() {
        let f = Fixture::new();
        let probe = Rc::new(());
        f.drag.arm(f.source, PRESS);
        f.drag.begin(Rc::clone(&probe));
        assert_eq!(Rc::strong_count(&probe), 2);
        f.drag.cancel();
        assert_eq!(Rc::strong_count(&probe), 1, "cancel drops the payload");

        f.drag.arm(f.source, PRESS);
        f.drag.begin(Rc::clone(&probe));
        f.drag.set_hovered(Some(f.first));
        f.drag.drop();
        assert_eq!(Rc::strong_count(&probe), 2, "still held while Dropping");
        f.drag.complete_drop();
        assert_eq!(
            Rc::strong_count(&probe),
            1,
            "an unclaimed payload is dropped on completion"
        );
    }

    #[test]
    fn subscribers_hear_every_change_in_subscription_order() {
        let drag = DragCoordinator::new();
        let source = drag.new_source_id();
        let log = Rc::new(RefCell::new(Vec::new()));
        let subscriptions: Vec<DragSubscription> = (0..3)
            .map(|index| {
                let sink = Rc::clone(&log);
                drag.subscribe(move |change| sink.borrow_mut().push((index, change)))
            })
            .collect();
        drag.arm(source, PRESS);
        let armed = DragStateChange::Phase {
            previous: Idle,
            next: Armed,
        };
        assert_eq!(*log.borrow(), vec![(0, armed), (1, armed), (2, armed)]);

        // Dropping a guard unsubscribes that listener only.
        log.borrow_mut().clear();
        let mut subscriptions = subscriptions;
        drop(subscriptions.remove(1));
        drag.cancel();
        let to_cancelled = DragStateChange::Phase {
            previous: Armed,
            next: Cancelled,
        };
        let to_idle = DragStateChange::Phase {
            previous: Cancelled,
            next: Idle,
        };
        assert_eq!(
            *log.borrow(),
            vec![
                (0, to_cancelled),
                (2, to_cancelled),
                (0, to_idle),
                (2, to_idle),
            ]
        );
    }

    #[test]
    fn a_reentrant_listener_keeps_the_global_order_for_every_subscriber() {
        let drag = DragCoordinator::new();
        let source = drag.new_source_id();
        let target = drag.new_target_id();
        let log = Rc::new(RefCell::new(Vec::new()));

        // The first listener cancels as soon as it hears the drop, reading
        // the state while it is at it.
        let handle = drag.clone();
        let sink = Rc::clone(&log);
        let _first = drag.subscribe(move |change| {
            sink.borrow_mut().push(("first", change));
            if change
                == (DragStateChange::Phase {
                    previous: Dragging,
                    next: Dropping,
                })
            {
                assert_eq!(handle.phase(), Dropping);
                handle.cancel();
                assert_eq!(
                    handle.phase(),
                    Idle,
                    "the re-entrant cancel applied at once"
                );
            }
        });
        let sink = Rc::clone(&log);
        let _second = drag.subscribe(move |change| sink.borrow_mut().push(("second", change)));

        drag.arm(source, PRESS);
        drag.begin(());
        drag.set_hovered(Some(target));
        log.borrow_mut().clear();
        drag.drop();

        let dropping = DragStateChange::Phase {
            previous: Dragging,
            next: Dropping,
        };
        let leave = DragStateChange::Leave { target };
        let to_cancelled = DragStateChange::Phase {
            previous: Dropping,
            next: Cancelled,
        };
        let to_idle = DragStateChange::Phase {
            previous: Cancelled,
            next: Idle,
        };
        assert_eq!(
            *log.borrow(),
            vec![
                ("first", dropping),
                ("second", dropping),
                ("first", leave),
                ("second", leave),
                ("first", to_cancelled),
                ("second", to_cancelled),
                ("first", to_idle),
                ("second", to_idle),
            ],
            "the second subscriber hears the drop before the cancel it triggered"
        );
    }

    #[test]
    fn a_listener_unsubscribed_mid_dispatch_hears_nothing_more() {
        let drag = DragCoordinator::new();
        let source = drag.new_source_id();
        let heard = Rc::new(RefCell::new(0_u32));
        let victim: Rc<RefCell<Option<DragSubscription>>> = Rc::new(RefCell::new(None));

        let slot = Rc::clone(&victim);
        let _killer = drag.subscribe(move |_| {
            slot.borrow_mut().take();
        });
        let count = Rc::clone(&heard);
        *victim.borrow_mut() = Some(drag.subscribe(move |_| *count.borrow_mut() += 1));

        drag.arm(source, PRESS);
        assert_eq!(*heard.borrow(), 0);
    }

    #[test]
    fn a_subscription_outliving_its_coordinator_drops_cleanly() {
        let drag = DragCoordinator::new();
        let subscription = drag.subscribe(|_| {});
        drop(drag);
        drop(subscription);
    }

    #[test]
    fn a_panicking_listener_does_not_wedge_dispatch() {
        let drag = DragCoordinator::new();
        let source = drag.new_source_id();
        let heard = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&heard);
        let armed = Rc::new(std::cell::Cell::new(true));
        let trip = Rc::clone(&armed);
        let _panicky = drag.subscribe(move |_| {
            if trip.replace(false) {
                panic!("listener failure");
            }
        });
        let _recorder = drag.subscribe(move |change| sink.borrow_mut().push(change));

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drag.arm(source, PRESS);
        }));
        assert!(outcome.is_err());
        assert_eq!(drag.phase(), Armed, "the transition itself applied");
        heard.borrow_mut().clear();
        drag.cancel();
        assert_eq!(
            *heard.borrow(),
            vec![
                DragStateChange::Phase {
                    previous: Armed,
                    next: Cancelled
                },
                DragStateChange::Phase {
                    previous: Cancelled,
                    next: Idle
                },
            ],
            "later changes are delivered, the undelivered one is not replayed"
        );
    }

    #[test]
    fn an_external_file_session_runs_its_whole_lifecycle() {
        let f = Fixture::new();
        let paths = vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.png")];
        f.drag.begin_external(paths.clone(), FILE_ENTRY);
        let DragState::Dragging(session) = f.drag.state() else {
            panic!("expected Dragging");
        };
        assert_eq!(session.kind, DragKind::ExternalFiles(paths.clone()));
        assert_eq!(session.source, None, "an OS drag has no source");
        assert_eq!(session.press, FILE_ENTRY);
        assert_eq!(session.pointer, FILE_ENTRY);
        assert!(f.drag.payload_is::<Vec<PathBuf>>());

        f.drag.update_pointer(MOVED);
        f.drag.set_hovered(Some(f.first));
        f.drag.set_hovered(Some(f.second));
        assert_eq!(f.drag.drop(), Some(f.second));
        assert_eq!(
            f.drag.take_payload::<Vec<PathBuf>>().map(|paths| *paths),
            Some(paths)
        );
        f.drag.complete_drop();
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Phase {
                    previous: Idle,
                    next: Dragging
                },
                DragStateChange::Move { pointer: MOVED },
                DragStateChange::Enter { target: f.first },
                DragStateChange::Leave { target: f.first },
                DragStateChange::Enter { target: f.second },
                DragStateChange::Phase {
                    previous: Dragging,
                    next: Dropping
                },
                DragStateChange::Phase {
                    previous: Dropping,
                    next: Idle
                },
            ]
        );
        assert_eq!(f.drag.state(), DragState::Idle);
    }

    #[test]
    fn an_external_session_cancelled_by_the_os_leaves_its_hovered_target() {
        let f = Fixture::new();
        f.drag
            .begin_external(vec![PathBuf::from("/tmp/a.txt")], FILE_ENTRY);
        f.drag.set_hovered(Some(f.first));
        f.take_log();
        f.drag.cancel();
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Leave { target: f.first },
                DragStateChange::Phase {
                    previous: Dragging,
                    next: Cancelled
                },
                DragStateChange::Phase {
                    previous: Cancelled,
                    next: Idle
                },
            ]
        );
        assert!(!f.drag.payload_is::<Vec<PathBuf>>());
    }

    #[test]
    fn unchanged_pointer_and_hover_notify_nothing() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag
            .set_target_bounds(f.first, Rect::from_center_size(PRESS, (20.0, 20.0)));
        Start::Hovering.reach(&f);
        f.take_log();
        f.drag.update_pointer(PRESS);
        f.drag.set_hovered(Some(f.first));
        assert!(f.take_log().is_empty());
    }

    #[test]
    fn ids_are_unique_and_every_change_raises_a_flush() {
        let f = Fixture::new();
        let mut ids = vec![
            f.source.get(),
            f.other_source.get(),
            f.first.get(),
            f.second.get(),
        ];
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);

        frust_core::take_pending_result_flush();
        f.drag.update_pointer(MOVED); // Idle: no change, no flush
        assert!(!frust_core::take_pending_result_flush());
        f.drag.arm(f.source, PRESS);
        assert!(frust_core::take_pending_result_flush());
    }

    #[test]
    fn clones_share_one_session() {
        let f = Fixture::new();
        let other = f.drag.clone();
        other.arm(f.source, PRESS);
        assert_eq!(f.drag.phase(), Armed);
        assert_eq!(
            f.take_log().len(),
            1,
            "a clone's change reaches the original's subscriber"
        );
    }

    #[test]
    fn register_and_unregister_targets() {
        let f = Fixture::new();
        assert_eq!(f.drag.registered_targets().len(), 0);
        f.drag.register_target(f.first);
        assert_eq!(f.drag.registered_targets().len(), 1);
        assert!(f.drag.registered_targets().contains(&f.first));
        // Re-registering is idempotent.
        f.drag.register_target(f.first);
        assert_eq!(f.drag.registered_targets().len(), 1);
        f.drag.register_target(f.second);
        assert_eq!(f.drag.registered_targets().len(), 2);
        f.drag.unregister_target(f.first);
        assert_eq!(f.drag.registered_targets().len(), 1);
        assert!(!f.drag.registered_targets().contains(&f.first));
        assert!(f.drag.registered_targets().contains(&f.second));
    }

    #[test]
    fn unregister_while_hovered_emits_leave_and_clears_hover() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        f.drag.arm(f.source, PRESS);
        f.drag.begin(7_i32);
        f.drag.set_hovered(Some(f.first));
        f.take_log();
        // Unregistering the hovered target emits Leave.
        f.drag.unregister_target(f.first);
        let changes = f.take_log();
        assert_eq!(
            changes,
            vec![DragStateChange::Leave { target: f.first }],
            "unregister while hovered emits Leave"
        );
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            None,
            "hover is cleared after unregister"
        );
    }

    #[test]
    fn unregister_while_dropping_on_target_emits_leave() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.arm(f.source, PRESS);
        f.drag.begin(7_i32);
        f.drag.set_hovered(Some(f.first));
        f.drag.drop();
        f.take_log();
        // Unregistering the drop target emits Leave.
        f.drag.unregister_target(f.first);
        let changes = f.take_log();
        assert_eq!(
            changes,
            vec![DragStateChange::Leave { target: f.first }],
            "unregister while dropping emits Leave"
        );
    }

    #[test]
    fn set_and_query_target_bounds() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        assert_eq!(f.drag.target_bounds(f.first), None, "unset bounds are None");
        let rect = Rect::from_origin_size(Point::new(10.0, 20.0), (100.0, 50.0));
        f.drag.set_target_bounds(f.first, rect);
        assert_eq!(f.drag.target_bounds(f.first), Some(rect));
        let new_rect = Rect::from_origin_size(Point::new(5.0, 10.0), (200.0, 100.0));
        f.drag.set_target_bounds(f.first, new_rect);
        assert_eq!(f.drag.target_bounds(f.first), Some(new_rect));
    }

    #[test]
    fn set_target_bounds_on_unknown_id_is_ignored() {
        let f = Fixture::new();
        let unregistered = f.drag.new_target_id();
        let rect = Rect::from_origin_size(Point::new(10.0, 20.0), (100.0, 50.0));
        // Should not panic, and should be logged in debug builds.
        f.drag.set_target_bounds(unregistered, rect);
        assert_eq!(f.drag.target_bounds(unregistered), None);
    }

    #[test]
    fn target_at_finds_single_containing_target() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        let rect = Rect::from_origin_size(Point::new(10.0, 10.0), (100.0, 100.0));
        f.drag.set_target_bounds(f.first, rect);
        let point = Point::new(50.0, 50.0);
        assert_eq!(
            f.drag.target_at(point),
            Some(f.first),
            "point inside bounds returns the target"
        );
        let outside = Point::new(200.0, 200.0);
        assert_eq!(
            f.drag.target_at(outside),
            None,
            "point outside all bounds returns None"
        );
    }

    #[test]
    fn a_nested_target_registered_after_its_container_wins() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        // The container, registered first.
        let large = Rect::from_origin_size(Point::new(0.0, 0.0), (200.0, 200.0));
        f.drag.set_target_bounds(f.first, large);
        // A target nested inside it, registered after it.
        let small = Rect::from_origin_size(Point::new(50.0, 50.0), (100.0, 100.0));
        f.drag.set_target_bounds(f.second, small);
        assert_eq!(f.drag.target_at(Point::new(100.0, 100.0)), Some(f.second));
        assert_eq!(
            f.drag.target_at(Point::new(10.0, 10.0)),
            Some(f.first),
            "outside the nested target the container still resolves"
        );
    }

    #[test]
    fn two_overlapping_targets_resolve_to_the_later_registration() {
        let f = Fixture::new();
        // Registered in the opposite order to their ids, and the later one is
        // the larger: registration order alone decides, not id or area.
        f.drag.register_target(f.second);
        f.drag.register_target(f.first);
        f.drag
            .set_target_bounds(f.second, Rect::new(0.0, 0.0, 100.0, 100.0));
        f.drag
            .set_target_bounds(f.first, Rect::new(50.0, 50.0, 300.0, 300.0));
        let overlap = Point::new(75.0, 75.0);
        assert_eq!(f.drag.target_at(overlap), Some(f.first));
        assert_eq!(f.drag.registered_targets(), vec![f.second, f.first]);

        // A drag resolves the same way.
        f.drag.arm(f.source, PRESS);
        f.drag.begin(1_u8);
        f.take_log();
        f.drag.update_pointer(overlap);
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Enter { target: f.first },
                DragStateChange::Move { pointer: overlap },
            ]
        );

        // Re-registering keeps the original stamp; registering anew after an
        // unregister moves the target to the end.
        f.drag.register_target(f.second);
        assert_eq!(f.drag.target_at(overlap), Some(f.first));
        f.drag.unregister_target(f.second);
        f.drag.register_target(f.second);
        f.drag
            .set_target_bounds(f.second, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(f.drag.target_at(overlap), Some(f.second));
    }

    #[test]
    fn moving_across_three_targets_enters_and_leaves_each_in_turn() {
        let f = Fixture::new();
        let third = f.drag.new_target_id();
        // Three side-by-side columns with a gap between the second and third.
        let columns = [
            (f.first, Rect::new(0.0, 0.0, 100.0, 300.0)),
            (f.second, Rect::new(100.0, 0.0, 200.0, 300.0)),
            (third, Rect::new(220.0, 0.0, 320.0, 300.0)),
        ];
        for (id, rect) in columns {
            f.drag.register_target(id);
            f.drag.set_target_bounds(id, rect);
        }
        f.drag.arm(f.source, Point::new(50.0, 10.0));
        f.drag.begin(1_u8);
        f.take_log();

        let path = [
            Point::new(50.0, 50.0),   // first
            Point::new(60.0, 60.0),   // still first
            Point::new(150.0, 60.0),  // second
            Point::new(210.0, 60.0),  // the gap
            Point::new(250.0, 60.0),  // third
            Point::new(250.0, 400.0), // below everything
        ];
        for point in path {
            f.drag.update_pointer(point);
        }
        let mv = |pointer: Point| DragStateChange::Move { pointer };
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Enter { target: f.first },
                mv(path[0]),
                mv(path[1]),
                DragStateChange::Leave { target: f.first },
                DragStateChange::Enter { target: f.second },
                mv(path[2]),
                DragStateChange::Leave { target: f.second },
                mv(path[3]),
                DragStateChange::Enter { target: third },
                mv(path[4]),
                DragStateChange::Leave { target: third },
                mv(path[5]),
            ]
        );
        assert_eq!(
            f.drag.drop(),
            None,
            "released outside every target: a cancel"
        );
        assert_eq!(f.drag.phase(), Idle);
    }

    #[test]
    fn an_explicit_hover_stands_until_the_next_resolution() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag
            .set_target_bounds(f.first, Rect::new(0.0, 0.0, 100.0, 100.0));
        Start::Dragging.reach(&f);
        f.drag.set_hovered(Some(f.second));
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(f.second),
            "the override applies at once"
        );
        f.take_log();
        f.drag.update_pointer(Point::new(40.0, 40.0));
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Leave { target: f.second },
                DragStateChange::Enter { target: f.first },
                DragStateChange::Move {
                    pointer: Point::new(40.0, 40.0)
                },
            ]
        );
    }

    #[test]
    fn resolve_hover_follows_bounds_that_move_under_a_still_pointer() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        f.drag
            .set_target_bounds(f.first, Rect::new(0.0, 0.0, 100.0, 50.0));
        f.drag
            .set_target_bounds(f.second, Rect::new(0.0, 50.0, 100.0, 100.0));
        f.drag.arm(f.source, PRESS);
        f.drag.begin(1_u8);
        f.drag.update_pointer(Point::new(10.0, 40.0));
        f.take_log();

        // The list scrolls up by 20 px: the second row now sits under the
        // pointer, which has not moved.
        f.drag
            .set_target_bounds(f.first, Rect::new(0.0, -20.0, 100.0, 30.0));
        f.drag
            .set_target_bounds(f.second, Rect::new(0.0, 30.0, 100.0, 80.0));
        assert!(f.take_log().is_empty(), "reporting bounds notifies nothing");
        f.drag.resolve_hover();
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Leave { target: f.first },
                DragStateChange::Enter { target: f.second },
            ]
        );
        f.drag.resolve_hover();
        assert!(f.take_log().is_empty(), "an unchanged hover is silent");

        // Outside `Dragging` it is a no-op.
        f.drag.drop();
        f.take_log();
        f.drag.resolve_hover();
        assert!(f.take_log().is_empty());
    }

    #[test]
    fn target_at_ignores_targets_without_bounds() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        // Only set bounds for the second target.
        let rect = Rect::from_origin_size(Point::new(0.0, 0.0), (100.0, 100.0));
        f.drag.set_target_bounds(f.second, rect);
        let point = Point::new(50.0, 50.0);
        assert_eq!(
            f.drag.target_at(point),
            Some(f.second),
            "ignores registered targets without bounds set"
        );
    }

    #[test]
    fn target_bounds_persist_across_drag_sessions() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        let rect = Rect::from_origin_size(Point::new(10.0, 10.0), (100.0, 100.0));
        f.drag.set_target_bounds(f.first, rect);
        // Start and complete a drag session.
        f.drag.arm(f.source, PRESS);
        f.drag.begin(7_i32);
        f.drag.update_pointer(MOVED);
        f.drag.drop();
        f.drag.complete_drop();
        // Bounds should still be there after the session.
        assert_eq!(f.drag.target_bounds(f.first), Some(rect));
    }

    #[test]
    fn lift_begins_a_keyboard_session_with_no_pointer_resolution() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag
            .set_target_bounds(f.first, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert!(f.drag.lift(f.source));
        assert_eq!(
            f.drag.state(),
            DragState::Armed {
                source: f.source,
                press: Point::ZERO,
                keyboard: true,
            }
        );
        assert!(f.drag.begin(9_u8));
        let DragState::Dragging(session) = f.drag.state() else {
            panic!("expected Dragging");
        };
        assert!(session.keyboard, "begin carries the keyboard flag over");
        assert_eq!(session.hovered, None);

        // A stray pointer report never fights the keyboard session: it
        // resolves nothing even though the pointer lands inside a
        // registered target's bounds.
        f.take_log();
        f.drag.update_pointer(Point::new(50.0, 50.0));
        assert!(f.take_log().is_empty());
        assert_eq!(f.drag.state().session().and_then(|s| s.hovered), None);
        f.drag.resolve_hover();
        assert!(f.take_log().is_empty());
    }

    #[test]
    fn move_to_next_and_previous_target_cycle_in_registration_order_and_wrap() {
        let f = Fixture::new();
        let third = f.drag.new_target_id();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        f.drag.register_target(third);
        f.drag.lift(f.source);
        f.drag.begin(1_u8);
        f.take_log();

        f.drag.move_to_next_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(f.first),
            "starts at the first registered target"
        );
        f.drag.move_to_next_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(f.second)
        );
        f.drag.move_to_next_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(third)
        );
        f.drag.move_to_next_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(f.first),
            "wraps back to the first"
        );
        assert_eq!(
            f.take_log(),
            vec![
                DragStateChange::Enter { target: f.first },
                DragStateChange::Leave { target: f.first },
                DragStateChange::Enter { target: f.second },
                DragStateChange::Leave { target: f.second },
                DragStateChange::Enter { target: third },
                DragStateChange::Leave { target: third },
                DragStateChange::Enter { target: f.first },
            ]
        );

        f.drag.move_to_previous_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(third),
            "wraps back to the last going the other way"
        );
    }

    #[test]
    fn move_to_previous_target_starts_at_the_last_when_nothing_is_hovered() {
        let f = Fixture::new();
        f.drag.register_target(f.first);
        f.drag.register_target(f.second);
        f.drag.lift(f.source);
        f.drag.begin(1_u8);
        f.drag.move_to_previous_target();
        assert_eq!(
            f.drag.state().session().and_then(|s| s.hovered),
            Some(f.second)
        );
    }

    #[test]
    fn move_target_is_a_no_op_outside_dragging_or_with_no_targets() {
        let f = Fixture::new();
        f.drag.move_to_next_target();
        assert_eq!(f.drag.phase(), DragPhase::Idle);
        f.drag.lift(f.source);
        f.take_log();
        f.drag.move_to_next_target();
        assert!(
            f.take_log().is_empty(),
            "Armed is not Dragging yet — still a no-op"
        );
        f.drag.begin(1_u8);
        f.take_log();
        f.drag.move_to_next_target();
        assert!(f.take_log().is_empty(), "no registered targets to cycle");
    }

    #[test]
    fn lift_while_dragging_is_ignored() {
        let f = Fixture::new();
        f.drag.arm(f.source, PRESS);
        f.drag.begin(1_u8);
        assert!(!f.drag.lift(f.other_source));
        assert_eq!(
            f.drag.state().session().and_then(|s| s.source),
            Some(f.source)
        );
    }
}
