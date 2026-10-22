//! The Android chrome arm (decision C3): [`AndroidChromeHost`] drives the
//! Kotlin `dev.frust.nativewidgets.FrustNativeChrome` object — Material 3
//! bars (`AppBarLayout` + `MaterialToolbar` on top, `BottomNavigationView`
//! at the bottom) placed beside the frust embedding's content root — over
//! JNI, and this module exports the object's two callbacks.
//!
//! # The frozen contract
//!
//! `FrustNativeChrome.kt`'s file header is the authority; the class name,
//! entry points, return codes and export symbols below may never move once
//! shipped (`docs/CODE_STANDARDS.md`'s JNI-export naming LAW), and
//! `tests/kotlin_conformance.rs` pins them against the Kotlin source.
//!
//! | Direction | Member | Notes |
//! |---|---|---|
//! | Rust → Kotlin | `@JvmStatic install(generation: Long, specJson: String): Int` | Every key ([`android_wire::install_json`]); answers an `INSTALL_*` code |
//! | Rust → Kotlin | `@JvmStatic apply(diffJson: String)` | Only the changed keys plus a `changed` array ([`android_wire::apply_json`]) |
//! | Rust → Kotlin | `@JvmStatic setIcon(slot: Int, index: Int, png: ByteArray)` | Icon bytes, never JSON; empty bytes clear the entry |
//! | Rust → Kotlin | `@JvmStatic setScroll(offset: Float, max: Float)` | [`ChromeField::Scroll`], never JSON; drives `AppBarLayout.setLifted` on a lifted-state change only |
//! | Rust → Kotlin | `@JvmStatic dispose()` | Idempotent; never calls back into Rust |
//! | Kotlin → Rust | [`Java_dev_frust_nativewidgets_FrustNativeChrome_nativeOnChromeEvent`]`(env, class, generation: jlong, kind: jint, detail: jlong)` | A tap, `FrustNativeListener`'s `KIND_CHROME_*` kinds → [`chrome::deliver`] |
//! | Kotlin → Rust | [`Java_dev_frust_nativewidgets_FrustNativeChrome_nativeOnChromeLost`]`(env, class, generation: jlong)` | The Activity or content root went away under the installed chrome |
//!
//! Both natives are `@JvmStatic external` members of a Kotlin `object`, so
//! the exports take the class, not an instance. They live here, beside the
//! host they serve, rather than in `crate::android`'s module root: that
//! module's exports are pinned to the factory and listener classes, the
//! presenter's (`crate::present::android_host`) precedent.
//!
//! # Install, refusal and the theme requirement
//!
//! `install` answers `INSTALL_OK` (0), `INSTALL_REFUSED` (1) or
//! `INSTALL_NO_ACTIVITY` (2). A refusal — an app theme that is not
//! Material-derived (Material's theme enforcement throws inflating the
//! bars), a malformed spec, or a menu of [`crate::events::CHROME_MENU_STRIDE`]
//! or more items — becomes [`ChromeError::Refused`]: the slot pins the
//! generation to the frust-drawn fallback chrome and logs the fix once
//! (limitation `native-chrome-android-material3-theme-required`: the app's
//! theme must derive from `Theme.Material3.*` or `Theme.MaterialComponents.*`).
//! When no live Activity's content hosts the frust surface — a spec before
//! `setContentView`, or only a foreign Activity (a billing proxy) resumed —
//! `install` answers [`ChromeError::NoHost`], which the slot retries on the
//! generation's next publish. Every successful install is followed by
//! `setScroll` with the spec's scroll state. Kotlin restores everything it touched before refusing; Rust only
//! drops the icon cache it filled. The bars join `android.R.id.content`
//! beside the frust root — no re-parent, the surface is never recreated —
//! so an install has no surface side effect on this side.
//!
//! # Threading, ordering and the stale-index invariant
//!
//! The frust UI thread is the Android main thread: the slot calls this host
//! from the scaffold's build, already there (debug-asserted). Kotlin runs
//! `install`/`apply`/`setIcon` and every listener synchronously, in call
//! order, on that thread — nothing is posted. So a tap always reports
//! against the spec the bars show, which is the slot's live spec, and the
//! export resolves its index with [`chrome::deliver`] directly. If Kotlin
//! ever posted an apply, this host would have to keep the spec Kotlin shows,
//! resolve each tap with `chrome::resolve_tap(shown, ChromeTap)` and hand
//! the id-based event to `chrome::deliver_event` — the iOS arm's pattern.
//!
//! Every host method runs with the slot locked, so none may re-enter it. A
//! listener that fired synchronously inside one of this host's own JNI calls
//! would deadlock in [`chrome::deliver`]; the export drops such a report
//! instead (`IN_HOST_CALL`). Kotlin's `dispose` never calls into Rust.
//!
//! # Failures after `install` answered `Ok`
//!
//! `nativeOnChromeLost` records [`ChromeError::NoHost`] for its generation
//! in a lock-free [`FailureCell`] and bumps the chrome epoch (outside any
//! lock), so the scaffold republishes: the slot takes the failure
//! ([`ChromeHost::take_failure`]), draws the fallback once and retries the
//! install on the following publish.
//!
//! # Icons
//!
//! Byte icons ride `setIcon` before the `install`/`apply` naming them,
//! re-sent only when their bytes change ([`android_wire::IconTable`]). A
//! bottom item has one icon (a tab's selected-state icon is unused), and an
//! Apple SF Symbol has no Android rendering: a symbol tab or action shows
//! its title alone and `Leading::Symbol` shows no leading item (limitation
//! `native-chrome-android-bitmap-icons`). A menu item's icon is not shown.
//!
//! # No unwind across FFI
//!
//! Both export bodies run inside [`jni::EnvUnowned::with_env`] (a
//! `catch_unwind` that resolves to a logged default), and every Rust → Kotlin
//! call goes through `crate::android::run_jni`, which clears a pending Java
//! exception. The only `unsafe` tokens here are the exports'
//! `#[unsafe(no_mangle)]` attributes.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JValue};
use jni::refs::Global;
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use super::{debug_assert_main_thread, run_jni};
use crate::NativeWidgetError;
use crate::chrome::android_wire::{self, IconTable, IconUpload};
use crate::chrome::host::{ChromeField, FailureCell};
use crate::chrome::{self, ChromeDiff, ChromeError, ChromeGeneration, ChromeHost, ChromeSpec};

