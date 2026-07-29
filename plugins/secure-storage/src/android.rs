//! The Android [`Backend`] — AES-256-GCM in `AndroidKeyStore`, ciphertext in a
//! per-store `SharedPreferences` file, all over plain JNI (no Kotlin glue).
//!
//! # Design (storage only)
//!
//! One AES-256-GCM key per store lives **inside** `AndroidKeyStore` under the
//! alias `frust.ss.<store>` ([`framing::store_id`]) — generated directly with
//! [`KeyGenParameterSpec`](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec)
//! (`PURPOSE_ENCRYPT | PURPOSE_DECRYPT`, `GCM`/`NoPadding`, 256-bit). There is
//! **no RSA key-wrap layer** — `flutter_secure_storage` carries that only for
//! backward migration; a fresh backend generates the AES key straight in the
//! Keystore. Values are `Cipher
//! "AES/GCM/NoPadding"` ciphertext, persisted `iv_len || iv || ciphertext`,
//! Base64, via `SharedPreferences.putString` in a file named `frust.ss.<store>`
//! (the same per-store namespace, so `clear` never touches another store's or
//! another library's data). Works on minSdk 24 (Keystore AES-GCM is available
//! from API 23). The Base64 + IV framing are pure Rust ([`framing`], host-
//! tested); everything below is the JNI wiring, exercised on-device only.
//!
//! # Platform handles, threading, and never-panic contract
//!
//! Every operation routes through [`frust_plugin::android::with_jni_env`],
//! which scoped-attaches the current thread and hands us a live [`Env`] + the
//! application [`Context`](JObject) (the same substrate
//! `frust-shared-preferences` uses). Before the host shell installs the
//! `(JavaVM, Context)` handles that call reports
//! [`frust_plugin::PlatformHandleError::NotInitialized`], which
//! [`with_context`] maps to [`SecureStorageError::PlatformNotInitialized`] —
//! an old scaffold predating `nativeInitPlatform` degrades to a typed error,
//! never a panic (the shared-preferences old-scaffold contract). The store
//! holds no JNI reference across calls (locals are frame-scoped); each op
//! re-fetches its handles inside a fresh scoped attachment, which is what keeps
//! [`AndroidStore`] trivially `Send + Sync`.
//!
//! # Exception mapping
//!
//! `jni` 0.22 leaves a thrown Java exception **pending** after the failing call
//! (returning `Err(Error::JavaException)`); a pending exception is undefined
//! behaviour for the next JNI call, so [`run_jni`] checks
//! (`ExceptionCheck`) and clears (`ExceptionClear`) it, then maps by class:
//! `KeyPermanentlyInvalidatedException` → [`SecureStorageError::KeyInvalidated`]
//! (a biometric re-enrollment invalidated an auth-bound key), everything else →
//! [`SecureStorageError::Storage`]`("<class>: <message>")`. No OEM-specific
//! branches — every Keystore exception is handled tolerantly (OEM quirks
//! are unconfirmed anecdotes, so we map, not special-case).
//!
//! # Biometric gate
//!
//! When a store is opened with [`AuthPolicy::Required`](crate::AuthPolicy) the
//! [`AuthOptions`](crate::AuthOptions) travel in as [`AndroidStore::auth`] and
//! three things change:
//!
//! 1. **Key binding** — [`generate_key`] adds
//!    `setUserAuthenticationRequired(true)`,
//!    `setInvalidatedByBiometricEnrollment(invalidate_on_enrollment)`, and
//!    `setUserAuthenticationParameters(validity, AUTH_BIOMETRIC_STRONG [|
//!    AUTH_DEVICE_CREDENTIAL])` so the AES key is released only by a fresh
//!    biometric (or, per [`AuthOptions::allow_device_credential`](crate::AuthOptions::allow_device_credential),
//!    device-credential) authentication.
//! 2. **Per-op prompt** — a gated `get`/`set` initializes the `Cipher`, wraps
//!    it in a `BiometricPrompt.CryptoObject`, and blocks on
//!    [`authenticate`] (the framework `BiometricPrompt`, rendered by the
//!    system) before `doFinal`.
//! 3. **The Kotlin helper** — the framework `AuthenticationCallback` is an
//!    abstract class JNI cannot subclass, so the installed app must carry
//!    `dev.frust.securestorage.FrustBiometric`. It ships in this plugin's own
//!    `com.android.library` module (`platform/android/`, included as
//!    `:frust-secure-storage`; see the plugin `README.md`), not as a file
//!    copied into the app. It is looked
//!    up through the **application `Context`'s classloader** (a bare
//!    `FindClass` on a JNI worker thread sees only the bootstrap loader, never
//!    app classes). Absent → [`SecureStorageError::NotAvailable`]`(`[`Unavailability::HelperMissing`](crate::Unavailability::HelperMissing)`)`.
//!
//! The framework `BiometricPrompt` path is API 28+; a gated open on API < 28
//! fails with [`Unavailability::UnsupportedApiLevel`](crate::Unavailability::UnsupportedApiLevel).
//! All of this compiles on the Android gate but is exercised only by a
//! physical-device manual gate; a plain (`auth: None`) store is unchanged.

