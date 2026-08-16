//! Process-wide **menu-activation** source: a desktop shell delivers a native
//! menu item's activation (an NSApp menu bar item on macOS, a `TranslateAccelerator`
//! -dispatched HMENU item on Windows) via [`push_menu_event`]; app code reads the
//! current state through [`menu_events`]/[`MenuEvents`] — the `frust` facade
//! re-exports both as `frust::menu_events()`/`frust::MenuEvents`, so app code
//! never names this crate directly.
//!
//! This mirrors the deep-link source next door (`frust-reactive::deep_link`) in
//! both shape and layering: one process-wide slot holding a lazily-created
//! [`RwSignal`], a shell-only push entry point, and an app-facing read surface —
//! and, like it, `frust-reactive` stays a leaf: the *menu vocabulary* (which
//! items exist, their labels/accelerators/roles) lives in the desktop shell tier
//! that owns the platform menu, never here. All that crosses this seam is the
//! activated item's id.
//!
//! # Why this is a reactive source, not a `theme_override`-style polled slot
//!
//! `frust-shell-common::theme_override`'s module docs draw the line: a value
//! every shell polls once per frame and pushes into non-reactive delivery paths
//! belongs there, while a source app code *subscribes* to belongs here. A menu
//! activation is the second kind — an app reacts to "Preferences… was chosen" by
//! writing its own state from a tracked read, exactly as it reacts to a deep
//! link, so the write must wake the shell through the tracked-signal/[`FrameWaker`]
//! machinery. (The per-OS shell still *drains* its platform menu queue once per
//! frame; that pump is the producer side, and it pushes here.)
//!
//! [`FrameWaker`]: crate::FrameWaker
//!
//! # A sequence number, not a bare id
//!
//! Each activation is an *event*, and unlike a deep link the same payload
//! repeats constantly — "Zoom In" chosen three times is three activations of one
//! id. So [`MenuEvent`] carries a monotonically increasing [`sequence`](MenuEvent::sequence)
//! beside the id, and glue dedupes by the last sequence it consumed (the
//! `RouterDeepLinks` consumed-marker pattern, and the same reasoning that makes
//! the back-press source a counter — see `crate::back`). Without it, a second
//! activation of an already-consumed id would be indistinguishable from the
//! first for any consumer that compares values rather than tracking writes.
//!
//! # Thread contract
//!
//! [`push_menu_event`] must be called on the UI thread — the one
//! [`ReactiveRuntime::init`] ran on — mirroring [`push_deep_link`](crate::push_deep_link)'s
//! contract: a desktop shell drains its platform menu queue from inside the
//! winit event loop, i.e. always on the UI thread, so an off-thread call is a
//! wiring bug, not a runtime-data condition, and panics with the same message
//! convention `Executor::spawn_local` uses.
//!
//! A push that races ahead of [`ReactiveRuntime::init`] (the shell installs the
//! menu only after `run_desktop` has initialized the runtime, so this should not
//! happen through it) is **dropped with a logged warning** rather than panicking
//! or buffering indefinitely — the same rationale the deep-link source records:
//! buffering would need a bound and a flush point for a path that isn't expected
//! to be exercised.

use std::sync::OnceLock;

use reactive_graph::signal::RwSignal;
use reactive_graph::traits::Update;

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// One delivered menu activation: the id the app gave the item in its
/// `MenuSpec` (e.g. `"file.open"`), plus the process-wide sequence number of
/// this activation (see the module docs). The id is passed through verbatim —
/// mapping it to an action is app code's job, not this crate's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEvent {
    /// The activated item's app-chosen id, exactly as it appeared in the
    /// `MenuSpec` the shell was configured with.
    pub id: String,
    /// This activation's position in the process-wide activation sequence,
    /// starting at `1` for the first push. Strictly increasing, so glue can
    /// dedupe by "the last sequence I consumed" and tell a repeat activation of
    /// the same id apart from a re-read of the previous one.
    pub sequence: u64,
}

impl MenuEvent {
    /// Pair an item id with its activation sequence number.
    ///
    /// Public for glue and tests that need to construct the value the signal
    /// carries; production pushes go through [`push_menu_event`], which assigns
    /// the sequence itself.
    pub fn new(id: impl Into<String>, sequence: u64) -> Self {
        Self {
            id: id.into(),
            sequence,
        }
    }
}

/// The app-facing menu-activation read surface. Obtained via [`menu_events`]
/// (`frust::menu_events()` at the facade).
#[derive(Clone)]
pub struct MenuEvents {
    /// The most recently activated item, or `None` when nothing has been
    /// activated in this process yet. Read/track it with the `Get`/`Track`
    /// traits (`frust::{Get, Track}`) the same way any other `RwSignal` is read.
    ///
    /// There is deliberately no `initial`-style snapshot beside it (the
    /// deep-link source's cold-start half): a menu cannot be activated before
    /// the app it belongs to is running, so "the first activation ever" carries
    /// no precedence meaning worth recording.
    pub latest: RwSignal<Option<MenuEvent>>,
}

/// The process-wide menu-activation slot. Lazily created (mirroring
/// [`ReactiveRuntime`]'s own process-wide, lazily-installed static, and the
/// deep-link slot next door) on first access once [`ReactiveRuntime`] exists,
/// without needing to modify `ReactiveRuntime::init` itself.
///
/// The sequence counter needs no state of its own: each push derives the next
/// number from the value already in the signal, so the signal *is* the whole
/// slot.
static SLOT: OnceLock<RwSignal<Option<MenuEvent>>> = OnceLock::new();

