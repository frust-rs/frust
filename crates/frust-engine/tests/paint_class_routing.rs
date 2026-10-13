//! Paint-class routing: how a compiled frame's strip segments are cut into
//! runs, each drawn with one class's specialisation of the strip program, and
//! that drawing them that way changes no pixel.
//!
//! The host-only half rebuilds a compiled frame's instances the way the
//! renderer emits them — one alpha span per strip pair plus its gap fill,
//! each carrying the draw's packed paint descriptor — and holds
//! [`split_class_runs`] to the runs that frame should produce: the expected
//! class per run, the threshold merge, and the kill switch's single Uber run.
//!
//! The GPU half (`#[ignore]`d, pinned runner as every engine device suite) is
//! the correctness bar the specialisation was admitted on: every scene below,
//! rendered through class runs at several thresholds, reads back
//! byte-identical to the same scene drawn entirely through the Uber program
//! ([`EngineRenderer::set_uber_program`], the programmatic counterpart of
//! `FRUST_ENGINE_UBER_PROGRAM`).
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 \
//!   cargo test -p frust-engine --test paint_class_routing -- --ignored
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_engine::gpu::strips::{
    ClassRun, PaintClass, PaintType, RUN_SPLIT_MIN, pack_paint_descriptor, split_class_runs,
};
use frust_engine::{CompiledFrame, EngineRenderer, EngineTarget, GpuStrip, OutputAlpha, StripDraw};
use frust_engine::{SceneCompiler, config};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Circle, Point, Rect, Shape};
use peniko::color::DynamicColor;
use peniko::{Blob, Brush, Color, ColorStop, Gradient, ImageAlphaType, ImageData, ImageFormat};
use vello_common::encode::{EncodedKind, EncodedPaint};
use vello_common::paint::Paint;

/// Target extent every case compiles and renders at.
const SIZE: u32 = 256;

/// The read-back format: R, G, B, A in byte order, no swizzle.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The paint descriptor the renderer packs for `paint` — the input
/// [`PaintClass::of_flag`] reads — or `None` for an indexed paint the frame
/// does not carry.
fn descriptor(frame: &CompiledFrame, paint: &Paint) -> Option<u32> {
    let paint_type = match paint {
        Paint::Solid(_) => PaintType::Solid,
        Paint::Indexed(indexed) => match frame.encoded_paints.get(indexed.index())? {
            EncodedPaint::Gradient(gradient) => match gradient.kind {
                EncodedKind::Linear(_) => PaintType::LinearGradient,
                EncodedKind::Radial(_) => PaintType::RadialGradient,
                EncodedKind::Sweep(_) => PaintType::SweepGradient,
            },
            EncodedPaint::Image(_) | EncodedPaint::ExternalTexture(_) => PaintType::Image,
            EncodedPaint::BlurredRoundedRect(_) => PaintType::BlurredRoundedRect,
        },
    };
    // The texel index is irrelevant to the class; any in-range value will do.
    Some(pack_paint_descriptor(paint_type, 1))
}

/// The surface instances `frame` emits, in painter order, exactly as the
/// renderer's planning walk builds them for a depth-free surface round: per
/// strip pair, the alpha-sampled span and the gap fill after it.
fn emitted(frame: &CompiledFrame) -> Vec<GpuStrip> {
    let strips = frame.strip_buf();
    let mut out = Vec::new();
    for draw in frame.draws() {
        let Some(paint) = descriptor(frame, &draw.paint) else {
            continue;
        };
        let values = StripDraw {
            payload: 0,
            paint,
            depth_index: draw.depth,
        };
        let Some(run) = strips.get(draw.strip_range.clone()) else {
            continue;
        };
        for pair in run.windows(2) {
            let span = GpuStrip::from_strip_pair(&pair[0], &pair[1], values);
            if span.width > 0 {
                out.push(span);
            }
            if let Some(gap) = GpuStrip::gap_fill(&pair[0], &pair[1], values) {
                out.push(gap);
            }
        }
    }
    out
}

fn compile(scene: &Scene) -> CompiledFrame {
    let size = u16::try_from(SIZE).expect("the test extent fits the device grid");
    SceneCompiler::new(size, size)
        .compile(scene, Affine::IDENTITY, (size, size))
        .expect("an in-range scene compiles")
}

