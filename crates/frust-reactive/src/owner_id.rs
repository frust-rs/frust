//! A supported identity for a reactive [`Owner`].
//!
//! `Owner` only exposes `debug_id()`, the address of its allocation, which is
//! documented as debugging-only and can be reused once an owner is dropped.
//! [`OwnerId`] is a monotonically assigned id that apps can use as a map key,
//! for example to keep one registration per component.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use reactive_graph::owner::Owner;

/// A process-unique identity for a reactive [`Owner`].
///
/// Contract:
/// - Equal ids mean the same owner (any clone of it).
/// - Ids are never reused within a process.
/// - The id of an owner that has been cleaned up is forgotten: a later request
///   on the same owner (or on a new owner reusing its allocation) gives a new
///   id.
/// - Ids are not stable across runs and carry no ordering meaning.
///
/// The registry is process-global and safe to use from any thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OwnerId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<usize, u64>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, u64>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> MutexGuard<'static, HashMap<usize, u64>> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

impl OwnerId {
    /// The id of `owner`, assigning one on first request.
    ///
    /// The entry is removed when the owner is cleaned up or dropped.
    pub fn of(owner: &Owner) -> OwnerId {
        let addr = owner.debug_id();
        let (id, fresh) = {
            let mut map = lock();
            match map.get(&addr) {
                Some(&id) => (id, false),
                None => {
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    map.insert(addr, id);
                    (id, true)
                }
            }
        };
        if fresh {
            // Registered outside the lock: cleanup closures take the lock.
            owner.with(|| {
                Owner::on_cleanup(move || {
                    let mut map = lock();
                    if map.get(&addr) == Some(&id) {
                        map.remove(&addr);
                    }
                })
            });
        }
        OwnerId(id)
    }

    /// The id of the current owner, or `None` when no owner is current.
    pub fn current() -> Option<OwnerId> {
        Owner::current().map(|o| OwnerId::of(&o))
    }
}

#[cfg(test)]
fn registry_len() -> usize {
    lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Serializes tests that assert on the global map's size.
    static MAP_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn clones_share_an_id() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let owner = Owner::new();
        let clone = owner.clone();
        assert_eq!(OwnerId::of(&owner), OwnerId::of(&clone));
    }

    #[test]
    fn child_differs_from_parent() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let parent = Owner::new();
        let child = parent.child();
        assert_ne!(OwnerId::of(&parent), OwnerId::of(&child));
    }

    #[test]
    fn dropped_owners_never_share_ids() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let owner = Owner::new();
            assert!(seen.insert(OwnerId::of(&owner)));
        }
    }

    #[test]
    fn cleanup_forgets_the_entry() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let owner = Owner::new();
        let before = OwnerId::of(&owner);
        owner.cleanup();
        let after = OwnerId::of(&owner);
        assert_ne!(before, after);

        let baseline = registry_len();
        for _ in 0..1000 {
            let o = Owner::new();
            OwnerId::of(&o);
            o.cleanup();
        }
        assert!(registry_len() <= baseline, "map grew across cycles");
    }

    #[test]
    fn current_tracks_the_ambient_owner() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(OwnerId::current(), None);
        let owner = Owner::new();
        owner.with(|| {
            assert_eq!(OwnerId::current(), Some(OwnerId::of(&owner)));
        });
        assert_eq!(OwnerId::current(), None);
    }

    #[test]
    fn works_from_another_thread() {
        let _guard = MAP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let owner = Owner::new();
        let here = OwnerId::of(&owner);
        let o2 = owner.clone();
        let there = std::thread::spawn(move || OwnerId::of(&o2)).join().unwrap();
        assert_eq!(here, there);
    }
}
