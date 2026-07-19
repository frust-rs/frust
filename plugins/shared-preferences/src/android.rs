//! The Android [`Backend`] — `Context.getSharedPreferences`-backed, via JNI.
//!
//! Every call routes through [`frust_plugin::android::with_jni_env`], which
//! scoped-attaches the current thread and hands us a live [`Env`] + the
//! application [`Context`](JObject). The store is
//! `getSharedPreferences("frust_prefs", MODE_PRIVATE)` — `MODE_PRIVATE`
//! already scopes the file to this app's private data dir, so a
//! package-qualified name (`"<package>_frust_prefs"`) would be redundant; a
//! fixed name avoids an extra `getPackageName` round-trip.
//!
//! # Storage encoding — one tagged JSON string per key
//!
//! Android `SharedPreferences` stores natively-typed values (`Boolean`,
//! `Long`, `String`, …), but a generic [`Backend::get`] must recover the
//! exact [`PrefValue`] variant, and two of Frust's five types are *not*
//! distinguishable from a sibling by native type alone: an `f64` stored as
//! `putLong(f64::to_bits())` is indistinguishable from an `i64` (both
//! `Long`), and a `Vec<String>` stored as a JSON string is
//! indistinguishable from a plain `String`. Rather than a lossy native
//! store plus a side-band type registry, this backend stores **every** value
//! as a single tagged JSON string via `putString`, using the same
//! `{"t":<tag>,"v":<value>}` shape as the [`crate::file`] backend (with
//! `f64` as a 16-hex-digit `to_bits()` string for exact round-trip including
//! `NaN`). `serde_json` is already a crate dependency for the file backend,
//! so the encoder is shared rather than hand-rolled (the plugin crate is not
//! bound by the shells' no-serde rule). On-store Flutter compatibility is a
//! non-goal (Design Decision 4), so this encoding is Frust's to choose.
//!
//! # Namespace + `clear`
//!
//! Keys are prefixed with [`crate::KEY_PREFIX`] (`"frust."`) so this backend
//! never collides with — nor [`clear`](AndroidStore::clear) ever removes —
//! another library's entries sharing the app's prefs; [`keys`](AndroidStore::keys)
//! strips the prefix. Enumeration (`getAll`) runs inside
//! [`Env::with_local_frame`] so the per-entry key/value local references the
//! map iterator creates are freed together, never overflowing the frame.
//!
//! # Writes
//!
//! Writes go `edit()` → `putString`/`remove` → `apply()`. `apply()` (not
//! `commit()`) persists asynchronously — thread-safe and Flutter's choice
//! too; the in-memory value is visible immediately.

