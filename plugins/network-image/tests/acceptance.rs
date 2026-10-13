//! End-to-end acceptance over the loopback server and the real `ReqwestFetcher`.
//! Every criterion, including the generated 4000x3000 JPEG and the PNGs of
//! (e), is served by the loopback server (no FakeFetcher is used here).
#![cfg(feature = "reqwest")]

mod support;

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ImageData, LayoutCtx, PaintCtx, PaintScene, Point, Rect,
    Size, View, Widget,
};
use frust::{Color, FrameTime, column, either};
use frust_core::RenderRoot;
use frust_network_image::{
    DiskCache, DiskCacheConfig, ImageLoader, LoadRequest, LoaderConfig, ReqwestFetcher, TargetPx,
    network_image,
};
use frust_reactive::ReactiveRuntime;
use support::{Route, spawn};

const TIMEOUT: Duration = Duration::from_secs(10);

fn encode(w: u32, h: u32, rgb: [u8; 3], format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if format == image::ImageFormat::Jpeg {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        } else {
            image::Rgb(rgb)
        }
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .expect("encode test image");
    out.into_inner()
}

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    encode(w, h, rgb, image::ImageFormat::Png)
}

fn loader(config: LoaderConfig) -> Arc<ImageLoader> {
    Arc::new(ImageLoader::new(
        Arc::new(ReqwestFetcher::new().expect("fetcher")),
        config,
    ))
}

fn no_disk() -> LoaderConfig {
    LoaderConfig {
        disk: DiskCacheConfig {
            enabled: false,
            ..DiskCacheConfig::default()
        },
        ..LoaderConfig::default()
    }
}

