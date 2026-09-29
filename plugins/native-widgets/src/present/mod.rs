//! Native presentations: platform-owned modal UI (an alert today) requested
//! imperatively from Rust and resolved to exactly one terminal outcome.
//!
//! # Why not a platform-view slot
//!
//! A control is a `platform_view` slot the differ owns: it disposes a slot
//! that goes missing from the ingest stream and knows nothing about modal
//! stacking. A modal belongs to the host instead — the resumed Android
//! `Activity`, the topmost iOS view controller, the macOS key window — so a
//! presentation is a **request** ([`show_alert`]) answered by a
//! [`Presentation`] future, never a node in the tree.
//!
//! # The contract
//!
//! - **One in flight, process-wide.** A second [`show_alert`] while one is
//!   live resolves [`PresentError::Busy`] immediately — no queueing, no
//!   replacing.
//! - **Exactly one terminal outcome.** Each accepted request is stamped with
//!   a fresh, never-zero **generation**, and resolves through a one-shot
//!   channel whose sending half is consumed by its first use: an action, a
//!   cancellation, a programmatic [`dismiss`], or the host going away
//!   ([`AlertOutcome::HostLost`]). A platform callback arriving for any other
//!   generation finds no live entry and is dropped.
//! - **The slot frees itself.** It is released when the outcome is sent
//!   (before the caller is woken, so the caller's continuation may present
//!   again), when an arm drops its sender without sending (resolving
//!   [`PresentError::Platform`]), or when the caller drops the
//!   [`Presentation`]. Dropping the future frees only this module's
//!   bookkeeping: the platform UI stays up until the user answers it, and
//!   that answer is discarded.
//! - **Awaitable anywhere.** [`Presentation`] is a plain [`Future`] with no
//!   executor dependency: resolution is driven by the platform's main thread,
//!   so it can be polled from `frust::spawn_local` or any other executor.
//!   Nothing ever blocks waiting for a modal result.
//!
//! # Platform arms
//!
//! `AlertHost` is the seam, selected by `cfg`: `apple_host` (iOS + macOS —
//! host discovery, the main-queue hop and the live-presentation guard),
//! `android_host` (the `dev.frust.nativewidgets.FrustNativePresenter` Kotlin
//! object that tracks the resumed `Activity`, its one `nativeOnOutcome`
//! callback and the live-presentation guard) and `unsupported` (every other
//! target: [`PresentError::Unsupported`]). Until a platform's alert arm is
//! built on its host module, that host still discovers the presenting host
//! — [`PresentError::NoHost`] when there is none — and answers
//! [`PresentError::Unsupported`] otherwise.

mod oneshot;

#[cfg(target_os = "android")]
mod android_host;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple_host;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod unsupported;

#[cfg(target_os = "android")]
use android_host::Host as PlatformHost;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use apple_host::Host as PlatformHost;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
use unsupported::Host as PlatformHost;

pub(crate) use oneshot::Sender;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll};

/// The most actions one alert takes — the common ceiling of the three
/// platforms (Android's `AlertDialog` has exactly three button slots).
pub const MAX_ALERT_ACTIONS: usize = 3;

/// What an alert asks: a title, a message, up to [`MAX_ALERT_ACTIONS`]
/// actions, and how it is presented.
///
/// Validated when submitted ([`show_alert`]), before any platform API is
/// touched: at most [`MAX_ALERT_ACTIONS`] actions, every action id non-empty
/// and unique (the id is what [`AlertOutcome::Action`] reports back), at
/// most one [`ActionRole::Cancel`] action (UIKit raises on a second one), and
/// an [`AnchorRect`] — when given — finite with a non-negative size. An
/// [`AlertStyle::ActionSheet`] on iPad additionally requires `anchor`; that
/// rule is the iOS arm's, checked when it presents (only it can tell an iPad
/// from an iPhone).
#[derive(Clone, Debug, PartialEq)]
pub struct AlertSpec {
    /// The alert's title.
    pub title: String,
    /// The alert's body text.
    pub message: String,
    /// The buttons, in presentation order.
    pub actions: Vec<AlertAction>,
    /// Whether the user may dismiss the alert without choosing an action
    /// (back key / outside tap where the platform has one) — answered as
    /// [`AlertOutcome::Cancelled`].
    pub cancelable: bool,
    /// Centered alert or action sheet.
    pub style: AlertStyle,
    /// Where an action sheet points from, in logical points of the presenting
    /// window's coordinate space (origin top-left).
    pub anchor: Option<AnchorRect>,
}

