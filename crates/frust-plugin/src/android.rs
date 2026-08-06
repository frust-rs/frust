//! Android platform-handle accessors — the crate's **one sanctioned-`unsafe`
//! module**.
//!
//! # Pre-init detection is an atomic flag, checked first
//!
//! This module owns the process-wide "handles installed" state: a single
//! [`AtomicBool`] ([`INITIALIZED`]) that Android's `initialize` flips to
//! `true` — *after* it has handed the `(JavaVM, application Context)` pointers
//! to `ndk-context` — and that every accessor
//! (`vm`/`context`/[`with_jni_env`], via `android_context`) reads
//! **first**. Before init the flag is `false`, so a plugin call returns
//! [`PlatformHandleError::NotInitialized`] on a path that never touches
//! `ndk_context::android_context` or `catch_unwind` — the pre-init decision is
//! a plain relaxed-free acquire load, so it stays a typed error even under a
//! release `panic = "abort"` profile where a would-be unwind cannot be caught
//! (the guarantee no longer rides on `catch_unwind`, Design Decision 5). The
//! flag is declared unconditionally (outside the Android `cfg`) so the
//! gate-before-`Err` logic is host-testable.
//!
//! On Android, once the flag is set, the accessors read the `(JavaVM,
//! application Context)` pair the shell installed and hand a live `jni::Env`
//! plus the context `JObject` to a caller closure. The only `unsafe` here is
//! the raw-pointer work: `initialize` forwarding the shell's opaque pointers to
//! `ndk-context`, and the *reconstruction* of a `JavaVM` / borrowed
//! `JObject` from the pointers `ndk-context` stores — each isolated in one
//! `unsafe` block with a `SAFETY:` comment stating the contract the shell
//! upholds (the pointers are the live VM and a process-lifetime `Global`
//! context reference).
//!
//! On every non-Android target this module is inert: no JNI types exist,
//! `initialize` does not exist, so the flag can never be set and
//! [`with_jni_env`] always reports [`PlatformHandleError::NotInitialized`] —
//! keeping the crate an unconditional dependency for plugins on every platform.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::PlatformHandleError;

/// Whether the host shell has installed the platform handles for this process.
///
/// Set to `true` exactly once by Android's [`initialize`], *after* the
/// `(JavaVM, Context)` pointers reach `ndk-context`; read (acquire) by
/// [`ensure_initialized`] on every accessor call. Declared unconditionally —
/// off-Android there is no [`initialize`], so it stays `false` forever and the
/// gate logic is exercisable by the host test.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The non-panicking pre-init gate: `Ok(())` once [`INITIALIZED`] is set, else
/// [`PlatformHandleError::NotInitialized`].
///
/// A plain [`Ordering::Acquire`] load — no `ndk-context` call, no
/// `catch_unwind` — so the pre-init error path is reached identically under an
/// unwinding *or* an aborting panic runtime. Pairs with [`initialize`]'s
/// [`Ordering::Release`] store so a caller that observes `true` also observes
/// the handles `initialize` published to `ndk-context` before the store.
fn ensure_initialized() -> Result<(), PlatformHandleError> {
    if INITIALIZED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(PlatformHandleError::NotInitialized)
    }
}

/// Inert non-Android stub: the flag is never set off-Android, so every call
/// reports [`PlatformHandleError::NotInitialized`] (via [`ensure_initialized`])
/// without running `f`.
///
/// Present (not absent) so plugins compile on every target and the pre-init
/// error path stays host-testable. Its closure takes no arguments because the
/// JNI env/context types it would otherwise borrow do not exist off-Android;
/// real plugin call sites gate the two-argument Android [`with_jni_env`] behind
/// `#[cfg(target_os = "android")]`.
#[cfg(not(target_os = "android"))]
pub fn with_jni_env<R>(f: impl FnOnce() -> R) -> Result<R, PlatformHandleError> {
    ensure_initialized().map(|()| f())
}