fn req(url: String, target: Option<TargetPx>) -> LoadRequest {
    LoadRequest {
        url,
        headers: Vec::new(),
        target,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "frust-ni-acceptance-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(Default)]
struct Rec {
    images: usize,
    other: usize,
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, _origin: Point, size: Size, _color: Color) {
        if size == Size::new(5.0, 5.0) {
            self.other += 1;
        }
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_image(&mut self, _data: &ImageData, _dest: Rect) {
        self.images += 1;
    }
    fn push_layer(&mut self, _origin: Point, _size: Size, _alpha: f32) {}
}

struct Marker;
struct MarkerWidget;

impl<S: 'static> View<S> for Marker {
    type Element = MarkerWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MarkerWidget {
        MarkerWidget
    }
    fn rebuild(&self, _p: &Self, _e: &mut MarkerWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for MarkerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(5.0, 5.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
    }
}

fn frame<S: 'static, V: View<S>>(
    rt: &ReactiveRuntime,
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    now: u64,
) -> Rec {
    rt.pump_local();
    rt.with_owner(|| root.rebuild(logic, state));
    root.layout(Size::new(400.0, 800.0));
    let mut rec = Rec::default();
    root.paint(&mut rec, FrameTime::from_nanos(now));
    rec
}

/// Init the reactive runtime from sync context (it owns a pool that must not be
/// created inside an async context), then drive `fut` on a tokio runtime.
fn run<F: std::future::Future>(fut: F) -> F::Output {
    init_runtime();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

fn init_runtime() -> &'static ReactiveRuntime {
    ReactiveRuntime::init(Arc::new(|| {}))
}

/// Criterion (a): eight views of one URL cost the server exactly one request.
#[test]
fn a_eight_views_one_fetch() {
    run(async {
        let rt = init_runtime();
        let server = spawn(vec![(
            "/shared.png",
            Route::Ok {
                content_type: "image/png",
                body: png(8, 8, [10, 20, 30]),
            },
        )])
        .await;
        let l = loader(no_disk());
        let url = server.url("/shared.png");
        let mut logic = move |_: &mut ()| {
            column().children(
                (0..8)
                    .map(|_| network_image(url.clone()).loader(l.clone()))
                    .collect::<Vec<_>>(),
            )
        };
        let mut root = RenderRoot::new();
        let mut state = ();
        let start = Instant::now();
        let mut now = 1_000_000_000;
        loop {
            now += 16_000_000;
            let rec = frame(rt, &mut root, &mut logic, &mut state, now);
            if rec.images == 8 {
                break;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for 8 images");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(server.request_count(), 1);
    });
}

/// Criterion (b): unmounting during a 500 ms fetch writes no state and does not
/// panic; the image never paints and nothing reaches the memory cache.
#[test]
fn b_unmount_during_fetch_writes_nothing() {
    run(async {
        let rt = init_runtime();
        // A 5-byte body trickled at 100 ms per byte keeps the fetch open for 500 ms.
        let server = spawn(vec![(
            "/slow.png",
            Route::Slow {
                body: vec![1; 5],
                delay: Duration::from_millis(100),
            },
        )])
        .await;
        let l = loader(no_disk());
        let probe = l.clone();
        let url = server.url("/slow.png");
        let mut logic = move |show: &mut bool| {
            let (l, url) = (l.clone(), url.clone());
            either(*show, move || network_image(url).loader(l), || Marker)
        };
        let mut root = RenderRoot::new();
        let mut state = true;
        let mut now = 1_000_000_000;
        let start = Instant::now();
        while server.request_count() == 0 {
            now += 16_000_000;
            frame(rt, &mut root, &mut logic, &mut state, now);
            assert!(start.elapsed() < TIMEOUT, "fetch never started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        state = false;
        now += 16_000_000;
        assert_eq!(frame(rt, &mut root, &mut logic, &mut state, now).other, 1);
        let until = Instant::now() + Duration::from_millis(800);
        while Instant::now() < until {
            now += 16_000_000;
            let rec = frame(rt, &mut root, &mut logic, &mut state, now);
            assert_eq!(rec.images, 0);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(probe.memory_stats().entries, 0);
    });
}

/// Criterion (c): a 4000x3000 JPEG with a 512x512 target decodes to at most
/// 512 on the long edge and memory stats show the small entry.
#[test]
fn c_large_jpeg_lands_at_target_px() {
    run(async {
        init_runtime();
        let server = spawn(vec![(
            "/big.jpg",
            Route::Ok {
                content_type: "image/jpeg",
                body: encode(4000, 3000, [0; 3], image::ImageFormat::Jpeg),
            },
        )])
        .await;
        let l = loader(no_disk());
        let target = Some(TargetPx {
            width: 512,
            height: 512,
        });
        let src = l
            .load(req(server.url("/big.jpg"), target))
            .await
            .expect("load");
        let size = src.natural_size();
        assert!(size.width.max(size.height) <= 512.0, "{size:?}");
        let stats = l.memory_stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(
            stats.used_bytes,
            size.width as usize * size.height as usize * 4
        );
        assert!(stats.used_bytes <= 512 * 512 * 4);
        assert!(l.peek_memory(&server.url("/big.jpg"), target).is_some());
    });
}

/// Criterion (d): a configured Authorization header reaches the server.
#[test]
fn d_authorization_header_reaches_server() {
    run(async {
        let rt = init_runtime();
        let server = spawn(vec![(
            "/auth.png",
            Route::Ok {
                content_type: "image/png",
                body: png(8, 8, [1, 2, 3]),
            },
        )])
        .await;
        let l = loader(no_disk());
        let url = server.url("/auth.png");
        let mut logic = move |_: &mut ()| {
            network_image(url.clone())
                .loader(l.clone())
                .headers(vec![("Authorization".into(), "Bearer t".into())])
        };
        let mut root = RenderRoot::new();
        let mut state = ();
        let start = Instant::now();
        let mut now = 1_000_000_000;
        loop {
            now += 16_000_000;
            if frame(rt, &mut root, &mut logic, &mut state, now).images == 1 {
                break;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let log = server.requests.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].header("authorization"), Some("Bearer t"));
    });
}

/// Criterion (e): with a 2-image memory budget and a 3-file disk cap, five
/// loads stay under both, and a cold second loader serves the newest three
/// from disk with zero new requests.
#[test]
fn e_budgets_hold_and_cold_loader_serves_from_disk() {
    run(async {
        init_runtime();
        let bodies: Vec<Vec<u8>> = (0..5u8).map(|i| png(16, 16, [i * 40, 9, 9])).collect();
        let max = bodies.iter().map(Vec::len).max().unwrap();
        let min = bodies.iter().map(Vec::len).min().unwrap();
        // Any three fit under the cap, any four do not.
        assert!(4 * min > 3 * max, "near-equal bodies");
        let paths = ["/0.png", "/1.png", "/2.png", "/3.png", "/4.png"];
        let server = spawn(
            paths
                .iter()
                .zip(bodies)
                .map(|(p, body)| {
                    (
                        *p,
                        Route::Ok {
                            content_type: "image/png",
                            body,
                        },
                    )
                })
                .collect(),
        )
        .await;
        let dir = temp_dir("e");
        let cap = (3 * max) as u64;
        let cfg = |budget| LoaderConfig {
            memory_budget_bytes: budget,
            disk: DiskCacheConfig {
                enabled: true,
                cap_bytes: cap,
                dir_override: Some(dir.clone()),
            },
            ..LoaderConfig::default()
        };
        let decoded = 16 * 16 * 4;
        {
            let l = loader(cfg(2 * decoded));
            for p in paths {
                l.load(req(server.url(p), None)).await.expect("load");
                let s = l.memory_stats();
                assert!(s.entries <= 2 && s.used_bytes <= 2 * decoded, "{s:?}");
            }
            assert_eq!(l.memory_stats().entries, 2);
        }
        assert_eq!(server.request_count(), 5);
        let disk = DiskCache::open(dir.clone(), cap).expect("open disk");
        let stats = disk.stats();
        assert_eq!(stats.entries, 3);
        assert!(stats.total_bytes <= cap);
        drop(disk);

        let cold = loader(cfg(2 * decoded));
        for p in &paths[2..] {
            cold.load(req(server.url(p), None)).await.expect("disk hit");
        }
        assert_eq!(server.request_count(), 5, "newest three served from disk");
        let _ = std::fs::remove_dir_all(&dir);
    });
}
