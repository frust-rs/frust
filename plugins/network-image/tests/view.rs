//! `network_image` headless: a real `RenderRoot`, a gated in-process fetcher,
//! and the UI-thread local queue pumped by hand each frame.

use std::future::poll_fn;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ImageData, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    Point, PointerButton, PointerEvent, PointerPhase, Rect, Size, View, Widget,
};
use frust::{Color, FrameTime, column, either};
use frust_core::RenderRoot;
use frust_network_image::{
    DiskCacheConfig, FetchError, FetchFuture, FetchRequest, FetchedBytes, ImageFetcher,
    ImageLoader, LoaderConfig, network_image,
};
use frust_reactive::ReactiveRuntime;

const PLACEHOLDER: f64 = 31.0;
const ERROR: f64 = 47.0;
const OTHER: f64 = 5.0;
const FRAME_NS: u64 = 16_000_000;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Opens once; every fetch waits on it.
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, Vec<Waker>)>,
}

impl Gate {
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

/// Serves an 8x8 PNG after the gate opens; the first `fail_first` fetches
/// answer 404 instead.
struct FakeFetcher {
    gate: Arc<Gate>,
    calls: AtomicUsize,
    completed: Arc<AtomicUsize>,
    fail_first: usize,
    png: Vec<u8>,
}

impl FakeFetcher {
    fn new(fail_first: usize) -> Arc<Self> {
        let img =
            image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 30, y as u8 * 30, 90]));
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode test image");
        Arc::new(Self {
            gate: Arc::new(Gate::default()),
            calls: AtomicUsize::new(0),
            completed: Arc::new(AtomicUsize::new(0)),
            fail_first,
            png: png.into_inner(),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }
}

impl ImageFetcher for FakeFetcher {
    fn fetch(&self, _request: FetchRequest) -> FetchFuture {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let fail = n < self.fail_first;
        let (gate, completed, bytes) =
            (self.gate.clone(), self.completed.clone(), self.png.clone());
        Box::pin(async move {
            gate.wait().await;
            completed.fetch_add(1, Ordering::SeqCst);
            if fail {
                return Err(FetchError::Status(404));
            }
            Ok(FetchedBytes {
                bytes,
                content_type: Some("image/png".into()),
            })
        })
    }
}

fn loader(fetcher: &Arc<FakeFetcher>) -> Arc<ImageLoader> {
    Arc::new(ImageLoader::new(
        fetcher.clone(),
        LoaderConfig {
            disk: DiskCacheConfig {
                enabled: false,
                ..DiskCacheConfig::default()
            },
            ..LoaderConfig::default()
        },
    ))
}

/// A square that paints one rect of its own size, to tell slots apart.
struct Marker(f64);
struct MarkerWidget(f64);

impl<S: 'static> View<S> for Marker {
    type Element = MarkerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MarkerWidget {
        MarkerWidget(self.0)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _el: &mut MarkerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for MarkerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.0, self.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
    }
}

/// What one frame painted.
#[derive(Default)]
struct Rec {
    rects: Vec<Size>,
    images: Vec<Rect>,
    layers: Vec<f32>,
    needs_frame: bool,
}

impl Rec {
    fn has_rect(&self, side: f64) -> bool {
        self.rects.iter().any(|s| *s == Size::new(side, side))
    }
}

impl PaintScene for Rec {
    fn fill_rect(&mut self, _origin: Point, size: Size, _color: Color) {
        self.rects.push(size);
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_image(&mut self, _data: &ImageData, dest: Rect) {
        self.images.push(dest);
    }
    fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
        self.layers.push(alpha);
    }
}

/// The shell's per-frame sequence: pump local tasks, rebuild under the root
/// owner, layout, paint at a advancing frame clock.
struct Harness<S: 'static, V: View<S>, F: FnMut(&mut S) -> V> {
    rt: &'static ReactiveRuntime,
    root: RenderRoot<S, V>,
    logic: F,
    state: S,
    now: u64,
}

impl<S: 'static, V: View<S>, F: FnMut(&mut S) -> V> Harness<S, V, F> {
    fn new(state: S, logic: F) -> Self {
        Self {
            rt: ReactiveRuntime::init(Arc::new(|| {})),
            root: RenderRoot::new(),
            logic,
            state,
            now: 1_000_000_000,
        }
    }

    fn frame_at(&mut self, now: u64) -> Rec {
        self.rt.pump_local();
        let (root, logic, state) = (&mut self.root, &mut self.logic, &mut self.state);
        self.rt.with_owner(|| root.rebuild(logic, state));
        self.root.layout(Size::new(400.0, 800.0));
        let mut rec = Rec::default();
        let outcome = self.root.paint(&mut rec, FrameTime::from_nanos(now));
        rec.needs_frame = outcome.needs_frame;
        self.now = now;
        rec
    }

    fn frame(&mut self) -> Rec {
        self.frame_at(self.now + FRAME_NS)
    }

    /// Frames until `cond` holds; panics after [`TIMEOUT`].
    fn frame_until(&mut self, what: &str, cond: impl Fn(&Rec) -> bool) -> Rec {
        let start = Instant::now();
        loop {
            let rec = self.frame();
            if cond(&rec) {
                return rec;
            }
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Pump the local queue (no frames) until `cond` holds.
    fn pump_until(&self, what: &str, cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            self.rt.pump_local();
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn tap(&mut self, at: Point) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let event = InputEvent::Pointer(PointerEvent {
                phase,
                position: at,
                button: PointerButton::Primary,
            });
            self.root.event(&mut self.state, &event);
        }
    }
}

