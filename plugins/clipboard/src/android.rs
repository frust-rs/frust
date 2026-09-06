//! The Android [`Backend`] — `android.content.ClipboardManager` over plain
//! JNI, no Kotlin/Gradle module.
//!
//! # Why no Gradle module
//!
//! Unlike `frust-secure-storage`'s biometric gate (which needs a Kotlin
//! `AuthenticationCallback` subclass JNI cannot provide) or `frust-camera`
//! (a `CameraX` session object model), `ClipboardManager`/`ClipData` are
//! plain framework classes reachable through the bootstrap class loader —
//! no app-defined helper class, and so no
//! `plugins/clipboard/platform/android` module, no manifest permission (
//! clipboard read/write needs none), and no R8 keep rule.
//!
//! # Platform handles + threading
//!
//! Every operation routes through
//! [`frust_plugin::android::with_jni_env`], which scoped-attaches the
//! current thread and hands us a live `Env` + the application `Context` —
//! the same substrate `frust-shared-preferences`/`frust-secure-storage` use.
//! Before the host shell installs the `(JavaVM, Context)` handles, that call
//! reports [`frust_plugin::PlatformHandleError::NotInitialized`], mapped to
//! [`ClipboardError::PlatformNotInitialized`] — an old scaffold predating
//! `nativeInitPlatform` degrades to a typed error, never a panic.
//!
//! `ClipboardManager` carries **no documented main-thread requirement** —
//! Google's own secure-clipboard sample calls it from a background
//! executor — so this backend calls it directly from the caller's thread,
//! inside a fresh scoped JNI attachment per operation (this crate holds no
//! JNI reference across calls, matching the shared-preferences/
//! secure-storage precedent).
//!
//! # Android 10+ focus gate (read-only)
//!
//! Starting with Android 10, `getPrimaryClip`/`getPrimaryClipDescription`
//! return `null` when the calling app is not the one currently in focus (a
//! privacy restriction against background apps snooping the clipboard). This
//! backend does not attempt to detect or work around that — a `null`
//! `ClipData` simply reads back as [`Backend::get_text`]'s `Ok(None)`, the
//! same outcome as a genuinely empty clipboard. Writing
//! (`setPrimaryClip`) carries no such restriction.
//!
//! # Sensitivity marking (`set_text_sensitive`)
//!
//! [`Backend::set_text_sensitive`] sets `ClipDescription.EXTRA_IS_SENSITIVE`
//! (API 33+) on the written clip's description extras — see
//! [`EXTRA_IS_SENSITIVE`]'s doc for why no `SDK_INT` branch is needed. This
//! suppresses the Android 13+ "copied text preview" toast for the clip and
//! signals clipboard-history UIs to redact/omit it; it does not encrypt the
//! clip or prevent another app with clipboard access from reading it back.

use jni::objects::{JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::{Backend, ClipboardError};

/// `Context.CLIPBOARD_SERVICE` — the string key `getSystemService` expects.
const CLIPBOARD_SERVICE: &str = "clipboard";

/// `ClipData.newPlainText`'s user-visible label. Cosmetic only (surfaced by
/// some OEM clipboard-history UIs); never read back by this crate.
const CLIP_LABEL: &str = "frust";

/// `ClipDescription.EXTRA_IS_SENSITIVE` — the `PersistableBundle` extras key
/// (API 33+) that suppresses the Android 13+ copied-text preview toast and
/// flags clipboard-history UIs. The string literal is used directly (rather
/// than the SDK constant, which doesn't exist before API 33) so this backend
/// needs no `Build.VERSION.SDK_INT` branch: an unrecognized extras key on a
/// pre-33 device is simply ignored by the platform, never an error. See this
/// module's [`Backend::set_text_sensitive`] impl below.
const EXTRA_IS_SENSITIVE: &str = "android.content.extra.IS_SENSITIVE";

/// The Android backend. Stateless — every operation re-fetches the
/// `ClipboardManager` inside a fresh scoped JNI attachment.
pub(crate) struct AndroidClipboard;

/// Run `f` with a live `Env` + application `Context`, flattening the two
/// error layers: a missing platform handle → [`ClipboardError::PlatformNotInitialized`];
/// a JVM attach failure → [`ClipboardError::Platform`]. Mirrors
/// `frust-secure-storage::android::with_context`.
fn with_context<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    match frust_plugin::android::with_jni_env(f) {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(ClipboardError::PlatformNotInitialized)
        }
        Err(other) => Err(ClipboardError::Platform(format!(
            "Android platform handle error: {other}"
        ))),
    }
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// [`ClipboardError::Platform`]. `jni` leaves a thrown exception **pending**
/// after the failing call (undefined behaviour for the next JNI call), so
/// this always checks/clears it before returning, whatever `f` reported —
/// mirrors `frust-secure-storage::android::run_jni`, minus that crate's
/// exception-class taxonomy (clipboard access has no equivalent to a
/// biometric `KeyPermanentlyInvalidatedException`).
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, ClipboardError> {
    let result = f(env);
    if env.exception_check() {
        let message = match env.exception_occurred() {
            Some(throwable) => {
                let name = env
                    .get_object_class(&throwable)
                    .ok()
                    .and_then(|class| class.get_name(env).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<unknown exception class>".to_string());
                let msg = throwable
                    .get_message(env)
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<no message>".to_string());
                format!("{name}: {msg}")
            }
            None => "JNI reported an exception with no throwable".to_string(),
        };
        env.exception_clear();
        // Defensive: don't leave a second exception pending.
        if env.exception_check() {
            env.exception_clear();
        }
        return Err(ClipboardError::Platform(format!(
            "Android ClipboardManager JNI exception: {message}"
        )));
    }
    result.map_err(|e| ClipboardError::Platform(format!("Android ClipboardManager JNI error: {e}")))
}

/// `context.getSystemService(Context.CLIPBOARD_SERVICE)`, cast to
/// `ClipboardManager`.
fn get_clipboard_manager<'local>(
    env: &mut Env<'local>,
    context: &JObject,
) -> Result<JObject<'local>, jni::errors::Error> {
    let service_name = env.new_string(CLIPBOARD_SERVICE)?;
    env.call_method(
        context,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&service_name)],
    )?
    .l()
}

