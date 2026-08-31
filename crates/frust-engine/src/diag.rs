//! The engine's frame-diagnostics vocabulary: which GPU spans a frame is split
//! into, and the label every GPU object it creates carries.
//!
//! # Spans
//!
//! [`EngineSpan`] names the four parts of an engine frame worth timing
//! separately, and is the *only* place that naming lives — the substrate ring
//! ([`frust_gpu::diag::TimestampRing`]) knows how many spans there are and
//! which index each pass was charged to, never what any of them means. A host
//! reads the frame's spans back out in this order and reports them as
//! `gpu_prepass_us`/`gpu_main_us`/`gpu_composite_us`/`gpu_blit_us`.
//!
//! Several passes may be charged to one span, which is the ordinary case:
//! [`EngineSpan::Main`] covers the clear, the opaque strip pass and every
//! surface round's alpha strips, and [`EngineSpan::Composite`] covers every
//! layer page round and every filter pass. Their durations add up.
//!
//! # Labels
//!
//! Every texture, buffer, pipeline and pass this crate creates is labelled
//! `frust-engine <kind>`, so a GPU capture (Xcode, RenderDoc,
//! `wgpu`'s own validation output) names the engine's own objects rather than
//! showing an anonymous wall of resources next to the host's. [`LABEL_PREFIX`]
//! is that prefix and [`labels_are_prefixed`] is the guard that keeps a new
//! resource from slipping in unlabelled.
//!
//! The prefix is inert where a platform strips debug information — the Android
//! emulator's instance flags drop `DEBUG` (see `frust_gpu::context`) — which
//! costs nothing: the label is a static string either way.

use frust_gpu::diag::TimestampRing;

/// The prefix every GPU object this crate creates carries.
pub const LABEL_PREFIX: &str = "frust-engine ";

/// The label the engine's own timestamp ring names its query sets and staging
/// buffers with.
pub const TIMESTAMP_RING_LABEL: &str = "frust-engine gpu timestamps";

/// One of the four GPU spans an engine frame is split into.
///
/// The discriminants are the span indices a [`TimestampRing`] is driven with,
/// and the order is the order a frame line reports them in — both are wire
/// contract, so a variant is appended, never re-ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EngineSpan {
    /// Work recorded strictly ahead of the frame's own passes: the glyph-atlas
    /// replay that draws newly cached glyphs into their array layers.
    Prepass = 0,
    /// The frame's own surface passes — the clear, the opaque depth-writing
    /// strip pass, every surface round's alpha strips, and the hole punch.
    Main = 1,
    /// Everything drawn into an off-screen page rather than the surface: each
    /// isolated layer's page round and each pass of a filter's sequence.
    Composite = 2,
    /// The host's own present-side conversion — the un-premultiplying pass a
    /// straight-alpha swapchain needs, or a blit into it.
    Blit = 3,
}

impl EngineSpan {
    /// Every span, in reporting order.
    pub const ALL: [EngineSpan; 4] = [
        EngineSpan::Prepass,
        EngineSpan::Main,
        EngineSpan::Composite,
        EngineSpan::Blit,
    ];

    /// How many spans a frame is split into — what a caller sizes a
    /// [`TimestampRing`] with.
    pub const COUNT: usize = EngineSpan::ALL.len();

    /// This span's index in a frame's reading.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// This span's stable name, as a frame line's field spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            EngineSpan::Prepass => "prepass",
            EngineSpan::Main => "main",
            EngineSpan::Composite => "composite",
            EngineSpan::Blit => "blit",
        }
    }
}

/// The per-frame timestamp sink the renderer records its pass boundaries
/// through — a borrow of the host's [`TimestampRing`], or nothing at all.
///
/// Copy and one pointer wide, so it is threaded through the frame walk by
/// value: every pass site asks it for its own `timestamp_writes` and takes
/// `None` for an answer without a branch of its own. An inert sink
/// ([`FrameTimestamps::default`]) is what an ordinary
/// [`crate::EngineRenderer::encode`] passes, so the untimed frame path is the
/// timed one with every answer `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTimestamps<'a> {
    ring: Option<&'a TimestampRing>,
}

impl<'a> FrameTimestamps<'a> {
    /// A sink that times nothing.
    #[must_use]
    pub const fn inert() -> Self {
        Self { ring: None }
    }

    /// A sink writing into `ring`, or an inert one when the ring is itself
    /// inert — a host holding a ring for a device without `TIMESTAMP_QUERY`
    /// records exactly the passes it always did.
    #[must_use]
    pub fn new(ring: &'a TimestampRing) -> Self {
        Self {
            ring: ring.is_active().then_some(ring),
        }
    }

