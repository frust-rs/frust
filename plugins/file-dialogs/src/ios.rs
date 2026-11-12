//! iOS backend: `UIDocumentPickerViewController` for opening, exporting and folder picks, and
//! `std::fs` over a security-scoped path for listing.
//!
//! # Flow
//!
//! Every call blocks a background thread, so the main thread is refused with
//! [`FileDialogError::UiThread`]. The picker is built and presented on the main queue, from the
//! top view controller of the foreground window (there is no anchor, so no popover source). The
//! request's title is set on the controller, but the system picker draws its own header.
//!
//! The delegate answers the waiting call once, through [`Reply`]:
//! `documentPicker:didPickDocumentsAtURLs:`, `documentPickerWasCancelled:`,
//! `presentationControllerDidDismiss:` (a swipe), a refused presentation, a failed presentation
//! and the cancel hook all take the one sender, so the first wins. The picker dismisses itself
//! after a pick or Cancel. Delegates run on the main queue and write no signals.
//!
//! A presentation is confirmed by its completion block. UIKit refuses one without calling back
//! when the presenter is off-screen or mid-transition, so if [`PRESENT_GRACE`] passes first the
//! call fails with [`FileDialogError::NoPresenter`], and a picker attached after that is
//! dismissed at once. The picker runs in-process, unlike `PHPicker`, so 10 s is generous; it is
//! kept equal to the photo picker's value.
//!
//! # Cancel and progress
//!
//! The token's hook runs on the cancelling thread and enqueues the main-queue step that answers
//! [`FileDialogError::Cancelled`] and dismisses a picker that is still presented. The wait also
//! checks the token every [`POLL`], so a blocked main queue cannot hold it. A cancel during a
//! presentation is finished by the completion block, because UIKit ignores a dismissal then.
//!
//! Progress starts once the picker closes: first `done: 0`, and a final call on success only.
//!
//! # Copies
//!
//! `asCopy` delivers each pick into the app's `tmp` Inbox, where it stays until removed. Each is
//! moved to `Caches/frust-file-dialogs/<unique>/`, by rename when possible and otherwise by
//! chunked copy on the waiting thread, checking the token between chunks. A cancelled or failed
//! call removes its directory and the delivered files. Only files under the app's `tmp` are ever
//! moved or deleted; anything else is copied and left in place.

mod ios_bookmark;

use std::cell::Cell;
use std::ffi::{OsStr, c_void};
use std::fs;
use std::io::ErrorKind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask, NSString, NSURL,
};
use objc2_ui_kit::{
    UIAdaptivePresentationControllerDelegate, UIApplication, UIDocumentPickerDelegate,
    UIDocumentPickerViewController, UIPresentationController, UISceneActivationState,
    UIViewController, UIWindow, UIWindowScene,
};
use objc2_uniform_type_identifiers::UTType;

pub(crate) use ios_bookmark::GrantState;

use crate::cancel::ProgressThrottle;
use crate::export::{copy_file_with, io_error};
use crate::{
    CallOptions, CancelToken, ExportOutcome, FileDialogError, FileFilter, FileProgress,
    FolderEntry, FolderGrant, Kind, ListOptions, Request,
};

/// How long UIKit may take to attach the picker before the call gives up.
const PRESENT_GRACE: Duration = Duration::from_secs(10);

/// How often the wait re-checks the cancel token.
const POLL: Duration = Duration::from_millis(100);

/// The picker's directory under the app's `Caches`.
const CACHE_DIR_NAME: &str = "frust-file-dialogs";

/// Attempts at a unique per-call directory name.
const MAX_CALL_DIR_ATTEMPTS: u32 = 1024;

type PickedUrls = Vec<Retained<NSURL>>;

/// What the main queue tells the waiting call.
enum Event {
    Picked(PickedUrls),
    Dismissed,
    Failed(FileDialogError),
}

/// The waiting call's sender, handed out once. A reply dropped unanswered reads as a dismissal.
struct Reply(Cell<Option<Sender<Event>>>);

impl Reply {
    fn new(tx: Sender<Event>) -> Self {
        Self(Cell::new(Some(tx)))
    }

    /// Sends `event` unless something answered first.
    fn answer(&self, event: Event) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(event);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.answer(Event::Dismissed);
    }
}