impl Backend for AndroidClipboard {
    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let clipboard = get_clipboard_manager(env, context)?;
                let label = env.new_string(CLIP_LABEL)?;
                let text_jstr = env.new_string(text)?;
                // ClipData.newPlainText(label, text) — a static factory.
                let clip_data = env
                    .call_static_method(
                        jni_str!("android/content/ClipData"),
                        jni_str!("newPlainText"),
                        jni_sig!(
                            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
                        ),
                        &[JValue::Object(&label), JValue::Object(&text_jstr)],
                    )?
                    .l()?;
                env.call_method(
                    &clipboard,
                    jni_str!("setPrimaryClip"),
                    jni_sig!("(Landroid/content/ClipData;)V"),
                    &[JValue::Object(&clip_data)],
                )?;
                Ok(())
            })
        })
    }

    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let clipboard = get_clipboard_manager(env, context)?;
                // `getPrimaryClip` returns `null` both for a genuinely empty
                // clipboard and — Android 10+ — for an unfocused caller (see
                // the module doc's *Android 10+ focus gate*); either way
                // this reads back as `Ok(None)`, never an error.
                let clip = env
                    .call_method(
                        &clipboard,
                        jni_str!("getPrimaryClip"),
                        jni_sig!("()Landroid/content/ClipData;"),
                        &[],
                    )?
                    .l()?;
                if clip.is_null() {
                    return Ok(None);
                }
                let count = env
                    .call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?
                    .i()?;
                if count == 0 {
                    return Ok(None);
                }
                let item = env
                    .call_method(
                        &clip,
                        jni_str!("getItemAt"),
                        jni_sig!("(I)Landroid/content/ClipData$Item;"),
                        &[JValue::Int(0)],
                    )?
                    .l()?;
                // `Item.coerceToText(context)` — never null (it falls back to
                // the item's URI/intent stringified), but a non-text payload
                // still degrades this crate's plain-text API to `Ok(None)`
                // if the coerced text itself is empty.
                let text = env
                    .call_method(
                        &item,
                        jni_str!("coerceToText"),
                        jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"),
                        &[JValue::Object(context)],
                    )?
                    .l()?;
                if text.is_null() {
                    return Ok(None);
                }
                let text_jstr = env
                    .call_method(
                        &text,
                        jni_str!("toString"),
                        jni_sig!("()Ljava/lang/String;"),
                        &[],
                    )?
                    .l()?;
                let text_jstr = env.cast_local::<JString>(text_jstr)?;
                let text = text_jstr.to_string();
                // Crate-wide contract (see `crate::Clipboard::get_text`'s
                // doc): an empty string reads back as `Ok(None)`, the same
                // as a genuinely absent clip — there is no way for a caller
                // to distinguish "never set" from "set to `\"\"`" through
                // this API.
                if text.is_empty() {
                    return Ok(None);
                }
                Ok(Some(text))
            })
        })
    }

    fn set_text_sensitive(&self, text: &str) -> Result<(), ClipboardError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let clipboard = get_clipboard_manager(env, context)?;
                let label = env.new_string(CLIP_LABEL)?;
                let text_jstr = env.new_string(text)?;
                // ClipData.newPlainText(label, text) — a static factory.
                let clip_data = env
                    .call_static_method(
                        jni_str!("android/content/ClipData"),
                        jni_str!("newPlainText"),
                        jni_sig!(
                            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
                        ),
                        &[JValue::Object(&label), JValue::Object(&text_jstr)],
                    )?
                    .l()?;
                // Mark the clip sensitive: ClipData.getDescription() ->
                // ClipDescription, then set a PersistableBundle extra on it
                // (see EXTRA_IS_SENSITIVE's doc for the API-33+/no-branch
                // rationale).
                let description = env
                    .call_method(
                        &clip_data,
                        jni_str!("getDescription"),
                        jni_sig!("()Landroid/content/ClipDescription;"),
                        &[],
                    )?
                    .l()?;
                let extras = env.new_object(
                    jni_str!("android/os/PersistableBundle"),
                    jni_sig!("()V"),
                    &[],
                )?;
                let sensitive_key = env.new_string(EXTRA_IS_SENSITIVE)?;
                env.call_method(
                    &extras,
                    jni_str!("putBoolean"),
                    jni_sig!("(Ljava/lang/String;Z)V"),
                    &[JValue::Object(&sensitive_key), JValue::Bool(true)],
                )?;
                env.call_method(
                    &description,
                    jni_str!("setExtras"),
                    jni_sig!("(Landroid/os/PersistableBundle;)V"),
                    &[JValue::Object(&extras)],
                )?;
                env.call_method(
                    &clipboard,
                    jni_str!("setPrimaryClip"),
                    jni_sig!("(Landroid/content/ClipData;)V"),
                    &[JValue::Object(&clip_data)],
                )?;
                Ok(())
            })
        })
    }
}
