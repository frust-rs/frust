//! `ImageLoader` against a counting in-process fetcher.

use std::future::poll_fn;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use frust_network_image::{
    DecodeError, DecodeLimits, DiskCacheConfig, FetchError, FetchFuture, FetchRequest,
    FetchedBytes, ImageFetcher, ImageLoader, LoadError, LoadRequest, LoaderConfig, TargetPx,
};

/// Every fetch waits on it until it opens.
struct Gate {
    state: Mutex<(bool, Vec<Waker>)>,
}

impl Gate {
    fn new(open: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((open, Vec::new())),
        })
    }

    fn open(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        for waker in state.1.drain(..) {
            waker.wake();
        }
    }

    async fn wait(&self) {
        poll_fn(|cx| {
            let mut state = self.state.lock().unwrap();
            if state.0 {
                Poll::Ready(())
            } else {
                state.1.push(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

/// Serves a PNG (or a JPEG for `.jpg` URLs) after `delay` and the gate; while
/// `fail` is set (read once the gate opens) it answers 503 instead.
struct FakeFetcher {
    calls: AtomicUsize,
    completed: Arc<AtomicUsize>,
    gate: Arc<Gate>,
    fail: Arc<AtomicBool>,
    delay: Duration,
    png: Vec<u8>,
    jpeg: Vec<u8>,
}

impl FakeFetcher {
    fn new(delay: Duration) -> Arc<Self> {
        Self::build(delay, Gate::new(true))
    }

    /// A fetcher whose fetches all wait until `gate.open()`.
    fn gated() -> Arc<Self> {
        Self::build(Duration::ZERO, Gate::new(false))
    }

    fn build(delay: Duration, gate: Arc<Gate>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            completed: Arc::new(AtomicUsize::new(0)),
            gate,
            fail: Arc::new(AtomicBool::new(false)),
            delay,
            png: encode(8, 8, image::ImageFormat::Png),
            jpeg: encode(4000, 3000, image::ImageFormat::Jpeg),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Fetches that got past the gate (a dropped fetch never does).
    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    fn set_fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }
}

fn encode(w: u32, h: u32, format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .expect("encode test image");
    out.into_inner()
}

impl ImageFetcher for FakeFetcher {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = self.delay;
        let bytes = if request.url.ends_with(".jpg") {
            self.jpeg.clone()
        } else {
            self.png.clone()
        };
        let (gate, fail, completed) =
            (self.gate.clone(), self.fail.clone(), self.completed.clone());
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            gate.wait().await;
            completed.fetch_add(1, Ordering::SeqCst);
            if fail.load(Ordering::SeqCst) {
                return Err(FetchError::Status(503));
            }
            Ok(FetchedBytes {
                bytes,
                content_type: None,
            })
        })
    }
}

/// Init the reactive runtime (it owns the blocking pool `spawn_blocking` uses)
/// from sync context, then drive `fut` on a tokio runtime.
fn run<F: std::future::Future>(fut: F) -> F::Output {
    frust_reactive::ReactiveRuntime::init(Arc::new(|| {}));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

fn req(url: &str, target: Option<TargetPx>) -> LoadRequest {
    LoadRequest {
        url: url.to_string(),
        headers: Vec::new(),
        target,
    }
}

fn loader(fetcher: &Arc<FakeFetcher>, budget: usize) -> ImageLoader {
    ImageLoader::new(
        fetcher.clone(),
        LoaderConfig {
            memory_budget_bytes: budget,
            disk: DiskCacheConfig {
                enabled: false,
                ..DiskCacheConfig::default()
            },
            ..LoaderConfig::default()
        },
    )
}

#[test]
fn concurrent_loads_share_one_fetch_then_hit_memory() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(50));
        let loader = loader(&fetcher, 1 << 20);
        let loads = (0..8).map(|_| loader.load(req("https://x/a.png", None)));
        let results = futures_join(loads).await;
        assert_eq!(fetcher.calls(), 1);
        let first = results[0].as_ref().expect("load ok");
        assert!(
            results
                .iter()
                .all(|r| r.as_ref().is_ok_and(|s| s.same(first)))
        );

        let again = loader
            .load(req("https://x/a.png", None))
            .await
            .expect("hit");
        assert!(again.same(first));
        assert_eq!(fetcher.calls(), 1);
    });
}

