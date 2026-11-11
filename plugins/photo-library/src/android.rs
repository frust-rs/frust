//! Android: the system photo picker, driven from a
//! `dev.frust.photolibrary.FrustPhotoLibraryHost` Kotlin host
//! (`plugins/photo-library/platform/android/`) over this crate's own JNI
//! surface.
//!
//! # The frozen contract
//!
//! **`FrustPhotoLibraryHost.kt` and this module build to this table — changing
//! it means updating both files together.** `dev.frust` is the embedding
//! module's exclusive package, so the host takes the `dev.frust.photolibrary`
//! subpackage (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions); its package is
//! baked into the JNI symbols below, so it may never move once shipped. The
//! integer codes are this module's `const`s, pinned against the Kotlin
//! constants by `tests/kotlin_contract.rs`.
//!
//! ## Rust → Kotlin (static methods, resolved through the application
//! classloader — see [`load_host_class`])
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `pick` | `(long requestId, int filter, int limit, String destDir) -> int` | [`PENDING`] after posting the launch (the answer arrives through `nativeOnPickResult` or `nativeOnPickError` below with the same `requestId`); [`NO_ACTIVITY`] (→ [`PhotoError::NoPresenter`]); [`BUSY`]; [`NOT_INITIALIZED`] (→ [`PhotoError::PlatformNotInitialized`]); [`LAUNCH_FAILED`] |
//! | `cancel` | `(long requestId) -> void` | nothing; an unknown or already-answered id is ignored (see "Cancel and progress") |
//! | `exportOriginal` | `(long requestId, String uri, String destPath) -> int` | [`PENDING`] after queuing the copy (answered through `nativeOnExportDone`); [`NOT_INITIALIZED`]; `NOT_FOUND` for an id that is not a `MediaStore` content URI |
//! | `cancelExport` | `(long requestId) -> void` | nothing; an unknown or already-answered id is ignored |
//! | `saveToLibrary` | `(long requestId, String sourcePath, boolean isVideo, String displayName, String album) -> int` | [`PENDING`] after queuing the save (answered through `nativeOnSaveDone`); [`NOT_INITIALIZED`] |
//!
//! `filter` is [`FILTER_IMAGES`], [`FILTER_VIDEOS`] or
//! [`FILTER_IMAGES_AND_VIDEOS`]; `limit` is [`PickOptions::limit`] (0 = the
//! platform maximum); `destDir` is a fresh, empty directory from the pick
//! cache that the host copies into.
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` exports)
//!
//! - [`Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickResult`]`(env, class, requestId: jlong, paths: String[], names: String[], mimes: String[])`
//!   — one staging copy per chosen item, in pick order, with the provider's
//!   display name and MIME type (each element may be null). Three empty
//!   arrays mean the user cancelled, or the launching Activity was destroyed
//!   while the picker was up.
//! - [`Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickError`]`(env, class, requestId: jlong, code: jint)`
//!   — [`NO_ACTIVITY`], [`LAUNCH_FAILED`], [`COPY_FAILED`] or [`CANCELLED`]
//!   (→ [`PhotoError::Cancelled`]).
//! - [`Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickProgress`]`(env, class, requestId: jlong, done: jint, items: jint, fraction: jdouble)`
//!   — copy progress, from the host's copy thread; never an answer.
//! - `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportProgress(env, class, requestId: jlong, done: jlong, total: jlong)`
//!   — bytes copied, once per chunk; absent when the size is unknown.
//! - `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportDone(env, class, requestId: jlong, code: jint, mime: String, name: String)`
//!   — an export's answer, with the provider's MIME type and display name.
//! - `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnSaveDone(env, class, requestId: jlong, code: jint, uri: String)`
//!   — a save's answer, with the new asset's URI.
//!
//! Every export upgrades its [`jni::EnvUnowned`] via
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind`, and
//! resolve with [`LogErrorAndDefault`] — the crate's no-unwind-across-FFI
//! guarantee (`docs/CODE_STANDARDS.md`'s Language Idioms), with no direct
//! `catch_unwind` here. This module holds no `unsafe` block; each export's
//! `#[unsafe(no_mangle)]` attribute is its only `unsafe` token.
//!
//! # Copies and names
//!
//! The host copies each chosen content URI into `destDir` under a staging
//! name built from its index alone (`.staging-<n>`), never from platform text.
//! [`pick`] then sanitises the reported display name with the pick cache's
//! own [`pick_cache::sanitize_file_name`] (one naming policy on every
//! platform), de-duplicates it within the call, and renames the copy. A host
//! path outside `destDir` is refused. Any failure, and a cancel, removes the
//! call's directory.
//!
//! # Waiting for the answer, keyed by request id
//!
//! [`pick`] mints a fresh id from [`NEXT_REQUEST_ID`], arms a [`WAITS`] slot
//! for it *before* the JNI call (so an answer relayed while `pick` is still
//! returning cannot be missed), and blocks on [`ANSWERED`]. There is **no
//! deadline**: a person may keep the picker open as long as they like, and the
//! host answers every [`PENDING`] request exactly once — with the copies, with
//! a cancel, with an error, or with a cancel when the launching Activity is
//! destroyed. The first answer for an id wins; anything later is dropped. The
//! wait happens outside the scoped JNI attachment
//! ([`frust_plugin::android::with_jni_env`] has returned), so a blocked caller
//! pins no JVM attachment.
//!
//! # One pick at a time
//!
//! [`pick`] takes the crate-wide [`pick_cache::PickGuard`] just after the
//! UI-thread check and holds it until it returns. A second pick while one is
//! in flight, or while `clear_pick_cache` runs, is refused at once with
//! [`PhotoError::Busy`] rather than queued behind a picker that may stay up
//! indefinitely. The Kotlin host keeps the same rule on its side and answers
//! [`BUSY`].
//!
//! # Never on the UI thread
//!
//! The picker is launched and answered on the main `Looper`; a caller parked
//! there would wait on its own wake-up forever. So [`pick`] calls
//! [`ensure_off_ui_thread`] before any other JNI work and reports
//! [`PhotoError::UiThread`].
//!
//! # Cancel and progress
//!
//! A [`CancelToken`] already cancelled fails the pick before anything runs.
//! Once the host answered [`PENDING`], the token's hook calls the host's
//! `cancel` (the system picker has no dismissal API, so an open picker stays
//! up and its result is discarded; running copies stop between 64 KiB chunks
//! and delete their staging files) and then records a local cancel, so the
//! wait ends at once without depending on the host's [`CANCELLED`] answer.
//! The progress export only stores the latest value in the request's
//! [`WAITS`] slot and wakes the waiter; the waiter forwards it through a
//! [`ProgressThrottle`] after releasing the lock, and reports the final
//! `{items, done: items, fraction: 1.0}` once the copies are renamed.
//!
//! # Library browsing
//!
//! With the `library` feature, `android_library` adds the `albums`, `assets`,
//! `thumbnail` and `manageLimitedSelection` host calls and refuses the UI
//! thread on each; see that module and the Kotlin host's contract table.
//!
//! # Change feed
//!
//! With the `library` feature, `android_changes` adds the `changeSnapshot`,
//! `startChangeObserver` and `stopChangeObserver` host calls and the
//! `nativeOnLibraryChanged` export: tokens from `MediaStore` versions,
//! generations and id sets, and a `ContentObserver` wake. Its token and diff
//! logic is platform-free and checked on the host; see that module.
//!
//! # Originals and saves
//!
//! `android_originals` implements `export_original` (`library`) and `save`
//! (`save`) with its own request map, so neither takes the pick guard or the
//! host's single pick slot; see that module.
//!
//! # Metadata
//!
//! With the `library` feature, `asset_metadata` refuses the UI thread and hands over to
//! `android_metadata`, which adds the `assetMetadata` host call. That module also holds the
//! `DecodeResult` read and the EXIF conversion the decoder shares, so it is compiled whenever
//! `decode` or `library` is on; see that module.
//!
//! # Decoding
//!
//! With the `decode` feature, the crate root's shared precheck (a cancelled
//! token, a zero cap) runs first; `decode_file` then refuses the UI thread
//! and hands over to `android_decode`, which adds the `decodeHeader`,
//! `decodeInto` and `cancelDecode` host calls and holds this crate's one
//! Android `unsafe` block (wrapping the pixel buffer for `decodeInto`); see
//! that module.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, OnceLock};

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JObjectArray, JString, JValue};
use jni::refs::Global;
use jni::sys::{jdouble, jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::cancel::ProgressThrottle;
use crate::media_types::classify_path;
use crate::pick_cache;
use crate::{
    CancelToken, MediaFilter, MediaKind, PhotoError, PickOptions, PickProgress, PickedItem,
};

/// The Kotlin host's fully-qualified class name, in the dotted form
/// `ClassLoader.loadClass` expects. Looked up through the application
/// classloader: a bare `FindClass` on a JNI worker thread sees only the
/// bootstrap loader, never app classes. One contract with the Kotlin module's
/// `namespace`/`package` declaration.
const HOST_CLASS_BINARY: &str = "dev.frust.photolibrary.FrustPhotoLibraryHost";

/// `pick` posted the launch; the answer arrives through an export.
const PENDING: jint = 0;
/// No resumed Activity to launch from, or it was finishing by launch time.
const NO_ACTIVITY: jint = 1;
/// The host already has a pick in flight.
const BUSY: jint = 2;
/// The host has no application context (its init provider never ran).
const NOT_INITIALIZED: jint = 3;
/// The picker could not be launched.
const LAUNCH_FAILED: jint = 4;
/// A chosen item could not be copied.
const COPY_FAILED: jint = 5;
/// The host honoured a `cancel` for the request.
const CANCELLED: jint = 6;

/// Filter code: still images only.
const FILTER_IMAGES: jint = 0;
/// Filter code: videos only.
const FILTER_VIDEOS: jint = 1;
/// Filter code: images and videos.
const FILTER_IMAGES_AND_VIDEOS: jint = 2;

/// The cached `FrustPhotoLibraryHost` class: one global reference for the
/// process, loaded on first use. A racing loser's reference is dropped
/// immediately (`Global`'s own `Drop` releases it).
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Outstanding picks by id. The waiter removes its own slot.
static WAITS: LazyLock<Mutex<HashMap<u64, Slot>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Woken by the exports and the cancel hook; waited on by [`pick`].
static ANSWERED: Condvar = Condvar::new();

/// The next request id. Starts at 1 so a zeroed id is never live.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// One outstanding pick: its answer once there is one, and the latest
/// progress the waiter has not forwarded yet.
#[derive(Default)]
struct Slot {
    answer: Option<Answer>,
    progress: Option<PickProgress>,
}

/// The answer to one pick.
enum Answer {
    /// The staging copies (empty = the user dismissed the picker).
    Picked(Vec<HostCopy>),
    /// A `nativeOnPickError` code.
    Failed(jint),
    /// The pick's [`CancelToken`] fired.
    Cancelled,
}

/// One staging copy as the host reported it.
struct HostCopy {
    path: String,
    name: Option<String>,
    mime: Option<String>,
}

/// The crate root's `pick` arm. Blocks while the picker is up; see the
/// module doc.
pub(crate) fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    with_host(|env, _class| ensure_off_ui_thread(env))?;

    let _one_at_a_time = pick_cache::PickGuard::acquire()?;

    let dir = pick_cache::new_call_dir()?;
    let result = pick_into(options, &dir);
    if let Some(token) = &options.cancel {
        token.clear_hook();
    }
    if !matches!(&result, Ok(items) if !items.is_empty()) {
        remove_call_dir(&dir);
    }
    result
}

