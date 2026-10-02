//! Frust plugin substrate: the platform handles a plugin needs to reach the
//! host OS through FFI, with **zero per-plugin native code**.
//!
//! Frust apps are Rust in the same process as the OS, so a plugin can call
//! platform APIs directly through FFI crates (`jni` on Android, `objc2` on
//! Apple) — no Kotlin/Swift wrapper, no message-channel bridge (the thing
//! Flutter's Dart VM forces and Rust doesn't need). This crate publishes the
//! one thing a plugin can't get for free: the Android `(JavaVM, application
//! Context)` pair. On Apple the ObjC runtime is globally reachable via `objc2`
//! with nothing to publish, so this crate has no Apple surface.
//!
//! # Charter: a leaf crate, two meeting points, opposite write/read direction
//!
//! `frust-plugin` is a **leaf** (like `frust-reactive`): it has **no
//! `frust-*` dependencies** and no `objc2`. It publishes two independent
//! substrate halves:
//!
//! - [`android`] — the Android **platform-handle slot**: the shell *writes*
//!   the `(JavaVM, application Context)` pair into `ndk-context`'s
//!   process-wide slot (it owns the JNI boundary — see
//!   `frust-shell-android`'s `nativeInitPlatform` export); plugins *read*
//!   them back. Its only platform dep (`jni` + `ndk-context`) is
//!   Android-target-gated. Using `ndk-context` as the storage slot (rather
//!   than a private static) also makes any third-party crate that reads
//!   `ndk-context` work inside a Frust app for free.
//! - [`desktop`] — the desktop **view-factory registry**: a plugin *writes* a
//!   [`desktop::DesktopViewFactory`] into a process-global table keyed by
//!   `view_type`; a desktop shell *reads* it back to create/update/dispose
//!   the native view a slot names. std-only, no `cfg` gate — compiled on
//!   every target — even though only a desktop shell (today: macOS) ever
//!   calls [`desktop::lookup_view_factory`].
//!
//! A **platform plugin** (e.g. `frust-shared-preferences`) may depend on this
//! crate plus `frust-paths` and FFI crates, and must NOT depend on any
//! `frust-*` framework crate — that keeps plugins tiny and free of framework
//! cycles.

pub mod android;
pub mod desktop;

/// Failure to obtain a platform handle from the host shell.
///
/// A `thiserror` enum because callers match on it (it is part of this crate's
/// API contract, per `docs/CODE_STANDARDS.md`), and because a plugin turns it
/// into its own typed error the app can act on rather than a leaf panic.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum PlatformHandleError {
    /// The host shell never installed the platform handles.
    ///
    /// On Android this means the app was scaffolded before the plugin platform
    /// bridge existed (or its generated `FrustSurfaceView` is missing the
    /// `nativeInitPlatform` call); on any non-Android target it is the
    /// permanent state of the inert stub. The message names the exact fix so a
    /// user hitting it in a log knows what to do — never a panic (Design
    /// Decision 5).
    #[error(
        "Frust platform handles are not initialized. If this is an Android app \
         scaffolded before the plugin platform bridge existed, re-scaffold with \
         `frust create --overwrite`, or add the single \
         `nativeInitPlatform(context.applicationContext)` call to your generated \
         `FrustSurfaceView` (right before `nativeInit`)."
    )]
    NotInitialized,

    /// Attaching the current thread to the Android `JavaVM` failed (Android
    /// only). Wraps the underlying `jni` error's message.
    #[cfg(target_os = "android")]
    #[error("failed to attach the current thread to the Android JavaVM: {0}")]
    Attach(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The re-scaffold hint is load-bearing (Design Decision 5): a user hitting
    /// `NotInitialized` in a log must be told both fixes.
    #[test]
    fn not_initialized_display_mentions_the_rescaffold_fix() {
        let msg = PlatformHandleError::NotInitialized.to_string();
        assert!(
            msg.contains("frust create --overwrite"),
            "message should name the re-scaffold fix: {msg}"
        );
        assert!(
            msg.contains("nativeInitPlatform"),
            "message should name the one-line Kotlin fix: {msg}"
        );
    }
}