/// Runs a callback UIKit or a dispatch queue invoked: a panic must not unwind into its frame.
fn guarded(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        log::error!("frust-file-dialogs: panic while handling {what}");
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct DelegateIvars {
    reply: Reply,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The class implements `Drop` only through its ivars, so the generated `dealloc` has
    //   nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // `UIDocumentPickerDelegate` is a main-actor protocol, and the delegate is built on the
    // main queue.
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct FrustFileDialogDelegate;

    unsafe impl NSObjectProtocol for FrustFileDialogDelegate {}

    // SAFETY: both methods are the protocol's optional
    // `documentPicker:didPickDocumentsAtURLs:` and `documentPickerWasCancelled:`, with the
    // argument and return types of `UIDocumentPickerViewController.h`.
    unsafe impl UIDocumentPickerDelegate for FrustFileDialogDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _controller: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            guarded("the picked documents", || {
                self.ivars().reply.answer(Event::Picked(urls.to_vec()));
            });
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn was_cancelled(&self, _controller: &UIDocumentPickerViewController) {
            guarded("the picker's Cancel", || {
                self.ivars().reply.answer(Event::Dismissed);
            });
        }
    }

    // SAFETY: the one method is the protocol's optional
    // `presentationControllerDidDismiss:` (`UIPresentationController.h`), with the header's
    // argument and return types. UIKit calls it on the main thread.
    unsafe impl UIAdaptivePresentationControllerDelegate for FrustFileDialogDelegate {
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_controller_did_dismiss(&self, _controller: &UIPresentationController) {
            guarded("the picker's dismissal", || {
                self.ivars().reply.answer(Event::Dismissed);
            });
        }
    }
);

impl FrustFileDialogDelegate {
    fn new(mtm: MainThreadMarker, tx: Sender<Event>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            reply: Reply::new(tx),
        });
        // SAFETY: `NSObject`'s designated initializer, on a freshly allocated instance whose
        // ivars are already set.
        unsafe { msg_send![super(this), init] }
    }
}

/// The associated-object key tying a delegate to its picker; only its address matters.
static DELEGATE_KEY: u8 = 0;

/// Makes `picker` own `delegate`: the picker's `delegate` property is weak.
fn tie_lifetime(picker: &UIDocumentPickerViewController, delegate: &FrustFileDialogDelegate) {
    let object: *const UIDocumentPickerViewController = picker;
    let value: *const FrustFileDialogDelegate = delegate;
    // SAFETY: both pointers come from live references; the key is the address of a private
    // `static`, unique to this association; the policy is
    // `OBJC_ASSOCIATION_RETAIN_NONATOMIC`, so the runtime retains the delegate and releases it
    // when the picker deallocates. Only the main thread touches either object.
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            object.cast::<AnyObject>().cast_mut(),
            (&raw const DELEGATE_KEY).cast::<c_void>(),
            value.cast::<AnyObject>().cast_mut(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

/// Which picker a call needs.
enum PickerMode {
    Open {
        multiple: bool,
        filters: Vec<FileFilter>,
    },
    Folder,
    Export(Retained<NSURL>),
}

struct PickerSpec {
    mode: PickerMode,
    title: Option<String>,
    directory: Option<PathBuf>,
}

/// The `UTType` class, found at run time. No `UTType*` framework constant is referenced, so
/// an app that embeds this crate through another plugin has no such symbol to resolve. UIKit
/// loads the framework; were it absent, every picker call would report `Platform`.
fn ut_type_class() -> Result<&'static AnyClass, FileDialogError> {
    AnyClass::get(c"UTType").ok_or_else(|| {
        log::warn!("frust-file-dialogs: the UTType class is not available");
        FileDialogError::Platform
    })
}

fn type_with_identifier(identifier: &str) -> Result<Retained<UTType>, FileDialogError> {
    let class = ut_type_class()?;
    let identifier = NSString::from_str(identifier);
    // SAFETY: `+[UTType typeWithIdentifier:]` takes an `NSString` and returns a nullable
    // autoreleased `UTType`; `class` is the runtime `UTType` class.
    let found: Option<Retained<UTType>> =
        unsafe { msg_send![class, typeWithIdentifier: &*identifier] };
    found.ok_or_else(|| {
        log::warn!("frust-file-dialogs: the system does not know a content type");
        FileDialogError::Platform
    })
}

fn type_with_extension(extension: &str) -> Option<Retained<UTType>> {
    let class = ut_type_class().ok()?;
    let extension = NSString::from_str(extension);
    // SAFETY: `+[UTType typeWithFilenameExtension:]` takes an `NSString` and returns a nullable
    // autoreleased `UTType`; `class` is the runtime `UTType` class.
    unsafe { msg_send![class, typeWithFilenameExtension: &*extension] }
}

fn item_type() -> Result<Retained<UTType>, FileDialogError> {
    type_with_identifier("public.item")
}

fn folder_type() -> Result<Retained<UTType>, FileDialogError> {
    type_with_identifier("public.folder")
}

/// The picker's content types: one per filter extension, or any item when there are none.
fn content_types(filters: &[FileFilter]) -> Result<Retained<NSArray<UTType>>, FileDialogError> {
    let mut types: Vec<Retained<UTType>> = Vec::new();
    for extension in filters.iter().flat_map(FileFilter::extensions) {
        types.push(match type_with_extension(extension) {
            Some(found) => found,
            None => item_type()?,
        });
    }
    if types.is_empty() {
        types.push(item_type()?);
    }
    Ok(NSArray::from_retained_slice(&types))
}

/// A configured picker and its delegate.
struct Picker {
    controller: Retained<UIDocumentPickerViewController>,
    delegate: Retained<FrustFileDialogDelegate>,
}

impl Picker {
    fn new(
        mtm: MainThreadMarker,
        spec: PickerSpec,
        tx: Sender<Event>,
    ) -> Result<Self, FileDialogError> {
        let alloc = UIDocumentPickerViewController::alloc(mtm);
        let controller = match &spec.mode {
            PickerMode::Open { filters, .. } => {
                UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
                    alloc,
                    &*content_types(filters)?,
                    true,
                )
            }
            PickerMode::Folder => {
                UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
                    alloc,
                    &NSArray::from_retained_slice(&[folder_type()?]),
                    false,
                )
            }
            PickerMode::Export(url) => UIDocumentPickerViewController::initForExportingURLs_asCopy(
                alloc,
                &NSArray::from_retained_slice(std::slice::from_ref(url)),
                true,
            ),
        };
        if let PickerMode::Open { multiple, .. } = spec.mode {
            controller.setAllowsMultipleSelection(multiple);
        }
        if let Some(title) = &spec.title {
            controller.setTitle(Some(&NSString::from_str(title)));
        }
        if let Some(url) = spec
            .directory
            .as_deref()
            .filter(|directory| directory.is_absolute())
            .and_then(NSURL::from_directory_path)
        {
            controller.setDirectoryURL(Some(&url));
        }
        let delegate = FrustFileDialogDelegate::new(mtm, tx);
        controller.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        tie_lifetime(&controller, &delegate);
        // Before presenting, this creates the sheet's presentation controller.
        if let Some(presentation) = controller.presentationController() {
            // SAFETY: `delegate` is a weak property (`UIPresentationController.h`).
            // `tie_lifetime` keeps the delegate alive as long as the picker, and a presentation
            // controller that outlives both reads a zeroed weak reference. Both objects are
            // touched on the main thread only.
            unsafe { presentation.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        }
        Ok(Self {
            controller,
            delegate,
        })
    }

    fn answer(&self, event: Event) {
        self.delegate.ivars().reply.answer(event);
    }

    fn downgrade(&self) -> WeakPicker {
        WeakPicker {
            controller: Weak::from_retained(&self.controller),
            delegate: Weak::from_retained(&self.delegate),
        }
    }
}

