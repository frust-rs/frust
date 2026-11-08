//! Android: exporting an asset's original file and saving a file into the library, the
//! `library` and `save` features' half of the Rust-Kotlin contract in [`super`]. Declared from
//! `android.rs`; every host call and export here is pinned by `tests/kotlin_contract.rs`.
//!
//! Export and save wait on this module's own request-id map ([`WAITS`]), never the pick map,
//! the pick in-flight slot or the pick guard: an export or a save runs while a pick does, and
//! exports queue on the host's own executor. Each call refuses the UI thread. The wait has no
//! deadline because the host answers every `PENDING` request exactly once; it runs outside any
//! JNI attachment and outside the lock, and forwards progress from the waiting thread.
//!
//! The host writes an export to `<dest>/.export-<id>` and reports the provider's display name
//! and MIME type. [`claim_final_path`] sanitises the name with the pick cache's policy, makes
//! it unique in the destination so an earlier export is never overwritten, and the staging
//! file is renamed onto it. Asset ids are `content://media/external/...` strings; the host
//! answers `NOT_FOUND` for anything else.
//!
//! A cancel reaches the host through `cancelExport`, which deletes the partial file and answers
//! `CANCELLED`, so a cancelled export never returns with a partial file left. If the JNI call
//! itself fails the wait ends locally. `AssetVersion` and `allow_network` are ignored here:
//! `MediaStore` has no rendition split and its files are on the device.

use std::collections::HashMap;
#[cfg(feature = "library")]
use std::collections::HashSet;
#[cfg(feature = "library")]
use std::fs::{self, File};
#[cfg(feature = "library")]
use std::io;
#[cfg(feature = "library")]
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Condvar, LazyLock, Mutex};

