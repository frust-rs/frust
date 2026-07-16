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
pub use forgekit_shell_common::{AppTree, new_boxed_app, new_boxed_app_with};

/// Bind a generated app's `State`/`app_logic` to the fixed iOS C-ABI exports
/// (spec §10.2, Makepad `app_main!` precedent).
///
/// Stamps out the ten `forgekit_*` symbols the generated Swift app declares (the
/// seven lifecycle/input exports plus the three text-input exports —
/// `forgekit_ime_apply`, `forgekit_ime_state_json`, `forgekit_string_free`),
/// each delegating to the non-generic runtime in [`ffi_glue`]. `forgekit_init`
/// constructs the app's erased view tree and returns an opaque handle; the rest
/// operate on that handle. It also initializes the process-wide
/// [`forgekit_reactive::ReactiveRuntime`] (idempotent, framework-side inside
/// [`ffi_glue::init`] rather than emitted here, so both arms below get it with
/// no macro duplication) before the state/view tree is constructed.
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
/// Two forms:
/// - `ios_app!($state_ty, $app_logic)` — `$state_ty` must implement [`Default`];
///   the initial state is `$state_ty::default()`.
/// - `ios_app!($state_ty, $state_init, $app_logic)` — `$state_init` is a
///   `FnOnce() -> $state_ty` factory, for a state type that doesn't implement
///   `Default` (e.g. one a future `Component::init` builds). The 2-arg form
///   delegates to this one with `<$state_ty as Default>::default` as the
///   factory.
///
/// `$app_logic` is a `FnMut(&mut State) -> impl View<State>`.
#[macro_export]
macro_rules! ios_app {
    ($state_ty:ty, $app_logic:expr $(,)?) => {
        $crate::ios_app!(
            $state_ty,
            <$state_ty as ::core::default::Default>::default,
            $app_logic
        );
    };
    ($state_ty:ty, $state_init:expr, $app_logic:expr $(,)?) => {
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
                $crate::new_boxed_app_with::<$state_ty, _, _, _>($state_init, $app_logic)
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
        ///
        /// `timestamp_ns` is the `CADisplayLink` tick's `timestamp`
        /// (`CFTimeInterval` seconds) converted to nanoseconds by the Swift
        /// caller — the shell-owned monotonic frame clock (spec §8).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_render_frame(
            handle: *mut ::core::ffi::c_void,
            timestamp_ns: u64,
        ) {
            $crate::ffi_glue::render_frame(handle, timestamp_ns)
        }

        /// `forgekit_dispatch_touch`: deliver one touch contact (spec §9).
        ///
        /// `phase` is the fixed code (`0`=began, `1`=moved, `2`=ended,
        /// `3`=cancelled — an ABI shared with the Swift `ForgeKitView`); `x`/`y`
        /// are logical points (passed through, no scale division).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_dispatch_touch(
            handle: *mut ::core::ffi::c_void,
            phase: u32,
            x: f32,
            y: f32,
        ) {
            $crate::ffi_glue::dispatch_touch(handle, phase, x, y)
        }

        /// `forgekit_ime_apply`: push a whole editing state from the Swift
        /// `UITextInput` mirror into the focused widget (spec §9 state-sync path).
        ///
        /// `text` is UTF-8; `sel_*`/`comp_*` are UTF-16 code-unit indices (`-1` =
        /// none) — the ABI shared with the Swift `ForgeKitTextInput` mirror.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_ime_apply(
            handle: *mut ::core::ffi::c_void,
            text: *const ::core::ffi::c_char,
            sel_base: i32,
            sel_ext: i32,
            comp_base: i32,
            comp_ext: i32,
        ) {
            $crate::ffi_glue::ime_apply(handle, text, sel_base, sel_ext, comp_base, comp_ext)
        }

        /// `forgekit_ime_state_json`: the focused field's IME surface as a
        /// heap-allocated JSON C string the caller must release with
        /// `forgekit_string_free`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_ime_state_json(
            handle: *mut ::core::ffi::c_void,
        ) -> *mut ::core::ffi::c_char {
            $crate::ffi_glue::ime_state_json(handle)
        }

        /// `forgekit_string_free`: release a C string returned by
        /// `forgekit_ime_state_json`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn forgekit_string_free(s: *mut ::core::ffi::c_char) {
            $crate::ffi_glue::string_free(s)
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
