//! App-facing pending-font registry:
//! `frust::register_app_fonts`.
//!
//! # The gap this closes
//!
//! Before this module, custom font registration
//! ([`frust_text::TextContext::register_fonts`]) was reachable only
//! from code holding a `&mut TextContext` — the shell itself, never app code.
//! An app wanting to bundle a custom font (a brand typeface, an icon font)
//! had no seam to push bytes in before the shell's `TextContext` exists (app
//! construction time) or after the app is already running.
//!
//! # Layering choice
//!
//! This process-global slot lives in `frust-shell-common`, mirroring
//! [`crate::theme_override`]'s slot+poll shape exactly (see that module's
//! doc comment for the fuller layering rationale, which applies here
//! unchanged): `frust-shell-common` already owns the shared, non-FFI
//! "poll a process-global once per frame" plumbing every shell composes,
//! and already depends on `frust-text` for the `TextContext` type
//! [`FontRegistryWatcher::drain_into`] threads through.
//!
//! # Thread contract
//!
//! Like `theme_override`, this is a plain `Mutex`-guarded slot with **no
//! thread restriction** — [`register_app_fonts`] may be called from any
//! thread (documented, not enforced by a panic). Each shell only *drains*
//! the slot on its own UI thread (construction time, and once per frame),
//! so a write racing in from a background thread is simply
//! picked up (or not) on the next drain.
//!
//! # Payload shape
//!
//! The registry stores raw byte payloads (`Vec<u8>`), not a `TextContext`
//! reference — `register_app_fonts` may run before any shell's
//! `TextContext` exists (app construction time), so the slot cannot borrow
//! or depend on one. [`FontRegistryWatcher::drain_into`] is the convenience
//! a shell calls once it has a `&mut TextContext` to apply against.

use std::sync::Mutex;

use frust_text::TextContext;

/// The process-wide pending-font slot: byte payloads pushed via
/// [`register_app_fonts`] that no [`FontRegistryWatcher`] has drained yet,
/// plus a generation counter bumped on every push — mirrors
/// [`crate::theme_override`]'s `OverrideSlot` shape.
struct PendingSlot {
    pending: Vec<Vec<u8>>,
    generation: u64,
}

static PENDING: Mutex<PendingSlot> = Mutex::new(PendingSlot {
    pending: Vec::new(),
    generation: 0,
});

/// Push raw font bytes (TTF/OTF, or a TTC/OTC collection) to be registered
/// into the shell-owned [`TextContext`] the next time a running shell drains
/// this registry (construction time, and once per frame).
///
/// Callable from any thread (see the module docs' thread contract) — the
/// process-wide slot is a plain `Mutex`. Multiple pushes before a drain all
/// apply, in push order (`Vec` push order is preserved end to end).
pub fn register_app_fonts(data: Vec<u8>) {
    let mut slot = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    slot.pending.push(data);
    slot.generation += 1;
}

/// Per-shell-instance watcher over the process-wide pending-font slot: each
/// shell owns one, draining it at construction time and once per frame
/// (mirroring [`crate::theme_override::ThemeOverrideWatcher`]) to pick up
/// any [`register_app_fonts`] call since the last drain.
#[derive(Debug, Default)]
pub struct FontRegistryWatcher {
    /// The slot generation as of the last [`poll`](Self::poll)/
    /// [`drain_into`](Self::drain_into) call. Starts at `0`, matching the
    /// slot's initial generation, so a fresh watcher never reports a pending
    /// change until `register_app_fonts` is actually called.
    last_generation: u64,
}

impl FontRegistryWatcher {
    /// A fresh watcher, matching the slot's initial (nothing-pending) state.
    pub fn new() -> Self {
        Self { last_generation: 0 }
    }

    /// Drain every payload pushed since the last poll, in push order.
    /// Returns an empty `Vec` (no allocation beyond the empty vec itself)
    /// when nothing is pending — cheap to call unconditionally every frame.
    pub fn poll(&mut self) -> Vec<Vec<u8>> {
        let mut slot = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if slot.generation == self.last_generation {
            return Vec::new();
        }
        self.last_generation = slot.generation;
        std::mem::take(&mut slot.pending)
    }