async fn futures_join<F: std::future::Future + Send + 'static>(
    futs: impl Iterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.expect("join"));
    }
    out
}

#[test]
fn large_jpeg_is_downscaled_to_target() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::ZERO);
        let loader = loader(&fetcher, 32 << 20);
        let target = TargetPx {
            width: 512,
            height: 512,
        };
        let src = loader
            .load(req("https://x/big.jpg", Some(target)))
            .await
            .expect("decode");
        let size = src.natural_size();
        assert!(size.width <= 512.0 && size.height <= 512.0);
        assert_eq!((size.width, size.height), (512.0, 384.0));
    });
}

#[test]
fn lru_evicts_least_recently_used() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::ZERO);
        // Each 8x8 image is 256 bytes; the budget holds exactly three.
        let loader = loader(&fetcher, 3 * 256);
        for n in ["a", "b", "c"] {
            loader
                .load(req(&format!("https://x/{n}.png"), None))
                .await
                .expect("ok");
        }
        assert_eq!(fetcher.calls(), 3);
        // Promote "a", then insert "d": "b" is the eviction victim.
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("hit");
        loader.load(req("https://x/d.png", None)).await.expect("ok");
        assert_eq!(fetcher.calls(), 4);
        assert_eq!(loader.memory_stats().entries, 3);
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("hit");
        loader
            .load(req("https://x/c.png", None))
            .await
            .expect("hit");
        assert_eq!(fetcher.calls(), 4);
        loader
            .load(req("https://x/b.png", None))
            .await
            .expect("refetch");
        assert_eq!(fetcher.calls(), 5);
    });
}

#[test]
fn different_targets_are_separate_keys_but_share_one_fetch() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(50));
        let loader = loader(&fetcher, 1 << 20);
        let t = |n| {
            Some(TargetPx {
                width: n,
                height: n,
            })
        };
        let a = loader.load(req("https://x/a.png", t(4)));
        let b = loader.load(req("https://x/a.png", t(2)));
        let (a, b) = (tokio::spawn(a), tokio::spawn(b));
        let (a, b) = (
            a.await.expect("join").expect("ok"),
            b.await.expect("join").expect("ok"),
        );
        assert_eq!(fetcher.calls(), 1);
        assert!(!a.same(&b));
        assert_eq!(a.natural_size().width, 4.0);
        assert_eq!(b.natural_size().width, 2.0);
        assert_eq!(loader.memory_stats().entries, 2);
    });
}

#[test]
fn loader_is_send_sync() {
    fn assert_ss<T: Send + Sync>() {}
    assert_ss::<ImageLoader>();
}

/// A unique scratch dir under the system temp dir, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("frust-ni-loader-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn disk_stats_and_clear_all_with_disk_enabled() {
    let tmp = TempDir::new("stats");
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(1));
        let loader = ImageLoader::new(
            fetcher.clone(),
            LoaderConfig {
                disk: DiskCacheConfig {
                    enabled: true,
                    dir_override: Some(tmp.0.clone()),
                    ..DiskCacheConfig::default()
                },
                ..LoaderConfig::default()
            },
        );
        assert!(loader.disk_stats().is_none());
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("load ok");
        let stats = loader.disk_stats().expect("disk open after a load");
        assert_eq!(stats.entries, 1);
        assert!(stats.total_bytes > 0);
        assert_eq!(loader.memory_stats().entries, 1);

        loader.clear_all();
        assert_eq!(loader.memory_stats().entries, 0);
        let stats = loader.disk_stats().expect("still open");
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.total_bytes, 0);
        assert!(tmp.0.is_dir());
    });
}

#[test]
fn disk_disabled_has_no_stats_and_clear_disk_is_noop() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(1));
        let loader = loader(&fetcher, 1 << 20);
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("load ok");
        assert!(loader.disk_stats().is_none());
        loader.clear_disk();
        assert!(loader.disk_stats().is_none());
        assert_eq!(loader.memory_stats().entries, 1);
    });
}

#[test]
fn inflight_count_tracks_pending_load() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(200));
        let loader = loader(&fetcher, 1 << 20);
        assert_eq!(loader.inflight_count(), 0);
        let pending = tokio::spawn(loader.load(req("https://x/a.png", None)));
        assert_eq!(loader.inflight_count(), 1);
        pending.await.expect("join").expect("load ok");
        assert_eq!(loader.inflight_count(), 0);
    });
}