/// [`pick`] after the guards: one host call, the wait, and the renames.
fn pick_into(options: &PickOptions, dir: &Path) -> Result<Vec<PickedItem>, PhotoError> {
    let dest = dir.to_str().ok_or_else(|| {
        PhotoError::Platform("the pick cache path is not valid UTF-8".to_string())
    })?;
    let filter = filter_code(options.filter);
    let limit = jint::try_from(options.limit).unwrap_or(jint::MAX);

    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    lock(&WAITS).insert(request_id, Slot::default());

    let code = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.pick", |env| {
            let dest = env.new_string(dest)?;
            env.call_static_method(
                class,
                jni_str!("pick"),
                jni_sig!("(JIILjava/lang/String;)I"),
                // `u64` -> `jlong` reinterprets the same 64 bits; the host
                // treats the id as opaque and the exports cast it back.
                &[
                    JValue::Long(request_id as jlong),
                    JValue::Int(filter),
                    JValue::Int(limit),
                    JValue::Object(&dest),
                ],
            )?
            .i()
        })
    });
    let code = match code {
        Ok(code) => code,
        Err(err) => {
            lock(&WAITS).remove(&request_id);
            return Err(err);
        }
    };
    if code != PENDING {
        lock(&WAITS).remove(&request_id);
        return Err(error_for(code));
    }

    // Only after `PENDING`: a host `cancel` then always finds the request.
    if let Some(token) = &options.cancel {
        token.set_hook(move || cancel_request(request_id));
    }
    let throttle = options.progress.clone().map(ProgressThrottle::new);
    let answer = await_answer(request_id, throttle.as_ref());
    // A cancel that raced the host's answer still wins.
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    match answer {
        Answer::Cancelled => Err(PhotoError::Cancelled),
        Answer::Failed(code) => Err(error_for(code)),
        Answer::Picked(copies) => {
            let items = finish_copies(dir, options, copies)?;
            if let (Some(throttle), false) = (&throttle, items.is_empty()) {
                throttle.report(PickProgress {
                    items: items.len(),
                    done: items.len(),
                    fraction: 1.0,
                });
            }
            Ok(items)
        }
    }
}