    /// Convenience: [`poll`](Self::poll), then apply every drained payload to
    /// `cx` via [`TextContext::register_fonts`], in push order. Returns
    /// whether at least one payload registered successfully — a payload
    /// [`register_fonts`](TextContext::register_fonts) rejects (invalid/empty
    /// bytes) is skipped rather than aborting the remaining drain, since one
    /// bad app-supplied payload shouldn't block the rest.
    ///
    /// Mirrors the caller-visible relayout contract
    /// [`TextContext::register_fonts`] documents: a shell calling this after
    /// it has already laid out text must force `ChangeFlags::LAYOUT | PAINT`
    /// on a `true` return, the same way a theme swap does — this module only
    /// owns the drain half of that contract.
    pub fn drain_into(&mut self, cx: &mut TextContext) -> bool {
        let payloads = self.poll();
        let mut applied = false;
        for data in payloads {
            if cx.register_fonts(data).is_ok() {
                applied = true;
            }
        }
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;

    /// Serializes every test in this module against the shared process-wide
    /// `PENDING` static — mirrors `theme_override`'s `TEST_LOCK` pattern for
    /// a global the crate under test owns.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// A tiny valid TTF face (shared with `frust-text`'s own registration
    /// tests) — kept as the cross-crate smoke
    /// fixture rather than duplicating a font asset in this crate.
    const TUFFY: &[u8] = include_bytes!("../../frust-text/tests/fonts/Tuffy-Subset.ttf");

    /// Reset the process-wide slot to its pristine (nothing-pending) state so
    /// each test starts from a known baseline regardless of execution order.
    fn reset_slot() {
        let mut slot = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        slot.pending.clear();
        slot.generation = 0;
    }

    #[test]
    fn push_then_poll_returns_payloads_in_order_second_poll_empty() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = FontRegistryWatcher::new();
        assert_eq!(watcher.poll(), Vec::<Vec<u8>>::new());

        register_app_fonts(vec![1, 2, 3]);
        register_app_fonts(vec![4, 5, 6]);

        let drained = watcher.poll();
        assert_eq!(drained, vec![vec![1, 2, 3], vec![4, 5, 6]]);

        // The same generation is not re-delivered on a second poll.
        assert_eq!(watcher.poll(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn push_from_a_spawned_thread_is_observed_on_the_next_poll() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = FontRegistryWatcher::new();
        assert_eq!(watcher.poll(), Vec::<Vec<u8>>::new());

        let handle = thread::spawn(|| {
            register_app_fonts(vec![9, 9, 9]);
        });
        handle.join().expect("spawned thread must not panic");

        assert_eq!(watcher.poll(), vec![vec![9, 9, 9]]);
        assert_eq!(watcher.poll(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn independent_watchers_each_drain_the_pending_payloads_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut a = FontRegistryWatcher::new();
        register_app_fonts(vec![7]);

        // `a` drains the pending payload...
        assert_eq!(a.poll(), vec![vec![7]]);
        assert_eq!(a.poll(), Vec::<Vec<u8>>::new());

        // ...a watcher created after the drain sees nothing pending either,
        // since the slot itself is now empty (draining is destructive, like
        // theme_override's `Mutex` slot is overwrite-only).
        let mut b = FontRegistryWatcher::new();
        assert_eq!(b.poll(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn drain_into_applies_a_registered_font_and_reports_applied() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        register_app_fonts(TUFFY.to_vec());

        let mut watcher = FontRegistryWatcher::new();
        let mut cx = TextContext::new();
        assert!(
            watcher.drain_into(&mut cx),
            "a valid font payload must report applied=true"
        );

        // Second drain: nothing pending, so nothing applied.
        assert!(!watcher.drain_into(&mut cx));
    }

    #[test]
    fn drain_into_skips_invalid_payloads_without_panicking() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        register_app_fonts(Vec::new()); // invalid: no parseable faces

        let mut watcher = FontRegistryWatcher::new();
        let mut cx = TextContext::new();
        assert!(
            !watcher.drain_into(&mut cx),
            "an invalid payload must not report applied=true"
        );
    }
}
