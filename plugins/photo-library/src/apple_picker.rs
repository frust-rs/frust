//! The PHPicker pieces of the iOS arm: the hand-declared view controller, its
//! delegate, and the eager copy of each picked file.
//!
//! # Answers
//!
//! The delegate answers the waiting call once, through [`Reply`]:
//! `picker:didFinishPicking:` (the user chose or tapped Cancel),
//! `presentationControllerDidDismiss:` (the user swiped the sheet down, which
//! does not call the picker delegate), a refused presentation and a cancel all
//! take the one sender, so the first wins.
//!
//! # Bindings
//!
//! `objc2-photos-ui` binds `PHPickerConfiguration`, `PHPickerFilter`,
//! `PHPickerResult` and the delegate protocol, but declares
//! `PHPickerViewController` (and the protocol's one method) for macOS only.
//! The iOS controller is therefore declared here over `UIViewController`, with
//! just the two methods this arm sends, and the delegate method is registered
//! by selector.
//!
//! # Eager copies
//!
//! `loadFileRepresentationForTypeIdentifier:completionHandler:` writes a
//! temporary file that the system deletes when the completion handler returns
//! (`NSItemProvider.h`). The handler here therefore copies the file
//! synchronously, inside the handler, before it returns; nothing reads the
//! temporary path afterwards.
//!
//! # Type choice
//!
//! For each result the first registered type identifier that conforms to
//! `public.movie` (a video) or `public.image` (an image), within the call's
//! filter, is loaded; package types (directories) are skipped. Conformance is
//! asked of `UTType`, looked up at run time so the crate links nothing extra;
//! without it the abstract identifier is loaded and the item provider picks
//! the conforming representation.

use std::cell::Cell;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class};
use objc2::{extern_methods, msg_send};
use objc2_foundation::{NSArray, NSError, NSInteger, NSItemProvider, NSProgress, NSString, NSURL};
use objc2_photos_ui::{
    PHPickerConfiguration, PHPickerConfigurationAssetRepresentationMode, PHPickerFilter,
    PHPickerResult, PHPickerViewControllerDelegate,
};
use objc2_ui_kit::{
    UIAdaptivePresentationControllerDelegate, UIPresentationController, UIResponder,
    UIViewController,
};

use super::{Event, Shared};
use crate::media_types;
use crate::pick_cache::sanitize_file_name;
use crate::{MediaFilter, MediaKind, PickOptions, PickedItem};

const IMAGE_TYPE: &str = "public.image";
const MOVIE_TYPE: &str = "public.movie";
const DIRECTORY_TYPE: &str = "public.directory";
/// A load handler's outcome after the call was cancelled.
const CANCELLED: &str = "the pick was cancelled";

extern_class!(
    /// `PHPickerViewController` as the iOS SDK declares it.
    // SAFETY: `PHPicker.h` declares `@interface PHPickerViewController :
    // UIViewController` when building for iPhone OS, and `UIViewController`'s
    // own chain is `UIResponder`, `NSObject`. Like every view controller it is
    // main-thread-only.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub(super) struct PHPickerViewController;
);

impl PHPickerViewController {
    extern_methods!(
        // SAFETY: the signature matches `PHPicker.h`'s designated initializer
        // `- (instancetype)initWithConfiguration:(PHPickerConfiguration *)`,
        // which takes a non-null configuration and returns a non-null picker.
        #[unsafe(method(initWithConfiguration:))]
        #[unsafe(method_family = init)]
        fn init_with_configuration(
            this: Allocated<Self>,
            configuration: &PHPickerConfiguration,
        ) -> Retained<Self>;

        // SAFETY: the signature matches `PHPicker.h`'s `delegate` property
        // (`weak id<PHPickerViewControllerDelegate>`, nullable). The property
        // is weak: [`Picker::new`] keeps the delegate alive by tying it to the
        // picker.
        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        fn set_delegate(
            &self,
            delegate: Option<&ProtocolObject<dyn PHPickerViewControllerDelegate>>,
        );
    );
}

/// The waiting call's sender, handed out once (module doc's *Answers*).
///
/// A reply dropped unanswered (its picker released without any callback)
/// sends `Finished(0)`, a dismissal, so the waiting call never depends on the
/// channel disconnecting: the cancel hook holds a sender of its own.
struct Reply(Cell<Option<Sender<Event>>>);