fn split(instances: &[GpuStrip], min_run: u32, uber_only: bool) -> Vec<ClassRun> {
    let mut out = Vec::new();
    split_class_runs(instances, min_run, uber_only, &mut out);
    out
}

/// A two-stop gradient from `a` to `b`.
fn stops(gradient: Gradient, a: Color, b: Color) -> Brush {
    Brush::Gradient(gradient.with_stops([
        ColorStop {
            offset: 0.0,
            color: DynamicColor::from_alpha_color(a),
        },
        ColorStop {
            offset: 1.0,
            color: DynamicColor::from_alpha_color(b),
        },
    ]))
}

/// A radial-gradient bubble of `radius` at `center` — S1's dominant draw.
fn bubble(builder: &mut SceneBuilder<'_>, center: Point, radius: f64, base: Color) {
    let fill = Gradient::new_radial(center, radius as f32);
    builder.fill_path(
        Circle::new(center, radius).to_path(0.1),
        stops(fill, base.with_alpha(0.9), base.with_alpha(0.4)),
    );
}

/// A 16x16 opaque checkerboard, straight-alpha RGBA8.
fn checkerboard(seed: u8) -> ImageData {
    let mut data = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16_u32 {
        for x in 0..16_u32 {
            if ((x / 2) + (y / 2)).is_multiple_of(2) {
                data.extend_from_slice(&[seed, 40, 200, 255]);
            } else {
                data.extend_from_slice(&[240, seed, 30, 180]);
            }
        }
    }
    ImageData {
        data: Blob::new(Arc::new(data)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: 16,
        height: 16,
    }
}

// --- Host: the runs a compiled frame splits into -------------------------

/// A big gradient bubble, a tiny solid mark, another bubble: the mark folds
/// into the bubble run before it, and the frame is ONE gradient run — the
/// solid program is in every class.
#[test]
fn a_compiled_frames_segment_splits_into_the_expected_runs() {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        bubble(
            &mut builder,
            Point::new(64.0, 64.0),
            48.0,
            Color::from_rgb8(40, 90, 200),
        );
        builder.fill_rect(
            Rect::new(60.0, 60.0, 68.0, 64.0),
            Brush::Solid(Color::WHITE),
        );
        bubble(
            &mut builder,
            Point::new(180.0, 160.0),
            56.0,
            Color::from_rgb8(200, 60, 90),
        );
    }
    let frame = compile(&scene);
    let instances = emitted(&frame);
    assert!(
        instances.len() > 2 * RUN_SPLIT_MIN as usize,
        "the bubbles are long runs"
    );

    let runs = split(&instances, RUN_SPLIT_MIN, false);
    assert_eq!(
        runs,
        vec![ClassRun {
            first: 0,
            count: instances.len() as u32,
            class: PaintClass::Gradient,
        }],
        "the solid mark folds into the gradient run around it"
    );

    // A long solid backdrop ahead of the bubbles stays a solid run of its own.
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_path(
            Circle::new(Point::new(128.0, 128.0), 120.0).to_path(0.1),
            Brush::Solid(Color::from_rgb8(18, 18, 24)),
        );
        bubble(
            &mut builder,
            Point::new(128.0, 128.0),
            60.0,
            Color::from_rgb8(40, 90, 200),
        );
    }
    let frame = compile(&scene);
    let instances = emitted(&frame);
    let runs = split(&instances, RUN_SPLIT_MIN, false);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0].class, PaintClass::SolidCoverage);
    assert_eq!(runs[1].class, PaintClass::Gradient);
    assert_eq!(runs[0].first, 0);
    assert_eq!(runs[1].first, runs[0].count);
    assert_eq!(runs[0].count + runs[1].count, instances.len() as u32);
}