use jni::objects::{JByteArray, JMap, JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::framing;
use crate::{AuthOptions, Backend, CanAuthenticate, SecureStorageError, Unavailability};

// --- Framework constants (documented, stable — not looked up reflectively) ---

/// `SharedPreferences.MODE_PRIVATE` (= 0): scopes the file to this app's
/// private data dir.
const MODE_PRIVATE: i32 = 0;
/// `KeyProperties.PURPOSE_ENCRYPT` (= 1).
const PURPOSE_ENCRYPT: i32 = 1;
/// `KeyProperties.PURPOSE_DECRYPT` (= 2).
const PURPOSE_DECRYPT: i32 = 2;
/// `Cipher.ENCRYPT_MODE` (= 1).
const ENCRYPT_MODE: i32 = 1;
/// `Cipher.DECRYPT_MODE` (= 2).
const DECRYPT_MODE: i32 = 2;
/// AES-256.
const KEY_SIZE_BITS: i32 = 256;
/// GCM authentication-tag length in bits (the standard 128-bit tag).
const GCM_TAG_BITS: i32 = 128;

/// The `AndroidKeyStore` provider name.
const KEYSTORE_PROVIDER: &str = "AndroidKeyStore";
/// The `Cipher` transformation.
const AES_GCM_TRANSFORM: &str = "AES/GCM/NoPadding";
/// `KeyProperties.BLOCK_MODE_GCM`.
const BLOCK_MODE_GCM: &str = "GCM";
/// `KeyProperties.ENCRYPTION_PADDING_NONE`.
const ENCRYPTION_PADDING_NONE: &str = "NoPadding";

// --- Biometric gate -----------------------------------------------------

/// The framework `BiometricPrompt` requires API 28 (`Build.VERSION_CODES.P`);
/// a gated open below this fails with [`Unavailability::UnsupportedApiLevel`].
const MIN_BIOMETRIC_API: i32 = 28;
/// `setUserAuthenticationParameters` (validity + auth-type flags) requires
/// API 30 (`Build.VERSION_CODES.R`); on 28–29 we fall back to the deprecated
/// `setUserAuthenticationValidityDurationSeconds`.
const AUTH_PARAMETERS_API: i32 = 30;
/// `KeyProperties.AUTH_BIOMETRIC_STRONG` (= 1<<1) — Class 3 biometrics.
const AUTH_BIOMETRIC_STRONG: i32 = 0x2;
/// `KeyProperties.AUTH_DEVICE_CREDENTIAL` (= 1<<0) — PIN/pattern/password.
const AUTH_DEVICE_CREDENTIAL: i32 = 0x1;
/// `BiometricManager.Authenticators.BIOMETRIC_STRONG` (= 0x000000F).
const AUTHENTICATOR_BIOMETRIC_STRONG: i32 = 0x0000_000F;
/// `BiometricManager.Authenticators.DEVICE_CREDENTIAL` (= 0x8000).
const AUTHENTICATOR_DEVICE_CREDENTIAL: i32 = 0x0000_8000;
/// `BiometricManager.BIOMETRIC_SUCCESS` (= 0).
const BIOMETRIC_SUCCESS: i32 = 0;
/// The sentinel `FrustBiometric.authenticate` returns on success (any other
/// value is a framework `BiometricPrompt` `ERROR_*` code — see
/// [`map_prompt_error`]). Source:
/// `platform/android/src/main/kotlin/dev/frust/securestorage/FrustBiometric.kt`.
const HELPER_SUCCESS: i32 = 0;

/// The Kotlin helper's fully-qualified class name (binary/dotted form, as
/// `ClassLoader.loadClass` expects — **not** the slash form `FindClass`
/// wants). Ships in this plugin's own Gradle library module
/// (`platform/android/`, included as `:frust-secure-storage` per the plugin
/// `README.md`); looked up through the application classloader (see
/// [`find_helper_class`]).
///
/// `dev.frust` is the `frust-embedding` module's exclusive package, so the
/// helper takes the `dev.frust.securestorage` subpackage. This string and the
/// module's `namespace`/`package` declaration are one contract — change one and
/// you must change the other.
const HELPER_CLASS_BINARY: &str = "dev.frust.securestorage.FrustBiometric";

/// A `frust.ss.<store>`-namespaced Android secure store: one Keystore AES-GCM
/// key + one `SharedPreferences` file, both keyed by [`Self::store_id`].
pub(crate) struct AndroidStore {
    /// `frust.ss.<store>` — used for **both** the Keystore key alias and the
    /// `SharedPreferences` file name (see the module doc).
    store_id: String,
    /// The per-store biometric policy: `None` for a plain store, `Some` for a
    /// gated one. Held as plain data so [`AndroidStore`] stays
    /// `Send + Sync` — the `Cipher`/`CryptoObject`/`BiometricPrompt` objects
    /// are all frame-scoped locals inside a fresh JNI attachment per op.
    auth: Option<AuthOptions>,
}

impl AndroidStore {
    /// Open the named Android secure store, optionally behind a biometric gate.
    ///
    /// # Errors
    /// [`SecureStorageError::PlatformNotInitialized`] if the host shell never
    /// installed the `(JavaVM, Context)` handles (an old scaffold predating
    /// `nativeInitPlatform`) — probed here so the failure is loud and early
    /// rather than on the first read (matching `frust-shared-preferences`'
    /// `AndroidStore::standard`). For a **gated** store:
    /// [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedApiLevel`]`)`
    /// on API < 28 (the framework `BiometricPrompt` floor), or
    /// `NotAvailable(`[`Unavailability::HelperMissing`]`)` if the
    /// `dev.frust.securestorage.FrustBiometric` helper class is absent (the
    /// plugin's Android module isn't wired in) — both probed here so a
    /// misconfigured gate fails at open, not mid-read.
    pub(crate) fn open(name: &str, auth: Option<AuthOptions>) -> Result<Self, SecureStorageError> {
        with_context(|env, context| {
            if auth.is_some() {
                // Fail fast on the two static preconditions the gate needs.
                let api = device_api_level(env)?;
                if api < MIN_BIOMETRIC_API {
                    return Err(SecureStorageError::NotAvailable(
                        Unavailability::UnsupportedApiLevel,
                    ));
                }
                if find_helper_class(env, context)?.is_none() {
                    return Err(SecureStorageError::NotAvailable(
                        Unavailability::HelperMissing,
                    ));
                }
            }
            Ok(())
        })?;
        Ok(Self {
            store_id: framing::store_id(name),
            auth,
        })
    }
}

// --- Error-layer flattening -------------------------------------------------

/// Run `f` with a live [`Env`] + application [`Context`] inside a scoped JNI
/// attachment, flattening the two error layers: a missing platform handle →
/// [`SecureStorageError::PlatformNotInitialized`]; a JVM attach failure →
/// [`SecureStorageError::Storage`]. `f` itself already returns a
/// [`SecureStorageError`].
fn with_context<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, SecureStorageError>,
) -> Result<T, SecureStorageError> {
    match frust_plugin::android::with_jni_env(f) {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(SecureStorageError::PlatformNotInitialized)
        }
        Err(other) => Err(SecureStorageError::Storage(format!(
            "Android platform handle error: {other}"
        ))),
    }
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// typed [`SecureStorageError`].
///
/// `jni` 0.22 returns `Err(Error::JavaException)` and leaves the exception
/// **pending** — undefined behaviour for the next JNI call — so we always
/// check/clear it here before returning, whatever `f` reported. A non-exception
/// `jni` error (e.g. a null return) maps to [`SecureStorageError::Storage`].
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, SecureStorageError> {
    let result = f(env);
    if env.exception_check() {
        return Err(take_pending_exception(env));
    }
    result.map_err(|e| SecureStorageError::Storage(format!("Android Keystore JNI error: {e}")))
}