use jni::errors::LogErrorAndDefault;
#[cfg(feature = "save")]
use jni::objects::JObject;
use jni::objects::{JClass, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

#[cfg(feature = "library")]
use super::kind_for;
use super::{
    CANCELLED, NEXT_REQUEST_ID, NOT_INITIALIZED, PENDING, ensure_off_ui_thread, error_for, lock,
    run_jni, with_host,
};
use crate::cancel::ProgressThrottle;
#[cfg(feature = "library")]
use crate::media_types::classify_path;
use crate::originals;
#[cfg(feature = "library")]
use crate::originals::{ExportOptions, ExportedOriginal};
#[cfg(feature = "save")]
use crate::originals::{SaveRequest, SavedAsset};
#[cfg(feature = "library")]
use crate::pick_cache;
#[cfg(feature = "library")]
use crate::types::MediaFilter;
#[cfg(feature = "save")]
use crate::types::MediaKind;
use crate::types::{AssetId, PhotoError};

/// The call succeeded.
const OK: jint = 7;
/// No access to the asset.
const PERMISSION_DENIED: jint = 8;
/// The asset no longer exists, or the id is not a `MediaStore` content URI.
const NOT_FOUND: jint = 9;
/// An export could not be copied.
#[cfg(feature = "library")]
const EXPORT_FAILED: jint = 12;
/// A save could not be written.
#[cfg(feature = "save")]
const SAVE_FAILED: jint = 13;

/// File-name prefix of an export's staging file in the destination directory. Starts with a dot,
/// which the sanitiser never produces, so a finished export can never land on it.
#[cfg(feature = "library")]
const STAGING_PREFIX: &str = ".export-";

/// Mid-copy progress stays below this: `1.0` is reserved for the final delivery on success.
const MAX_COPY_FRACTION: f64 = 0.99;

/// Outstanding exports and saves by id. The waiter removes its own slot.
static WAITS: LazyLock<Mutex<HashMap<u64, Slot>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Woken by the exports' answers and progress; waited on by [`await_answer`].
static ANSWERED: Condvar = Condvar::new();

/// One outstanding export or save: its answer once there is one, and the latest bytes-copied
/// report the waiter has not forwarded yet.
#[derive(Default)]
struct Slot {
    answer: Option<Answer>,
    progress: Option<(jlong, jlong)>,
}

/// A host answer: its code, and on success the export's MIME type and display name, or the saved
/// asset's URI in `text`.
struct Answer {
    code: jint,
    text: Option<String>,
    #[cfg_attr(not(feature = "library"), allow(dead_code))]
    name: Option<String>,
}

/// The crate root's `export_original` arm. Blocks until the copy finishes; see the module doc.
#[cfg(feature = "library")]
pub(crate) fn export_original(
    id: &AssetId,
    options: &ExportOptions,
) -> Result<ExportedOriginal, PhotoError> {
    if originals::is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    with_host(|env, _class| ensure_off_ui_thread(env))?;

    let dir = match &options.dest_dir {
        Some(dir) => dir.clone(),
        None => originals::default_dest_dir()?,
    };
    fs::create_dir_all(&dir)?;
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let staging = dir.join(format!("{STAGING_PREFIX}{request_id}"));
    let result = run_export(id, options, request_id, &staging);
    if let Some(token) = &options.cancel {
        token.clear_hook();
    }
    if result.is_err() {
        remove_file(&staging);
    }
    result
}

/// [`export_original`] after the guards: one host call, the wait, and the final file.
#[cfg(feature = "library")]
fn run_export(
    id: &AssetId,
    options: &ExportOptions,
    request_id: u64,
    staging: &Path,
) -> Result<ExportedOriginal, PhotoError> {
    let dest = staging.to_str().ok_or_else(not_utf8)?;
    lock(&WAITS).insert(request_id, Slot::default());
    let code = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.exportOriginal", |env| {
            let uri = env.new_string(id.as_str())?;
            let dest = env.new_string(dest)?;
            env.call_static_method(
                class,
                jni_str!("exportOriginal"),
                jni_sig!("(JLjava/lang/String;Ljava/lang/String;)I"),
                // `u64` -> `jlong` reinterprets the same 64 bits; the exports cast it back.
                &[
                    JValue::Long(request_id as jlong),
                    JValue::Object(&uri),
                    JValue::Object(&dest),
                ],
            )?
            .i()
        })
    });
    ensure_pending(request_id, code)?;

    // Only after `PENDING`: a host `cancelExport` then always finds the request.
    if let Some(token) = &options.cancel {
        token.set_hook(move || cancel_export(request_id));
    }
    let throttle = options.progress.clone().map(originals::throttled);
    if let Some(throttle) = &throttle {
        originals::report_fraction(throttle, 0.0);
    }
    let answer = await_answer(request_id, throttle.as_ref());
    // A cancel that raced the host's answer still wins.
    if originals::is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    if answer.code != OK {
        return Err(host_error(answer.code));
    }
    let exported = finish_export(id, staging, answer)?;
    if let Some(throttle) = &throttle {
        originals::report_fraction(throttle, 1.0);
    }
    Ok(exported)
}

/// Renames the staging file onto its final name and describes it.
#[cfg(feature = "library")]
fn finish_export(
    id: &AssetId,
    staging: &Path,
    answer: Answer,
) -> Result<ExportedOriginal, PhotoError> {
    let mime = answer.text.filter(|m| !m.is_empty());
    let file_name = answer.name.filter(|n| !n.is_empty());
    let dir = staging.parent().ok_or_else(not_utf8)?;
    let path = claim_final_path(dir, file_name.as_deref(), mime.as_deref())?;
    if let Err(err) = fs::rename(staging, &path) {
        remove_file(&path);
        return Err(err.into());
    }
    let byte_len = fs::metadata(&path)?.len();
    let filter = if id.as_str().contains("/video/") {
        MediaFilter::Videos
    } else {
        MediaFilter::Images
    };
    let kind = kind_for(mime.as_deref(), &path, filter);
    let mime = mime.or_else(|| classify_path(&path).map(|(_, m)| m.to_owned()));
    Ok(ExportedOriginal {
        path,
        file_name,
        mime,
        kind,
        byte_len,
    })
}

