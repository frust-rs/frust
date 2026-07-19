//! Android platform-handle accessors — the crate's **one sanctioned-`unsafe`
//! module**.
//!
//! On Android these read the `(JavaVM, application Context)` pair the shell
//! stashed in [`ndk-context`](ndk_context) at `nativeInitPlatform`, and hand a
//! live [`jni::Env`] + the context [`JObject`] to a caller closure. The only
//! `unsafe` in the whole crate is the raw-pointer *reconstruction* here —
//! rebuilding a [`JavaVM`] and a borrowed [`JObject`] from the opaque pointers
//! `ndk-context` stores — each isolated in one `unsafe` block with a `SAFETY:`
//! comment stating the contract the shell upholds (the pointers are the live
//! VM and a process-lifetime `Global` context reference).
//!
//! On every non-Android target this module is inert: no JNI types exist, and
//! [`with_jni_env`] is a stub that always reports
//! [`PlatformHandleError::NotInitialized`], so the crate stays an unconditional
//! dependency for plugins on every platform.

#[cfg(not(target_os = "android"))]
use crate::PlatformHandleError;

#[cfg(target_os = "android")]
pub use imp::{context, vm, with_jni_env};

/// Inert non-Android stub: platform handles never exist off-Android, so every
/// call reports [`PlatformHandleError::NotInitialized`] without running `f`.
///
/// Present (not absent) so plugins compile on every target and the pre-init
/// error path stays host-testable. Its closure takes no arguments because the
/// JNI env/context types it would otherwise borrow do not exist off-Android;
/// real plugin call sites gate the two-argument Android [`with_jni_env`] behind
/// `#[cfg(target_os = "android")]`.
#[cfg(not(target_os = "android"))]
pub fn with_jni_env<R>(_f: impl FnOnce() -> R) -> Result<R, PlatformHandleError> {
    Err(PlatformHandleError::NotInitialized)
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::JObject;
    use jni::{Env, JavaVM};

    use crate::PlatformHandleError;

    /// Read `ndk-context`'s stored `(JavaVM, Context)` pointers, mapping the
    /// "not yet initialized" state to [`PlatformHandleError::NotInitialized`].
    ///
    /// [`ndk_context::android_context`] *panics* (its only "empty" signal) when
    /// the shell has not called `initialize_android_context` yet, so we catch
    /// that unwind and convert it to a typed error. A defensive null-check on
    /// the stored pointers maps a corrupt (but "present") slot to the same
    /// error rather than handing a null handle to `jni`.
    ///
    /// Caveat: under a `panic = "abort"` profile (Frust's release apps)
    /// `catch_unwind` cannot intercept the pre-init panic, so a prefs call from
    /// an old scaffold that never calls `nativeInitPlatform` aborts instead of
    /// returning this error. A new scaffold always initializes before any
    /// plugin call, and debug builds (`frust run`'s default, unwind) degrade
    /// gracefully; the abort edge is the pre-init path on a release build only.
    fn android_context() -> Result<ndk_context::AndroidContext, PlatformHandleError> {
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

    /// The host stub's pre-init contract: no handles off-Android, so
    /// [`with_jni_env`](super::with_jni_env) always reports `NotInitialized`
    /// without running its closure (Phase 1 acceptance: pre-init is a typed
    /// error, never a panic).
    #[test]
    fn with_jni_env_reports_not_initialized_pre_init() {
        let result: Result<u8, _> =
            super::with_jni_env(|| unreachable!("stub must not run the closure"));
        assert!(matches!(result, Err(PlatformHandleError::NotInitialized)));
    }
}