impl Reply {
    fn new(tx: Sender<Event>) -> Self {
        Self(Cell::new(Some(tx)))
    }

    /// The sender, for the first caller only.
    fn take(&self) -> Option<Sender<Event>> {
        self.0.take()
    }

    /// Answer "dismissed with nothing chosen", unless something answered first.
    fn dismissed(&self) {
        if let Some(tx) = self.take() {
            let _ = tx.send(Event::Finished(0));
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.dismissed();
    }
}

/// The delegate's state for one pick.
struct DelegateIvars {
    /// The waiting call.
    reply: Reply,
    /// This call's pick-cache directory.
    dir: PathBuf,
    /// Which kinds of item to load.
    filter: MediaFilter,
    /// The call's kept loads and cancel flag.
    shared: Arc<Shared>,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - This class implements `Drop` only through its ivars (no manual impl),
    //   so the macro's generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // `PHPickerViewControllerDelegate` is a main-actor protocol, and the
    // delegate is built on the main queue (`Self::alloc(mtm)`).
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct FrustPhotoPickerDelegate;

    unsafe impl NSObjectProtocol for FrustPhotoPickerDelegate {}

    // SAFETY: the one method is the protocol's required
    // `- (void)picker:(PHPickerViewController *)picker
    // didFinishPicking:(NSArray<PHPickerResult *> *)results`, with the
    // header's argument and return types.
    unsafe impl PHPickerViewControllerDelegate for FrustPhotoPickerDelegate {
        #[unsafe(method(picker:didFinishPicking:))]
        fn did_finish_picking(
            &self,
            picker: &PHPickerViewController,
            results: &NSArray<PHPickerResult>,
        ) {
            // Called by UIKit: a panic here would unwind into an Objective-C
            // frame, which is undefined behavior (no unwind across FFI).
            let outcome = catch_unwind(AssertUnwindSafe(|| self.finish(picker, results)));
            if outcome.is_err() {
                log::error!("photo picker: panic while handling the picked items");
            }
        }
    }

    // SAFETY: the one method is the protocol's optional
    // `- (void)presentationControllerDidDismiss:(UIPresentationController *)`
    // (`UIPresentationController.h`), with the header's argument and return
    // types. UIKit calls it on the main thread after an interactive dismissal.
    unsafe impl UIAdaptivePresentationControllerDelegate for FrustPhotoPickerDelegate {
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_controller_did_dismiss(&self, _controller: &UIPresentationController) {
            // Called by UIKit: never unwind into its frame.
            let outcome = catch_unwind(AssertUnwindSafe(|| self.ivars().reply.dismissed()));
            if outcome.is_err() {
                log::error!("photo picker: panic while handling the dismissal");
            }
        }
    }
);

impl FrustPhotoPickerDelegate {
    fn new(
        mtm: MainThreadMarker,
        tx: Sender<Event>,
        dir: PathBuf,
        filter: MediaFilter,
        shared: Arc<Shared>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            reply: Reply::new(tx),
            dir,
            filter,
            shared,
        });
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set.
        unsafe { msg_send![super(this), init] }
    }

    /// `picker:didFinishPicking:`: dismiss, report the count, start the loads.
    fn finish(&self, picker: &PHPickerViewController, results: &NSArray<PHPickerResult>) {
        // The picker does not dismiss itself (`PHPicker.h`).
        picker.dismissViewControllerAnimated_completion(true, None);
        let Some(tx) = self.ivars().reply.take() else {
            return;
        };
        if self.ivars().shared.is_cancelled() {
            // The cancel's main-queue step is still queued: start nothing.
            return;
        }
        let _ = tx.send(Event::Finished(results.count()));
        let ut_type = UtType::lookup();
        for (index, result) in results.iter().enumerate() {
            // SAFETY: `itemProvider` is a read-only, non-null property getter
            // of a live `PHPickerResult` (`PHPicker.h`).
            let provider = unsafe { result.itemProvider() };
            let Some((type_id, kind)) =
                choose_type(&provider, self.ivars().filter, ut_type.as_ref())
            else {
                let _ = tx.send(Event::Item(
                    index,
                    Err("no image or video representation".to_owned()),
                ));
                continue;
            };
            load_copy(
                &provider,
                &type_id,
                LoadJob {
                    index,
                    kind,
                    dir: self.ivars().dir.clone(),
                    suggested_name: provider.suggestedName().map(|n| n.to_string()),
                    tx: tx.clone(),
                    shared: Arc::clone(&self.ivars().shared),
                },
            );
        }
    }
}