/// Creates an empty file under the first free name for `display` in `dir`: created new, so two
/// exports can never claim the same name and an existing file is never replaced.
#[cfg(feature = "library")]
fn claim_final_path(
    dir: &Path,
    display: Option<&str>,
    mime: Option<&str>,
) -> Result<PathBuf, PhotoError> {
    let mut used = HashSet::new();
    loop {
        let name = pick_cache::copy_name(display, mime, 0, &mut used);
        let path = dir.join(name);
        match File::options().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

/// The cancel hook for `request_id`: tell the host, which deletes the partial file and answers.
/// Runs on whichever thread called `CancelToken::cancel`.
#[cfg(feature = "library")]
fn cancel_export(request_id: u64) {
    let sent = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.cancelExport", |env| {
            env.call_static_method(
                class,
                jni_str!("cancelExport"),
                jni_sig!("(J)V"),
                &[JValue::Long(request_id as jlong)],
            )?
            .v()
        })
    });
    if let Err(err) = sent {
        log::warn!("frust-photo-library: cancelling the export on the host failed: {err}");
        deliver(
            request_id,
            Answer {
                code: CANCELLED,
                text: None,
                name: None,
            },
        );
    }
}

/// The crate root's `save` arm. Blocks until the save finishes.
#[cfg(feature = "save")]
pub(crate) fn save(request: &SaveRequest, kind: MediaKind) -> Result<SavedAsset, PhotoError> {
    with_host(|env, _class| ensure_off_ui_thread(env))?;
    let source = request.source.to_str().ok_or_else(not_utf8)?;
    let display_name = match &request.file_name {
        Some(name) => name.as_str(),
        None => request
            .source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(not_utf8)?,
    };
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    lock(&WAITS).insert(request_id, Slot::default());
    let code = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.saveToLibrary", |env| {
            let source = env.new_string(source)?;
            let name = env.new_string(display_name)?;
            let album = match &request.album {
                Some(album) => JObject::from(env.new_string(album)?),
                None => JObject::null(),
            };
            env.call_static_method(
                class,
                jni_str!("saveToLibrary"),
                jni_sig!("(JLjava/lang/String;ZLjava/lang/String;Ljava/lang/String;)I"),
                &[
                    JValue::Long(request_id as jlong),
                    JValue::Object(&source),
                    JValue::Bool(kind == MediaKind::Video),
                    JValue::Object(&name),
                    JValue::Object(&album),
                ],
            )?
            .i()
        })
    });
    ensure_pending(request_id, code)?;

    let answer = await_answer(request_id, None);
    if answer.code != OK {
        return Err(host_error(answer.code));
    }
    Ok(SavedAsset {
        id: answer
            .text
            .filter(|uri| !uri.is_empty())
            .map(AssetId::from_raw),
    })
}

/// Checks the host's synchronous answer; anything but `PENDING` removes the slot and fails.
fn ensure_pending(request_id: u64, code: Result<jint, PhotoError>) -> Result<(), PhotoError> {
    let code = code.inspect_err(|_| {
        lock(&WAITS).remove(&request_id);
    })?;
    if code == PENDING {
        return Ok(());
    }
    lock(&WAITS).remove(&request_id);
    Err(host_error(code))
}

/// The typed error for a non-`OK` code of this module's calls.
fn host_error(code: jint) -> PhotoError {
    match code {
        NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
        PERMISSION_DENIED => PhotoError::PermissionDenied,
        NOT_FOUND => PhotoError::NotFound,
        CANCELLED => PhotoError::Cancelled,
        #[cfg(feature = "library")]
        EXPORT_FAILED => PhotoError::Platform("the original could not be exported".to_string()),
        #[cfg(feature = "save")]
        SAVE_FAILED => {
            PhotoError::Platform("the file could not be saved to the library".to_string())
        }
        other => error_for(other),
    }
}

fn not_utf8() -> PhotoError {
    PhotoError::Platform("the file path is not valid UTF-8".to_string())
}

#[cfg(feature = "library")]
fn remove_file(path: &Path) {
    if let Err(err) = fs::remove_file(path)
        && err.kind() != io::ErrorKind::NotFound
    {
        log::debug!(
            "frust-photo-library: could not remove an export file: {:?}",
            err.kind()
        );
    }
}

