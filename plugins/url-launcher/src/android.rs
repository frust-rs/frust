//! The Android backend — `Intent(Intent.ACTION_VIEW)` +
//! `Context.startActivity`, plain JNI, no Kotlin/Gradle module.
//!
//! # Why no Gradle module
//!
//! Like `frust-haptics`'s `Vibrator` backend: `Intent`/`Uri` are plain
//! framework classes reachable through the bootstrap class loader — no
//! app-defined helper class, and so no `plugins/url-launcher/platform/android`
//! module and no R8 keep rule.
//!
//! # No `resolveActivity`/`queryIntentActivities`, no `<queries>`, no permission
//!
//! Since API 30, package visibility rules hide most other apps' package
//! info from a static query like `PackageManager.resolveActivity`/
//! `queryIntentActivities` unless the manifest declares a matching
//! `<queries>` element — and a `<queries>` entry for "any browser" is itself
//! a non-trivial, easy-to-get-wrong manifest addition this crate would have
//! to own via the plugin registry. `startActivity` itself needs no such
//! declaration to *launch* a matching activity (only to *query* for one in
//! advance), and the `Intent(ACTION_VIEW)` + `http`/`https` `Uri` pair here
//! is exactly the "web browsable" intent filter every browser on the device
//! already advertises — so this backend skips the query entirely and instead
//! catches the one exception `startActivity` throws when nothing resolves
//! ([`ActivityNotFoundException`], mapped to
//! [`UrlLauncherError::NoHandler`] below). No manifest permission is needed
//! either: launching another app's exported activity is not a
//! permission-gated operation.
//!
//! # Platform handles + threading
//!
//! Every operation routes through [`frust_plugin::android::with_jni_env`],
//! which scoped-attaches the current thread and hands us a live `Env` + the
//! application `Context` — the same substrate `frust-haptics`/
//! `frust-clipboard`/`frust-shared-preferences`/`frust-secure-storage` use.
//! Before the host shell installs the `(JavaVM, Context)` handles, that call
//! reports [`frust_plugin::PlatformHandleError::NotInitialized`], mapped to
//! [`UrlLauncherError::PlatformNotInitialized`] — an old scaffold predating
//! `nativeInitPlatform` degrades to a typed error, never a panic.
//!
//! `Context.startActivity(Intent)` carries no documented main-thread
//! requirement for launching another app's activity (unlike, say, showing a
//! dialog tied to the calling `Activity`), so this backend calls it directly
//! from the caller's thread, inside a fresh scoped JNI attachment per
//! operation — matching the crate-wide fire-and-forget contract with no
//! extra dispatch machinery needed (unlike [`crate::apple`], which does need
//! one).
//!
//! `Intent.FLAG_ACTIVITY_NEW_TASK` ([`FLAG_ACTIVITY_NEW_TASK`]) is set
//! unconditionally on the intent before starting it: the shell hands this
//! backend the *application*
//! `Context` (`crates/frust-shell-android/src/jni_glue.rs`'s
//! `nativeInitPlatform`, which installs a process-lifetime application
//! `Context`, not an `Activity` one), and `Context.startActivity` on a
//! non-`Activity` context throws `AndroidRuntimeException` unless the intent
//! carries `FLAG_ACTIVITY_NEW_TASK` — so this flag is mandatory here, not
//! optional tuning.
//!
//! [`ActivityNotFoundException`]: https://developer.android.com/reference/android/content/ActivityNotFoundException

use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::UrlLauncherError;

/// `android.content.Intent.ACTION_VIEW` — the string constant, not the `int`
/// field some other `Intent` actions use (`ACTION_VIEW`'s value is itself a
/// `String`).
const ACTION_VIEW: &str = "android.intent.action.VIEW";

/// `android.content.Intent.FLAG_ACTIVITY_NEW_TASK` (= `0x1000_0000`) —
/// mandatory here because the shell hands this backend the application
/// `Context`, not an `Activity` one (see the module doc's *Platform handles +
/// threading*).
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

/// Run `f` with a live `Env` + application `Context`, flattening the two
/// error layers: a missing platform handle → [`UrlLauncherError::PlatformNotInitialized`];
/// a JVM attach failure → [`UrlLauncherError::Platform`]. Mirrors
/// `frust-haptics::android::with_context`.
fn with_context<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, UrlLauncherError>,
) -> Result<T, UrlLauncherError> {
    match frust_plugin::android::with_jni_env(f) {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(UrlLauncherError::PlatformNotInitialized)
        }
        Err(other) => Err(UrlLauncherError::Platform(format!(
            "Android platform handle error: {other}"
        ))),
    }
}

/// Run a sequence of JNI calls, converting a pending
/// `ActivityNotFoundException` into [`UrlLauncherError::NoHandler`] and any
/// other pending exception into [`UrlLauncherError::Platform`] — using the
/// exception's **class name only**, never its message: `ActivityNotFoundException`'s
/// (and most other JNI exceptions') message echoes the failing `Intent`,
/// which would otherwise leak the URL back into this crate's error text (see
/// `crate::UrlLauncherError`'s doc). Mirrors
/// `frust-haptics::android::run_jni`'s shape, not its exception-message
/// behaviour.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, UrlLauncherError> {
    let result = f(env);
    if env.exception_check() {
        let outcome = match env.exception_occurred() {
            Some(throwable) => {
                // Clear before any further JNI call — several `Env` methods
                // (including `is_instance_of`/`get_object_class` below)
                // refuse to run while an exception is pending.
                env.exception_clear();
                if env.exception_check() {
                    env.exception_clear();
                }
                let is_not_found = env
                    .is_instance_of(
                        &throwable,
                        jni_str!("android/content/ActivityNotFoundException"),
                    )
                    .unwrap_or(false);
                if is_not_found {
                    Err(UrlLauncherError::NoHandler)
                } else {
                    let class_name = env
                        .get_object_class(&throwable)
                        .ok()
                        .and_then(|class| class.get_name(env).ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<unknown exception class>".to_string());
                    Err(UrlLauncherError::Platform(format!(
                        "Android JNI exception: {class_name}"
                    )))
                }
            }
            None => {
                env.exception_clear();
                Err(UrlLauncherError::Platform(
                    "Android JNI reported an exception with no throwable".to_string(),
                ))
            }
        };
        return outcome;
    }
    result.map_err(|e| UrlLauncherError::Platform(format!("Android Intent JNI error: {e}")))
}

/// Build and launch `Intent(ACTION_VIEW, Uri.parse(url))` with
/// [`FLAG_ACTIVITY_NEW_TASK`] set, via `context.startActivity`.
pub(crate) fn open_external(url: &str) -> Result<(), UrlLauncherError> {
    with_context(|env, context| {
        run_jni(env, |env| {
            let action = env.new_string(ACTION_VIEW)?;
            let intent = env.new_object(
                jni_str!("android/content/Intent"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&action)],
            )?;
            let url_string = env.new_string(url)?;
            let uri = env
                .call_static_method(
                    jni_str!("android/net/Uri"),
                    jni_str!("parse"),
                    jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                    &[JValue::Object(&url_string)],
                )?
                .l()?;
            env.call_method(
                &intent,
                jni_str!("setData"),
                jni_sig!("(Landroid/net/Uri;)Landroid/content/Intent;"),
                &[JValue::Object(&uri)],
            )?;
            env.call_method(
                &intent,
                jni_str!("addFlags"),
                jni_sig!("(I)Landroid/content/Intent;"),
                &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
            )?;
            env.call_method(
                context,
                jni_str!("startActivity"),
                jni_sig!("(Landroid/content/Intent;)V"),
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        })
    })
}
