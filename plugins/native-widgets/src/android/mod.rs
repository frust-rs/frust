//! The Android backend: four JNI exports behind ONE generic Kotlin factory
//! and ONE generic listener — the whole platform surface this plugin will
//! ever need, however many controls [`crate::runtime`] grows.
//!
//! # The frozen Kotlin ↔ Rust contract
//!
//! Both classes live in this plugin's own Gradle library module,
//! `plugins/native-widgets/platform/android/src/main/kotlin/dev/frust/nativewidgets/`
//! (`dev.frust.nativewidgets` package — see *Package* below). The package and
//! class names are baked into the mangled symbol names below, so **they may
//! never move once shipped** (`docs/CODE_STANDARDS.md`'s JNI-export naming
//! LAW).
//!
//! ## `FrustNativeControlFactory` (a `dev.frust.FrustPlatformViewFactory`)
//!
//! | Kotlin | Export | Contract |
//! |---|---|---|
//! | `createView(activity, context, paramsJson)` | [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeCreateControl`] | Rust builds the real control and returns it; on any failure it throws a Java exception instead of returning `null` — honouring `FrustPlatformViewFactory`'s non-null contract — which the host's `catch (Throwable)` logs and treats as a dead slot |
//! | `updateParams(view, paramsJson)` | [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeUpdateParams`] | Props change, Rust-diffed before any setter runs |
//! | `disposeView(view)` | [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeDisposeControl`] | Paired-delete of every reference the control retained |
//!
//! ## `FrustNativeListener` (the generic listener glue)
//!
//! | Kotlin | Export | Contract |
//! |---|---|---|
//! | `nativeOnEvent(slotId, kind, detail)` | [`Java_dev_frust_nativewidgets_FrustNativeListener_nativeOnEvent`] | ONE class, ONE native method, dispatched by `(slot id, kind)`; `detail` packs a primitive payload, never JSON on the hot path |
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
//! in `catch_unwind` and resolves to a benign default — the crate's half of
//! `docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule, the same shape
//! `frust-camera`'s exports use. Every export but
//! [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeCreateControl`] resolves
//! via `LogErrorAndDefault` (log and return the default). That one export
//! resolves via `ThrowRuntimeExAndDefault` instead: the *frozen Kotlin ↔
//! Rust contract* table above promises `createView` throws on **any**
//! failure, not only the ones [`create_control`] catches and throws for
//! itself — a JNI-level error reading `paramsJson`, or a caught panic
//! anywhere in the call, must throw too, or the contract and the code
//! disagree again.
//! The only `unsafe` tokens in this module are the exports'
//! `#[unsafe(no_mangle)]` attributes.
//!
//! # Package
//!
//! The two Kotlin files sit in the `dev.frust.nativewidgets` subpackage of
//! this plugin's own `com.android.library` module — the shape
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions requires of every plugin
//! (`plugins/secure-storage`'s `dev.frust.securestorage`,
//! `plugins/camera`'s `dev.frust.camera`), wired into a consuming app by
//! `Contribution::GradleModule` rather than copied into it.
//!
//! The subpackage — rather than a package outside `dev.frust` entirely — is
//! load-bearing twice over: AGP's `namespace` must differ from the embedding
//! module's exclusive bare `dev.frust`, *and* the embedding's `FrustViewHost`
//! only resolves a platform-view factory whose FQCN carries the `dev.frust.`
//! prefix (its `FACTORY_PACKAGE_PREFIX`), which is the string
//! [`crate::api`]'s Android `VIEW_TYPE` publishes.
//!
//! The package is baked into every symbol name below and into
//! [`ctx::LISTENER_CLASS`], so this is a one-time move: it may not happen
//! again once shipped.

mod ctx;
// Theme ladder L3: registering Glyph font bytes with Android and
// creating/caching the resulting `Typeface` objects. `pub(crate)`, not
// private like `ctx`: `controls::platform`'s `Setter::Typeface` apply arm
// needs `fonts::typeface_for` from OUTSIDE this module's own subtree, the
// same reason `theme` below is `pub(crate)`.
pub(crate) mod fonts;
// `pub(crate)`, not private like `ctx`: `controls::platform`'s
// `Setter::ThemedBackground` apply arm (theme ladder L2) needs
// `theme::dp_to_px` from OUTSIDE this module's own subtree.
pub(crate) mod theme;