fn is_cancelled(options: &PickOptions) -> bool {
    options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
}

/// The cancel hook for `request_id`: tell the host, then end the wait
/// locally. Runs on whichever thread called [`CancelToken::cancel`].
fn cancel_request(request_id: u64) {
    let sent = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.cancel", |env| {
            env.call_static_method(
                class,
                jni_str!("cancel"),
                jni_sig!("(J)V"),
                &[JValue::Long(request_id as jlong)],
            )?
            .v()
        })
    });
    if let Err(err) = sent {
        log::warn!("frust-photo-library: cancelling the pick on the host failed: {err}");
    }
    deliver(request_id, Answer::Cancelled);
}

/// The filter code for `filter`.
fn filter_code(filter: MediaFilter) -> jint {
    match filter {
        MediaFilter::Images => FILTER_IMAGES,
        MediaFilter::Videos => FILTER_VIDEOS,
        MediaFilter::ImagesAndVideos => FILTER_IMAGES_AND_VIDEOS,
    }
}

/// The typed error for a non-[`PENDING`] `pick` return or a
/// `nativeOnPickError` code.
fn error_for(code: jint) -> PhotoError {
    match code {
        NO_ACTIVITY => PhotoError::NoPresenter,
        BUSY => PhotoError::Busy,
        NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
        LAUNCH_FAILED => PhotoError::Platform("the photo picker could not be launched".to_string()),
        COPY_FAILED => PhotoError::Platform("a picked item could not be copied".to_string()),
        CANCELLED => PhotoError::Cancelled,
        other => PhotoError::Platform(format!(
            "photo picker host returned an unknown code {other}"
        )),
    }
}

