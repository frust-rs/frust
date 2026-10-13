//! [`ImageLoader`]: memory hits, single-flight fetches, bounded decoding.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll};

use frust::{ImageSource, spawn_blocking};
use futures_util::FutureExt;
use futures_util::future::Shared;

use crate::decode::{DecodeError, TargetPx, decode_shared};
use crate::disk_cache::{DiskCache, DiskStats};
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
type KeyedFuture = Shared<LoadFuture<LoadResult>>;
type RawFuture = Shared<LoadFuture<Result<Arc<Vec<u8>>, FetchError>>>;

/// One keyed load, alive while any [`Consumer`] of it is.
struct InflightEntry {
    fut: KeyedFuture,
    /// Live consumers; the entry is removed (and its future dropped) at zero.
    consumers: usize,
    /// Tells this entry apart from a later one for the same key.
    id: u64,
}

/// Shared raw-bytes fetch for one URL, alive while any keyed load uses it.
struct RawEntry {
    fut: RawFuture,
    /// Unreleased [`RawRelease`] guards; the entry is removed at zero.
    users: usize,
    /// Tells this entry apart from a later one for the same URL.
    id: u64,
}

struct Inner {
    fetcher: Arc<dyn ImageFetcher>,
    config: LoaderConfig,
    memory: Mutex<MemoryCache>,
    inflight: Mutex<HashMap<CacheKey, InflightEntry>>,
    /// Raw bytes by URL while keyed loads still need them, so two targets of
    /// one URL fetch once. Transient: removed when the last user releases,
    /// whether it finished or was dropped.
    raw: Mutex<HashMap<String, RawEntry>>,
    /// Opened once, lazily; `None` inside means memory-only.
    disk: OnceLock<Option<Arc<DiskCache>>>,
    /// Source of [`InflightEntry::id`] / [`RawEntry::id`].
    next_id: AtomicU64,
}