/// Every class the strip program dispatches on, in one frame, in painter
/// order: each draw lands in a run of its own class (or of a class whose
/// program draws it), and the runs tile the frame.
#[test]
fn every_paint_class_in_one_frame_routes_to_its_own_program() {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_path(
            Circle::new(Point::new(128.0, 128.0), 126.0).to_path(0.1),
            Brush::Solid(Color::from_rgb8(10, 10, 10)),
        );
        builder.fill_path(
            Circle::new(Point::new(70.0, 70.0), 60.0).to_path(0.1),
            stops(
                Gradient::new_linear((10.0, 10.0), (130.0, 130.0)),
                Color::from_rgb8(255, 0, 0),
                Color::from_rgb8(0, 0, 255),
            ),
        );
        builder.fill_path(
            Circle::new(Point::new(190.0, 70.0), 56.0).to_path(0.1),
            stops(
                Gradient::new_sweep((190.0, 70.0), 0.0, 6.0),
                Color::from_rgb8(0, 255, 0),
                Color::from_rgb8(255, 255, 0),
            ),
        );
        builder.draw_blurred_rounded_rect(
            Rect::new(40.0, 150.0, 120.0, 230.0),
            8.0,
            4.0,
            Color::from_rgb8(255, 255, 255),
        );
    }
    let frame = compile(&scene);
    let instances = emitted(&frame);
    let runs = split(&instances, 0, false);

    let classes: Vec<_> = runs.iter().map(|run| run.class).collect();
    assert_eq!(
        classes,
        vec![
            PaintClass::SolidCoverage,
            PaintClass::Gradient,
            PaintClass::BlurredRect,
        ],
        "the two gradients are one run; the backdrop and the shadow each their own"
    );
    let mut next = 0;
    for run in &runs {
        assert_eq!(run.first, next);
        for instance in &instances[run.first as usize..(run.first + run.count) as usize] {
            assert!(run.class.draws(PaintClass::of_instance(instance)));
        }
        next += run.count;
    }
    assert_eq!(next, instances.len() as u32);
}

/// A run shorter than the threshold is merged into its neighbours and drawn
/// with the Uber program; the threshold decides how much of a frame keeps
/// its own class.
#[test]
fn the_threshold_merge_draws_short_runs_with_uber() {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        bubble(
            &mut builder,
            Point::new(64.0, 64.0),
            50.0,
            Color::from_rgb8(40, 90, 200),
        );
        // Two small shadows between the bubbles: a short blurred-rect run.
        builder.draw_blurred_rounded_rect(
            Rect::new(120.0, 120.0, 124.0, 124.0),
            1.0,
            0.5,
            Color::WHITE,
        );
        bubble(
            &mut builder,
            Point::new(180.0, 180.0),
            50.0,
            Color::from_rgb8(200, 60, 90),
        );
    }
    let frame = compile(&scene);
    let instances = emitted(&frame);

    let unmerged = split(&instances, 0, false);
    assert_eq!(
        unmerged.iter().map(|r| r.class).collect::<Vec<_>>(),
        vec![
            PaintClass::Gradient,
            PaintClass::BlurredRect,
            PaintClass::Gradient
        ]
    );
    let shadow = unmerged[1].count;
    assert!(
        shadow < RUN_SPLIT_MIN,
        "the shadow is a short run ({shadow})"
    );

    let merged = split(&instances, RUN_SPLIT_MIN, false);
    assert_eq!(
        merged.iter().map(|r| r.class).collect::<Vec<_>>(),
        vec![PaintClass::Gradient, PaintClass::Uber, PaintClass::Gradient],
        "the short shadow run is drawn with the whole program"
    );
    assert_eq!(merged[1].count, shadow);

    // A threshold above every run collapses the frame into one Uber run.
    let all = split(&instances, u32::MAX, false);
    assert_eq!(
        all,
        vec![ClassRun {
            first: 0,
            count: instances.len() as u32,
            class: PaintClass::Uber,
        }]
    );
}

/// `FRUST_ENGINE_UBER_PROGRAM`: one Uber run per segment, whatever it holds —
/// the unsplit segment exactly.
#[test]
fn the_kill_switch_yields_one_uber_run() {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 256.0, 256.0),
            Brush::Solid(Color::BLACK),
        );
        bubble(
            &mut builder,
            Point::new(128.0, 128.0),
            90.0,
            Color::from_rgb8(40, 90, 200),
        );
        builder.draw_blurred_rounded_rect(
            Rect::new(40.0, 40.0, 100.0, 100.0),
            6.0,
            3.0,
            Color::WHITE,
        );
    }
    let frame = compile(&scene);
    let instances = emitted(&frame);
    assert!(split(&instances, RUN_SPLIT_MIN, false).len() > 1);
    assert_eq!(
        split(&instances, RUN_SPLIT_MIN, true),
        vec![ClassRun {
            first: 0,
            count: instances.len() as u32,
            class: PaintClass::Uber,
        }]
    );
    // Unset in the gate, so a renderer starts with the classes on.
    assert!(!config::uber_program_forced());
}

