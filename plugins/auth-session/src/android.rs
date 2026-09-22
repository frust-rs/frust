//! The Android backend for [`crate::AuthSession::start`] — Chrome Custom
//! Tabs, driven from a `dev.frust.authsession.FrustAuthSessionHost` Kotlin
//! host (`plugins/auth-session/platform/android/`) over this crate's own
//! JNI surface.
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
//! | Rust → Kotlin | `@JvmStatic fun start(url: String, callbackScheme: String, ephemeral: Boolean, generation: Long): Int` — `(Ljava/lang/String;Ljava/lang/String;ZJ)I` | Resolved once via `context.getClassLoader().loadClass(...)` (the `FrustIapHost`/`FrustBiometric` mechanism — see [`host_class`]) and cached. |
//! | Kotlin → Rust | [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`]`(env, class, generation: jlong, kind: jint, url: JString)` | The host's one callback into Rust — see *Two numbering schemes* below. |
//!
//! # The generation round trip
//!
//! `start` is handed the live session's generation
//! ([`crate::SessionToken::generation`]) and the host echoes back **exactly
//! that value** on every `nativeOnAuthSessionResult` it makes for that
//! launch. [`crate::resolve`] completes a session only against its own
//! generation, so a result from a session the caller already abandoned —
//! or a duplicate delivery of one already consumed — is discarded instead
//! of completing whichever session happens to be live by then. The host
//! never invents a generation: a launch it has no record of has nothing to
//! report.
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
//!   host cannot reuse it).
//!
//! [`outcome_from_kind`] maps each posted failure onto **the same**
//! [`crate::AuthSessionError`] its synchronous counterpart returns, so a
//! caller cannot tell the two delivery routes apart: `kind` `2` is the
//! [`crate::AuthSessionError::NoHandler`] return code `2` reports, and
//! `kind` `4` carries the same message return code `1` does
//! ([`NO_RESUMED_ACTIVITY`]). Only `3` and an unrecognized value stay
//! generic, naming the raw `kind`.
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
//! acts on it is that Kotlin module's own concern, not this file's.

use std::sync::OnceLock;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::{jint, jlong};
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

/// The [`AuthSessionError::Platform`] message for "no resumed `Activity`",
/// shared by `start`'s synchronous return code
/// [`START_NO_RESUMED_ACTIVITY`] and the posted
/// [`RESULT_NO_RESUMED_ACTIVITY`] `kind` — the same condition reached by
/// two delivery routes must read identically to a caller (module doc's
/// *Two numbering schemes*).
const NO_RESUMED_ACTIVITY: &str = "no resumed Activity to launch the Custom Tab from";

/// `nativeOnAuthSessionResult`'s `kind` meaning the identity provider
/// redirected back — `url` carries the callback (module doc's *Two
/// numbering schemes*).
const RESULT_CALLBACK: jint = 0;
/// `nativeOnAuthSessionResult`'s `kind` meaning the user dismissed the tab.
const RESULT_CANCELLED: jint = 1;
/// `nativeOnAuthSessionResult`'s `kind` meaning the posted launch found no
/// browser capable of a Custom Tab — the asynchronous twin of
/// [`START_ACTIVITY_NOT_FOUND`].
const RESULT_NO_BROWSER: jint = 2;
/// `nativeOnAuthSessionResult`'s `kind` meaning the posted launch found no
/// resumed `Activity` — the asynchronous twin of
/// [`START_NO_RESUMED_ACTIVITY`], re-coded because `1` already means
/// Cancelled on this scale.
const RESULT_NO_RESUMED_ACTIVITY: jint = 4;