use jni::errors::{LogErrorAndDefault, ThrowRuntimeExAndDefault};
use jni::objects::{JObject, JString};
use jni::strings::JNIString;
use jni::sys::{jint, jlong, jobject};
use jni::{Env, EnvUnowned, jni_str};

pub(crate) use ctx::NativeCtx;
// Re-exported so `crate::demo`'s `guard_jni` (the `ComponentCtx::env()`
// escape hatch's own check-and-clear, `docs/CODE_STANDARDS.md`-sanctioned)
// can reuse the crate's own exception-extracting helper instead of
// re-implementing a weaker bare check-and-clear (R0-8) — `ctx` itself stays
// private to this module, only this one function widens. `demo` is the only
// consumer, so this is unused with `demo-components` off.
#[cfg(feature = "demo-components")]
pub(crate) use ctx::run_jni;

use crate::NativeWidgetError;
use crate::controls::{button, image, label, progress, slider, spinner, switch};
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
/// link-time discovery is banned here. This table and the api layer's
/// builders are the two
/// ends of the same kind strings, which is why each one is a `KIND` const in
/// its own control module rather than a literal here.
pub(crate) fn register_controls(runtime: &mut NativeRuntime) {
    runtime.register::<button::Button>(button::KIND);
    runtime.register::<label::Label>(label::KIND);
    runtime.register::<switch::Switch>(switch::KIND);
    runtime.register::<slider::Slider>(slider::KIND);
    runtime.register::<progress::Progress>(progress::KIND);
    runtime.register::<image::Image>(image::KIND);
    runtime.register::<spinner::Spinner>(spinner::KIND);
}

// --- exports: FrustNativeControlFactory -------------------------------------

/// `FrustNativeControlFactory.createView` → the Rust-built control.
///
/// Returns the new view as a local reference on success. On failure — the
/// params carry no known control kind, the control's own `create` failed, a
/// re-entrant runtime borrow, an unreadable `paramsJson`, or a caught panic
/// — this throws a Java exception instead of returning `null`, honouring
/// `FrustPlatformViewFactory`'s documented contract (`createView` returns a
/// non-null `View`; a thrown exception, not a `null` return, is the failure
/// channel the host's `applyCreate` already catches and treats as a dead
/// slot rather than crashing its frame loop). The
/// `resolve::<ThrowRuntimeExAndDefault>()` below is what covers the last two
/// of those (a JNI-level error reading `paramsJson`, and a caught panic
/// anywhere in this call) — every other export in this module resolves via
/// the quieter `LogErrorAndDefault` instead, since only this one export's
/// contract promises a throw.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeCreateControl<
    'local,
>(
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
        .resolve::<ThrowRuntimeExAndDefault>();
    created.unwrap_or(std::ptr::null_mut())
}

/// The typed half of [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeCreateControl`]:
/// dispatch, build, retain, and hand a fresh local reference back to Kotlin.
/// On failure this throws a Java exception ([`throw_create_failed`]) rather
/// than returning `null` — see that function's doc and the module doc's
/// contract table above. This now includes [`runtime::with_runtime`]
/// returning `None` (the runtime's `RefCell` already borrowed — a
/// re-entrant call): [`reentrant_create_failure`] turns that into the same
/// `NativeWidgetError` shape every other failure takes, so it rides the
/// identical `match outcome` / `throw_create_failed` path below instead of
/// `?`-ing straight past it the way it used to (an earlier fix found this
/// one path was silently returning `null`).
///
/// Theme ladder L1: builds every control against a night-qualified
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
    })
    // `with_runtime` returns `None` only when the runtime's `RefCell` is
    // already borrowed (a re-entrant call — a platform setter fired its own
    // listener back into create). `with_runtime` itself already
    // `log::warn!`s that at the point of detection; folding it into the
    // same `Result` shape here is what makes it fall through the `match
    // outcome` arm below and throw, instead of `?`-ing straight past the
    // throwing contract like before.
    .unwrap_or_else(reentrant_create_failure);

    match outcome {
        Ok((slot_id, raw)) => {
            log::debug!("frust-native-widgets: created slot {slot_id}");
            Some(raw)
        }
        Err(e) => {
            log::warn!("frust-native-widgets: createView failed: {e}");
            throw_create_failed(env, &e);
            None
        }
    }
}

