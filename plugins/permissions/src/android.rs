//! The Android backend — runtime permissions driven from a
//! `dev.frust.permissions.FrustPermissionsHost` Kotlin host
//! (`plugins/permissions/platform/android/`) over this crate's own JNI
//! surface.
//!
//! # The frozen contract
//!
//! **`FrustPermissionsHost.kt` and this module build to this table — changing
//! it means updating both files together.** `dev.frust` is the embedding
//! module's exclusive package, so the host takes the `dev.frust.permissions`
//! subpackage (`docs/CODE_STANDARDS.md`'s Plugin Conventions); its package is
//! baked into the JNI symbol below, so it may never move once shipped. The
//! integer codes are [`crate::codes`]'s, pinned against the Kotlin constants
//! by `tests/kotlin_contract.rs`.
//!
//! ## Rust → Kotlin (static methods, resolved through the application
//! classloader — see [`load_host_class`])
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `status` | `(int kind) -> int` | a status code; [`codes::NOT_DECLARED`]; [`codes::NO_ACTIVITY`] when the host has no application context (→ [`PermissionError::PlatformNotInitialized`]) |
//! | `request` | `(long requestId, int kind) -> int` | a final status code; [`codes::PENDING`] after launching the prompt (the answer arrives via [`Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`] with the same `requestId`); [`codes::NO_ACTIVITY`] (→ [`PermissionError::NoActivity`]); [`codes::NOT_DECLARED`] |
//! | `undeclaredPermission` | `(int kind) -> String` | the first required permission missing from the manifest, or `null` — read only to name it in [`PermissionError::NotDeclared`] |
//! | `openSettings` | `() -> int` | [`SETTINGS_OPENED`]; [`SETTINGS_FAILED`]; [`codes::NO_ACTIVITY`] when the host has no application context (→ [`PermissionError::PlatformNotInitialized`]) |
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` export)
//!
//! - [`Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`]`(env, class, requestId: jlong, code: jint)`
//!   — a status code, or [`codes::NO_ACTIVITY`] when the posted launch found
//!   the Activity finishing.
//!
//! The export upgrades its [`jni::EnvUnowned`] via
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind`, and
//! resolves with [`LogErrorAndDefault`] — the crate's no-unwind-across-FFI
//! guarantee (`docs/CODE_STANDARDS.md`'s Language Idioms), with no direct
//! `catch_unwind` here. This module holds no `unsafe` block; the export's
//! `#[unsafe(no_mangle)]` attribute is its only `unsafe` token. A `no_mangle`
//! symbol is never stripped by LTO, so it needs no app-crate hook.
//!
//! # Waiting for an answer, keyed by request id
//!
//! Each [`request`] that reaches the prompt mints a fresh id from
//! [`NEXT_REQUEST_ID`], arms a [`PENDING`] slot for it *before* the JNI call
//! (so an answer relayed while `request` is still returning cannot be
//! missed), and blocks on [`ANSWERED`] for up to [`REQUEST_TIMEOUT`]. The host
//! echoes the id back unchanged, so two concurrent requests never answer each
//! other, and a late answer to a request that already timed out finds no slot
//! and is dropped.
//!
//! A timeout, or an answer that is not a status, is never turned into a
//! fabricated grant or denial: [`request`] re-reads [`status`] and returns
//! that. The wait happens outside the scoped JNI attachment
//! ([`frust_plugin::android::with_jni_env`] has returned), so a blocked
//! caller pins no JVM attachment.
//!
//! # One request at a time
//!
//! [`request`] holds the process-wide [`REQUEST_LOCK`] from just after the
//! UI-thread check until it returns, so concurrent `request` calls run one
//! after another, each with its own prompt. A caller that waited re-reads
//! [`status`] once it holds the lock, so a permission the previous request
//! decided returns without another prompt. The Kotlin host keeps its own
//! guard for the case this lock cannot see: a request that timed out here
//! while its prompt is still on screen. Until that prompt is answered, the
//! host launches nothing and returns the current status code. The lock is a
//! plain `std::sync::Mutex<()>` taken through this module's poison-tolerant
//! `lock`; it is not factored out for a host test because this module builds
//! for Android only, and `tests/kotlin_contract.rs` pins its use by a source
//! scan instead.
//!
//! # Never on the UI thread
//!
//! The prompt is launched and answered on the main `Looper`; a caller parked
//! there would wait on its own wake-up until the timeout. So [`request`]
//! calls [`ensure_off_ui_thread`] before arming anything and reports
//! [`PermissionError::UiThread`]. A permission that is already decided
//! (granted, blocked, restricted) returns first, and is exempt.
//!
//! Feature gating is the crate root's: a permission whose cargo feature is off
//! never reaches this module, so no JNI work happens for it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::codes;
use crate::{Permission, PermissionError, PermissionStatus};

