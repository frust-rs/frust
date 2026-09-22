//! The Android backend for [`crate::AuthSession::start`] — Chrome Custom
//! Tabs, driven from a `dev.frust.authsession.FrustAuthSessionHost` Kotlin
//! host (`plugins/auth-session/platform/android/`, the sibling `a2-01`
//! card) over this crate's own JNI surface.
//!
//! # The frozen contract
//!
//! **`FrustAuthSessionHost.kt` and this module build to this table —
//! changing it means updating both files together.** `dev.frust` is the
//! embedding module's exclusive package, so the host takes the
//! `dev.frust.authsession` subpackage (`docs/CODE_STANDARDS.md`'s Plugin
//! Conventions); its package is baked into every JNI symbol name below, so
//! it may never move once shipped.
//!
//! | Direction | Member | Notes |
//! |---|---|---|
//! | Rust → Kotlin | `@JvmStatic fun start(url: String, callbackScheme: String, ephemeral: Boolean): Int` | Resolved once via `context.getClassLoader().loadClass(...)` (the `FrustIapHost`/`FrustBiometric` mechanism — see [`host_class`]) and cached. |
//! | Kotlin → Rust | [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`]`(env, class, kind: jint, url: JString)` | The host's one callback into Rust — see *Two numbering schemes* below. |
//!
//! # Two numbering schemes
//!
//! `start`'s **return code** and `nativeOnAuthSessionResult`'s **`kind`**
//! argument are deliberately different scales — do not conflate them:
//!
//! - `start`'s return `Int`: `0` the launch was accepted (either presented
//!   synchronously, or — when `start` is called off the main thread —
//!   posted for a later main-thread turn and *provisionally* accepted);
//!   `1` no resumed `Activity` was available to launch the Custom Tab from;
//!   `2` `startActivity` threw `ActivityNotFoundException` (no browser
//!   capable of a Custom Tab); `3` some other exception.
//! - `nativeOnAuthSessionResult`'s `kind`: `0` the identity provider
//!   redirected back (`url` carries the callback); `1` the user dismissed
//!   the tab. Every other value is a **posted-launch failure** arriving
//!   asynchronously because `start` already returned `0` before the launch
//!   actually ran: `2` no browser (`ActivityNotFoundException`), `3`
//!   another exception, `4` no resumed `Activity` (re-coded from `start`'s
//!   own return code `1` — `kind == 1` already means Cancelled here, so the
//!   host cannot reuse it). [`outcome_from_kind`] does not need to
//!   distinguish `2`/`3`/`4` from one another: every `kind` other than `0`
//!   and `1` resolves the same [`crate::AuthSessionError::Platform`],
//!   naming the raw value.
//!
//! # The resume-observation design
//!
//! This backend never blocks [`start`]'s caller: `FrustAuthSessionHost.start`
//! either launches the Custom Tab synchronously or posts the launch and
//! returns `0` immediately, and the *real* answer — the redirect, a
//! cancellation, or a posted-launch failure — always arrives later through
//! [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`],
//! which this module maps and hands to [`crate::resolve`]. There is no
//! Rust-side object to keep alive across that gap (unlike [`crate::apple`]'s
//! `LIVE` thread-local): the Kotlin host owns the Custom Tab and the
//! Activity-result plumbing, and this module's part starts and ends inside
//! one JNI call each way. The one Android-only limitation this design
//! carries is documented at the crate level — [`crate`]'s *The Android
//! double-delivery note* — the callback URL also reaches
//! `frust::deep_links()` as an ordinary deep link; this future is the single
//! authoritative source for it, never that stream.
//!
//! # `unsafe`
//!
//! This module holds **no `unsafe` block at all** — only its one JNI
//! export's `#[unsafe(no_mangle)]` attribute (mirrors `frust-iap`'s Android
//! backend, `docs/CODE_STANDARDS.md`'s `unsafe` inventory entry for it). The
//! export upgrades its [`jni::EnvUnowned`] via [`jni::EnvUnowned::with_env`],
//! which wraps the body in `catch_unwind` — this crate's no-unwind-across-FFI
//! guarantee (`docs/CODE_STANDARDS.md`'s Language Idioms) — so no manual
//! `catch_unwind` is needed here.
//!
//! # Surviving release LTO
//!
//! [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`]
//! is referenced only from Kotlin, never from any Rust call site, so nothing
//! in this crate's own dependency graph looks like a reason for the linker
//! to keep it. It needs no `#[used]`/`AppCrateMacro` shim to survive
//! `lto = "fat"` regardless: an `#[unsafe(no_mangle)] pub extern "system" fn`
//! is an externally-visible symbol by construction, and rustc's dead-code
//! elimination — LTO included — never strips a `no_mangle` symbol, since
//! doing so could break a C/JNI caller no static analysis inside the crate
//! can see. `frust-iap`'s Android backend relies on exactly this and
//! registers no [`Contribution::AppCrateMacro`](../../../crates/frust-drive/src/plugin/registry.rs)
//! for its Android arm (`IAP_BASE`'s own doc comment states the same
//! absence) — this module does the same, needing no app-crate hook at all.
//!
//! # `ephemeral`
//!
//! [`crate::AuthSessionRequest::ephemeral`] forwards to `start`'s own
//! `ephemeral: Boolean` parameter; how (or whether) `FrustAuthSessionHost`
//! acts on it is that Kotlin module's own concern, not this file's (the
//! `a2-01` card, out of this task's scope).