/// The typed failure [`create_control`] reports in place of the bare `?`
/// early return an earlier version left over `runtime::with_runtime`'s `None` arm (the
/// runtime's `RefCell` already borrowed) — see that function's doc. Kept as
/// its own function, mirroring [`create_failure_message`] below, so the
/// message is host-testable the same way.
fn reentrant_create_failure() -> Result<(SlotId, jobject), NativeWidgetError> {
    Err(NativeWidgetError::Platform(
        "runtime re-entrant — a platform setter fired its own listener during create".into(),
    ))
}

/// Throws a Java exception reporting a create failure — the fix for
/// a null-create NPE: `createView` returns a non-null `View` by
/// contract (`FrustPlatformViewFactory`'s KDoc), so a failure must surface
/// as a thrown exception rather than a `null` return that later NPEs on
/// `view.visibility` in the embedding's `FrustViewHost.applyCreate`.
///
/// `Env::throw_new` sets the pending exception as a side effect and then
/// returns `Err(Error::JavaException)` to signal that success (see its own
/// doc comment) — deliberately discarded here (`let _ =`) since the caller
/// ([`create_control`]) already logged the original failure and only needs
/// to fall through to its own `None`. That `None` rides back as `Ok(None)`
/// from the outer `with_env` closure, so
/// `resolve::<ThrowRuntimeExAndDefault>()` back in
/// [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeCreateControl`] never
/// even runs for it (its `on_error`/`on_panic` fire only on `Err`/a panic,
/// never on `Ok`) — the pending exception this function set rides the JNI
/// call back to Kotlin untouched, where `applyCreate`'s `catch (Throwable)`
/// is exactly what turns it into a dead slot instead of an NPE.
fn throw_create_failed(env: &mut Env<'_>, error: &NativeWidgetError) {
    let msg = JNIString::new(create_failure_message(error));
    let _ = env.throw_new(jni_str!("java/lang/RuntimeException"), msg);
}

/// Builds the exception message [`throw_create_failed`] throws — factored
/// out as a pure function so the message itself is host-testable (below)
/// even though the JNI throw call that consumes it isn't: this module is
/// `#[cfg(target_os = "android")]`-gated and this repo carries no
/// embedded-JVM test harness (confirmed by inspection — no other plugin's
/// Android arm has one either), so no host process can actually execute a
/// `Java_..._nativeCreateControl` call and observe a pending exception.
/// This crate's own "documented compile-gated equivalent"
/// is the test below plus `cargo check --target aarch64-linux-android -p
/// frust-native-widgets --tests`, which type-checks it; it cannot *run*
/// without an Android device/emulator test runner, the same limitation as
/// every other android-cfg-gated test in this crate.
fn create_failure_message(error: &NativeWidgetError) -> String {
    format!("frust-native-widgets: createView failed: {error}")
}

#[cfg(test)]
mod create_failure_message_tests {
    use super::*;

    #[test]
    fn names_the_underlying_error() {
        let err = NativeWidgetError::UnknownControl("bogus".into());
        let msg = create_failure_message(&err);
        assert!(msg.contains("createView failed"));
        assert!(msg.contains("bogus"));
    }
}