impl AlertSpec {
    /// A cancelable [`AlertStyle::Alert`] with no actions and no anchor —
    /// add buttons with [`Self::with_action`].
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            actions: Vec::new(),
            cancelable: true,
            style: AlertStyle::Alert,
            anchor: None,
        }
    }

    /// Append one action.
    #[must_use]
    pub fn with_action(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        role: ActionRole,
    ) -> Self {
        self.actions.push(AlertAction {
            id: id.into(),
            label: label.into(),
            role,
        });
        self
    }

    /// The submit-time validation described on [`AlertSpec`].
    ///
    /// # Errors
    /// [`PresentError::InvalidSpec`] naming the first rule broken.
    pub fn validate(&self) -> Result<(), PresentError> {
        if self.actions.len() > MAX_ALERT_ACTIONS {
            return Err(PresentError::InvalidSpec(format!(
                "an alert takes at most {MAX_ALERT_ACTIONS} actions, got {}",
                self.actions.len()
            )));
        }
        for (index, action) in self.actions.iter().enumerate() {
            if action.id.is_empty() {
                return Err(PresentError::InvalidSpec(format!(
                    "action {index} has an empty id"
                )));
            }
            if self.actions[..index].iter().any(|a| a.id == action.id) {
                return Err(PresentError::InvalidSpec(format!(
                    "duplicate action id {:?}",
                    action.id
                )));
            }
        }
        let cancels = self
            .actions
            .iter()
            .filter(|a| a.role == ActionRole::Cancel)
            .count();
        if cancels > 1 {
            return Err(PresentError::InvalidSpec(format!(
                "at most one action may take the Cancel role, got {cancels}"
            )));
        }
        if let Some(anchor) = &self.anchor
            && !anchor.is_valid()
        {
            return Err(PresentError::InvalidSpec(
                "the anchor rect must be finite with a non-negative size".to_string(),
            ));
        }
        Ok(())
    }
}

/// One alert button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertAction {
    /// Reported back verbatim as [`AlertOutcome::Action`] when chosen.
    pub id: String,
    /// The button's visible label.
    pub label: String,
    /// How the platform styles and places it.
    pub role: ActionRole,
}

/// An action's semantic role, mapped onto each platform's own button styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ActionRole {
    /// An ordinary action.
    #[default]
    Default,
    /// The action that backs out; at most one per alert.
    Cancel,
    /// An action that destroys data (red where the platform has a style).
    Destructive,
}

/// How an alert is presented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlertStyle {
    /// A centered modal alert.
    #[default]
    Alert,
    /// A sheet of actions rising from the bottom (iPhone), pointing at
    /// [`AlertSpec::anchor`] (iPad); platforms without the idiom present it
    /// as an ordinary alert.
    ActionSheet,
}

/// A rectangle in logical points of the presenting window's coordinate
/// space, origin top-left — where an action sheet's popover points from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnchorRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width, non-negative.
    pub width: f64,
    /// Height, non-negative.
    pub height: f64,
}

impl AnchorRect {
    /// Every coordinate finite and the size non-negative.
    fn is_valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

/// How an alert ended — exactly one per accepted [`show_alert`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AlertOutcome {
    /// The user chose the action with this [`AlertAction::id`].
    Action(String),
    /// The user dismissed a cancelable alert without choosing an action.
    Cancelled,
    /// [`dismiss`] took the alert down.
    Dismissed,
    /// The presenting host (Activity, scene, window) went away before the
    /// alert was answered.
    HostLost,
}

/// Why a presentation could not be shown or did not complete.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PresentError {
    /// Another presentation is live — one at a time, process-wide.
    #[error("native presentation: another presentation is live")]
    Busy,
    /// No host to present over: no resumed Android `Activity`, no iOS
    /// window scene with a root view controller, no macOS key/main window.
    #[error("native presentation: no host to present over")]
    NoHost,
    /// This platform has no native presentation of this kind.
    #[error("native presentation: unsupported on this platform")]
    Unsupported,
    /// The request broke a submit-time rule (see [`AlertSpec`]).
    #[error("native presentation: invalid spec: {0}")]
    InvalidSpec(String),
    /// A platform failure not covered above (a JNI/Objective-C error, an arm
    /// that ended without an outcome).
    #[error("native presentation: platform error: {0}")]
    Platform(String),
}

