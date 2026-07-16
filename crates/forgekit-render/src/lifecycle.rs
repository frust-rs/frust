//! Surface lifecycle state machine (spec §8.1).
//!
//! Android destroys and recreates the GPU surface on rotation and
//! backgrounding — `wgpu` surfaces raise `ERROR_SURFACE_LOST_KHR` and panic on
//! `configure`-after-resume if this is handled ad hoc. ForgeKit therefore makes
//! surface state a first-class machine in [`crate::SurfaceRenderer`], driven by
//! the platform shells' `surfaceCreated`/`surfaceChanged`/`surfaceDestroyed`
//! callbacks (Android) and `resumed`/`Resized`/`suspended` events (desktop).
//!
//! # States
//!
//! ```text
//!            on_surface_created                 acquire == Lost
//!   NoSurface ───────────────▶ SurfaceReady ──────────────────▶ SurfaceLost
//!       ▲   on_surface_destroyed  │  ▲  on_surface_changed          │
//!       └───────────────────────┘  └── (resize, stays Ready)       │
//!       ▲                                on_surface_destroyed       │
//!       └──────────────────────────────────────────────────────────┘
//!                              on_surface_created (recreate)
//! ```
//!
//! # Invariants (enforced by construction)
//!
//! - No `SurfaceTexture` outlives a surface transition: [`crate::SurfaceRenderer::render`]
//!   acquires and presents within a single call and stores nothing.
//! - `Surface::configure` runs only from [`SurfacePhase::SurfaceReady`] (on entry
//!   via `on_surface_created`/`on_surface_changed`, or on an `Outdated` acquire).
//! - Frame requests in [`SurfacePhase::NoSurface`]/[`SurfacePhase::SurfaceLost`]
//!   are dropped — [`crate::SurfaceRenderer::render`] returns
//!   [`FrameOutcome::Skipped`], never panicking and never queueing.
//!
//! The pure decision logic ([`next_phase`], [`SurfacePhase::can_render`],
//! [`decide_acquire`]) is separated from the `wgpu` calls so it is unit-testable
//! without a GPU.

use core::ffi::c_void;
use core::ptr::NonNull;

use anyhow::{Result, anyhow};

/// Which lifecycle state the surface is in (spec §8.1).
///
/// Rendering only happens in [`SurfacePhase::SurfaceReady`]; the other two
/// phases mean there is no usable swapchain and frames are skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfacePhase {
    /// No surface has been created yet, or the surface was explicitly destroyed.
    NoSurface,
    /// A configured surface is available and frames can be rendered.
    SurfaceReady,
    /// The surface was lost mid-render (e.g. `ERROR_SURFACE_LOST_KHR`); it has
    /// been dropped and the shell must recreate it via `on_surface_created`.
    SurfaceLost,
}

impl SurfacePhase {
    /// Whether a frame may be rendered in this phase.
    ///
    /// Only [`SurfacePhase::SurfaceReady`] can render; the machine drops frames
    /// in every other phase rather than queueing them (spec §8.1).
    pub(crate) fn can_render(self) -> bool {
        matches!(self, SurfacePhase::SurfaceReady)
    }
}

/// A platform lifecycle event that drives a phase transition.
///
/// Kept separate from the `wgpu` side so the transition table is pure and
/// unit-testable ([`next_phase`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceEvent {
    /// `surfaceCreated` (Android) / `resumed` (desktop): a surface is available.
    Created,
    /// `surfaceDestroyed` (Android) / `suspended` (desktop): the surface is gone.
    Destroyed,
    /// The swapchain reported `Lost` on acquire, mid-render.
    Lost,
}

/// Pure phase-transition table (spec §8.1).
///
/// Transitions are total by design: `Created` always lands in `SurfaceReady`
/// (creating or recreating), `Destroyed` always in `NoSurface`, and `Lost`
/// always in `SurfaceLost`. `Lost` is only ever emitted from `SurfaceReady`
/// (it originates in `render`), so no illegal edge is reachable in practice.
pub(crate) fn next_phase(_current: SurfacePhase, event: SurfaceEvent) -> SurfacePhase {
    match event {
        SurfaceEvent::Created => SurfacePhase::SurfaceReady,
        SurfaceEvent::Destroyed => SurfacePhase::NoSurface,
        SurfaceEvent::Lost => SurfacePhase::SurfaceLost,
    }
}