    /// Whether this sink writes anything at all.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.ring.is_some()
    }

    /// The `timestamp_writes` for one pass belonging to `span`, or `None` for
    /// an untimed pass. Every call takes a fresh query pair, so a span made of
    /// several passes sums rather than overwriting itself.
    #[must_use]
    pub fn writes(&self, span: EngineSpan) -> Option<wgpu::RenderPassTimestampWrites<'a>> {
        self.ring?.pass_writes(span.index())
    }
}

/// Whether `source` labels every GPU object it creates `frust-engine <kind>`.
///
/// Answers `Err(offender)` naming the first label that does not, so the guard
/// below reports the literal rather than only its file. Deliberately a literal
/// scan rather than a parser: it recognises the two shapes this crate writes a
/// label in — `label: Some("…")` at a `wgpu` descriptor and `label: "…"` at
/// one of the renderer's own pass plans — which is what a new resource is
/// added with.
pub fn labels_are_prefixed(source: &str) -> Result<(), String> {
    for line in source.lines() {
        let trimmed = line.trim_start();
        // A prose mention in a comment is not a label site.
        if trimmed.starts_with("//") {
            continue;
        }
        for opener in ["label: Some(\"", "label: \""] {
            let Some(at) = trimmed.find(opener) else {
                continue;
            };
            let rest = &trimmed[at + opener.len()..];
            let Some(end) = rest.find('"') else {
                continue;
            };
            let label = &rest[..end];
            if !label.starts_with(LABEL_PREFIX) {
                return Err(label.to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn span_indices_are_the_reporting_order() {
        assert_eq!(EngineSpan::COUNT, 4);
        for (index, span) in EngineSpan::ALL.into_iter().enumerate() {
            assert_eq!(span.index(), index, "{span:?} must keep its wire index");
        }
        assert_eq!(EngineSpan::Prepass.index(), 0);
        assert_eq!(EngineSpan::Blit.index(), 3);
    }

    #[test]
    fn span_names_are_the_frame_lines_own_field_names() {
        let names: Vec<&str> = EngineSpan::ALL.into_iter().map(EngineSpan::name).collect();
        assert_eq!(names, ["prepass", "main", "composite", "blit"]);
    }

    #[test]
    fn an_inert_sink_times_no_pass() {
        let sink = FrameTimestamps::inert();
        assert!(!sink.is_active());
        for span in EngineSpan::ALL {
            assert!(sink.writes(span).is_none());
        }
        assert!(!FrameTimestamps::default().is_active());
    }

    #[test]
    fn a_sink_over_an_inert_ring_is_itself_inert() {
        let ring = TimestampRing::inert(EngineSpan::COUNT);
        let sink = FrameTimestamps::new(&ring);
        assert!(!sink.is_active());
        assert!(sink.writes(EngineSpan::Main).is_none());
    }

    #[test]
    fn the_label_scan_accepts_both_shapes_this_crate_writes() {
        assert!(labels_are_prefixed("    label: Some(\"frust-engine clear\"),").is_ok());
        assert!(labels_are_prefixed("    label: \"frust-engine hole punch\",").is_ok());
        assert_eq!(
            labels_are_prefixed("    label: Some(\"scratch\"),"),
            Err("scratch".to_string())
        );
        assert_eq!(
            labels_are_prefixed("    label: \"page\","),
            Err("page".to_string())
        );
    }

    #[test]
    fn the_label_scan_ignores_a_comment_mentioning_one() {
        assert!(labels_are_prefixed("    // label: Some(\"anything\") is prose here").is_ok());
    }

    /// Blocks on `future` by polling it to completion — `wgpu`'s native
    /// adapter and device requests resolve without an executor driving them,
    /// and this crate has no async runtime of its own.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    #[ignore = "needs a real GPU adapter offering TIMESTAMP_QUERY; run with \
                `cargo test -p frust-engine -- --ignored` (pin the adapter on a \
                multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
    fn a_real_frame_reports_plausible_per_span_gpu_time() {
        use crate::{EngineRenderer, EngineTarget, OutputAlpha};
        use frust_gpu::{HeadlessTarget, TierCaps};
        use frust_scene::{Scene, SceneBuilder};
        use kurbo::{Affine, Rect};
        use peniko::Brush;
        use peniko::color::palette::css;
        use std::time::Duration;

        const SIZE: u32 = 512;
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

        block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            println!(
                "frust-engine gpu-timestamp adapter: {:?}",
                adapter.get_info()
            );
            let caps = TierCaps::probe(&adapter);
            if !caps.has_timestamp_query {
                println!("adapter offers no TIMESTAMP_QUERY — the inert path is the whole story");
                return;
            }
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine gpu-timestamp device"),
                    // Exactly what `frust_gpu::context`'s `required_features`
                    // asks for under a `perf-trace` build on this adapter.
                    required_features: wgpu::Features::TIMESTAMP_QUERY,
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");

            let mut renderer = EngineRenderer::new(&device, &caps, FORMAT, None)
                .expect("engine renderer for a headless target");
            renderer.finish_warm_up(&device);
            let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
            let mut ring = frust_gpu::diag::TimestampRing::new(
                &device,
                &queue,
                EngineSpan::COUNT,
                TIMESTAMP_RING_LABEL,
            );
            assert!(
                ring.is_active(),
                "a TIMESTAMP_QUERY device must arm the ring"
            );

            // Surface draws (Main) plus a sub-unity layer, which the scheduler
            // renders into its own page and composites back (Composite). Big
            // rectangles, so both spans are comfortably above the clock's
            // resolution rather than sitting in its noise.
            let mut scene = Scene::new();
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(
                Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
                Brush::Solid(css::REBECCA_PURPLE),
            );
            builder.fill_rect(
                Rect::new(16.0, 16.0, 480.0, 480.0),
                Brush::Solid(css::CORNFLOWER_BLUE.with_alpha(0.5)),
            );
            builder.push_layer(Rect::new(32.0, 32.0, 464.0, 464.0), 0.5);
            builder.fill_rect(
                Rect::new(48.0, 48.0, 448.0, 448.0),
                Brush::Solid(css::ORANGE),
            );
            builder.pop_layer();

            let mut readings = 0_u64;
            for frame in 0..(frust_gpu::diag::RING_FRAMES * 3) {
                ring.begin_frame(&device);
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frust-engine gpu-timestamp frame"),
                });
                renderer
                    .encode_traced(
                        &device,
                        &queue,
                        &mut encoder,
                        &scene,
                        EngineTarget {
                            view: target.view(),
                            format: FORMAT,
                            width: SIZE,
                            height: SIZE,
                            depth: None,
                            output: OutputAlpha::Premultiplied,
                        },
                        css::BLACK,
                        Affine::IDENTITY,
                        FrameTimestamps::new(&ring),
                    )
                    .expect("the engine must serve this frame");
                ring.resolve(&mut encoder);
                queue.submit([encoder.finish()]);
                ring.end_frame();
                renderer.end_frame(&queue);
                // The frame path never waits on a map; a test may, and does.
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                if ring.harvested() > readings {
                    readings = ring.harvested();
                    let reading = ring.latest().expect("a harvest produces a reading");
                    println!(
                        "frame {frame}: main={:?} composite={:?} total={:?}",
                        reading.span(EngineSpan::Main.index()),
                        reading.span(EngineSpan::Composite.index()),
                        reading.total()
                    );
                }
            }

            let reading = ring
                .latest()
                .expect("at least one reading must have landed");
            assert_eq!(reading.len(), EngineSpan::COUNT);
            assert!(
                reading.span(EngineSpan::Main.index()) > Duration::ZERO,
                "the frame's own surface passes drew: {reading:?}"
            );
            assert!(
                reading.span(EngineSpan::Composite.index()) > Duration::ZERO,
                "the sub-unity layer rendered into a page: {reading:?}"
            );
            // The two spans this build records; the other two need a pass
            // descriptor seam their own modules do not carry yet.
            assert_eq!(
                reading.total(),
                reading.span(EngineSpan::Main.index())
                    + reading.span(EngineSpan::Composite.index())
                    + reading.span(EngineSpan::Prepass.index())
                    + reading.span(EngineSpan::Blit.index())
            );
            assert!(
                reading.total() < Duration::from_millis(100),
                "one 512x512 frame cannot plausibly take {:?}",
                reading.total()
            );
        });
    }

    /// Every `.rs` file under `dir`, recursively.
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn every_gpu_object_this_crate_creates_is_labelled_frust_engine() {
        // The mechanical half of the labelling rule: a texture, buffer,
        // pipeline or pass added without the prefix fails here rather than
        // showing up anonymous in someone's GPU capture months later.
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        files.sort();
        assert!(!files.is_empty(), "{} has no sources", src.display());

        let mut failures = Vec::new();
        for path in files {
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Err(label) = labels_are_prefixed(&contents) {
                failures.push(format!("{}: `{label}`", path.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "every GPU object must be labelled `{LABEL_PREFIX}<kind>`: {failures:?}"
        );
    }
}
