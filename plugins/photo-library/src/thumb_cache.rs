//! A process-wide, byte-budgeted LRU of decoded thumbnails.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::library::{Thumbnail, ThumbnailMode, ThumbnailRequest};
use crate::types::AssetId;

/// Default budget: 48 MiB of pixel data.
pub(crate) const DEFAULT_BUDGET: usize = 48 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    id: AssetId,
    width: u32,
    height: u32,
    mode: ThumbnailMode,
}

impl Key {
    pub(crate) fn new(id: &AssetId, request: &ThumbnailRequest) -> Self {
        Self {
            id: id.clone(),
            width: request.width,
            height: request.height,
            mode: request.mode,
        }
    }
}

struct Entry {
    /// Shared, so a hit hands out a reference and never copies the pixels.
    thumb: Arc<Thumbnail>,
    used: u64,
}

/// The cache proper; a value so tests own an instance.
pub(crate) struct Lru {
    map: HashMap<Key, Entry>,
    /// Every key of `map`, by its entry's `used` tick: the first is the eviction victim.
    order: BTreeMap<u64, Key>,
    bytes: usize,
    budget: usize,
    tick: u64,
    /// Bumped by every [`clear`](Self::clear).
    epoch: u64,
}

impl Lru {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: BTreeMap::new(),
            bytes: 0,
            budget,
            tick: 0,
            epoch: 0,
        }
    }

    /// A hit refreshes the entry's recency.
    pub(crate) fn get(&mut self, key: &Key) -> Option<Arc<Thumbnail>> {
        let entry = self.map.get_mut(key)?;
        self.tick += 1;
        if let Some(indexed) = self.order.remove(&entry.used) {
            self.order.insert(self.tick, indexed);
        }
        entry.used = self.tick;
        Some(Arc::clone(&entry.thumb))
    }

    /// Caches `thumb`, evicting least-recently-used entries until under budget. An entry
    /// larger than the whole budget is not cached.
    pub(crate) fn insert(&mut self, key: Key, thumb: Arc<Thumbnail>) {
        let size = thumb.rgba.len();
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.thumb.rgba.len();
            self.order.remove(&old.used);
        }
        if size > self.budget {
            return;
        }
        self.tick += 1;
        self.bytes += size;
        self.order.insert(self.tick, key.clone());
        self.map.insert(
            key,
            Entry {
                thumb,
                used: self.tick,
            },
        );
        self.evict();
    }

    /// [`insert`](Self::insert), unless the cache was cleared since `epoch` was read: pixels
    /// loaded before a clear must not repopulate the cache after it.
    fn insert_since(&mut self, epoch: u64, key: Key, thumb: Arc<Thumbnail>) {
        if epoch == self.epoch {
            self.insert(key, thumb);
        }
    }

    /// Drops every entry and invalidates in-flight loads. The budget is unchanged.
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.bytes = 0;
        self.epoch += 1;
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.evict();
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }

    fn evict(&mut self) {
        while self.bytes > self.budget {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.thumb.rgba.len();
            }
        }
    }
}

