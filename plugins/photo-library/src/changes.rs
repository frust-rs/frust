//! The library change feed: the persisted [`ChangeToken`], the [`LibraryChanges`] a token
//! answers, and the wake-up registry behind [`PhotoLibrary::on_library_changed`].
//!
//! The registry is platform-independent. A platform arm implements two hooks the crate root
//! hands it, `start_change_observer() -> Result<(), PhotoError>` (run when the first
//! subscriber registers) and `stop_change_observer()` (run when the last one drops), and its
//! native observer calls [`notify_all`] on every change. Both hooks run under the registry's
//! transition lock, never under the lock [`notify_all`] takes, so a native callback that fires
//! during a start or stop does not deadlock.
//!
//! [`PhotoLibrary::on_library_changed`]: crate::PhotoLibrary::on_library_changed

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::thumb_cache;
use crate::types::{AssetId, PhotoError};

/// A position in the photo library's change history, from
/// [`PhotoLibrary::current_change_token`](crate::PhotoLibrary::current_change_token) or a
/// [`LibraryChanges`] value's `next`.
///
/// Opaque: platform- and device-specific bytes the app stores and hands back, never
/// interprets. Persist it with [`as_bytes`](Self::as_bytes) and restore it with
/// [`from_bytes`](Self::from_bytes). A token from another device, platform or library (or one
/// the platform has expired) is not an error: [`PhotoLibrary::changes_since`] answers
/// [`LibraryChanges::FullRescan`] for it. `Debug` prints only the length.
///
/// [`PhotoLibrary::changes_since`]: crate::PhotoLibrary::changes_since
#[derive(Clone, PartialEq, Eq)]
pub struct ChangeToken(Vec<u8>);

impl ChangeToken {
    /// The token's bytes, to persist.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// A token restored from bytes [`as_bytes`](Self::as_bytes) returned earlier.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }
}

impl fmt::Debug for ChangeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeToken({} bytes)", self.0.len())
    }
}

/// What changed since a [`ChangeToken`], from
/// [`PhotoLibrary::changes_since`](crate::PhotoLibrary::changes_since).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LibraryChanges {
    /// The assets inserted, updated and deleted since the token. An id may already be gone by
    /// the time the app uses it ([`PhotoError::NotFound`]).
    Delta {
        /// Assets added to the library.
        inserted: Vec<AssetId>,
        /// Assets whose content or metadata changed.
        updated: Vec<AssetId>,
        /// Assets removed from the library.
        deleted: Vec<AssetId>,
        /// The token to pass next time.
        next: ChangeToken,
    },
    /// The platform cannot say what changed (a foreign or expired token, or no change history
    /// on this OS version): the app should re-read the library.
    FullRescan {
        /// The token to pass next time.
        next: ChangeToken,
    },
}

/// The `changes_since` path of the crate root; `load` is the platform call. A `Delta` evicts
/// every id it names from the thumbnail cache (an inserted id may be a row made visible again,
/// edited while hidden), a `FullRescan` clears it, both before the value is returned.
pub(crate) fn changes_since_with(
    token: &ChangeToken,
    load: impl FnOnce(&ChangeToken) -> Result<LibraryChanges, PhotoError>,
) -> Result<LibraryChanges, PhotoError> {
    let changes = load(token)?;
    match &changes {
        LibraryChanges::Delta {
            inserted,
            updated,
            deleted,
            ..
        } => thumb_cache::evict_ids(inserted.iter().chain(updated).chain(deleted)),
        LibraryChanges::FullRescan { .. } => thumb_cache::clear(),
    }
    Ok(changes)
}

type Callback = Arc<dyn Fn() + Send + Sync>;

/// The live subscribers, by registration id.
#[derive(Default)]
struct Subscribers {
    next_id: u64,
    live: Vec<(u64, Callback)>,
}

/// A subscriber registry with its observer transitions; a value so tests own an instance.
pub(crate) struct Feed {
    /// Held across a register or an unregister, so the empty/non-empty check and the observer
    /// hook it triggers are one step.
    transitions: Mutex<()>,
    /// Held only to change or snapshot the list; [`notify_all`](Self::notify_all) takes this one.
    subscribers: Mutex<Subscribers>,
}

impl Feed {
    pub(crate) const fn new() -> Self {
        Self {
            transitions: Mutex::new(()),
            subscribers: Mutex::new(Subscribers {
                next_id: 0,
                live: Vec::new(),
            }),
        }
    }

    fn subscribers(&self) -> MutexGuard<'_, Subscribers> {
        self.subscribers.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Registers `callback`, running `start` first when the registry is empty. A failed start
    /// registers nothing and returns its error.
    pub(crate) fn subscribe(
        &'static self,
        callback: Callback,
        start: impl FnOnce() -> Result<(), PhotoError>,
        stop: fn(),
    ) -> Result<ChangeSubscription, PhotoError> {
        let _transition = self.transitions.lock().unwrap_or_else(|p| p.into_inner());
        if self.subscribers().live.is_empty() {
            start()?;
        }
        let mut subs = self.subscribers();
        let id = subs.next_id;
        subs.next_id += 1;
        subs.live.push((id, callback));
        Ok(ChangeSubscription {
            id,
            feed: self,
            stop,
        })
    }