/// Dismisses `controller` when it is presented and not already leaving.
fn dismiss_if_presented(controller: &UIDocumentPickerViewController, animated: bool) {
    if controller.presentingViewController().is_some() && !controller.isBeingDismissed() {
        controller.dismissViewControllerAnimated_completion(animated, None);
    }
}

/// A picker that a cancel can still reach while it lives.
struct WeakPicker {
    controller: Weak<UIDocumentPickerViewController>,
    delegate: Weak<FrustFileDialogDelegate>,
}

impl WeakPicker {
    /// Answers `Cancelled`, so a dismissed picker cannot also report, then dismisses it.
    fn cancel(&self) {
        if let Some(delegate) = self.delegate.load() {
            delegate
                .ivars()
                .reply
                .answer(Event::Failed(FileDialogError::Cancelled));
        }
        if let Some(controller) = self.controller.load() {
            dismiss_if_presented(&controller, true);
        }
    }
}

/// What the cancel hook shares with the main queue.
#[derive(Default)]
struct Shared {
    /// The presented picker. Set and taken on the main queue only.
    picker: Mutex<Option<MainThreadBound<WeakPicker>>>,
}

/// The token hook: runs once, on whichever thread cancels.
fn on_cancel(shared: &Arc<Shared>) {
    let shared = Arc::clone(shared);
    DispatchQueue::main().exec_async(move || {
        // SAFETY: the block is submitted to the main queue, which always runs on the process's
        // main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        guarded("the cancel", || {
            if let Some(picker) = lock(&shared.picker).take() {
                picker.get(mtm).cancel();
            }
        });
    });
}

/// One blocking call: registers the cancel hook and clears it when the call ends.
struct Call<'a> {
    options: &'a CallOptions,
    shared: Arc<Shared>,
}

impl<'a> Call<'a> {
    fn new(options: &'a CallOptions) -> Self {
        let shared = Arc::new(Shared::default());
        if let Some(token) = &options.cancel {
            let hooked = Arc::clone(&shared);
            token.set_hook(move || on_cancel(&hooked));
        }
        Self { options, shared }
    }

    /// Presents a picker and waits for its answer: `None` when the user dismissed it.
    fn present(&self, spec: PickerSpec) -> Result<Option<PickedUrls>, FileDialogError> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        let token = self.options.cancel.clone();
        DispatchQueue::main().exec_async(move || {
            // SAFETY: the block is submitted to the main queue, which always runs on the
            // process's main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            // A panic must not unwind out of a dispatch block.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                present_on_main(mtm, spec, &tx, token.as_ref(), &shared)
            }))
            .unwrap_or_else(|_| {
                log::error!("frust-file-dialogs: panic while presenting the picker");
                Err(FileDialogError::Platform)
            });
            if let Err(error) = outcome {
                let _ = tx.send(Event::Failed(error));
            }
        });
        let answer = wait(&rx, self.options);
        // Queued after the presentation, so the handle is released on the main thread once
        // nothing needs it. A wait that saw the token fire can get here ahead of the hook's own
        // main-queue step, so a still-presented picker is cancelled from here too.
        let release = Arc::clone(&self.shared);
        let token = self.options.cancel.clone();
        DispatchQueue::main().exec_async(move || {
            // SAFETY: the block is submitted to the main queue, which always runs on the
            // process's main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            guarded("releasing the picker", || {
                if let Some(picker) = lock(&release.picker).take()
                    && token.as_ref().is_some_and(CancelToken::is_cancelled)
                {
                    picker.get(mtm).cancel();
                }
            });
        });
        answer
    }
}