use jni::objects::{JMap, JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::{Backend, KEY_PREFIX, PrefValue, PrefsError};

/// The `SharedPreferences` file name — see the module doc for why a fixed
/// (rather than package-qualified) name is sufficient under `MODE_PRIVATE`.
const STORE_NAME: &str = "frust_prefs";

/// `SharedPreferences.MODE_PRIVATE` — a documented, stable framework
/// constant (`= 0`), not a value we look up reflectively.
const MODE_PRIVATE: i32 = 0;

/// Android `SharedPreferences`-backed preferences store.
///
/// Holds no JNI reference across calls (local references are frame-scoped);
/// each operation re-fetches the shared-prefs handle inside a fresh scoped
/// attachment, which Android returns from a cheap in-memory cache. This is
/// what keeps the store trivially `Send + Sync`.
pub(crate) struct AndroidStore;

impl AndroidStore {
    /// Open the app's Frust preferences store.
    ///
    /// # Errors
    /// [`PrefsError::PlatformNotInitialized`] if the host shell never
    /// installed the `(JavaVM, Context)` platform handles (an old scaffold
    /// predating `nativeInitPlatform`) — surfaced here at construction via a
    /// no-op handle probe, so the failure is loud and early rather than on
    /// the first read.
    pub(crate) fn standard() -> Result<Self, PrefsError> {
        // Probe the platform handles without doing JNI work: `with_jni_env`
        // returns `NotInitialized` pre-init (mapped to
        // `PlatformNotInitialized` via `PrefsError`'s `From`).
        frust_plugin::android::with_jni_env(|_env, _ctx| ())?;
        Ok(Self)
    }
}

/// Run `f` with the app's `SharedPreferences` handle inside a scoped JNI
/// attachment. Flattens the two error layers: a missing platform handle maps
/// to [`PrefsError::PlatformNotInitialized`]; any JNI failure to
/// [`PrefsError::Storage`].
fn with_prefs<T>(
    f: impl FnOnce(&mut Env, &JObject) -> Result<T, jni::errors::Error>,
) -> Result<T, PrefsError> {
    frust_plugin::android::with_jni_env(|env, context| {
        let name = env.new_string(STORE_NAME)?;
        let prefs = env
            .call_method(
                context,
                jni_str!("getSharedPreferences"),
                jni_sig!("(Ljava/lang/String;I)Landroid/content/SharedPreferences;"),
                &[JValue::Object(&name), JValue::Int(MODE_PRIVATE)],
            )?
            .l()?;
        f(env, &prefs)
    })?
    .map_err(|e| PrefsError::Storage(format!("Android SharedPreferences JNI error: {e}")))
}

/// `"frust."`-prefixed key.
fn namespaced(key: &str) -> String {
    format!("{KEY_PREFIX}{key}")
}

/// Read the JSON string currently stored at `nskey` (already namespaced), or
/// `None` if absent — `getString(nskey, null)`.
fn read_string(
    env: &mut Env,
    prefs: &JObject,
    nskey: &str,
) -> Result<Option<String>, jni::errors::Error> {
    let key = env.new_string(nskey)?;
    let null = JObject::null();
    let value = env
        .call_method(
            prefs,
            jni_str!("getString"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(&key), JValue::Object(&null)],
        )?
        .l()?;
    if value.is_null() {
        return Ok(None);
    }
    let jstr = env.cast_local::<JString>(value)?;
    Ok(Some(jstr.to_string()))
}

/// Enumerate this backend's `frust.`-prefixed keys. Returns `(namespaced,
/// stripped)` pairs so callers that need the raw store key (`clear`) and
/// those that need the public key (`keys`) share one enumeration. Runs
/// inside [`Env::with_local_frame`] (the map iterator's per-entry local refs
/// are freed together on frame pop).
fn frust_keys(env: &mut Env, prefs: &JObject) -> Result<Vec<(String, String)>, jni::errors::Error> {
    env.with_local_frame(
        16,
        |env| -> Result<Vec<(String, String)>, jni::errors::Error> {
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
            let mut out = Vec::new();
            while let Some(entry) = iter.next(env)? {
                let key_obj = entry.key(env)?;
                let jstr = env.cast_local::<JString>(key_obj)?;
                let full = jstr.to_string();
                if let Some(stripped) = full.strip_prefix(KEY_PREFIX) {
                    out.push((full.clone(), stripped.to_string()));
                }
            }
            Ok(out)
        },
    )
}

impl Backend for AndroidStore {
    fn get(&self, key: &str) -> Option<PrefValue> {
        with_prefs(|env, prefs| {
            let json = read_string(env, prefs, &namespaced(key))?;
            Ok(json.as_deref().and_then(decode))
        })
        // A tolerant `get` (matching the file backend): a pre-init or JNI
        // failure reads as "absent" rather than surfacing here — the typed
        // write paths (`set`/`remove`/`clear`) carry the error contract.
        .ok()
        .flatten()
    }

    fn set(&self, key: &str, value: PrefValue) -> Result<(), PrefsError> {
        let json = encode(&value);
        with_prefs(|env, prefs| {
            let nskey = env.new_string(namespaced(key))?;
            let nsval = env.new_string(&json)?;
            let editor = env
                .call_method(
                    prefs,
                    jni_str!("edit"),
                    jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
                    &[],
                )?
                .l()?;
            env.call_method(
                &editor,
                jni_str!("putString"),
                jni_sig!(
                    "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;"
                ),
                &[JValue::Object(&nskey), JValue::Object(&nsval)],
            )?;
            env.call_method(&editor, jni_str!("apply"), jni_sig!("()V"), &[])?;
            Ok(())
        })
    }

    fn remove(&self, key: &str) -> Result<(), PrefsError> {
        with_prefs(|env, prefs| {
            let nskey = env.new_string(namespaced(key))?;
            let editor = env
                .call_method(
                    prefs,
                    jni_str!("edit"),
                    jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
                    &[],
                )?
                .l()?;
            env.call_method(
                &editor,
                jni_str!("remove"),
                jni_sig!("(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;"),
                &[JValue::Object(&nskey)],
            )?;
            env.call_method(&editor, jni_str!("apply"), jni_sig!("()V"), &[])?;
            Ok(())
        })
    }

    fn clear(&self) -> Result<(), PrefsError> {
        with_prefs(|env, prefs| {
            let editor = env
                .call_method(
                    prefs,
                    jni_str!("edit"),
                    jni_sig!("()Landroid/content/SharedPreferences$Editor;"),
                    &[],
                )?
                .l()?;
            // Remove only `frust.`-namespaced keys — never another library's
            // entries in the shared prefs (Design Decision 4).
            for (full, _stripped) in frust_keys(env, prefs)? {
                let nskey = env.new_string(&full)?;
                env.call_method(
                    &editor,
                    jni_str!("remove"),
                    jni_sig!("(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;"),
                    &[JValue::Object(&nskey)],
                )?;
            }
            env.call_method(&editor, jni_str!("apply"), jni_sig!("()V"), &[])?;
            Ok(())
        })
    }

    fn contains(&self, key: &str) -> bool {
        with_prefs(|env, prefs| {
            let nskey = env.new_string(namespaced(key))?;
            env.call_method(
                prefs,
                jni_str!("contains"),
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&nskey)],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    fn keys(&self) -> Vec<String> {
        with_prefs(|env, prefs| {
            Ok(frust_keys(env, prefs)?
                .into_iter()
                .map(|(_full, stripped)| stripped)
                .collect())
        })
        .unwrap_or_default()
    }
}

/// Encode one [`PrefValue`] into its tagged JSON string (the file backend's
/// `{"t":<tag>,"v":<value>}` shape — see the module doc). `f64` is a
/// 16-hex-digit `to_bits()` string for exact round-trip.
fn encode(value: &PrefValue) -> String {
    let tagged = |t: &str, v: serde_json::Value| serde_json::json!({ "t": t, "v": v }).to_string();
    match value {
        PrefValue::Bool(v) => tagged("bool", serde_json::Value::Bool(*v)),
        PrefValue::I64(v) => tagged("i64", serde_json::Value::from(*v)),
        PrefValue::F64(v) => tagged(
            "f64",
            serde_json::Value::String(format!("{:016x}", v.to_bits())),
        ),
        PrefValue::Str(v) => tagged("string", serde_json::Value::String(v.clone())),
        PrefValue::StrList(v) => tagged(
            "string_list",
            serde_json::Value::Array(v.iter().cloned().map(serde_json::Value::String).collect()),
        ),
    }
}

/// Decode one tagged JSON string back into a [`PrefValue`]; `None` for
/// anything that doesn't match the expected shape (a corrupted single entry
/// degrades to "absent", never a panic — mirroring the file backend).
fn decode(json: &str) -> Option<PrefValue> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = value.get("t")?.as_str()?;
    let v = value.get("v")?;
    match tag {
        "bool" => v.as_bool().map(PrefValue::Bool),
        "i64" => v.as_i64().map(PrefValue::I64),
        "f64" => {
            let hex = v.as_str()?;
            let bits = u64::from_str_radix(hex, 16).ok()?;
            Some(PrefValue::F64(f64::from_bits(bits)))
        }
        "string" => v.as_str().map(str::to_string).map(PrefValue::Str),
        "string_list" => v
            .as_array()?
            .iter()
            .map(|entry| entry.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(PrefValue::StrList),
        _ => None,
    }
}