/// Host-testable coverage for the re-entrancy path:
/// [`reentrant_create_failure`] is
/// the pure function that stands in for `runtime::with_runtime`'s `None`
/// arm, so it can be exercised the same way [`create_failure_message_tests`]
/// exercises the create-failed message above, without a JVM.
///
/// The **panic** path has no equivalent pure function to unit-test here —
/// `resolve::<ThrowRuntimeExAndDefault>()`'s `on_panic` arm is the jni crate's
/// own `ErrorPolicy` impl (`jni-0.22.4/src/errors/policy.rs`), exercised by
/// its own upstream test suite, not this crate's. What this crate owns and
/// *can* verify without a device is that `nativeCreateControl` actually
/// resolves via that throwing policy rather than `LogErrorAndDefault` — the
/// `.resolve::<ThrowRuntimeExAndDefault>()` call above, confirmed by
/// `cargo check --target aarch64-linux-android -p frust-native-widgets`
/// (a policy-name typo would be a compile error, since `EnvOutcome::resolve`
/// is generic over the `ErrorPolicy` type parameter) — the same
/// "documented compile-gated equivalent" shape `create_failure_message`'s
/// doc above already uses for the parts of this contract no host process
/// can execute end-to-end.
#[cfg(test)]
mod reentrant_create_failure_tests {
    use super::*;

    #[test]
    fn reports_reentrancy_distinctly() {
        let Err(err) = reentrant_create_failure() else {
            panic!("reentrant_create_failure must always return Err");
        };
        let msg = create_failure_message(&err);
        assert!(msg.contains("createView failed"));
        assert!(msg.contains("re-entrant"));
    }
}

/// `FrustNativeControlFactory.updateParams` → a Rust-diffed props update.
///
/// The `view` argument is part of the factory contract but unused: the slot
/// id in `paramsJson` is authoritative (module doc's *Which call carries the
/// slot id*), and a params payload naming a slot with no live instance is
/// tolerated rather than an error.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeUpdateParams<
    'local,
>(
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
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeDisposeControl<
    'local,
>(
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

/// The typed half of [`Java_dev_frust_nativewidgets_FrustNativeControlFactory_nativeDisposeControl`].
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
/// write, which wakes exactly one frust frame — runs
/// there too.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeListener_nativeOnEvent<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    slot_id: jlong,
    kind: jint,
    detail: jlong,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        debug_assert_main_thread(env, "nativeOnEvent");
        let slot_id = match validate_event_slot_id(slot_id) {
            Ok(slot_id) => slot_id,
            Err(message) => {
                log::warn!("{message}");
                return Ok(());
            }
        };
        let event = NativeEvent { kind, detail };
        let delivered = runtime::with_runtime(|runtime| runtime.on_event(slot_id, event));
        if delivered.is_none() {
            log::debug!("frust-native-widgets: event for slot {slot_id} dropped (re-entrant)");
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Validates the raw `slot_id` Kotlin passes into `nativeOnEvent` as a
/// `jlong` (`i64`) before it dispatches — the same [`SlotId::try_from`]
/// discipline [`crate::runtime::Params::identity`] applies to the very same
/// field arriving over `params_json`'s `__frustSlot` key. This export used to
/// do a bare `slot_id as SlotId`, which silently wraps a negative id into a
/// huge, wrong `u64` instead of rejecting it (R0-20) — an internal
/// inconsistency with the params path, not just a theoretical gap. Reachable
/// only from the app's own process, so this buys a clear failure instead of a
/// silent misroute, not a security boundary.
///
/// Factored out as a pure function, mirroring [`create_failure_message`]
/// above, so the rejection path is host-testable without a JVM.
fn validate_event_slot_id(raw: jlong) -> Result<SlotId, String> {
    SlotId::try_from(raw).map_err(|_| {
        format!("frust-native-widgets: nativeOnEvent got a negative slot id {raw} — dropped")
    })
}

#[cfg(test)]
mod validate_event_slot_id_tests {
    use super::*;

    #[test]
    fn accepts_a_non_negative_id() {
        assert_eq!(validate_event_slot_id(42), Ok(42));
    }

    #[test]
    fn rejects_a_negative_id() {
        let Err(message) = validate_event_slot_id(-1) else {
            panic!("validate_event_slot_id(-1) must return Err");
        };
        assert!(message.contains("nativeOnEvent"));
        assert!(message.contains("-1"));
        assert!(message.contains("dropped"));
    }
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
    use jni::jni_sig;

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
