//! Android platform shell: the Rust half of the JNI bridge (spec §10.1).
//!
//! This crate is the Android counterpart to `forgekit-shell-desktop`. Where the
//! desktop shell owns a `winit` event loop, the Android shell is *driven* by the
//! Kotlin `ForgeKitSurfaceView` (see `templates/app/android.tmpl/.../
//! ForgeKitSurfaceView.kt`): the JVM calls a fixed set of
//! `Java_dev_forgekit_ForgeKitSurfaceView_native*` symbols, and each generated
//! app supplies its own `State`/`app_logic` through the [`android_app!`] macro,
//! which stamps out those symbols bound to the app's types.
//!
//! # Layering
//!
//! Like the desktop shell, this is an integration crate: on Android it composes
//! `forgekit-core` (the retained tree / `RenderRoot`), `forgekit-scene`,
//! `forgekit-render` (the `SurfaceRenderer` §8.1 state machine) and
//! `forgekit-text`, plus the `jni`/`ndk` FFI crates. The platform-agnostic
//! plumbing (the `AppTree` erasure and the `guard`/`sanitize_scale`/
//! `logical_size` helpers) is reused from `forgekit-shell-common` rather than
//! duplicated. All of the JNI/render composition is
//! `#[cfg(target_os = "android")]`; on every other target only the macro
//! definition and one pure host-testable helper ([`ffi_support`]) compile, so
//! the crate is inert in a host `cargo test --workspace`.
//!
//! # Unsafe
//!
//! This crate is a sanctioned `unsafe` zone (spec §10.1): the framework crates
//! stay `unsafe`-free, but a platform shell must cross the FFI boundary. The
//! crate's `unsafe` surface is [`jni_glue`]'s `unsafe` code — the
//! `ANativeWindow_fromSurface` call, `Box::into_raw`/`from_raw` for the opaque
//! handle, and reconstituting it as a `&mut` — each with a safety comment, plus
//! the `#[unsafe(no_mangle)]` attributes the [`android_app!`] macro emits on its
//! generated `extern "system"` exports (edition 2024 spells `no_mangle` as an
//! unsafe attribute).

// The one Android-specific pure helper (the opaque-handle liveness check),
// consumed by the (Android-only) FFI layer and by host unit tests. In a non-test
// host build nothing calls it, so silence dead-code there while keeping the
// warning active for the Android target where it must stay used.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod ffi_support;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
pub mod jni_glue;

// Re-exports the macro-generated `extern "system" fn`s reach for, so a generated
// app only needs to depend on `forgekit` (which re-exports `android_app!`), never
// on `jni`/`forgekit-shell-common`/`forgekit-shell-android` directly. The
// platform-agnostic erasure lives in `forgekit-shell-common`; it is surfaced
// through `$crate` here so the macro's `$crate::new_boxed_app`/`$crate::AppTree`
// paths resolve. Android-only: the macro body is never expanded off-Android (its
// call site is `#[cfg(target_os = "android")]`).
#[cfg(target_os = "android")]
#[doc(hidden)]
pub use forgekit_shell_common::{AppTree, new_boxed_app};

/// Re-exported `jni` types used verbatim by [`android_app!`]'s expansion.
///
/// Referencing them through `$crate::__jni` keeps the generated `extern` symbols
/// free of any assumption that the app crate depends on `jni` directly.
#[cfg(target_os = "android")]
#[doc(hidden)]
pub mod __jni {
    // `EnvUnowned` is the FFI-safe env type for native-method arguments in
    // `jni` 0.22 (the bare `JNIEnv` alias is deprecated); we only ever read its
    // raw pointer to hand to the NDK, never call the JNI API through it.
    pub use jni::EnvUnowned;
    pub use jni::objects::{JClass, JObject};
    pub use jni::sys::{jfloat, jint, jlong};
}

/// Bind a generated app's `State`/`app_logic` to the fixed Android JNI exports
/// (spec §10.1, Makepad `app_main!` precedent).
///
/// Stamps out the seven `Java_dev_forgekit_ForgeKitSurfaceView_native*` symbols
/// the Kotlin `ForgeKitSurfaceView` declares `external`, each delegating to the
/// non-generic runtime in [`jni_glue`]. `nativeInit` constructs the app's erased
/// view tree from `$state_ty::default()` and `$app_logic`; the rest operate on
/// the opaque `jlong` handle.
///
/// The macro is defined on every target but only *expands* to real code where
/// its call site is gated, e.g. in the generated `src/lib.rs`:
///
/// ```ignore
/// #[cfg(target_os = "android")]
/// forgekit::android_app!(AppState, app_logic);
/// ```
///
/// `$state_ty` must implement [`Default`]; `$app_logic` is a
/// `FnMut(&mut State) -> impl View<State>`.
#[macro_export]
macro_rules! android_app {
    ($state_ty:ty, $app_logic:expr $(,)?) => {
        /// JNI `nativeInit`: create the native handle for one surface.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeInit<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            surface: $crate::__jni::JObject<'local>,
            scale: $crate::__jni::jfloat,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_init(env, surface, scale, || {
                $crate::new_boxed_app::<$state_ty, _, _>(
                    <$state_ty as ::core::default::Default>::default(),
                    $app_logic,
                )
            })
        }

        /// JNI `nativeOnSurfaceChanged`: (re)create or resize the surface.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnSurfaceChanged<
            'local,
        >(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            surface: $crate::__jni::JObject<'local>,
            width: $crate::__jni::jint,
            height: $crate::__jni::jint,
        ) {
            $crate::jni_glue::native_on_surface_changed(env, handle, surface, width, height)
        }

        /// JNI `nativeOnSurfaceDestroyed`: tear the surface down.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnSurfaceDestroyed<
            'local,
        >(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_surface_destroyed(handle)
        }

        /// JNI `nativeOnFrame`: run one Choreographer-driven frame.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnFrame<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            frame_time_nanos: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_frame(handle, frame_time_nanos)
        }

        /// JNI `nativeOnResume`: activity resumed (bookkeeping only in v0).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnResume<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_resume(handle)
        }

        /// JNI `nativeOnPause`: activity paused (bookkeeping only in v0).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnPause<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_pause(handle)
        }

        /// JNI `nativeOnDestroy`: release the native handle.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnDestroy<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_destroy(handle)
        }
    };
}