/// `wgpu`-independent classification of a swapchain-acquire attempt.
///
/// [`crate::SurfaceRenderer::render`] maps `wgpu::CurrentSurfaceTexture` onto
/// this so the acquire policy ([`decide_acquire`]) stays pure and testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcquireStatus {
    /// `Success`/`Suboptimal`: a texture is available to present.
    Usable,
    /// `Outdated`: the swapchain is stale and must be reconfigured.
    Outdated,
    /// `Lost`: the surface is gone; drop it and move to `SurfaceLost`.
    Lost,
    /// `Timeout`/`Occluded`: transient; skip this frame.
    Transient,
    /// `Validation`: the swapchain raised a validation error on acquire (observed
    /// under the Android emulator's SwiftShader driver as a one-off hiccup).
    /// Treated as recoverable — reconfigure the surface and retry — rather than
    /// a hard failure that would wedge the app, since the uncaptured-error
    /// handler (see [`crate::context`]) already prevents the process-killing
    /// panic wgpu would otherwise raise.
    Invalid,
}

/// What [`crate::SurfaceRenderer::render`] should do after an acquire attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcquireAction {
    /// Blit and present the acquired texture; report [`FrameOutcome::Rendered`].
    Present,
    /// Reconfigure the surface and ask the shell to redraw
    /// ([`FrameOutcome::Redraw`]) — fixes the Phase-0/1 deferred bug where the
    /// `Outdated` path never requested a redraw.
    Reconfigure,
    /// Drop the surface, transition to `SurfaceLost`, report
    /// [`FrameOutcome::SurfaceLost`] so the shell can recreate it.
    Lose,
    /// Skip this frame; report [`FrameOutcome::Skipped`].
    Skip,
}

/// Cap on consecutive `Invalid`-acquire reconfigure retries before the machine
/// gives up and transitions to [`SurfacePhase::SurfaceLost`].
///
/// Mirrors `forgekit-shell-ios::ffi_support::MAX_RECREATE_ATTEMPTS`'s style and
/// rationale (same spec §8.1 phase, applied one step earlier): without a cap, a
/// persistently-`Invalid` swapchain would retry a reconfigure every frame,
/// forever. At the cap the existing per-shell `SurfaceLost` recovery paths take
/// over instead — desktop recreates on the next resize/redraw, iOS's own
/// `recover_surface` (capped separately), Android on the next `surfaceChanged`
/// — so no shell code needs to change for this to be honoured.
pub(crate) const MAX_INVALID_RECONFIGURES: u8 = 3;

/// Pure acquire-error policy (spec §8.1).
///
/// `consecutive_invalid` is the number of `Invalid` acquires already retried
/// (via `Reconfigure`) since the last successful acquire; only the `Invalid`
/// arm consults it. Host-testable: no `wgpu` state is touched here, only the
/// counter [`crate::SurfaceRenderer::render`] tracks and resets on a
/// successful acquire.
pub(crate) fn decide_acquire(status: AcquireStatus, consecutive_invalid: u8) -> AcquireAction {
    match status {
        AcquireStatus::Usable => AcquireAction::Present,
        AcquireStatus::Outdated => AcquireAction::Reconfigure,
        AcquireStatus::Lost => AcquireAction::Lose,
        AcquireStatus::Transient => AcquireAction::Skip,
        // A validation error on acquire is treated like `Outdated`: rebuild the
        // swapchain and ask for a redraw. Recovering (rather than `Fail`ing)
        // keeps a transient driver hiccup from permanently freezing the surface.
        // But only up to `MAX_INVALID_RECONFIGURES` consecutive attempts — past
        // that this is no longer a one-off hiccup, and retrying forever would
        // spin the render loop on a permanently-broken swapchain. At the cap,
        // give up and drop to `SurfaceLost` like a genuine `Lost` acquire, so
        // the shell's existing recovery paths take over.
        AcquireStatus::Invalid => {
            if consecutive_invalid < MAX_INVALID_RECONFIGURES {
                AcquireAction::Reconfigure
            } else {
                AcquireAction::Lose
            }
        }
    }
}