/// Rename each staging copy to its sanitised, de-duplicated name in `dir`
/// and describe it. A host path outside `dir` is refused.
fn finish_copies(
    dir: &Path,
    options: &PickOptions,
    copies: Vec<HostCopy>,
) -> Result<Vec<PickedItem>, PhotoError> {
    let mut used = HashSet::new();
    let mut items = Vec::with_capacity(copies.len());
    for (n, copy) in copies.into_iter().enumerate() {
        let staging = PathBuf::from(&copy.path);
        if staging.parent() != Some(dir) {
            return Err(PhotoError::Platform(
                "photo picker host reported a copy outside the pick directory".to_string(),
            ));
        }
        let mime = copy.mime.filter(|m| !m.is_empty());
        let file_name = copy.name.filter(|n| !n.is_empty());
        let name = pick_cache::copy_name(file_name.as_deref(), mime.as_deref(), n, &mut used);
        let path = dir.join(&name);
        fs::rename(&staging, &path)?;
        let kind = kind_for(mime.as_deref(), &path, options.filter);
        let mime = mime.or_else(|| classify_path(&path).map(|(_, m)| m.to_owned()));
        items.push(PickedItem {
            path,
            kind,
            file_name,
            mime,
            asset_id: None,
            is_copy: true,
        });
    }
    Ok(items)
}

