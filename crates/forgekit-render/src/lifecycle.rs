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
    /// `Validation`: a hard error the caller cannot recover from.
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
    /// Surface a hard error to the caller.
    Fail,
}

/// Pure acquire-error policy (spec §8.1).
pub(crate) fn decide_acquire(status: AcquireStatus) -> AcquireAction {
    match status {
        AcquireStatus::Usable => AcquireAction::Present,
        AcquireStatus::Outdated => AcquireAction::Reconfigure,
        AcquireStatus::Lost => AcquireAction::Lose,
        AcquireStatus::Transient => AcquireAction::Skip,
        AcquireStatus::Invalid => AcquireAction::Fail,
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
            decide_acquire(AcquireStatus::Usable),
            AcquireAction::Present
        );
        assert_eq!(
            decide_acquire(AcquireStatus::Outdated),
            AcquireAction::Reconfigure
        );
        assert_eq!(decide_acquire(AcquireStatus::Lost), AcquireAction::Lose);
        assert_eq!(
            decide_acquire(AcquireStatus::Transient),
            AcquireAction::Skip
        );
        assert_eq!(decide_acquire(AcquireStatus::Invalid), AcquireAction::Fail);
    }
}
