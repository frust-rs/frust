//! Android: the Storage Access Framework, driven from a
//! `dev.frust.filedialogs.FrustFileDialogsHost` Kotlin host
//! (`plugins/file-dialogs/platform/android/`) over this crate's own JNI
//! surface.
//!
//! # The frozen contract
//!
//! **`FrustFileDialogsHost.kt` and this module build to this table: changing it
//! means updating both files together.** The host's package is baked into the
//! JNI symbols below, so it may never move once shipped. The integer codes are
//! this module's `const`s, pinned against the Kotlin constants by
//! `tests/kotlin_contract.rs`.
//!
//! ## Rust to Kotlin (static methods, resolved through the application classloader)
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `openDocuments` | `(long requestId, String[] mimes, boolean multiple, String destDir) -> int` | [`PENDING`], [`NO_ACTIVITY`], [`BUSY`] or [`NOT_INITIALIZED`]; answered through `nativeOnPaths` or `nativeOnCode` |
//! | `exportFile` | `(long requestId, String sourcePath, String suggestedName, String mime) -> int` | as above; answered through `nativeOnCode` ([`SAVED`], [`DISMISSED`], ...) |
//! | `pickTree` | `(long requestId) -> int` | as above; answered through `nativeOnTree` or `nativeOnCode` |
//! | `listTree` | `(long requestId, String uri, boolean recursive, int maxDepth, int maxEntries, String[] extensions, boolean includeHidden) -> int` | [`PENDING`] or [`NOT_INITIALIZED`]; answered through `nativeOnListing` or `nativeOnCode` |
//! | `resolveEntry` | `(long requestId, String treeUri, String[] segments) -> int` | [`PENDING`] or [`NOT_INITIALIZED`]; answered through `nativeOnListing` with zero entries (not found) or one, or `nativeOnCode` |
//! | `copyDocument` | `(long requestId, String uri, String destPath, long maxBytes) -> int` | [`PENDING`] or [`NOT_INITIALIZED`]; answered through `nativeOnCode`; a negative `maxBytes` means no limit |
//! | `cancel` | `(long requestId) -> void` | nothing; an unknown or already-answered id is ignored |
//! | `isTreeGranted` | `(String uri) -> boolean` | whether a persisted read grant covers `uri` |
//! | `releaseTree` | `(String uri) -> void` | nothing; drops the persisted grant |
//! | `releaseNewTree` | `(long requestId, String uri) -> void` | nothing; drops the persisted grant only if the answer to `requestId` newly took it |
//! | `mimesForExtensions` | `(String[] extensions) -> String[]` | the MIME types to filter by (`*/*` when any extension is unknown) |
//! | `mimeForName` | `(String name) -> String` | the MIME type for a new document of that name |
//! | `cacheRoot` | `() -> String` | the app cache directory, or null before initialisation |
//!
//! ## Kotlin to Rust (this crate's own `#[unsafe(no_mangle)]` exports)
//!
//! - `nativeOnPaths(requestId: jlong, paths: String[])`: the app-private copies; an empty
//!   array means the user dismissed the dialog.
//! - `nativeOnTree(requestId: jlong, uri: String)`: the persisted tree URI; null means the
//!   user dismissed the dialog.
//! - `nativeOnListing(requestId: jlong, names: String[], relativePaths: String[], sizes: long[],
//!   modified: long[], uris: String[])`: parallel arrays, with [`NO_VALUE`] for an unknown size or time.
//! - `nativeOnCode(requestId: jlong, code: jint)`: every other outcome.
//! - `nativeOnProgress(requestId: jlong, done: jint, items: jint, fraction: jdouble)`: copy
//!   progress, from the host's copy thread; never an answer.
//!
//! Every export upgrades its [`jni::EnvUnowned`] through
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind`, and
//! resolves with [`LogErrorAndDefault`]. This module holds no `unsafe` block;
//! each export's `#[unsafe(no_mangle)]` attribute is its only `unsafe` token.
//!
//! # Waiting for the answer, keyed by request id
//!
//! [`run_request`] mints an id, arms a [`WAITS`] slot for it *before* the JNI
//! call (so an answer relayed while the call is returning cannot be missed),
//! and blocks on [`ANSWERED`] outside the JNI attachment. There is no deadline:
//! a person may keep a system dialog open as long as they like, and the host
//! answers every [`PENDING`] request exactly once. The first answer for an id
//! wins; anything later is dropped.
//!
//! # Never on the UI thread
//!
//! The dialogs are launched and answered on the main `Looper`, so a caller
//! parked there would wait on its own wake-up forever. Every blocking entry
//! calls [`ensure_off_ui_thread`] first and reports
//! [`FileDialogError::UiThread`]. The host keeps one dialog in flight and
//! answers [`BUSY`] for a second; `FileDialogError` has no busy variant, so
//! that surfaces as [`FileDialogError::Platform`].
//!
//! # Cancel and progress
//!
//! Once the host answered [`PENDING`], the token's hook calls the host's
//! `cancel` and then records a local cancel, so the wait ends at once without
//! depending on the host. A system dialog already showing cannot be closed
//! from the app: it stays up, the host frees its slot when the result arrives,
//! and that result is discarded. Copies, exports and listings stop between
//! chunks or entries and remove their partial output. A user dismissing a
//! dialog keeps its own result (`Ok(None)`, an empty `Vec` or
//! [`ExportOutcome::Cancelled`]); [`FileDialogError::Cancelled`] means the
//! token fired. The progress export only stores the latest value in the
//! request's slot and wakes the waiter, which forwards it through a
//! [`ProgressThrottle`] after releasing the lock.
//!
//! # Copies
//!
//! Opening a file returns an app-private copy under
//! `<cache>/frust-file-dialogs/<unique>/`, named after the provider's display
//! name by the host (which replaces path separators and de-duplicates within
//! the call). A host path outside that directory is refused. Any failure, a
//! cancel, or a dismissal removes the call's directory. The copies otherwise
//! stay until the OS clears the app cache, so move what must be kept.
//!
//! # Folder grants
//!
//! A [`FolderGrant`]'s token is the tree URI. [`restore_folder`] reports
//! whether the persisted permission is still held; a folder deleted out from
//! under a held grant surfaces as [`FileDialogError::AccessLost`] from the
//! first [`FolderGrant::list`](crate::FolderGrant::list). The system caps the
//! persisted grants per app and silently drops the oldest beyond the cap.
//! [`resolve_entry`] finds one file by its relative path without a full
//! listing: the host lists one folder per segment and matches display names
//! exactly, never building a document id from the path text.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::HashMap;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JLongArray, JObject, JObjectArray, JString, JValue};
use jni::refs::Global;
use jni::sys::{jdouble, jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::cancel::ProgressThrottle;
use crate::export::io_error;
use crate::{
    CallOptions, CancelToken, EntryLocation, ExportOutcome, FileDialogError, FileProgress,
    FolderEntry, FolderGrant, Kind, ListOptions, Request,
};

/// The Kotlin host's fully-qualified class name, in the dotted form
/// `ClassLoader.loadClass` expects. A bare `FindClass` on a JNI worker thread
/// sees only the bootstrap loader, never app classes.
const HOST_CLASS_BINARY: &str = "dev.frust.filedialogs.FrustFileDialogsHost";

/// The cache subdirectory holding every call's copies.
const CACHE_SUBDIR: &str = "frust-file-dialogs";

/// The call was queued; the answer arrives through an export.
const PENDING: jint = 0;
/// No resumed Activity to launch from, or it was finishing by launch time.
const NO_ACTIVITY: jint = 1;
/// The host already has a dialog in flight.
const BUSY: jint = 2;
/// The host has no application context (its init provider never ran).
const NOT_INITIALIZED: jint = 3;
/// The system dialog could not be launched.
const LAUNCH_FAILED: jint = 4;
/// A file operation failed.
const ERROR: jint = 5;
/// The host honoured a `cancel` for the request.
const CANCELLED: jint = 6;
/// An export or document copy finished.
const SAVED: jint = 7;
/// The user dismissed the export dialog.
const DISMISSED: jint = 8;
/// The folder or its persisted grant is gone.
const ACCESS_LOST: jint = 9;
/// A document exceeded the copy's byte limit.
const TOO_LARGE: jint = 10;
/// The system refused to persist the folder grant (for example the per-app cap).
const GRANT_FAILED: jint = 11;

/// `copyDocument`'s `maxBytes` when there is no limit.
const NO_LIMIT: jlong = -1;
/// `nativeOnListing`'s marker for an unknown size or time.
const NO_VALUE: jlong = -1;

/// The cached host class: one global reference for the process, loaded on
/// first use.
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Outstanding requests by id. The waiter removes its own slot.
static WAITS: LazyLock<Mutex<HashMap<u64, Slot>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Woken by the exports and the cancel hook; waited on by [`run_request`].
static ANSWERED: Condvar = Condvar::new();

/// The next request id. Starts at 1 so a zeroed id is never live.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Separates call directories created in the same instant.
static NEXT_CALL_DIR: AtomicU64 = AtomicU64::new(0);

/// One outstanding request: its answer once there is one, and the latest
/// progress the waiter has not forwarded yet.
#[derive(Default)]
struct Slot {
    answer: Option<Answer>,
    progress: Option<FileProgress>,
}

/// The answer to one request.
enum Answer {
    /// `nativeOnPaths`: the copies (empty when dismissed).
    Paths(Vec<String>),
    /// `nativeOnTree`: the persisted tree URI (`None` when dismissed).
    Tree(Option<String>),
    /// `nativeOnListing`: the listed files.
    Listing(Vec<HostEntry>),
    /// `nativeOnCode`: every other outcome.
    Code(jint),
    /// The request's [`CancelToken`] fired.
    Cancelled,
}

/// One file as the host listed it.
struct HostEntry {
    name: String,
    relative_path: String,
    size: jlong,
    modified: jlong,
    uri: String,
}

/// Android keeps nothing per grant: the persisted permission is the grant.
pub(crate) struct GrantState;

/// The crate root's dialog arm: opens one or many documents.
pub(crate) fn show(request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    // A save destination cannot be a filesystem path here: `export_file` is the route.
    if request.kind == Kind::SaveFile {
        return Err(FileDialogError::Unsupported);
    }
    refuse_ui_thread()?;
    note_ignored_hints(&request);
    let dir = new_call_dir()?;
    let result = open_into(&request, &dir);
    if !matches!(&result, Ok(paths) if !paths.is_empty()) {
        remove_call_dir(&dir);
    }
    result
}

/// [`show`] after the guards: one host call, the wait and the path checks.
fn open_into(request: &Request, dir: &Path) -> Result<Vec<PathBuf>, FileDialogError> {
    let dest = utf8(dir)?;
    let multiple = request.kind == Kind::OpenFiles;
    let extensions: Vec<&str> = request
        .filters
        .iter()
        .flat_map(|filter| filter.extensions().iter().map(String::as_str))
        .collect();
    let throttle = throttle_for(&request.options);
    let answer = run_request(
        request.options.cancel.as_ref(),
        throttle.as_ref(),
        |env, class, id| {
            let mimes = run_jni(env, "FrustFileDialogsHost.mimesForExtensions", |env| {
                let extensions = string_array(env, &extensions)?;
                env.call_static_method(
                    class,
                    jni_str!("mimesForExtensions"),
                    jni_sig!("([Ljava/lang/String;)[Ljava/lang/String;"),
                    &[JValue::Object(&extensions)],
                )?
                .l()
            })?;
            run_jni(env, "FrustFileDialogsHost.openDocuments", |env| {
                let dest = env.new_string(dest)?;
                env.call_static_method(
                    class,
                    jni_str!("openDocuments"),
                    jni_sig!("(J[Ljava/lang/String;ZLjava/lang/String;)I"),
                    &[
                        JValue::Long(id),
                        JValue::Object(&mimes),
                        JValue::Bool(multiple),
                        JValue::Object(&dest),
                    ],
                )?
                .i()
            })
        },
    )?;
    let Answer::Paths(paths) = answer else {
        return Err(failure(answer));
    };
    let mut copies = Vec::with_capacity(paths.len());
    for path in paths {
        let path = PathBuf::from(path);
        if path.parent() != Some(dir) {
            log::warn!("frust-file-dialogs: the host reported a copy outside the call directory");
            return Err(FileDialogError::Platform);
        }
        copies.push(path);
    }
    if !copies.is_empty() {
        report_finished(throttle.as_ref(), copies.len());
    }
    Ok(copies)
}

/// Picks a folder and takes a persisted read grant on it.
pub(crate) fn pick_folder(request: Request) -> Result<Option<FolderGrant>, FileDialogError> {
    refuse_ui_thread()?;
    note_ignored_hints(&request);
    let answer = run_request(request.options.cancel.as_ref(), None, |env, class, id| {
        run_jni(env, "FrustFileDialogsHost.pickTree", |env| {
            env.call_static_method(
                class,
                jni_str!("pickTree"),
                jni_sig!("(J)I"),
                &[JValue::Long(id)],
            )?
            .i()
        })
    })?;
    match answer {
        Answer::Tree(Some(uri)) => Ok(Some(grant_for(uri))),
        Answer::Tree(None) => Ok(None),
        other => Err(failure(other)),
    }
}

/// Re-acquires a folder from its tree URI when the persisted grant still holds.
pub(crate) fn restore_folder(token: &str) -> Result<FolderGrant, FileDialogError> {
    refuse_ui_thread()?;
    let granted = with_host(|env, class| {
        run_jni(env, "FrustFileDialogsHost.isTreeGranted", |env| {
            let uri = env.new_string(token)?;
            env.call_static_method(
                class,
                jni_str!("isTreeGranted"),
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&uri)],
            )?
            .z()
        })
    })?;
    if granted {
        Ok(grant_for(token.to_owned()))
    } else {
        Err(FileDialogError::AccessLost)
    }
}

