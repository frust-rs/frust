//! The Android backend: four JNI exports behind ONE generic Kotlin factory
//! and ONE generic listener — the whole platform surface this plugin will
//! ever need, however many controls [`crate::runtime`] grows.
//!
//! # The frozen Kotlin ↔ Rust contract
//!
//! Both classes live in `plugins/native-widgets/platform/android/`
//! (`dev.frust` package — see *Package* below). The package and class names
//! are baked into the mangled symbol names below, so **they may never move
//! once shipped** (`docs/CODE_STANDARDS.md`'s JNI-export naming LAW).
//!
//! ## `FrustNativeControlFactory` (a `dev.frust.FrustPlatformViewFactory`)
//!
//! | Kotlin | Export | Contract |
//! |---|---|---|
//! | `createView(activity, context, paramsJson)` | [`Java_dev_frust_FrustNativeControlFactory_nativeCreateControl`] | Rust builds the real control and returns it; `null` on any failure, which the host logs and treats as a dead slot |
//! | `updateParams(view, paramsJson)` | [`Java_dev_frust_FrustNativeControlFactory_nativeUpdateParams`] | Props change, Rust-diffed before any setter runs |
//! | `disposeView(view)` | [`Java_dev_frust_FrustNativeControlFactory_nativeDisposeControl`] | Paired-delete of every reference the control retained |
//!
//! ## `FrustNativeListener` (the generic listener glue)
//!
//! | Kotlin | Export | Contract |
//! |---|---|---|
//! | `nativeOnEvent(slotId, kind, detail)` | [`Java_dev_frust_FrustNativeListener_nativeOnEvent`] | ONE class, ONE native method, dispatched by `(slot id, kind)`; `detail` packs a primitive payload, never JSON on the hot path |
//!
//! ## Which call carries the slot id
//!
//! `createView`/`updateParams`/`disposeView` are the embedding's fixed
//! `FrustPlatformViewFactory` contract, and **none of them is passed the
//! differ's slot id**. So:
//!
//! - create/update read it out of `params_json`
//!   ([`crate::runtime::SLOT_KEY`], injected by the api layer) — the additive
//!   fix, rather than widening the factory interface;
//! - dispose has no params at all, so it resolves the instance **by object
//!   identity** (`Env::is_same_object` against each live control's retained
//!   view). That is also what makes a late dispose safe: after a replacement
//!   `Create` re-used the slot id, the stale dispose names a view no live
//!   instance has, finds nothing, and leaves the live control alone
//!   (`crate::runtime`'s *late and duplicate disposal*).
//!
//! # Threading
//!
//! Every export runs on the platform main thread: the host drives the factory
//! from its post-frame command poll (`FrustViewHost`, itself driven from
//! `FrustSurfaceView.doFrame`), and a `View` listener fires there too. Each
//! export debug-asserts it ([`debug_assert_main_thread`]) rather than trusting
//! the comment — a control's state and the runtime's registry are
//! thread-confined, not locked.
//!
//! # No unwind across FFI
//!
//! Every export body runs inside [`jni::EnvUnowned::with_env`], which wraps it
//! in `catch_unwind` and resolves to a benign default
//! (`LogErrorAndDefault`) — the crate's half of `docs/CODE_STANDARDS.md`'s
//! no-unwind-across-FFI rule, the same shape `frust-camera`'s exports use.
//! The only `unsafe` tokens in this module are the exports'
//! `#[unsafe(no_mangle)]` attributes.
//!
//! # Package
//!
//! The two Kotlin files sit in `dev.frust` (not a `dev.frust.nativewidgets`
//! subpackage) because v1 ships them as hand-copied app-module sources, the
//! camera-style manual wiring the plan defers to Phase 3's packaging work;
//! moving them later changes every symbol name below, so the decision is
//! deliberately recorded here.

mod ctx;
// Theme ladder L3 (p1-08): registering Glyph font bytes with Android and
// creating/caching the resulting `Typeface` objects. `pub(crate)`, not
// private like `ctx`: `controls::platform`'s `Setter::Typeface` apply arm
// needs `fonts::typeface_for` from OUTSIDE this module's own subtree, the
// same reason `theme` below is `pub(crate)`.
pub(crate) mod fonts;
// `pub(crate)`, not private like `ctx`: `controls::platform`'s
// `Setter::ThemedBackground` apply arm (theme ladder L2, p1-07) needs
// `theme::dp_to_px` from OUTSIDE this module's own subtree.
pub(crate) mod theme;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JObject, JString};
use jni::sys::{jint, jlong, jobject};
use jni::{Env, EnvUnowned};

pub(crate) use ctx::NativeCtx;

use crate::NativeWidgetError;
use crate::controls::{button, image, label, progress, slider, switch};
use crate::registry::SlotId;
use crate::runtime::{self, NativeEvent, NativeRuntime, UpdateOutcome};