use std::sync::OnceLock;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::jint;
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::{AuthSessionError, AuthSessionOutcome, AuthSessionRequest, SessionToken};

/// The Kotlin host's fully-qualified class name (binary/dotted form, as
/// `ClassLoader.loadClass` expects — **not** the slash form `FindClass`
/// wants). Ships in this plugin's own Gradle library module
/// (`plugins/auth-session/platform/android/`), looked up through the
/// application classloader (see [`load_host_class`]) — a bare `FindClass` on
/// a JNI worker thread sees only the bootstrap loader, never app classes
/// (`plugins/iap/src/android.rs`'s/`plugins/secure-storage/src/android.rs`'s
/// identical rationale).
///
/// `dev.frust` is the `frust-embedding` module's exclusive package, so the
/// host takes the `dev.frust.authsession` subpackage. This string and the
/// Kotlin module's `namespace`/`package` declaration are one contract —
/// change one and you must change the other.
const HOST_CLASS_BINARY: &str = "dev.frust.authsession.FrustAuthSessionHost";

/// `FrustAuthSessionHost.start`'s return code meaning the launch was
/// accepted (module doc's *Two numbering schemes*).
const START_ACCEPTED: jint = 0;
/// `FrustAuthSessionHost.start`'s return code meaning no resumed `Activity`
/// was available to launch the Custom Tab from.
const START_NO_RESUMED_ACTIVITY: jint = 1;
/// `FrustAuthSessionHost.start`'s return code meaning `startActivity` threw
/// `ActivityNotFoundException` (no browser capable of a Custom Tab).
const START_ACTIVITY_NOT_FOUND: jint = 2;

/// `nativeOnAuthSessionResult`'s `kind` meaning the identity provider
/// redirected back — `url` carries the callback (module doc's *Two
/// numbering schemes*).
const RESULT_CALLBACK: jint = 0;
/// `nativeOnAuthSessionResult`'s `kind` meaning the user dismissed the tab.
const RESULT_CANCELLED: jint = 1;

/// The cached `FrustAuthSessionHost` class, loaded once via the application
/// classloader. A racing loser's reference is dropped immediately
/// (`Global`'s own `Drop` releases it), so at most one global ref survives
/// (mirrors `plugins/iap/src/android.rs`'s `HOST_CLASS`).
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Start a Chrome Custom Tabs auth session — see this module's doc's
/// contract table and *The resume-observation design*. `token` is not
/// retained: this backend never keeps a live Rust-side object across the
/// async gap (unlike [`crate::apple`]'s `LIVE` slot) — the eventual outcome
/// arrives entirely through
/// [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`],
/// which resolves [`crate::ACTIVE`]'s live sender directly via
/// [`crate::resolve`].
pub(crate) fn start(req: AuthSessionRequest, _token: SessionToken) -> Result<(), AuthSessionError> {
    with_host(|env, class| {
        let code = run_jni(env, "FrustAuthSessionHost.start", |env| {
            let url = env.new_string(&req.url)?;
            let scheme = env.new_string(&req.callback_scheme)?;
            env.call_static_method(
                class,
                jni_str!("start"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;Z)I"),
                &[
                    JValue::Object(&url),
                    JValue::Object(&scheme),
                    JValue::Bool(req.ephemeral),
                ],
            )?
            .i()
        })?;

        match code {
            START_ACCEPTED => Ok(()),
            START_NO_RESUMED_ACTIVITY => Err(AuthSessionError::Platform(
                "no resumed Activity to launch the Custom Tab from".to_string(),
            )),
            START_ACTIVITY_NOT_FOUND => Err(AuthSessionError::NoHandler),
            other => Err(AuthSessionError::Platform(format!(
                "host start returned {other}"
            ))),
        }
    })
}

