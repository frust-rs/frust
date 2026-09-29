//! The Android presentation host: the `dev.frust.nativewidgets.FrustNativePresenter`
//! Kotlin object, reached over JNI, plus the live-presentation guard its one
//! callback resolves through.
//!
//! # The contract
//!
//! **`FrustNativePresenter.kt` and this module build to this table —
//! changing it means updating both files together** (pinned by
//! `super`'s drift tests). The package and object name are baked into the
//! export's mangled symbol and into [`PRESENTER_CLASS_BINARY`], so they may
//! never move once shipped.
//!
//! | Direction | Member | Notes |
//! |---|---|---|
//! | Rust → Kotlin | `@JvmStatic fun hasResumedActivity(): Boolean` — `()Z` | The host-discovery probe: `false` → [`PresentError::NoHost`] |
//! | Kotlin → Rust | [`Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`]`(env, class, generation: jlong, code: jint, actionIndex: jint)` | The presenter's ONE callback, for every presentation kind; `code` is `super::wire`'s `OUTCOME_*` table |
//!
//! # Why an init provider
//!
//! A dialog needs an `Activity` window to attach to, and this plugin's
//! factory only ever sees one inside a `createView` — an app may show an
//! alert with no native control on screen at all. The presenter therefore
//! tracks the resumed `Activity` itself, through
//! `Application.registerActivityLifecycleCallbacks` registered from
//! `FrustNativePresenterInitProvider` (a manifest-declared `ContentProvider`,
//! created before `Application.onCreate` — `frust-auth-session`'s
//! `FrustAuthSessionInitProvider` shape). It holds the `Activity` weakly and
//! clears it on that `Activity`'s `onActivityDestroyed`, never a
//! process-lifetime strong reference.
//!
//! # The live-presentation guard
//!
//! An arm parks a resolver for its presentation in [`LIVE`], tagged with the
//! generation it also hands the Kotlin side; [`nativeOnOutcome`] takes the
//! entry back **by generation** and runs it once. A callback for any other
//! generation — a duplicate, or one from a presentation the caller already
//! abandoned and a newer one replaced — finds nothing and is dropped. The
//! resolver maps `(code, actionIndex)` into its own outcome type, so the
//! one callback serves every presentation kind.
//!
//! # Where the export lives
//!
//! Beside the host it belongs to, not in `crate::android`: that module's
//! exports are pinned to the factory and listener classes
//! (`tests/kotlin_conformance.rs`), and this one is pinned to the presenter
//! by `super`'s drift tests instead. The symbol name is what the JVM binds;
//! the Rust module it is defined in is free to move.
//!
//! # No unwind across FFI, no `unsafe`
//!
//! The export's body runs inside [`jni::EnvUnowned::with_env`], which wraps
//! it in `catch_unwind`; the only `unsafe` token here is its
//! `#[unsafe(no_mangle)]` attribute.
//!
//! [`nativeOnOutcome`]: Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome

use std::sync::{Mutex, MutexGuard, OnceLock};

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JValue};
use jni::refs::Global;
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use super::{AlertHost, AlertOutcome, AlertSpec, PresentError, Sender, wire};

/// The presenter's fully-qualified name in the **binary/dotted** form
/// `ClassLoader.loadClass` expects. Resolved through the application
/// classloader: a bare `FindClass` on a JNI-attached worker thread sees only
/// the bootstrap loader, never app classes.
pub(crate) const PRESENTER_CLASS_BINARY: &str = "dev.frust.nativewidgets.FrustNativePresenter";

/// The cached presenter class, loaded once. A racing loser's reference is
/// dropped immediately, so at most one global reference survives.
static PRESENTER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Run `f` with a live [`Env`] and the presenter class inside a scoped JNI
/// attachment (`frust_plugin::android::with_jni_env` — never a permanent
/// attach).
///
/// # Errors
/// [`PresentError::Platform`] when the platform handles are not installed
/// yet, the attach fails, the class cannot be loaded, or `f` fails.
pub(crate) fn with_presenter<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, PresentError>,
) -> Result<T, PresentError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = presenter_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => Err(PresentError::Platform(
            "the Android platform handles are not installed (a scaffold predating \
             nativeInitPlatform)"
                .to_string(),
        )),
        Err(other) => Err(PresentError::Platform(format!(
            "android presenter: platform handle error: {other}"
        ))),
    }
}