/// A control's retained native references — the runtime's per-instance handle
/// on Android is exactly the registry's [`AndroidHandle`], so a control's
/// `create` returns the view it built plus every secondary reference
/// (listener, child views) it wants pair-deleted with it.
///
/// [`AndroidHandle`]: crate::registry::android::AndroidHandle
pub(crate) type NativeView = crate::registry::android::AndroidHandle;

/// Register every control kind this backend serves, once, when the thread's
/// runtime is first touched (`crate::runtime`'s `seeded_runtime`).
///
/// Registration is explicit and central by design — `inventory`-style
/// link-time discovery is banned here (RESEARCH-NATIVE-COMPONENT §Open
/// questions). This table and the api layer's builders (p1-06) are the two
/// ends of the same kind strings, which is why each one is a `KIND` const in
/// its own control module rather than a literal here.
pub(crate) fn register_controls(runtime: &mut NativeRuntime) {
    runtime.register::<button::Button>(button::KIND);
    runtime.register::<label::Label>(label::KIND);
    runtime.register::<switch::Switch>(switch::KIND);
    runtime.register::<slider::Slider>(slider::KIND);
    runtime.register::<progress::Progress>(progress::KIND);
    runtime.register::<image::Image>(image::KIND);
}

// --- exports: FrustNativeControlFactory -------------------------------------

/// `FrustNativeControlFactory.createView` → the Rust-built control.
///
/// Returns the new view as a local reference, or `null` when the params carry
/// no known control kind or the control's own `create` failed — the host
/// logs a `null`/exception and marks the slot dead rather than crashing its
/// frame loop.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeControlFactory_nativeCreateControl<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    params: JString<'local>,
    activity: JObject<'local>,
    context: JObject<'local>,
) -> jobject {
    let created: Option<jobject> = env
        .with_env(|env| -> Result<Option<jobject>, jni::errors::Error> {
            debug_assert_main_thread(env, "nativeCreateControl");
            let params = params.try_to_string(env)?;
            Ok(create_control(env, &params, &activity, &context))
        })
        .resolve::<LogErrorAndDefault>();
    created.unwrap_or(std::ptr::null_mut())
}

/// The typed half of [`Java_dev_frust_FrustNativeControlFactory_nativeCreateControl`]:
/// dispatch, build, retain, and hand a fresh local reference back to Kotlin.
///
/// Theme ladder L1 (p1-07): builds every control against a night-qualified
/// `Context` (`theme::night_qualified_context`) before dispatching to the
/// runtime — see `theme`'s module doc for why. A qualification failure
/// degrades to the factory's own unqualified `context` (logged), never a
/// reason to fail the whole create.
fn create_control<'local>(
    env: &mut Env<'local>,
    params: &str,
    activity: &JObject<'local>,
    context: &JObject<'local>,
) -> Option<jobject> {
    let dark = theme::brightness_is_dark(params);
    let qualified = theme::night_qualified_context(env, context, dark);
    let themed_context: &JObject<'local> = match &qualified {
        Ok(qualified) => qualified,
        Err(e) => {
            log::warn!(
                "frust-native-widgets: L1 night-qualified Context failed ({e}) — creating \
                 against the unqualified Context; the control still renders, only its \
                 platform-default chrome may resolve the wrong brightness"
            );
            context
        }
    };

    let outcome = runtime::with_runtime(|runtime| {
        let mut ctx = NativeCtx::for_create(env, activity, themed_context);
        let slot_id = runtime.create(&mut ctx, params)?;
        let handed_back = {
            let view = runtime
                .instance(slot_id)
                .expect("a successful create retained its instance")
                .view();
            ctx.run_jni("NewLocalRef(control)", |env| env.new_local_ref(&*view.view))
        };
        match handed_back {
            Ok(local) => Ok::<_, NativeWidgetError>((slot_id, local.into_raw())),
            Err(e) => {
                // Kotlin never receives this view, so `disposeView` will never
                // be called for it — roll the instance back here instead of
                // stranding its global references for the process lifetime.
                runtime.dispose_slot(&mut ctx, slot_id);
                Err(e)
            }
        }
    })?;

    match outcome {
        Ok((slot_id, raw)) => {
            log::debug!("frust-native-widgets: created slot {slot_id}");
            Some(raw)
        }
        Err(e) => {
            log::warn!("frust-native-widgets: createView failed: {e}");
            None
        }
    }
}