/// Lists the files under the grant's tree, sorted by relative path.
pub(crate) fn list_folder(
    grant: &FolderGrant,
    options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    let token = options.cancel_token();
    if token.is_some_and(CancelToken::is_cancelled) {
        return Err(FileDialogError::Cancelled);
    }
    refuse_ui_thread()?;
    let max_depth = jint::try_from(options.max_depth).unwrap_or(jint::MAX);
    let max_entries = jint::try_from(options.max_entries).unwrap_or(jint::MAX);
    let extensions: Vec<&str> = options.extensions.iter().map(String::as_str).collect();
    let answer = run_request(token, None, |env, class, id| {
        run_jni(env, "FrustFileDialogsHost.listTree", |env| {
            let uri = env.new_string(&grant.token)?;
            let extensions = string_array(env, &extensions)?;
            env.call_static_method(
                class,
                jni_str!("listTree"),
                jni_sig!("(JLjava/lang/String;ZII[Ljava/lang/String;Z)I"),
                &[
                    JValue::Long(id),
                    JValue::Object(&uri),
                    JValue::Bool(options.recursive),
                    JValue::Int(max_depth),
                    JValue::Int(max_entries),
                    JValue::Object(&extensions),
                    JValue::Bool(options.include_hidden),
                ],
            )?
            .i()
        })
    })?;
    let Answer::Listing(entries) = answer else {
        return Err(failure(answer));
    };
    let mut entries: Vec<FolderEntry> = entries.into_iter().map(folder_entry).collect();
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(entries)
}