/// Image or video: from the MIME type, then the file extension, then the
/// filter (a single-kind filter can only return that kind).
fn kind_for(mime: Option<&str>, path: &Path, filter: MediaFilter) -> MediaKind {
    if let Some(mime) = mime {
        let mime = mime.to_ascii_lowercase();
        if mime.starts_with("image/") {
            return MediaKind::Image;
        }
        if mime.starts_with("video/") {
            return MediaKind::Video;
        }
    }
    if let Some((kind, _)) = classify_path(path) {
        return kind;
    }
    match filter {
        MediaFilter::Videos => MediaKind::Video,
        MediaFilter::Images | MediaFilter::ImagesAndVideos => MediaKind::Image,
    }
}

/// Best-effort removal of a call directory that returns nothing.
fn remove_call_dir(dir: &Path) {
    if let Err(err) = fs::remove_dir_all(dir) {
        log::debug!(
            "frust-photo-library: could not remove a pick directory: {:?}",
            err.kind()
        );
    }
}

/// Block until pick `request_id` is answered, forwarding its progress to
/// `throttle` on the way, always outside the [`WAITS`] lock. No deadline
/// (module doc, "Waiting for the answer").
fn await_answer(request_id: u64, throttle: Option<&ProgressThrottle>) -> Answer {
    loop {
        let (progress, answer) = next_event(request_id);
        let forward = !matches!(answer, Some(Answer::Cancelled | Answer::Failed(_)));
        if let (Some(progress), Some(throttle), true) = (progress, throttle, forward) {
            throttle.report(progress);
        }
        if let Some(answer) = answer {
            return answer;
        }
    }
}