/// Extract, **clear**, and classify the pending Java exception. Clears first
/// (mirroring `jni`'s own `exception_catch`) so the subsequent class/message
/// queries run without a pending exception; a defensive final clear covers the
/// unlikely case one of those queries itself throws.
fn take_pending_exception(env: &mut Env) -> SecureStorageError {
    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return SecureStorageError::Storage(
            "Android Keystore: JNI reported an exception with no throwable".to_string(),
        );
    };
    env.exception_clear();

    let class_name = match env.get_object_class(&throwable) {
        Ok(class) => match class.get_name(env) {
            Ok(name) => name.to_string(),
            Err(_) => "<unknown exception class>".to_string(),
        },
        Err(_) => "<unknown exception class>".to_string(),
    };
    let message = match throwable.get_message(env) {
        Ok(msg) => msg.to_string(),
        Err(_) => "<no message>".to_string(),
    };

    // Defensive: don't leave a second exception pending for the next JNI call.
    if env.exception_check() {
        env.exception_clear();
    }

    // The one class we distinguish: an auth-bound key invalidated by a
    // biometric re-enrollment. Match on the simple class name so
    // the framework and any legacy package variant both map.
    if class_name.ends_with("KeyPermanentlyInvalidatedException") {
        SecureStorageError::KeyInvalidated
    } else {
        SecureStorageError::Storage(format!("{class_name}: {message}"))
    }
}

// --- Keystore key: get-or-create --------------------------------------------

/// Fetch the store's AES-GCM key from `AndroidKeyStore`, generating it on first
/// use. Framework classes (`java.security.*`, `javax.crypto.*`,
/// `android.security.keystore.*`) resolve through the default (bootstrap)
/// class loader — no application-classloader lookup is needed for these; that
/// is only required for app-defined classes (the biometric helper class).
fn get_or_create_key<'local>(
    env: &mut Env<'local>,
    alias: &str,
    auth: Option<&AuthOptions>,
) -> Result<JObject<'local>, jni::errors::Error> {
    let keystore = load_keystore(env)?;
    let alias_jstr = env.new_string(alias)?;

    let exists = env
        .call_method(
            &keystore,
            jni_str!("containsAlias"),
            jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&alias_jstr)],
        )?
        .z()?;

    if exists {
        // keyStore.getKey(alias, null) -> java.security.Key (a SecretKey).
        let null = JObject::null();
        env.call_method(
            &keystore,
            jni_str!("getKey"),
            jni_sig!("(Ljava/lang/String;[C)Ljava/security/Key;"),
            &[JValue::Object(&alias_jstr), JValue::Object(&null)],
        )?
        .l()
    } else {
        generate_key(env, alias, auth)
    }
}

/// `KeyStore.getInstance("AndroidKeyStore")` then `.load(null, null)`.
fn load_keystore<'local>(env: &mut Env<'local>) -> Result<JObject<'local>, jni::errors::Error> {
    let provider = env.new_string(KEYSTORE_PROVIDER)?;
    let keystore = env
        .call_static_method(
            jni_str!("java/security/KeyStore"),
            jni_str!("getInstance"),
            jni_sig!("(Ljava/lang/String;)Ljava/security/KeyStore;"),
            &[JValue::Object(&provider)],
        )?
        .l()?;
    // keyStore.load((InputStream) null, (char[]) null) — required before use.
    let null_stream = JObject::null();
    let null_password = JObject::null();
    env.call_method(
        &keystore,
        jni_str!("load"),
        jni_sig!("(Ljava/io/InputStream;[C)V"),
        &[JValue::Object(&null_stream), JValue::Object(&null_password)],
    )?;
    Ok(keystore)
}