/// A configured picker and its delegate, built on the main queue.
pub(super) struct Picker {
    /// The view controller to present.
    pub(super) controller: Retained<PHPickerViewController>,
    delegate: Retained<FrustPhotoPickerDelegate>,
}

impl Picker {
    /// Build the configured picker with its delegate attached
    /// (`apple.rs`'s *Threading*).
    pub(super) fn new(
        mtm: MainThreadMarker,
        options: &PickOptions,
        tx: Sender<Event>,
        dir: PathBuf,
        shared: Arc<Shared>,
    ) -> Self {
        let configuration = configuration(options);
        let controller = PHPickerViewController::init_with_configuration(
            PHPickerViewController::alloc(mtm),
            &configuration,
        );
        let delegate = FrustPhotoPickerDelegate::new(mtm, tx, dir, options.filter, shared);
        controller.set_delegate(Some(ProtocolObject::from_ref(&*delegate)));
        tie_lifetime(&controller, &delegate);
        // Before presenting, this creates the sheet's presentation controller.
        if let Some(presentation) = controller.presentationController() {
            // SAFETY: `delegate` is a weak property (`UIPresentationController.h`).
            // `tie_lifetime` keeps the delegate alive as long as the picker, and
            // a presentation controller that outlives both reads a zeroed weak
            // reference. Both objects are touched on the main thread only.
            unsafe { presentation.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        }
        Self {
            controller,
            delegate,
        }
    }

    /// Stop the delegate from reporting: the presentation was refused.
    pub(super) fn abandon(&self) {
        drop(self.delegate.ivars().reply.take());
    }

    /// Weak handles for a cancel, which must not keep the picker alive.
    pub(super) fn downgrade(&self) -> WeakPicker {
        WeakPicker {
            controller: Weak::from_retained(&self.controller),
            delegate: Weak::from_retained(&self.delegate),
        }
    }
}

/// A picker that a cancel can still reach while it lives.
pub(super) struct WeakPicker {
    controller: Weak<PHPickerViewController>,
    delegate: Weak<FrustPhotoPickerDelegate>,
}

impl WeakPicker {
    /// Abandon the delegate first, so a dismissed picker never delivers
    /// `didFinishPicking:`, then dismiss the picker if it is still presented.
    pub(super) fn cancel(&self) {
        if let Some(delegate) = self.delegate.load() {
            drop(delegate.ivars().reply.take());
        }
        if let Some(controller) = self.controller.load()
            && controller.presentingViewController().is_some()
            && !controller.isBeingDismissed()
        {
            controller.dismissViewControllerAnimated_completion(true, None);
        }
    }
}

/// `PHPickerConfiguration()` (system library, no authorization), with the
/// call's limit and filter and the current representation (no transcoding).
fn configuration(options: &PickOptions) -> Retained<PHPickerConfiguration> {
    let limit = NSInteger::try_from(options.limit).unwrap_or(0);
    // SAFETY: plain `PHPickerConfiguration` class and instance messages with
    // the header's types: `new`, the `selectionLimit` setter (`0` = system
    // maximum), the `preferredAssetRepresentationMode` setter with a declared
    // enum value, the class filter getters, `anyFilterMatchingSubfilters:` with
    // an `NSArray<PHPickerFilter *>`, and the copying `filter` setter.
    unsafe {
        let filter = match options.filter {
            MediaFilter::Images => PHPickerFilter::imagesFilter(),
            MediaFilter::Videos => PHPickerFilter::videosFilter(),
            MediaFilter::ImagesAndVideos => {
                PHPickerFilter::anyFilterMatchingSubfilters(&NSArray::from_retained_slice(&[
                    PHPickerFilter::imagesFilter(),
                    PHPickerFilter::videosFilter(),
                ]))
            }
        };
        let configuration = PHPickerConfiguration::new();
        configuration.setSelectionLimit(limit);
        configuration.setFilter(Some(&filter));
        configuration.setPreferredAssetRepresentationMode(
            PHPickerConfigurationAssetRepresentationMode::Current,
        );
        configuration
    }
}

/// The associated-object key tying a delegate to its picker; only its address
/// matters.
static DELEGATE_KEY: u8 = 0;

/// Make `picker` own `delegate`: the `delegate` property is weak, and a
/// delegate released with its picker drops its sender, which the waiting call
/// reads as a cancel (`apple.rs`'s *Threading*).
fn tie_lifetime(picker: &PHPickerViewController, delegate: &FrustPhotoPickerDelegate) {
    let object: *const PHPickerViewController = picker;
    let value: *const FrustPhotoPickerDelegate = delegate;
    // SAFETY: both pointers come from live references; the key is the
    // address of a private `static`, unique to this association; the policy
    // is `OBJC_ASSOCIATION_RETAIN_NONATOMIC`, so the runtime retains the
    // delegate and releases it when the picker deallocates. Only the main
    // thread touches either object.
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            object.cast::<AnyObject>().cast_mut(),
            (&raw const DELEGATE_KEY).cast::<c_void>(),
            value.cast::<AnyObject>().cast_mut(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

/// `UTType`, reached at run time (module doc's *Type choice*).
struct UtType {
    class: &'static AnyClass,
    image: Retained<AnyObject>,
    movie: Retained<AnyObject>,
    directory: Retained<AnyObject>,
}

impl UtType {
    fn lookup() -> Option<Self> {
        let class = AnyClass::get(c"UTType")?;
        Some(Self {
            image: Self::type_with_identifier(class, &NSString::from_str(IMAGE_TYPE))?,
            movie: Self::type_with_identifier(class, &NSString::from_str(MOVIE_TYPE))?,
            directory: Self::type_with_identifier(class, &NSString::from_str(DIRECTORY_TYPE))?,
            class,
        })
    }

    fn type_with_identifier(
        class: &AnyClass,
        identifier: &NSString,
    ) -> Option<Retained<AnyObject>> {
        // SAFETY: `+[UTType typeWithIdentifier:]` takes an `NSString` and
        // returns a nullable autoreleased `UTType`; `class` is the runtime
        // `UTType` class.
        unsafe { msg_send![class, typeWithIdentifier: identifier] }
    }

    fn conforms(object: &AnyObject, to: &AnyObject) -> bool {
        // SAFETY: `-[UTType conformsToType:]` takes a non-null `UTType` and
        // returns `BOOL`; both receivers come from `typeWithIdentifier:`.
        unsafe { msg_send![object, conformsToType: to] }
    }

    /// The kind `identifier` stands for under `filter`, if any.
    fn kind_of(&self, identifier: &NSString, filter: MediaFilter) -> Option<MediaKind> {
        let ty = Self::type_with_identifier(self.class, identifier)?;
        if Self::conforms(&ty, &self.directory) {
            return None;
        }
        wanted(filter).iter().find_map(|&(_, kind)| {
            let target = match kind {
                MediaKind::Video => &self.movie,
                MediaKind::Image => &self.image,
            };
            Self::conforms(&ty, target).then_some(kind)
        })
    }
}

/// The abstract types `filter` admits, videos first (an item matches one).
fn wanted(filter: MediaFilter) -> &'static [(&'static str, MediaKind)] {
    match filter {
        MediaFilter::Images => &[(IMAGE_TYPE, MediaKind::Image)],
        MediaFilter::Videos => &[(MOVIE_TYPE, MediaKind::Video)],
        MediaFilter::ImagesAndVideos => &[
            (MOVIE_TYPE, MediaKind::Video),
            (IMAGE_TYPE, MediaKind::Image),
        ],
    }
}

/// The type identifier to load for `provider` and the kind it yields
/// (module doc's *Type choice*).
fn choose_type(
    provider: &NSItemProvider,
    filter: MediaFilter,
    ut_type: Option<&UtType>,
) -> Option<(Retained<NSString>, MediaKind)> {
    if let Some(ut_type) = ut_type {
        for identifier in provider.registeredTypeIdentifiers().iter() {
            if let Some(kind) = ut_type.kind_of(&identifier, filter) {
                return Some((identifier, kind));
            }
        }
    }
    wanted(filter).iter().find_map(|&(identifier, kind)| {
        let identifier = NSString::from_str(identifier);
        provider
            .hasItemConformingToTypeIdentifier(&identifier)
            .then_some((identifier, kind))
    })
}

/// What one item's completion handler needs.
struct LoadJob {
    index: usize,
    kind: MediaKind,
    dir: PathBuf,
    suggested_name: Option<String>,
    tx: Sender<Event>,
    /// The call's cancel flag and kept loads.
    shared: Arc<Shared>,
}

/// Start loading one file and keep its progress; the handler copies the file
/// and reports through the job.
fn load_copy(provider: &NSItemProvider, type_id: &NSString, job: LoadJob) {
    let (index, shared) = (job.index, Arc::clone(&job.shared));
    let handler = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
        // Runs on an item-provider queue: never unwind into its frame.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the item provider passes either null or a pointer to an
            // `NSURL`/`NSError` that stays valid for the handler's duration;
            // the references do not outlive this call.
            let (url, error) = unsafe { (url.as_ref(), error.as_ref()) };
            match url {
                // The temporary file is deleted when this handler returns, so
                // the copy happens here, synchronously (module doc).
                Some(url) => copy_into_cache(url, &job),
                None => Err(load_error(error)),
            }
        }))
        .unwrap_or_else(|_| Err("panic while copying the picked file".to_owned()));
        let _ = job.tx.send(Event::Item(job.index, outcome));
    });
    // SAFETY: the signature matches `NSItemProvider.h`'s
    // `loadFileRepresentationForTypeIdentifier:completionHandler:` (an
    // `NSString`, a `void (^)(NSURL *, NSError *)` block; returns a non-null
    // `NSProgress`, read as nullable). The block may run on any queue: it
    // captures only `Send` data (a `Sender`, owned paths and strings, an
    // `Arc<Shared>`).
    let progress: Option<Retained<NSProgress>> = unsafe {
        msg_send![
            provider,
            loadFileRepresentationForTypeIdentifier: type_id,
            completionHandler: &*handler
        ]
    };
    if let Some(progress) = progress {
        shared.keep_load(index, progress);
    }
}

