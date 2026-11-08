//! The Android backend: `Intent(ACTION_SEND)` wrapped in
//! `Intent.createChooser`, started with `Context.startActivity`; plain JNI.
//!
//! The shell hands us the *application* `Context`, so the chooser carries
//! `FLAG_ACTIVITY_NEW_TASK` (mandatory, same reason as `frust-url-launcher`).
//! Exceptions are mapped to the exception class name only: messages can echo
//! the intent extras, i.e. the shared payload.

use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::{ShareError, ShareOutcome};

/// `android.content.Intent.ACTION_SEND`.
const ACTION_SEND: &str = "android.intent.action.SEND";
/// `Intent.EXTRA_TEXT`.
const EXTRA_TEXT: &str = "android.intent.extra.TEXT";
/// `Intent.EXTRA_SUBJECT`.
const EXTRA_SUBJECT: &str = "android.intent.extra.SUBJECT";
/// `Intent.FLAG_ACTIVITY_NEW_TASK` (= `0x1000_0000`).
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

/// Hand-written JNI signature strings (also asserted by a unit test, since
/// `jni_sig!` needs literals).
#[cfg(test)]
const SIGNATURES: &[&str] = &[
    "(Ljava/lang/String;)V",
    "(Ljava/lang/String;)Landroid/content/Intent;",
    "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
    "(I)Landroid/content/Intent;",
    "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
    "(Landroid/content/Intent;)V",
    SHARE_FILES_SIG,
    "(Ljava/lang/String;)Ljava/lang/Class;",
];

fn with_context<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, ShareError>,
) -> Result<T, ShareError> {
    match frust_plugin::android::with_jni_env(f) {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(ShareError::PlatformNotInitialized)
        }
        Err(other) => Err(ShareError::Platform(format!(
            "Android platform handle error: {other}"
        ))),
    }
}

/// Run JNI calls, mapping a pending exception to its class name only.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, ShareError> {
    let result = f(env);
    if env.exception_check() {
        let outcome = match env.exception_occurred() {
            Some(throwable) => {
                env.exception_clear();
                if env.exception_check() {
                    env.exception_clear();
                }
                let class_name = env
                    .get_object_class(&throwable)
                    .ok()
                    .and_then(|class| class.get_name(env).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<unknown exception class>".to_string());
                Err(ShareError::Platform(format!(
                    "Android JNI exception: {class_name}"
                )))
            }
            None => {
                env.exception_clear();
                Err(ShareError::Platform(
                    "Android JNI reported an exception with no throwable".to_string(),
                ))
            }
        };
        return outcome;
    }
    result.map_err(|e| ShareError::Platform(format!("Android Intent JNI error: {e}")))
}

/// Build the `ACTION_SEND` chooser and start it.
pub(crate) fn share(text: &str, subject: Option<&str>) -> Result<ShareOutcome, ShareError> {
    with_context(|env, context| {
        run_jni(env, |env| {
            let action = env.new_string(ACTION_SEND)?;
            let intent = env.new_object(
                jni_str!("android/content/Intent"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&action)],
            )?;
            let mime = env.new_string("text/plain")?;
            env.call_method(
                &intent,
                jni_str!("setType"),
                jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
                &[JValue::Object(&mime)],
            )?;
            let key = env.new_string(EXTRA_TEXT)?;
            let value = env.new_string(text)?;
            env.call_method(
                &intent,
                jni_str!("putExtra"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
                &[JValue::Object(&key), JValue::Object(&value)],
            )?;
            if let Some(subject) = subject {
                let key = env.new_string(EXTRA_SUBJECT)?;
                let value = env.new_string(subject)?;
                env.call_method(
                    &intent,
                    jni_str!("putExtra"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
                    &[JValue::Object(&key), JValue::Object(&value)],
                )?;
            }
            let chooser = env
                .call_static_method(
                    jni_str!("android/content/Intent"),
                    jni_str!("createChooser"),
                    jni_sig!(
                        "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;"
                    ),
                    &[JValue::Object(&intent), JValue::Object(&JObject::null())],
                )?
                .l()?;
            env.call_method(
                &chooser,
                jni_str!("addFlags"),
                jni_sig!("(I)Landroid/content/Intent;"),
                &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)],
            )?;
            env.call_method(
                context,
                jni_str!("startActivity"),
                jni_sig!("(Landroid/content/Intent;)V"),
                &[JValue::Object(&chooser)],
            )?;
            Ok(ShareOutcome::Presented)
        })
    })
}

