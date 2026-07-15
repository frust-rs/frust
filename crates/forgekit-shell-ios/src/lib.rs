//! iOS platform shell: the Rust half of the C-ABI bridge (spec §10.2).
//!
//! This crate is the iOS counterpart to `forgekit-shell-desktop`/
//! `forgekit-shell-android`. Where the desktop shell owns a `winit` event loop
//! and the Android shell is driven by a Kotlin `SurfaceView` over JNI, the iOS
//! shell is *driven* by the generated Swift app (see the Swift template): Swift
//! calls a fixed set of `forgekit_*` C functions, and each generated app supplies
//! its own `State`/`app_logic` through the [`ios_app!`] macro, which stamps out
//! those exports bound to the app's types.
//!
//! # Layering
//!
//! Like the other shells, this is an integration crate: on iOS it composes
//! `forgekit-core` (the retained tree / `RenderRoot`), `forgekit-scene`,
//! `forgekit-render` (the `SurfaceRenderer` §8.1 state machine) and
//! `forgekit-text`. The platform-agnostic plumbing (the `AppTree` erasure and the
//! `guard`/`sanitize_scale`/`logical_size` helpers) is reused from
//! `forgekit-shell-common` rather than duplicated. All of the C-ABI/render
//! composition is `#[cfg(target_os = "ios")]`; on every other target only the
//! macro definition and the pure host-testable helpers ([`ffi_support`]) compile,
//! so the crate is inert in a host `cargo test --workspace` and on Android.
//!
//! # Unsafe
//!
//! This crate is a sanctioned `unsafe` zone (spec §10.2): the framework crates
//! stay `unsafe`-free, but a platform shell must cross the FFI boundary. The
//! crate's `unsafe` surface is [`ffi_glue`]'s `unsafe` code — the call into
//! `on_surface_created_from_metal_layer`, `Box::into_raw`/`from_raw` for the
//! opaque handle, and reconstituting it as a `&mut` — each with a safety comment,
//! plus the `#[unsafe(no_mangle)]` attributes the [`ios_app!`] macro emits on its
//! generated `extern "C"` exports (edition 2024 spells `no_mangle` as an unsafe
//! attribute).

// The iOS-specific pure helpers (the null-handle sentinel and the paused/ready
// frame gate), consumed by the (iOS-only) FFI layer and by host unit tests. In a
// non-test host build nothing calls them, so silence dead-code there while
// keeping the warning active for the iOS target where they must stay used.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ffi_support;

#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
pub mod ffi_glue;

// Re-exports the macro-generated `extern "C" fn`s reach for, so a generated app
// only needs to depend on `forgekit` (which re-exports `ios_app!`), never on
// `forgekit-shell-common`/`forgekit-shell-ios` directly. The platform-agnostic
// erasure lives in `forgekit-shell-common`; it is surfaced through `$crate` here
// so the macro's `$crate::new_boxed_app`/`$crate::AppTree` paths resolve.
// iOS-only: the macro's generated bodies are themselves `#[cfg(target_os =
// "ios")]`, so nothing references these off-iOS.
#[cfg(target_os = "ios")]
#[doc(hidden)]
pub use forgekit_shell_common::{AppTree, new_boxed_app};

/// Bind a generated app's `State`/`app_logic` to the fixed iOS C-ABI exports
/// (spec §10.2, Makepad `app_main!` precedent).
///
/// Stamps out the six `forgekit_*` symbols the generated Swift app declares, each
/// delegating to the non-generic runtime in [`ffi_glue`]. `forgekit_init`
/// constructs the app's erased view tree from `$state_ty::default()` and
/// `$app_logic` and returns an opaque handle; the rest operate on that handle.
///
/// Unlike [`android_app!`](forgekit_shell_android::android_app), this macro is
/// **call-site transparent**: it is invoked unconditionally in the generated
/// `src/lib.rs`, because every symbol it emits is itself
/// `#[cfg(target_os = "ios")]`, so off-iOS the invocation expands to nothing:
///
/// ```ignore
/// forgekit::ios_app!(AppState, app_logic);
/// ```
///
/// `$state_ty` must implement [`Default`]; `$app_logic` is a
/// `FnMut(&mut State) -> impl View<State>`.
#[macro_export]
macro_rules! ios_app {
    ($state_ty:ty, $app_logic:expr $(,)?) => {
        /// `forgekit_init`: create the native handle for the app's CAMetalLayer.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_init(
            metal_layer: *mut ::core::ffi::c_void,
            width: u32,
            height: u32,
            scale: f32,
        ) -> *mut ::core::ffi::c_void {
            $crate::ffi_glue::init(metal_layer, width, height, scale, || {
                $crate::new_boxed_app::<$state_ty, _, _>(
                    <$state_ty as ::core::default::Default>::default(),
                    $app_logic,
                )
            })
        }

        /// `forgekit_resize`: resize the live surface (rotation / bounds change).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_resize(
            handle: *mut ::core::ffi::c_void,
            width: u32,
            height: u32,
            scale: f32,
        ) {
            $crate::ffi_glue::resize(handle, width, height, scale)
        }

        /// `forgekit_render_frame`: run one CADisplayLink-driven frame.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_render_frame(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::render_frame(handle)
        }

        /// `forgekit_pause`: app backgrounded — stop submitting frames.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_pause(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::pause(handle)
        }

        /// `forgekit_resume`: app foregrounded — resume submitting frames.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_resume(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::resume(handle)
        }

        /// `forgekit_destroy`: release the native handle (and its surface).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_destroy(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::destroy(handle)
        }
    };
}