/// Generate the store's AES-256-GCM key directly in `AndroidKeyStore` via
/// `KeyGenParameterSpec` (no RSA wrap). When `auth` is `Some` the key is bound
/// to a fresh authentication: `setUserAuthenticationRequired(true)` plus
/// enrollment-invalidation and validity/auth-type per the options (see
/// [`apply_auth_binding`]).
fn generate_key<'local>(
    env: &mut Env<'local>,
    alias: &str,
    auth: Option<&AuthOptions>,
) -> Result<JObject<'local>, jni::errors::Error> {
    // KeyGenerator.getInstance("AES", "AndroidKeyStore")
    let algorithm = env.new_string("AES")?;
    let provider = env.new_string(KEYSTORE_PROVIDER)?;
    let key_generator = env
        .call_static_method(
            jni_str!("javax/crypto/KeyGenerator"),
            jni_str!("getInstance"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;"),
            &[JValue::Object(&algorithm), JValue::Object(&provider)],
        )?
        .l()?;

    // new KeyGenParameterSpec.Builder(alias, PURPOSE_ENCRYPT | PURPOSE_DECRYPT)
    let alias_jstr = env.new_string(alias)?;
    let builder = env.new_object(
        jni_str!("android/security/keystore/KeyGenParameterSpec$Builder"),
        jni_sig!("(Ljava/lang/String;I)V"),
        &[
            JValue::Object(&alias_jstr),
            JValue::Int(PURPOSE_ENCRYPT | PURPOSE_DECRYPT),
        ],
    )?;

    // .setBlockModes("GCM")
    let block_modes = string_array(env, &[BLOCK_MODE_GCM])?;
    let builder = env
        .call_method(
            &builder,
            jni_str!("setBlockModes"),
            jni_sig!(
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
            ),
            &[JValue::Object(&block_modes)],
        )?
        .l()?;

    // .setEncryptionPaddings("NoPadding")
    let paddings = string_array(env, &[ENCRYPTION_PADDING_NONE])?;
    let builder = env
        .call_method(
            &builder,
            jni_str!("setEncryptionPaddings"),
            jni_sig!(
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
            ),
            &[JValue::Object(&paddings)],
        )?
        .l()?;

    // .setKeySize(256)
    let builder = env
        .call_method(
            &builder,
            jni_str!("setKeySize"),
            jni_sig!("(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Int(KEY_SIZE_BITS)],
        )?
        .l()?;

    // Gated store: bind the key to a fresh authentication. `builder` is
    // rebound to the (same) builder the auth setters return.
    let builder = match auth {
        Some(opts) => apply_auth_binding(env, builder, opts)?,
        None => builder,
    };

    // .build()
    let spec = env
        .call_method(
            &builder,
            jni_str!("build"),
            jni_sig!("()Landroid/security/keystore/KeyGenParameterSpec;"),
            &[],
        )?
        .l()?;

    // keyGenerator.init(spec); keyGenerator.generateKey()
    env.call_method(
        &key_generator,
        jni_str!("init"),
        jni_sig!("(Ljava/security/spec/AlgorithmParameterSpec;)V"),
        &[JValue::Object(&spec)],
    )?;
    env.call_method(
        &key_generator,
        jni_str!("generateKey"),
        jni_sig!("()Ljavax/crypto/SecretKey;"),
        &[],
    )?
    .l()
}

/// Build a `String[]` of one element — `KeyGenParameterSpec.Builder`'s
/// `setBlockModes`/`setEncryptionPaddings` take varargs (a `String[]`).
fn string_array<'local>(
    env: &mut Env<'local>,
    values: &[&str],
) -> Result<JObject<'local>, jni::errors::Error> {
    let empty = JObject::null();
    let array = env.new_object_array(values.len() as i32, jni_str!("java/lang/String"), &empty)?;
    for (i, value) in values.iter().enumerate() {
        let element = env.new_string(value)?;
        array.set_element(env, i, &element)?;
    }
    Ok(array.into())
}

// --- Biometric gate -----------------------------------------------------

/// `Build.VERSION.SDK_INT` — the device's API level. Read reflectively via a
/// static field so the backend needs no compile-time SDK constant.
fn device_api_level(env: &mut Env) -> Result<i32, SecureStorageError> {
    run_jni(env, |env| {
        env.get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?
        .i()
    })
}

/// The application `Context`'s classloader — the only loader that can see
/// app-defined classes (a JNI worker thread's `FindClass` sees the bootstrap
/// loader only). `context.getClassLoader()`.
fn context_class_loader<'local>(
    env: &mut Env<'local>,
    context: &JObject,
) -> Result<JObject<'local>, jni::errors::Error> {
    env.call_method(
        context,
        jni_str!("getClassLoader"),
        jni_sig!("()Ljava/lang/ClassLoader;"),
        &[],
    )?
    .l()
}