impl Drop for Call<'_> {
    fn drop(&mut self) {
        if let Some(token) = &self.options.cancel {
            token.clear_hook();
        }
    }
}

/// Waits for the one answer. A fired token ends the wait even when the main queue is blocked, and
/// overrides a dismissal that raced it.
fn wait(
    rx: &Receiver<Event>,
    options: &CallOptions,
) -> Result<Option<PickedUrls>, FileDialogError> {
    loop {
        match rx.recv_timeout(POLL) {
            Ok(Event::Picked(urls)) if !urls.is_empty() => return Ok(Some(urls)),
            Ok(Event::Picked(_) | Event::Dismissed) => {
                options.check()?;
                return Ok(None);
            }
            Ok(Event::Failed(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => options.check()?,
            Err(RecvTimeoutError::Disconnected) => {
                log::warn!("frust-file-dialogs: the picker went away without answering");
                return Err(FileDialogError::Platform);
            }
        }
    }
}

/// Where one picker's presentation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Requested; UIKit has not attached it yet.
    Pending,
    /// The presentation's completion block ran.
    Presented,
    /// [`PRESENT_GRACE`] passed first and the call failed.
    GaveUp,
}

/// Builds the picker and presents it. Runs on the main queue.
fn present_on_main(
    mtm: MainThreadMarker,
    spec: PickerSpec,
    tx: &Sender<Event>,
    token: Option<&CancelToken>,
    shared: &Arc<Shared>,
) -> Result<(), FileDialogError> {
    if token.is_some_and(CancelToken::is_cancelled) {
        return Err(FileDialogError::Cancelled);
    }
    let presenter = presenter(mtm).ok_or(FileDialogError::NoPresenter)?;
    let picker = Picker::new(mtm, spec, tx.clone())?;
    let shown = Rc::new(Cell::new(Shown::Pending));
    let on_shown = {
        let shown = Rc::clone(&shown);
        let token = token.cloned();
        let controller = Weak::from_retained(&picker.controller);
        RcBlock::new(move || {
            guarded("the picker's presentation", || {
                let gave_up = shown.get() == Shown::GaveUp;
                if !gave_up {
                    shown.set(Shown::Presented);
                }
                if (gave_up || token.as_ref().is_some_and(CancelToken::is_cancelled))
                    && let Some(controller) = controller.load()
                {
                    // Attached after the call ended, or a cancel UIKit ignored mid-transition.
                    dismiss_if_presented(&controller, false);
                }
            });
        })
    };
    presenter.presentViewController_animated_completion(&picker.controller, true, Some(&on_shown));
    *lock(&shared.picker) = Some(MainThreadBound::new(picker.downgrade(), mtm));
    let watch = MainThreadBound::new((picker, shown), mtm);
    let when = DispatchTime::try_from(PRESENT_GRACE).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, move || {
        // SAFETY: the block is submitted to the main queue, which always runs on the process's
        // main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        guarded("the presentation watch", || {
            let (picker, shown) = watch.get(mtm);
            if shown.get() == Shown::Pending {
                shown.set(Shown::GaveUp);
                picker.answer(Event::Failed(FileDialogError::NoPresenter));
            }
        });
    });
    Ok(())
}

/// The top view controller of the anchor window, followed up the `presentedViewController` chain
/// past anything being dismissed.
fn presenter(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let mut top = anchor_window(mtm)?.rootViewController()?;
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    Some(top)
}

/// The key window of a foreground-active scene, falling back to any window of one, then to any
/// window scene's window.
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();
    let mut fallback = None;
    for scene in scenes.iter() {
        let Some(window_scene) = scene.downcast_ref::<UIWindowScene>() else {
            continue;
        };
        let Some(window) = window_scene
            .keyWindow()
            .or_else(|| window_scene.windows().firstObject())
        else {
            continue;
        };
        if scene.activationState() == UISceneActivationState::ForegroundActive {
            return Some(window);
        }
        fallback = fallback.or(Some(window));
    }
    fallback
}

/// The refusal [`FolderEntry`] reads share with the backend calls.
pub(crate) fn refuse_ui_thread() -> Result<(), FileDialogError> {
    refuse_main_thread()
}

fn refuse_main_thread() -> Result<(), FileDialogError> {
    if MainThreadMarker::new().is_some() {
        return Err(FileDialogError::UiThread);
    }
    Ok(())
}