/// The Kotlin host's fully-qualified class name, in the dotted form
/// `ClassLoader.loadClass` expects. Looked up through the application
/// classloader: a bare `FindClass` on a JNI worker thread sees only the
/// bootstrap loader, never app classes. One contract with the Kotlin module's
/// `namespace`/`package` declaration.
const HOST_CLASS_BINARY: &str = "dev.frust.permissions.FrustPermissionsHost";

/// `openSettings` started the Settings activity.
const SETTINGS_OPENED: jint = 0;
/// `openSettings` could not start the Settings activity.
const SETTINGS_FAILED: jint = -1;

/// How long [`request`] waits for `nativeOnPermissionResult` after the host
/// reports [`codes::PENDING`], before re-reading [`status`].
///
/// **Not a platform value** — the system permission dialog has no published
/// deadline (a user may leave it on screen indefinitely). Two minutes is long
/// enough that a real answer is never cut short, short enough that a caller
/// paired with `frust::spawn_blocking` cannot pin a blocking-pool thread
/// forever if the host never relays a result (the process-death case, or an
/// Activity recreated mid-prompt). The camera plugin uses the same bound for
/// the same reason.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The cached `FrustPermissionsHost` class: one global reference for the
/// process, loaded on first use. A racing loser's reference is dropped
/// immediately (`Global`'s own `Drop` releases it).
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Outstanding requests by id: `None` while waiting, `Some(code)` once
/// `nativeOnPermissionResult` answered. The waiter removes its own slot.
static PENDING: LazyLock<Mutex<HashMap<u64, Option<i32>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Woken by `nativeOnPermissionResult`; waited on by [`request`].
static ANSWERED: Condvar = Condvar::new();

/// Held by [`request`] for the whole request, so concurrent requests run one
/// after another (module doc, "One request at a time"). Never taken by
/// [`deliver`], so the answer that ends a held request can always arrive.
static REQUEST_LOCK: Mutex<()> = Mutex::new(());

/// The next request id. Starts at 1 so a zeroed id is never live.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// The crate root's `status` arm: one host call, never blocks.
pub(crate) fn status(p: Permission) -> Result<PermissionStatus, PermissionError> {
    let kind = codes::kind_code(p);
    let code = with_host(|env, class| {
        run_jni(env, "FrustPermissionsHost.status", |env| {
            env.call_static_method(
                class,
                jni_str!("status"),
                jni_sig!("(I)I"),
                &[JValue::Int(kind)],
            )?
            .i()
        })
    })?;
    match code {
        codes::NOT_DECLARED => Err(not_declared(p)),
        codes::NO_ACTIVITY => Err(PermissionError::PlatformNotInitialized),
        other => codes::status_from_android_code(other).ok_or_else(|| {
            PermissionError::Platform(format!("host status returned an unknown code {other}"))
        }),
    }
}