/// Look the `dev.frust.securestorage.FrustBiometric` helper class up through
/// the application classloader. Returns `Ok(None)` — never an error — when the
/// class is absent (`ClassNotFoundException`), which the gate maps to
/// [`Unavailability::HelperMissing`]; a genuine JNI failure is a
/// [`SecureStorageError::Storage`].
fn find_helper_class<'local>(
    env: &mut Env<'local>,
    context: &JObject,
) -> Result<Option<JObject<'local>>, SecureStorageError> {
    let loader = run_jni(env, |env| context_class_loader(env, context))?;
    let name = run_jni(env, |env| env.new_string(HELPER_CLASS_BINARY))?;
    // classLoader.loadClass("dev.frust.securestorage.FrustBiometric") — throws
    // ClassNotFoundException if the plugin's Android module isn't wired in.
    let class = env.call_method(
        &loader,
        jni_str!("loadClass"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
        &[JValue::Object(&name)],
    );
    if env.exception_check() {
        // The expected "helper not shipped" signal — clear and report absent.
        env.exception_clear();
        return Ok(None);
    }
    match class {
        Ok(value) => match value.l() {
            Ok(obj) if !obj.is_null() => Ok(Some(obj)),
            Ok(_) => Ok(None),
            Err(e) => Err(SecureStorageError::Storage(format!(
                "loading biometric helper class: {e}"
            ))),
        },
        Err(e) => Err(SecureStorageError::Storage(format!(
            "loading biometric helper class: {e}"
        ))),
    }
}

/// Chain the auth-binding setters onto a `KeyGenParameterSpec.Builder` for a
/// gated key: `setUserAuthenticationRequired(true)`,
/// `setInvalidatedByBiometricEnrollment(invalidate_on_enrollment)`, and the
/// validity/auth-type binding — `setUserAuthenticationParameters` on API 30+,
/// the deprecated `setUserAuthenticationValidityDurationSeconds` on 28–29.
fn apply_auth_binding<'local>(
    env: &mut Env<'local>,
    builder: JObject<'local>,
    opts: &AuthOptions,
) -> Result<JObject<'local>, jni::errors::Error> {
    // .setUserAuthenticationRequired(true)
    let builder = env
        .call_method(
            &builder,
            jni_str!("setUserAuthenticationRequired"),
            jni_sig!("(Z)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Bool(true)],
        )?
        .l()?;

    // .setInvalidatedByBiometricEnrollment(invalidate_on_enrollment)
    let builder = env
        .call_method(
            &builder,
            jni_str!("setInvalidatedByBiometricEnrollment"),
            jni_sig!("(Z)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Bool(opts.invalidate_on_enrollment)],
        )?
        .l()?;

    // `validity: None` → 0 seconds → a fresh auth for every use.
    let seconds = opts
        .validity
        .map(|d| d.as_secs().min(i32::MAX as u64) as i32)
        .unwrap_or(0);

    let api = env
        .get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?
        .i()?;
    if api >= AUTH_PARAMETERS_API {
        // .setUserAuthenticationParameters(seconds, BIOMETRIC_STRONG [| DEVICE_CREDENTIAL])
        let mut types = AUTH_BIOMETRIC_STRONG;
        if opts.allow_device_credential {
            types |= AUTH_DEVICE_CREDENTIAL;
        }
        env.call_method(
            &builder,
            jni_str!("setUserAuthenticationParameters"),
            jni_sig!("(II)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Int(seconds), JValue::Int(types)],
        )?
        .l()
    } else {
        // Legacy (API 28–29): duration only. `-1` = biometric-only, every use;
        // a non-negative window additionally permits the device credential.
        #[allow(deprecated)]
        let legacy = if opts.allow_device_credential {
            seconds
        } else {
            -1
        };
        env.call_method(
            &builder,
            jni_str!("setUserAuthenticationValidityDurationSeconds"),
            jni_sig!("(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Int(legacy)],
        )?
        .l()
    }
}

/// Block on the framework `BiometricPrompt` for one gated cipher operation:
/// wrap `cipher` in a `BiometricPrompt.CryptoObject`, then call the
/// `dev.frust.securestorage.FrustBiometric.authenticate(...)` static helper,
/// which posts the prompt to the main executor, subclasses the abstract
/// `AuthenticationCallback`, and latches the result on this background thread.
/// Returns the authenticated `Cipher` (`CryptoObject.getCipher()`) ready for
/// `doFinal`.
fn authenticate<'local>(
    env: &mut Env<'local>,
    context: &JObject,
    opts: &AuthOptions,
    cipher: &JObject,
) -> Result<JObject<'local>, SecureStorageError> {
    let helper = match find_helper_class(env, context)? {
        Some(class) => class,
        None => {
            return Err(SecureStorageError::NotAvailable(
                Unavailability::HelperMissing,
            ));
        }
    };

    // new BiometricPrompt.CryptoObject(cipher)
    let crypto = run_jni(env, |env| {
        env.new_object(
            jni_str!("android/hardware/biometrics/BiometricPrompt$CryptoObject"),
            jni_sig!("(Ljavax/crypto/Cipher;)V"),
            &[JValue::Object(cipher)],
        )
    })?;

    let title = run_jni(env, |env| env.new_string(&opts.prompt.title))?;
    let subtitle = run_jni(env, |env| env.new_string(&opts.prompt.subtitle))?;
    let negative = run_jni(env, |env| env.new_string(&opts.prompt.negative_button))?;

    // FrustBiometric.authenticate(context, title, subtitle, negative,
    //   allowDeviceCredential, cryptoObject) -> int (0 = success; else a
    //   BiometricPrompt ERROR_* code — see `map_prompt_error`).
    let helper_class = env
        .cast_local::<jni::objects::JClass>(helper)
        .map_err(|e| SecureStorageError::Storage(format!("biometric helper class cast: {e}")))?;
    let result = run_jni(env, |env| {
        env.call_static_method(
            &helper_class,
            jni_str!("authenticate"),
            jni_sig!(
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZLandroid/hardware/biometrics/BiometricPrompt$CryptoObject;)I"
            ),
            &[
                JValue::Object(context),
                JValue::Object(&title),
                JValue::Object(&subtitle),
                JValue::Object(&negative),
                JValue::Bool(opts.allow_device_credential),
                JValue::Object(&crypto),
            ],
        )?
        .i()
    })?;

    if result != HELPER_SUCCESS {
        return Err(map_prompt_error(result));
    }

    // On success the authenticated cipher is `cryptoObject.getCipher()`.
    run_jni(env, |env| {
        env.call_method(
            &crypto,
            jni_str!("getCipher"),
            jni_sig!("()Ljavax/crypto/Cipher;"),
            &[],
        )?
        .l()
    })
}

