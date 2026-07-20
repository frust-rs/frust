//! The Android [`Backend`] — AES-256-GCM in `AndroidKeyStore`, ciphertext in a
//! per-store `SharedPreferences` file, all over plain JNI (no Kotlin glue).
//!
//! # Design (task S03, Plan Phase 3 — storage only)
//!
//! One AES-256-GCM key per store lives **inside** `AndroidKeyStore` under the
//! alias `frust.ss.<store>` ([`framing::store_id`]) — generated directly with
//! [`KeyGenParameterSpec`](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec)
//! (`PURPOSE_ENCRYPT | PURPOSE_DECRYPT`, `GCM`/`NoPadding`, 256-bit). There is
//! **no RSA key-wrap layer** — `flutter_secure_storage` carries that only for
//! backward migration; a fresh backend generates the AES key straight in the
//! Keystore (research `RESEARCH.md` §2). Values are `Cipher
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
//! branches — every Keystore exception is handled tolerantly (research §2:
//! OEM quirks are unconfirmed anecdotes, so we map, not special-case).
//!
//! # Storage-only in this phase
//!
//! `AuthPolicy::Required` never reaches this backend: `lib.rs`'s `open_with`
//! rejects it with `NotAvailable(UnsupportedPlatform)` before any backend is
//! constructed (S05 adds the biometric gate). So this module implements plain
//! secure storage only.

#![allow(dead_code)] // AndroidStore is constructed by lib.rs's Android
// selection arm, which the parallel-wave freeze defers to the conductor's
// integration flip (see the completion summary). Until that one-line flip
// lands, nothing in this module is reachable on the host toolchain, so the
// whole module is allow(dead_code); remove this once the arm is flipped.

use jni::objects::{JByteArray, JMap, JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::framing;
use crate::{Backend, SecureStorageError};

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

/// A `frust.ss.<store>`-namespaced Android secure store: one Keystore AES-GCM
/// key + one `SharedPreferences` file, both keyed by [`Self::store_id`].
pub(crate) struct AndroidStore {
    /// `frust.ss.<store>` — used for **both** the Keystore key alias and the
    /// `SharedPreferences` file name (see the module doc).
    store_id: String,
}

impl AndroidStore {
    /// Open the named Android secure store.
    ///
    /// # Errors
    /// [`SecureStorageError::PlatformNotInitialized`] if the host shell never
    /// installed the `(JavaVM, Context)` handles (an old scaffold predating
    /// `nativeInitPlatform`) — probed here so the failure is loud and early
    /// rather than on the first read (matching `frust-shared-preferences`'
    /// `AndroidStore::standard`).
    pub(crate) fn open(name: &str) -> Result<Self, SecureStorageError> {
        // No-op handle probe: `with_context` maps a missing platform handle to
        // `PlatformNotInitialized` without doing any JNI work.
        with_context(|_env, _ctx| Ok(()))?;
        Ok(Self {
            store_id: framing::store_id(name),
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
fn run_jni<T>(
    env: &mut Env,
    f: impl FnOnce(&mut Env) -> Result<T, jni::errors::Error>,
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
    // biometric re-enrollment (research §2). Match on the simple class name so
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
/// is only required for app-defined classes (the S05 biometric helper).
fn get_or_create_key<'local>(
    env: &mut Env<'local>,
    alias: &str,
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
        generate_key(env, alias)
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
/// `KeyGenParameterSpec` (no RSA wrap).
fn generate_key<'local>(
    env: &mut Env<'local>,
    alias: &str,
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

// --- Encrypt / decrypt ------------------------------------------------------

/// Encrypt `plaintext` under `key`, returning `(iv, ciphertext)`. GCM in
/// `AndroidKeyStore` generates a fresh random IV per encryption
/// (`setRandomizedEncryptionRequired` defaults on), read back via
/// `Cipher.getIV()`.
fn encrypt(
    env: &mut Env,
    key: &JObject,
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), jni::errors::Error> {
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

    let plaintext_array = env.byte_array_from_slice(plaintext)?;
    let ciphertext_obj = env
        .call_method(
            &cipher,
            jni_str!("doFinal"),
            jni_sig!("([B)[B"),
            &[JValue::Object(&plaintext_array)],
        )?
        .l()?;
    let ciphertext_array = env.cast_local::<JByteArray>(ciphertext_obj)?;
    let ciphertext = env.convert_byte_array(&ciphertext_array)?;

    Ok((iv, ciphertext))
}

/// Decrypt `ciphertext` under `key` with the stored `iv`.
fn decrypt(
    env: &mut Env,
    key: &JObject,
    iv: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, jni::errors::Error> {
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

    let ciphertext_array = env.byte_array_from_slice(ciphertext)?;
    let plaintext_obj = env
        .call_method(
            &cipher,
            jni_str!("doFinal"),
            jni_sig!("([B)[B"),
            &[JValue::Object(&ciphertext_array)],
        )?
        .l()?;
    let plaintext_array = env.cast_local::<JByteArray>(plaintext_obj)?;
    env.convert_byte_array(&plaintext_array)
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
            // value; `get_or_create_key` returns it, and GCM tag verification
            // in `doFinal` surfaces any tampering as a mapped exception.
            let plaintext = run_jni(env, |env| {
                let cipher_key = get_or_create_key(env, &self.store_id)?;
                decrypt(env, &cipher_key, iv, ciphertext)
            })?;
            let text = String::from_utf8(plaintext).map_err(|e| {
                SecureStorageError::Storage(format!("decrypted value is not valid UTF-8: {e}"))
            })?;
            Ok(Some(text))
        })
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        with_context(|env, context| {
            // Encrypt (JNI) — a fresh random IV per write.
            let (iv, ciphertext) = run_jni(env, |env| {
                let cipher_key = get_or_create_key(env, &self.store_id)?;
                encrypt(env, &cipher_key, value.as_bytes())
            })?;
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