#[cfg(target_os = "android")]
pub use imp::{context, initialize, vm, with_jni_env};

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;

    use jni::objects::JObject;
    use jni::{Env, JavaVM};

    use crate::PlatformHandleError;
    use crate::android::{INITIALIZED, ensure_initialized};

    /// Install the process platform handles: hand the `(JavaVM, application
    /// Context)` pointers to [`ndk-context`](ndk_context), then flip
    /// [`INITIALIZED`] so the accessors report "ready".
    ///
    /// Called by the Android shell's `nativeInitPlatform` (the JNI-boundary
    /// owner) exactly once per process. The [`INITIALIZED`] store is
    /// [`Ordering::Release`] and happens **after** the `ndk-context` install,
    /// so any accessor that observes the flag `true` (an
    /// [`Ordering::Acquire`] load) also observes the installed handles.
    ///
    /// # Safety
    ///
    /// - `java_vm` must be the live process [`JavaVM`] pointer, valid for the
    ///   whole process lifetime.
    /// - `context_jobject` must be a valid application-`Context` JNI reference
    ///   that stays valid for the whole process lifetime — the shell keeps it
    ///   alive as a leaked `Global` reference (a dropped one would dangle).
    /// - Must be called **at most once** per process:
    ///   `ndk_context::initialize_android_context` panics on a second call. The
    ///   shell's `PLATFORM_INIT` [`Once`](std::sync::Once) upholds this even
    ///   though Kotlin re-invokes `nativeInitPlatform` after activity
    ///   recreation — the second and later calls never reach here.
    pub unsafe fn initialize(java_vm: *mut c_void, context_jobject: *mut c_void) {
        // SAFETY: the caller's contract (this fn's `# Safety`) guarantees a
        // live process-lifetime VM pointer, a process-lifetime context
        // reference, and an exactly-once call — precisely what
        // `initialize_android_context` requires.
        unsafe {
            ndk_context::initialize_android_context(java_vm, context_jobject);
        }
        // Publish *after* the install (Release) so an `Acquire` observer of
        // `true` also sees the handles above.
        INITIALIZED.store(true, Ordering::Release);
    }

    /// Read `ndk-context`'s stored `(JavaVM, Context)` pointers, gated on the
    /// pre-init flag so the "not yet initialized" state is a typed
    /// [`PlatformHandleError::NotInitialized`] on a panic-machinery-free path.
    ///
    /// [`ensure_initialized`] is checked **first**: before [`initialize`] runs
    /// this returns the error without ever calling
    /// [`ndk_context::android_context`] (which *panics* as its only "empty"
    /// signal). That flag check is the load-bearing guarantee — it holds under
    /// a `panic = "abort"` release profile, where an unwind cannot be caught.
    ///
    /// The `catch_unwind` wrap below is kept only as a documented **second
    /// belt** for a now-narrow edge: a third party calling
    /// `ndk_context::release_android_context()` *after* our [`initialize`] would
    /// re-arm the panic while our flag still reads `true`. A defensive
    /// null-check on the stored pointers maps a corrupt (but "present") slot to
    /// the same error rather than handing a null handle to `jni`.
    fn android_context() -> Result<ndk_context::AndroidContext, PlatformHandleError> {
        // Pre-init gate — no `ndk-context`/`catch_unwind` on this path.
        ensure_initialized()?;
        // Second belt (see doc): only reachable post-init; not load-bearing.
        let ctx = std::panic::catch_unwind(ndk_context::android_context)
            .map_err(|_| PlatformHandleError::NotInitialized)?;
        if ctx.vm().is_null() || ctx.context().is_null() {
            return Err(PlatformHandleError::NotInitialized);
        }
        Ok(ctx)
    }

    /// The process [`JavaVM`], reconstructed from `ndk-context`'s stored
    /// pointer. Returns [`PlatformHandleError::NotInitialized`] before the shell
    /// installs the handles.
    pub fn vm() -> Result<JavaVM, PlatformHandleError> {
        let ctx = android_context()?;
        // SAFETY: `ctx.vm()` is the live `JavaVM` pointer the shell captured in
        // `JNI_OnLoad` and handed to `ndk-context` at `nativeInitPlatform`; it
        // is valid for the whole process lifetime. `JavaVM::from_raw` only
        // null-checks (already ruled out above) and interns the pointer.
        Ok(unsafe { JavaVM::from_raw(ctx.vm().cast()) })
    }

    /// The raw application-`Context` jobject pointer `ndk-context` holds — a
    /// process-lifetime global reference the shell created. Callers must use it
    /// only within an attached thread / valid JNI frame; the safe path is
    /// [`with_jni_env`], which supplies both the env and this context.
    pub fn context() -> Result<jni::sys::jobject, PlatformHandleError> {
        Ok(android_context()?.context().cast())
    }

    /// Attach the current thread to the `JavaVM`, call `f` with a live
    /// [`Env`] and the application-[`Context`](JObject), and return its value.
    ///
    /// The attachment is **scoped** ([`JavaVM::attach_current_thread_for_scope`]):
    /// it detaches when `f` returns. This is deliberate — Frust does not own
    /// the threads a plugin may run on (a tokio worker, a caller thread) and
    /// JNI threads do **not** auto-detach on exit, so a permanent attach would
    /// leak the attachment until process death. Prefs-style calls are rare and
    /// cheap, so the per-call attach cost is acceptable.
    ///
    /// Returns [`PlatformHandleError::NotInitialized`] before the shell installs
    /// the handles, or [`PlatformHandleError::Attach`] if the JVM attach itself
    /// fails.
    pub fn with_jni_env<R>(
        f: impl FnOnce(&mut Env, &JObject) -> R,
    ) -> Result<R, PlatformHandleError> {
        let ctx = android_context()?;
        let context_ptr = ctx.context();
        // SAFETY: see `vm()` — `ctx.vm()` is the live, process-lifetime VM
        // pointer, non-null (checked in `android_context`).
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };
        vm.attach_current_thread_for_scope(|env| {
            // SAFETY: `context_ptr` is the process-lifetime `Global` application
            // context reference the shell created; it is a valid JNI reference
            // for the whole process, and this borrowed `JObject` never outlives
            // the scoped attachment `f` runs inside. We hold exactly one wrapper
            // for it here.
            let context = unsafe { JObject::from_raw(env, context_ptr.cast()) };
            Ok::<R, jni::errors::Error>(f(env, &context))
        })
        .map_err(|err: jni::errors::Error| PlatformHandleError::Attach(err.to_string()))
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use crate::PlatformHandleError;

    /// The unconditional pre-init gate ([`ensure_initialized`](super::ensure_initialized)):
    /// with the flag unset it returns `NotInitialized` on a path that touches
    /// **no** panic machinery — a plain acquire load, not `catch_unwind` /
    /// `ndk-context` (fix F1: the guarantee no longer rides on `catch_unwind`,
    /// so it holds under `panic = "abort"`). Off-Android [`initialize`] does not
    /// exist, so the flag is permanently `false` and this exercises exactly the
    /// pre-init branch every accessor takes first.
    #[test]
    fn ensure_initialized_reports_not_initialized_pre_init() {
        assert!(matches!(
            super::ensure_initialized(),
            Err(PlatformHandleError::NotInitialized)
        ));
    }

    /// The host stub's pre-init contract: no handles off-Android, so
    /// [`with_jni_env`](super::with_jni_env) reports `NotInitialized` (via the
    /// flag gate) without running its closure (pre-init is a
    /// typed error, never a panic).
    #[test]
    fn with_jni_env_reports_not_initialized_pre_init() {
        let result: Result<u8, _> =
            super::with_jni_env(|| unreachable!("stub must not run the closure"));
        assert!(matches!(result, Err(PlatformHandleError::NotInitialized)));
    }
}