/// Names one accepted presentation, for [`dismiss`]. Stale once that
/// presentation has resolved: dismissing it then does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresentationHandle {
    generation: u64,
}

/// The future a presentation request answers with — see the module doc's
/// *The contract*.
///
/// Resolves once; every further poll answers [`Poll::Pending`] rather than
/// panicking (a completion can be driven from a platform thread the polling
/// executor knows nothing about, and a spurious re-poll must not take a
/// process down). Dropping it releases the process-wide slot.
#[must_use = "a presentation's outcome is only observed by polling it; dropping it abandons the outcome"]
pub struct Presentation<T> {
    state: PresentationState<T>,
    handle: Option<PresentationHandle>,
}

/// Either an already-decided result (validation failure, `Busy`, a host's
/// synchronous refusal) or the live channel an arm resolves later — one
/// concrete type so both paths are the same future.
enum PresentationState<T> {
    Ready(Option<Result<T, PresentError>>),
    Pending(oneshot::Receiver<T>),
}

impl<T> Presentation<T> {
    fn ready(result: Result<T, PresentError>) -> Self {
        Self {
            state: PresentationState::Ready(Some(result)),
            handle: None,
        }
    }

    fn pending(receiver: oneshot::Receiver<T>, generation: u64) -> Self {
        Self {
            state: PresentationState::Pending(receiver),
            handle: Some(PresentationHandle { generation }),
        }
    }

    /// The handle [`dismiss`] takes — `None` when the request was refused
    /// before anything was presented (invalid spec, `Busy`, a synchronous
    /// host refusal).
    pub fn handle(&self) -> Option<PresentationHandle> {
        self.handle
    }
}

impl<T> fmt::Debug for Presentation<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.state {
            PresentationState::Ready(Some(_)) => "ready",
            PresentationState::Ready(None) => "taken",
            PresentationState::Pending(_) => "pending",
        };
        f.debug_struct("Presentation")
            .field("state", &state)
            .field("handle", &self.handle)
            .finish()
    }
}

// `Presentation` never pin-projects: the ready value is moved out by
// `Option::take` and the receiver is itself `Unpin` (an `Arc`).
impl<T> Unpin for Presentation<T> {}

impl<T> Future for Presentation<T> {
    type Output = Result<T, PresentError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().state {
            PresentationState::Ready(value) => match value.take() {
                Some(value) => Poll::Ready(value),
                None => Poll::Pending,
            },
            // A receiver whose value was already taken registers the waker
            // and answers `Pending`, exactly like one still waiting.
            PresentationState::Pending(receiver) => Pin::new(receiver).poll(cx),
        }
    }
}