/// Looks up the file at `segments` under the grant's tree: the host lists one folder per
/// segment and answers a listing of zero entries (not found) or one.
pub(crate) fn resolve_entry(
    grant: &FolderGrant,
    segments: &[&str],
) -> Result<Option<FolderEntry>, FileDialogError> {
    refuse_ui_thread()?;
    let answer = run_request(None, None, |env, class, id| {
        run_jni(env, "FrustFileDialogsHost.resolveEntry", |env| {
            let uri = env.new_string(&grant.token)?;
            let segments = string_array(env, segments)?;
            env.call_static_method(
                class,
                jni_str!("resolveEntry"),
                jni_sig!("(JLjava/lang/String;[Ljava/lang/String;)I"),
                &[
                    JValue::Long(id),
                    JValue::Object(&uri),
                    JValue::Object(&segments),
                ],
            )?
            .i()
        })
    })?;
    let Answer::Listing(mut entries) = answer else {
        return Err(failure(answer));
    };
    let relative_path = segments.join("/");
    match entries.pop() {
        None => Ok(None),
        Some(entry) if entries.is_empty() && entry.relative_path == relative_path => {
            Ok(Some(folder_entry(entry)))
        }
        Some(_) => {
            log::warn!("frust-file-dialogs: the host answered a lookup with another entry");
            Err(FileDialogError::Platform)
        }
    }
}

