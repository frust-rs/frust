//! iOS: the library change feed and the cloud-identifier mapping (`library` feature only).
//!
//! # Change history
//!
//! Photos keeps a persistent change history from iOS 16: `currentChangeToken` and
//! `fetchPersistentChangesSinceToken:error:` (`PHPhotoLibrary.h`) are both iOS 16 API, so a
//! runtime availability check gates every use of them. Below iOS 16 there is no history: the
//! token is a fixed sentinel and every `changes_since` answers [`LibraryChanges::FullRescan`]
//! with that sentinel.
//!
//! From iOS 16 a [`ChangeToken`] holds a `PHPersistentChangeToken` archived with
//! `NSKeyedArchiver` under secure coding (the class adopts `NSSecureCoding`). `changes_since`
//! unarchives it; bytes that do not unarchive to a token (the sentinel, another platform's
//! bytes, anything foreign) answer a full rescan with a fresh token, not an error. It then
//! fetches the persistent changes since the token on the calling thread and merges the asset
//! details of every change, in order, into one delta ([`Merge`] holds the precedence rules).
//! `next` is the last change's token, or the token read just before the fetch when nothing
//! changed, so a change that lands during the call is reported next time rather than lost. An
//! expired token (`PHPhotosErrorPersistentChangeTokenExpired`) or a change whose details are gone
//! (`PHPhotosErrorPersistentChangeDetailsUnavailable`) also answers a full rescan with a fresh
//! token ([`history_failure`]).
//!
//! # Access and threading
//!
//! The token, change and cloud-identifier calls follow browsing (`apple_library`): they refuse
//! the main thread with [`PhotoError::UiThread`], and read-write authorization must be
//! `Authorized` or `Limited`, otherwise [`PhotoError::PermissionDenied`]. Both checks run on every
//! OS version, so iOS 15 answers the same errors as iOS 16.
//!
//! # Observer
//!
//! `start_change_observer` registers one process-wide `PHPhotoLibraryChangeObserver` after the
//! same authorization check (it never blocks, so it does not refuse the main thread); its
//! `photoLibraryDidChange:` calls `crate::changes::notify_all`. `stop_change_observer`
//! unregisters it. The observer object is built once and never released, so a stop issued from
//! the observer's own callback thread (a callback dropping the last subscription) never frees
//! the object whose method is running. The registration flag is held only around the register
//! and unregister messages and never while the callback runs. Both work on iOS 15.
//!
//! # Cloud identifiers
//!
//! `cloud_identifiers` sends `cloudIdentifierMappingsForLocalIdentifiers:` (iOS 15, the
//! deployment floor) once for the whole batch and answers one entry per input id, in order
//! ([`cloud_entry`]).
//!
//! The pure half of this module (the sentinel, the merge, the error mapping) has no Photos
//! dependency; the crate root also compiles it on the host for its tests. The PhotoKit half is
//! iOS-only.

use std::collections::BTreeMap;

use crate::changes::{ChangeToken, LibraryChanges};
use crate::types::{AssetId, PhotoError};

/// The token below iOS 16, where Photos keeps no change history. Not a keyed archive, so it never
/// unarchives to a token on iOS 16 either.
const NO_HISTORY_TOKEN: &[u8] = b"frust-photo-library:ios:no-change-history";

/// `PHPhotosErrorDomain`. The bindings' `PHError` feature is off, so the codes below are spelled
/// here from `PHError.h`, as in the crate's other Photos modules.
const PHOTOS_ERROR_DOMAIN: &str = "PHPhotosErrorDomain";
/// `PHPhotosErrorPersistentChangeTokenExpired` (`PHError.h`).
const PERSISTENT_CHANGE_TOKEN_EXPIRED: isize = 3105;
/// `PHPhotosErrorPersistentChangeDetailsUnavailable` (`PHError.h`).
const PERSISTENT_CHANGE_DETAILS_UNAVAILABLE: isize = 3210;
/// `PHPhotosErrorAccessRestricted` (`PHError.h`).
const ACCESS_RESTRICTED: isize = 3310;
/// `PHPhotosErrorAccessUserDenied` (`PHError.h`).
const ACCESS_USER_DENIED: isize = 3311;

/// The sentinel token iOS 15 hands out.
fn no_history_token() -> ChangeToken {
    ChangeToken::from_bytes(NO_HISTORY_TOKEN)
}

/// What `changes_since` answers below iOS 16, whatever the token.
fn without_history() -> LibraryChanges {
    LibraryChanges::FullRescan {
        next: no_history_token(),
    }
}