/// A display-safe reason for a failed load: domain and code, never the
/// localized description (which can carry a path).
fn load_error(error: Option<&NSError>) -> String {
    match error {
        Some(e) => format!("item provider error {} {}", e.domain(), e.code()),
        None => "item provider returned no file".to_owned(),
    }
}

/// Copy the temporary file at `url` into the job's cache directory. A cancel
/// before or during the copy leaves no file: the call may already have removed
/// its directory, so a copy that lands afterwards deletes itself.
fn copy_into_cache(url: &NSURL, job: &LoadJob) -> Result<PickedItem, String> {
    if job.shared.is_cancelled() {
        return Err(CANCELLED.to_owned());
    }
    let source = url
        .path()
        .map(|p| PathBuf::from(p.to_string()))
        .ok_or_else(|| "item provider returned a non-file URL".to_owned())?;
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_owned();
    let raw_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let file_name = display_name(job.suggested_name.as_deref(), raw_name, &ext);
    let cached = sanitize_file_name(&file_name, job.index, &ext);
    let (path, mut dest) = create_unique(&job.dir, &cached, job.index)
        .map_err(|e| format!("cannot create the cache file: {:?}", e.kind()))?;
    let copied = File::open(&source).and_then(|mut src| io::copy(&mut src, &mut dest));
    if let Err(e) = copied {
        drop(dest);
        let _ = fs::remove_file(&path);
        return Err(format!("cannot copy the picked file: {:?}", e.kind()));
    }
    if job.shared.is_cancelled() {
        drop(dest);
        let _ = fs::remove_file(&path);
        // Succeeds only once the directory is empty; best effort.
        let _ = fs::remove_dir(&job.dir);
        return Err(CANCELLED.to_owned());
    }
    let mime = media_types::lookup(&ext).map(|(_, mime)| mime.to_owned());
    Ok(PickedItem {
        path,
        kind: job.kind,
        file_name: (!file_name.is_empty()).then_some(file_name),
        mime,
        asset_id: None,
        is_copy: true,
    })
}

