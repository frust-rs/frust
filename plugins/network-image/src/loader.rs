//! [`ImageLoader`]: memory hits, single-flight fetches, bounded decoding.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use frust::{ImageSource, spawn_blocking};
use futures_util::FutureExt;
use futures_util::future::Shared;

use crate::decode::{DecodeError, TargetPx, decode_shared};
use crate::disk_cache::DiskCache;
use crate::fetch::{FetchError, FetchRequest, ImageFetcher};
use crate::memory_cache::{CacheKey, DEFAULT_BUDGET_BYTES, MemoryCache, MemoryStats};

/// The boxed future [`ImageLoader::load`] returns.
pub type LoadFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Default disk-cache cap: 128 MiB.
const DEFAULT_DISK_CAP_BYTES: u64 = 128 * 1024 * 1024;

/// Disk-cache tuning. Nothing is opened until the first memory miss.
#[derive(Clone, Debug)]
pub struct DiskCacheConfig {
    /// Persist raw response bytes across restarts (default true).
    pub enabled: bool,
    /// Byte cap before LRU eviction (default 128 MiB).
    pub cap_bytes: u64,
    /// Cache directory; `None` uses `frust_paths::cache_dir()/frust-network-image`.
    pub dir_override: Option<PathBuf>,
}

impl Default for DiskCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cap_bytes: DEFAULT_DISK_CAP_BYTES,
            dir_override: None,
        }
    }
}

/// Loader tuning.
#[derive(Clone, Debug)]
pub struct LoaderConfig {
    /// Decoded-image memory budget in bytes (default 32 MiB).
    pub memory_budget_bytes: usize,
    /// Per-fetch body cap in bytes (default 20 MiB).
    pub max_fetch_bytes: usize,
    /// Persistent raw-bytes cache.
    pub disk: DiskCacheConfig,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            memory_budget_bytes: DEFAULT_BUDGET_BYTES,
            max_fetch_bytes: 20 * 1024 * 1024,
            disk: DiskCacheConfig::default(),
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
    /// No loader was reachable: neither `NetworkImages::install` nor a
    /// per-view `.loader(..)` override supplied one.
    #[error("no image loader installed")]
    NotInstalled,
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
    /// Opened once, lazily; `None` inside means memory-only.
    disk: OnceLock<Option<Arc<DiskCache>>>,
}

/// Log a disk problem at `warn` the first time only, `debug` afterwards.
fn warn_once(msg: &str) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        log::debug!("network-image: {msg}");
    } else {
        log::warn!("network-image: {msg} (further disk errors logged at debug)");
    }
}

/// The disk cache, opening it on the blocking pool the first time.
async fn disk_handle(inner: Arc<Inner>) -> Option<Arc<DiskCache>> {
    if !inner.config.disk.enabled {
        return None;
    }
    if let Some(open) = inner.disk.get() {
        return open.clone();
    }
    spawn_blocking(move || {
        inner
            .disk
            .get_or_init(|| open_disk(&inner.config.disk))
            .clone()
    })
    .await
    .unwrap_or(None)
}

fn open_disk(cfg: &DiskCacheConfig) -> Option<Arc<DiskCache>> {
    let dir = cfg
        .dir_override
        .clone()
        .or_else(|| frust_paths::cache_dir().map(|d| d.join("frust-network-image")));
    let Some(dir) = dir else {
        log::info!("network-image: no cache directory available; memory-only");
        return None;
    };
    match DiskCache::open(dir, cfg.cap_bytes) {
        Ok(cache) => Some(Arc::new(cache)),
        Err(e) => {
            warn_once(&format!("disk cache unavailable ({e}); memory-only"));
            None
        }
    }
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
                memory: Mutex::new(MemoryCache::new(config.memory_budget_bytes)),
                config,
                inflight: Mutex::new(HashMap::new()),
                raw: Mutex::new(HashMap::new()),
                disk: OnceLock::new(),
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
        let inner = self.inner.clone();
        let url = req.url.clone();
        let headers = req.headers.clone();
        // Disk hop and fetch both run lazily on first poll, so a disk hit
        // never reaches the fetcher.
        let boxed: LoadFuture<Result<Arc<Vec<u8>>, FetchError>> = Box::pin(async move {
            let disk = disk_handle(inner.clone()).await;
            if let Some(disk) = &disk {
                let (d, u) = (disk.clone(), url.clone());
                if let Ok(Some(bytes)) = spawn_blocking(move || d.get(&u)).await {
                    log::debug!("network-image: disk hit");
                    return Ok(Arc::new(bytes));
                }
            }
            let fetched = inner
                .fetcher
                .fetch(FetchRequest {
                    url: url.clone(),
                    headers,
                    max_bytes: inner.config.max_fetch_bytes,
                })
                .await?;
            let bytes = Arc::new(fetched.bytes);
            if let Some(disk) = disk {
                let b = bytes.clone();
                match spawn_blocking(move || disk.put(&url, &b)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn_once(&format!("disk write failed ({e})")),
                    Err(e) => warn_once(&format!("disk write task failed ({e})")),
                }
            }
            Ok(bytes)
        });
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

    /// The decoded image for `url` fitted to `target`, if the memory cache
    /// holds it. Synchronous, so a view can paint a cached image on its first
    /// frame instead of waiting a task round-trip. Counts as a use for LRU.
    pub fn peek_memory(&self, url: &str, target: Option<TargetPx>) -> Option<ImageSource> {
        let key = CacheKey {
            url: url.to_string(),
            target,
        };
        lock(&self.inner.memory).get(&key)
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