/// Reports one call's progress through the caller's throttled callback.
struct Progress {
    throttle: Option<ProgressThrottle>,
    items: usize,
}

impl Progress {
    fn new(options: &CallOptions, items: usize) -> Self {
        Self {
            throttle: options.progress.clone().map(ProgressThrottle::new),
            items,
        }
    }

    fn report(&self, done: usize, fraction: f64) {
        if let Some(throttle) = &self.throttle {
            throttle.report(FileProgress {
                items: self.items,
                done,
                fraction,
            });
        }
    }
}

/// `<Caches>/frust-file-dialogs`.
fn cache_root() -> Result<PathBuf, FileDialogError> {
    NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(
            NSSearchPathDirectory::CachesDirectory,
            NSSearchPathDomainMask::UserDomainMask,
        )
        .firstObject()
        .and_then(|url| url.to_file_path())
        .map(|caches| caches.join(CACHE_DIR_NAME))
        .ok_or_else(|| {
            log::warn!("frust-file-dialogs: the app has no Caches directory");
            FileDialogError::Platform
        })
}

/// A fresh directory under `root`, for one call's files.
fn new_call_dir(root: &Path) -> Result<PathBuf, FileDialogError> {
    fs::create_dir_all(root).map_err(io_error)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    for attempt in 0..MAX_CALL_DIR_ATTEMPTS {
        let dir = root.join(format!("{stamp:x}-{}-{attempt}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error(err)),
        }
    }
    log::warn!("frust-file-dialogs: no free directory name left in the cache");
    Err(FileDialogError::Io)
}

/// The app's temporary directory, canonicalised for [`is_under`].
fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir();
    dir.canonicalize().unwrap_or(dir)
}

fn is_under(path: &Path, root: &Path) -> bool {
    path.canonicalize().is_ok_and(|path| path.starts_with(root))
}

/// The file name a pick is kept under.
fn picked_name(source: &Path, index: usize) -> String {
    source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("file-{index}"))
}

/// A path in `dir` that nothing occupies. `dir` has one writer, so checking first is enough.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let plain = dir.join(name);
    if !plain.exists() {
        return plain;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    (1u32..)
        .map(|n| match extension {
            Some(extension) => dir.join(format!("{stem}-{n}.{extension}")),
            None => dir.join(format!("{stem}-{n}")),
        })
        .find(|path| !path.exists())
        .unwrap_or(plain)
}

/// Brings one delivered file to `destination`. A rename needs no chunks; a file not under
/// `temp_root` is the user's own and is only ever read.
fn take_file(
    source: &Path,
    destination: &Path,
    temp_root: &Path,
    cancel: Option<&CancelToken>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), FileDialogError> {
    let ours = is_under(source, temp_root);
    if ours && fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    copy_file_with(source, destination, cancel, progress)?;
    if ours {
        let _ = fs::remove_file(source);
    }
    Ok(())
}

fn move_all(
    sources: &[PathBuf],
    dir: &Path,
    temp_root: &Path,
    options: &CallOptions,
    progress: &Progress,
) -> Result<Vec<PathBuf>, FileDialogError> {
    let items = sources.len();
    let mut delivered = Vec::with_capacity(items);
    for (index, source) in sources.iter().enumerate() {
        options.check()?;
        let destination = free_path(dir, &picked_name(source, index));
        take_file(
            source,
            &destination,
            temp_root,
            options.cancel.as_ref(),
            &mut |copied, total| {
                let part = if total == 0 {
                    1.0
                } else {
                    copied as f64 / total as f64
                };
                progress.report(index, (index as f64 + part) / items as f64);
            },
        )?;
        delivered.push(destination);
        if index + 1 < items {
            progress.report(index + 1, (index + 1) as f64 / items as f64);
        }
    }
    options.check()?;
    Ok(delivered)
}

/// Moves the delivered files into a fresh directory under `root` and returns their new paths,
/// reporting progress. A cancelled or failed call leaves neither directory nor delivered files.
fn deliver(
    sources: &[PathBuf],
    root: &Path,
    temp_root: &Path,
    options: &CallOptions,
) -> Result<Vec<PathBuf>, FileDialogError> {
    let progress = Progress::new(options, sources.len());
    let result = options.check().and_then(|()| {
        progress.report(0, 0.0);
        let dir = new_call_dir(root)?;
        move_all(sources, &dir, temp_root, options, &progress).inspect_err(|_| {
            let _ = fs::remove_dir_all(&dir);
        })
    });
    match &result {
        Ok(_) => progress.report(sources.len(), 1.0),
        Err(_) => {
            for source in sources.iter().filter(|s| is_under(s, temp_root)) {
                let _ = fs::remove_file(source);
            }
        }
    }
    result
}

/// The file an export hands to the picker, which names the destination after it.
struct ExportSource {
    path: PathBuf,
    /// The staging directory to remove, when `path` is a renamed stand-in.
    staging: Option<PathBuf>,
}

