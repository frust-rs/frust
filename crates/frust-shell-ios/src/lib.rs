//! iOS platform shell: the Rust half of the C-ABI bridge.
//!
//! This crate is the iOS counterpart to `frust-shell-desktop`/
//! `frust-shell-android`. Where the desktop shell owns a `winit` event loop
//! and the Android shell is driven by a Kotlin `SurfaceView` over JNI, the iOS
//! shell is *driven* by the generated Swift app (see the Swift template): Swift
//! calls a fixed set of `frust_*` C functions, and each generated app supplies
//! its own `State`/`app_logic` through the [`ios_app!`] macro, which stamps out
//! those exports bound to the app's types.
//!
//! # Layering
//!
//! Like the other shells, this is an integration crate: on iOS it composes
//! `frust-core` (the retained tree / `RenderRoot`), `frust-scene`,
//! `frust-render` (the `SurfaceRenderer` state machine) and
//! `frust-text`. The platform-agnostic plumbing (the `AppTree` erasure and the
//! `guard`/`sanitize_scale`/`logical_size` helpers) is reused from
//! `frust-shell-common` rather than duplicated. All of the C-ABI/render
//! composition is `#[cfg(target_os = "ios")]`; on every other target only the
//! macro definition and the pure host-testable helpers ([`ffi_support`]) compile,
//! so the crate is inert in a host `cargo test --workspace` and on Android.
//!
//! # GPU-device hand-back (`gpu` feature)
//!
//! This shell's `gpu` Cargo feature forwards into `frust-shell-common/gpu`
//! and `frust-render/gpu` (the process-wide device slot and its
//! `DeviceHandle` re-export — see `frust-shell-common`'s `gpu` module and
//! `crates/frust/src/lib.rs`'s `gpu` module), the same seam
//! `frust-shell-desktop`'s render executor installs its live device into.
//! **This shell does not install one yet** — its render-thread device
//! creation lives in `ffi_glue`'s render-loop bring-up and `app::executor`,
//! wiring an install call there is a render-thread-split-aware change of
//! its own, not a one-line addition — so `frust::gpu::with_context`
//! currently answers `None` on iOS even with `--features gpu` on. An
//! accepted, explicitly tracked gap rather than a half-built accessor.
//!
//! # Unsafe
//!
//! This crate is a sanctioned `unsafe` zone: the framework crates
//! stay `unsafe`-free, but a platform shell must cross the FFI boundary. The
//! crate's `unsafe` surface is [`ffi_glue`]'s `unsafe` code — the call into
//! `on_surface_created_from_metal_layer`, the
//! `accessibility::IosA11yAdapter::new` construction (a sanctioned raw-`UIView*`
//! constructor, analogous to `frust-render`'s surface constructors — declared
//! `unsafe fn` in [`accessibility`] but *called* only from `ffi_glue`),
//! `Box::into_raw`/`from_raw` for the opaque handle, and reconstituting it as a
//! `&mut` — each with a safety comment, plus the `#[unsafe(no_mangle)]`
//! attributes the [`ios_app!`] macro emits on its generated `extern "C"` exports
//! (edition 2024 spells `no_mangle` as an unsafe attribute). The `app` module —
//! the frame/lifecycle runtime — contains no `unsafe`; the only `unsafe fn`
//! declaration outside `ffi_glue` is `IosA11yAdapter::new`, and its sole call
//! site is `ffi_glue::init_accessibility`.

// The iOS-specific pure helpers (the null-handle sentinel and the paused/ready
// frame gate), consumed by the (iOS-only) FFI layer and by host unit tests. In a
// non-test host build nothing calls them, so silence dead-code there while
// keeping the warning active for the iOS target where they must stay used.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ffi_support;

#[cfg(target_os = "ios")]
mod accessibility;
#[cfg(target_os = "ios")]
mod app;
#[cfg(target_os = "ios")]
pub mod ffi_glue;

// Re-exports the macro-generated `extern "C" fn`s reach for, so a generated app
// only needs to depend on `frust` (which re-exports `ios_app!`), never on
// `frust-shell-common`/`frust-shell-ios` directly. The platform-agnostic
// erasure lives in `frust-shell-common`; it is surfaced through `$crate` here
// so the macro's `$crate::new_boxed_app`/`$crate::AppTree` paths resolve.
// iOS-only: the macro's generated bodies are themselves `#[cfg(target_os =
// "ios")]`, so nothing references these off-iOS.
#[cfg(target_os = "ios")]
#[doc(hidden)]
pub use frust_shell_common::{AppTree, new_boxed_app, new_boxed_app_with};