/// Returns the process-wide signal, creating it (under the reactive root
/// [`Owner`](reactive_graph::owner::Owner)) on first access.
///
/// # Panics
///
/// Panics if [`ReactiveRuntime::init`] has not run yet — reading a menu event
/// before the reactive runtime exists (`push_menu_event`'s pre-init case is
/// handled separately, before this is ever called) is a genuine wiring bug: the
/// caller must initialize the runtime first.
fn slot() -> &'static RwSignal<Option<MenuEvent>> {
    SLOT.get_or_init(|| {
        let rt = ReactiveRuntime::get().expect(
            "frust-reactive: menu_events() was called before ReactiveRuntime::init — an app \
             must run under the Frust facade's entry point (which initializes the reactive \
             runtime) before reading menu events",
        );
        rt.with_owner(|| RwSignal::new(None))
    })
}

/// Deliver a native menu activation into the process-wide source. Called by a
/// desktop shell's per-frame menu pump on the UI thread; app code never calls
/// this directly.
///
/// The activation is stamped with the next sequence number (derived from the
/// one already in the signal, starting at `1`) and written to
/// [`MenuEvents::latest`], so a tracked reader is woken even when the same item
/// is chosen twice in a row (see the module docs).
///
/// # Panics
///
/// Panics if called off the UI thread (see the module docs' thread contract). A
/// call before [`ReactiveRuntime::init`] does **not** panic — it is dropped with
/// a logged warning (see the module docs).
pub fn push_menu_event(id: impl Into<String>) {
    let id = id.into();

    if !is_ui_thread() {
        panic!(
            "frust-reactive: push_menu_event was called off the UI thread. Menu events can \
             only be pushed from the UI thread (the one `ReactiveRuntime::init` ran on) — this \
             is a wiring bug: drain the platform menu queue from the shell's own event loop, \
             the same contract `Executor::spawn_local` enforces."
        );
    }

    if ReactiveRuntime::get().is_none() {
        eprintln!(
            "frust-reactive: push_menu_event(\"{id}\") dropped — ReactiveRuntime::init has \
             not run yet. A shell installs its platform menu only after the runtime exists; \
             reaching this indicates an odd init-ordering race, not normal operation."
        );
        return;
    }

    slot().update(|latest| {
        let sequence = latest.as_ref().map_or(1, |event| event.sequence + 1);
        *latest = Some(MenuEvent::new(id, sequence));
    });
}

/// The current menu-activation read surface: the live [`MenuEvents::latest`]
/// signal. Call from a tracked context (e.g. inside `Component::build`) to
/// observe subsequent activations as they arrive.
pub fn menu_events() -> MenuEvents {
    MenuEvents { latest: *slot() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameWaker, TrackedScope};
    use reactive_graph::traits::{Get, GetUntracked};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (waker, seen)
    }

    /// Every acceptance criterion in one `#[test]`, serialized on the shared
    /// waker lock (`ReactiveRuntime::init` here swaps the process-wide waker,
    /// which would otherwise race the other waker-asserting tests in this
    /// crate — see `WAKER_TEST_LOCK`'s doc comment in `lib.rs`). The slot is
    /// process-wide with no reset, so every assertion below is relative to the
    /// sequence observed at the start rather than an absolute number.
    #[test]
    fn menu_push_and_wake_bridge() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker, wakes) = recording_waker();
        let _rt = ReactiveRuntime::init(waker);

        // Criterion: push before any tracked read — a late subscriber sees it
        // immediately through the live signal.
        push_menu_event("file.open");
        let first = menu_events()
            .latest
            .get_untracked()
            .expect("a push must leave the latest activation readable");
        assert_eq!(first.id, "file.open");

        // A tracked scope reading `latest` after the push observes the
        // already-pushed activation with no wake needed — an ordinary read.
        let scope = TrackedScope::new();
        let seen = scope.track(|| menu_events().latest.get());
        assert_eq!(seen, Some(first.clone()));
        assert!(!scope.is_dirty(), "a fresh track starts clean");

        // Criterion: push after a tracked read fires the signal-write -> wake
        // contract — the tracked scope re-dirties and the waker fires exactly
        // once (coalesced).
        let before = wakes.load(Ordering::SeqCst);
        push_menu_event("file.save");
        assert!(
            scope.is_dirty(),
            "push_menu_event must dirty a scope tracking `latest`"
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst) - before,
            1,
            "a push must fire the waker exactly once"
        );

        let second = menu_events()
            .latest
            .get_untracked()
            .expect("the second push is readable too");
        assert_eq!(second.id, "file.save");
        assert_eq!(
            second.sequence,
            first.sequence + 1,
            "each activation takes the next sequence number"
        );

        // Criterion: the same id activated twice is still two distinguishable
        // events — the reason the sequence number exists at all (see the module
        // docs).
        push_menu_event("file.save");
        let third = menu_events()
            .latest
            .get_untracked()
            .expect("the repeat activation is readable");
        assert_eq!(third.id, second.id);
        assert_ne!(
            third, second,
            "a repeat activation of the same id must not compare equal to the previous one"
        );
        assert_eq!(third.sequence, second.sequence + 1);
    }

    /// Criterion: the UI-thread contract is enforced (mirrors
    /// `push_deep_link`'s off-thread panic). Also serializes on the waker lock
    /// since `ReactiveRuntime::init` swaps the process-wide waker.
    #[test]
    #[should_panic(expected = "wiring bug")]
    fn push_off_ui_thread_panics() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        std::thread::spawn(|| {
            push_menu_event("file.open");
        })
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