/// Map a framework `BiometricPrompt.BIOMETRIC_ERROR_*` code (as the Kotlin
/// helper returns it) to a typed [`SecureStorageError`] — the biometric
/// exception taxonomy. Codes per `android.hardware.biometrics.BiometricPrompt`.
fn map_prompt_error(code: i32) -> SecureStorageError {
    match code {
        // ERROR_USER_CANCELED (10), ERROR_NEGATIVE_BUTTON (13).
        10 | 13 => SecureStorageError::UserCanceled,
        // ERROR_CANCELED (5) — the system canceled (e.g. app backgrounded).
        5 => SecureStorageError::SystemCanceled,
        // ERROR_LOCKOUT (7) — too many attempts, retry after a cooldown.
        7 => SecureStorageError::LockoutTemporary,
        // ERROR_LOCKOUT_PERMANENT (9) — needs a strong-auth unlock.
        9 => SecureStorageError::LockoutPermanent,
        // ERROR_NO_BIOMETRICS (11) — none enrolled.
        11 => SecureStorageError::NotAvailable(Unavailability::NotEnrolled),
        // ERROR_HW_NOT_PRESENT (12).
        12 => SecureStorageError::NotAvailable(Unavailability::NoHardware),
        // ERROR_HW_UNAVAILABLE (1).
        1 => SecureStorageError::NotAvailable(Unavailability::HardwareUnavailable),
        // ERROR_NO_DEVICE_CREDENTIAL (14) — passcode not set.
        14 => SecureStorageError::NotAvailable(Unavailability::PasscodeNotSet),
        // Everything else (timeout, vendor, unable-to-process, …): the auth
        // ran but did not succeed.
        _ => SecureStorageError::AuthFailed,
    }
}

/// Probe whether biometric authentication is available on this device without
/// prompting — [`crate::SecureStorage::can_authenticate`]'s Android arm.
/// `BiometricManager.canAuthenticate(BIOMETRIC_STRONG)` (API 30+) /
/// `canAuthenticate()` (API 29). A missing platform handle, API < 29, or any
/// probe failure yields the closest [`Unavailability`] rather than an error
/// (the probe never panics).
pub(crate) fn can_authenticate() -> CanAuthenticate {
    let outcome = with_context(|env, context| {
        let api = device_api_level(env)?;
        if api < MIN_BIOMETRIC_API {
            return Ok(CanAuthenticate::Unavailable(
                Unavailability::UnsupportedApiLevel,
            ));
        }
        let status = run_jni(env, |env| {
            // (BiometricManager) context.getSystemService("biometric")
            let service_name = env.new_string("biometric")?;
            let class = env
                .call_method(
                    context,
                    jni_str!("getSystemService"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                    &[JValue::Object(&service_name)],
                )?
                .l()?;
            if class.is_null() {
                return Ok(None);
            }
            let mut authenticators = AUTHENTICATOR_BIOMETRIC_STRONG;
            authenticators |= AUTHENTICATOR_DEVICE_CREDENTIAL;
            let status = env
                .call_method(
                    &class,
                    jni_str!("canAuthenticate"),
                    jni_sig!("(I)I"),
                    &[JValue::Int(authenticators)],
                )?
                .i()?;
            Ok::<Option<i32>, jni::errors::Error>(Some(status))
        })?;
        Ok(match status {
            Some(BIOMETRIC_SUCCESS) => CanAuthenticate::Available,
            Some(11) => CanAuthenticate::Unavailable(Unavailability::NotEnrolled),
            Some(12) => CanAuthenticate::Unavailable(Unavailability::NoHardware),
            Some(1) => CanAuthenticate::Unavailable(Unavailability::HardwareUnavailable),
            _ => CanAuthenticate::Unavailable(Unavailability::HardwareUnavailable),
        })
    });
    outcome.unwrap_or(CanAuthenticate::Unavailable(
        Unavailability::HardwareUnavailable,
    ))
}

// --- Encrypt / decrypt ------------------------------------------------------

/// Initialize an encrypt `Cipher` under `key`, returning `(cipher, iv)`
/// **without** calling `doFinal`. GCM in `AndroidKeyStore` generates a fresh
/// random IV at init (`setRandomizedEncryptionRequired` defaults on), read
/// back via `Cipher.getIV()`. Split from the final step so a gated store can
/// interpose the biometric prompt ([`authenticate`]) between init and
/// `doFinal`; a plain store runs the two back to back.
fn init_encrypt<'local>(
    env: &mut Env<'local>,
    key: &JObject,
) -> Result<(JObject<'local>, Vec<u8>), jni::errors::Error> {
    let cipher = cipher_instance(env)?;
    env.call_method(
        &cipher,
        jni_str!("init"),
        jni_sig!("(ILjava/security/Key;)V"),
        &[JValue::Int(ENCRYPT_MODE), JValue::Object(key)],
    )?;

    let iv_obj = env
        .call_method(&cipher, jni_str!("getIV"), jni_sig!("()[B"), &[])?
        .l()?;
    let iv_array = env.cast_local::<JByteArray>(iv_obj)?;
    let iv = env.convert_byte_array(&iv_array)?;

    Ok((cipher, iv))
}