/// Bind a generated app's `State`/`app_logic` to the fixed iOS C-ABI exports
/// (Makepad `app_main!` precedent).
///
/// Stamps out the twenty-four `frust_*` symbols the generated Swift app
/// declares
/// (the seven lifecycle/input exports, the three text-input exports —
/// `frust_ime_apply`, `frust_ime_state_json`, `frust_string_free` —
/// the four clipboard/system-edit-menu exports — `frust_edit_command`,
/// `frust_take_clipboard_write`, `frust_take_paste_request` and
/// `frust_selection_toolbar_json`, the route that keeps every paste exempt
/// from iOS's paste-permission alert by going through the platform's own
/// menu (see `ffi_glue::edit_command`) —
/// `frust_set_appearance` (the dark-mode export),
/// `frust_set_reduce_motion` (the reduced-motion accessibility export — the
/// same transport over a different sensor: `UIAccessibility`, not
/// `UITraitCollection`),
/// `frust_on_deep_link` (cold-start/running deep-link delivery),
/// `frust_init_accessibility` (the accesskit adapter
/// attach — the one export whose UIView pointer the CAMetalLayer-only
/// `frust_init` cannot supply), `frust_set_insets` (the
/// SafeArea inset delivery), `frust_system_ui_state`
/// (the system-UI override peek — see
/// `ffi_glue::system_ui_state`), plus `frust_set_surface_mode` and
/// `frust_platform_view_commands_json` (the
/// translucent-surface opt-in latch and command-backlog peek getter — see
/// `ffi_glue::set_surface_mode`/`ffi_glue::platform_view_commands_json`),
/// plus `frust_set_present_sync` and `frust_present_frame` (the
/// present-sync latch and its per-tick UI-thread present — see
/// `ffi_glue::set_present_sync`/`ffi_glue::present_frame`)),
/// each delegating to the non-generic runtime in
/// [`ffi_glue`]. `frust_init` constructs the app's erased view tree and returns
/// an opaque handle; the rest operate on that handle. It also initializes the
/// process-wide
/// [`frust_reactive::ReactiveRuntime`] (idempotent, framework-side inside
/// [`ffi_glue::init`] rather than emitted here, so both arms below get it with
/// no macro duplication) before the state/view tree is constructed.
///
/// Unlike [`android_app!`](frust_shell_android::android_app), this macro is
/// **call-site transparent**: it is invoked unconditionally in the generated
/// `src/lib.rs`, because every symbol it emits is itself
/// `#[cfg(target_os = "ios")]`, so off-iOS the invocation expands to nothing:
///
/// ```ignore
/// frust::ios_app!(AppState, app_logic);
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
        /// `frust_init`: create the native handle for the app's CAMetalLayer.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_init(
            metal_layer: *mut ::core::ffi::c_void,
            width: u32,
            height: u32,
            scale: f32,
        ) -> *mut ::core::ffi::c_void {
            $crate::ffi_glue::init(metal_layer, width, height, scale, || {
                $crate::new_boxed_app_with::<$state_ty, _, _, _>($state_init, $app_logic)
            })
        }

        /// `frust_init_accessibility`: attach the accesskit adapter to the
        /// app's `FrustView`.
        ///
        /// Separate from `frust_init` because the accesskit `SubclassingAdapter`
        /// needs the **UIView** pointer, whereas `frust_init` only receives the
        /// `CAMetalLayer` (the GPU surface). Swift calls this once, on the first
        /// layout, right after `frust_init` succeeds and before the view is
        /// shown — passing `view` as the raw `FrustView` pointer. A null handle
        /// or null view is a benign no-op.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_init_accessibility(
            handle: *mut ::core::ffi::c_void,
            view: *mut ::core::ffi::c_void,
        ) {
            $crate::ffi_glue::init_accessibility(handle, view)
        }

        /// `frust_resize`: resize the live surface (rotation / bounds change).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_resize(
            handle: *mut ::core::ffi::c_void,
            width: u32,
            height: u32,
            scale: f32,
        ) {
            $crate::ffi_glue::resize(handle, width, height, scale)
        }

        /// `frust_render_frame`: run one CADisplayLink-driven frame.
        ///
        /// `timestamp_ns` is the `CADisplayLink` tick's `timestamp`
        /// (`CFTimeInterval` seconds) converted to nanoseconds by the Swift
        /// caller — the shell-owned monotonic frame clock.
        ///
        /// Returns `u8`: `0` = a fatal render-thread failure
        /// (first-surface install could not succeed), on which Swift's
        /// `renderFrame` latches `initFailed` and invalidates its `CADisplayLink`;
        /// `1` = keep driving frames. The bridging header's `void` return becomes
        /// `uint8_t` in lockstep.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_render_frame(
            handle: *mut ::core::ffi::c_void,
            timestamp_ns: u64,
        ) -> u8 {
            $crate::ffi_glue::render_frame(handle, timestamp_ns)
        }

        /// `frust_dispatch_touch`: deliver one touch contact.
        ///
        /// `phase` is the fixed code (`0`=began, `1`=moved, `2`=ended,
        /// `3`=cancelled — an ABI shared with the Swift `FrustView`); `x`/`y`
        /// are logical points (passed through, no scale division).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_dispatch_touch(
            handle: *mut ::core::ffi::c_void,
            phase: u32,
            x: f32,
            y: f32,
        ) {
            $crate::ffi_glue::dispatch_touch(handle, phase, x, y)
        }

        /// `frust_ime_apply`: push a whole editing state from the Swift
        /// `UITextInput` mirror into the focused widget (the state-sync path).
        ///
        /// `text` is UTF-8; `sel_*`/`comp_*` are UTF-16 code-unit indices (`-1` =
        /// none) — the ABI shared with the Swift `FrustTextInput` mirror.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_ime_apply(
            handle: *mut ::core::ffi::c_void,
            text: *const ::core::ffi::c_char,
            sel_base: i32,
            sel_ext: i32,
            comp_base: i32,
            comp_ext: i32,
        ) {
            $crate::ffi_glue::ime_apply(handle, text, sel_base, sel_ext, comp_base, comp_ext)
        }

        /// `frust_ime_state_json`: the focused field's IME surface as a
        /// heap-allocated JSON C string the caller must release with
        /// `frust_string_free`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_ime_state_json(
            handle: *mut ::core::ffi::c_void,
        ) -> *mut ::core::ffi::c_char {
            $crate::ffi_glue::ime_state_json(handle)
        }

        /// `frust_string_free`: release a C string returned by
        /// `frust_ime_state_json`, `frust_take_clipboard_write`,
        /// `frust_selection_toolbar_json` or
        /// `frust_platform_view_commands_json`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_string_free(s: *mut ::core::ffi::c_char) {
            $crate::ffi_glue::string_free(s)
        }

        /// `frust_edit_command`: deliver one clipboard/selection verb from the
        /// system edit menu or a hardware-keyboard chord.
        ///
        /// `cmd` is a fixed numeric ABI shared with the Swift `FrustView` edit
        /// actions — `0`=copy, `1`=cut, `2`=paste, `3`=select-all, **do not
        /// renumber**; an unrecognized code is dropped rather than guessed at.
        /// `text` is the pasted UTF-8 payload, read for `2` only (null
        /// otherwise) — see `ffi_glue::edit_command`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_edit_command(
            handle: *mut ::core::ffi::c_void,
            cmd: u8,
            text: *const ::core::ffi::c_char,
        ) {
            $crate::ffi_glue::edit_command(handle, cmd, text)
        }

        /// `frust_take_clipboard_write`: take (and clear) the text a widget
        /// asked to put on the host pasteboard — a heap-allocated C string the
        /// caller must release with `frust_string_free`, or null when nothing
        /// was copied. Destructive: draining and dropping the result loses the
        /// write (see `ffi_glue::take_clipboard_write`).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_take_clipboard_write(
            handle: *mut ::core::ffi::c_void,
        ) -> *mut ::core::ffi::c_char {
            $crate::ffi_glue::take_clipboard_write(handle)
        }

        /// `frust_take_paste_request`: take (and clear) whether a widget asked
        /// the shell to read the host pasteboard back to it. `1` = asked,
        /// `0` = not (a plain `u8`, mirroring `frust_set_appearance`'s `dark`).
        /// Effectively unused on iOS, and deliberately so — see
        /// `ffi_glue::take_paste_request` for why this is the one paste route
        /// the platform does *not* exempt from its permission alert.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_take_paste_request(handle: *mut ::core::ffi::c_void) -> u8 {
            $crate::ffi_glue::take_paste_request(handle)
        }

        /// `frust_selection_toolbar_json`: where the system edit menu should be
        /// anchored and which verbs it may offer, as a heap-allocated JSON C
        /// string the caller must release with `frust_string_free` — or null
        /// when no field has a selection worth a menu. The anchor is in logical
        /// points, the same space as the IME JSON's caret rect (see
        /// `ffi_glue::selection_toolbar_json`).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_selection_toolbar_json(
            handle: *mut ::core::ffi::c_void,
        ) -> *mut ::core::ffi::c_char {
            $crate::ffi_glue::selection_toolbar_json(handle)
        }

        /// `frust_pause`: app backgrounded — stop submitting frames.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_pause(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::pause(handle)
        }

        /// `frust_resume`: app foregrounded — resume submitting frames.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_resume(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::resume(handle)
        }

        /// `frust_destroy`: release the native handle (and its surface).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_destroy(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::destroy(handle)
        }

        /// `frust_set_appearance`: flip the app's theme brightness (a
        /// dark-mode change) between light and dark. `dark` is `0`/`1`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_set_appearance(handle: *mut ::core::ffi::c_void, dark: u8) {
            $crate::ffi_glue::set_appearance(handle, dark)
        }

        /// `frust_set_reduce_motion`: apply the platform's reduced-motion
        /// accessibility preference (`UIAccessibility.isReduceMotionEnabled`)
        /// to the active theme's motion tokens. `reduce` is `0`/`1`,
        /// mirroring `frust_set_appearance`'s `dark: u8`.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_set_reduce_motion(handle: *mut ::core::ffi::c_void, reduce: u8) {
            $crate::ffi_glue::set_reduce_motion(handle, reduce)
        }

        /// `frust_set_insets`: deliver the platform window insets
        /// (SafeArea). The eight `f32`s are `view_padding` (`vp_*`: from
        /// `safeAreaInsets`) then `view_insets` (`vi_*`: the keyboard frame),
        /// each l/t/r/b, in **logical points** (no scale division — the same
        /// asymmetry `frust_dispatch_touch` uses). Swift calls this from
        /// `safeAreaInsetsDidChange` and the keyboard-frame notifications.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        #[allow(clippy::too_many_arguments)]
        pub extern "C" fn frust_set_insets(
            handle: *mut ::core::ffi::c_void,
            vp_l: f32,
            vp_t: f32,
            vp_r: f32,
            vp_b: f32,
            vi_l: f32,
            vi_t: f32,
            vi_r: f32,
            vi_b: f32,
        ) {
            $crate::ffi_glue::set_insets(handle, vp_l, vp_t, vp_r, vp_b, vi_l, vi_t, vi_r, vi_b)
        }

        /// `frust_on_deep_link`: deliver a platform deep link (cold-start
        /// or running) into the process-wide deep-link source.
        /// `url` is the URL's `absoluteString` as a UTF-8 C string.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_on_deep_link(
            handle: *mut ::core::ffi::c_void,
            url: *const ::core::ffi::c_char,
        ) {
            $crate::ffi_glue::on_deep_link(handle, url)
        }

        /// `frust_system_ui_state`: peek the process-wide system-UI override
        /// slot for `FrustViewController`'s per-frame poll — see
        /// `ffi_glue::system_ui_state`'s doc comment for the returned `u64`'s
        /// packing scheme and the iOS status-bar/home-indicator mapping table.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_system_ui_state(handle: *mut ::core::ffi::c_void) -> u64 {
            $crate::ffi_glue::system_ui_state(handle)
        }

        /// `frust_set_surface_mode`: latch the process-wide translucent-surface
        /// opt-in — callable BEFORE `frust_init`.
        /// `translucent` is `0`/`1` (no `<stdbool.h>` precedent in this crate's
        /// ABI, mirroring `frust_set_appearance`'s `dark: u8`). Takes no handle:
        /// the slot is process-global.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_set_surface_mode(translucent: u8) {
            $crate::ffi_glue::set_surface_mode(translucent)
        }

        /// `frust_platform_view_commands_json`: the platform-view command
        /// backlog as a heap-allocated JSON C string
        /// the caller must release with `frust_string_free`. `ack_generation`
        /// is the generation the caller last finished applying (`0` on the
        /// first call); returns null on the no-change fast path
        /// (`generation == ack_generation`) or with no live handle.
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_platform_view_commands_json(
            handle: *mut ::core::ffi::c_void,
            ack_generation: u64,
        ) -> *mut ::core::ffi::c_char {
            $crate::ffi_glue::platform_view_commands_json(handle, ack_generation)
        }

        /// `frust_set_present_sync`: latch the process-wide present-sync
        /// opt-in — callable BEFORE `frust_init`, like
        /// `frust_set_surface_mode`, and paired in the same host branch with
        /// `CAMetalLayer.presentsWithTransaction = true`. `enabled` is `0`/`1`.
        /// Armed, the render thread hands each submitted frame to the UI
        /// thread, which presents it via `frust_present_frame` inside the
        /// `CATransaction` that commits platform-view geometry — so frust's
        /// surface and its native siblings land in one visual frame with the
        /// render-thread split still on (see `ffi_glue::set_present_sync`).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_set_present_sync(enabled: u8) {
            $crate::ffi_glue::set_present_sync(enabled)
        }

        /// `frust_present_frame`: present the frame the render thread parked
        /// for the UI thread. Called once per
        /// `CADisplayLink` tick, right after `FrustViewHost.poll`. A no-op
        /// when present-sync is unarmed, on the inline render path, or on a
        /// gate-skipped tick (see `ffi_glue::present_frame`).
        #[cfg(target_os = "ios")]
        #[unsafe(no_mangle)]
        pub extern "C" fn frust_present_frame(handle: *mut ::core::ffi::c_void) {
            $crate::ffi_glue::present_frame(handle)
        }
    };
}