/// The platform seam an alert arm implements (selected by `cfg`, see the
/// module doc's *Platform arms*).
pub(crate) trait AlertHost {
    /// Present `spec` for presentation `generation`, resolving `tx` exactly
    /// once — later, from the platform's own callback — or return an error
    /// synchronously without having sent anything. Never blocks for the
    /// user's answer. `spec` has already passed [`AlertSpec::validate`].
    fn show_alert(
        spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Result<(), PresentError>;

    /// Take presentation `generation` down programmatically, resolving it
    /// [`AlertOutcome::Dismissed`]. Must ignore a generation it is not
    /// presenting: the live check [`dismiss`] makes first can race a
    /// platform callback resolving that very presentation.
    fn dismiss(generation: u64);
}

/// The live presentation's generation, if any — the process-wide Busy slot.
static ACTIVE: Mutex<Option<u64>> = Mutex::new(None);

/// The counter behind [`next_generation`]. Starts at `1` so a generation is
/// never `0` — a zero-initialized `jlong`/`u64` arriving from a host that
/// lost track of its own presentation must not look like a valid one.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A fresh, process-wide generation. Never `0`, including across the
/// (theoretical) wrap of [`NEXT_GENERATION`].
fn next_generation() -> u64 {
    loop {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        if generation != 0 {
            return generation;
        }
    }
}

/// Lock [`ACTIVE`], recovering from poisoning instead of panicking — a panic
/// on either side (a polling caller, a platform callback) must not turn the
/// other side's next call into a panic too. The guarded data is a plain
/// `Option<u64>`, coherent after any panic.
fn lock_active() -> MutexGuard<'static, Option<u64>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Release the Busy slot if it still holds `generation` — the hook both
/// channel halves call (`oneshot`'s module doc). A no-op when a newer
/// presentation holds the slot or none does.
pub(crate) fn release_if_live(generation: u64) {
    let mut active = lock_active();
    if *active == Some(generation) {
        *active = None;
    }
}

/// Claim the Busy slot for a fresh generation, then hand the sending half to
/// `start`. The lock is released before `start` runs: a host that resolves
/// synchronously releases the slot through the sender, which must not
/// deadlock on a lock this function still holds.
fn submit<T>(start: impl FnOnce(Sender<T>, u64) -> Result<(), PresentError>) -> Presentation<T> {
    let generation = {
        let mut active = lock_active();
        if active.is_some() {
            return Presentation::ready(Err(PresentError::Busy));
        }
        let generation = next_generation();
        *active = Some(generation);
        generation
    };

    let (tx, rx) = oneshot::channel(generation);
    match start(tx, generation) {
        Ok(()) => Presentation::pending(rx, generation),
        Err(err) => {
            // The host refused before presenting anything. Dropping `rx`
            // releases the slot (a no-op if the host's dropped sender
            // already did); the refusal itself is the answer.
            drop(rx);
            Presentation::ready(Err(err))
        }
    }
}

fn show_alert_on<H: AlertHost>(spec: AlertSpec) -> Presentation<AlertOutcome> {
    if let Err(err) = spec.validate() {
        return Presentation::ready(Err(err));
    }
    submit(|tx, generation| H::show_alert(spec, tx, generation))
}

fn dismiss_on<H: AlertHost>(handle: &PresentationHandle) {
    // Checked, then released, before calling into the host: the host may
    // resolve synchronously, which re-enters `release_if_live`.
    let live = *lock_active() == Some(handle.generation);
    if live {
        H::dismiss(handle.generation);
    }
}

/// Present a native alert — see the module doc's *The contract*.
///
/// The returned [`Presentation`] resolves [`AlertOutcome`] once the user (or
/// [`dismiss`], or the host going away) ends the alert, or
/// [`PresentError`]: `InvalidSpec` / `Busy` / a synchronous host refusal
/// (`NoHost`, `Unsupported`, `Platform`) on its first poll, or a later
/// `NoHost`/`Platform` from an arm that discovered it only on the platform's
/// main thread.
pub fn show_alert(spec: AlertSpec) -> Presentation<AlertOutcome> {
    show_alert_on::<PlatformHost>(spec)
}

/// Take the presentation `handle` names down; it resolves
/// [`AlertOutcome::Dismissed`] exactly once. A stale handle — its
/// presentation already resolved, or superseded — is ignored.
pub fn dismiss(handle: &PresentationHandle) {
    dismiss_on::<PlatformHost>(handle);
}

/// The integer outcome codes the Android presenter reports through its one
/// `nativeOnOutcome(generation, code, actionIndex)` callback — mirrored
/// verbatim by `FrustNativePresenter.kt`'s `OUTCOME_*` constants and pinned
/// against drift by this module's tests. Consumed by the Android alert arm;
/// until that arm is built only the tests read it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod wire {
    use super::{AlertOutcome, PresentError};

    /// The user chose the action at `actionIndex` (spec order).
    pub(crate) const OUTCOME_ACTION: i32 = 0;
    /// The user cancelled (back key, outside tap).
    pub(crate) const OUTCOME_CANCELLED: i32 = 1;
    /// Programmatic dismissal.
    pub(crate) const OUTCOME_DISMISSED: i32 = 2;
    /// The hosting Activity was destroyed first.
    pub(crate) const OUTCOME_HOST_LOST: i32 = 3;
    /// No resumed Activity when the show actually ran.
    pub(crate) const OUTCOME_NO_HOST: i32 = 4;
    /// The show threw.
    pub(crate) const OUTCOME_FAILED: i32 = 5;