impl Inner {
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// One caller's handle on a keyed load. Dropping it (completed or not)
/// releases its consumer count; the last one out removes the entry, so the
/// shared future is dropped rather than parked for a later caller to resume.
struct Consumer {
    fut: KeyedFuture,
    inner: Arc<Inner>,
    key: CacheKey,
    id: u64,
}

impl Future for Consumer {
    type Output = LoadResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<LoadResult> {
        self.fut.poll_unpin(cx)
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        // Bound outside the lock: the entry's future drops after it is released.
        let _removed = {
            let mut inflight = lock(&self.inner.inflight);
            match inflight.get_mut(&self.key) {
                Some(entry) if entry.id == self.id => {
                    entry.consumers = entry.consumers.saturating_sub(1);
                    if entry.consumers == 0 {
                        inflight.remove(&self.key)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
    }
}

/// One keyed load's share of a [`RawEntry`], released exactly once: when the
/// load drops it after the bytes arrive, or when the load itself is dropped.
struct RawRelease {
    inner: Arc<Inner>,
    url: String,
    id: u64,
}

impl Drop for RawRelease {
    fn drop(&mut self) {
        // Bound outside the lock: a removed entry may hold the last handle on
        // the fetch, whose drop cancels it.
        let _removed = {
            let mut raw = lock(&self.inner.raw);
            match raw.get_mut(&self.url) {
                Some(entry) if entry.id == self.id => {
                    entry.users = entry.users.saturating_sub(1);
                    if entry.users == 0 {
                        raw.remove(&self.url)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
    }
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
                next_id: AtomicU64::new(0),
            }),
        }
    }

    /// Load `req`: memory hit, else join an in-flight load, else start one.
    /// Registration happens now (not on first poll), so concurrent calls
    /// always share one fetch.
    ///
    /// A load lives exactly as long as its consumers: when every future this
    /// returned for a key is dropped before it completes (an unmounted view's
    /// aborted task), the fetch is cancelled and the next call for that key
    /// starts afresh. A finished load is never joined, so a failure is only
    /// ever seen by the callers that were waiting on it.
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
        if let Some(entry) = inflight.get_mut(&key) {
            entry.consumers += 1;
            return Box::pin(Consumer {
                fut: entry.fut.clone(),
                inner: self.inner.clone(),
                key,
                id: entry.id,
            });
        }
        let id = self.inner.next_id();
        let (raw, release) = self.acquire_raw(&req);
        let inner = self.inner.clone();
        let task_key = key.clone();
        let target = req.target;
        // Dropping this body before `raw` resolves drops `release` with it.
        let fut: LoadFuture<LoadResult> = Box::pin(async move {
            let fetched = raw.await;
            drop(release);
            let result = match fetched {
                Ok(bytes) => decode_shared(bytes, target).await.map_err(LoadError::from),
                Err(e) => Err(LoadError::Fetch(e)),
            };
            if let Ok(source) = &result {
                lock(&inner.memory).insert(task_key.clone(), source.clone());
            }
            let _removed = {
                let mut inflight = lock(&inner.inflight);
                match inflight.get(&task_key) {
                    Some(entry) if entry.id == id => inflight.remove(&task_key),
                    _ => None,
                }
            };
            result
        });
        let shared = fut.shared();
        inflight.insert(
            key.clone(),
            InflightEntry {
                fut: shared.clone(),
                consumers: 1,
                id,
            },
        );
        Box::pin(Consumer {
            fut: shared,
            inner: self.inner.clone(),
            key,
            id,
        })
    }

    /// Join or start the raw fetch for `req.url`, counting one more user.
    /// A fetch that already finished with an error is replaced, not joined.
    fn acquire_raw(&self, req: &LoadRequest) -> (RawFuture, RawRelease) {
        let mut raw = lock(&self.inner.raw);
        if let Some(entry) = raw.get_mut(&req.url)
            && !matches!(entry.fut.peek(), Some(Err(_)))
        {
            entry.users += 1;
            let release = RawRelease {
                inner: self.inner.clone(),
                url: req.url.clone(),
                id: entry.id,
            };
            return (entry.fut.clone(), release);
        }
        let id = self.inner.next_id();
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
        // A replaced failed entry keeps serving the users that already hold it;
        // their releases no longer match this URL's entry.
        let _replaced = raw.insert(
            req.url.clone(),
            RawEntry {
                fut: fut.clone(),
                users: 1,
                id,
            },
        );
        let release = RawRelease {
            inner: self.inner.clone(),
            url: req.url.clone(),
            id,
        };
        (fut, release)
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

    /// Disk cache occupancy, or `None` while the disk cache is disabled or
    /// not yet opened. Never triggers the lazy open, and does no IO beyond
    /// the cache's own mutex, so it is cheap enough to call every frame.
    pub fn disk_stats(&self) -> Option<DiskStats> {
        let cache = self.inner.disk.get()?.as_ref()?;
        Some(cache.stats())
    }

    /// Remove every on-disk entry and the index; the directory itself stays.
    /// A no-op while the disk cache is disabled or not yet opened.
    ///
    /// Synchronous: the removal is a handful of small unlinks plus one index
    /// write, fine for a button handler. A failure is logged, not returned.
    pub fn clear_disk(&self) {
        let Some(Some(cache)) = self.inner.disk.get() else {
            return;
        };
        if let Err(e) = cache.clear() {
            warn_once(&format!("disk clear failed ({e})"));
        }
    }

    /// [`clear_memory`](Self::clear_memory) plus [`clear_disk`](Self::clear_disk).
    ///
    /// Raw response bytes are never cached in memory beyond the loads that
    /// need them, so there is nothing else to clear: decoded images live in
    /// the memory cache, bytes in the disk cache.
    pub fn clear_all(&self) {
        self.clear_memory();
        self.clear_disk();
    }

    /// Number of keyed loads still running for at least one live consumer.
    /// A load whose every future was dropped (an unmounted view) is not
    /// counted: it was cancelled and removed with its last consumer.
    pub fn inflight_count(&self) -> usize {
        lock(&self.inner.inflight).len()
    }

    /// Number of per-URL raw fetches held for running loads. Test hook: zero
    /// once every load has finished or been dropped.
    #[doc(hidden)]
    pub fn raw_entry_count(&self) -> usize {
        lock(&self.inner.raw).len()
    }
}