impl ExportSource {
    /// `source` itself, or a stand-in under `root` named `name` when that differs. `name` comes
    /// from the caller and must be a bare file name; anything else keeps `source`'s own.
    fn prepare(
        source: &Path,
        name: Option<&str>,
        root: impl FnOnce() -> Result<PathBuf, FileDialogError>,
        options: &CallOptions,
    ) -> Result<Self, FileDialogError> {
        let absolute = std::path::absolute(source).map_err(io_error)?;
        let wanted = name.filter(|name| {
            Path::new(name).file_name() == Some(OsStr::new(name))
                && source.file_name() != Some(OsStr::new(name))
        });
        let Some(wanted) = wanted else {
            return Ok(Self {
                path: absolute,
                staging: None,
            });
        };
        let dir = new_call_dir(&root()?)?;
        let this = Self {
            path: dir.join(wanted),
            staging: Some(dir),
        };
        if fs::hard_link(&absolute, &this.path).is_err() {
            copy_file_with(
                &absolute,
                &this.path,
                options.cancel.as_ref(),
                &mut |_, _| {},
            )?;
        }
        Ok(this)
    }
}

impl Drop for ExportSource {
    fn drop(&mut self) {
        if let Some(dir) = &self.staging {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

pub(crate) fn show(request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    let multiple = match request.kind {
        Kind::OpenFile => false,
        Kind::OpenFiles => true,
        // `save_file` is documented as unsupported here: `export_file` is the iOS route.
        Kind::SaveFile | Kind::PickFolder => return Err(FileDialogError::Unsupported),
    };
    refuse_main_thread()?;
    let options = request.options.clone();
    let call = Call::new(&options);
    let picked = call.present(PickerSpec {
        mode: PickerMode::Open {
            multiple,
            filters: request.filters,
        },
        title: request.title,
        directory: request.directory,
    })?;
    let Some(urls) = picked else {
        return Ok(Vec::new());
    };
    // The picker's copies carry no scope; this only guards a provider that hands back one.
    let _scopes: Vec<_> = urls
        .iter()
        .map(ios_bookmark::SecurityScope::start)
        .collect();
    let sources = urls
        .iter()
        .map(|url| url.to_file_path())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            log::warn!("frust-file-dialogs: the picker returned a non-file URL");
            FileDialogError::Platform
        })?;
    deliver(&sources, &cache_root()?, &temp_root(), &options)
}

pub(crate) fn pick_folder(request: Request) -> Result<Option<FolderGrant>, FileDialogError> {
    refuse_main_thread()?;
    let options = request.options.clone();
    let call = Call::new(&options);
    let picked = call.present(PickerSpec {
        mode: PickerMode::Folder,
        title: request.title,
        directory: request.directory,
    })?;
    let Some(url) = picked.and_then(|urls| urls.into_iter().next()) else {
        return Ok(None);
    };
    options.check()?;
    let progress = Progress::new(&options, 1);
    progress.report(0, 0.0);
    let grant = ios_bookmark::grant_from_pick(&url)?;
    // A cancel here drops the grant, which leaves the scope.
    options.check()?;
    progress.report(1, 1.0);
    Ok(Some(grant))
}

pub(crate) fn restore_folder(token: &str) -> Result<FolderGrant, FileDialogError> {
    refuse_main_thread()?;
    ios_bookmark::restore(token)
}

pub(crate) fn list_folder(
    grant: &FolderGrant,
    options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    refuse_main_thread()?;
    let root = grant.local_path().ok_or(FileDialogError::AccessLost)?;
    crate::walk::list(root, options)
}

/// Like [`list_folder`]: the grant holds the folder's security scope for as long as it
/// lives, so the lookup runs inside it.
pub(crate) fn resolve_entry(
    grant: &FolderGrant,
    segments: &[&str],
) -> Result<Option<FolderEntry>, FileDialogError> {
    refuse_main_thread()?;
    let root = grant.local_path().ok_or(FileDialogError::AccessLost)?;
    crate::walk::entry(root, segments)
}

pub(crate) fn release_folder(state: GrantState, _token: &str) {
    drop(state);
}

pub(crate) fn export_file(
    request: Request,
    source: &Path,
) -> Result<ExportOutcome, FileDialogError> {
    refuse_main_thread()?;
    if !source.is_file() {
        log::warn!("frust-file-dialogs: the export source is not a readable file");
        return Err(FileDialogError::Io);
    }
    let options = request.options.clone();
    let export = ExportSource::prepare(source, request.file_name.as_deref(), cache_root, &options)?;
    let url = NSURL::from_file_path(&export.path).ok_or(FileDialogError::Io)?;
    let call = Call::new(&options);
    let picked = call.present(PickerSpec {
        mode: PickerMode::Export(url),
        title: request.title,
        directory: request.directory,
    })?;
    if picked.is_none() {
        return Ok(ExportOutcome::Cancelled);
    }
    // The system wrote the file before answering, so a token firing now cannot undo it: the
    // answer stays `Saved`, which is what happened.
    let progress = Progress::new(&options, 1);
    progress.report(0, 0.0);
    progress.report(1, 1.0);
    Ok(ExportOutcome::Saved)
}

pub(crate) fn copy_content_uri(_uri: &str, _destination: &Path) -> Result<(), FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn read_content_uri(_uri: &str, _max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn read_prefix_content_uri(
    _uri: &str,
    _max_bytes: u64,
) -> Result<Vec<u8>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn the_first_answer_wins_and_a_dropped_reply_reads_as_a_dismissal() {
        let (tx, rx) = mpsc::channel();
        let reply = Reply::new(tx);
        reply.answer(Event::Failed(FileDialogError::Cancelled));
        reply.answer(Event::Dismissed);
        drop(reply);
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Event::Failed(FileDialogError::Cancelled)
        ));