    /// Map one `nativeOnOutcome` report onto an alert's result, reading the
    /// chosen action's id out of `action_ids` (the spec's ids, in order).
    pub(crate) fn alert_outcome(
        code: i32,
        action_index: i32,
        action_ids: &[String],
    ) -> Result<AlertOutcome, PresentError> {
        match code {
            OUTCOME_ACTION => usize::try_from(action_index)
                .ok()
                .and_then(|index| action_ids.get(index))
                .map(|id| AlertOutcome::Action(id.clone()))
                .ok_or_else(|| {
                    PresentError::Platform(format!(
                        "action index {action_index} out of range for {} actions",
                        action_ids.len()
                    ))
                }),
            OUTCOME_CANCELLED => Ok(AlertOutcome::Cancelled),
            OUTCOME_DISMISSED => Ok(AlertOutcome::Dismissed),
            OUTCOME_HOST_LOST => Ok(AlertOutcome::HostLost),
            OUTCOME_NO_HOST => Err(PresentError::NoHost),
            OUTCOME_FAILED => Err(PresentError::Platform(
                "the presenter failed to show the alert".to_string(),
            )),
            other => Err(PresentError::Platform(format!(
                "unknown presenter outcome code {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::Ordering as AtomicOrdering;
    use std::task::Waker;

    use super::oneshot::tests::counting_waker;
    use super::*;

    /// Serializes every test that touches [`ACTIVE`] — one process-global
    /// slot, and `#[test]`s run on parallel threads.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// A scripted stand-in host: what `show_alert` should do, the one live
    /// sender it holds, and a record of every call.
    #[derive(Default)]
    struct FakeState {
        refuse: Option<PresentError>,
        live: Option<(u64, Sender<AlertOutcome>)>,
        shown: Vec<AlertSpec>,
        dismissed: Vec<u64>,
    }

    thread_local! {
        static FAKE: RefCell<FakeState> = RefCell::new(FakeState::default());
    }

    struct FakeHost;

    impl AlertHost for FakeHost {
        fn show_alert(
            spec: AlertSpec,
            tx: Sender<AlertOutcome>,
            generation: u64,
        ) -> Result<(), PresentError> {
            FAKE.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.shown.push(spec);
                if let Some(err) = fake.refuse.clone() {
                    return Err(err);
                }
                fake.live = Some((generation, tx));
                Ok(())
            })
        }

        fn dismiss(generation: u64) {
            let taken = FAKE.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.dismissed.push(generation);
                match &fake.live {
                    Some((live, _)) if *live == generation => fake.live.take(),
                    _ => None,
                }
            });
            // Resolved outside the borrow, like a real arm resolving outside
            // its own lock.
            if let Some((_, tx)) = taken {
                tx.send(Ok(AlertOutcome::Dismissed));
            }
        }
    }

    /// The platform delivering `outcome` for whatever is live on the fake —
    /// `false` when nothing is (a duplicate or stale callback).
    fn platform_resolves(outcome: Result<AlertOutcome, PresentError>) -> bool {
        match FAKE.with(|fake| fake.borrow_mut().live.take()) {
            Some((_, tx)) => {
                tx.send(outcome);
                true
            }
            None => false,
        }
    }

    /// Run `f` holding [`TEST_LOCK`], with [`ACTIVE`] and the fake reset
    /// before and after.
    fn isolated<R>(f: impl FnOnce() -> R) -> R {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reset = || {
            *lock_active() = None;
            FAKE.with(|fake| *fake.borrow_mut() = FakeState::default());
        };
        reset();
        let result = f();
        reset();
        result
    }

    fn poll_with<T>(
        presentation: &mut Presentation<T>,
        waker: &Waker,
    ) -> Poll<Result<T, PresentError>> {
        let mut cx = Context::from_waker(waker);
        Pin::new(presentation).poll(&mut cx)
    }

    fn poll_once<T>(presentation: &mut Presentation<T>) -> Poll<Result<T, PresentError>> {
        poll_with(presentation, &counting_waker().0)
    }

    fn two_button_spec() -> AlertSpec {
        AlertSpec::new("Delete draft?", "This cannot be undone.")
            .with_action("keep", "Keep", ActionRole::Cancel)
            .with_action("delete", "Delete", ActionRole::Destructive)
    }

    #[test]
    fn the_outcome_is_delivered_exactly_once() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            assert!(presentation.handle().is_some());

            let (waker, woken) = counting_waker();
            assert_eq!(poll_with(&mut presentation, &waker), Poll::Pending);

            assert!(platform_resolves(Ok(AlertOutcome::Action("delete".into()))));
            assert_eq!(woken.load(AtomicOrdering::SeqCst), 1);
            // The sender was consumed by that first delivery: a duplicate
            // platform callback has nothing left to resolve.
            assert!(!platform_resolves(Ok(AlertOutcome::Cancelled)));

            assert_eq!(
                poll_with(&mut presentation, &waker),
                Poll::Ready(Ok(AlertOutcome::Action("delete".into())))
            );
            assert_eq!(poll_with(&mut presentation, &waker), Poll::Pending);
            assert_eq!(woken.load(AtomicOrdering::SeqCst), 1);
        });
    }

    #[test]
    fn a_second_show_while_one_is_live_is_busy() {
        isolated(|| {
            let mut first = show_alert_on::<FakeHost>(two_button_spec());
            let mut second = show_alert_on::<FakeHost>(two_button_spec());

            assert_eq!(second.handle(), None);
            assert_eq!(poll_once(&mut second), Poll::Ready(Err(PresentError::Busy)));
            assert_eq!(
                FAKE.with(|fake| fake.borrow().shown.len()),
                1,
                "a Busy request must never reach the host"
            );
            // The refused request did not disturb the live one.
            assert_eq!(poll_once(&mut first), Poll::Pending);
            assert!(platform_resolves(Ok(AlertOutcome::Cancelled)));
            assert_eq!(
                poll_once(&mut first),
                Poll::Ready(Ok(AlertOutcome::Cancelled))
            );
        });
    }

    #[test]
    fn the_slot_is_free_again_once_the_outcome_is_sent() {
        isolated(|| {
            let first = show_alert_on::<FakeHost>(two_button_spec());
            assert!(platform_resolves(Ok(AlertOutcome::Cancelled)));
            // `first` is still held (not even polled): the send alone freed
            // the slot, so a continuation may present again at once.
            let mut next = show_alert_on::<FakeHost>(two_button_spec());
            assert!(next.handle().is_some());
            assert_eq!(poll_once(&mut next), Poll::Pending);
            drop(first);
        });
    }

    #[test]
    fn dismiss_resolves_dismissed_once() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            let handle = presentation.handle().expect("accepted");

            dismiss_on::<FakeHost>(&handle);
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(AlertOutcome::Dismissed))
            );

            // Now stale: filtered before it reaches the host at all.
            dismiss_on::<FakeHost>(&handle);
            assert_eq!(
                FAKE.with(|fake| fake.borrow().dismissed.clone()),
                vec![handle.generation]
            );
            assert_eq!(poll_once(&mut presentation), Poll::Pending);
        });
    }

    #[test]
    fn a_late_dismiss_for_an_old_generation_is_ignored() {
        isolated(|| {
            let old = show_alert_on::<FakeHost>(two_button_spec());
            let old_handle = old.handle().expect("accepted");
            assert!(platform_resolves(Ok(AlertOutcome::Action("keep".into()))));
            drop(old);

            let mut current = show_alert_on::<FakeHost>(two_button_spec());
            let current_handle = current.handle().expect("accepted");
            assert_ne!(old_handle, current_handle);

            dismiss_on::<FakeHost>(&old_handle);
            assert!(FAKE.with(|fake| fake.borrow().dismissed.is_empty()));
            assert_eq!(poll_once(&mut current), Poll::Pending);

            // Even a host asked directly with the old generation (the race
            // the trait doc names) leaves the live presentation alone.
            FakeHost::dismiss(old_handle.generation);
            assert_eq!(poll_once(&mut current), Poll::Pending);

            dismiss_on::<FakeHost>(&current_handle);
            assert_eq!(
                poll_once(&mut current),
                Poll::Ready(Ok(AlertOutcome::Dismissed))
            );
        });
    }

    #[test]
    fn host_lost_resolves_and_frees_the_slot() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            let handle = presentation.handle().expect("accepted");

            assert!(platform_resolves(Ok(AlertOutcome::HostLost)));
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(AlertOutcome::HostLost))
            );
            assert_eq!(*lock_active(), None);

            // A dismiss arriving after the host is gone is stale.
            dismiss_on::<FakeHost>(&handle);
            assert!(FAKE.with(|fake| fake.borrow().dismissed.is_empty()));
        });
    }

    #[test]
    fn dropping_the_presentation_releases_the_guard() {
        isolated(|| {
            let abandoned = show_alert_on::<FakeHost>(two_button_spec());
            drop(abandoned);
            assert_eq!(*lock_active(), None);

            // The abandoned alert is still up on the platform; its eventual
            // answer arrives after a newer presentation took the slot.
            let stale_sender = FAKE.with(|fake| fake.borrow_mut().live.take());
            let mut current = show_alert_on::<FakeHost>(two_button_spec());
            let (_, stale_tx) = stale_sender.expect("the fake held the first sender");
            assert!(!stale_tx.send(Ok(AlertOutcome::Cancelled)), "discarded");

            assert_eq!(poll_once(&mut current), Poll::Pending);
            assert_eq!(*lock_active(), current.handle().map(|h| h.generation));
        });
    }

    #[test]
    fn a_host_refusal_resolves_the_error_and_frees_the_slot() {
        isolated(|| {
            FAKE.with(|fake| fake.borrow_mut().refuse = Some(PresentError::NoHost));
            let mut refused = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(refused.handle(), None);
            assert_eq!(
                poll_once(&mut refused),
                Poll::Ready(Err(PresentError::NoHost))
            );
            assert_eq!(*lock_active(), None);
        });
    }

    #[test]
    fn an_arm_dropping_its_sender_resolves_platform_and_frees_the_slot() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            drop(FAKE.with(|fake| fake.borrow_mut().live.take()));
            assert_eq!(*lock_active(), None);
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Err(PresentError::Platform(
                    oneshot::DROPPED_WITHOUT_OUTCOME.to_string()
                )))
            );
        });
    }

    #[test]
    fn an_invalid_spec_is_refused_before_claiming_the_slot() {
        isolated(|| {
            let four = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Default)
                .with_action("b", "B", ActionRole::Default)
                .with_action("c", "C", ActionRole::Default)
                .with_action("d", "D", ActionRole::Default);
            let duplicate = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Default)
                .with_action("a", "Again", ActionRole::Default);
            let empty_id = AlertSpec::new("t", "m").with_action("", "A", ActionRole::Default);
            let two_cancels = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Cancel)
                .with_action("b", "B", ActionRole::Cancel);
            let mut bad_anchor = AlertSpec::new("t", "m");
            bad_anchor.style = AlertStyle::ActionSheet;
            bad_anchor.anchor = Some(AnchorRect {
                x: f64::NAN,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            });
            let mut negative_anchor = bad_anchor.clone();
            negative_anchor.anchor = Some(AnchorRect {
                x: 0.0,
                y: 0.0,
                width: -1.0,
                height: 10.0,
            });

            for spec in [
                four,
                duplicate,
                empty_id,
                two_cancels,
                bad_anchor,
                negative_anchor,
            ] {
                let mut presentation = show_alert_on::<FakeHost>(spec.clone());
                assert!(
                    matches!(
                        poll_once(&mut presentation),
                        Poll::Ready(Err(PresentError::InvalidSpec(_)))
                    ),
                    "{spec:?} should be refused"
                );
                assert_eq!(*lock_active(), None);
            }
            assert!(FAKE.with(|fake| fake.borrow().shown.is_empty()));
        });
    }

    #[test]
    fn a_valid_spec_passes() {
        let mut sheet = two_button_spec().with_action("share", "Share", ActionRole::Default);
        sheet.style = AlertStyle::ActionSheet;
        sheet.anchor = Some(AnchorRect {
            x: 10.0,
            y: 20.0,
            width: 0.0,
            height: 0.0,
        });
        assert_eq!(sheet.validate(), Ok(()));
        assert_eq!(AlertSpec::new("", "").validate(), Ok(()));
    }

    #[test]
    fn generations_are_never_zero_and_always_fresh() {
        let a = next_generation();
        let b = next_generation();
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn the_wire_table_maps_every_code() {
        let ids = ["keep".to_string(), "delete".to_string()];
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_ACTION, 1, &ids),
            Ok(AlertOutcome::Action("delete".into()))
        );
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_ACTION, 2, &ids),
            Err(PresentError::Platform(_))
        ));
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_ACTION, -1, &ids),
            Err(PresentError::Platform(_))
        ));
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_CANCELLED, -1, &ids),
            Ok(AlertOutcome::Cancelled)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_DISMISSED, -1, &ids),
            Ok(AlertOutcome::Dismissed)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_HOST_LOST, -1, &ids),
            Ok(AlertOutcome::HostLost)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_NO_HOST, -1, &ids),
            Err(PresentError::NoHost)
        );
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_FAILED, -1, &ids),
            Err(PresentError::Platform(_))
        ));
        assert!(matches!(
            wire::alert_outcome(99, -1, &ids),
            Err(PresentError::Platform(_))
        ));
    }

    // --- Kotlin <-> Rust drift checks for the presenter ------------------
    //
    // `FrustNativePresenter.kt` and `android_host.rs` are one contract: the
    // Kotlin package + class are baked into the Rust export's mangled symbol
    // and into the binary class name Rust loads, and the outcome codes are
    // shared integers. Neither side can be compiled against the other on a
    // host, so these read both sources as text.

    const PRESENTER_KT: &str = include_str!(
        "../../platform/android/src/main/kotlin/dev/frust/nativewidgets/FrustNativePresenter.kt"
    );
    const ANDROID_HOST_RS: &str = include_str!("android_host.rs");

    fn kotlin_package() -> &'static str {
        PRESENTER_KT
            .lines()
            .find_map(|line| line.trim().strip_prefix("package "))
            .expect("FrustNativePresenter.kt declares a package")
            .trim()
    }

    fn kotlin_object_name() -> String {
        PRESENTER_KT
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("object ")
                    .filter(|rest| rest.starts_with(char::is_uppercase))
            })
            .expect("FrustNativePresenter.kt declares a named `object`")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }

    #[test]
    fn presenter_outcome_codes_match_between_kotlin_and_rust() {
        let mut kotlin = BTreeMap::new();
        for line in PRESENTER_KT.lines() {
            let Some(rest) = line.trim().strip_prefix("const val OUTCOME_") else {
                continue;
            };
            let (name, value) = rest.split_once('=').expect("`NAME = value`");
            let value: i32 = value
                .trim()
                .parse()
                .unwrap_or_else(|e| panic!("OUTCOME_{name}: {e}"));
            kotlin.insert(name.trim().to_string(), value);
        }
        let rust = BTreeMap::from([
            ("ACTION".to_string(), wire::OUTCOME_ACTION),
            ("CANCELLED".to_string(), wire::OUTCOME_CANCELLED),
            ("DISMISSED".to_string(), wire::OUTCOME_DISMISSED),
            ("HOST_LOST".to_string(), wire::OUTCOME_HOST_LOST),
            ("NO_HOST".to_string(), wire::OUTCOME_NO_HOST),
            ("FAILED".to_string(), wire::OUTCOME_FAILED),
        ]);
        assert_eq!(
            kotlin, rust,
            "FrustNativePresenter.kt's OUTCOME_* constants and present::wire must be edited \
             together"
        );
    }

    #[test]
    fn presenter_class_and_export_match_the_kotlin_declaration() {
        let package = kotlin_package();
        let object = kotlin_object_name();

        let binary = format!("\"{package}.{object}\"");
        assert!(
            ANDROID_HOST_RS.contains(&format!("const PRESENTER_CLASS_BINARY: &str = {binary};")),
            "android_host.rs's PRESENTER_CLASS_BINARY must be {binary}"
        );

        assert!(
            PRESENTER_KT.contains(
                "external fun nativeOnOutcome(generation: Long, code: Int, actionIndex: Int)"
            ),
            "FrustNativePresenter.kt must declare the nativeOnOutcome(Long, Int, Int) callback"
        );
        let symbol = format!(
            "pub extern \"system\" fn Java_{}_{object}_nativeOnOutcome",
            package.replace('.', "_")
        );
        assert!(
            ANDROID_HOST_RS.contains(&symbol),
            "android_host.rs must export `{symbol}` — the JVM binds `external` methods by \
             mangled name alone"
        );
    }
}
