//! Android platform shell: the Rust half of the JNI bridge.
//!
//! This crate is the Android counterpart to `frust-shell-desktop`. Where the
//! desktop shell owns a `winit` event loop, the Android shell is *driven* by the
//! Kotlin `FrustSurfaceView` (see `platform/android/frust-embedding/src/main/kotlin/dev/frust/
//! FrustSurfaceView.kt`): the JVM calls a fixed set of
//! `Java_dev_frust_FrustSurfaceView_native*` symbols, and each generated
//! app supplies its own `State`/`build` through the [`android_app!`] macro,
//! which stamps out those symbols bound to the app's types.
//!
//! # Layering
//!
//! Like the desktop shell, this is an integration crate: on Android it composes
//! `frust-core` (the retained tree / `RenderRoot`), `frust-scene`,
//! `frust-render` (the `SurfaceRenderer` §8.1 state machine) and
//! `frust-text`, plus the `jni`/`ndk` FFI crates. The platform-agnostic
//! plumbing (the `AppTree` erasure and the `guard`/`sanitize_scale`/
//! `logical_size` helpers) is reused from `frust-shell-common` rather than
//! duplicated. All of the JNI/render composition is
//! `#[cfg(target_os = "android")]`; on every other target only the macro
//! definition and one pure host-testable helper ([`ffi_support`]) compile, so
//! the crate is inert in a host `cargo test --workspace`.
//!
//! # GPU-device hand-back (`gpu` feature)
//!
//! This shell's `gpu` Cargo feature forwards into `frust-shell-common/gpu`
//! and `frust-render/gpu` (the process-wide device slot and its
//! `DeviceHandle` re-export — see `frust-shell-common`'s `gpu` module and
//! `crates/frust/src/lib.rs`'s `gpu` module), the same seam
//! `frust-shell-desktop`'s render executor installs its live device into.
//! **This shell does not install one yet** — its render-thread device
//! creation lives in `jni_glue`'s pre-init/`create_handle` path and the
//! `app::executor` render loop, wiring an install call there is a
//! render-thread-split-aware change of its own, not a one-line addition —
//! so `frust::gpu::with_context` currently answers `None` on Android even
//! with `--features gpu` on. An accepted, explicitly tracked gap rather than
//! a half-built accessor.
//!
//! # Unsafe
//!
//! This crate is a sanctioned `unsafe` zone: the framework crates
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

// The shape-aware scroll-sync tail: pure logic — two scalars
// per display frame in, a hold depth out — deliberately kept out of the
// Android-only modules below so its regime state machine and hold aging are
// unit-testable on the host (the same "platform-agnostic brain" split the
// frame-id gate's `FramePairing` uses in `frust-shell-common`). Only the
// (Android-only) frame loop drives it, hence the same dead-code shape as
// [`ffi_support`].
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod sync_tail;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
pub mod jni_glue;

// Re-exports the macro-generated `extern "system" fn`s reach for, so a generated
// app only needs to depend on `frust` (which re-exports `android_app!`), never
// on `jni`/`frust-shell-common`/`frust-shell-android` directly. The
// platform-agnostic erasure lives in `frust-shell-common`; it is surfaced
// through `$crate` here so the macro's `$crate::new_boxed_app`/
// `$crate::new_boxed_app_with`/`$crate::AppTree` paths resolve. Android-only: the
// macro body is never expanded off-Android (its call site is
// `#[cfg(target_os = "android")]`).
#[cfg(target_os = "android")]
#[doc(hidden)]
pub use frust_shell_common::{AppTree, new_boxed_app_with};

/// Re-exported `jni` types used verbatim by [`android_app!`]'s expansion.
///
/// Referencing them through `$crate::__jni` keeps the generated `extern` symbols
/// free of any assumption that the app crate depends on `jni` directly.
#[cfg(target_os = "android")]
#[doc(hidden)]
pub mod __jni {
    // `EnvUnowned` is the FFI-safe env type for native-method arguments in
    // `jni` 0.22 (the bare `JNIEnv` alias is deprecated). For the surface/touch
    // exports we only read its raw pointer to hand to the NDK; the IME exports
    // additionally call the JNI string API through it (`EnvUnowned::with_env`) to
    // read/return a `java.lang.String` — see [`crate::jni_glue`].
    pub use jni::EnvUnowned;
    pub use jni::objects::{JClass, JObject, JString};
    pub use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
}