/// `FrustNativeControlFactory.updateParams` → a Rust-diffed props update.
///
/// The `view` argument is part of the factory contract but unused: the slot
/// id in `paramsJson` is authoritative (module doc's *Which call carries the
/// slot id*), and a params payload naming a slot with no live instance is
/// tolerated rather than an error.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeControlFactory_nativeUpdateParams<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    _view: JObject<'local>,
    params: JString<'local>,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        debug_assert_main_thread(env, "nativeUpdateParams");
        let params = params.try_to_string(env)?;
        runtime::with_runtime(|runtime| {
            let mut ctx = NativeCtx::detached(env);
            match runtime.update_params(&mut ctx, &params) {
                Ok(UpdateOutcome::Applied | UpdateOutcome::Unchanged) => {}
                Ok(UpdateOutcome::UnknownSlot) => {
                    log::debug!("frust-native-widgets: updateParams for a slot with no instance");
                }
                Err(e) => log::warn!("frust-native-widgets: updateParams failed: {e}"),
            }
        });
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `FrustNativeControlFactory.disposeView` → paired-delete of every reference
/// the control retained.
///
/// Resolution is by object identity, never by "whichever instance holds that
/// slot now" — see the module doc's *Which call carries the slot id*.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeControlFactory_nativeDisposeControl<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    view: JObject<'local>,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        debug_assert_main_thread(env, "nativeDisposeControl");
        dispose_control(env, &view);
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// The typed half of [`Java_dev_frust_FrustNativeControlFactory_nativeDisposeControl`].
///
/// Identity is resolved **before** the teardown context exists: matching needs
/// only a shared `&Env` (`is_same_object`), while the control's own `dispose`
/// needs a `&mut` one.
fn dispose_control(env: &mut Env<'_>, view: &JObject<'_>) {
    let taken = runtime::with_runtime(|runtime| {
        runtime.take_matching(|retained| env.is_same_object(view, &*retained.view).unwrap_or(false))
    });

    let Some((slot_id, instance)) = taken.flatten() else {
        // Expected, not exceptional: a dispose can arrive after the slot was
        // already replaced or torn down (`crate::runtime`'s *late and
        // duplicate disposal*).
        log::debug!("frust-native-widgets: disposeView for an unknown view — ignored");
        return;
    };

    let mut ctx = NativeCtx::detached(env);
    if let Err(e) = instance.dispose(&mut ctx) {
        log::warn!("frust-native-widgets: disposing slot {slot_id}: {e}");
    }
    let live = runtime::with_runtime(|runtime| runtime.live_count()).unwrap_or_default();
    log::debug!("frust-native-widgets: disposed slot {slot_id}; live controls={live}");
}

// --- exports: FrustNativeListener -------------------------------------------

/// `FrustNativeListener`'s `onClick`/`onCheckedChanged`/
/// `onProgressChanged`/`onStartTrackingTouch`/`onStopTrackingTouch` → the
/// runtime's event dispatch, which decodes the raw `(kind, detail)` pair
/// into the typed `crate::events::EventPayload` vocabulary
/// (`crate::runtime::NativeWidget::on_event`) and hands it to the slot's
/// registered callback (`crate::runtime::NativeRuntime::set_callback`).
///
/// Runs on the main thread, so the control's handler — typically a signal
/// write, which wakes exactly one frust frame (SPIKE.md's receipt) — runs
/// there too.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_FrustNativeListener_nativeOnEvent<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    slot_id: jlong,
    kind: jint,
    detail: jlong,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        debug_assert_main_thread(env, "nativeOnEvent");
        let slot_id = slot_id as SlotId;
        let event = NativeEvent { kind, detail };
        let delivered = runtime::with_runtime(|runtime| runtime.on_event(slot_id, event));
        if delivered.is_none() {
            log::debug!("frust-native-widgets: event for slot {slot_id} dropped (re-entrant)");
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

// --- main-thread guard ------------------------------------------------------

/// Assert (debug builds only) that we are on the platform main thread.
///
/// The test is Android's own — `Looper.myLooper() == Looper.getMainLooper()`
/// — the camera backend's idiom, inverted: a plain JNI worker thread has no
/// `Looper` at all, so `myLooper()` returns null there and `IsSameObject`
/// against the never-null main looper is false. `android.os.Looper` is a
/// framework class, so the bare `find_class` works on any thread.
///
/// Debug-only: the two extra JNI calls are worth a loud failure while
/// developing (the whole runtime is thread-confined) and cost nothing in a
/// release build.
#[cfg(debug_assertions)]
fn debug_assert_main_thread(env: &mut Env<'_>, op: &str) {
    use jni::{jni_sig, jni_str};

    let on_main = ctx::run_jni(env, "Looper.myLooper", |env| {
        let looper = env.find_class(jni_str!("android/os/Looper"))?;
        let mine = env
            .call_static_method(
                &looper,
                jni_str!("myLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        let main = env
            .call_static_method(
                &looper,
                jni_str!("getMainLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        env.is_same_object(&mine, &main)
    });
    match on_main {
        Ok(true) => {}
        Ok(false) => {
            log::error!("frust-native-widgets: {op} called off the platform main thread");
            debug_assert!(false, "frust-native-widgets: {op} off the main thread");
        }
        Err(e) => log::warn!("frust-native-widgets: main-thread check for {op} failed: {e}"),
    }
}

/// Release builds skip the check entirely — see the debug counterpart.
#[cfg(not(debug_assertions))]
fn debug_assert_main_thread(_env: &mut Env<'_>, _op: &str) {}