/// Given the just-observed acquire `status` and the [`AcquireAction`]
/// [`decide_acquire`] chose for it, returns the next consecutive-`Invalid`
/// streak count [`crate::SurfaceRenderer`] should store.
///
/// Pure and host-testable, split out of [`crate::SurfaceRenderer::render`] so
/// the counter's reset-on-success / increment-on-retry / reset-on-give-up
/// discipline is unit-tested without a GPU:
/// - a successful acquire (`Usable`) always resets the streak to zero — the
///   next `Invalid`, if any, starts a fresh episode with a full retry budget;
/// - a retried `Invalid` (`Reconfigure`) increments the streak;
/// - a given-up `Invalid` (`Lose`, i.e. the cap was hit) resets to zero — the
///   giving-up transition itself ends the episode;
/// - every other status/action pairing leaves the streak untouched.
pub(crate) fn next_invalid_streak(status: AcquireStatus, action: AcquireAction, current: u8) -> u8 {
    match (status, action) {
        (AcquireStatus::Usable, _) => 0,
        (AcquireStatus::Invalid, AcquireAction::Reconfigure) => current.saturating_add(1),
        (AcquireStatus::Invalid, AcquireAction::Lose) => 0,
        _ => current,
    }
}

/// The outcome of a single [`crate::SurfaceRenderer::render`] call.
///
/// Lets the shell react without knowing the internal state: request another
/// redraw when the surface was reconfigured, recreate the surface when it was
/// lost, or do nothing when a frame was drawn or skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// A frame was encoded and presented to the swapchain.
    Rendered,
    /// No renderable surface (`NoSurface`/`SurfaceLost`) or a transient acquire
    /// failure: nothing was drawn and nothing was queued.
    Skipped,
    /// The surface was stale and has been reconfigured; the shell should request
    /// another redraw to draw against the fresh configuration.
    Redraw,
    /// The surface was lost and has been dropped; the machine is now in
    /// [`SurfacePhase::SurfaceLost`] and the shell must recreate the surface via
    /// `on_surface_created`.
    SurfaceLost,
}

/// Builds a `wgpu::Surface` from a raw `ANativeWindow` pointer.
///
/// This is one of the framework's sanctioned `unsafe` boundaries: turning a
/// caller-owned raw pointer into a GPU surface. It is deliberately isolated in
/// one function so the safety contract lives in exactly one place. See also
/// [`create_metal_surface`] (the iOS/macOS counterpart) and
/// `forgekit-shell-android`'s `jni_glue` module.
///
/// # Safety
///
/// `window_ptr` must be a valid, non-null `ANativeWindow*` that the caller has
/// **acquired** (e.g. via `ndk::NativeWindow` / `ANativeWindow_acquire`, which
/// the Android shell owns) and that **outlives** the returned [`wgpu::Surface`]
/// and every `SurfaceTexture` acquired from it. The caller must drop the
/// surface (and any outstanding textures) before releasing the window, i.e.
/// before returning from the `surfaceDestroyed` callback.
///
/// Compiled unconditionally (the raw-handle types are host-available) so a host
/// `cargo check --target aarch64-linux-android` exercises it.
pub(crate) unsafe fn create_android_surface(
    instance: &wgpu::Instance,
    window_ptr: *mut c_void,
) -> Result<wgpu::Surface<'static>> {
    use wgpu::rwh::{
        AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
    };

    let window = NonNull::new(window_ptr)
        .ok_or_else(|| anyhow!("forgekit-render: null ANativeWindow pointer"))?;
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(RawDisplayHandle::Android(AndroidDisplayHandle::new())),
        raw_window_handle: RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window)),
    };

    // SAFETY: upheld by this function's own safety contract — `window_ptr` is a
    // valid, acquired `ANativeWindow*` that outlives the returned surface.
    let surface = unsafe { instance.create_surface_unsafe(target) }
        .map_err(|e| anyhow!("forgekit-render: failed to create Android surface: {e}"))?;
    Ok(surface)
}

