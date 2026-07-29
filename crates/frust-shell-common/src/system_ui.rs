//! App-facing system-UI (system-bar) override slot: `frust::set_system_ui_mode`
//! (Flutter `SystemChrome.setEnabledSystemUIMode` parity).
//!
//! # The gap this closes
//!
//! Before this module every app that wanted to hide the status/navigation
//! bars hardcoded the platform call directly in generated project glue (e.g.
//! `examples/shadertoy/android/.../MainActivity.kt`'s
//! `WindowCompat.getInsetsController(...).hide(systemBars())`, or
//! `FrustViewController.swift`'s `prefersStatusBarHidden`) — there was no
//! app-facing Rust API and no cross-platform channel to drive one from. This
//! module is the Rust-side half; each mobile shell's own FFI layer decodes
//! and applies it, threaded into that shell's per-frame wiring.
//!
//! # Layering choice
//!
//! Same rationale as [`crate::theme_override`]: `frust-shell-common` already
//! owns the "process-global `Mutex` slot + generation counter, polled once
//! per frame by a per-shell watcher" pattern
//! ([`crate::theme_override::ThemeOverrideWatcher`],
//! [`crate::font_registry::FontRegistryWatcher`]) — this module mirrors that
//! shape exactly rather than introducing a new one. It adds no new
//! dependency: `SystemUiMode`/`SystemUiOverlay` are plain enums, not
//! `frust-theme` types.
//!
//! # Thread contract
//!
//! Like [`crate::theme_override::set_app_theme`], [`set_system_ui_mode`] is
//! callable from any thread — a plain `Mutex` guards the slot, and each
//! shell only *observes* it once per frame on its own UI thread via
//! [`SystemUiWatcher::poll`] (or the FFI-side [`encoded_state`] peek —
//! see below).
//!
//! # FFI encoding
//!
//! [`encoded_state`] is the single source of the wire format both mobile
//! shells' FFI getters export verbatim; each shell's own Kotlin/Swift
//! decoder is built against this doc. The packed `u64` is
//! `(generation << 8) | mode_bits`:
//!
//! - low byte, mode discriminant: `0` = [`SystemUiMode::EdgeToEdge`], `1` =
//!   [`SystemUiMode::Immersive`], `2` = [`SystemUiMode::ImmersiveSticky`],
//!   `3` = [`SystemUiMode::LeanBack`], `4` = [`SystemUiMode::Manual`].
//! - for `Manual`, two additional flag bits on top of the `4` discriminant:
//!   bit 4 (`0x10`) = `top`, bit 5 (`0x20`) = `bottom`.
//! - generation occupies every bit above the low byte, so a platform side
//!   can tell "changed since I last looked" apart from "still the same
//!   value" the same way [`SystemUiWatcher::poll`] does, without needing a
//!   second FFI call.
//!
//! Generation `0` (the initial, never-called state) means nothing has been
//! requested yet — a platform shell should leave its own default system-bar
//! behavior untouched until it observes a generation advance.
//!
//! # Platform behavior differences
//!
//! This module models the full Flutter-parity vocabulary, but neither
//! platform can express all five modes faithfully:
//!
//! - **Android 16 (API 36+) forces edge-to-edge** and silently ignores every
//!   other mode (a Flutter breaking change carried over here, not a Frust
//!   choice) — an app targeting API 36+ that requests
//!   [`SystemUiMode::Immersive`] (or any non-`EdgeToEdge` mode) sees no
//!   effect on those OS versions.
//! - **iOS has no sticky/non-sticky or lean-back distinction.** Every
//!   hiding mode (`Immersive`/`ImmersiveSticky`/`LeanBack`) maps to the same
//!   iOS behavior: status bar hidden + home-indicator *auto*-hide (never a
//!   force-hide) — the system, not the app, decides when a swipe re-reveals
//!   it, and always swallows the edge-swipe gesture rather than delivering
//!   it to the app (unlike Android's `Immersive`, which lets the gesture
//!   through). `Manual { top, bottom }` on iOS folds to hiding the status
//!   bar when `!top` and has no separate control for `bottom` (there is no
//!   iOS home-indicator equivalent of a bottom system bar to show/hide
//!   independently).

use std::sync::Mutex;

/// One of the two system bars a [`SystemUiMode::Manual`] mode can name —
/// Flutter's `SystemUiOverlay` kept here for doc/mapping parity even though
/// [`SystemUiMode::Manual`] itself uses named bools (`top`/`bottom`) rather
/// than a `Vec<SystemUiOverlay>`, the more Rust-idiomatic shape for a
/// fixed two-element set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SystemUiOverlay {
    /// The top system bar (Android status bar; iOS status bar).
    Top,
    /// The bottom system bar (Android navigation bar; iOS home indicator).
    Bottom,
}

