//! App-facing theme override seam: `frust::set_app_theme`/
//! `clear_app_theme`.
//!
//! # The gap this closes
//!
//! Before this module, only a *shell* could set the active [`Theme`] —
//! `RenderRoot::set_theme` (the widget path) and `provide_context` (the
//! `use_context::<Theme>()` app-code path) were both shell-owned exclusively,
//! seeded from the platform's light/dark preference (`WindowEvent::ThemeChanged`,
//! `nativeSetAppearance`, `frust_set_appearance`). An app that wanted to force
//! a specific `Theme` (e.g. the widget catalog's Material/Cupertino toggle)
//! had nowhere to hook in — the gallery example's brightness toggle
//! only touched the `provide_context` copy, which no widget reads (a
//! documented gap this module closes).
//!
//! # Layering choice
//!
//! This process-global slot lives in `frust-shell-common`, not
//! `frust-reactive` or `frust-theme`:
//!
//! - **Not `frust-theme`**: that crate is pure data + constructors
//!   with no process-global/shell-polling concept of its own — adding one
//!   would give the design-token crate a state-management responsibility it
//!   has never had, and every consumer (including non-shell contexts, if any
//!   ever exist) would inherit it.
//! - **Not `frust-reactive`**: the deep-link slot there
//!   (`frust_reactive::deep_link`) is a genuinely *reactive* source — app
//!   code subscribes to `DeepLinks::latest` via `Get`/`Track` and a write wakes
//!   the shell through the tracked-signal/`FrameWaker` machinery. A theme
//!   override is not: no widget or `Component::build` tracks it reactively:
//!   every shell already polls its *own* theme state once per frame (mirroring
//!   the existing `set_appearance` path) and pushes it through the same two
//!   non-reactive delivery calls (`RenderRoot::set_theme` + a `provide_context`
//!   re-provide) the platform-appearance path already uses. Routing it through
//!   `frust-reactive` would mean a plain `Mutex`-guarded value masquerading
//!   as a signal for no benefit, and it would hand `frust-reactive` a
//!   `frust-theme` dependency it has never needed.
//! - **`frust-shell-common`** already owns the shared, non-FFI plumbing all
//!   three shells compose (`AppTree`, `guard`, `sanitize_scale`) and is the one
//!   crate every shell already imports for exactly this kind of "poll a
//!   process-global once per frame, then push to both theme-delivery paths"
//!   logic — the [`ThemeOverrideWatcher`] here is the per-shell-instance half
//!   of that contract, [`set_app_theme`]/[`clear_app_theme`] the process-global
//!   half. This does add a `frust-theme` dependency to `frust-shell-common`
//!   (previously core+scene+text only) — a deliberate, narrow addition (see
//!   `docs/ARCHITECTURE.md`'s Layer Dependencies), not a general theme-crate
//!   dependency creeping into every layer: `frust-core`/`frust-scene` stay
//!   theme-free.
//!
//! # Thread contract
//!
//! Unlike `push_deep_link`'s UI-thread-only panic contract, this slot is a
//! plain `Mutex<OverrideSlot>` with **no thread restriction** — `set_app_theme`/
//! `clear_app_theme` may be called from any thread (documented, not enforced by
//! a panic): a `Mutex` guards every access, and each shell only *observes* the
//! slot once per frame on its own UI thread via [`ThemeOverrideWatcher::poll`],
//! so a write racing in from a background thread is simply picked up (or not)
//! on the next frame — there is no tracked-signal wake to get racy about, so
//! the stricter `push_deep_link`-style panic-off-thread contract buys nothing
//! here. This is the simpler of the two available contracts.
//!
//! # Override-wins-over-appearance rule
//!
//! Once an app calls [`set_app_theme`], a live platform appearance change
//! (`WindowEvent::ThemeChanged`/`nativeSetAppearance`/`frust_set_appearance`)
//! must NOT flip the active theme's brightness back — the app-forced theme wins
//! entirely until [`clear_app_theme`] runs. [`effective_brightness_for_platform_change`]
//! is the pure, shared decision function every shell's appearance handler calls
//! to implement this rule identically (see its own doc for the two cases).

use std::sync::Mutex;

use frust_theme::{Brightness, Theme};

/// The process-wide override slot: the app's forced [`Theme`] (`None` when no
/// override is active) plus a generation counter bumped on every
/// [`set_app_theme`]/[`clear_app_theme`] call, so a [`ThemeOverrideWatcher`]
/// can tell "changed since I last looked" apart from "still the same value".
struct OverrideSlot {
    theme: Option<Theme>,
    generation: u64,
}

static OVERRIDE: Mutex<OverrideSlot> = Mutex::new(OverrideSlot {
    theme: None,
    generation: 0,
});

/// Force the app's active [`Theme`], overriding whatever the platform's own
/// light/dark preference would otherwise select — reaching BOTH delivery paths
/// (widget paint/layout via `RenderRoot::set_theme`, and `use_context::<Theme>()`
/// via `provide_context`) the next time the running shell polls
/// [`ThemeOverrideWatcher::poll`] (once per frame — see the module docs).
///
/// Callable from any thread (see the module docs' thread contract); the
/// process-wide slot is a plain `Mutex`, not a UI-thread-only primitive.
pub fn set_app_theme(theme: Theme) {
    let mut slot = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
    slot.theme = Some(theme);
    slot.generation += 1;
}

/// Clear a previously-set override, returning to the platform's own
/// light/dark-derived default theme on the next poll (see [`set_app_theme`]).
///
/// A no-op call (no override was ever set) still bumps the generation, so a
/// watcher that polled before any [`set_app_theme`]/[`clear_app_theme`] call
/// and one that polls after a redundant `clear_app_theme` both observe the
/// same "no override" state deterministically rather than depending on
/// whether the slot happened to already be `None`.
pub fn clear_app_theme() {
    let mut slot = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
    slot.theme = None;
    slot.generation += 1;
}