/// Yield until `cond` holds; panics after 10 s.
async fn until(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Abort a spawned load and wait until tokio has dropped its future.
async fn abort(handle: tokio::task::JoinHandle<Result<frust::ImageSource, LoadError>>) {
    handle.abort();
    let err = handle.await.expect_err("aborted before completing");
    assert!(err.is_cancelled());
}

fn target(n: u32) -> Option<TargetPx> {
    Some(TargetPx {
        width: n,
        height: n,
    })
}

/// Review scenario (act_…b4b5f): a load aborted mid-fetch is cancelled with
/// its last consumer, so the same key loads afresh instead of resuming the
/// stale future (whose transport timeout may already have elapsed).
#[test]
fn aborted_load_is_replaced_by_a_fresh_fetch() {
    run(async {
        let fetcher = FakeFetcher::gated();
        let loader = loader(&fetcher, 1 << 20);
        let first = tokio::spawn(loader.load(req("https://x/a.png", None)));
        until("the first fetch to start", || fetcher.calls() == 1).await;
        assert_eq!(loader.inflight_count(), 1);
        abort(first).await;
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0);

        fetcher.gate.open();
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("fresh load ok");
        assert_eq!(fetcher.calls(), 2, "a second, fresh fetch");
        assert_eq!(fetcher.completed(), 1, "the aborted fetch never resumed");
        assert_eq!(loader.memory_stats().entries, 1);
    });
}

/// The same for a load dropped before it was ever polled: nothing fetched,
/// nothing left registered.
#[test]
fn never_polled_load_dropped_leaves_nothing() {
    run(async {
        let fetcher = FakeFetcher::gated();
        let loader = loader(&fetcher, 1 << 20);
        let a = loader.load(req("https://x/a.png", None));
        let b = loader.load(req("https://x/a.png", None));
        assert_eq!(loader.inflight_count(), 1);
        drop(a);
        assert_eq!(loader.inflight_count(), 1, "b still consumes it");
        drop(b);
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0);
        assert_eq!(fetcher.calls(), 0);
    });
}

/// Review scenario (act_…b4b61): a thumbnail load dropped mid-fetch releases
/// its raw share, so after the shared fetch fails for the detail load, a
/// retry fetches again instead of joining the cached Err.
#[test]
fn dropped_share_never_pins_a_failed_raw_fetch() {
    run(async {
        let fetcher = FakeFetcher::gated();
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        let thumb = tokio::spawn(loader.load(req(url, target(2))));
        let detail = tokio::spawn(loader.load(req(url, target(4))));
        until("the shared fetch to start", || fetcher.calls() == 1).await;
        assert_eq!(loader.inflight_count(), 2);
        assert_eq!(loader.raw_entry_count(), 1);
        abort(thumb).await;
        assert_eq!(loader.inflight_count(), 1);
        assert_eq!(loader.raw_entry_count(), 1, "detail still shares it");

        fetcher.set_fail(true);
        fetcher.gate.open();
        let err = detail.await.expect("join").expect_err("the fetch fails");
        assert!(matches!(err, LoadError::Fetch(FetchError::Status(503))));
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0, "a failed fetch is not kept");

        fetcher.set_fail(false);
        let src = loader.load(req(url, target(4))).await.expect("retry ok");
        assert_eq!(src.natural_size().width, 4.0);
        assert_eq!(fetcher.calls(), 2);
    });
}

/// Every consumer gone leaves an empty inflight map and raw map, whether the
/// loads finished or were dropped; a finished load keeps no raw bytes.
#[test]
fn no_entries_outlive_their_consumers() {
    run(async {
        let fetcher = FakeFetcher::gated();
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        let a = tokio::spawn(loader.load(req(url, target(2))));
        let b = tokio::spawn(loader.load(req(url, target(4))));
        until("the shared fetch to start", || fetcher.calls() == 1).await;
        abort(a).await;
        abort(b).await;
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0);

        fetcher.gate.open();
        let c = loader.load(req(url, target(2)));
        let d = loader.load(req(url, target(4)));
        let (c, d) = (tokio::spawn(c), tokio::spawn(d));
        c.await.expect("join").expect("ok");
        d.await.expect("join").expect("ok");
        assert_eq!(fetcher.calls(), 2, "one fresh fetch for both targets");
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0, "no raw body retained");
        assert_eq!(loader.memory_stats().entries, 2);
    });
}