/// The cached [`PRESENTER_CLASS`], loading it on first use.
fn presenter_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, PresentError> {
    if let Some(class) = PRESENTER_CLASS.get() {
        return Ok(class);
    }
    let class = load_presenter_class(env, context)?;
    Ok(PRESENTER_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass(PRESENTER_CLASS_BINARY)`, promoted to
/// a process-lifetime global reference.
fn load_presenter_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, PresentError> {
    let loader = jni_call(env, "Context.getClassLoader", |env| {
        env.call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()
    })?;
    let class = jni_call(
        env,
        "ClassLoader.loadClass(FrustNativePresenter) — is the frust-native-widgets Gradle \
         module linked?",
        |env| {
            let name = env.new_string(PRESENTER_CLASS_BINARY)?;
            let class = env
                .call_method(
                    &loader,
                    jni_str!("loadClass"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                    &[JValue::Object(&name)],
                )?
                .l()?;
            env.cast_local::<JClass>(class)
        },
    )?;
    jni_call(env, "NewGlobalRef(FrustNativePresenter)", |env| {
        env.new_global_ref(class)
    })
}

/// `FrustNativePresenter.hasResumedActivity()` — whether there is an
/// `Activity` to present over right now.
///
/// # Errors
/// [`PresentError::Platform`] on a JNI failure.
pub(crate) fn has_resumed_activity(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
) -> Result<bool, PresentError> {
    jni_call(env, "FrustNativePresenter.hasResumedActivity", |env| {
        env.call_static_method(class, jni_str!("hasResumedActivity"), jni_sig!("()Z"), &[])?
            .z()
    })
}

/// Run JNI calls through the crate's exception-clearing helper
/// (`crate::android::run_jni`), as a [`PresentError::Platform`].
fn jni_call<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, PresentError> {
    crate::android::run_jni(env, op, f).map_err(|e| PresentError::Platform(e.to_string()))
}

/// Maps one `nativeOnOutcome(code, actionIndex)` report into the parked
/// presentation's own outcome and resolves it — see the module doc's *The
/// live-presentation guard*.
pub(crate) type Resolve = Box<dyn FnOnce(i32, i32) + Send>;

/// One parked presentation, tagged with its generation.
struct LiveEntry {
    generation: u64,
    resolve: Resolve,
}

/// The one live presentation's resolver, if any. A `Mutex` rather than a
/// thread-local: the arm parks it from the requesting thread, the callback
/// takes it on the main thread.
static LIVE: Mutex<Option<LiveEntry>> = Mutex::new(None);

/// Lock [`LIVE`], recovering from poisoning rather than panicking near the
/// FFI boundary — the guarded data is a plain `Option`.
fn lock_live() -> MutexGuard<'static, Option<LiveEntry>> {
    LIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Park `resolve` as the live entry for `generation`, answering the entry it
/// displaced (one whose caller dropped the future). Returned rather than
/// dropped here so no resolver code runs under the lock; dropping it drops
/// its sender, whose receiver is already gone.
#[allow(dead_code)] // consumed by the platform alert/sheet arms
pub(crate) fn install_live(generation: u64, resolve: Resolve) -> Option<Resolve> {
    lock_live()
        .replace(LiveEntry {
            generation,
            resolve,
        })
        .map(|displaced| displaced.resolve)
}

/// Take the live resolver out, but only if it is still `generation`'s. The
/// lock is released by the time the caller runs it.
pub(crate) fn take_live(generation: u64) -> Option<Resolve> {
    let mut live = lock_live();
    match live.as_ref() {
        Some(entry) if entry.generation == generation => live.take().map(|entry| entry.resolve),
        _ => None,
    }
}

/// The resolver an alert parks: `super::wire`'s table over the spec's action
/// ids, into the alert's sender.
#[allow(dead_code)] // consumed by the platform alert arm
pub(crate) fn alert_resolver(tx: Sender<AlertOutcome>, action_ids: Vec<String>) -> Resolve {
    Box::new(move |code, action_index| {
        tx.send(wire::alert_outcome(code, action_index, &action_ids));
    })
}

/// `FrustNativePresenter.nativeOnOutcome` — the presenter's one callback
/// into Rust (module doc's contract table), resolving presentation
/// `generation` through its parked resolver. Called on the main thread; a
/// generation with no parked resolver is dropped.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    generation: jlong,
    code: jint,
    action_index: jint,
) {
    env.with_env(|_env| {
        // The inverse of the `u64 as jlong` an arm hands Kotlin — the same
        // 64 bits back.
        match take_live(generation as u64) {
            Some(resolve) => resolve(code, action_index),
            None => log::debug!(
                "frust-native-widgets: presenter outcome {code} for generation {generation} \
                 has no live presentation — dropped"
            ),
        }
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// The Android host. Until the alert arm is built on it, a request runs host
/// discovery — [`PresentError::NoHost`] when no `Activity` is resumed — and
/// answers [`PresentError::Unsupported`] otherwise.
pub(crate) struct Host;

impl AlertHost for Host {
    fn show_alert(
        _spec: AlertSpec,
        _tx: Sender<AlertOutcome>,
        _generation: u64,
    ) -> Result<(), PresentError> {
        if !with_presenter(has_resumed_activity)? {
            return Err(PresentError::NoHost);
        }
        log::warn!("frust-native-widgets: no native alert is built for Android");
        Err(PresentError::Unsupported)
    }

    fn dismiss(_generation: u64) {
        // `show_alert` above accepts nothing, so nothing is ever parked here
        // to dismiss.
    }
}
