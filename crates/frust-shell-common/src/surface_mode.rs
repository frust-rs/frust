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
//! # This latch is the REQUEST, not the outcome
//!
//! What an app asks for here is not necessarily what it gets. Each shell
//! translates this latch into a `frust_render::SurfaceAlphaRequest`, and
//! `frust-render` resolves *that* against the platform's advertised
//! `CompositeAlphaMode`s at configure time — falling back to an opaque
//! swapchain (with a `log::warn!`) when the platform advertises no translucent
//! mode. **The resolved truth lives at a different seam**:
//! `frust_render::SurfaceRenderer::surface_resolved_translucent`, read by each
//! shell after every surface (re)install and threaded into
//! `RenderRoot::set_surface_translucent` (review finding M1).
//!
//! So: read this latch to decide what to *request*; never to decide whether to
//! paint the Mode B contract (a transparent base clear, a `platform_view`
//! hole punch). Keying paint off the request means a fallback clears to
//! `TRANSPARENT` and `DestOut`-punches every slot rect on an OPAQUE
//! swapchain — black rectangles instead of a graceful degrade to Mode A.
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
//! One-way applies to the REQUEST only. The *resolved* state above is not
//! one-way and is not fixed before the surface exists: every (re)install
//! re-resolves it, and a failed install clears it — which is exactly why the
//! shells re-read it per frame rather than caching it at construction.
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
///
/// Requesting translucency does not guarantee it: the platform may refuse
/// (see the module docs' *This latch is the REQUEST, not the outcome*), in
/// which case the app degrades to the opaque Mode A contract.
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