/// Gives up the persisted grant. Never blocks on the user, so any thread may call it.
pub(crate) fn release_folder(_state: GrantState, token: &str) {
    release_tree(token);
}

/// Copies `source` to a document the user creates.
pub(crate) fn export_file(
    request: Request,
    source: &Path,
) -> Result<ExportOutcome, FileDialogError> {
    refuse_ui_thread()?;
    note_ignored_hints(&request);
    if !source.is_file() {
        log::warn!("frust-file-dialogs: the export source is not a readable file");
        return Err(FileDialogError::Io);
    }
    let source = utf8(source)?;
    let name = request.file_name.as_deref().unwrap_or("export");
    let throttle = throttle_for(&request.options);
    let answer = run_request(
        request.options.cancel.as_ref(),
        throttle.as_ref(),
        |env, class, id| {
            let mime = run_jni(env, "FrustFileDialogsHost.mimeForName", |env| {
                let name = env.new_string(name)?;
                env.call_static_method(
                    class,
                    jni_str!("mimeForName"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                    &[JValue::Object(&name)],
                )?
                .l()
            })?;
            run_jni(env, "FrustFileDialogsHost.exportFile", |env| {
                let source = env.new_string(source)?;
                let name = env.new_string(name)?;
                env.call_static_method(
                    class,
                    jni_str!("exportFile"),
                    jni_sig!("(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)I"),
                    &[
                        JValue::Long(id),
                        JValue::Object(&source),
                        JValue::Object(&name),
                        JValue::Object(&mime),
                    ],
                )?
                .i()
            })
        },
    )?;
    match answer {
        Answer::Code(SAVED) => {
            report_finished(throttle.as_ref(), 1);
            Ok(ExportOutcome::Saved)
        }
        Answer::Code(DISMISSED) => Ok(ExportOutcome::Cancelled),
        other => Err(failure(other)),
    }
}

/// Copies the document at `uri` over `destination`.
pub(crate) fn copy_content_uri(uri: &str, destination: &Path) -> Result<(), FileDialogError> {
    refuse_ui_thread()?;
    copy_document(uri, destination, NO_LIMIT, false)
}

/// Reads the document at `uri`, refusing one larger than `max_bytes`.
pub(crate) fn read_content_uri(uri: &str, max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
    refuse_ui_thread()?;
    let dir = new_call_dir()?;
    let file = dir.join("read");
    let limit = jlong::try_from(max_bytes).unwrap_or(jlong::MAX);
    let result = copy_document(uri, &file, limit, false)
        .and_then(|()| fs::read(&file).map_err(io_error))
        .map_err(|err| match err {
            FileDialogError::TooLarge { .. } => FileDialogError::TooLarge {
                limit: max_bytes,
                size: None,
            },
            other => other,
        });
    remove_call_dir(&dir);
    result
}

/// Reads the first `max_bytes` bytes of the document at `uri`; a longer one is cut short.
pub(crate) fn read_prefix_content_uri(
    uri: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, FileDialogError> {
    refuse_ui_thread()?;
    let dir = new_call_dir()?;
    let file = dir.join("read");
    let limit = jlong::try_from(max_bytes).unwrap_or(jlong::MAX);
    let result =
        copy_document(uri, &file, limit, true).and_then(|()| fs::read(&file).map_err(io_error));
    remove_call_dir(&dir);
    result
}

/// The system dialogs take neither a title nor a filesystem starting directory.
fn note_ignored_hints(request: &Request) {
    if request.title.is_some() || request.directory.is_some() {
        log::debug!("frust-file-dialogs: the system dialog ignores the title and directory");
    }
}

/// One `copyDocument` request (or `copyDocumentPrefix` when `truncate`) and its wait.
fn copy_document(
    uri: &str,
    destination: &Path,
    limit: jlong,
    truncate: bool,
) -> Result<(), FileDialogError> {
    let dest = utf8(destination)?;
    let answer = if truncate {
        run_request(None, None, |env, class, id| {
            run_jni(env, "FrustFileDialogsHost.copyDocumentPrefix", |env| {
                let uri = env.new_string(uri)?;
                let dest = env.new_string(dest)?;
                env.call_static_method(
                    class,
                    jni_str!("copyDocumentPrefix"),
                    jni_sig!("(JLjava/lang/String;Ljava/lang/String;J)I"),
                    &[
                        JValue::Long(id),
                        JValue::Object(&uri),
                        JValue::Object(&dest),
                        JValue::Long(limit),
                    ],
                )?
                .i()
            })
        })?
    } else {
        run_request(None, None, |env, class, id| {
            run_jni(env, "FrustFileDialogsHost.copyDocument", |env| {
                let uri = env.new_string(uri)?;
                let dest = env.new_string(dest)?;
                env.call_static_method(
                    class,
                    jni_str!("copyDocument"),
                    jni_sig!("(JLjava/lang/String;Ljava/lang/String;J)I"),
                    &[
                        JValue::Long(id),
                        JValue::Object(&uri),
                        JValue::Object(&dest),
                        JValue::Long(limit),
                    ],
                )?
                .i()
            })
        })?
    };
    match answer {
        Answer::Code(SAVED) => Ok(()),
        other => Err(failure(other)),
    }
}

// --- The request cycle ----------------------------------------------------

/// One host call and its wait: mints the id, arms the slot, starts the request
/// through `start`, then blocks until the host answers or the token fires.
fn run_request(
    cancel: Option<&CancelToken>,
    throttle: Option<&ProgressThrottle>,
    start: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>, jlong) -> Result<jint, FileDialogError>,
) -> Result<Answer, FileDialogError> {
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    lock(&WAITS).insert(request_id, Slot::default());

    // `u64` -> `jlong` reinterprets the same 64 bits; the host treats the id
    // as opaque and the exports cast it back.
    let code = match with_host(|env, class| start(env, class, request_id as jlong)) {
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
    if let Some(token) = cancel {
        token.set_hook(move || cancel_request(request_id));
    }
    let answer = await_answer(request_id, throttle);
    if let Some(token) = cancel {
        token.clear_hook();
    }
    // A cancel that raced the host's answer still wins, except over a finished
    // export: once the host reported `SAVED` the document exists, so that stays
    // the outcome (as on iOS).
    if cancel.is_some_and(CancelToken::is_cancelled) && !matches!(answer, Answer::Code(SAVED)) {
        if let Answer::Tree(Some(uri)) = &answer {
            // Only a permission this request newly took: a re-picked folder
            // keeps the grant an older `FolderGrant` holds.
            release_new_tree(request_id, uri);
        }
        return Ok(Answer::Cancelled);
    }
    Ok(answer)
}

/// The cancel hook for `request_id`: tell the host, then end the wait
/// locally. Runs on whichever thread called [`CancelToken::cancel`].
fn cancel_request(request_id: u64) {
    let sent = with_host(|env, class| {
        run_jni(env, "FrustFileDialogsHost.cancel", |env| {
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
        log::warn!("frust-file-dialogs: cancelling the request on the host failed: {err}");
    }
    deliver(request_id, Answer::Cancelled);
}

/// Block until `request_id` is answered, forwarding its progress to `throttle`
/// on the way, always outside the [`WAITS`] lock. No deadline (module doc).
fn await_answer(request_id: u64, throttle: Option<&ProgressThrottle>) -> Answer {
    loop {
        let (progress, answer) = next_event(request_id);
        let forward = !matches!(answer, Some(Answer::Cancelled | Answer::Code(_)));
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
fn next_event(request_id: u64) -> (Option<FileProgress>, Option<Answer>) {
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
        // Only the waiter removes its slot, so this cannot happen; fail rather than hang.
        return (None, Some(Answer::Code(ERROR)));
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
        _ => log::debug!("frust-file-dialogs: dropped a late answer for request {request_id}"),
    }
}

/// Store the latest progress for an unanswered `request_id` and wake the
/// waiter. Never runs the caller's callback: this is the JNI thread.
fn record_progress(request_id: u64, progress: FileProgress) {
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

// --- Results --------------------------------------------------------------

/// The error for an answer that is not the call's success shape.
fn failure(answer: Answer) -> FileDialogError {
    match answer {
        Answer::Code(code) => error_for(code),
        Answer::Cancelled => FileDialogError::Cancelled,
        Answer::Paths(_) | Answer::Tree(_) | Answer::Listing(_) => {
            log::warn!("frust-file-dialogs: the host answered with the wrong result kind");
            FileDialogError::Platform
        }
    }
}

/// The typed error for a non-[`PENDING`] return or a `nativeOnCode` code.
fn error_for(code: jint) -> FileDialogError {
    match code {
        NO_ACTIVITY => FileDialogError::NoPresenter,
        NOT_INITIALIZED => FileDialogError::PlatformNotInitialized,
        CANCELLED => FileDialogError::Cancelled,
        ACCESS_LOST => FileDialogError::AccessLost,
        ERROR => FileDialogError::Io,
        TOO_LARGE => {
            log::warn!("frust-file-dialogs: the document exceeds the read limit");
            // The caller fills in the limit and any size it knows.
            FileDialogError::TooLarge {
                limit: 0,
                size: None,
            }
        }
        GRANT_FAILED => {
            log::warn!("frust-file-dialogs: the system refused to persist the folder grant");
            FileDialogError::Platform
        }
        BUSY => {
            log::warn!("frust-file-dialogs: another file dialog is already in flight");
            FileDialogError::Platform
        }
        LAUNCH_FAILED => {
            log::warn!("frust-file-dialogs: the system dialog could not be launched");
            FileDialogError::Platform
        }
        other => {
            log::warn!("frust-file-dialogs: the host returned an unknown code {other}");
            FileDialogError::Platform
        }
    }
}

fn grant_for(token: String) -> FolderGrant {
    FolderGrant {
        token,
        local_path: None,
        state: GrantState,
    }
}

fn folder_entry(entry: HostEntry) -> FolderEntry {
    FolderEntry {
        name: entry.name,
        relative_path: entry.relative_path,
        size: u64::try_from(entry.size).ok(),
        modified_unix_ms: (entry.modified != NO_VALUE).then_some(entry.modified),
        location: EntryLocation::ContentUri(entry.uri),
    }
}

fn throttle_for(options: &CallOptions) -> Option<ProgressThrottle> {
    options.progress.clone().map(ProgressThrottle::new)
}

/// Delivers the final `{items, done: items, fraction: 1.0}`.
fn report_finished(throttle: Option<&ProgressThrottle>, items: usize) {
    if let Some(throttle) = throttle {
        throttle.report(FileProgress {
            items,
            done: items,
            fraction: 1.0,
        });
    }
}

fn utf8(path: &Path) -> Result<&str, FileDialogError> {
    path.to_str().ok_or_else(|| {
        log::warn!("frust-file-dialogs: a path is not valid UTF-8");
        FileDialogError::Io
    })
}

/// A fresh, empty directory under `<cache>/frust-file-dialogs`.
fn new_call_dir() -> Result<PathBuf, FileDialogError> {
    let cache = with_host(|env, class| {
        run_jni(env, "FrustFileDialogsHost.cacheRoot", |env| {
            let value = env
                .call_static_method(
                    class,
                    jni_str!("cacheRoot"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            if value.is_null() {
                return Ok(None);
            }
            let text = env.cast_local::<JString>(value)?;
            Ok(Some(text.try_to_string(env)?))
        })
    })?
    .ok_or(FileDialogError::PlatformNotInitialized)?;
    let root = Path::new(&cache).join(CACHE_SUBDIR);
    fs::create_dir_all(&root).map_err(io_error)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    // A directory left by an earlier process can hold the same name.
    loop {
        let unique = NEXT_CALL_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{stamp}-{unique}"));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error(err)),
        }
    }
}

/// Best-effort removal of a call directory that returns nothing.
fn remove_call_dir(dir: &Path) {
    if let Err(err) = fs::remove_dir_all(dir) {
        log::debug!(
            "frust-file-dialogs: could not remove a call directory: {:?}",
            err.kind()
        );
    }
}

// --- JNI plumbing ---------------------------------------------------------

/// Refuse a call on the main `Looper` with [`FileDialogError::UiThread`].
fn refuse_ui_thread() -> Result<(), FileDialogError> {
    with_host(|env, _class| ensure_off_ui_thread(env))
}

/// Run `f` with a live [`Env`] and the cached host class inside a scoped JNI
/// attachment.
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, FileDialogError>,
) -> Result<T, FileDialogError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(FileDialogError::PlatformNotInitialized)
        }
        Err(other) => {
            log::warn!("frust-file-dialogs: platform handle error: {other}");
            Err(FileDialogError::Platform)
        }
    }
}

/// The cached [`HOST_CLASS`], loading it on first use.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, FileDialogError> {
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
) -> Result<Global<JClass<'static>>, FileDialogError> {
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
    // failure (the plugin's Gradle module is not wired into the app).
    let class = env.call_method(
        &loader,
        jni_str!("loadClass"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
        &[JValue::Object(&name)],
    );
    if env.exception_check() {
        env.exception_clear();
        log::warn!(
            "frust-file-dialogs: the Gradle module is not linked; add the plugin from the frust \
             TUI (Add Plugin) or include the module by hand"
        );
        return Err(FileDialogError::PlatformNotInitialized);
    }
    let class = class.and_then(|value| value.l()).map_err(|err| {
        log::warn!("frust-file-dialogs: ClassLoader.loadClass: {err}");
        FileDialogError::Platform
    })?;
    let class = run_jni(env, "casting FrustFileDialogsHost", |env| {
        env.cast_local::<JClass>(class)
    })?;
    run_jni(env, "pinning FrustFileDialogsHost", |env| {
        env.new_global_ref(class)
    })
}

/// Run a sequence of JNI calls, converting a pending Java exception into
/// [`FileDialogError::Platform`] and logging `op` and the exception's class
/// only, never its message. `jni` 0.22 leaves a thrown exception pending after
/// the failing call, so this always checks and clears it before returning.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, FileDialogError> {
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
        log::warn!("frust-file-dialogs: {op}: {class_name}");
        return Err(FileDialogError::Platform);
    }
    result.map_err(|err| {
        log::warn!("frust-file-dialogs: {op}: {err}");
        FileDialogError::Platform
    })
}

/// Refuse a call on the main `Looper` with [`FileDialogError::UiThread`].
///
/// The test is Android's own: `Looper.myLooper() == Looper.getMainLooper()`. A
/// plain Rust worker thread has no `Looper`, so `myLooper()` is null there.
fn ensure_off_ui_thread(env: &mut Env<'_>) -> Result<(), FileDialogError> {
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
        return Err(FileDialogError::UiThread);
    }
    Ok(())
}

