//! [`ImageLoader`]: memory hits, single-flight fetches, bounded decoding.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use frust::ImageSource;
use futures_util::FutureExt;
use futures_util::future::Shared;

use crate::decode::{DecodeError, TargetPx, decode_shared};
use crate::fetch::{FetchError, FetchRequest, ImageFetcher};
use crate::memory_cache::{CacheKey, DEFAULT_BUDGET_BYTES, MemoryCache, MemoryStats};

/// The boxed future [`ImageLoader::load`] returns.
pub type LoadFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Loader tuning.
#[derive(Clone, Copy, Debug)]
pub struct LoaderConfig {
    /// Decoded-image memory budget in bytes (default 32 MiB).
    pub memory_budget_bytes: usize,
    /// Per-fetch body cap in bytes (default 20 MiB).
    pub max_fetch_bytes: usize,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            memory_budget_bytes: DEFAULT_BUDGET_BYTES,
            max_fetch_bytes: 20 * 1024 * 1024,
        }
    }
}

/// One image load.
#[derive(Clone, Debug)]
pub struct LoadRequest {
    /// Absolute `http`/`https` URL.
    pub url: String,
    /// Extra request headers. Not part of the cache key.
    pub headers: Vec<(String, String)>,
    /// Pixel box to fit the decoded image inside; `None` keeps natural size.
    pub target: Option<TargetPx>,
}

/// Why a load failed.
#[non_exhaustive]
#[derive(thiserror::Error, Debug, Clone)]
pub enum LoadError {
    /// The fetch failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// The bytes could not be decoded.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

type LoadResult = Result<ImageSource, LoadError>;
type RawFuture = Shared<LoadFuture<Result<Arc<Vec<u8>>, FetchError>>>;

/// Shared raw-bytes fetch for one URL, alive while any keyed load uses it.
struct RawEntry {
    fut: RawFuture,
    users: usize,
}

struct Inner {
    fetcher: Arc<dyn ImageFetcher>,
    config: LoaderConfig,
    memory: Mutex<MemoryCache>,
    inflight: Mutex<HashMap<CacheKey, Shared<LoadFuture<LoadResult>>>>,
    /// Raw bytes by URL for the duration of the inflight set, so two targets
    /// of one URL still fetch once. Dropped when the last user finishes.
    raw: Mutex<HashMap<String, RawEntry>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Loads, decodes and caches network images. Cheap to clone-share via `Arc`.
pub struct ImageLoader {
    inner: Arc<Inner>,
}

impl ImageLoader {
    /// A loader fetching through `fetcher`.
    pub fn new(fetcher: Arc<dyn ImageFetcher>, config: LoaderConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                fetcher,
                config,
                memory: Mutex::new(MemoryCache::new(config.memory_budget_bytes)),
                inflight: Mutex::new(HashMap::new()),
                raw: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Load `req`: memory hit, else join an in-flight load, else start one.
    /// Registration happens now (not on first poll), so concurrent calls
    /// always share one fetch.
    pub fn load(&self, req: LoadRequest) -> LoadFuture<LoadResult> {
        let key = CacheKey {
            url: req.url.clone(),
            target: req.target,
        };
        let mut inflight = lock(&self.inner.inflight);
        // Checked under the inflight lock: completion inserts into memory
        // before it takes this lock to unregister, so no gap loses a result.
        if let Some(hit) = lock(&self.inner.memory).get(&key) {
            return Box::pin(std::future::ready(Ok(hit)));
        }
        if let Some(existing) = inflight.get(&key) {
            return Box::pin(existing.clone());
        }
        let raw = self.acquire_raw(&req);
        let inner = self.inner.clone();
        let task_key = key.clone();
        let target = req.target;
        let fut: LoadFuture<LoadResult> = Box::pin(async move {
            let fetched = raw.await;
            release_raw(&inner, &task_key.url);
            let result = match fetched {
                Ok(bytes) => decode_shared(bytes, target).await.map_err(LoadError::from),
                Err(e) => Err(LoadError::Fetch(e)),
            };
            if let Ok(source) = &result {
                lock(&inner.memory).insert(task_key.clone(), source.clone());
            }
            lock(&inner.inflight).remove(&task_key);
            result
        });
        let shared = fut.shared();
        inflight.insert(key, shared.clone());
        Box::pin(shared)
    }

    /// Join or start the raw fetch for `req.url`, counting one more user.
    fn acquire_raw(&self, req: &LoadRequest) -> RawFuture {
        let mut raw = lock(&self.inner.raw);
        if let Some(entry) = raw.get_mut(&req.url) {
            entry.users += 1;
            return entry.fut.clone();
        }
        let fetch = self.inner.fetcher.fetch(FetchRequest {
            url: req.url.clone(),
            headers: req.headers.clone(),
            max_bytes: self.inner.config.max_fetch_bytes,
        });
        let boxed: LoadFuture<Result<Arc<Vec<u8>>, FetchError>> =
            Box::pin(async move { fetch.await.map(|f| Arc::new(f.bytes)) });
        let fut = boxed.shared();
        raw.insert(
            req.url.clone(),
            RawEntry {
                fut: fut.clone(),
                users: 1,
            },
        );
        fut
    }

    /// Memory cache occupancy.
    pub fn memory_stats(&self) -> MemoryStats {
        lock(&self.inner.memory).stats()
    }

    /// Drop every cached decoded image (in-flight loads are unaffected).
    pub fn clear_memory(&self) {
        lock(&self.inner.memory).clear();
    }
}

fn release_raw(inner: &Inner, url: &str) {
    let mut raw = lock(&inner.raw);
    if let Some(entry) = raw.get_mut(url) {
        entry.users = entry.users.saturating_sub(1);
        if entry.users == 0 {
            raw.remove(url);
        }
    }
}