// --- GPU: routed frames are pixel-identical to the Uber program ----------

/// Serializes every device-creating test in this binary (see
/// `encode_contract.rs`'s identical guard for why the NVIDIA driver needs it).
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

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

fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        println!("frust-engine paint-class adapter: {:?}", adapter.get_info());
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-engine paint-class device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// S1's shape at test scale: a solid backdrop, radial bubbles with solid
/// rims, small image "labels" on each, a linear and a sweep gradient, and
/// blurred shadows — every payload class interleaved in painter order.
fn mixed_scene() -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 256.0, 256.0),
            Brush::Solid(Color::from_rgb8(18, 18, 24)),
        );
        builder.fill_rect(
            Rect::new(8.5, 200.5, 120.5, 250.5),
            stops(
                Gradient::new_linear((8.0, 200.0), (120.0, 250.0)),
                Color::from_rgb8(255, 0, 0),
                Color::from_rgba8(0, 0, 255, 128),
            ),
        );
        builder.fill_path(
            Circle::new(Point::new(200.0, 210.0), 40.0).to_path(0.1),
            stops(
                Gradient::new_sweep((200.0, 210.0), 0.0, std::f32::consts::TAU),
                Color::from_rgb8(0, 255, 0),
                Color::from_rgba8(255, 255, 0, 64),
            ),
        );
        for i in 0..6_u8 {
            let x = 30.0 + f64::from(i % 3) * 80.0;
            let y = 40.0 + f64::from(i / 3) * 80.0;
            let center = Point::new(x, y);
            let base = Color::from_rgb8(60 + i * 30, 90 + i * 20, 200 - i * 15);
            builder.draw_blurred_rounded_rect(
                Rect::new(x - 30.0, y - 26.0, x + 34.0, y + 38.0),
                12.0,
                4.0,
                Color::from_rgba8(0, 0, 0, 160),
            );
            bubble(&mut builder, center, 32.0, base);
            builder.stroke_path(
                Circle::new(center, 32.0).to_path(0.1),
                2.0,
                Brush::Solid(base.with_alpha(0.6)),
            );
            builder.draw_image(
                &checkerboard(i * 40),
                Rect::new(x - 10.0, y - 6.0, x + 10.0, y + 6.0),
            );
        }
    }
    scene
}

/// A translucent layer over a gradient: the composite reads its page through
/// the layer class, and the page round holds an image, a gradient and a
/// solid of its own.
fn layer_scene() -> Scene {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 256.0, 256.0),
            stops(
                Gradient::new_linear((0.0, 0.0), (256.0, 256.0)),
                Color::from_rgb8(20, 200, 120),
                Color::from_rgb8(200, 20, 120),
            ),
        );
        builder.push_layer(Rect::new(32.0, 32.0, 224.0, 224.0), 0.5);
        builder.fill_rect(
            Rect::new(32.0, 32.0, 224.0, 224.0),
            Brush::Solid(Color::WHITE),
        );
        bubble(
            &mut builder,
            Point::new(128.0, 128.0),
            70.0,
            Color::from_rgb8(40, 90, 200),
        );
        builder.draw_image(&checkerboard(90), Rect::new(100.0, 100.0, 156.0, 156.0));
        builder.push_layer(Rect::new(140.0, 140.0, 220.0, 220.0), 0.5);
        builder.draw_blurred_rounded_rect(
            Rect::new(150.0, 150.0, 210.0, 210.0),
            6.0,
            3.0,
            Color::from_rgb8(255, 200, 0),
        );
        builder.pop_layer();
        builder.pop_layer();
        builder.fill_rect(
            Rect::new(10.5, 10.5, 40.5, 40.5),
            Brush::Solid(Color::from_rgba8(255, 0, 0, 200)),
        );
    }
    scene
}