        let (tx, rx) = mpsc::channel();
        drop(Reply::new(tx));
        assert!(matches!(rx.try_recv(), Ok(Event::Dismissed)));
    }

    fn options_with(token: &CancelToken) -> CallOptions {
        CallOptions {
            progress: None,
            cancel: Some(token.clone()),
        }
    }

    #[test]
    fn wait_ends_on_a_fired_token_without_an_answer() {
        let (_tx, rx) = mpsc::channel::<Event>();
        let token = CancelToken::new();
        token.cancel();
        assert!(matches!(
            wait(&rx, &options_with(&token)),
            Err(FileDialogError::Cancelled)
        ));
    }

    #[test]
    fn a_dismissal_is_none_unless_the_token_fired() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Dismissed).unwrap();
        assert!(matches!(wait(&rx, &CallOptions::default()), Ok(None)));

        let (tx, rx) = mpsc::channel();
        tx.send(Event::Dismissed).unwrap();
        let token = CancelToken::new();
        token.cancel();
        assert!(matches!(
            wait(&rx, &options_with(&token)),
            Err(FileDialogError::Cancelled)
        ));
    }

    #[test]
    fn a_disconnected_channel_never_hangs() {
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx);
        assert!(matches!(
            wait(&rx, &CallOptions::default()),
            Err(FileDialogError::Platform)
        ));
    }

    /// `(inbox, cache root, canonical temp root)`, the inbox under the temp root.
    fn layout(dir: &TestDir) -> (PathBuf, PathBuf, PathBuf) {
        let inbox = dir.path().join("tmp/Inbox");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let temp = dir.path().join("tmp").canonicalize().unwrap();
        (inbox, cache, temp)
    }

    fn write(path: &Path, bytes: &[u8]) -> PathBuf {
        fs::write(path, bytes).unwrap();
        path.to_path_buf()
    }

    type Log = Arc<Mutex<Vec<FileProgress>>>;

    fn recording(log: &Log) -> crate::ProgressCallback {
        let log = Arc::clone(log);
        Arc::new(move |p| log.lock().unwrap().push(p))
    }

    #[test]
    fn delivered_copies_move_into_one_fresh_directory() {
        let dir = TestDir::new();
        let (inbox, cache, temp) = layout(&dir);
        let a = write(&inbox.join("a.txt"), b"one");
        let b = write(&inbox.join("b.txt"), b"two");
        let log: Log = Arc::default();
        let options = CallOptions {
            progress: Some(recording(&log)),
            cancel: None,
        };
        let out = deliver(&[a.clone(), b.clone()], &cache, &temp, &options).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].parent(), out[1].parent());
        assert!(out[0].starts_with(&cache));
        assert_eq!(fs::read(&out[1]).unwrap(), b"two");
        assert!(!a.exists() && !b.exists());
        let seen = log.lock().unwrap();
        assert_eq!((seen[0].items, seen[0].done, seen[0].fraction), (2, 0, 0.0));
        let last = seen.last().unwrap();
        assert_eq!((last.items, last.done, last.fraction), (2, 2, 1.0));
    }

    #[test]
    fn a_file_outside_the_temp_directory_is_copied_and_kept() {
        let dir = TestDir::new();
        let (_, cache, temp) = layout(&dir);
        let original = write(&dir.path().join("original.txt"), b"mine");
        let out = deliver(
            std::slice::from_ref(&original),
            &cache,
            &temp,
            &CallOptions::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&out[0]).unwrap(), b"mine");
        assert_eq!(fs::read(&original).unwrap(), b"mine");
    }

    #[test]
    fn equal_names_are_numbered() {
        let dir = TestDir::new();
        let (inbox, cache, temp) = layout(&dir);
        fs::create_dir_all(inbox.join("x")).unwrap();
        let first = write(&inbox.join("a.txt"), b"1");
        let second = write(&inbox.join("x/a.txt"), b"2");
        let out = deliver(&[first, second], &cache, &temp, &CallOptions::default()).unwrap();
        assert_eq!(out[0].file_name().unwrap(), "a.txt");
        assert_eq!(out[1].file_name().unwrap(), "a-1.txt");
    }

    #[test]
    fn a_token_fired_as_progress_starts_leaves_nothing_behind() {
        let dir = TestDir::new();
        let (inbox, cache, temp) = layout(&dir);
        let original = write(&dir.path().join("original.txt"), b"mine");
        let delivered = write(&inbox.join("waiting.txt"), b"x");
        let token = CancelToken::new();
        let firing = token.clone();
        let log: Log = Arc::default();
        let sink = recording(&log);
        let options = CallOptions {
            progress: Some(Arc::new(move |p| {
                firing.cancel();
                sink(p);
            })),
            cancel: Some(token),
        };
        let result = deliver(
            &[original.clone(), delivered.clone()],
            &cache,
            &temp,
            &options,
        );
        assert!(matches!(result, Err(FileDialogError::Cancelled)));
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        assert!(!delivered.exists());
        assert!(original.exists());
        assert!(log.lock().unwrap().iter().all(|p| p.done < p.items));
    }

    #[test]
    fn a_cancel_during_a_copy_leaves_no_partial_file() {
        let dir = TestDir::new();
        let (_, cache, temp) = layout(&dir);
        let source = write(&dir.path().join("big.bin"), &vec![1u8; 1024 * 1024]);
        let destination = cache.join("big.bin");
        let token = CancelToken::new();
        let result = take_file(&source, &destination, &temp, Some(&token), &mut |_, _| {
            token.cancel();
        });
        assert_eq!(result, Err(FileDialogError::Cancelled));
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        assert!(source.exists());
    }

    #[test]
    fn an_already_fired_token_reports_nothing() {
        let dir = TestDir::new();
        let (inbox, cache, temp) = layout(&dir);
        let delivered = write(&inbox.join("a.txt"), b"x");
        let token = CancelToken::new();
        token.cancel();
        let log: Log = Arc::default();
        let options = CallOptions {
            progress: Some(recording(&log)),
            cancel: Some(token),
        };
        assert!(matches!(
            deliver(std::slice::from_ref(&delivered), &cache, &temp, &options),
            Err(FileDialogError::Cancelled)
        ));
        assert!(log.lock().unwrap().is_empty());
        assert!(!delivered.exists());
    }

    fn cache_of(dir: &TestDir) -> impl FnOnce() -> Result<PathBuf, FileDialogError> {
        let cache = dir.path().join("cache");
        move || Ok(cache)
    }

    #[test]
    fn an_export_under_its_own_name_needs_no_stand_in() {
        let dir = TestDir::new();
        let source = dir.write("report.pdf", b"pdf");
        for name in [Some("report.pdf"), None] {
            let export =
                ExportSource::prepare(&source, name, cache_of(&dir), &CallOptions::default())
                    .unwrap();
            assert_eq!(export.path, std::path::absolute(&source).unwrap());
            assert!(export.staging.is_none());
        }
    }

    #[test]
    fn an_unsafe_suggested_name_is_ignored() {
        let dir = TestDir::new();
        let source = dir.write("report.pdf", b"pdf");
        for name in ["../evil.pdf", "a/b.pdf", "..", ""] {
            let export =
                ExportSource::prepare(&source, Some(name), cache_of(&dir), &CallOptions::default())
                    .unwrap();
            assert!(export.staging.is_none(), "{name:?}");
        }
    }

    #[test]
    fn a_renamed_export_uses_a_stand_in_removed_with_it() {
        let dir = TestDir::new();
        let source = dir.write("report.pdf", b"pdf");
        let export = ExportSource::prepare(
            &source,
            Some("copy.pdf"),
            cache_of(&dir),
            &CallOptions::default(),
        )
        .unwrap();
        assert_eq!(export.path.file_name().unwrap(), "copy.pdf");
        assert_eq!(fs::read(&export.path).unwrap(), b"pdf");
        let staging = export.staging.clone().unwrap();
        drop(export);
        assert!(!staging.exists());
        assert_eq!(fs::read(&source).unwrap(), b"pdf");
    }

    #[test]
    fn filters_become_content_types_and_no_filter_means_any_item() {
        let filters = [FileFilter::new("Images", &["png", "jpg"])];
        assert_eq!(content_types(&filters).unwrap().count(), 2);
        let any = content_types(&[]).unwrap();
        assert_eq!(any.count(), 1);
        assert_eq!(*any.firstObject().unwrap(), *item_type().unwrap());
    }

    #[test]
    fn a_missing_export_source_is_io() {
        let dir = TestDir::new();
        let request = Request {
            kind: Kind::SaveFile,
            title: None,
            directory: None,
            file_name: None,
            filters: Vec::new(),
            options: CallOptions::default(),
        };
        assert_eq!(
            export_file(request, &dir.path().join("missing.txt")),
            Err(FileDialogError::Io)
        );
    }

    #[test]
    fn save_file_is_unsupported() {
        let request = Request {
            kind: Kind::SaveFile,
            title: None,
            directory: None,
            file_name: None,
            filters: Vec::new(),
            options: CallOptions::default(),
        };
        assert_eq!(show(request), Err(FileDialogError::Unsupported));
    }
}
