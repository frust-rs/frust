//! The Android detection backend — `Resources.getSystem().getConfiguration()
//! .getLocales()` (`android.os.LocaleList`, API 24+ — always present since
//! this workspace's Android floor is API 26) via plain JNI, no
//! Kotlin/Gradle module, the same "no app-defined helper class" shape
//! `frust-haptics`'s `Vibrator` backend uses.
//!
//! Every call routes through
//! [`frust_plugin::android::with_jni_env`], which scoped-attaches the
//! current thread and hands us a live `Env` + the application `Context` —
//! the substrate `frust-shared-preferences`/`frust-haptics` use. A pre-init
//! platform handle or any JNI failure along the way maps to
//! [`I18nError::Detection`] — never a panic, matching this crate's *no
//! panics near the FFI boundary* rule.

use jni::objects::{JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::I18nError;

/// `context.getResources().getConfiguration().getLocales()`, iterated
/// `size()`/`get(i)`/`toLanguageTag()` into raw BCP-47 tag strings, most-
/// preferred first (`LocaleList`'s own iteration order).
///
/// Any failure — the platform handles not yet installed, or a JNI error
/// partway through the call chain — flattens into one
/// [`I18nError::Detection`]; `detect::mod`'s [`super::parse_tags`] is the
/// only place that decides whether an empty/partial result is fatal.
pub(crate) fn raw_locale_tags() -> Result<Vec<String>, I18nError> {
    match frust_plugin::android::with_jni_env(|env, context| locale_tags(env, context)) {
        Ok(Ok(tags)) => Ok(tags),
        Ok(Err(jni_err)) => Err(I18nError::Detection(format!(
            "Android LocaleList JNI error: {jni_err}"
        ))),
        Err(handle_err) => Err(I18nError::Detection(format!(
            "Android platform handle error: {handle_err}"
        ))),
    }
}

/// The JNI call chain itself, run inside a scoped attachment: `getResources`
/// → `getConfiguration` → `getLocales` → per-entry `get(i)` →
/// `toLanguageTag`.
fn locale_tags(env: &mut Env, context: &JObject) -> Result<Vec<String>, jni::errors::Error> {
    let resources = env
        .call_method(
            context,
            jni_str!("getResources"),
            jni_sig!("()Landroid/content/res/Resources;"),
            &[],
        )?
        .l()?;
    let configuration = env
        .call_method(
            &resources,
            jni_str!("getConfiguration"),
            jni_sig!("()Landroid/content/res/Configuration;"),
            &[],
        )?
        .l()?;
    let locale_list = env
        .call_method(
            &configuration,
            jni_str!("getLocales"),
            jni_sig!("()Landroid/os/LocaleList;"),
            &[],
        )?
        .l()?;
    let size = env
        .call_method(&locale_list, jni_str!("size"), jni_sig!("()I"), &[])?
        .i()?;

    let mut tags = Vec::with_capacity(size.max(0) as usize);
    for i in 0..size {
        let locale = env
            .call_method(
                &locale_list,
                jni_str!("get"),
                jni_sig!("(I)Ljava/util/Locale;"),
                &[JValue::Int(i)],
            )?
            .l()?;
        if locale.is_null() {
            continue;
        }
        let tag = env
            .call_method(
                &locale,
                jni_str!("toLanguageTag"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        if tag.is_null() {
            continue;
        }
        let jstr = env.cast_local::<JString>(tag)?;
        tags.push(jstr.to_string());
    }
    Ok(tags)
}