/// A hole punch through mixed content on a translucent base: the punch pass
/// is drawn with the solid class, cut between runs of other classes.
fn punch_scene() -> Scene {
    let mut scene = mixed_scene();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        builder.clear_rect(Rect::new(64.0, 64.0, 160.0, 160.0));
        builder.draw_image(&checkerboard(7), Rect::new(100.0, 100.0, 140.0, 140.0));
    }
    scene
}

/// Renders `scene` twice — a cold frame, then a steady one — through a fresh
/// renderer configured by `configure`, returning both read-backs and how many
/// pipelines the renderer compiled.
fn render_twice(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    caps: &TierCaps,
    scene: &Scene,
    base: Color,
    configure: impl FnOnce(&mut EngineRenderer),
) -> (Vec<Vec<u8>>, u64) {
    let mut renderer =
        EngineRenderer::new(device, caps, FORMAT, None).expect("the engine builds on this device");
    configure(&mut renderer);
    renderer.finish_warm_up(device);
    let target = HeadlessTarget::new(device, SIZE, SIZE, FORMAT);
    let mut frames = Vec::new();
    for _ in 0..2 {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine paint-class frame"),
        });
        renderer
            .encode(
                device,
                queue,
                &mut encoder,
                scene,
                EngineTarget {
                    view: target.view(),
                    format: FORMAT,
                    width: SIZE,
                    height: SIZE,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                base,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(queue);
        let error = drain_error_scope(device, scope);
        assert!(error.is_none(), "the frame raised {error:?}");
        frames.push(target.read_back(device, queue));
    }
    (frames, renderer.compiled_pipelines())
}

/// The acceptance bar: every class renders the paints it owns byte for byte
/// as the Uber program does, so a frame drawn through class runs — at every
/// threshold, which moves which runs fall back to Uber — reads back
/// identical to the same frame drawn through the Uber program alone.
#[test]
#[ignore = "requires a GPU (Vulkan/Metal); run with `cargo test -p frust-engine -- --ignored`"]
fn routed_frames_render_byte_identically_to_the_uber_program() {
    let _guard = render_lock();
    let (device, queue, caps) = gpu();

    let cases: [(&str, Scene, Color); 4] = [
        ("mixed", mixed_scene(), Color::TRANSPARENT),
        (
            "mixed-opaque-base",
            mixed_scene(),
            Color::from_rgb8(30, 30, 30),
        ),
        ("layer-over-gradient", layer_scene(), Color::TRANSPARENT),
        ("punch", punch_scene(), Color::from_rgba8(0, 0, 0, 0)),
    ];
    for (name, scene, base) in &cases {
        let (uber, uber_compiled) = render_twice(&device, &queue, &caps, scene, *base, |r| {
            r.set_uber_program(true)
        });
        assert!(
            uber[0].as_chunks::<4>().0.iter().any(|px| px[3] != 0),
            "{name}: the Uber frame drew nothing"
        );
        // The highest threshold first: every class run is then too short and
        // drawn with Uber, so the pipelines that renderer builds are the
        // baseline a lower threshold's class pipelines are counted against.
        let mut merged_compiled = None;
        for min_run in [u32::MAX, 64, RUN_SPLIT_MIN, 1, 0] {
            let (routed, routed_compiled) =
                render_twice(&device, &queue, &caps, scene, *base, |r| {
                    r.set_uber_program(false);
                    r.set_run_split_min(min_run);
                });
            for (frame, (a, b)) in uber.iter().zip(&routed).enumerate() {
                let differing = a
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(b.as_chunks::<4>().0)
                    .filter(|(x, y)| x != y)
                    .count();
                assert_eq!(
                    differing, 0,
                    "{name} (threshold {min_run}, frame {frame}): {differing} pixels differ from \
                     the Uber program"
                );
            }
            let baseline = *merged_compiled.get_or_insert(routed_compiled);
            // Each case draws a long run of a class the warm-up list does not
            // cover (a shadow on the surface, a page round's classes, a
            // punch), so at the default threshold that class's pipeline is
            // built because a run used it — and with every run merged to Uber
            // it is not.
            if min_run <= RUN_SPLIT_MIN {
                assert!(
                    routed_compiled > baseline,
                    "{name} (threshold {min_run}): no class pipeline beyond the all-Uber \
                     routing's {baseline} — the classes never ran"
                );
            }
            assert!(routed_compiled >= uber_compiled, "{name}");
        }
    }
}