/// Binary name of the Kotlin host in the plugin's Gradle module.
const HOST_CLASS_BINARY: &str = "dev.frust.share.FrustShareHost";
/// `shareFiles(Context, String[], String): Int`; asserted by a unit test, since
/// `jni_sig!` needs literals.
#[cfg(test)]
const SHARE_FILES_SIG: &str = "(Landroid/content/Context;[Ljava/lang/String;Ljava/lang/String;)I";

/// Shown when the host class is absent (an app scaffolded before the module).
const NOT_WIRED: &str = "frust-share Android module not wired; re-run Add Plugin";

/// Synchronous host return codes (`FrustShareHost.ERR_*`); 0 means the share
/// was validated and enqueued. Staging and chooser-launch failures happen later
/// on the host's background thread, are logged under tag `FrustShare` and show
/// no chooser.
fn host_error(code: i32) -> ShareError {
    let what = match code {
        1 => "invalid file list",
        2 => "could not stage a file for sharing",
        3 => "could not start the share chooser",
        _ => "unexpected host result",
    };
    ShareError::Platform(format!("Android file share failed: {what} (code {code})"))
}

/// Share `paths` through `FrustShareHost`, which validates them and returns at
/// once; a background thread then stages them in the cache (hard link, else
/// copy), grants read access via the module's `FileProvider` and starts the
/// chooser. `Presented` therefore means enqueued: a later staging or launch
/// failure is logged under tag `FrustShare` and shows no chooser.
/// Exceptions map to the class name only; a missing host class means the
/// Gradle module is not wired into the app.
pub(crate) fn share_files(
    paths: &[std::path::PathBuf],
    subject: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    with_context(|env, context| {
        let loader = run_jni(env, |env| {
            env.call_method(
                context,
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )?
            .l()
        })?;
        let class = match run_jni(env, |env| {
            let name = env.new_string(HOST_CLASS_BINARY)?;
            env.call_method(
                &loader,
                jni_str!("loadClass"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                &[JValue::Object(&name)],
            )?
            .l()
        }) {
            Ok(class) => class,
            Err(ShareError::Platform(message))
                if message.ends_with("java.lang.ClassNotFoundException") =>
            {
                return Err(ShareError::Platform(NOT_WIRED.to_string()));
            }
            Err(other) => return Err(other),
        };
        let class = run_jni(env, |env| env.cast_local::<jni::objects::JClass>(class))?;
        let code = run_jni(env, |env| {
            let string_class = env.find_class(jni_str!("java/lang/String"))?;
            let array = env.new_object_array(
                i32::try_from(paths.len()).unwrap_or(i32::MAX),
                &string_class,
                JObject::null(),
            )?;
            for (index, path) in paths.iter().enumerate() {
                let value = env.new_string(path.to_string_lossy())?;
                array.set_element(env, index, &value)?;
            }
            let subject = match subject {
                Some(subject) => JObject::from(env.new_string(subject)?),
                None => JObject::null(),
            };
            env.call_static_method(
                &class,
                jni_str!("shareFiles"),
                jni_sig!("(Landroid/content/Context;[Ljava/lang/String;Ljava/lang/String;)I"),
                &[
                    JValue::Object(context),
                    JValue::Object(&array),
                    JValue::Object(&subject),
                ],
            )?
            .i()
        })?;
        if code == 0 {
            Ok(ShareOutcome::Presented)
        } else {
            Err(host_error(code))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jni_signature_strings_are_well_formed() {
        for sig in SIGNATURES {
            assert!(sig.starts_with('(') && sig.contains(')'), "{sig}");
            assert_eq!(sig.matches('L').count(), sig.matches(';').count(), "{sig}");
        }
        assert_eq!(FLAG_ACTIVITY_NEW_TASK, 0x1000_0000);
    }

    #[test]
    fn host_codes_map_without_payload() {
        for code in 1..=4 {
            let ShareError::Platform(message) = host_error(code) else {
                panic!("expected Platform");
            };
            assert!(message.contains(&format!("code {code}")));
        }
        assert!(NOT_WIRED.contains("re-run Add Plugin"));
        assert_eq!(HOST_CLASS_BINARY, "dev.frust.share.FrustShareHost");
    }
}
