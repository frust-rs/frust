//! Design-system-facing default-theme seed seam:
//! `set_default_theme`/`default_theme`.
//!
//! # The gap this closes
//!
//! Every shell seeds itself with a built-in fallback theme
//! (`Theme::glyph_baseline()`) that a design-system plugin has no way to
//! replace — the plugin can only reach for [`crate::theme_override::set_app_theme`],
//! but that seam **forces** the active theme end-to-end, pinning it against a
//! live platform appearance change until `clear_app_theme` runs (see
//! [`crate::theme_override::effective_brightness_for_platform_change`]). A
//! Glyph-themed app installed that way would silently stop honouring system
//! dark mode — wrong for a design system that only wants to supply the app's
//! *starting point*, not commandeer its appearance forever. This module is
//! the narrower seam that closes that gap: it supplies the **base** theme a
//! shell seeds itself with in place of its own built-in fallback, while
//! leaving brightness free to keep following the platform.
//!
//! # Precedence
//!
//! A shell resolves the active theme in this order, highest wins:
//!
//! 1. [`crate::theme_override::set_app_theme`] — an app-forced override, if
//!    one is active. Brightness is pinned; see that module's
//!    override-wins-over-appearance rule.
//! 2. [`set_default_theme`] — the design-system-supplied base, if one was
//!    seeded. Brightness is **not** pinned: the shell's base theme object
//!    re-derives light/dark from the platform's own appearance against this
//!    same base (why [`default_theme`] is a non-destructive read — see
//!    below).
//! 3. The shell's own built-in fallback (`Theme::glyph_baseline()` today),
//!    when neither of the above was ever set.
//!
//! # Layering choice
//!
//! This process-global slot lives in `frust-shell-common`, mirroring
//! [`crate::theme_override`]'s slot shape (see that module's doc comment for
//! the fuller layering rationale, which applies here unchanged):
//! `frust-shell-common` already owns the shared, non-FFI "poll a
//! process-global once per frame" plumbing every shell composes around, and
//! already depends on `frust-theme` for the [`Theme`] type this slot holds.
//!
//! # Thread contract
//!
//! Like `theme_override` and `font_registry`, this is a plain `Mutex`-guarded
//! slot with **no thread restriction** — [`set_default_theme`] may be called
//! from any thread (documented, not enforced by a panic): a `Mutex` guards
//! every access, and a shell only *reads* the slot at construction time and
//! when `clear_app_theme` is called (to revert to the base seeded here), so a
//! write racing in from a background thread is simply picked up (or not) on
//! the next such read.
//!
//! # Non-destructive read
//!
//! Unlike [`crate::font_registry`]'s drain-on-poll shape,
//! [`default_theme`] does **not** consume the slot: a shell needs the same
//! seeded base again every time it re-seeds itself (when `clear_app_theme` is
//! called to revert from an app override), not just once at construction. Two
//! consecutive calls to [`default_theme`] with no intervening
//! [`set_default_theme`] call return the same value.
//!
//! # Timing
//!
//! Intended to be called before a shell's first frame — typically from a
//! design-system plugin's `install()`, which runs during app construction.
//! A call *after* the first frame takes effect only on the next
//! `clear_app_theme`-driven reseed, which may never happen if no app override
//! is ever set. Late calls are supported but carry this limitation: a plugin
//! cannot dynamically re-theme a live app by calling this at runtime.

use std::sync::Mutex;

use frust_theme::Theme;

/// The process-wide default-theme slot: the design-system-supplied base
/// [`Theme`] (`None` when no default has ever been seeded) plus a generation
/// counter bumped on every [`set_default_theme`] call — mirrors
/// [`crate::theme_override`]'s `OverrideSlot` shape.
struct DefaultSlot {
    theme: Option<Theme>,
    #[allow(dead_code)]
    generation: u64,
}

static DEFAULT: Mutex<DefaultSlot> = Mutex::new(DefaultSlot {
    theme: None,
    generation: 0,
});

/// Supply the base theme a shell seeds itself with, in place of its built-in
/// fallback. Call before the first frame — typically from a design-system
/// plugin's `install()`.
///
/// Unlike [`crate::theme_override::set_app_theme`], this does NOT pin
/// brightness: the shell's retained theme object continues to re-derive
/// light/dark from the platform's appearance against this same base (see the
/// module docs' Precedence section). A late call (after the first frame) takes
/// effect only if the app later calls `clear_app_theme`; until then, any
/// active override dominates.
///
/// Callable from any thread (see the module docs' thread contract); the
/// process-wide slot is a plain `Mutex`, not a UI-thread-only primitive.
pub fn set_default_theme(theme: Theme) {
    let mut slot = DEFAULT.lock().unwrap_or_else(|e| e.into_inner());
    slot.theme = Some(theme);
    slot.generation += 1;
}

/// Read the seeded default, if any (`None` when [`set_default_theme`] has
/// never been called). **Non-destructive** — a shell may need it again when
/// reverting an app override via `clear_app_theme`, to re-seed the base (see
/// the module docs' Non-destructive read section); unlike
/// [`crate::font_registry::FontRegistryWatcher::poll`], repeated calls with
/// no intervening [`set_default_theme`] all return the same value rather than
/// draining the slot.
pub fn default_theme() -> Option<Theme> {
    DEFAULT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .theme
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serializes every test in this module against the shared process-wide
    // `DEFAULT` static — mirrors `theme_override`'s `TEST_LOCK` pattern for a
    // global the crate under test owns.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Reset the process-wide slot to its pristine (never-seeded) state so
    /// each test starts from a known baseline regardless of execution order.
    fn reset_slot() {
        let mut slot = DEFAULT.lock().unwrap_or_else(|e| e.into_inner());
        slot.theme = None;
        slot.generation = 0;
    }

    #[test]
    fn default_theme_is_none_before_any_set() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        assert_eq!(default_theme(), None);
    }

    #[test]
    fn default_theme_read_is_non_destructive() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let theme = Theme::glyph_baseline();
        set_default_theme(theme.clone());

        // Two (in fact three) consecutive reads all return the same value —
        // no drain-on-read behavior like `font_registry`'s watcher.
        assert_eq!(default_theme(), Some(theme.clone()));
        assert_eq!(default_theme(), Some(theme.clone()));
        assert_eq!(default_theme(), Some(theme));
    }

    #[test]
    fn set_default_theme_replaces_a_previous_default() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        set_default_theme(Theme::m3_baseline());
        assert_eq!(default_theme(), Some(Theme::m3_baseline()));

        let cupertino = Theme::cupertino_baseline();
        set_default_theme(cupertino.clone());
        assert_eq!(default_theme(), Some(cupertino));
    }

    #[test]
    fn set_default_theme_from_a_spawned_thread_is_observed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        let theme = Theme::glyph_baseline();
        let handle = std::thread::spawn({
            let theme = theme.clone();
            move || set_default_theme(theme)
        });
        handle.join().expect("spawned thread must not panic");

        assert_eq!(default_theme(), Some(theme));
    }
}