/// Whether `bytes` are worth unarchiving: neither empty nor the sentinel.
fn may_be_archived(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes != NO_HISTORY_TOKEN
}

/// How a failed history read ends the call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// Photos cannot say what changed: answer a full rescan with a fresh token.
    Rescan,
    /// The call fails with this error.
    Error(PhotoError),
}

/// The outcome for an `NSError` from the persistent-change fetch or a change's details.
fn history_failure(domain: &str, code: isize) -> Failure {
    let photos = domain == PHOTOS_ERROR_DOMAIN;
    match code {
        PERSISTENT_CHANGE_TOKEN_EXPIRED | PERSISTENT_CHANGE_DETAILS_UNAVAILABLE if photos => {
            Failure::Rescan
        }
        ACCESS_RESTRICTED | ACCESS_USER_DENIED if photos => {
            Failure::Error(PhotoError::PermissionDenied)
        }
        _ => Failure::Error(PhotoError::Platform(format!(
            "photos change history error {domain} {code}"
        ))),
    }
}

/// One cloud-identifier mapping's entry: `None` when the lookup failed or found nothing.
fn cloud_entry(value: Option<String>, failed: bool) -> Option<String> {
    if failed {
        return None;
    }
    value.filter(|value| !value.is_empty())
}

/// Where one asset stands across the merged window.
#[derive(Debug, Clone, Copy)]
struct Fate {
    /// Whether it existed when the window opened: its first event was an update or a delete.
    existed: bool,
    /// Whether it exists when the window closes: its last event was an insert or an update.
    exists: bool,
}

/// The asset details of a run of change batches, merged into one delta.
///
/// An asset that did not exist when the window opened and exists at its end is inserted, one
/// that existed and still exists is updated, and one gone at the end is deleted. So an id
/// inserted then deleted inside the window appears only in `deleted`, inserted then updated only
/// in `inserted`, updated then deleted only in `deleted`. Within one batch the inserted set
/// applies first, then the updated, then the deleted. Each list is sorted by identifier and holds
/// an id at most once, and no id is in two lists.
#[derive(Debug, Default)]
struct Merge {
    fates: BTreeMap<String, Fate>,
}

impl Merge {
    /// Applies one change batch's asset identifiers.
    fn batch(
        &mut self,
        inserted: impl IntoIterator<Item = String>,
        updated: impl IntoIterator<Item = String>,
        deleted: impl IntoIterator<Item = String>,
    ) {
        for id in inserted {
            self.note(id, false, true);
        }
        for id in updated {
            self.note(id, true, true);
        }
        for id in deleted {
            self.note(id, true, false);
        }
    }

    /// Records one event: `existed` counts only when it is the id's first.
    fn note(&mut self, id: String, existed: bool, exists: bool) {
        self.fates
            .entry(id)
            .and_modify(|fate| fate.exists = exists)
            .or_insert(Fate { existed, exists });
    }