/// The requested system-bar visibility mode (Flutter `SystemUiMode` parity —
/// see the module docs' platform-behavior-differences section for where
/// Android/iOS diverge from this vocabulary).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SystemUiMode {
    /// Default: bars visible, app draws edge-to-edge behind them.
    EdgeToEdge,
    /// Hide all bars; any edge swipe re-shows them (system keeps the gesture).
    Immersive,
    /// Hide all bars; transient overlay on swipe, auto-hides again.
    ImmersiveSticky,
    /// Hide all bars; re-shown by system interactions (tap on Android leanback).
    LeanBack,
    /// Show exactly the listed overlays.
    Manual {
        /// Whether the top bar (status bar) is shown.
        top: bool,
        /// Whether the bottom bar (nav bar / home indicator) is shown.
        bottom: bool,
    },
}

/// The process-wide slot: the last requested [`SystemUiMode`] plus a
/// generation counter bumped on every [`set_system_ui_mode`] call, so a
/// [`SystemUiWatcher`] (or the FFI-side [`encoded_state`] peek) can tell
/// "changed since I last looked" apart from "still the same value".
struct SystemUiSlot {
    mode: SystemUiMode,
    generation: u64,
}

/// Initial state: [`SystemUiMode::EdgeToEdge`] at generation `0` — "nothing
/// to apply", per the module docs' FFI-encoding section. Platform defaults
/// stand until an app actually calls [`set_system_ui_mode`].
static SYSTEM_UI: Mutex<SystemUiSlot> = Mutex::new(SystemUiSlot {
    mode: SystemUiMode::EdgeToEdge,
    generation: 0,
});

/// Request a system-bar visibility mode, reaching whichever shell is running
/// the next time it polls (once per frame — see the module docs' thread
/// contract). Callable from any thread; the process-wide slot is a plain
/// `Mutex`, not a UI-thread-only primitive.
pub fn set_system_ui_mode(mode: SystemUiMode) {
    let mut slot = SYSTEM_UI.lock().unwrap_or_else(|e| e.into_inner());
    slot.mode = mode;
    slot.generation += 1;
}

/// A cheap peek at the slot's current `(generation, mode)` pair, for a
/// caller that wants the raw state without consuming/tracking a
/// [`SystemUiWatcher`]'s "last seen" cursor — e.g. [`encoded_state`], or an
/// FFI glue module polling from the platform side.
pub fn current_system_ui_mode() -> (u64, SystemUiMode) {
    let slot = SYSTEM_UI.lock().unwrap_or_else(|e| e.into_inner());
    (slot.generation, slot.mode)
}

/// Pack the slot's current `(generation, mode)` into a single `u64` for an
/// FFI getter to return verbatim — see the module docs' FFI-encoding
/// section for the exact bit layout. Each mobile shell exports this
/// unchanged; its own Kotlin/Swift decoder is built against it.
pub fn encoded_state() -> u64 {
    let (generation, mode) = current_system_ui_mode();
    let low: u64 = match mode {
        SystemUiMode::EdgeToEdge => 0,
        SystemUiMode::Immersive => 1,
        SystemUiMode::ImmersiveSticky => 2,
        SystemUiMode::LeanBack => 3,
        SystemUiMode::Manual { top, bottom } => 4 | ((top as u64) << 4) | ((bottom as u64) << 5),
    };
    (generation << 8) | low
}

/// Per-shell-instance watcher over the process-wide system-UI slot: each
/// shell owns one, polling it once per frame (mirroring
/// [`crate::theme_override::ThemeOverrideWatcher`]) to detect a
/// [`set_system_ui_mode`] call since the last poll.
#[derive(Debug, Default)]
pub struct SystemUiWatcher {
    /// The slot generation as of the last [`poll`](Self::poll) call. Starts
    /// at `0`, matching the slot's initial generation, so a shell that never
    /// observes a `set_system_ui_mode` call never sees a change.
    last_generation: u64,
}

impl SystemUiWatcher {
    /// A fresh watcher, matching the slot's initial (never-requested) state.
    pub fn new() -> Self {
        Self { last_generation: 0 }
    }