/// The crate root's `request` arm. Blocks while the prompt is up; see the
/// module doc.
///
/// One request runs at a time: this holds [`REQUEST_LOCK`] from the
/// UI-thread check until it returns, so a concurrent call waits for this one
/// to finish (module doc, "One request at a time").
pub(crate) fn request(p: Permission) -> Result<PermissionStatus, PermissionError> {
    let current = status(p)?;
    if is_decided(current) {
        return Ok(current);
    }
    with_host(|env, _class| ensure_off_ui_thread(env))?;

    let _one_at_a_time = lock(&REQUEST_LOCK);
    // A request that held the lock before this one may have decided `p`.
    let current = status(p)?;
    if is_decided(current) {
        return Ok(current);
    }

    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    lock(&PENDING).insert(request_id, None);

    let kind = codes::kind_code(p);
    let code = with_host(|env, class| {
        run_jni(env, "FrustPermissionsHost.request", |env| {
            env.call_static_method(
                class,
                jni_str!("request"),
                jni_sig!("(JI)I"),
                // `u64` -> `jlong` reinterprets the same 64 bits; the host
                // treats the id as opaque and the export casts it back.
                &[JValue::Long(request_id as jlong), JValue::Int(kind)],
            )?
            .i()
        })
    });
    let code = match code {
        Ok(code) => code,
        Err(err) => {
            lock(&PENDING).remove(&request_id);
            return Err(err);
        }
    };
    if code != codes::PENDING {
        lock(&PENDING).remove(&request_id);
        return match code {
            codes::NO_ACTIVITY => Err(PermissionError::NoActivity),
            codes::NOT_DECLARED => Err(not_declared(p)),
            other => codes::status_from_android_code(other).ok_or_else(|| {
                PermissionError::Platform(format!("host request returned an unknown code {other}"))
            }),
        };
    }

    match await_answer(request_id) {
        Some(codes::NO_ACTIVITY) => Err(PermissionError::NoActivity),
        Some(code) => match codes::status_from_android_code(code) {
            Some(answer) => Ok(answer),
            None => status(p),
        },
        None => {
            log::warn!(
                "frust-permissions: no answer within {}s; re-reading the status",
                REQUEST_TIMEOUT.as_secs()
            );
            status(p)
        }
    }
}

/// The crate root's `open_settings` arm. Fire-and-forget.
pub(crate) fn open_settings() -> Result<(), PermissionError> {
    let code = with_host(|env, class| {
        run_jni(env, "FrustPermissionsHost.openSettings", |env| {
            env.call_static_method(class, jni_str!("openSettings"), jni_sig!("()I"), &[])?
                .i()
        })
    })?;
    match code {
        SETTINGS_OPENED => Ok(()),
        SETTINGS_FAILED => Err(PermissionError::Platform(
            "the Settings page could not be opened".to_string(),
        )),
        codes::NO_ACTIVITY => Err(PermissionError::PlatformNotInitialized),
        other => Err(PermissionError::Platform(format!(
            "host openSettings returned an unknown code {other}"
        ))),
    }
}

/// Whether `s` is final: no prompt can change it, so [`request`] returns it
/// without launching one (and without the UI-thread guard).
fn is_decided(s: PermissionStatus) -> bool {
    matches!(
        s,
        PermissionStatus::Granted | PermissionStatus::Blocked | PermissionStatus::Restricted
    )
}