/// Single-flight survives consumers leaving: eight loads of one key, seven
/// aborted mid-fetch, still cost one fetch and the survivor gets the image.
#[test]
fn single_flight_survives_aborted_siblings() {
    run(async {
        let fetcher = FakeFetcher::gated();
        let loader = loader(&fetcher, 1 << 20);
        let mut handles: Vec<_> = (0..8)
            .map(|_| tokio::spawn(loader.load(req("https://x/a.png", None))))
            .collect();
        until("the fetch to start", || fetcher.calls() == 1).await;
        let survivor = handles.pop().expect("eight handles");
        for h in handles {
            abort(h).await;
        }
        assert_eq!(loader.inflight_count(), 1);
        fetcher.gate.open();
        survivor.await.expect("join").expect("load ok");
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(loader.inflight_count(), 0);
        assert_eq!(loader.raw_entry_count(), 0);
    });
}

fn req_h(url: &str, headers: &[(&str, &str)]) -> LoadRequest {
    LoadRequest {
        url: url.to_string(),
        headers: headers
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
        target: None,
    }
}

#[test]
fn different_headers_are_separate_fetches_and_memory_entries() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::ZERO);
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        let a = [("Authorization", "Bearer a")];
        let b = [("Authorization", "Bearer b")];
        loader.load(req_h(url, &a)).await.expect("a");
        loader.load(req_h(url, &b)).await.expect("b");
        assert_eq!(fetcher.calls(), 2);
        assert_eq!(loader.memory_stats().entries, 2);
        let hdr = |h: &[(&str, &str)]| -> Vec<(String, String)> {
            h.iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect()
        };
        assert!(
            loader
                .peek_memory_with_headers(url, &hdr(&a), None)
                .is_some()
        );
        assert!(
            loader
                .peek_memory_with_headers(url, &hdr(&b), None)
                .is_some()
        );
        assert!(loader.peek_memory(url, None).is_none());
    });
}

#[test]
fn same_headers_share_one_fetch_regardless_of_order_and_name_case() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(50));
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        let one = [("Authorization", "Bearer a"), ("X-Tenant", "t")];
        let two = [("x-tenant", "t"), ("authorization", "Bearer a")];
        let loads = vec![
            loader.load(req_h(url, &one)),
            loader.load(req_h(url, &two)),
            loader.load(req_h(url, &one)),
        ];
        let results = futures_join(loads.into_iter()).await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(loader.memory_stats().entries, 1);
    });
}

#[test]
fn concurrent_loads_with_different_headers_do_not_join() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::from_millis(50));
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        let loads = vec![
            loader.load(req_h(url, &[("Authorization", "Bearer a")])),
            loader.load(req_h(url, &[("Authorization", "Bearer b")])),
        ];
        let results = futures_join(loads.into_iter()).await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(fetcher.calls(), 2);
    });
}

#[test]
fn headerless_key_is_the_url_only_key() {
    run(async {
        let fetcher = FakeFetcher::new(Duration::ZERO);
        let loader = loader(&fetcher, 1 << 20);
        let url = "https://x/a.png";
        loader.load(req(url, None)).await.expect("load");
        assert!(loader.peek_memory(url, None).is_some());
        assert!(loader.peek_memory_with_headers(url, &[], None).is_some());
        loader.load(req(url, None)).await.expect("hit");
        assert_eq!(fetcher.calls(), 1);
    });
}

#[test]
fn configured_decode_limits_reject_an_oversized_image() {
    let fetcher = FakeFetcher::new(Duration::ZERO);
    let loader = ImageLoader::new(
        fetcher.clone(),
        LoaderConfig {
            decode_limits: DecodeLimits {
                max_width: 100,
                ..DecodeLimits::default()
            },
            disk: DiskCacheConfig {
                enabled: false,
                ..DiskCacheConfig::default()
            },
            ..LoaderConfig::default()
        },
    );
    run(async {
        // The 4000x3000 JPEG exceeds the 100 px width limit.
        let err = loader
            .load(req("https://x/a.jpg", None))
            .await
            .expect_err("over the limit");
        assert!(
            matches!(
                err,
                LoadError::Decode(DecodeError::TooLarge { width: 4000, .. })
            ),
            "{err:?}"
        );
        // The 8x8 PNG fits.
        loader
            .load(req("https://x/a.png", None))
            .await
            .expect("fits");
    });
}