    /// Poll the slot once. Returns:
    /// - `None` — no [`set_system_ui_mode`] call since the last poll (or
    ///   since construction); the shell does nothing.
    /// - `Some(mode)` — a new mode to apply.
    pub fn poll(&mut self) -> Option<SystemUiMode> {
        let slot = SYSTEM_UI.lock().unwrap_or_else(|e| e.into_inner());
        if slot.generation == self.last_generation {
            return None;
        }
        self.last_generation = slot.generation;
        Some(slot.mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;

    // Serializes every test in this module against the shared process-wide
    // `SYSTEM_UI` static — mirrors `theme_override`'s `TEST_LOCK` pattern for
    // a global the crate under test owns.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Reset the process-wide slot to its pristine (never-requested) state so
    /// each test starts from a known baseline regardless of execution order.
    fn reset_slot() {
        let mut slot = SYSTEM_UI.lock().unwrap_or_else(|e| e.into_inner());
        slot.mode = SystemUiMode::EdgeToEdge;
        slot.generation = 0;
    }

    #[test]
    fn set_system_ui_mode_bumps_generation_and_watcher_observes_it_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = SystemUiWatcher::new();
        // No call yet: a fresh watcher sees no pending change.
        assert_eq!(watcher.poll(), None);

        set_system_ui_mode(SystemUiMode::Immersive);

        let observed = watcher.poll();
        assert_eq!(observed, Some(SystemUiMode::Immersive));
        // The same generation is not re-delivered on a second poll.
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn independent_watchers_each_see_the_change_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut a = SystemUiWatcher::new();
        let mut b = SystemUiWatcher::new();
        set_system_ui_mode(SystemUiMode::LeanBack);

        assert_eq!(a.poll(), Some(SystemUiMode::LeanBack));
        assert_eq!(b.poll(), Some(SystemUiMode::LeanBack));
        assert_eq!(a.poll(), None);
        assert_eq!(b.poll(), None);
    }

    #[test]
    fn never_calling_the_api_leaves_a_fresh_watcher_silent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = SystemUiWatcher::new();
        assert_eq!(watcher.poll(), None);
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn set_from_a_spawned_thread_is_observed_on_the_polling_thread() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = SystemUiWatcher::new();
        assert_eq!(watcher.poll(), None);

        thread::spawn(|| {
            set_system_ui_mode(SystemUiMode::ImmersiveSticky);
        })
        .join()
        .unwrap();

        assert_eq!(watcher.poll(), Some(SystemUiMode::ImmersiveSticky));
    }

    #[test]
    fn encoded_state_packs_generation_and_each_mode() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        // Initial state: generation 0, EdgeToEdge (mode bits 0).
        assert_eq!(encoded_state(), 0);

        set_system_ui_mode(SystemUiMode::EdgeToEdge);
        assert_eq!(encoded_state(), 1u64 << 8);

        set_system_ui_mode(SystemUiMode::Immersive);
        assert_eq!(encoded_state(), (2u64 << 8) | 1);

        set_system_ui_mode(SystemUiMode::ImmersiveSticky);
        assert_eq!(encoded_state(), (3u64 << 8) | 2);

        set_system_ui_mode(SystemUiMode::LeanBack);
        assert_eq!(encoded_state(), (4u64 << 8) | 3);
    }

    #[test]
    fn encoded_state_packs_manual_flag_combinations() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        set_system_ui_mode(SystemUiMode::Manual {
            top: false,
            bottom: false,
        });
        assert_eq!(encoded_state(), (1u64 << 8) | 4);

        set_system_ui_mode(SystemUiMode::Manual {
            top: true,
            bottom: false,
        });
        assert_eq!(encoded_state(), (2u64 << 8) | 4 | (1 << 4));

        set_system_ui_mode(SystemUiMode::Manual {
            top: false,
            bottom: true,
        });
        assert_eq!(encoded_state(), (3u64 << 8) | 4 | (1 << 5));

        set_system_ui_mode(SystemUiMode::Manual {
            top: true,
            bottom: true,
        });
        assert_eq!(encoded_state(), (4u64 << 8) | 4 | (1 << 4) | (1 << 5));
    }

    #[test]
    fn current_system_ui_mode_peeks_without_consuming_a_watcher_cursor() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        assert_eq!(current_system_ui_mode(), (0, SystemUiMode::EdgeToEdge));
        set_system_ui_mode(SystemUiMode::Immersive);
        assert_eq!(current_system_ui_mode(), (1, SystemUiMode::Immersive));
        // A peek doesn't consume anything — repeated calls see the same value.
        assert_eq!(current_system_ui_mode(), (1, SystemUiMode::Immersive));
    }
}
