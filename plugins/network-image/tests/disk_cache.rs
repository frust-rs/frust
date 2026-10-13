//! `DiskCache` behaviour and the loader's disk read-through/write-through.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use frust_network_image::{
    DiskCache, DiskCacheConfig, FetchFuture, FetchRequest, FetchedBytes, ImageFetcher, ImageLoader,
    LoadRequest, LoaderConfig,
};

/// A unique scratch dir under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("frust-ni-disk-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn put_get_round_trip() {
    let tmp = TempDir::new("rt");
    let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
    assert!(cache.get("https://x/a").is_none());
    cache.put("https://x/a", b"hello").unwrap();
    assert_eq!(cache.get("https://x/a").unwrap(), b"hello");
    let stats = cache.stats();
    assert_eq!((stats.entries, stats.total_bytes), (1, 5));
    cache.clear().unwrap();
    assert!(cache.get("https://x/a").is_none());
    assert_eq!(cache.stats().entries, 0);
}

#[test]
fn cap_eviction_keeps_newest() {
    let tmp = TempDir::new("cap");
    let cache = DiskCache::open(tmp.path(), 25).unwrap();
    for name in ["a", "b", "c"] {
        cache.put(&format!("https://x/{name}"), &[7u8; 10]).unwrap();
    }
    assert!(cache.get("https://x/a").is_none(), "oldest evicted");
    assert!(cache.get("https://x/b").is_some());
    assert!(cache.get("https://x/c").is_some());
    assert!(cache.stats().total_bytes <= 25);
    // Oversize bodies are not stored.
    cache.put("https://x/big", &[1u8; 26]).unwrap();
    assert!(cache.get("https://x/big").is_none());
}

#[test]
fn recent_get_protects_from_eviction() {
    let tmp = TempDir::new("lru");
    let cache = DiskCache::open(tmp.path(), 25).unwrap();
    cache.put("https://x/a", &[1u8; 10]).unwrap();
    cache.put("https://x/b", &[2u8; 10]).unwrap();
    assert!(cache.get("https://x/a").is_some());
    cache.put("https://x/c", &[3u8; 10]).unwrap();
    assert!(cache.get("https://x/a").is_some());
    assert!(cache.get("https://x/b").is_none());
}

#[test]
fn index_survives_drop_and_reopen() {
    let tmp = TempDir::new("reopen");
    {
        let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
        cache.put("https://x/a", b"one").unwrap();
        cache.put("https://x/b", b"two").unwrap();
    }
    let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
    assert_eq!(cache.get("https://x/a").unwrap(), b"one");
    assert_eq!(cache.get("https://x/b").unwrap(), b"two");
    assert_eq!(cache.stats().entries, 2);
}

#[test]
fn corrupt_index_is_rebuilt_from_files() {
    let tmp = TempDir::new("corrupt");
    {
        let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
        cache.put("https://x/a", b"payload").unwrap();
    }
    std::fs::write(tmp.path().join("index.json"), b"{ not json").unwrap();
    let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
    assert_eq!(cache.get("https://x/a").unwrap(), b"payload");
    assert_eq!(cache.stats().entries, 1);
}

#[test]
fn indexed_but_missing_file_is_dropped() {
    let tmp = TempDir::new("missing");
    {
        let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
        cache.put("https://x/a", b"payload").unwrap();
    }
    for f in std::fs::read_dir(tmp.path()).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_some_and(|e| e == "img") {
            std::fs::remove_file(p).unwrap();
        }
    }
    let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
    assert_eq!(cache.stats().entries, 0);
    assert!(cache.get("https://x/a").is_none());
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn directory_never_contains_the_url() {
    let tmp = TempDir::new("grep");
    let token = "UNIQUE-TOKEN-9f3a7c1e";
    let url = format!("https://example.test/{token}/pic.png?sig={token}");
    {
        let cache = DiskCache::open(tmp.path(), 1 << 20).unwrap();
        cache.put(&url, b"bytes").unwrap();
        cache.get(&url);
    }
    let mut files = 0;
    for f in std::fs::read_dir(tmp.path()).unwrap() {
        let f = f.unwrap();
        let name = f.file_name().to_string_lossy().into_owned();
        assert!(!name.contains(token), "file name leaks the URL: {name}");
        let body = std::fs::read(f.path()).unwrap();
        assert!(!contains(&body, token.as_bytes()), "{name} leaks the URL");
        files += 1;
    }
    assert!(files >= 2, "expected data file and index");
}

struct CountingFetcher {
    calls: AtomicUsize,
    png: Vec<u8>,
}

impl ImageFetcher for CountingFetcher {
    fn fetch(&self, _request: FetchRequest) -> FetchFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let bytes = self.png.clone();
        Box::pin(async move {
            Ok(FetchedBytes {
                bytes,
                content_type: None,
            })
        })
    }
}

fn counting() -> Arc<CountingFetcher> {
    let img = image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8, y as u8, 90]));
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    Arc::new(CountingFetcher {
        calls: AtomicUsize::new(0),
        png: out.into_inner(),
    })
}

fn loader(fetcher: &Arc<CountingFetcher>, dir: PathBuf) -> ImageLoader {
    ImageLoader::new(
        fetcher.clone(),
        LoaderConfig {
            disk: DiskCacheConfig {
                dir_override: Some(dir),
                ..DiskCacheConfig::default()
            },
            ..LoaderConfig::default()
        },
    )
}

fn run<F: std::future::Future>(fut: F) -> F::Output {
    frust_reactive::ReactiveRuntime::init(Arc::new(|| {}));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

#[test]
fn second_loader_serves_from_disk_without_fetching() {
    let tmp = TempDir::new("loader");
    let url = "https://x/disk.png";
    let req = || LoadRequest {
        url: url.to_string(),
        headers: Vec::new(),
        target: None,
    };
    let first = counting();
    run(async {
        let loader = loader(&first, tmp.path());
        loader.load(req()).await.expect("first load");
    });
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);

    let second = counting();
    run(async {
        let loader = loader(&second, tmp.path());
        loader.load(req()).await.expect("disk-served load");
    });
    assert_eq!(second.calls.load(Ordering::SeqCst), 0);
}