/// Bind a generated app's `State`/`build` to the fixed Android JNI exports
/// (a Makepad `app_main!` precedent).
///
/// Stamps out the twenty-two `Java_dev_frust_FrustSurfaceView_native*` symbols
/// the Kotlin `FrustSurfaceView` declares `external`, each delegating to the
/// non-generic runtime in [`jni_glue`]. `nativeInit` constructs the app's erased
/// view tree from a state factory and `$build`; the rest operate on the
/// opaque `jlong` handle. The three IME exports (`nativeImeApply`,
/// `nativeImeState`, `nativeImeAction`) carry the soft-keyboard
/// state-sync contract: Kotlin pushes a whole editing state in
/// (`nativeImeApply`), pulls the reconciled state back out (`nativeImeState`), and
/// forwards an editor action (Enter) via `nativeImeAction`. `nativeSetAppearance`
/// flips the app's theme brightness from the platform's dark-mode
/// preference, and `nativeSetReduceMotion` does the same for the platform's
/// reduced-motion accessibility preference (a different sensor over the same
/// transport — `Settings.Global.ANIMATOR_DURATION_SCALE`, not
/// `Configuration`). `nativeAppIsDark` is the read half of the appearance
/// seam: it returns whether the app's currently active theme is dark, so
/// Kotlin's status-bar icon contrast can follow the APP's resolved theme
/// (which an app-forced `frust::set_app_theme` override may have pinned away
/// from the platform's own preference) instead of re-reading
/// `Configuration.uiMode` directly. `nativeOnDeepLink` delivers a cold-start/running
/// platform deep link into the process-wide deep-link source.
/// `nativeInitAccessibility` attaches the accesskit Android
/// adapter to the host view. Two more exports:
/// `nativeOnInsetsChanged` delivers the platform window insets (SafeArea), and
/// `nativeOnBackPress` routes a hardware/gesture back press through the framework
/// (returning whether it consumed it). `nativeOnSurfaceChanged` also gained a
/// trailing `density` argument alongside these. `nativeSystemUiState`
/// returns the app-facing `frust::set_system_ui_mode` override slot's
/// packed `(generation, mode)` state for a per-frame Kotlin poll. Two more,
/// additive over those seventeen: `nativeSetSurfaceMode`
/// latches a translucent GPU surface pre-init (a one-way opt-in — see
/// `frust_shell_common::surface_mode`'s module docs), and
/// `nativePlatformViewCommands` returns the native-sibling-compositor command
/// backlog (the differ) as JSON for Kotlin's own per-frame poll, mirroring
/// `nativeSystemUiState`'s generation-gated shape but JSON-encoded (the
/// `ViewCommand` vocabulary) rather than packed into a `jlong`. One more:
/// `nativeSetFrameTimeline` pushes the Choreographer frame
/// timeline's `expectedPresentationTimeNanos − frameTimeNanos` (API 33+, the
/// one signal only the JVM side can read) into the scroll-sync tail — additive
/// and optional, `0`/never-called leaves the platform-view release path
/// gate-only. `nativeInit` also
/// initializes the process-wide [`frust_reactive::ReactiveRuntime`] (see
/// [`jni_glue::native_init`]) before the state factory runs, so a `State`'s own
/// construction may already create signals/controllers.
///
/// The macro is defined on every target but only *expands* to real code where
/// its call site is gated, e.g. in the generated `src/lib.rs`:
///
/// ```ignore
/// #[cfg(target_os = "android")]
/// frust::android_app!(AppState, move |s| root.build(s));
/// ```
///
/// Two forms:
/// - `android_app!($state_ty, $build)` — `$state_ty` must implement
///   [`Default`]; the state is built via `<$state_ty as Default>::default`.
/// - `android_app!($state_ty, $state_init, $build)` — `$state_init` is a
///   `FnOnce() -> $state_ty` factory (e.g. a closure or a bare function path
///   like `MyState::new`), for a `State` that doesn't implement `Default`. The
///   2-arg form delegates to this one.
///
/// `$build` is the root component's build closure, a `FnMut(&mut State) -> impl View<State>` (`move |s| root.build(s)`).
#[macro_export]
macro_rules! android_app {
    ($state_ty:ty, $build:expr $(,)?) => {
        $crate::android_app!(
            $state_ty,
            <$state_ty as ::core::default::Default>::default,
            $build
        );
    };
    ($state_ty:ty, $state_init:expr, $build:expr $(,)?) => {
        /// JNI `nativeInit`: create the native handle for one surface.
        ///
        /// `cache_dir` is the app's `context.cacheDir.absolutePath`,
        /// used to persist the wgpu pipeline cache across launches so a warm
        /// start skips Vulkan shader-pipeline compilation. Same export name as
        /// before — the mangled JNI symbol does not encode Java-side params — but
        /// the Kotlin `external` declaration and this signature gained the
        /// `String cacheDir` parameter together (see `FrustSurfaceView`).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeInit<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            surface: $crate::__jni::JObject<'local>,
            scale: $crate::__jni::jfloat,
            cache_dir: $crate::__jni::JString<'local>,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_init(env, surface, scale, cache_dir, || {
                $crate::new_boxed_app_with::<$state_ty, _, _, _>($state_init, $build)
            })
        }

        /// JNI `nativeOnSurfaceChanged`: (re)create or resize the surface.
        ///
        /// `density` is the display's `displayMetrics.density` for this
        /// configuration — a **breaking** signature change from the pre-parity
        /// export (the Kotlin `external` declaration gains the trailing argument
        /// alongside it) so a density-altering config change
        /// re-sanitizes the stored scale used for layout/paint/insets.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnSurfaceChanged<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            surface: $crate::__jni::JObject<'local>,
            width: $crate::__jni::jint,
            height: $crate::__jni::jint,
            density: $crate::__jni::jfloat,
        ) {
            $crate::jni_glue::native_on_surface_changed(
                env, handle, surface, width, height, density,
            )
        }

        /// JNI `nativeOnSurfaceDestroyed`: tear the surface down.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnSurfaceDestroyed<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_surface_destroyed(handle)
        }

        /// JNI `nativeOnFrame`: run one Choreographer-driven frame.
        ///
        /// Returns `jboolean`: `false` = a fatal render-thread
        /// failure (first-surface install could not succeed), on which Kotlin's
        /// `doFrame` stops the Choreographer loop. The JNI symbol name is
        /// unchanged; the Kotlin `external` declaration gains the `Boolean` return
        /// in lockstep (the `nativeOnSurfaceChanged`-density precedent).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnFrame<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            frame_time_nanos: $crate::__jni::jlong,
        ) -> $crate::__jni::jboolean {
            $crate::jni_glue::native_on_frame(handle, frame_time_nanos)
        }

        /// JNI `nativeOnTouch`: deliver one touch contact.
        ///
        /// `action` is the normalised phase code (`0`=down, `1`=move, `2`=up,
        /// `3`=cancel — an ABI shared with the Kotlin `FrustSurfaceView`);
        /// `pointer_id` is that contact's own `MotionEvent.pointerId`; `x`/`y`
        /// are physical view-local pixels. A BREAKING signature change
        /// (`(JIIFF)V`, was `(JIFF)V`) — the Kotlin `external` declaration
        /// gains the `pointerId` argument alongside it, carrying every active
        /// contact rather than only the primary one.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnTouch<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            action: $crate::__jni::jint,
            pointer_id: $crate::__jni::jint,
            x: $crate::__jni::jfloat,
            y: $crate::__jni::jfloat,
        ) {
            $crate::jni_glue::native_on_touch(handle, action, pointer_id, x, y)
        }

        /// JNI `nativeOnResume`: activity resumed (bookkeeping only in v0).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnResume<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_resume(handle)
        }

        /// JNI `nativeOnPause`: activity paused (bookkeeping only in v0).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnPause<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_pause(handle)
        }

        /// JNI `nativeOnDestroy`: release the native handle.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnDestroy<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_on_destroy(handle)
        }

        /// JNI `nativeImeApply`: push a whole platform editing state into the
        /// focused widget (the mobile IME state-sync path).
        ///
        /// `text` is the Kotlin mirror `Editable`'s content; the four indices are
        /// **UTF-16 code units** (Java-native) and cross the seam unchanged — the
        /// focused widget converts them to Rust byte offsets.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeImeApply<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            text: $crate::__jni::JString<'local>,
            sel_base: $crate::__jni::jint,
            sel_ext: $crate::__jni::jint,
            comp_base: $crate::__jni::jint,
            comp_ext: $crate::__jni::jint,
        ) {
            $crate::jni_glue::native_ime_apply(
                env, handle, text, sel_base, sel_ext, comp_base, comp_ext,
            )
        }

        /// JNI `nativeImeState`: return the focused widget's published IME surface
        /// as JSON (`active`/text/selection/composing + logical-px caret) for the
        /// Kotlin side to reconcile against its mirror and drive the `IMM`.
        ///
        /// Returns `null` (a null `jstring`) when there is no live native handle.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeImeState<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jstring {
            $crate::jni_glue::native_ime_state(env, handle)
        }

        /// JNI `nativeImeAction`: forward a soft-keyboard editor action
        /// (`performEditorAction`, e.g. `IME_ACTION_DONE`) as an `Enter` key press
        /// down the focus path.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeImeAction<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            action: $crate::__jni::jint,
        ) {
            $crate::jni_glue::native_ime_action(handle, action)
        }

        /// JNI `nativeSetAppearance`: flip the app's theme brightness (config/
        /// uiMode change) between light and dark.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeSetAppearance<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            dark: $crate::__jni::jboolean,
        ) {
            $crate::jni_glue::native_set_appearance(handle, dark)
        }

        /// JNI `nativeAppIsDark`: the read half of the appearance seam —
        /// return whether the APP's currently active theme is dark right now,
        /// for Kotlin to drive `updateSystemBarsAppearance` from instead of
        /// re-reading `Configuration.uiMode` directly (the
        /// device and the app can legitimately disagree once an app-forced
        /// `frust::set_app_theme` override or a design system's seeded
        /// default is in play). Additive: older generated Kotlin that never
        /// calls this is unaffected.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeAppIsDark<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jboolean {
            $crate::jni_glue::native_app_is_dark(handle)
        }

        /// JNI `nativeSetReduceMotion`: apply the platform's reduced-motion
        /// accessibility preference (`Settings.Global.ANIMATOR_DURATION_SCALE
        /// == 0`) to the active theme's motion tokens.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeSetReduceMotion<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            reduce: $crate::__jni::jboolean,
        ) {
            $crate::jni_glue::native_set_reduce_motion(handle, reduce)
        }

        /// JNI `nativeOnDeepLink`: deliver a platform deep link (cold-start or
        /// running) into the process-wide deep-link source.
        ///
        /// `url` is the Kotlin `Intent.data` `Uri`'s `toString()`.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnDeepLink<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            url: $crate::__jni::JString<'local>,
        ) {
            $crate::jni_glue::native_on_deep_link(env, handle, url)
        }

        /// JNI `nativeInitAccessibility`: attach the accesskit Android adapter to
        /// the host `FrustSurfaceView`.
        ///
        /// `view` is the host `View` (`this`); called once by Kotlin's
        /// `surfaceCreated` right after `nativeInit`. Best-effort and isolated in
        /// its own guarded entry so an a11y-init failure never blocks startup.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeInitAccessibility<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            view: $crate::__jni::JObject<'local>,
        ) {
            $crate::jni_glue::native_init_accessibility(env, handle, view)
        }

        /// JNI `nativeOnInsetsChanged`: deliver the platform window insets
        /// (device px).
        ///
        /// The eight `jfloat`s are `view_padding` (`vp_*`: system-bar/cutout
        /// occlusion) then `view_insets` (`vi_*`: the IME area), each l/t/r/b.
        /// Kotlin reads them from `WindowInsetsCompat` in a
        /// `setOnApplyWindowInsetsListener` and forwards them here; the handle
        /// converts device→logical px and pushes them onto the render root.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnInsetsChanged<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            vp_l: $crate::__jni::jfloat,
            vp_t: $crate::__jni::jfloat,
            vp_r: $crate::__jni::jfloat,
            vp_b: $crate::__jni::jfloat,
            vi_l: $crate::__jni::jfloat,
            vi_t: $crate::__jni::jfloat,
            vi_r: $crate::__jni::jfloat,
            vi_b: $crate::__jni::jfloat,
        ) {
            $crate::jni_glue::native_on_insets_changed(
                handle, vp_l, vp_t, vp_r, vp_b, vi_l, vi_t, vi_r, vi_b,
            )
        }

        /// JNI `nativeOnBackPress`: the Android back contract. Returns
        /// `JNI_TRUE` when the framework
        /// consumed the press (it will pop on the next rebuild — Kotlin must not
        /// finish the activity), `JNI_FALSE` when it should fall through to the
        /// default `OnBackPressedDispatcher` (activity finish). The generated
        /// template's `MainActivity` registers an **always-enabled**
        /// `OnBackPressedCallback` that calls this via
        /// `FrustSurfaceView.dispatchBackPress()`; on a `false` return it
        /// momentarily disables itself, re-dispatches `onBackPressed()` to fall
        /// through to the platform default, then re-enables — the standard
        /// consume-or-fall-through idiom (the callback's `isEnabled` is NOT bound
        /// to the framework's `handles_back` answer; the Rust side owns that
        /// decision per call).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeOnBackPress<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jboolean {
            $crate::jni_glue::native_on_back_press(handle)
        }

        /// JNI `nativeSystemUiState`: return the process-wide system-UI
        /// override slot's packed `(generation, mode)` state for
        /// Kotlin's `doFrame` to poll each frame, generation-gated like
        /// `nativeImeState`'s established per-frame-poll idiom, and apply via
        /// `WindowInsetsControllerCompat` on change (the Kotlin
        /// decoder). Additive: older generated Kotlin that never calls this is
        /// unaffected.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeSystemUiState<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_system_ui_state(handle)
        }

        /// JNI `nativeFocusGeneration`: the focus/IME session generation — a
        /// counter over changes to the published focus/IME surface. Retained
        /// for Kotlin generated before `nativeFocusEpoch`, which is what the
        /// embedding's clipboard resolver binds to now: this one stands still
        /// when focus crosses between two fields that publish alike, and moves
        /// for an edit that never left the field (see
        /// `jni_glue::native_focus_generation`). A missing handle returns `0`,
        /// which is ALSO a reachable live value here — do not read this
        /// export's `0` as "no session". Additive: older generated Kotlin that
        /// never calls this is unaffected.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeFocusGeneration<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_focus_generation(handle)
        }

        /// JNI `nativeFocusEpoch`: the live focus session's identity, so the
        /// Kotlin clipboard resolver can bind an async URI-backed paste to the
        /// session that asked for it. `EditCommand::Paste` is focus-routed, so
        /// a slow `ContentProvider`'s answer would otherwise land in whatever
        /// field holds focus when it arrives. Advanced once per honoured focus
        /// claim and once per session release, so it moves when focus crosses
        /// fields (published surface or not) and stands still through an edit
        /// inside one session. A missing handle returns `0`, which no live
        /// root ever reports (see `jni_glue::native_focus_epoch`). Additive:
        /// older generated Kotlin that never calls this is unaffected.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeFocusEpoch<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_focus_epoch(handle)
        }

        /// JNI `nativeSetSurfaceMode`: latch a translucent (alpha-channel) GPU
        /// surface before it is created. Takes no
        /// handle — a process-wide, pre-init-only opt-in the generated glue
        /// calls BEFORE `nativeInit`; a call after a
        /// surface already exists in this process is logged and has no
        /// effect on it.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeSetSurfaceMode<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            translucent: $crate::__jni::jboolean,
        ) {
            $crate::jni_glue::native_set_surface_mode(translucent)
        }

        /// JNI `nativePlatformViewCommands`: return the native-sibling-
        /// compositor command backlog (the differ) as JSON for
        /// Kotlin's per-frame poll.
        /// `ack_generation` round-trips the generation Kotlin's own last
        /// poll returned (`0` on the first call); returns `null` on the
        /// no-change fast path or a missing handle.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativePlatformViewCommands<
            'local,
        >(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            ack_generation: $crate::__jni::jlong,
        ) -> $crate::__jni::jstring {
            $crate::jni_glue::native_platform_view_commands(env, handle, ack_generation)
        }

        /// JNI `nativeSetFrameTimeline`: push this tick's Choreographer
        /// frame-timeline delta (`expectedPresentationTimeNanos −
        /// frameTimeNanos`, API 33+) into the scroll-sync tail.
        /// `0` means "no sample" — below API 33, or no platform view is
        /// hosted, in which case Kotlin never samples the timeline at all —
        /// and leaves the platform-view release path gate-only. The value is
        /// only ever a hold *depth* input; nothing about correctness depends
        /// on it.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_frust_FrustSurfaceView_nativeSetFrameTimeline<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            expected_present_delta_nanos: $crate::__jni::jlong,
        ) {
            $crate::jni_glue::native_set_frame_timeline(handle, expected_present_delta_nanos)
        }
    };
}