#[test]
fn placeholder_first_then_image_fades_in() {
    let fetcher = FakeFetcher::new(0);
    let l = loader(&fetcher);
    let mut h = Harness::new((), move |_: &mut ()| {
        network_image("https://x/a.png")
            .loader(l.clone())
            .placeholder(Marker(PLACEHOLDER))
            .error(Marker(ERROR))
    });

    let first = h.frame();
    assert!(first.has_rect(PLACEHOLDER), "placeholder on frame 1");
    assert!(first.images.is_empty());

    h.pump_until("the fetch to start", || fetcher.calls() == 1);
    fetcher.gate.open();
    let landed = h.frame_until("the image", |r| !r.images.is_empty());
    assert!(!landed.has_rect(PLACEHOLDER), "placeholder replaced");
    assert!(
        landed.layers.iter().all(|a| *a < 1.0),
        "fade starts below full opacity"
    );
    assert!(landed.needs_frame, "the fade requests frames");

    let settled = h.frame_at(h.now + 300_000_000);
    assert_eq!(settled.layers, vec![1.0]);
    assert_eq!(settled.images.len(), 1);
    assert_eq!(fetcher.calls(), 1);
}

#[test]
fn unmount_while_loading_writes_nothing() {
    let fetcher = FakeFetcher::new(0);
    let l = loader(&fetcher);
    let probe = l.clone();
    let mut h = Harness::new(true, move |show: &mut bool| {
        let l = l.clone();
        either(
            *show,
            move || {
                network_image("https://x/gone.png")
                    .loader(l)
                    .placeholder(Marker(PLACEHOLDER))
            },
            || Marker(OTHER),
        )
    });

    assert!(h.frame().has_rect(PLACEHOLDER));
    h.pump_until("the fetch to start", || fetcher.calls() == 1);

    // Switch the arm hosting the view: teardown disposes the load's owner.
    h.state = false;
    assert!(h.frame().has_rect(OTHER));

    fetcher.gate.open();
    for _ in 0..50 {
        let rec = h.frame();
        assert!(rec.images.is_empty());
        std::thread::sleep(Duration::from_millis(2));
    }
    // The aborted load was never polled past the gate, so no result exists
    // for a coordinator to write, and nothing reached the cache.
    assert_eq!(fetcher.completed(), 0);
    assert_eq!(probe.memory_stats().entries, 0);
}

#[test]
fn eight_views_of_one_url_fetch_once() {
    let fetcher = FakeFetcher::new(0);
    fetcher.gate.open();
    let l = loader(&fetcher);
    let mut h = Harness::new((), move |_: &mut ()| {
        column().children(
            (0..8)
                .map(|_| network_image("https://x/shared.png").loader(l.clone()))
                .collect::<Vec<_>>(),
        )
    });
    h.frame_until("eight images", |r| r.images.len() == 8);
    assert_eq!(fetcher.calls(), 1);
}

#[test]
fn memory_hit_paints_on_first_frame_at_full_opacity() {
    let fetcher = FakeFetcher::new(0);
    fetcher.gate.open();
    let l = loader(&fetcher);

    let warm = l.clone();
    let mut first = Harness::new((), move |_: &mut ()| {
        network_image("https://x/hot.png").loader(warm.clone())
    });
    first.frame_until("the warm-up image", |r| !r.images.is_empty());
    assert_eq!(l.memory_stats().entries, 1);

    let mut second = Harness::new((), move |_: &mut ()| {
        network_image("https://x/hot.png")
            .loader(l.clone())
            .placeholder(Marker(PLACEHOLDER))
    });
    let frame1 = second.frame();
    assert_eq!(frame1.images.len(), 1, "image on frame 1");
    assert_eq!(frame1.layers, vec![1.0], "no fade on a memory hit");
    assert!(!frame1.has_rect(PLACEHOLDER));
    assert_eq!(fetcher.calls(), 1);
}

#[test]
fn failure_shows_error_slot_and_tap_retries() {
    let fetcher = FakeFetcher::new(1);
    fetcher.gate.open();
    let l = loader(&fetcher);
    let mut h = Harness::new((), move |_: &mut ()| {
        network_image("https://x/flaky.png")
            .loader(l.clone())
            .placeholder(Marker(PLACEHOLDER))
            .error(Marker(ERROR))
            .retry_on_tap(true)
            .fade(Duration::ZERO)
    });
    h.frame_until("the error slot", |r| r.has_rect(ERROR));
    assert_eq!(fetcher.calls(), 1);

    h.tap(Point::new(ERROR / 2.0, ERROR / 2.0));
    let rec = h.frame_until("the retried image", |r| !r.images.is_empty());
    assert_eq!(rec.layers, vec![1.0], "fade disabled");
    assert_eq!(fetcher.calls(), 2);
}

#[test]
fn no_loader_renders_error_slot() {
    let mut h = Harness::new((), |_: &mut ()| {
        network_image("https://x/none.png")
            .placeholder(Marker(PLACEHOLDER))
            .error(Marker(ERROR))
    });
    let rec = h.frame();
    assert!(rec.has_rect(ERROR));
    assert!(rec.images.is_empty());
}