/// Block until request `request_id` is answered or [`REQUEST_TIMEOUT`]
/// elapses, removing its slot either way. `None` on timeout.
fn await_answer(request_id: u64) -> Option<i32> {
    let pending = lock(&PENDING);
    let (mut pending, _timeout) = ANSWERED
        .wait_timeout_while(pending, REQUEST_TIMEOUT, |pending| {
            matches!(pending.get(&request_id), Some(None))
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pending.remove(&request_id).flatten()
}

/// Record the host's answer for `request_id` and wake the waiters. An id with
/// no slot (the request timed out) is dropped.
fn deliver(request_id: u64, code: i32) {
    let mut pending = lock(&PENDING);
    match pending.get_mut(&request_id) {
        Some(slot) => {
            *slot = Some(code);
            drop(pending);
            ANSWERED.notify_all();
        }
        None => log::debug!("frust-permissions: dropped a late answer for request {request_id}"),
    }
}

/// [`PermissionError::NotDeclared`] naming the permission the host reports
/// missing, as the `&'static str` [`codes::android_permissions`] holds.
fn not_declared(p: Permission) -> PermissionError {
    let missing = with_host(|env, class| undeclared_permission(env, class, codes::kind_code(p)));
    let fallback = codes::android_permissions(p, 34).first().copied();
    let permission = match missing {
        Ok(Some(name)) => static_permission_name(p, &name).or(fallback),
        _ => fallback,
    };
    PermissionError::NotDeclared {
        permission: permission.unwrap_or("android.permission"),
    }
}

/// `name` as the static string [`codes::android_permissions`] carries for `p`
/// on any supported SDK band.
fn static_permission_name(p: Permission, name: &str) -> Option<&'static str> {
    [29, 33, 34]
        .into_iter()
        .flat_map(|sdk| codes::android_permissions(p, sdk).iter().copied())
        .find(|candidate| *candidate == name)
}

/// `FrustPermissionsHost.undeclaredPermission(kind)`, decoded.
fn undeclared_permission(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    kind: jint,
) -> Result<Option<String>, PermissionError> {
    run_jni(env, "FrustPermissionsHost.undeclaredPermission", |env| {
        let name = env
            .call_static_method(
                class,
                jni_str!("undeclaredPermission"),
                jni_sig!("(I)Ljava/lang/String;"),
                &[JValue::Int(kind)],
            )?
            .l()?;
        if name.is_null() {
            return Ok(None);
        }
        let name = env.cast_local::<JString>(name)?;
        Ok(Some(name.try_to_string(env)?))
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- JNI plumbing (the `plugins/auth-session/src/android.rs` shape) --------

/// Run `f` with a live [`Env`] and the cached host class inside a scoped JNI
/// attachment. A missing platform handle is
/// [`PermissionError::PlatformNotInitialized`], checked before any JNI work.
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, PermissionError>,
) -> Result<T, PermissionError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(PermissionError::PlatformNotInitialized)
        }
        Err(other) => Err(PermissionError::Platform(format!(
            "android permissions backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, PermissionError> {
    if let Some(class) = HOST_CLASS.get() {
        return Ok(class);
    }
    let class = load_host_class(env, context)?;
    Ok(HOST_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass(HOST_CLASS_BINARY)`, promoted to a
/// process-lifetime global reference.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, PermissionError> {
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

    // Checked by hand: a `ClassNotFoundException` here is the one expected
    // failure (the plugin's Gradle module is not wired into the app), so it
    // gets the concrete fix rather than `run_jni`'s generic report.
    let class = env.call_method(
        &loader,
        jni_str!("loadClass"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
        &[JValue::Object(&name)],
    );
    if env.exception_check() {
        env.exception_clear();
        return Err(PermissionError::Platform(
            "frust-permissions Gradle module is not linked — add the plugin from the frust TUI \
             (Add Plugin), or include the module by hand (see the frust-permissions README)"
                .to_string(),
        ));
    }
    let class = class.and_then(|value| value.l()).map_err(|err| {
        PermissionError::Platform(format!(
            "android permissions backend: ClassLoader.loadClass: {err}"
        ))
    })?;
    let class = run_jni(env, "casting FrustPermissionsHost", |env| {
        env.cast_local::<JClass>(class)
    })?;
    run_jni(env, "pinning FrustPermissionsHost", |env| {
        env.new_global_ref(class)
    })
}

/// Run a sequence of JNI calls, converting a pending Java exception into
/// [`PermissionError::Platform`] naming `op` and the exception's class only,
/// never its message. `jni` 0.22 leaves a thrown exception pending after the
/// failing call, so this always checks and clears it before returning.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, PermissionError> {
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
        if env.exception_check() {
            env.exception_clear();
        }
        return Err(PermissionError::Platform(format!(
            "android permissions backend: {op}: {class_name}"
        )));
    }
    result.map_err(|err| {
        PermissionError::Platform(format!("android permissions backend: {op}: {err}"))
    })
}

/// Refuse a call on the main `Looper` with [`PermissionError::UiThread`].
///
/// The test is Android's own: `Looper.myLooper() == Looper.getMainLooper()`.
/// A plain Rust worker thread has no `Looper`, so `myLooper()` is null there
/// and `IsSameObject(null, mainLooper)` is false. `android.os.Looper` is a
/// framework class, so the bare `find_class` works on any thread.
fn ensure_off_ui_thread(env: &mut Env<'_>) -> Result<(), PermissionError> {
    let on_ui_thread = run_jni(env, "android.os.Looper.myLooper", |env| {
        let looper_class = env.find_class(jni_str!("android/os/Looper"))?;
        let mine = env
            .call_static_method(
                &looper_class,
                jni_str!("myLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        let main = env
            .call_static_method(
                &looper_class,
                jni_str!("getMainLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        env.is_same_object(&mine, &main)
    })?;
    if on_ui_thread {
        return Err(PermissionError::UiThread);
    }
    Ok(())
}

// --- Kotlin -> Rust JNI export -------------------------------------------

/// `Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`
/// — the host's answer to the [`request`] that minted `request_id`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    code: jint,
) {
    env.with_env(|_env| {
        // The inverse of `request`'s own `as jlong`.
        deliver(request_id as u64, code);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