/// Wait for news on `request_id`: its pending progress, and its answer once
/// there is one (the slot is then removed). The lock is released on return.
fn next_event(request_id: u64) -> (Option<PickProgress>, Option<Answer>) {
    let waits = lock(&WAITS);
    let mut waits = ANSWERED
        .wait_while(waits, |waits| {
            matches!(
                waits.get(&request_id),
                Some(Slot {
                    answer: None,
                    progress: None
                })
            )
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(slot) = waits.get_mut(&request_id) else {
        return (None, Some(Answer::Picked(Vec::new())));
    };
    let progress = slot.progress.take();
    if slot.answer.is_none() {
        return (progress, None);
    }
    let answer = waits.remove(&request_id).and_then(|slot| slot.answer);
    (progress, answer)
}

/// Record the answer for `request_id` and wake the waiters. The first answer
/// wins; an id with no slot, or one already answered, is dropped.
fn deliver(request_id: u64, answer: Answer) {
    let mut waits = lock(&WAITS);
    match waits.get_mut(&request_id) {
        Some(Slot {
            answer: slot @ None,
            ..
        }) => {
            *slot = Some(answer);
            drop(waits);
            ANSWERED.notify_all();
        }
        _ => log::debug!("frust-photo-library: dropped a late answer for request {request_id}"),
    }
}

/// Store the latest progress for an unanswered `request_id` and wake the
/// waiter. Never runs the caller's callback: this is the JNI thread.
fn record_progress(request_id: u64, progress: PickProgress) {
    let mut waits = lock(&WAITS);
    if let Some(Slot {
        answer: None,
        progress: latest,
    }) = waits.get_mut(&request_id)
    {
        *latest = Some(progress);
        drop(waits);
        ANSWERED.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- JNI plumbing (the `plugins/permissions/src/android.rs` shape) --------

/// Run `f` with a live [`Env`] and the cached host class inside a scoped JNI
/// attachment. A missing platform handle is
/// [`PhotoError::PlatformNotInitialized`], checked before any JNI work.
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, PhotoError>,
) -> Result<T, PhotoError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(PhotoError::PlatformNotInitialized)
        }
        Err(other) => Err(PhotoError::Platform(format!(
            "android photo-library backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, PhotoError> {
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
) -> Result<Global<JClass<'static>>, PhotoError> {
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
        return Err(PhotoError::Platform(
            "frust-photo-library Gradle module is not linked — add the plugin from the frust TUI \
             (Add Plugin), or include the module by hand (see the frust-photo-library README)"
                .to_string(),
        ));
    }
    let class = class.and_then(|value| value.l()).map_err(|err| {
        PhotoError::Platform(format!(
            "android photo-library backend: ClassLoader.loadClass: {err}"
        ))
    })?;
    let class = run_jni(env, "casting FrustPhotoLibraryHost", |env| {
        env.cast_local::<JClass>(class)
    })?;
    run_jni(env, "pinning FrustPhotoLibraryHost", |env| {
        env.new_global_ref(class)
    })
}

/// Run a sequence of JNI calls, converting a pending Java exception into
/// [`PhotoError::Platform`] naming `op` and the exception's class only, never
/// its message. `jni` 0.22 leaves a thrown exception pending after the
/// failing call, so this always checks and clears it before returning.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, PhotoError> {
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
        return Err(PhotoError::Platform(format!(
            "android photo-library backend: {op}: {class_name}"
        )));
    }
    result
        .map_err(|err| PhotoError::Platform(format!("android photo-library backend: {op}: {err}")))
}

/// Refuse a call on the main `Looper` with [`PhotoError::UiThread`].
///
/// The test is Android's own: `Looper.myLooper() == Looper.getMainLooper()`.
/// A plain Rust worker thread has no `Looper`, so `myLooper()` is null there
/// and `IsSameObject(null, mainLooper)` is false. `android.os.Looper` is a
/// framework class, so the bare `find_class` works on any thread.
fn ensure_off_ui_thread(env: &mut Env<'_>) -> Result<(), PhotoError> {
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
        return Err(PhotoError::UiThread);
    }
    Ok(())
}

/// The elements of a `String[]` (null elements as `None`). A null array is
/// empty. Each element's local reference is released as it is read.
fn read_strings(
    env: &mut Env<'_>,
    array: &JObjectArray<'_>,
) -> Result<Vec<Option<String>>, jni::errors::Error> {
    if array.is_null() {
        return Ok(Vec::new());
    }
    let len = array.len(env)?;
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        let element = array.get_element(env, index)?;
        if element.is_null() {
            out.push(None);
            continue;
        }
        let text = env.cast_local::<JString>(element)?;
        out.push(Some(text.try_to_string(env)?));
        env.delete_local_ref(text);
    }
    Ok(out)
}

/// Zip the three parallel arrays of `nativeOnPickResult`. Mismatched lengths,
/// or a null path, fail the pick rather than misattribute a name or type.
fn host_copies(
    paths: Vec<Option<String>>,
    names: Vec<Option<String>>,
    mimes: Vec<Option<String>>,
) -> Option<Vec<HostCopy>> {
    if names.len() != paths.len() || mimes.len() != paths.len() {
        return None;
    }
    paths
        .into_iter()
        .zip(names)
        .zip(mimes)
        .map(|((path, name), mime)| {
            Some(HostCopy {
                path: path?,
                name,
                mime,
            })
        })
        .collect()
}