/// Builds a `wgpu::Surface` from a raw `CAMetalLayer*` pointer.
///
/// This is one of the framework's sanctioned `unsafe` boundaries (see
/// [`create_android_surface`] for the sibling Android path): turning a
/// caller-owned raw pointer into a GPU surface. It is deliberately isolated in
/// one function so the safety contract lives in exactly one place.
///
/// # Safety
///
/// `layer_ptr` must be a valid, live `CAMetalLayer*` that the caller (the
/// Swift shell) owns and that **outlives** the returned [`wgpu::Surface`] and
/// every `SurfaceTexture` acquired from it — enforced by the
/// `forgekit_destroy`-before-view-teardown ordering in the generated app. The
/// caller must drop the surface (and any outstanding textures) before the
/// layer/view is torn down.
///
/// Only compiled on Apple targets: `wgpu::SurfaceTargetUnsafe::CoreAnimationLayer`
/// is itself Metal-feature-gated in `wgpu` (available whenever `target_vendor
/// = "apple"`), so this function is gated the same way rather than being
/// compiled unconditionally like [`create_android_surface`] (whose raw-handle
/// types are host-available on every platform).
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) unsafe fn create_metal_surface(
    instance: &wgpu::Instance,
    layer_ptr: *mut c_void,
) -> Result<wgpu::Surface<'static>> {
    if layer_ptr.is_null() {
        return Err(anyhow!("forgekit-render: null CAMetalLayer pointer"));
    }
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer_ptr);

    // SAFETY: upheld by this function's own safety contract — `layer_ptr` is a
    // valid, live `CAMetalLayer*` that outlives the returned surface.
    let surface = unsafe { instance.create_surface_unsafe(target) }
        .map_err(|e| anyhow!("forgekit-render: failed to create Metal surface: {e}"))?;
    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_always_lands_in_surface_ready() {
        for phase in [
            SurfacePhase::NoSurface,
            SurfacePhase::SurfaceReady,
            SurfacePhase::SurfaceLost,
        ] {
            assert_eq!(
                next_phase(phase, SurfaceEvent::Created),
                SurfacePhase::SurfaceReady,
                "creating (or recreating) a surface must reach SurfaceReady from {phase:?}"
            );
        }
    }

    #[test]
    fn destroyed_always_lands_in_no_surface() {
        for phase in [
            SurfacePhase::NoSurface,
            SurfacePhase::SurfaceReady,
            SurfacePhase::SurfaceLost,
        ] {
            assert_eq!(
                next_phase(phase, SurfaceEvent::Destroyed),
                SurfacePhase::NoSurface,
                "destroying a surface must reach NoSurface from {phase:?}"
            );
        }
    }

    #[test]
    fn lost_transitions_ready_to_surface_lost() {
        assert_eq!(
            next_phase(SurfacePhase::SurfaceReady, SurfaceEvent::Lost),
            SurfacePhase::SurfaceLost
        );
    }

    #[test]
    fn only_surface_ready_can_render() {
        assert!(SurfacePhase::SurfaceReady.can_render());
        assert!(!SurfacePhase::NoSurface.can_render());
        assert!(!SurfacePhase::SurfaceLost.can_render());
    }

    #[test]
    fn acquire_policy_matches_spec_8_1() {
        assert_eq!(
            decide_acquire(AcquireStatus::Usable, 0),
            AcquireAction::Present
        );
        assert_eq!(
            decide_acquire(AcquireStatus::Outdated, 0),
            AcquireAction::Reconfigure
        );
        assert_eq!(decide_acquire(AcquireStatus::Lost, 0), AcquireAction::Lose);
        assert_eq!(
            decide_acquire(AcquireStatus::Transient, 0),
            AcquireAction::Skip
        );
        // A validation error on acquire recovers by reconfiguring the surface,
        // not by failing — a transient SwiftShader hiccup must not wedge the app.
        assert_eq!(
            decide_acquire(AcquireStatus::Invalid, 0),
            AcquireAction::Reconfigure
        );
    }

    #[test]
    fn invalid_reconfigures_under_the_cap() {
        // Every count below the cap still reconfigures — mirrors the iOS
        // `MAX_RECREATE_ATTEMPTS` "under cap" behavior at the analogous phase.
        for consecutive_invalid in 0..MAX_INVALID_RECONFIGURES {
            assert_eq!(
                decide_acquire(AcquireStatus::Invalid, consecutive_invalid),
                AcquireAction::Reconfigure,
                "expected Reconfigure at consecutive_invalid={consecutive_invalid}"
            );
        }
    }

    #[test]
    fn invalid_gives_up_at_the_cap() {
        assert_eq!(
            decide_acquire(AcquireStatus::Invalid, MAX_INVALID_RECONFIGURES),
            AcquireAction::Lose,
            "at the cap the machine must give up and drop to SurfaceLost"
        );
        // Saturating past the cap stays given-up (never re-enters Reconfigure).
        assert_eq!(
            decide_acquire(AcquireStatus::Invalid, u8::MAX),
            AcquireAction::Lose
        );
    }

    #[test]
    fn invalid_streak_resets_on_successful_acquire() {
        assert_eq!(
            next_invalid_streak(AcquireStatus::Usable, AcquireAction::Present, 2),
            0
        );
        // Even a streak already at (or past) the cap resets on success.
        assert_eq!(
            next_invalid_streak(AcquireStatus::Usable, AcquireAction::Present, u8::MAX),
            0
        );
    }

    #[test]
    fn invalid_streak_increments_while_reconfiguring() {
        assert_eq!(
            next_invalid_streak(AcquireStatus::Invalid, AcquireAction::Reconfigure, 0),
            1
        );
        assert_eq!(
            next_invalid_streak(
                AcquireStatus::Invalid,
                AcquireAction::Reconfigure,
                MAX_INVALID_RECONFIGURES - 1
            ),
            MAX_INVALID_RECONFIGURES
        );
        // Saturates rather than overflowing.
        assert_eq!(
            next_invalid_streak(AcquireStatus::Invalid, AcquireAction::Reconfigure, u8::MAX),
            u8::MAX
        );
    }

    #[test]
    fn invalid_streak_resets_on_giving_up() {
        assert_eq!(
            next_invalid_streak(
                AcquireStatus::Invalid,
                AcquireAction::Lose,
                MAX_INVALID_RECONFIGURES
            ),
            0
        );
    }

    #[test]
    fn invalid_streak_untouched_by_unrelated_status_action_pairs() {
        assert_eq!(
            next_invalid_streak(AcquireStatus::Outdated, AcquireAction::Reconfigure, 2),
            2
        );
        assert_eq!(
            next_invalid_streak(AcquireStatus::Lost, AcquireAction::Lose, 2),
            2
        );
        assert_eq!(
            next_invalid_streak(AcquireStatus::Transient, AcquireAction::Skip, 2),
            2
        );
    }

    #[test]
    fn non_invalid_statuses_are_unaffected_by_the_counter() {
        // The counter only matters for `Invalid`; every other status ignores it.
        for consecutive_invalid in [0, 1, MAX_INVALID_RECONFIGURES, u8::MAX] {
            assert_eq!(
                decide_acquire(AcquireStatus::Usable, consecutive_invalid),
                AcquireAction::Present
            );
            assert_eq!(
                decide_acquire(AcquireStatus::Outdated, consecutive_invalid),
                AcquireAction::Reconfigure
            );
            assert_eq!(
                decide_acquire(AcquireStatus::Lost, consecutive_invalid),
                AcquireAction::Lose
            );
            assert_eq!(
                decide_acquire(AcquireStatus::Transient, consecutive_invalid),
                AcquireAction::Skip
            );
        }
    }
}
