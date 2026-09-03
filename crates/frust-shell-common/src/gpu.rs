//! The process-wide GPU-device slot: [`install_gpu_handle`]/[`gpu_handle`].
//!
//! # The gap this closes
//!
//! Every shell creates its GPU device lazily, on its own thread, the first
//! time a surface comes up (`frust_render::RenderContext`, owned wholly by
//! the shell's render executor — the desktop split moves it onto a dedicated
//! render thread, the mobile shells' equivalent). Nothing before this module
//! ever handed that device back out: an app compiled with the facade's `gpu`
//! feature had no way to reach the *live*, shell-owned device — only to build
//! a throwaway standalone one of its own (`frust::gpu::Context::new`).
//!
//! This module is that hand-back seam's shared slot: a shell installs its
//! device once, from wherever it creates one, and app code — through the
//! facade's `frust::gpu::with_context` — reads it back without the facade
//! ever depending on `frust-gpu`/`frust-render` for this seam's own sake (it
//! already carries a separate, narrower, optional edge for the standalone
//! `Context` — see `crates/frust/src/lib.rs`'s `gpu` module docs).
//!
//! # Type-erased on purpose — this crate stays a platform-free leaf
//!
//! `frust-shell-common` depends on none of `frust-gpu`/`frust-render`/`wgpu`
//! (see the crate's own docs' "deliberately platform-free" paragraph) and
//! this seam adds no edge to change that: the slot stores an opaque
//! `Box<dyn Any + Send + Sync>` rather than naming the concrete device type
//! (`frust_render::DeviceHandle`, cheap to clone — see its own doc comment).
//! Every real caller — [`install_gpu_handle`] from a shell's render executor,
//! [`gpu_handle`] from the facade's `with_context` — instantiates the generic
//! with that one concrete type, so the erasure is invisible in practice; it
//! only exists so this crate never has to name a GPU type to host the slot.
//! Reading with a mismatched `T` (a caller error, never a real code path) is
//! not a panic — it answers `None`, exactly like reading before anything
//! installed.
//!
//! # Layering and thread contract
//!
//! One `OnceLock`, unlike the `Mutex`-backed slots in
//! [`crate::theme_override`]/[`crate::surface_mode`]/[`crate::system_ui`]:
//! those slots change during a running app's lifetime (a theme override, a
//! resolved surface mode), so they need a lock a later write can take again.
//! A GPU device does not — once a shell's device exists it lives for the rest
//! of the process (`frust_gpu::context::RenderContext` reuses it across
//! surface loss/recreation), so this slot is **install-once**:
//! [`install_gpu_handle`] only ever moves it from empty to occupied, is a
//! no-op (returning `false`) on every call after the first, and there is no
//! corresponding clear. That also means [`gpu_handle`] never blocks: once
//! occupied, a read is a lock-free `OnceLock::get`, which is what lets the
//! facade's `with_context` promise it never blocks the UI thread.
//!
//! Both functions are callable from any thread — the installing shell's
//! render thread and a reading UI thread are typically different ones, and
//! `OnceLock` itself is the synchronization; no additional lock is taken.
//!
//! # Callers
//!
//! [`install_gpu_handle`] is called by a shell's render executor, at the
//! point it first brings a device up (desktop: `frust-shell-desktop::render`,
//! right after a surface install succeeds, so the device is guaranteed
//! present — see `RenderContext::device_handle`'s own doc comment for why
//! that call is panic-free there). Unlike
//! [`crate::surface_mode::declare_host_translucent_surface`] this is not
//! restricted to generated host glue — installing a device handle carries no
//! host-configuration precondition to violate — so it is `pub`, reachable
//! from any crate that depends on this one. In practice only a shell's own
//! render executor ever calls it.
//!
//! [`gpu_handle`] is read by the facade's `frust::gpu::with_context` (see
//! `crates/frust/src/lib.rs`); nothing stops another caller from reading it
//! directly, the same openness [`crate::surface_mode::resolved_surface_mode`]
//! has.

use std::any::Any;
use std::sync::OnceLock;

/// The process-wide slot, install-once — see the module docs' Layering and
/// thread contract. Erased to `Box<dyn Any + Send + Sync>` so this crate
/// never names the concrete GPU-device type; every real caller instantiates
/// [`install_gpu_handle`]/[`gpu_handle`]'s generic with the same one type.
static GPU_HANDLE: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

/// Install the shell's GPU device handle, once.
///
/// Returns `true` if this call was the one that occupied the slot, `false`
/// if it was already occupied (by an earlier call — with this type or any
/// other) — the slot never overwrites, so a later call is inert rather than
/// replacing a live handle a reader may already be holding a reference into.
///
/// `T` must be `Any + Send + Sync` (enforced at the call site, not asserted
/// separately): a device handle read back on a different thread than it was
/// installed on must be safe to share, which is exactly the guarantee a
/// shell's own device type documents (cheap to clone, `Arc`-backed wgpu
/// resources underneath).
pub fn install_gpu_handle<T>(handle: T) -> bool
where
    T: Any + Send + Sync,
{
    GPU_HANDLE.set(Box::new(handle)).is_ok()
}

/// Read the installed GPU device handle back, `None` before any shell has
/// installed one (no surface has come up yet) or if `T` does not match the
/// type that was actually installed (a caller error — see the module docs'
/// Type-erased section — never a real code path in practice, since exactly
/// one concrete type is ever installed).
///
/// A lock-free `OnceLock::get` once occupied, so this never blocks — see the
/// module docs' Layering and thread contract.
pub fn gpu_handle<T>() -> Option<&'static T>
where
    T: Any + Send + Sync,
{
    GPU_HANDLE.get().and_then(|boxed| boxed.downcast_ref::<T>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading a type nothing in this process ever installs answers `None` —
    /// this doubles as the "before any shell has installed anything" case,
    /// since a mismatched-type read is indistinguishable from an empty slot
    /// (see the module docs' Type-erased section) and, unlike the occupied
    /// slot below, is safe to assert regardless of what other tests in this
    /// binary have done to the one shared static.
    #[test]
    fn gpu_handle_of_a_never_installed_type_is_none() {
        struct NeverInstalled;
        assert!(gpu_handle::<NeverInstalled>().is_none());
    }

    /// The only test in this module allowed to call [`install_gpu_handle`]
    /// (the static is process-wide and install-once, so a second test doing
    /// the same would race it under parallel test execution): proves the
    /// round trip and, in the same test, that a second install is inert
    /// rather than replacing the first.
    #[test]
    fn install_first_write_wins_and_reads_round_trip() {
        #[derive(Debug, PartialEq)]
        struct Installed(u32);

        let first = install_gpu_handle(Installed(42));
        assert!(first, "the first install in this process must win");

        let second = install_gpu_handle(Installed(7));
        assert!(!second, "a later install must not replace the first");

        let read = gpu_handle::<Installed>().expect("installed above");
        assert_eq!(*read, Installed(42));
    }
}