/// Whether an app-forced override is active right now, for a shell's
/// appearance-change handler to consult before applying a platform brightness
/// flip (see [`effective_brightness_for_platform_change`]). Reads the slot
/// directly — unlike [`ThemeOverrideWatcher::poll`], this does not consume or
/// depend on any per-caller "last seen" state.
pub fn theme_override_active() -> bool {
    OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .theme
        .is_some()
}

/// Per-shell-instance watcher over the process-wide override slot: each of the
/// three shells owns one, polling it once per frame (desktop: before rebuild in
/// `RedrawRequested`; mobile: at the top of the frame callback) to detect a
/// [`set_app_theme`]/[`clear_app_theme`] call since the last poll.
#[derive(Debug, Default)]
pub struct ThemeOverrideWatcher {
    /// The slot generation as of the last [`poll`](Self::poll) call. Starts at
    /// `0`, matching the slot's initial generation, so a shell that never
    /// observes a `set_app_theme`/`clear_app_theme` call never sees a change
    /// (no behavior change when the API is never called).
    last_generation: u64,
}

impl ThemeOverrideWatcher {
    /// A fresh watcher, matching the slot's initial (never-overridden) state.
    pub fn new() -> Self {
        Self { last_generation: 0 }
    }

    /// Poll the slot once. Returns:
    /// - `None` — no [`set_app_theme`]/[`clear_app_theme`] call since the last
    ///   poll (or since construction); the shell does nothing.
    /// - `Some(Some(theme))` — a new forced `theme` to push to both delivery
    ///   paths.
    /// - `Some(None)` — the override was cleared; the shell should revert to
    ///   its platform-derived default theme.
    pub fn poll(&mut self) -> Option<Option<Theme>> {
        let slot = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
        if slot.generation == self.last_generation {
            return None;
        }
        self.last_generation = slot.generation;
        Some(slot.theme.clone())
    }
}

/// The override-wins-over-appearance rule (see the module docs), as a pure,
/// shared decision every shell's platform-appearance handler (`WindowEvent::
/// ThemeChanged`/`nativeSetAppearance`/`frust_set_appearance`) calls before
/// mutating its stored theme's brightness:
///
/// - `override_active` (an app called [`set_app_theme`] and has not since
///   called [`clear_app_theme`]): the platform change is ignored entirely —
///   `current` (the override theme's own brightness) passes through unchanged.
/// - Otherwise: `platform` (the newly reported platform preference) wins, the
///   existing pre-override behavior.
pub fn effective_brightness_for_platform_change(
    override_active: bool,
    current: Brightness,
    platform: Brightness,
) -> Brightness {
    if override_active { current } else { platform }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serializes every test in this module against the shared process-wide
    // `OVERRIDE` static — mirrors `frust_reactive`'s `WAKER_TEST_LOCK`
    // pattern for a global the crate under test owns.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Reset the process-wide slot to its pristine (never-overridden) state so
    /// each test starts from a known baseline regardless of execution order.
    fn reset_slot() {
        let mut slot = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
        slot.theme = None;
        slot.generation = 0;
    }

    #[test]
    fn set_app_theme_bumps_generation_and_watcher_observes_it_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = ThemeOverrideWatcher::new();
        // No call yet: a fresh watcher sees no pending change.
        assert_eq!(watcher.poll(), None);

        let theme = Theme::neutral();
        set_app_theme(theme.clone());
        assert!(theme_override_active());

        let observed = watcher.poll();
        assert_eq!(observed, Some(Some(theme)));
        // The same generation is not re-delivered on a second poll.
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn clear_app_theme_delivers_none_and_deactivates() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = ThemeOverrideWatcher::new();
        set_app_theme(Theme::neutral());
        watcher.poll(); // consume the set

        clear_app_theme();
        assert!(!theme_override_active());
        assert_eq!(watcher.poll(), Some(None));
        assert_eq!(watcher.poll(), None);
    }

    #[test]
    fn independent_watchers_each_see_the_change_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut a = ThemeOverrideWatcher::new();
        let mut b = ThemeOverrideWatcher::new();
        let theme = Theme::neutral();
        set_app_theme(theme.clone());

        assert_eq!(a.poll(), Some(Some(theme.clone())));
        assert_eq!(b.poll(), Some(Some(theme)));
        assert_eq!(a.poll(), None);
        assert_eq!(b.poll(), None);
    }

    #[test]
    fn never_calling_the_api_leaves_a_fresh_watcher_silent() {
        // Acceptance criterion 2: no behavior change when the API is never
        // called. A watcher that never sees a set/clear call must never report
        // a pending change, regardless of what earlier tests left in the slot
        // (reset to the pristine state here, then only ever polled).
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let mut watcher = ThemeOverrideWatcher::new();
        assert_eq!(watcher.poll(), None);
        assert_eq!(watcher.poll(), None);
        assert!(!theme_override_active());
    }

    #[test]
    fn effective_brightness_respects_override_wins_rule() {
        // No override: the platform's newly reported preference wins (the
        // pre-override behavior).
        assert_eq!(
            effective_brightness_for_platform_change(false, Brightness::Light, Brightness::Dark),
            Brightness::Dark
        );
        // Override active: the platform change is ignored; the override
        // theme's own current brightness passes through unchanged.
        assert_eq!(
            effective_brightness_for_platform_change(true, Brightness::Light, Brightness::Dark),
            Brightness::Light
        );
    }
}