/// The Kotlin object's binary (dotted) name, for `ClassLoader.loadClass`.
pub(crate) const CHROME_CLASS: &str = "dev.frust.nativewidgets.FrustNativeChrome";

/// `FrustNativeChrome.INSTALL_OK`: the chrome is built.
pub(crate) const INSTALL_OK: i32 = 0;
/// `FrustNativeChrome.INSTALL_REFUSED`: declined; the fallback is pinned.
pub(crate) const INSTALL_REFUSED: i32 = 1;
/// `FrustNativeChrome.INSTALL_NO_ACTIVITY`: no Activity hosts the frust surface; retried.
pub(crate) const INSTALL_NO_ACTIVITY: i32 = 2;

/// The local references one host call creates at most (a string, a byte
/// array per icon) — a hint; the frame grows past it.
const LOCAL_FRAME_CAPACITY: usize = 16;

/// [`FAILURES`]'s kind for a lost host (the only one Android records).
const FAILURE_LOST: u8 = 1;

/// The failure channel `nativeOnChromeLost` writes and
/// [`AndroidChromeHost::take_failure`] reads.
static FAILURES: FailureCell = FailureCell::new();

/// The cached `FrustNativeChrome` class, loaded once through the
/// application classloader (a bare `FindClass` sees no app class on an
/// attached thread).
static CHROME_CLASS_REF: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Whether a host JNI call is in progress (module doc's *Threading*).
/// Process-wide rather than a `thread_local!`: host calls and the exports
/// all run on the main thread, and an Android-target `thread_local!` trips
/// clippy's `missing_const_for_thread_local` false positive.
static IN_HOST_CALL: AtomicBool = AtomicBool::new(false);

/// The process-wide chrome host on Android — see the module doc.
#[derive(Debug)]
pub(crate) struct AndroidChromeHost {
    /// The installed generation; `None` before install, after dispose and
    /// once a lost host was taken.
    live: Option<u64>,
    /// The icon bytes Kotlin has cached.
    icons: IconTable,
}

impl AndroidChromeHost {
    /// An idle host (const, for the slot's `static`).
    pub(crate) const fn new() -> Self {
        Self {
            live: None,
            icons: IconTable::new(),
        }
    }

    /// Drop Kotlin's icon cache and ours: Kotlin's `dispose` with nothing
    /// installed only clears its cache.
    fn drop_icons(&mut self) {
        self.icons.clear();
        dispose_kotlin("FrustNativeChrome.dispose (icon cache)");
    }
}

impl Default for AndroidChromeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeHost for AndroidChromeHost {
    fn install(
        &mut self,
        generation: ChromeGeneration,
        spec: &ChromeSpec,
    ) -> Result<(), ChromeError> {
        let generation = generation.get();
        self.live = None;
        self.icons.clear();
        let uploads = self.icons.sync(spec);
        let json = android_wire::install_json(spec, &self.icons);
        let code = host_call("FrustNativeChrome.install", |env, class| {
            debug_assert_main_thread(env, "FrustNativeChrome.install");
            upload_icons(env, class, &uploads)?;
            let json = env.new_string(&json)?;
            env.call_static_method(
                class,
                jni_str!("install"),
                jni_sig!("(JLjava/lang/String;)I"),
                &[JValue::Long(generation as jlong), JValue::Object(&json)],
            )?
            .i()
        });
        match code {
            Ok(INSTALL_OK) => {
                self.live = Some(generation);
                push_scroll(spec);
                Ok(())
            }
            Ok(code) => {
                self.drop_icons();
                Err(install_error(code))
            }
            Err(CallError::NoEnv) => {
                self.icons.clear();
                Err(ChromeError::Refused(
                    "the Android platform handles are not installed (a scaffold predating \
                     nativeInitPlatform)"
                        .into(),
                ))
            }
            Err(CallError::Failed(message)) => {
                self.drop_icons();
                Err(ChromeError::Refused(message))
            }
        }
    }

    fn apply(&mut self, diff: &ChromeDiff<'_>) {
        if self.live.is_none() {
            return;
        }
        let touches_icons = [
            ChromeField::Tabs,
            ChromeField::Actions,
            ChromeField::Leading,
        ]
        .into_iter()
        .any(|field| diff.contains(field));
        let uploads = if touches_icons {
            self.icons.sync(diff.spec)
        } else {
            Vec::new()
        };
        let json = android_wire::apply_json(diff.spec, &diff.changed, &self.icons);
        let scroll = diff
            .contains(ChromeField::Scroll)
            .then(|| scroll_args(diff.spec));
        if uploads.is_empty() && json.is_none() && scroll.is_none() {
            return;
        }
        let applied = host_call("FrustNativeChrome.apply", |env, class| {
            debug_assert_main_thread(env, "FrustNativeChrome.apply");
            upload_icons(env, class, &uploads)?;
            if let Some(json) = &json {
                let json = env.new_string(json)?;
                env.call_static_method(
                    class,
                    jni_str!("apply"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &[JValue::Object(&json)],
                )?;
            }
            if let Some((offset, max)) = scroll {
                env.call_static_method(
                    class,
                    jni_str!("setScroll"),
                    jni_sig!("(FF)V"),
                    &[JValue::Float(offset), JValue::Float(max)],
                )?;
            }
            Ok(())
        });
        if let Err(error) = applied {
            // The icon table may now be ahead of Kotlin: re-send every icon
            // with the next change.
            self.icons.clear();
            log::warn!("frust-native-widgets: native chrome apply failed: {error}");
        }
    }

    /// Idempotent and panic-free (it runs from `ScaffoldWidget::drop` under
    /// the slot lock); skips Kotlin when there is no JNI environment, and
    /// Kotlin's own `dispose` tolerates a destroyed Activity.
    fn dispose(&mut self) {
        self.live = None;
        self.icons.clear();
        dispose_kotlin("FrustNativeChrome.dispose");
    }

    /// Lock-free: reads and clears [`FAILURES`]. Taking a failure forgets the
    /// install, so a later dispose never targets the lost generation.
    fn take_failure(&mut self, generation: ChromeGeneration) -> Option<ChromeError> {
        let generation = generation.get();
        FAILURES.take(generation)?;
        if self.live == Some(generation) {
            self.live = None;
            self.icons.clear();
        }
        Some(ChromeError::NoHost)
    }

    /// Nothing retries on Android (the slot retries on the next publish);
    /// only the icon cache a failed or lost install left in Kotlin is freed.
    fn abandon(&mut self, generation: ChromeGeneration) {
        FAILURES.take(generation.get());
        if self.live.is_none() {
            self.drop_icons();
        }
    }
}

/// [`ChromeError`] for a non-OK `install` code.
fn install_error(code: i32) -> ChromeError {
    match code {
        INSTALL_REFUSED => ChromeError::Refused(
            "FrustNativeChrome refused the install — the app theme is not Material-derived \
             or the spec was rejected (logcat tag `frust` names which)"
                .into(),
        ),
        INSTALL_NO_ACTIVITY => ChromeError::NoHost,
        other => ChromeError::Refused(format!("FrustNativeChrome.install answered {other}")),
    }
}

/// `setScroll`'s `(offset, max)`, `(0, 0)` without a scroll state (at the top,
/// so not lifted). The frust scroll offset, not a RecyclerView, is the lift
/// source, and Android is the only consumer of `ChromeSpec.scroll` (iOS draws
/// an edge fade instead). The scaffold publishes at most one per frame and
/// Kotlin calls `setLifted` only when the lifted state flips.
fn scroll_args(spec: &ChromeSpec) -> (f32, f32) {
    spec.scroll.map_or((0.0, 0.0), |scroll| {
        (scroll.offset as f32, scroll.max_offset as f32)
    })
}

/// Hand the freshly installed chrome `spec`'s scroll state: Kotlin's
/// install starts unlifted, so an install over a scrolled body (an Activity
/// relaunch, a retried [`ChromeError::NoHost`]) lifts the bar at once rather
/// than at the next offset change. A failure only logs; the install stands.
fn push_scroll(spec: &ChromeSpec) {
    let (offset, max) = scroll_args(spec);
    let pushed = host_call("FrustNativeChrome.setScroll", |env, class| {
        env.call_static_method(
            class,
            jni_str!("setScroll"),
            jni_sig!("(FF)V"),
            &[JValue::Float(offset), JValue::Float(max)],
        )
        .map(drop)
    });
    if let Err(error) = pushed {
        log::warn!("frust-native-widgets: native chrome scroll after install failed: {error}");
    }
}

/// Hand `uploads` to `setIcon`, in order (empty bytes clear an entry). An
/// index past `i32` cannot cross JNI: it is logged and skipped.
fn upload_icons(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    uploads: &[IconUpload],
) -> Result<(), jni::errors::Error> {
    for upload in uploads {
        let Ok(index) = i32::try_from(upload.index) else {
            log::warn!(
                "frust-native-widgets: chrome icon {} of slot {} does not fit a JNI int — not sent",
                upload.index,
                upload.slot
            );
            continue;
        };
        let bytes = env.byte_array_from_slice(upload.bytes.as_deref().unwrap_or_default())?;
        env.call_static_method(
            class,
            jni_str!("setIcon"),
            jni_sig!("(II[B)V"),
            &[
                JValue::Int(upload.slot),
                JValue::Int(index),
                JValue::Object(&bytes),
            ],
        )?;
    }
    Ok(())
}

/// Kotlin's `dispose`, logged on failure and skipped without a JNI
/// environment; never panics.
fn dispose_kotlin(op: &str) {
    let disposed = catch_unwind(AssertUnwindSafe(|| {
        host_call(op, |env, class| {
            env.call_static_method(class, jni_str!("dispose"), jni_sig!("()V"), &[])
                .map(drop)
        })
    }));
    match disposed {
        Ok(Ok(()) | Err(CallError::NoEnv)) => {}
        Ok(Err(CallError::Failed(message))) => {
            log::warn!("frust-native-widgets: {op} failed: {message}");
        }
        Err(_) => log::error!("frust-native-widgets: {op} panicked"),
    }
}

/// Why a host call did not run to completion.
#[derive(Debug)]
enum CallError {
    /// No platform handles: no JNI environment to call through.
    NoEnv,
    /// The class lookup, a JNI call or a Kotlin exception failed.
    Failed(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEnv => f.write_str("the Android platform handles are not installed"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Marks this thread as inside a host call until dropped.
struct InHostCall(bool);

impl InHostCall {
    fn enter() -> Self {
        Self(IN_HOST_CALL.swap(true, Ordering::AcqRel))
    }
}

impl Drop for InHostCall {
    fn drop(&mut self) {
        IN_HOST_CALL.store(self.0, Ordering::Release);
    }
}

/// Run `f` against the cached `FrustNativeChrome` class inside a scoped JNI
/// attachment and a local frame, with any pending Java exception cleared
/// and reported (`run_jni`).
fn host_call<T>(
    op: &str,
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, jni::errors::Error>,
) -> Result<T, CallError> {
    let _in_call = InHostCall::enter();
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = chrome_class(env, context)?;
        env.with_local_frame(LOCAL_FRAME_CAPACITY, |env| {
            run_jni(env, op, |env| f(env, class))
        })
    });
    match attached {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(CallError::Failed(error.to_string())),
        Err(frust_plugin::PlatformHandleError::NotInitialized) => Err(CallError::NoEnv),
        Err(other) => Err(CallError::Failed(format!(
            "android chrome: platform handle error: {other}"
        ))),
    }
}

/// The cached [`CHROME_CLASS_REF`], loading it on first use.
fn chrome_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, NativeWidgetError> {
    if let Some(class) = CHROME_CLASS_REF.get() {
        return Ok(class);
    }
    let class = load_chrome_class(env, context)?;
    Ok(CHROME_CLASS_REF.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass(CHROME_CLASS)`, promoted to a
/// process-lifetime global reference.
fn load_chrome_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, NativeWidgetError> {
    let loader = run_jni(env, "Context.getClassLoader", |env| {
        env.call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()
    })?;
    let class = run_jni(
        env,
        "ClassLoader.loadClass(FrustNativeChrome) — is the frust-native-widgets Gradle module \
         linked?",
        |env| {
            let name = env.new_string(CHROME_CLASS)?;
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
    run_jni(env, "NewGlobalRef(FrustNativeChrome)", |env| {
        env.new_global_ref(class)
    })
}

// --- exports: FrustNativeChrome ----------------------------------------------

/// `FrustNativeChrome.nativeOnChromeEvent` — a tap on the installed chrome,
/// stamped with the generation `install` was given, decoded against the
/// live spec by [`chrome::deliver`] (module doc's stale-index invariant). A
/// withdrawn generation, an undecodable report or a disabled item is
/// dropped.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeChrome_nativeOnChromeEvent<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    generation: jlong,
    kind: jint,
    detail: jlong,
) {
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        debug_assert_main_thread(env, "nativeOnChromeEvent");
        if IN_HOST_CALL.load(Ordering::Acquire) {
            log::warn!(
                "frust-native-widgets: chrome event {kind} reported from inside a chrome host \
                 call — dropped (the slot is locked)"
            );
            return Ok(());
        }
        // The inverse of the `u64 as jlong` `install` handed Kotlin.
        if !chrome::deliver(generation as u64, kind, detail) {
            log::debug!(
                "frust-native-widgets: chrome event {kind}/{detail} for generation \
                 {generation} dropped"
            );
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `FrustNativeChrome.nativeOnChromeLost` — the Activity or the content root
/// went away under installed generation `generation`: record
/// [`ChromeError::NoHost`] for it and bump the chrome epoch so the scaffold
/// republishes, falls back and retries (module doc's *Failures*).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_nativewidgets_FrustNativeChrome_nativeOnChromeLost<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    generation: jlong,
) {
    env.with_env(|_env| {
        log::warn!(
            "frust-native-widgets: native chrome generation {generation} lost its host \
             Activity; falling back to frust-drawn chrome until it can reinstall"
        );
        FAILURES.record(generation as u64, FAILURE_LOST);
        chrome::bump_epoch();
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_codes_map_to_the_slot_errors() {
        assert!(matches!(
            install_error(INSTALL_REFUSED),
            ChromeError::Refused(message) if message.contains("Material")
        ));
        assert_eq!(install_error(INSTALL_NO_ACTIVITY), ChromeError::NoHost);
        assert!(matches!(install_error(7), ChromeError::Refused(_)));
    }
}