/// The cached `FrustAuthSessionHost` class, loaded once via the application
/// classloader. A racing loser's reference is dropped immediately
/// (`Global`'s own `Drop` releases it), so at most one global ref survives
/// (mirrors `plugins/iap/src/android.rs`'s `HOST_CLASS`).
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Start a Chrome Custom Tabs auth session — see this module's doc's
/// contract table and *The resume-observation design*. No Rust-side object
/// is kept alive across the async gap (unlike [`crate::apple`]'s `LIVE`
/// slot): all this backend carries forward is the token's generation, which
/// the host echoes back on
/// [`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`]
/// so [`crate::resolve`] can attribute the result (module doc's *The
/// generation round trip*).
pub(crate) fn start(req: AuthSessionRequest, token: SessionToken) -> Result<(), AuthSessionError> {
    with_host(|env, class| {
        let code = run_jni(env, "FrustAuthSessionHost.start", |env| {
            let url = env.new_string(&req.url)?;
            let scheme = env.new_string(&req.callback_scheme)?;
            env.call_static_method(
                class,
                jni_str!("start"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;ZJ)I"),
                &[
                    JValue::Object(&url),
                    JValue::Object(&scheme),
                    JValue::Bool(req.ephemeral),
                    // `u64` → `jlong` is a reinterpretation, not a
                    // truncation: the host treats the value as opaque and
                    // echoes the same 64 bits back, which
                    // `nativeOnAuthSessionResult` casts straight back to
                    // `u64`.
                    JValue::Long(token.generation as jlong),
                ],
            )?
            .i()
        })?;

        match code {
            START_ACCEPTED => Ok(()),
            START_NO_RESUMED_ACTIVITY => {
                Err(AuthSessionError::Platform(NO_RESUMED_ACTIVITY.to_string()))
            }
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
///
/// A [`RESULT_CALLBACK`] with no URL in it — a null or undecodable JNI
/// string ([`decode_result_url`]) — is refused rather than handed back as
/// an empty callback a caller would parse as a real answer. `crate`
/// re-checks a surviving callback's scheme on the way out
/// ([`crate::resolve`]).
fn outcome_from_kind(kind: jint, url: String) -> Result<AuthSessionOutcome, AuthSessionError> {
    match kind {
        RESULT_CALLBACK if url.is_empty() => Err(AuthSessionError::Platform(
            crate::CALLBACK_WITHOUT_URL.to_string(),
        )),
        RESULT_CALLBACK => Ok(AuthSessionOutcome::Callback(url)),
        RESULT_CANCELLED => Ok(AuthSessionOutcome::Cancelled),
        RESULT_NO_BROWSER => Err(AuthSessionError::NoHandler),
        RESULT_NO_RESUMED_ACTIVITY => {
            Err(AuthSessionError::Platform(NO_RESUMED_ACTIVITY.to_string()))
        }
        other => Err(AuthSessionError::Platform(format!(
            "host launch failed (kind {other})"
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
/// table), completing the session `generation` names via
/// [`crate::resolve`]. `generation` is the value the host was started with
/// (module doc's *The generation round trip*); a result for any other
/// session is discarded there, not here.
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
    generation: jlong,
    kind: jint,
    url: JString<'local>,
) {
    env.with_env(|env| {
        let decoded_url = decode_result_url(env, &url);
        // The inverse of `start`'s own `as jlong` — the same 64 bits back.
        crate::resolve(generation as u64, outcome_from_kind(kind, decoded_url));
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

    /// A [`RESULT_CALLBACK`] the host delivered with nothing in it (a null
    /// or undecodable JNI string) is refused, never handed back as an
    /// empty callback.
    #[test]
    fn callback_kind_without_a_url_is_refused() {
        assert_eq!(
            outcome_from_kind(RESULT_CALLBACK, String::new()),
            Err(AuthSessionError::Platform(
                crate::CALLBACK_WITHOUT_URL.to_string()
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

    /// A posted no-browser failure is the same
    /// [`AuthSessionError::NoHandler`] the synchronous
    /// [`START_ACTIVITY_NOT_FOUND`] return code reports — a caller cannot
    /// tell the two delivery routes apart (module doc's *Two numbering
    /// schemes*).
    #[test]
    fn no_browser_kind_matches_the_synchronous_no_handler() {
        assert_eq!(
            outcome_from_kind(RESULT_NO_BROWSER, String::new()),
            Err(AuthSessionError::NoHandler)
        );
    }

    /// A posted no-resumed-`Activity` failure carries the same message the
    /// synchronous [`START_NO_RESUMED_ACTIVITY`] return code does.
    #[test]
    fn no_resumed_activity_kind_matches_the_synchronous_message() {
        assert_eq!(
            outcome_from_kind(RESULT_NO_RESUMED_ACTIVITY, String::new()),
            Err(AuthSessionError::Platform(NO_RESUMED_ACTIVITY.to_string()))
        );
    }

    /// `3` (the host's "some other exception") and any value this crate
    /// doesn't classify at all stay generic, naming the raw `kind` so a
    /// device log can tell them apart.
    #[test]
    fn an_unclassified_kind_is_a_platform_error_naming_the_raw_value() {
        for kind in [3, -1, 99] {
            match outcome_from_kind(kind, String::new()) {
                Err(AuthSessionError::Platform(message)) => {
                    assert_eq!(message, format!("host launch failed (kind {kind})"));
                }
                other => panic!("expected Err(Platform(..)) for kind {kind}, got {other:?}"),
            }
        }
    }
}