/// The user-facing name: the provider's suggested name with the delivered
/// file's extension, or the delivered file's own name.
fn display_name(suggested: Option<&str>, delivered: &str, ext: &str) -> String {
    match suggested.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) if ext.is_empty() => name.to_owned(),
        Some(name) => {
            let has_ext = Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext));
            if has_ext {
                name.to_owned()
            } else {
                format!("{name}.{ext}")
            }
        }
        None => delivered.to_owned(),
    }
}

/// Create a new file named `name` in `dir`, or `<index>-name`,
/// `<index>-<n>-name` when taken. Handlers run concurrently, so creation is
/// exclusive (`create_new`) rather than checked first.
fn create_unique(dir: &Path, name: &str, index: usize) -> io::Result<(PathBuf, File)> {
    let mut attempt = 0usize;
    loop {
        let candidate = match attempt {
            0 => name.to_owned(),
            1 => format!("{index}-{name}"),
            n => format!("{index}-{n}-{name}"),
        };
        let path = dir.join(candidate);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 64 => attempt += 1,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_appends_the_delivered_extension() {
        assert_eq!(
            display_name(Some("IMG_0001"), "x.heic", "heic"),
            "IMG_0001.heic"
        );
        assert_eq!(
            display_name(Some("IMG_0001.HEIC"), "x.heic", "heic"),
            "IMG_0001.HEIC"
        );
        assert_eq!(display_name(Some("clip"), "x", ""), "clip");
        assert_eq!(display_name(None, "x.mov", "mov"), "x.mov");
        assert_eq!(display_name(Some("  "), "x.mov", "mov"), "x.mov");
    }

    #[test]
    fn create_unique_never_overwrites() {
        let dir = std::env::temp_dir().join(format!(
            "frust-photo-library-apple-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let (a, _) = create_unique(&dir, "a.jpg", 3).unwrap();
        let (b, _) = create_unique(&dir, "a.jpg", 3).unwrap();
        let (c, _) = create_unique(&dir, "a.jpg", 3).unwrap();
        assert_eq!(a.file_name().unwrap(), "a.jpg");
        assert_eq!(b.file_name().unwrap(), "3-a.jpg");
        assert_eq!(c.file_name().unwrap(), "3-2-a.jpg");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wanted_respects_the_filter() {
        assert_eq!(
            wanted(MediaFilter::Images),
            [(IMAGE_TYPE, MediaKind::Image)]
        );
        assert_eq!(
            wanted(MediaFilter::Videos),
            [(MOVIE_TYPE, MediaKind::Video)]
        );
        assert_eq!(wanted(MediaFilter::ImagesAndVideos).len(), 2);
    }

    #[test]
    fn load_errors_never_carry_a_path() {
        assert_eq!(load_error(None), "item provider returned no file");
    }

    /// Every `Event` the channel holds right now: `Some(n)` for `Finished(n)`,
    /// `None` for anything else.
    fn drain(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<Option<usize>> {
        rx.try_iter()
            .map(|e| match e {
                Event::Finished(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_dismissal_first_leaves_did_finish_picking_nothing() {
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = Reply::new(tx);
        reply.dismissed();
        assert!(reply.take().is_none());
        drop(reply);
        assert_eq!(drain(&rx), [Some(0)]);
    }

    #[test]
    fn did_finish_picking_first_leaves_a_dismissal_nothing() {
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = Reply::new(tx);
        let tx = reply.take().expect("the first answer gets the sender");
        tx.send(Event::Finished(2)).unwrap();
        reply.dismissed();
        drop(reply);
        assert_eq!(drain(&rx), [Some(2)]);
    }

    #[test]
    fn an_unanswered_reply_reads_as_a_dismissal() {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(Reply::new(tx));
        assert_eq!(drain(&rx), [Some(0)]);
        let (tx, rx) = std::sync::mpsc::channel::<Event>();
        let reply = Reply::new(tx);
        drop(reply.take());
        drop(reply);
        assert_eq!(drain(&rx), []);
    }
}