    /// Removes subscriber `id`, running `stop` when it was the last one.
    fn unsubscribe(&self, id: u64, stop: fn()) {
        let _transition = self.transitions.lock().unwrap_or_else(|p| p.into_inner());
        let now_empty = {
            let mut subs = self.subscribers();
            let before = subs.live.len();
            subs.live.retain(|(live, _)| *live != id);
            before != subs.live.len() && subs.live.is_empty()
        };
        if now_empty {
            stop();
        }
    }

    /// Calls every subscriber live when the call starts, outside every lock. A panicking
    /// callback is logged and does not stop the others.
    pub(crate) fn notify_all(&self) {
        let snapshot: Vec<Callback> = self
            .subscribers()
            .live
            .iter()
            .map(|(_, cb)| Arc::clone(cb))
            .collect();
        for callback in snapshot {
            if catch_unwind(AssertUnwindSafe(|| callback())).is_err() {
                log::error!("photo-library: a library-change callback panicked");
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.subscribers().live.len()
    }
}

/// The process-wide registry [`PhotoLibrary::on_library_changed`] registers into.
///
/// [`PhotoLibrary::on_library_changed`]: crate::PhotoLibrary::on_library_changed
pub(crate) static FEED: Feed = Feed::new();

/// Wakes every live [`PhotoLibrary::on_library_changed`] subscriber. The platform observers
/// call it from any thread; it never blocks on a callback's lock.
///
/// [`PhotoLibrary::on_library_changed`]: crate::PhotoLibrary::on_library_changed
// Only the mobile observers call it; desktop and web have none.
#[allow(dead_code)]
pub(crate) fn notify_all() {
    FEED.notify_all();
}

/// A live [`PhotoLibrary::on_library_changed`] registration. Dropping it unregisters the
/// callback; when the last one drops, the platform's change observer stops.
///
/// [`PhotoLibrary::on_library_changed`]: crate::PhotoLibrary::on_library_changed
#[must_use = "dropping the subscription unregisters the callback"]
pub struct ChangeSubscription {
    id: u64,
    feed: &'static Feed,
    stop: fn(),
}

impl fmt::Debug for ChangeSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChangeSubscription")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for ChangeSubscription {
    fn drop(&mut self) {
        self.feed.unsubscribe(self.id, self.stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Thumbnail, ThumbnailMode, ThumbnailRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serialises the tests that drive the process-wide thumbnail cache.
    fn cache_lock() -> MutexGuard<'static, ()> {
        static CACHE_TESTS: Mutex<()> = Mutex::new(());
        CACHE_TESTS.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn token_bytes_round_trip_and_debug_hides_them() {
        let token = ChangeToken::from_bytes(vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(token.as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(ChangeToken::from_bytes(token.as_bytes()), token);
        assert_ne!(ChangeToken::from_bytes(*b"other"), token);
        assert_eq!(format!("{token:?}"), "ChangeToken(4 bytes)");
        assert_eq!(
            format!("{:?}", ChangeToken::from_bytes(Vec::new())),
            "ChangeToken(0 bytes)"
        );
    }

    fn id(name: &str) -> AssetId {
        AssetId::from_raw(name)
    }

    fn key(name: &str) -> thumb_cache::Key {
        thumb_cache::Key::new(&id(name), &ThumbnailRequest::new(1, 1, ThumbnailMode::Fill))
    }

    fn cache(name: &str) {
        let epoch = thumb_cache::epoch();
        thumb_cache::insert(epoch, key(name), Arc::new(Thumbnail::new(1, 1, vec![0; 4])));
    }

    fn cached(name: &str) -> bool {
        thumb_cache::get(&key(name)).is_some()
    }

    fn next() -> ChangeToken {
        ChangeToken::from_bytes(*b"next")
    }

    #[test]
    fn a_delta_evicts_every_id_it_names_and_keeps_the_rest() {
        let _serial = cache_lock();
        thumb_cache::clear();
        for name in ["delta-ins", "delta-upd", "delta-del", "delta-keep"] {
            cache(name);
        }
        let delta = LibraryChanges::Delta {
            inserted: vec![id("delta-ins")],
            updated: vec![id("delta-upd")],
            deleted: vec![id("delta-del")],
            next: next(),
        };
        let answer = delta.clone();
        assert_eq!(
            changes_since_with(&ChangeToken::from_bytes(*b"old"), |token| {
                assert_eq!(token.as_bytes(), b"old");
                Ok(answer)
            }),
            Ok(delta)
        );
        // An inserted id may be a row made visible again after an edit while it was hidden.
        assert!(!cached("delta-ins"), "an inserted id is evicted");
        assert!(!cached("delta-upd"));
        assert!(!cached("delta-del"));
        assert!(
            cached("delta-keep"),
            "an id the delta does not name is kept"
        );
    }

    #[test]
    fn a_full_rescan_clears_the_cache() {
        let _serial = cache_lock();
        thumb_cache::clear();
        cache("rescan-a");
        cache("rescan-b");
        let rescan = LibraryChanges::FullRescan { next: next() };
        let answer = rescan.clone();
        assert_eq!(changes_since_with(&next(), |_| Ok(answer)), Ok(rescan));
        assert!(!cached("rescan-a"));
        assert!(!cached("rescan-b"));
    }

    #[test]
    fn a_failed_call_leaves_the_cache_alone() {
        let _serial = cache_lock();
        thumb_cache::clear();
        cache("failed-keep");
        assert_eq!(
            changes_since_with(&next(), |_| Err(PhotoError::PermissionDenied)),
            Err(PhotoError::PermissionDenied)
        );
        assert!(cached("failed-keep"));
    }

    #[test]
    fn an_in_flight_load_for_an_evicted_id_is_not_cached() {
        let _serial = cache_lock();
        thumb_cache::clear();
        // The load starts, the delta lands, then the stale pixels arrive.
        let started = thumb_cache::epoch();
        let delta = LibraryChanges::Delta {
            inserted: Vec::new(),
            updated: vec![id("in-flight")],
            deleted: Vec::new(),
            next: next(),
        };
        assert!(changes_since_with(&next(), |_| Ok(delta)).is_ok());
        thumb_cache::insert(
            started,
            key("in-flight"),
            Arc::new(Thumbnail::new(1, 1, vec![0; 4])),
        );
        assert!(!cached("in-flight"));
        // A load that starts after the delta is cached as usual.
        cache("in-flight");
        assert!(cached("in-flight"));
    }

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    static STOPS: AtomicUsize = AtomicUsize::new(0);

    fn count_stop() {
        STOPS.fetch_add(1, Ordering::SeqCst);
    }

    fn counting_start() -> Result<(), PhotoError> {
        STARTS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn counter() -> (Arc<AtomicUsize>, Callback) {
        let hits = Arc::new(AtomicUsize::new(0));
        let inner = Arc::clone(&hits);
        let callback: Callback = Arc::new(move || {
            inner.fetch_add(1, Ordering::SeqCst);
        });
        (hits, callback)
    }

    #[test]
    fn notify_reaches_only_live_subscribers_and_drop_unregisters() {
        static FEED: Feed = Feed::new();
        let (a_hits, a) = counter();
        let (b_hits, b) = counter();
        let first = FEED
            .subscribe(a, counting_start, count_stop)
            .expect("starts");
        let second = FEED
            .subscribe(b, counting_start, count_stop)
            .expect("joins");
        // The observer started once, for the first subscriber only.
        assert_eq!(STARTS.load(Ordering::SeqCst), 1);
        assert_eq!(FEED.len(), 2);

        FEED.notify_all();
        assert_eq!(
            (a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)),
            (1, 1)
        );

        drop(first);
        assert_eq!(FEED.len(), 1);
        assert_eq!(
            STOPS.load(Ordering::SeqCst),
            0,
            "a subscriber is still live"
        );
        FEED.notify_all();
        assert_eq!(
            (a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)),
            (1, 2)
        );

        drop(second);
        assert_eq!(FEED.len(), 0);
        assert_eq!(
            STOPS.load(Ordering::SeqCst),
            1,
            "the last drop stops the observer"
        );
        FEED.notify_all();
        assert_eq!(
            (a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)),
            (1, 2)
        );
    }

    #[test]
    fn a_failed_start_registers_nothing() {
        static FEED: Feed = Feed::new();
        fn refuse() -> Result<(), PhotoError> {
            Err(PhotoError::Unsupported)
        }
        fn never_stops() {
            panic!("nothing was registered, so nothing stops");
        }
        let (hits, callback) = counter();
        assert_eq!(
            FEED.subscribe(callback, refuse, never_stops).map(|_| ()),
            Err(PhotoError::Unsupported)
        );
        assert_eq!(FEED.len(), 0);
        FEED.notify_all();
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_callback_may_drop_its_own_subscription_and_a_panic_spares_the_others() {
        static FEED: Feed = Feed::new();
        fn no_op() {}
        let slot: Arc<Mutex<Option<ChangeSubscription>>> = Arc::new(Mutex::new(None));
        let inner = Arc::clone(&slot);
        let dropper: Callback = Arc::new(move || {
            inner.lock().unwrap().take();
        });
        let panicker: Callback = Arc::new(|| panic!("a callback that panics"));
        let (hits, counted) = counter();
        let own = FEED.subscribe(dropper, || Ok(()), no_op).expect("starts");
        *slot.lock().unwrap() = Some(own);
        let _panics = FEED.subscribe(panicker, || Ok(()), no_op).expect("joins");
        let _counted = FEED.subscribe(counted, || Ok(()), no_op).expect("joins");

        FEED.notify_all();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(FEED.len(), 2, "the dropper unregistered itself");
    }
}