// --- JNI plumbing (mirrors `plugins/iap/src/android.rs`'s `with_host`/
// `host_class`/`load_host_class`/`run_jni` shape) -----------------------

/// Run `f` with a live [`Env`] and the cached `FrustAuthSessionHost` class
/// inside a scoped JNI attachment, flattening the two error layers: a
/// missing platform handle → [`AuthSessionError::PlatformNotInitialized`]
/// (checked *first*, before any JNI work), a JVM attach failure →
/// [`AuthSessionError::Platform`].
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, AuthSessionError>,
) -> Result<T, AuthSessionError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(AuthSessionError::PlatformNotInitialized)
        }
        Err(other) => Err(AuthSessionError::Platform(format!(
            "android auth-session backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, AuthSessionError> {
    if let Some(class) = HOST_CLASS.get() {
        return Ok(class);
    }
    let class = load_host_class(env, context)?;
    Ok(HOST_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass("dev.frust.authsession.FrustAuthSessionHost")`,
/// promoted to a process-lifetime global reference.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, AuthSessionError> {
    let loader = run_jni(env, "Context.getClassLoader", |env| {
        env.call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()
    })?;
    let name = run_jni(env, "new_string(HOST_CLASS_BINARY)", |env| {
        env.new_string(HOST_CLASS_BINARY)
    })?;

    // Checked by hand rather than through `run_jni`: a `ClassNotFoundException`
    // here is the one *expected* failure — the plugin's own Android Gradle
    // module isn't wired into the app — so it gets the concrete fix message
    // rather than `run_jni`'s generic "which exception class" report.
    let class = env.call_method(
        &loader,
        jni_str!("loadClass"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
        &[JValue::Object(&name)],
    );
    if env.exception_check() {
        env.exception_clear();
        return Err(AuthSessionError::Platform(
            "frust-auth-session Gradle module is not linked — add it with `frust plugin add \
             auth-session`"
                .to_string(),
        ));
    }
    let class = class.and_then(|value| value.l()).map_err(|err| {
        AuthSessionError::Platform(format!(
            "android auth-session backend: ClassLoader.loadClass: {err}"
        ))
    })?;
    let class = run_jni(env, "casting FrustAuthSessionHost", |env| {
        env.cast_local::<JClass>(class)
    })?;
    run_jni(env, "pinning FrustAuthSessionHost", |env| {
        env.new_global_ref(class)
    })
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// typed [`AuthSessionError::Platform`] naming `op` **and the exception's
/// class only** — never its message, which can quote the request URL or the
/// identity provider's redirect (this crate doc's *The URL never appears in
/// a `Display`/`Debug` string* rule). Narrower than
/// `plugins/haptics/src/android.rs`'s/`plugins/iap/src/android.rs`'s own
/// `run_jni`, which both also report the exception's message.
///
/// `jni` 0.22 leaves a thrown exception **pending** after the failing call
/// (`Err(Error::JavaException)`) — undefined behaviour for the next JNI
/// call — so this always checks/clears it before returning, whatever `f`
/// reported.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, AuthSessionError> {
    let result = f(env);
    if env.exception_check() {
        let class_name = match env.exception_occurred() {
            Some(throwable) => {
                env.exception_clear();
                env.get_object_class(&throwable)
                    .ok()
                    .and_then(|class| class.get_name(env).ok())
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "<unknown exception class>".to_string())
            }
            None => {
                env.exception_clear();
                "<no throwable>".to_string()
            }
        };
        // Defensive: don't leave a second exception pending for the next
        // JNI call.
        if env.exception_check() {
            env.exception_clear();
        }
        return Err(AuthSessionError::Platform(format!(
            "android auth-session backend: {op}: {class_name}"
        )));
    }
    result.map_err(|err| {
        AuthSessionError::Platform(format!("android auth-session backend: {op}: {err}"))
    })
}

// --- Kotlin -> Rust JNI export -------------------------------------------

/// Map `nativeOnAuthSessionResult`'s `(kind, url)` pair into this crate's own
/// outcome — the pure core of the export below (module doc's *Two numbering
/// schemes*), factored out so it is exercisable by a plain unit test rather
/// than only on-device.
fn outcome_from_kind(kind: jint, url: String) -> Result<AuthSessionOutcome, AuthSessionError> {
    match kind {
        RESULT_CALLBACK => Ok(AuthSessionOutcome::Callback(url)),
        RESULT_CANCELLED => Ok(AuthSessionOutcome::Cancelled),
        other => Err(AuthSessionError::Platform(format!(
            "host result kind {other}"
        ))),
    }
}

/// Decode `url` into a plain `String`: a null `JString` (every `kind` other
/// than [`RESULT_CALLBACK`], and a should-never-happen callback with none)
/// decodes to an empty string, as does a malformed one — logged at debug
/// level rather than surfaced, since this export's only other recourse
/// would be to drop the whole result. Never writes `url`'s contents into a
/// log line otherwise (this crate doc's URL rule).
fn decode_result_url(env: &Env<'_>, url: &JString<'_>) -> String {
    if url.is_null() {
        return String::new();
    }
    match url.try_to_string(env) {
        Ok(decoded) => decoded,
        Err(_) => {
            #[cfg(debug_assertions)]
            eprintln!(
                "frust-auth-session: nativeOnAuthSessionResult: the host's URL string could not \
                 be decoded"
            );
            String::new()
        }
    }
}

/// `Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`
/// — the Kotlin host's one callback into Rust (module doc's contract
/// table), completing the one live session via [`crate::resolve`].
///
/// Wrapped in [`jni::EnvUnowned::with_env`], which runs the body under
/// `catch_unwind` (module doc's `unsafe` section) — this module holds no
/// `unsafe` block of its own; the only `unsafe` token here is this export's
/// `#[unsafe(no_mangle)]` attribute.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    kind: jint,
    url: JString<'local>,
) {
    env.with_env(|env| {
        let decoded_url = decode_result_url(env, &url);
        crate::resolve(outcome_from_kind(kind, decoded_url));
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`RESULT_CALLBACK`] carries the decoded redirect URL through
    /// unchanged.
    #[test]
    fn callback_kind_carries_the_url() {
        assert_eq!(
            outcome_from_kind(RESULT_CALLBACK, "frustplay://cb?code=abc".to_string()),
            Ok(AuthSessionOutcome::Callback(
                "frustplay://cb?code=abc".to_string()
            ))
        );
    }

    /// [`RESULT_CANCELLED`] ignores whatever `url` string it was handed —
    /// the host never sends a meaningful one for this kind.
    #[test]
    fn cancelled_kind_ignores_the_url() {
        assert_eq!(
            outcome_from_kind(RESULT_CANCELLED, String::new()),
            Ok(AuthSessionOutcome::Cancelled)
        );
    }

    /// Every other kind (`2`/`3`/`4` per the host's contract, plus any
    /// value this crate doesn't otherwise expect) is a
    /// [`AuthSessionError::Platform`] naming the raw value — module doc's
    /// *Two numbering schemes*: this crate does not need to distinguish
    /// them from one another.
    #[test]
    fn every_other_kind_is_a_platform_error_naming_the_raw_value() {
        for kind in [2, 3, 4, -1, 99] {
            match outcome_from_kind(kind, String::new()) {
                Err(AuthSessionError::Platform(message)) => {
                    assert!(
                        message.contains(&kind.to_string()),
                        "expected the raw kind {kind} in {message:?}"
                    );
                }
                other => panic!("expected Err(Platform(..)) for kind {kind}, got {other:?}"),
            }
        }
    }
}
