//! `ImageLoader` against a counting in-process fetcher.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use frust_network_image::{
    FetchFuture, FetchRequest, FetchedBytes, ImageFetcher, ImageLoader, LoadRequest, LoaderConfig,
    TargetPx,
};

struct FakeFetcher {
    calls: AtomicUsize,
    delay: Duration,
    png: Vec<u8>,
    jpeg: Vec<u8>,
}

impl FakeFetcher {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            delay,
            png: encode(8, 8, image::ImageFormat::Png),
            jpeg: encode(4000, 3000, image::ImageFormat::Jpeg),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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
        Box::pin(async move {
            tokio::time::sleep(delay).await;
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