/// Block until `request_id` is answered, forwarding its progress to `throttle` outside the
/// lock. No deadline (module doc).
fn await_answer(request_id: u64, throttle: Option<&ProgressThrottle>) -> Answer {
    loop {
        let (progress, answer) = next_event(request_id);
        let keep = answer.as_ref().is_none_or(|a| a.code == OK);
        if let (Some(progress), Some(throttle), true) = (progress, throttle, keep) {
            forward_progress(throttle, progress);
        }
        if let Some(answer) = answer {
            return answer;
        }
    }
}

/// Wait for news on `request_id`: its pending progress and its answer once there is one (the
/// slot is then removed). The lock is released on return.
fn next_event(request_id: u64) -> (Option<(jlong, jlong)>, Option<Answer>) {
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
        // Only the waiter removes its slot, so this means no answer will ever come.
        return (
            None,
            Some(Answer {
                code: CANCELLED,
                text: None,
                name: None,
            }),
        );
    };
    let progress = slot.progress.take();
    if slot.answer.is_none() {
        return (progress, None);
    }
    let answer = waits.remove(&request_id).and_then(|slot| slot.answer);
    (progress, answer)
}

/// Maps the host's byte counts to a fraction. An unknown total reports nothing: only the `0.0`
/// and the final `1.0` then reach the caller.
fn forward_progress(throttle: &ProgressThrottle, (done, total): (jlong, jlong)) {
    if total > 0 {
        let fraction = (done.max(0) as f64 / total as f64).min(MAX_COPY_FRACTION);
        originals::report_fraction(throttle, fraction);
    }
}

/// Record the answer for `request_id` and wake the waiters. The first answer wins; an id with
/// no slot, or one already answered, is dropped.
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

/// Store the latest byte counts for an unanswered `request_id` and wake the waiter. Never runs
/// the caller's callback: this is the JNI thread.
#[cfg(feature = "library")]
fn record_progress(request_id: u64, progress: (jlong, jlong)) {
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

/// A JNI string that may be null. Only reads text, and leaves a read failure for the caller.
fn read_string(
    env: &mut Env<'_>,
    text: &JString<'_>,
) -> Result<Option<String>, jni::errors::Error> {
    if text.is_null() {
        return Ok(None);
    }
    text.try_to_string(env).map(Some)
}

// --- Kotlin -> Rust JNI exports ------------------------------------------

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportProgress` — bytes copied for
/// the export that minted `request_id`; `total` is the size the provider reports. Stores it for
/// the waiting thread and never runs the caller's callback.
#[cfg(feature = "library")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportProgress<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    done: jlong,
    total: jlong,
) {
    env.with_env(|_env| {
        record_progress(request_id as u64, (done, total));
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportDone` — the answer for the
/// export that minted `request_id`; `mime` and `name` are its provider's reports, each nullable.
#[cfg(feature = "library")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnExportDone<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    code: jint,
    mime: JString<'local>,
    name: JString<'local>,
) {
    env.with_env(|env| {
        let read =
            (|| Ok::<_, jni::errors::Error>((read_string(env, &mime)?, read_string(env, &name)?)))(
            );
        let answer = match read {
            Ok((text, name)) => Answer { code, text, name },
            Err(err) => {
                log::warn!("frust-photo-library: reading the export answer failed: {err}");
                if env.exception_check() {
                    env.exception_clear();
                }
                Answer {
                    code: EXPORT_FAILED,
                    text: None,
                    name: None,
                }
            }
        };
        deliver(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnSaveDone` — the answer for the
/// save that minted `request_id`; `uri` is the new asset's id, null unless the code is `OK`.
#[cfg(feature = "save")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnSaveDone<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    code: jint,
    uri: JString<'local>,
) {
    env.with_env(|env| {
        let answer = match read_string(env, &uri) {
            Ok(text) => Answer {
                code,
                text,
                name: None,
            },
            Err(err) => {
                log::warn!("frust-photo-library: reading the save answer failed: {err}");
                if env.exception_check() {
                    env.exception_clear();
                }
                Answer {
                    code: SAVE_FAILED,
                    text: None,
                    name: None,
                }
            }
        };
        deliver(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
