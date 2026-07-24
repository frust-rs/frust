//! App-facing translucent-surface opt-in slot (task 03):
//! [`request_translucent_surface`]/[`SurfaceModeWatcher::current`].
//!
//! # The gap this closes
//!
//! Platform-view compositing (Mode B — see
//! `workflow/plans/features/frust-platform-views/PLAN.md`) needs the GPU
//! surface itself to be created with an alpha channel (Android's `EGLConfig`,
//! iOS's `CAMetalLayer.isOpaque`) so a native sibling view placed *behind* it
//! can show through wherever frust paints nothing. That surface-format choice
//! happens once, at surface-creation time, well before any app code runs — so
//! there is no "widget asks for translucency" moment the way there is for,
//! say, `set_app_theme`. An app (or the platform-views facade glue, task 07)
//! instead calls [`request_translucent_surface`] during startup, and each
//! shell's surface-creation path (task 04) reads
//! [`SurfaceModeWatcher::current`] **before** configuring the surface.
//!
//! # Layering choice
//!
//! Same rationale as [`crate::theme_override`]/[`crate::system_ui`]: a
//! process-global `Mutex` slot living in `frust-shell-common`, the crate every
//! shell already polls this kind of state from. Unlike those two, though,
//! this slot is **not** a per-frame generation/poll pair — see the Latch
//! contract below — so [`SurfaceModeWatcher`] carries no per-instance
//! "last seen" cursor; `current` is an associated function, a plain peek at
//! the process-wide slot.
//!
//! # Latch contract (one-way, v1)
//!
//! [`request_translucent_surface`] only ever moves the slot from
//! [`SurfaceMode::Opaque`] to [`SurfaceMode::Translucent`] — there is no
//! `request_opaque_surface`/"undo" call, and once observed as
//! `Translucent` it never reverts. This is deliberate, not an oversight: the
//! surface format is fixed at creation (the platform APIs above expose no
//! supported runtime toggle), so "reverting" would mean destroying and
//! recreating the whole surface — out of scope for v1, and nothing in the
//! plan needs it (an app either wants platform-view compositing for the
//! process's lifetime, or it doesn't). A future version needing a live flip
//! would have to plumb a full surface-recreation round-trip through each
//! shell's `SurfacePhase` state machine (`docs/ARCHITECTURE.md`'s frame
//! pipeline) — not a slot-shape change.
//!
//! # Thread contract
//!
//! Like [`crate::theme_override::set_app_theme`]/
//! [`crate::system_ui::set_system_ui_mode`], [`request_translucent_surface`]
//! is callable from any thread — a plain `Mutex` guards the slot. In
//! practice it must be called before the shell's surface-creation path reads
//! [`SurfaceModeWatcher::current`] (startup-time only — see the module docs
//! above), but that ordering is a caller responsibility, not something this
//! module enforces.

use std::sync::Mutex;

/// Whether a shell's GPU surface should be created with an alpha channel.
/// See the module docs' Latch contract — this only ever moves
/// `Opaque` → `Translucent`, never back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceMode {
    /// Default: an opaque surface, matching every shell's pre-platform-views
    /// behavior.
    #[default]
    Opaque,
    /// Create the surface with an alpha channel so a native sibling view
    /// placed behind it can show through unpainted regions.
    Translucent,
}

/// The process-wide latch. No generation counter (unlike
/// [`crate::theme_override`]/[`crate::system_ui`]'s slots) — see the module
/// docs' Latch contract for why a per-frame "changed since last poll" concept
/// doesn't apply here.
static SURFACE_MODE: Mutex<SurfaceMode> = Mutex::new(SurfaceMode::Opaque);

/// Request a translucent (alpha-channel) GPU surface. Callable from any
/// thread (see the module docs' Thread contract), and idempotent — calling
/// it more than once, or after the surface already latched translucent, has
/// no additional effect.
///
/// Must be called before the running shell's surface-creation path reads
/// [`SurfaceModeWatcher::current`] (see the module docs) — calling it after
/// the surface already exists has no effect on that surface.
pub fn request_translucent_surface() {
    let mut slot = SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner());
    *slot = SurfaceMode::Translucent;
}

/// Per-shell-instance reader over the process-wide latch. Kept as a type
/// (mirroring [`crate::theme_override::ThemeOverrideWatcher`]/
/// [`crate::system_ui::SystemUiWatcher`]'s shape) even though it carries no
/// state of its own — [`current`](Self::current) is a plain peek, not a
/// diffed poll, per the module docs' Latch contract.
#[derive(Debug, Default)]
pub struct SurfaceModeWatcher;

impl SurfaceModeWatcher {
    /// A fresh (stateless) watcher.
    pub fn new() -> Self {
        Self
    }

    /// Read the latch's current value. Not a "since last call" diff — a
    /// shell's surface-creation path calls this once, at surface-creation
    /// time, and applies whatever it reads.
    pub fn current() -> SurfaceMode {
        *SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;

    // Serializes every test in this module against the shared process-wide
    // `SURFACE_MODE` static — mirrors `theme_override`'s `TEST_LOCK` pattern.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn reset_slot() {
        let mut slot = SURFACE_MODE.lock().unwrap_or_else(|e| e.into_inner());
        *slot = SurfaceMode::Opaque;
    }

    #[test]
    fn defaults_to_opaque() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Opaque);
    }

    #[test]
    fn request_latches_translucent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        request_translucent_surface();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }

    #[test]
    fn repeated_requests_are_idempotent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        request_translucent_surface();
        request_translucent_surface();
        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }

    #[test]
    fn request_from_a_spawned_thread_is_observed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot();

        thread::spawn(|| {
            request_translucent_surface();
        })
        .join()
        .unwrap();

        assert_eq!(SurfaceModeWatcher::current(), SurfaceMode::Translucent);
    }
}