/// Initialize a decrypt `Cipher` under `key` with the stored `iv`, **without**
/// calling `doFinal` (see [`init_encrypt`] for the split rationale).
fn init_decrypt<'local>(
    env: &mut Env<'local>,
    key: &JObject,
    iv: &[u8],
) -> Result<JObject<'local>, jni::errors::Error> {
    let cipher = cipher_instance(env)?;

    // new GCMParameterSpec(128, iv)
    let iv_array = env.byte_array_from_slice(iv)?;
    let gcm_spec = env.new_object(
        jni_str!("javax/crypto/spec/GCMParameterSpec"),
        jni_sig!("(I[B)V"),
        &[JValue::Int(GCM_TAG_BITS), JValue::Object(&iv_array)],
    )?;

    env.call_method(
        &cipher,
        jni_str!("init"),
        jni_sig!("(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V"),
        &[
            JValue::Int(DECRYPT_MODE),
            JValue::Object(key),
            JValue::Object(&gcm_spec),
        ],
    )?;

    Ok(cipher)
}

/// `cipher.doFinal(input)` — the authenticated final step for both directions.
fn cipher_final(
    env: &mut Env,
    cipher: &JObject,
    input: &[u8],
) -> Result<Vec<u8>, jni::errors::Error> {
    let input_array = env.byte_array_from_slice(input)?;
    let output_obj = env
        .call_method(
            cipher,
            jni_str!("doFinal"),
            jni_sig!("([B)[B"),
            &[JValue::Object(&input_array)],
        )?
        .l()?;
    let output_array = env.cast_local::<JByteArray>(output_obj)?;
    env.convert_byte_array(&output_array)
}

/// `Cipher.getInstance("AES/GCM/NoPadding")`.
fn cipher_instance<'local>(env: &mut Env<'local>) -> Result<JObject<'local>, jni::errors::Error> {
    let transform = env.new_string(AES_GCM_TRANSFORM)?;
    env.call_static_method(
        jni_str!("javax/crypto/Cipher"),
        jni_str!("getInstance"),
        jni_sig!("(Ljava/lang/String;)Ljavax/crypto/Cipher;"),
        &[JValue::Object(&transform)],
    )?
    .l()
}

// --- SharedPreferences access ----------------------------------------------

/// `context.getSharedPreferences("frust.ss.<store>", MODE_PRIVATE)`.
fn get_prefs<'local>(
    env: &mut Env<'local>,
    context: &JObject,
    prefs_name: &str,
) -> Result<JObject<'local>, jni::errors::Error> {
    let name = env.new_string(prefs_name)?;
    env.call_method(
        context,
        jni_str!("getSharedPreferences"),
        jni_sig!("(Ljava/lang/String;I)Landroid/content/SharedPreferences;"),
        &[JValue::Object(&name), JValue::Int(MODE_PRIVATE)],
    )?
    .l()
}

/// `prefs.getString(key, null)`, or `None` if absent.
fn read_string(
    env: &mut Env,
    prefs: &JObject,
    key: &str,
) -> Result<Option<String>, jni::errors::Error> {
    let key_jstr = env.new_string(key)?;
    let null = JObject::null();
    let value = env
        .call_method(
            prefs,
            jni_str!("getString"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(&key_jstr), JValue::Object(&null)],
        )?
        .l()?;
    if value.is_null() {
        return Ok(None);
    }
    let value = env.cast_local::<JString>(value)?;
    Ok(Some(value.to_string()))
}

/// `prefs.edit().putString(key, value).apply()`.
fn write_string(
    env: &mut Env,
    prefs: &JObject,
    key: &str,
    value: &str,
) -> Result<(), jni::errors::Error> {
    let editor = edit(env, prefs)?;
    let key_jstr = env.new_string(key)?;
    let value_jstr = env.new_string(value)?;
    env.call_method(
        &editor,
        jni_str!("putString"),
        jni_sig!(
            "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;"
        ),
        &[JValue::Object(&key_jstr), JValue::Object(&value_jstr)],
    )?;
    apply(env, &editor)
}

/// `prefs.edit()`.
fn edit<'local>(
    env: &mut Env<'local>,
    prefs: &JObject,
) -> Result<JObject<'local>, jni::errors::Error> {
    env.call_method(
        prefs,
        jni_str!("edit"),
        jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
        &[],
    )?
    .l()
}

/// `editor.apply()` (async, thread-safe persist; the in-memory value is
/// visible immediately — matching the shared-preferences backend).
fn apply(env: &mut Env, editor: &JObject) -> Result<(), jni::errors::Error> {
    env.call_method(editor, jni_str!("apply"), jni_sig!("()V"), &[])?;
    Ok(())
}

