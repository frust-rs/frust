//! A hand-rolled, byte-budgeted LRU of decoded images.

use std::collections::{HashMap, VecDeque};

use frust::ImageSource;

use crate::decode::TargetPx;

/// Default memory budget: 32 MiB of decoded RGBA8.
pub(crate) const DEFAULT_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// Cache identity: the URL plus the pixel box it was fitted to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub(crate) url: String,
    pub(crate) target: Option<TargetPx>,
}

struct Entry {
    source: ImageSource,
    bytes: usize,
}

/// A snapshot of the memory cache, for tests and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryStats {
    /// Number of cached images.
    pub entries: usize,
    /// Decoded bytes held.
    pub used_bytes: usize,
    /// The configured budget.
    pub budget_bytes: usize,
}

pub(crate) struct MemoryCache {
    budget_bytes: usize,
    used: usize,
    map: HashMap<CacheKey, Entry>,
    /// Least recently used at the front.
    order: VecDeque<CacheKey>,
}

impl MemoryCache {
    pub(crate) fn new(budget_bytes: usize) -> Self {
        Self {
            budget_bytes,
            used: 0,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn touch(&mut self, key: &CacheKey) {
        if let Some(k) = self
            .order
            .iter()
            .position(|k| k == key)
            .and_then(|pos| self.order.remove(pos))
        {
            self.order.push_back(k);
        }
    }

    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<ImageSource> {
        let source = self.map.get(key)?.source.clone();
        self.touch(key);
        Some(source)
    }

    pub(crate) fn insert(&mut self, key: CacheKey, source: ImageSource) {
        let size = source.natural_size();
        let bytes = (size.width as usize)
            .saturating_mul(size.height as usize)
            .saturating_mul(4);
        if let Some(old) = self.map.remove(&key) {
            self.used -= old.bytes;
            self.order.retain(|k| k != &key);
        }
        if bytes > self.budget_bytes {
            return;
        }
        while self.used + bytes > self.budget_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.used -= e.bytes;
            }
        }
        self.used += bytes;
        self.order.push_back(key.clone());
        self.map.insert(key, Entry { source, bytes });
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.used = 0;
    }

    pub(crate) fn stats(&self) -> MemoryStats {
        MemoryStats {
            entries: self.map.len(),
            used_bytes: self.used,
            budget_bytes: self.budget_bytes,
        }
    }
}