/// Drops the persisted grant on `uri`; a failure is only logged.
fn release_tree(uri: &str) {
    let released = with_host(|env, class| {
        run_jni(env, "FrustFileDialogsHost.releaseTree", |env| {
            let uri = env.new_string(uri)?;
            env.call_static_method(
                class,
                jni_str!("releaseTree"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&uri)],
            )?
            .v()
        })
    });
    if let Err(err) = released {
        log::warn!("frust-file-dialogs: releasing the folder grant failed: {err}");
    }
}

/// Drops the persisted grant on `uri` only if the host's answer to `request_id`
/// newly took it; a failure is only logged.
fn release_new_tree(request_id: u64, uri: &str) {
    let released = with_host(|env, class| {
        run_jni(env, "FrustFileDialogsHost.releaseNewTree", |env| {
            let uri = env.new_string(uri)?;
            env.call_static_method(
                class,
                jni_str!("releaseNewTree"),
                jni_sig!("(JLjava/lang/String;)V"),
                &[JValue::Long(request_id as jlong), JValue::Object(&uri)],
            )?
            .v()
        })
    });
    if let Err(err) = released {
        log::warn!("frust-file-dialogs: releasing the new folder grant failed: {err}");
    }
}

/// A `String[]` holding `values`.
fn string_array<'local>(
    env: &mut Env<'local>,
    values: &[&str],
) -> Result<JObject<'local>, jni::errors::Error> {
    let empty = JObject::null();
    let len = i32::try_from(values.len()).unwrap_or(i32::MAX);
    let array = env.new_object_array(len, jni_str!("java/lang/String"), &empty)?;
    for (index, value) in values.iter().enumerate() {
        let element = env.new_string(value)?;
        array.set_element(env, index, &element)?;
        env.delete_local_ref(element);
    }
    Ok(array.into())
}