/// Every key currently in this store's `SharedPreferences` file. The file is
/// this store's exclusively (its name is `frust.ss.<store>`), so all keys are
/// ours — no namespace filtering, like the [`crate::file`] backend. Runs inside
/// [`Env::with_local_frame`] so the map iterator's per-entry local refs are
/// freed together.
fn all_keys(env: &mut Env, prefs: &JObject) -> Result<Vec<String>, jni::errors::Error> {
    env.with_local_frame(16, |env| -> Result<Vec<String>, jni::errors::Error> {
        let all = env
            .call_method(
                prefs,
                jni_str!("getAll"),
                jni_sig!("()Ljava/util/Map;"),
                &[],
            )?
            .l()?;
        let map = env.cast_local::<JMap>(all)?;
        let mut iter = map.iter(env)?;
        let mut keys = Vec::new();
        while let Some(entry) = iter.next(env)? {
            let key_obj = entry.key(env)?;
            let key = env.cast_local::<JString>(key_obj)?;
            keys.push(key.to_string());
        }
        Ok(keys)
    })
}

// --- Backend impl -----------------------------------------------------------

impl Backend for AndroidStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        with_context(|env, context| {
            // Read the stored Base64 ciphertext (JNI).
            let stored = run_jni(env, |env| {
                let prefs = get_prefs(env, context, &self.store_id)?;
                read_string(env, &prefs, key)
            })?;
            let Some(encoded) = stored else {
                return Ok(None);
            };
            // Decode + unframe in pure Rust (host-tested). A corrupted entry is
            // a typed Storage error, not a silent None — this is a secure store.
            let bytes = framing::b64_decode(&encoded).ok_or_else(|| {
                SecureStorageError::Storage(
                    "corrupted secure entry: value is not valid Base64".to_string(),
                )
            })?;
            let (iv, ciphertext) = framing::unframe(&bytes).ok_or_else(|| {
                SecureStorageError::Storage(
                    "corrupted secure entry: truncated IV framing".to_string(),
                )
            })?;
            // Decrypt (JNI). The key must already exist to read an existing
            // value; `get_or_create_key` returns it. A gated store
            // interposes the biometric prompt between cipher init and
            // `doFinal`, unlocking the auth-bound key via the `CryptoObject`;
            // a plain store runs the two back to back. GCM tag verification in
            // `doFinal` surfaces any tampering as a mapped exception.
            let cipher = run_jni(env, |env| {
                let cipher_key = get_or_create_key(env, &self.store_id, self.auth.as_ref())?;
                init_decrypt(env, &cipher_key, iv)
            })?;
            let cipher = match &self.auth {
                Some(opts) => authenticate(env, context, opts, &cipher)?,
                None => cipher,
            };
            let plaintext = run_jni(env, |env| cipher_final(env, &cipher, ciphertext))?;
            let text = String::from_utf8(plaintext).map_err(|e| {
                SecureStorageError::Storage(format!("decrypted value is not valid UTF-8: {e}"))
            })?;
            Ok(Some(text))
        })
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        with_context(|env, context| {
            // Encrypt (JNI) — a fresh random IV per write. As in `get`, a
            // gated store interposes the biometric prompt between cipher
            // init and `doFinal`; a plain store runs them back to back.
            let (cipher, iv) = run_jni(env, |env| {
                let cipher_key = get_or_create_key(env, &self.store_id, self.auth.as_ref())?;
                init_encrypt(env, &cipher_key)
            })?;
            let cipher = match &self.auth {
                Some(opts) => authenticate(env, context, opts, &cipher)?,
                None => cipher,
            };
            let ciphertext = run_jni(env, |env| cipher_final(env, &cipher, value.as_bytes()))?;
            // Frame + Base64 in pure Rust.
            let encoded = framing::b64_encode(&framing::frame(&iv, &ciphertext));
            // Persist the Base64 string (JNI).
            run_jni(env, |env| {
                let prefs = get_prefs(env, context, &self.store_id)?;
                write_string(env, &prefs, key, &encoded)
            })
        })
    }

    fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let prefs = get_prefs(env, context, &self.store_id)?;
                let editor = edit(env, &prefs)?;
                let key_jstr = env.new_string(key)?;
                env.call_method(
                    &editor,
                    jni_str!("remove"),
                    jni_sig!("(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;"),
                    &[JValue::Object(&key_jstr)],
                )?;
                apply(env, &editor)
            })
        })
    }

    fn contains(&self, key: &str) -> Result<bool, SecureStorageError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let prefs = get_prefs(env, context, &self.store_id)?;
                let key_jstr = env.new_string(key)?;
                env.call_method(
                    &prefs,
                    jni_str!("contains"),
                    jni_sig!("(Ljava/lang/String;)Z"),
                    &[JValue::Object(&key_jstr)],
                )?
                .z()
            })
        })
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                let prefs = get_prefs(env, context, &self.store_id)?;
                all_keys(env, &prefs)
            })
        })
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        with_context(|env, context| {
            run_jni(env, |env| {
                // The `frust.ss.<store>` prefs file is this store's exclusively
                // (its name carries the store namespace), so `clear()` removing
                // every entry never touches another store's or library's data —
                // matching the file backend's whole-file clear. The Keystore
                // key is intentionally left in place; a subsequent `set`
                // re-uses it.
                let prefs = get_prefs(env, context, &self.store_id)?;
                let editor = edit(env, &prefs)?;
                env.call_method(
                    &editor,
                    jni_str!("clear"),
                    jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
                    &[],
                )?;
                apply(env, &editor)
            })
        })
    }
}