static CACHE: Mutex<Option<Lru>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Lru>> {
    CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The cached thumbnail, shared with the cache: no pixel is copied.
pub(crate) fn get(key: &Key) -> Option<Arc<Thumbnail>> {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .get(key)
}

/// The value to pass to [`insert`] for a load starting now.
pub(crate) fn epoch() -> u64 {
    lock().get_or_insert_with(|| Lru::new(DEFAULT_BUDGET)).epoch
}

/// Caches a thumbnail whose load started at `epoch`; a load older than the last [`clear`] is
/// dropped.
pub(crate) fn insert(epoch: u64, key: Key, thumb: Arc<Thumbnail>) {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .insert_since(epoch, key, thumb);
}

/// Empties the cache and refuses the insert of any load already in flight.
pub(crate) fn clear() {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .clear();
}

pub(crate) fn set_budget(budget: usize) {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .set_budget(budget);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key {
        Key::new(
            &AssetId::from_raw(name),
            &ThumbnailRequest::new(1, 1, ThumbnailMode::Fill),
        )
    }

    /// A thumbnail of exactly `bytes` pixel bytes (a multiple of 4).
    fn thumb(bytes: usize) -> Arc<Thumbnail> {
        Arc::new(Thumbnail::new(1, (bytes / 4) as u32, vec![0; bytes]))
    }

    #[test]
    fn hit_and_miss() {
        let mut c = Lru::new(100);
        assert!(c.get(&key("a")).is_none());
        c.insert(key("a"), thumb(40));
        assert_eq!(c.get(&key("a")).map(|t| t.rgba.len()), Some(40));
        assert!(c.get(&key("b")).is_none());
        // The size is part of the key.
        let other = Key::new(
            &AssetId::from_raw("a"),
            &ThumbnailRequest::new(2, 1, ThumbnailMode::Fill),
        );
        assert!(c.get(&other).is_none());
        let fit = Key::new(
            &AssetId::from_raw("a"),
            &ThumbnailRequest::new(1, 1, ThumbnailMode::Fit),
        );
        assert!(c.get(&fit).is_none());
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let mut c = Lru::new(100);
        c.insert(key("a"), thumb(40));
        c.insert(key("b"), thumb(40));
        // Touch `a`, so `b` is now the oldest.
        assert!(c.get(&key("a")).is_some());
        c.insert(key("c"), thumb(40));
        assert!(c.get(&key("b")).is_none());
        assert!(c.get(&key("a")).is_some());
        assert!(c.get(&key("c")).is_some());
        assert_eq!(c.bytes(), 80);
    }

    #[test]
    fn hits_set_the_eviction_order() {
        let mut c = Lru::new(120);
        c.insert(key("a"), thumb(40));
        c.insert(key("b"), thumb(40));
        c.insert(key("c"), thumb(40));
        // Recency, oldest first, is now b, c, a; then c, a, b.
        assert!(c.get(&key("a")).is_some());
        assert!(c.get(&key("b")).is_some());
        c.insert(key("d"), thumb(40));
        assert_eq!(c.len(), 3);
        c.set_budget(80);
        assert_eq!(c.len(), 2);
        // `c` went to `d`, then `a` to the shrink: `b` and `d` are the newest two.
        assert!(c.get(&key("c")).is_none());
        assert!(c.get(&key("a")).is_none());
        assert!(c.get(&key("b")).is_some());
        assert!(c.get(&key("d")).is_some());
        assert_eq!(c.order.len(), c.len());
    }

    #[test]
    fn replacing_an_entry_refreshes_it_in_the_order() {
        let mut c = Lru::new(80);
        c.insert(key("a"), thumb(40));
        c.insert(key("b"), thumb(40));
        c.insert(key("a"), thumb(40));
        assert_eq!(c.order.len(), 2);
        c.insert(key("c"), thumb(40));
        assert!(c.get(&key("b")).is_none());
        assert!(c.get(&key("a")).is_some());
    }

    #[test]
    fn a_hit_shares_the_cached_pixels() {
        let mut c = Lru::new(100);
        let stored = thumb(40);
        c.insert(key("a"), Arc::clone(&stored));
        let first = c.get(&key("a")).expect("cached");
        let second = c.get(&key("a")).expect("cached");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&first, &stored));
    }

    #[test]
    fn clear_empties_and_keeps_the_budget() {
        let mut c = Lru::new(100);
        c.insert(key("a"), thumb(40));
        c.clear();
        assert_eq!((c.len(), c.bytes(), c.order.len()), (0, 0, 0));
        assert!(c.get(&key("a")).is_none());
        c.insert(key("b"), thumb(100));
        assert_eq!(c.bytes(), 100);
    }

    #[test]
    fn a_load_older_than_a_clear_is_not_cached() {
        let mut c = Lru::new(100);
        let before = c.epoch;
        c.insert_since(before, key("a"), thumb(40));
        assert!(c.get(&key("a")).is_some());
        c.clear();
        c.insert_since(before, key("b"), thumb(40));
        assert!(c.get(&key("b")).is_none());
        c.insert_since(c.epoch, key("c"), thumb(40));
        assert!(c.get(&key("c")).is_some());
    }

    #[test]
    fn shrinking_the_budget_evicts() {
        let mut c = Lru::new(100);
        c.insert(key("a"), thumb(40));
        c.insert(key("b"), thumb(40));
        c.set_budget(50);
        assert_eq!(c.len(), 1);
        assert!(c.get(&key("b")).is_some());
        c.set_budget(0);
        assert_eq!(c.len(), 0);
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn oversize_entry_is_not_cached() {
        let mut c = Lru::new(100);
        c.insert(key("a"), thumb(40));
        c.insert(key("big"), thumb(104));
        assert!(c.get(&key("big")).is_none());
        assert!(c.get(&key("a")).is_some());
    }

    #[test]
    fn reinsert_replaces_without_double_counting() {
        let mut c = Lru::new(100);
        c.insert(key("a"), thumb(40));
        c.insert(key("a"), thumb(60));
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 60);
    }
}