/// The elements of a `String[]`; a null array is empty, and a null element
/// fails the read. Each element's local reference is released as it is read.
fn read_strings(
    env: &mut Env<'_>,
    array: &JObjectArray<'_>,
) -> Result<Vec<String>, jni::errors::Error> {
    if array.is_null() {
        return Ok(Vec::new());
    }
    let len = array.len(env)?;
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        let element = array.get_element(env, index)?;
        if element.is_null() {
            return Err(jni::errors::Error::NullPtr("a null array element"));
        }
        let text = env.cast_local::<JString>(element)?;
        out.push(text.try_to_string(env)?);
        env.delete_local_ref(text);
    }
    Ok(out)
}

/// The elements of a `long[]`; a null array is empty.
fn read_longs(env: &mut Env<'_>, array: &JLongArray<'_>) -> Result<Vec<jlong>, jni::errors::Error> {
    if array.is_null() {
        return Ok(Vec::new());
    }
    let mut values = vec![0; array.len(env)?];
    array.get_region(env, 0, &mut values)?;
    Ok(values)
}

/// Zips the listing's parallel arrays; `None` when their lengths differ.
fn host_entries(
    names: Vec<String>,
    relative_paths: Vec<String>,
    sizes: Vec<jlong>,
    modified: Vec<jlong>,
    uris: Vec<String>,
) -> Option<Vec<HostEntry>> {
    let len = names.len();
    if relative_paths.len() != len
        || sizes.len() != len
        || modified.len() != len
        || uris.len() != len
    {
        return None;
    }
    Some(
        names
            .into_iter()
            .zip(relative_paths)
            .zip(sizes)
            .zip(modified)
            .zip(uris)
            .map(
                |((((name, relative_path), size), modified), uri)| HostEntry {
                    name,
                    relative_path,
                    size,
                    modified,
                    uri,
                },
            )
            .collect(),
    )
}