// --- Kotlin -> Rust JNI exports ------------------------------------------

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickResult` —
/// the host's copies for the [`pick`] that minted `request_id` (empty =
/// cancelled).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickResult<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    paths: JObjectArray<'local>,
    names: JObjectArray<'local>,
    mimes: JObjectArray<'local>,
) {
    env.with_env(|env| {
        // The inverse of `pick`'s own `as jlong`.
        let request_id = request_id as u64;
        let read = (|| {
            Ok::<_, jni::errors::Error>((
                read_strings(env, &paths)?,
                read_strings(env, &names)?,
                read_strings(env, &mimes)?,
            ))
        })();
        let answer = match read {
            Ok((paths, names, mimes)) => match host_copies(paths, names, mimes) {
                Some(copies) => Answer::Picked(copies),
                None => Answer::Failed(COPY_FAILED),
            },
            Err(err) => {
                log::warn!("frust-photo-library: reading the pick result failed: {err}");
                if env.exception_check() {
                    env.exception_clear();
                }
                Answer::Failed(COPY_FAILED)
            }
        };
        deliver(request_id, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickError` —
/// the host's failure code for the [`pick`] that minted `request_id`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickError<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    code: jint,
) {
    env.with_env(|_env| {
        // The inverse of `pick`'s own `as jlong`.
        deliver(request_id as u64, Answer::Failed(code));
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickProgress` —
/// copy progress for the [`pick`] that minted `request_id`. Stores it for the
/// waiting thread; never blocks on, or calls, the caller's callback.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickProgress<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    done: jint,
    items: jint,
    fraction: jdouble,
) {
    env.with_env(|_env| {
        let progress = PickProgress {
            items: usize::try_from(items).unwrap_or(0),
            done: usize::try_from(done).unwrap_or(0),
            fraction,
        };
        // The inverse of `pick`'s own `as jlong`.
        record_progress(request_id as u64, progress);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

// Library browsing: `albums`, `assets`, `thumbnail`, `manage_limited_selection`.
#[cfg(feature = "library")]
#[path = "android_library.rs"]
mod android_library;
#[cfg(feature = "library")]
pub(crate) use android_library::{albums, assets, manage_limited_selection, thumbnail};

// Originals and saving: `export_original` and `save`.
#[cfg(any(feature = "library", feature = "save"))]
#[path = "android_originals.rs"]
mod android_originals;
#[cfg(feature = "library")]
pub(crate) use android_originals::export_original;
#[cfg(feature = "save")]
pub(crate) use android_originals::save;

// Change feed: `current_change_token`, `changes_since` and the `ContentObserver` wake.
#[cfg(feature = "library")]
#[path = "android_changes.rs"]
mod android_changes;
#[cfg(feature = "library")]
pub(crate) use android_changes::{
    changes_since, current_change_token, start_change_observer, stop_change_observer,
};

// EXIF: the `DecodeResult` read both calls share, and `asset_metadata`'s host call.
#[cfg(any(feature = "decode", feature = "library"))]
#[path = "android_metadata.rs"]
mod android_metadata;

/// The crate root's `asset_metadata` arm. The crate root has already refused a cancelled token;
/// this refuses the UI thread before any platform work, then reads in `android_metadata`.
#[cfg(feature = "library")]
pub(crate) fn asset_metadata(
    id: &crate::AssetId,
    options: &crate::MetadataOptions,
) -> Result<crate::ExifData, PhotoError> {
    with_host(|env, _class| ensure_off_ui_thread(env))?;
    android_metadata::asset_metadata(id, options)
}

// Decode: `decode_file` through `ImageDecoder` into a direct `ByteBuffer`.
#[cfg(feature = "decode")]
#[path = "android_decode.rs"]
mod android_decode;

/// The crate root's `decode_file` arm. The shared precheck (a cancelled token, a zero cap) has
/// already run; this refuses the UI thread before any platform work, then decodes in
/// `android_decode`.
#[cfg(feature = "decode")]
pub(crate) fn decode_file(
    path: &Path,
    options: &crate::DecodeOptions,
) -> Result<crate::DecodedImage, PhotoError> {
    with_host(|env, _class| ensure_off_ui_thread(env))?;
    android_decode::decode(path, options)
}
