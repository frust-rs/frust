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
// through `$crate` here so the macro's `$crate::new_boxed_app`/
// `$crate::new_boxed_app_with`/`$crate::AppTree` paths resolve. Android-only: the
// macro body is never expanded off-Android (its call site is
// `#[cfg(target_os = "android")]`).
#[cfg(target_os = "android")]
#[doc(hidden)]
pub use forgekit_shell_common::{AppTree, new_boxed_app_with};

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

/// Bind a generated app's `State`/`app_logic` to the fixed Android JNI exports
/// (spec §10.1, Makepad `app_main!` precedent).
///
/// Stamps out the sixteen `Java_dev_forgekit_ForgeKitSurfaceView_native*` symbols
/// the Kotlin `ForgeKitSurfaceView` declares `external`, each delegating to the
/// non-generic runtime in [`jni_glue`]. `nativeInit` constructs the app's erased
/// view tree from a state factory and `$app_logic`; the rest operate on the
/// opaque `jlong` handle. The three IME exports (`nativeImeApply`,
/// `nativeImeState`, `nativeImeAction`) carry the Phase 4B soft-keyboard
/// state-sync contract (spec §14 Phase 4): Kotlin pushes a whole editing state in
/// (`nativeImeApply`), pulls the reconciled state back out (`nativeImeState`), and
/// forwards an editor action (Enter) via `nativeImeAction`. `nativeSetAppearance`
/// (task 08) flips the app's theme brightness from the platform's dark-mode
/// preference. `nativeOnDeepLink` (task 07) delivers a cold-start/running
/// platform deep link into the process-wide deep-link source.
/// `nativeInitAccessibility` (phase 6d, task 04) attaches the accesskit Android
/// adapter to the host view. The two device-parity exports (task 06):
/// `nativeOnInsetsChanged` delivers the platform window insets (SafeArea), and
/// `nativeOnBackPress` routes a hardware/gesture back press through the framework
/// (returning whether it consumed it). `nativeOnSurfaceChanged` also gained a
/// trailing `density` argument in the same task. `nativeInit` also
/// initializes the process-wide [`forgekit_reactive::ReactiveRuntime`] (see
/// [`jni_glue::native_init`]) before the state factory runs, so a `State`'s own
/// construction may already create signals/controllers.
///
/// The macro is defined on every target but only *expands* to real code where
/// its call site is gated, e.g. in the generated `src/lib.rs`:
///
/// ```ignore
/// #[cfg(target_os = "android")]
/// forgekit::android_app!(AppState, app_logic);
/// ```
///
/// Two forms:
/// - `android_app!($state_ty, $app_logic)` — `$state_ty` must implement
///   [`Default`]; the state is built via `<$state_ty as Default>::default`.
/// - `android_app!($state_ty, $state_init, $app_logic)` — `$state_init` is a
///   `FnOnce() -> $state_ty` factory (e.g. a closure or a bare function path
///   like `MyState::new`), for a `State` that doesn't implement `Default`. The
///   2-arg form delegates to this one.
///
/// `$app_logic` is a `FnMut(&mut State) -> impl View<State>`.
#[macro_export]
macro_rules! android_app {
    ($state_ty:ty, $app_logic:expr $(,)?) => {
        $crate::android_app!(
            $state_ty,
            <$state_ty as ::core::default::Default>::default,
            $app_logic
        );
    };
    ($state_ty:ty, $state_init:expr, $app_logic:expr $(,)?) => {
        /// JNI `nativeInit`: create the native handle for one surface.
        ///
        /// `cache_dir` is the app's `context.cacheDir.absolutePath` (task 13),
        /// used to persist the wgpu pipeline cache across launches so a warm
        /// start skips Vulkan shader-pipeline compilation. Same export name as
        /// before — the mangled JNI symbol does not encode Java-side params — but
        /// the Kotlin `external` declaration and this signature gained the
        /// `String cacheDir` parameter together (see `ForgeKitSurfaceView`).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeInit<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            surface: $crate::__jni::JObject<'local>,
            scale: $crate::__jni::jfloat,
            cache_dir: $crate::__jni::JString<'local>,
        ) -> $crate::__jni::jlong {
            $crate::jni_glue::native_init(env, surface, scale, cache_dir, || {
                $crate::new_boxed_app_with::<$state_ty, _, _, _>($state_init, $app_logic)
            })
        }

        /// JNI `nativeOnSurfaceChanged`: (re)create or resize the surface.
        ///
        /// `density` (task 06) is the display's `displayMetrics.density` for this
        /// configuration — a **breaking** signature change from the pre-parity
        /// export (the Kotlin `external` declaration gains the trailing argument
        /// in the same phase, task 08) so a density-altering config change
        /// re-sanitizes the stored scale used for layout/paint/insets.
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
            density: $crate::__jni::jfloat,
        ) {
            $crate::jni_glue::native_on_surface_changed(
                env, handle, surface, width, height, density,
            )
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

        /// JNI `nativeOnTouch`: deliver one touch contact (spec §9).
        ///
        /// `action` is the normalised phase code (`0`=down, `1`=move, `2`=up,
        /// `3`=cancel — an ABI shared with the Kotlin `ForgeKitSurfaceView`);
        /// `x`/`y` are physical view-local pixels.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnTouch<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            action: $crate::__jni::jint,
            x: $crate::__jni::jfloat,
            y: $crate::__jni::jfloat,
        ) {
            $crate::jni_glue::native_on_touch(handle, action, x, y)
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

        /// JNI `nativeImeApply`: push a whole platform editing state into the
        /// focused widget (the mobile IME state-sync path, spec §14 Phase 4).
        ///
        /// `text` is the Kotlin mirror `Editable`'s content; the four indices are
        /// **UTF-16 code units** (Java-native) and cross the seam unchanged — the
        /// focused widget converts them to Rust byte offsets.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeImeApply<'local>(
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
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeImeState<'local>(
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
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeImeAction<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            action: $crate::__jni::jint,
        ) {
            $crate::jni_glue::native_ime_action(handle, action)
        }

        /// JNI `nativeSetAppearance`: flip the app's theme brightness (config/
        /// uiMode change) between light and dark (task 08).
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeSetAppearance<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            dark: $crate::__jni::jboolean,
        ) {
            $crate::jni_glue::native_set_appearance(handle, dark)
        }

        /// JNI `nativeOnDeepLink`: deliver a platform deep link (cold-start or
        /// running — task 07) into the process-wide deep-link source.
        ///
        /// `url` is the Kotlin `Intent.data` `Uri`'s `toString()`.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnDeepLink<'local>(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            url: $crate::__jni::JString<'local>,
        ) {
            $crate::jni_glue::native_on_deep_link(env, handle, url)
        }

        /// JNI `nativeInitAccessibility`: attach the accesskit Android adapter to
        /// the host `ForgeKitSurfaceView` (spec §9, phase 6d — task 04).
        ///
        /// `view` is the host `View` (`this`); called once by Kotlin's
        /// `surfaceCreated` right after `nativeInit`. Best-effort and isolated in
        /// its own guarded entry so an a11y-init failure never blocks startup.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeInitAccessibility<
            'local,
        >(
            env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
            view: $crate::__jni::JObject<'local>,
        ) {
            $crate::jni_glue::native_init_accessibility(env, handle, view)
        }

        /// JNI `nativeOnInsetsChanged`: deliver the platform window insets
        /// (device px, task 06 — RESEARCH.md "Insets / SafeArea").
        ///
        /// The eight `jfloat`s are `view_padding` (`vp_*`: system-bar/cutout
        /// occlusion) then `view_insets` (`vi_*`: the IME area), each l/t/r/b.
        /// Kotlin reads them from `WindowInsetsCompat` in a
        /// `setOnApplyWindowInsetsListener` and forwards them here; the handle
        /// converts device→logical px and pushes them onto the render root.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnInsetsChanged<
            'local,
        >(
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

        /// JNI `nativeOnBackPress`: the Android back contract (task 06 —
        /// RESEARCH.md "Android back"). Returns `JNI_TRUE` when the framework
        /// consumed the press (it will pop on the next rebuild — Kotlin must not
        /// finish the activity), `JNI_FALSE` when it should fall through to the
        /// default `OnBackPressedDispatcher` (activity finish). Kotlin calls this
        /// from an `OnBackPressedCallback` whose `isEnabled` it toggles off the
        /// framework's advertised `handles_back` answer.
        #[unsafe(no_mangle)]
        pub extern "system" fn Java_dev_forgekit_ForgeKitSurfaceView_nativeOnBackPress<'local>(
            _env: $crate::__jni::EnvUnowned<'local>,
            _class: $crate::__jni::JClass<'local>,
            handle: $crate::__jni::jlong,
        ) -> $crate::__jni::jboolean {
            $crate::jni_glue::native_on_back_press(handle)
        }
    };
}