/// The answer for a failed read of a callback's arrays.
fn unreadable(env: &mut Env<'_>, err: jni::errors::Error) -> Answer {
    log::warn!("frust-file-dialogs: reading a host result failed: {err}");
    if env.exception_check() {
        env.exception_clear();
    }
    Answer::Code(ERROR)
}

// --- Kotlin -> Rust JNI exports ------------------------------------------

/// `nativeOnPaths`: the copies for the request `request_id` (empty = dismissed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnPaths<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    paths: JObjectArray<'local>,
) {
    env.with_env(|env| {
        let answer = match read_strings(env, &paths) {
            Ok(paths) => Answer::Paths(paths),
            Err(err) => unreadable(env, err),
        };
        deliver(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `nativeOnTree`: the persisted tree URI for `request_id` (null = dismissed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnTree<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    uri: JString<'local>,
) {
    env.with_env(|env| {
        let answer = if uri.is_null() {
            Answer::Tree(None)
        } else {
            match uri.try_to_string(env) {
                Ok(uri) => Answer::Tree(Some(uri)),
                Err(err) => unreadable(env, err),
            }
        };
        deliver(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `nativeOnListing`: the files listed for `request_id`, as parallel arrays.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnListing<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    names: JObjectArray<'local>,
    relative_paths: JObjectArray<'local>,
    sizes: JLongArray<'local>,
    modified: JLongArray<'local>,
    uris: JObjectArray<'local>,
) {
    env.with_env(|env| {
        let read = (|| {
            Ok::<_, jni::errors::Error>((
                read_strings(env, &names)?,
                read_strings(env, &relative_paths)?,
                read_longs(env, &sizes)?,
                read_longs(env, &modified)?,
                read_strings(env, &uris)?,
            ))
        })();
        let answer = match read {
            Ok((names, relative_paths, sizes, modified, uris)) => {
                match host_entries(names, relative_paths, sizes, modified, uris) {
                    Some(entries) => Answer::Listing(entries),
                    None => {
                        log::warn!("frust-file-dialogs: the listing arrays differ in length");
                        Answer::Code(ERROR)
                    }
                }
            }
            Err(err) => unreadable(env, err),
        };
        deliver(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `nativeOnCode`: every other outcome of `request_id`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnCode<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    code: jint,
) {
    env.with_env(|_env| {
        deliver(request_id as u64, Answer::Code(code));
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `nativeOnProgress`: copy progress for `request_id`. Stores it for the
/// waiting thread; never blocks on, or calls, the caller's callback.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnProgress<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    done: jint,
    items: jint,
    fraction: jdouble,
) {
    env.with_env(|_env| {
        let progress = FileProgress {
            items: usize::try_from(items).unwrap_or(0),
            done: usize::try_from(done).unwrap_or(0),
            fraction,
        };
        record_progress(request_id as u64, progress);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