    /// The merged delta, continuing from `next`.
    fn finish(self, next: ChangeToken) -> LibraryChanges {
        let mut inserted = Vec::new();
        let mut updated = Vec::new();
        let mut deleted = Vec::new();
        for (id, fate) in self.fates {
            let id = AssetId::from_raw(id);
            match (fate.existed, fate.exists) {
                (false, true) => inserted.push(id),
                (true, true) => updated.push(id),
                (_, false) => deleted.push(id),
            }
        }
        LibraryChanges::Delta {
            inserted,
            updated,
            deleted,
            next,
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) use photokit::{
    changes_since, cloud_identifiers, current_change_token, start_change_observer,
    stop_change_observer,
};

/// The PhotoKit half: everything that sends a message.
#[cfg(target_os = "ios")]
mod photokit {
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr::NonNull;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use block2::StackBlock;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, ClassType, MainThreadMarker, available, define_class, msg_send};
    use objc2_foundation::{
        NSArray, NSData, NSError, NSKeyedArchiver, NSKeyedUnarchiver, NSSet, NSString,
    };
    use objc2_photos::{
        PHAccessLevel, PHAuthorizationStatus, PHObjectType, PHPersistentChange,
        PHPersistentChangeFetchResult, PHPersistentChangeToken, PHPhotoLibrary,
        PHPhotoLibraryChangeObserver,
    };

    use super::{
        Failure, Merge, cloud_entry, history_failure, may_be_archived, no_history_token,
        without_history,
    };
    use crate::changes::{ChangeToken, LibraryChanges};
    use crate::types::{AssetId, PhotoError};

    /// Whether Photos keeps a persistent change history here (iOS 16 and later).
    fn has_history() -> bool {
        available!(ios = 16.0)
    }

    fn refuse_main_thread() -> Result<(), PhotoError> {
        if MainThreadMarker::new().is_some() {
            return Err(PhotoError::UiThread);
        }
        Ok(())
    }

    /// Read-write access is `Authorized` or `Limited`, failing first when Photos is not loaded
    /// (the bindings panic on a missing class).
    fn ensure_readable() -> Result<(), PhotoError> {
        if AnyClass::get(c"PHPhotoLibrary").is_none() {
            return Err(PhotoError::Platform(
                "Photos is not loaded; link Photos.framework".to_owned(),
            ));
        }
        // SAFETY: a class method taking a declared `PHAccessLevel` value and returning the
        // status; it reads state and never prompts.
        let status =
            unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite) };
        if status == PHAuthorizationStatus::Authorized || status == PHAuthorizationStatus::Limited {
            Ok(())
        } else {
            Err(PhotoError::PermissionDenied)
        }
    }

    fn library() -> Retained<PHPhotoLibrary> {
        // SAFETY: a class method on a class `ensure_readable` has confirmed is loaded; the
        // bindings mark the library `Send + Sync`.
        unsafe { PHPhotoLibrary::sharedPhotoLibrary() }
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn failure_of(error: &NSError) -> Failure {
        history_failure(&error.domain().to_string(), error.code())
    }

    pub(crate) fn current_change_token() -> Result<ChangeToken, PhotoError> {
        refuse_main_thread()?;
        autoreleasepool(|_| {
            ensure_readable()?;
            if !has_history() {
                return Ok(no_history_token());
            }
            fresh_token(&library())
        })
    }

    pub(crate) fn changes_since(token: &ChangeToken) -> Result<LibraryChanges, PhotoError> {
        refuse_main_thread()?;
        autoreleasepool(|_| {
            ensure_readable()?;
            if !has_history() {
                return Ok(without_history());
            }
            let library = library();
            let Some(since) = unarchive(token.as_bytes()) else {
                return rescan(&library);
            };
            // Read before the fetch: a change landing after it is in the fetch, or in the next.
            let before = fresh_token(&library)?;
            // SAFETY: the shared library and a live token, on iOS 16 or later (checked above).
            // The fetch is synchronous and runs on this (non-main) thread; its result and every
            // object it yields stay on this thread.
            let fetched = match unsafe { library.fetchPersistentChangesSinceToken_error(&since) } {
                Ok(fetched) => fetched,
                Err(error) => return settle(&library, failure_of(&error)),
            };
            match merge_changes(&fetched) {
                Ok((merge, Some(last))) => Ok(merge.finish(archive(&last)?)),
                Ok((merge, None)) => Ok(merge.finish(before)),
                Err(failure) => settle(&library, failure),
            }
        })
    }

    /// A full rescan continuing from the current token.
    fn rescan(library: &PHPhotoLibrary) -> Result<LibraryChanges, PhotoError> {
        Ok(LibraryChanges::FullRescan {
            next: fresh_token(library)?,
        })
    }

    fn settle(library: &PHPhotoLibrary, failure: Failure) -> Result<LibraryChanges, PhotoError> {
        match failure {
            Failure::Rescan => rescan(library),
            Failure::Error(error) => Err(error),
        }
    }

    /// The library's current token, archived. iOS 16 and later only.
    fn fresh_token(library: &PHPhotoLibrary) -> Result<ChangeToken, PhotoError> {
        // SAFETY: a property read on the shared library, on iOS 16 or later (every caller checks
        // `has_history` first), used on this thread only.
        let token = unsafe { library.currentChangeToken() };
        archive(&token)
    }

    /// `token` as secure-coded keyed-archive bytes.
    fn archive(token: &PHPersistentChangeToken) -> Result<ChangeToken, PhotoError> {
        let object: &AnyObject = token.as_ref();
        // SAFETY: the root object is a live `PHPersistentChangeToken`, which adopts
        // `NSSecureCoding` (`PHPersistentChangeToken.h`), so secure coding can encode it.
        let data = unsafe {
            NSKeyedArchiver::archivedDataWithRootObject_requiringSecureCoding_error(object, true)
        }
        .map_err(|error| {
            PhotoError::Platform(format!(
                "archiving the change token failed: {} {}",
                error.domain(),
                error.code()
            ))
        })?;
        Ok(ChangeToken::from_bytes(data.to_vec()))
    }

    /// The token `bytes` archive, or `None` for anything else.
    fn unarchive(bytes: &[u8]) -> Option<Retained<PHPersistentChangeToken>> {
        if !may_be_archived(bytes) {
            return None;
        }
        let data = NSData::with_bytes(bytes);
        // SAFETY: `cls` is the token class, which adopts `NSSecureCoding`. This class method
        // decodes with `requiresSecureCoding` on and the set-error-and-return failure policy
        // (`NSKeyedArchiver.h`), so bytes that are not such an archive return an error.
        let object = unsafe {
            NSKeyedUnarchiver::unarchivedObjectOfClass_fromData_error(
                PHPersistentChangeToken::class(),
                &data,
            )
        }
        .ok()?;
        object.downcast::<PHPersistentChangeToken>().ok()
    }

    /// The merge of every change in `fetched` so far, the last change's token, and the failure
    /// that stopped the walk.
    #[derive(Default)]
    struct Walk {
        merge: Merge,
        last: Option<Retained<PHPersistentChangeToken>>,
        failure: Option<Failure>,
    }

    type Merged = (Merge, Option<Retained<PHPersistentChangeToken>>);

    /// Walks every change batch in order, merging its asset details.
    fn merge_changes(fetched: &PHPersistentChangeFetchResult) -> Result<Merged, Failure> {
        let walk = RefCell::new(Walk::default());
        let block = StackBlock::new(|change: NonNull<PHPersistentChange>, stop: NonNull<Bool>| {
            // Called from Photos' enumeration: never unwind into its frame.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                autoreleasepool(|_| {
                    // SAFETY: Photos passes a change valid for this call; the reference does
                    // not outlive it.
                    let change = unsafe { change.as_ref() };
                    step(&mut walk.borrow_mut(), change)
                })
            }));
            let failure = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(failure)) => failure,
                Err(_) => Failure::Error(PhotoError::Platform(
                    "panic while reading the change history".to_owned(),
                )),
            };
            walk.borrow_mut().failure = Some(failure);
            // SAFETY: `stop` is the enumeration's own out-flag, valid for this call.
            unsafe { stop.as_ptr().write(Bool::YES) };
        });
        // SAFETY: a live result and a block with the header's signature. The enumeration is
        // synchronous and the block non-escaping (the binding borrows it), so `walk` outlives
        // every call.
        unsafe { fetched.enumerateChangesWithBlock(&block) };
        let walk = walk.into_inner();
        match walk.failure {
            Some(failure) => Err(failure),
            None => Ok((walk.merge, walk.last)),
        }
    }

    /// One change batch: its asset details into the merge, its token as the latest.
    fn step(walk: &mut Walk, change: &PHPersistentChange) -> Result<(), Failure> {
        // SAFETY: a live change, read on the thread that fetched it; `PHObjectType::Asset` is a
        // declared value.
        let details = unsafe { change.changeDetailsForObjectType_error(PHObjectType::Asset) }
            .map_err(|error| failure_of(&error))?;
        // SAFETY: read-only properties of live details and change objects, on this thread.
        unsafe {
            walk.merge.batch(
                strings(&details.insertedLocalIdentifiers()),
                strings(&details.updatedLocalIdentifiers()),
                strings(&details.deletedLocalIdentifiers()),
            );
            walk.last = Some(change.changeToken());
        }
        Ok(())
    }

    fn strings(set: &NSSet<NSString>) -> Vec<String> {
        set.iter().map(|id| id.to_string()).collect()
    }

    pub(crate) fn cloud_identifiers(ids: &[AssetId]) -> Result<Vec<Option<String>>, PhotoError> {
        refuse_main_thread()?;
        autoreleasepool(|_| {
            ensure_readable()?;
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let keys: Vec<Retained<NSString>> = ids
                .iter()
                .map(|id| NSString::from_str(id.as_str()))
                .collect();
            let local = NSArray::from_retained_slice(&keys);
            // SAFETY: the shared library and an `NSArray<NSString>` of local identifiers; the
            // method is iOS 15 API, the crate's deployment floor.
            let mappings = unsafe { library().cloudIdentifierMappingsForLocalIdentifiers(&local) };
            Ok(keys
                .iter()
                .map(|key| {
                    let mapping = mappings.objectForKey(key)?;
                    // SAFETY: read-only properties of a live mapping and cloud identifier.
                    unsafe {
                        let failed = mapping.error().is_some();
                        let value = mapping
                            .cloudIdentifier()
                            .map(|cloud| cloud.stringValue().to_string());
                        cloud_entry(value, failed)
                    }
                })
                .collect())
        })
    }

    /// The registered observer, if any. Held only around the register and unregister messages.
    static REGISTERED: Mutex<bool> = Mutex::new(false);

    define_class!(
        // SAFETY:
        // - `NSObject` has no subclassing requirements.
        // - The class has no ivars and no `Drop`, so the generated `dealloc` upholds nothing
        //   extra.
        #[unsafe(super(NSObject))]
        struct FrustPhotoLibraryChangeObserver;

        unsafe impl NSObjectProtocol for FrustPhotoLibraryChangeObserver {}

        // SAFETY: the one method is the protocol's required
        // `- (void)photoLibraryDidChange:(PHChange *)changeInstance` with the header's return
        // type; the argument is never read, so it is taken as an untyped object.
        unsafe impl PHPhotoLibraryChangeObserver for FrustPhotoLibraryChangeObserver {
            #[unsafe(method(photoLibraryDidChange:))]
            fn photo_library_did_change(&self, _change: &AnyObject) {
                // Called on a Photos queue: never unwind into its frame.
                if catch_unwind(crate::changes::notify_all).is_err() {
                    log::error!("photo-library: panic while waking the change subscribers");
                }
            }
        }
    );

    // SAFETY: the class has no ivars and its one method only calls `notify_all`, which is safe
    // from any thread, so sharing and sending a reference across threads is sound.
    unsafe impl Send for FrustPhotoLibraryChangeObserver {}
    // SAFETY: as for `Send`.
    unsafe impl Sync for FrustPhotoLibraryChangeObserver {}

    impl FrustPhotoLibraryChangeObserver {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: `NSObject`'s designated initializer, called on a freshly allocated
            // instance.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// The one observer, built on first use and never released.
    fn observer() -> &'static ProtocolObject<dyn PHPhotoLibraryChangeObserver> {
        static OBSERVER: OnceLock<Retained<FrustPhotoLibraryChangeObserver>> = OnceLock::new();
        ProtocolObject::from_ref(&**OBSERVER.get_or_init(FrustPhotoLibraryChangeObserver::new))
    }

    pub(crate) fn start_change_observer() -> Result<(), PhotoError> {
        autoreleasepool(|_| {
            ensure_readable()?;
            let mut registered = lock(&REGISTERED);
            if !*registered {
                // SAFETY: the shared library and a live observer that is never released.
                unsafe { library().registerChangeObserver(observer()) };
                *registered = true;
            }
            Ok(())
        })
    }

    pub(crate) fn stop_change_observer() {
        autoreleasepool(|_| {
            let mut registered = lock(&REGISTERED);
            if *registered {
                // SAFETY: the shared library and the observer registered above. Photos is loaded:
                // the registration's `ensure_readable` confirmed it.
                unsafe { library().unregisterChangeObserver(observer()) };
                *registered = false;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn asset_ids(names: &[&str]) -> Vec<AssetId> {
        names.iter().map(|name| AssetId::from_raw(*name)).collect()
    }

    fn next() -> ChangeToken {
        ChangeToken::from_bytes(*b"next")
    }

    fn delta(inserted: &[&str], updated: &[&str], deleted: &[&str]) -> LibraryChanges {
        LibraryChanges::Delta {
            inserted: asset_ids(inserted),
            updated: asset_ids(updated),
            deleted: asset_ids(deleted),
            next: next(),
        }
    }

    #[test]
    fn single_events_land_in_their_own_list() {
        let mut merge = Merge::default();
        merge.batch(ids(&["ins"]), ids(&["upd"]), ids(&["del"]));
        assert_eq!(merge.finish(next()), delta(&["ins"], &["upd"], &["del"]));
    }

    #[test]
    fn inserted_then_deleted_is_only_deleted() {
        let mut merge = Merge::default();
        merge.batch(ids(&["a"]), ids(&[]), ids(&[]));
        merge.batch(ids(&[]), ids(&[]), ids(&["a"]));
        assert_eq!(merge.finish(next()), delta(&[], &[], &["a"]));
    }

    #[test]
    fn inserted_then_updated_is_only_inserted() {
        let mut merge = Merge::default();
        merge.batch(ids(&["a"]), ids(&[]), ids(&[]));
        merge.batch(ids(&[]), ids(&["a"]), ids(&[]));
        merge.batch(ids(&[]), ids(&["a"]), ids(&[]));
        assert_eq!(merge.finish(next()), delta(&["a"], &[], &[]));
    }

    #[test]
    fn updated_then_deleted_is_only_deleted() {
        let mut merge = Merge::default();
        merge.batch(ids(&[]), ids(&["a"]), ids(&[]));
        merge.batch(ids(&[]), ids(&[]), ids(&["a"]));
        assert_eq!(merge.finish(next()), delta(&[], &[], &["a"]));
    }

    #[test]
    fn inserted_updated_and_deleted_in_one_batch_is_only_deleted() {
        let mut merge = Merge::default();
        merge.batch(ids(&["a", "b"]), ids(&["a"]), ids(&["a"]));
        assert_eq!(merge.finish(next()), delta(&["b"], &[], &["a"]));
    }

    #[test]
    fn deleted_then_back_is_updated() {
        // It existed before the window and exists after it.
        let mut merge = Merge::default();
        merge.batch(ids(&[]), ids(&[]), ids(&["a"]));
        merge.batch(ids(&["a"]), ids(&[]), ids(&[]));
        assert_eq!(merge.finish(next()), delta(&[], &["a"], &[]));
    }

    #[test]
    fn lists_are_sorted_and_hold_each_id_once() {
        let mut merge = Merge::default();
        merge.batch(ids(&["c", "a"]), ids(&["z", "y"]), ids(&[]));
        merge.batch(ids(&["b"]), ids(&["y", "c"]), ids(&["m"]));
        merge.batch(ids(&[]), ids(&["z"]), ids(&["m"]));
        assert_eq!(
            merge.finish(next()),
            delta(&["a", "b", "c"], &["y", "z"], &["m"])
        );
    }

    #[test]
    fn no_changes_is_an_empty_delta_with_the_given_token() {
        assert_eq!(Merge::default().finish(next()), delta(&[], &[], &[]));
    }

    #[test]
    fn expiry_and_missing_details_answer_a_rescan() {
        assert_eq!(
            history_failure(PHOTOS_ERROR_DOMAIN, PERSISTENT_CHANGE_TOKEN_EXPIRED),
            Failure::Rescan
        );
        assert_eq!(
            history_failure(PHOTOS_ERROR_DOMAIN, PERSISTENT_CHANGE_DETAILS_UNAVAILABLE),
            Failure::Rescan
        );
        assert_eq!(PERSISTENT_CHANGE_TOKEN_EXPIRED, 3105);
        assert_eq!(PERSISTENT_CHANGE_DETAILS_UNAVAILABLE, 3210);
    }

    #[test]
    fn access_errors_are_permission_denied_and_the_rest_platform_errors() {
        for code in [ACCESS_RESTRICTED, ACCESS_USER_DENIED] {
            assert_eq!(
                history_failure(PHOTOS_ERROR_DOMAIN, code),
                Failure::Error(PhotoError::PermissionDenied)
            );
        }
        assert!(matches!(
            history_failure(PHOTOS_ERROR_DOMAIN, 3169),
            Failure::Error(PhotoError::Platform(_))
        ));
        // The same numbers in another domain are not Photos' codes.
        for code in [PERSISTENT_CHANGE_TOKEN_EXPIRED, ACCESS_USER_DENIED] {
            assert!(matches!(
                history_failure("NSCocoaErrorDomain", code),
                Failure::Error(PhotoError::Platform(_))
            ));
        }
    }

    #[test]
    fn below_the_history_floor_every_answer_is_a_rescan_with_the_sentinel() {
        let sentinel = no_history_token();
        assert!(!sentinel.as_bytes().is_empty());
        assert_eq!(no_history_token(), sentinel, "the sentinel is fixed");
        assert_eq!(
            without_history(),
            LibraryChanges::FullRescan {
                next: sentinel.clone()
            }
        );
    }

    #[test]
    fn the_sentinel_and_empty_bytes_are_never_unarchived() {
        assert!(!may_be_archived(no_history_token().as_bytes()));
        assert!(!may_be_archived(&[]));
        assert!(may_be_archived(b"bplist00"));
        assert!(may_be_archived(b"another platform's token"));
    }

    #[test]
    fn a_cloud_entry_is_none_when_the_lookup_failed_or_found_nothing() {
        assert_eq!(
            cloud_entry(Some("cloud".to_owned()), false),
            Some("cloud".to_owned())
        );
        assert_eq!(cloud_entry(Some("cloud".to_owned()), true), None);
        assert_eq!(cloud_entry(None, false), None);
        assert_eq!(cloud_entry(Some(String::new()), false), None);
    }
}
