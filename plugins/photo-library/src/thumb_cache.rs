//! A process-wide, byte-budgeted LRU of decoded thumbnails.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

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
    thumb: Thumbnail,
    used: u64,
}

/// The cache proper; a value so tests own an instance.
pub(crate) struct Lru {
    map: HashMap<Key, Entry>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl Lru {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
        }
    }

    /// A hit refreshes the entry's recency.
    pub(crate) fn get(&mut self, key: &Key) -> Option<Thumbnail> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.map.get_mut(key)?;
        entry.used = tick;
        Some(entry.thumb.clone())
    }

    /// Caches `thumb`, evicting least-recently-used entries until under budget. An entry
    /// larger than the whole budget is not cached.
    pub(crate) fn insert(&mut self, key: Key, thumb: Thumbnail) {
        let size = thumb.rgba.len();
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.thumb.rgba.len();
        }
        if size > self.budget {
            return;
        }
        self.tick += 1;
        self.bytes += size;
        self.map.insert(
            key,
            Entry {
                thumb,
                used: self.tick,
            },
        );
        self.evict();
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
            let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
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

pub(crate) fn get(key: &Key) -> Option<Thumbnail> {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .get(key)
}

pub(crate) fn insert(key: Key, thumb: Thumbnail) {
    lock()
        .get_or_insert_with(|| Lru::new(DEFAULT_BUDGET))
        .insert(key, thumb);
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
    fn thumb(bytes: usize) -> Thumbnail {
        Thumbnail::new(1, (bytes / 4) as u32, vec![0; bytes])
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
