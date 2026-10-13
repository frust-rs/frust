//! Diagnostic microbenches for `frust-engine`'s CPU-side render pipeline.
//!
//! Host-only, `harness = false` criterion benches mirroring the stages a
//! frame walks through before a single GPU command is recorded: scene
//! compilation, sparse-strip generation, alpha-instance packing, and paint
//! encoding. Nothing in the workspace's build or test gate reads these
//! numbers — `cargo bench -p frust-engine` is a manual diagnostic, run and
//! read by a person chasing a regression, never a CI check.
//!
//! Skips the SIMD-level fan-out upstream benches use: which `fearless_simd`
//! level is active is a target-detection concern orthogonal to what these
//! groups measure, so every bench here just runs at whatever level
//! `Level::try_detect` picks on the host it executes on.

mod scenes;

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use kurbo::{Affine, Point, Shape};
use peniko::color::{ColorSpaceTag, DynamicColor, HueDirection};
use peniko::{
    Brush, Color, ColorStop, ColorStops, Fill, Gradient, GradientKind, LinearGradientPosition,
};

use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};

use frust_engine::gpu::{GpuStrip, StripDraw};
use frust_engine::{SceneCompiler, encode_brush};

use frust_scene::Scene;
use scenes::{
    BenchText, PHONE_VIEWPORT, S1Parts, S6_WIDTHS, S6Parts, VIEWPORT, glyph_run_placeholder,
    icon_set, rect_page, s5_like,
};

/// Number of rectangles in the compile group's page fixture.
const RECT_COUNT: usize = 200;
/// Number of paths in the compile group's icon-set fixture.
const ICON_COUNT: usize = 50;
/// Number of glyphs in the compile group's reserved placeholder run.
const GLYPH_COUNT: usize = 400;

/// `SceneCompiler::compile` over the three fixed-size fixtures the task
/// names: a rectangle-heavy page, a path-heavy icon set, and a glyph-run
/// placeholder.
fn compile_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("compile");

    let page = rect_page(RECT_COUNT);
    group.bench_function("rect_page_200", |b| {
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
        b.iter(|| {
            let frame = compiler
                .compile(black_box(&page), Affine::IDENTITY, VIEWPORT)
                .expect("fixture scene compiles");
            black_box(frame.draws().len());
        });
    });

    let icons = icon_set(ICON_COUNT);
    group.bench_function("icon_set_50", |b| {
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
        b.iter(|| {
            let frame = compiler
                .compile(black_box(&icons), Affine::IDENTITY, VIEWPORT)
                .expect("fixture scene compiles");
            black_box(frame.draws().len());
        });
    });

    // Reserved: glyphs are not compiled this phase (`SceneCompiler` skips
    // `Command::GlyphRun`), so this only measures the walk's cost of
    // recognizing and stepping over 400 glyph-run entries, not shaping or
    // rasterizing them. Re-baseline once glyph compilation lands.
    let glyphs = glyph_run_placeholder(GLYPH_COUNT);
    group.bench_function("glyph_run_400_reserved", |b| {
        let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
        b.iter(|| {
            let frame = compiler
                .compile(black_box(&glyphs), Affine::IDENTITY, VIEWPORT)
                .expect("fixture scene compiles");
            black_box(frame.draws().len());
        });
    });

    group.finish();
}

/// `vello_common::strip_generator::StripGenerator` driven directly — no
/// `Scene`/`SceneCompiler` in the loop — isolating raw flatten-to-strip cost
/// from the scene walk and brush encoding the `compile` group also pays for.
fn strip_generation_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("strip_generation");

    let (width, height) = VIEWPORT;
    let paths: Vec<_> = (0..RECT_COUNT)
        .map(|i| {
            let col = (i % 20) as f64;
            let row = (i / 20) as f64;
            let center = Point::new(20.0 + col * 48.0, 20.0 + row * 32.0);
            kurbo::Circle::new(center, 10.0).to_path(0.1)
        })
        .collect();

    group.bench_function("filled_circles_200", |b| {
        let level = Level::try_detect().unwrap_or(Level::baseline());
        let mut generator = StripGenerator::new(width, height, level);
        b.iter(|| {
            generator.reset(width, height);
            let mut storage = StripStorage::new(GenerationMode::Append);
            for path in &paths {
                generator.generate_filled_path(
                    black_box(path.iter()),
                    Fill::NonZero,
                    Affine::IDENTITY,
                    None,
                    &mut storage,
                    None,
                );
            }
            black_box(storage.strips.len());
        });
    });

    group.finish();
}

/// Converts a compiled frame's strips into the `GpuStrip` instances the
/// vertex shader steps over — the alpha-coverage packing step between CPU
/// compilation and the GPU upload.
fn alpha_packing_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("alpha_packing");

    let page = rect_page(RECT_COUNT);
    let mut compiler = SceneCompiler::new(VIEWPORT.0, VIEWPORT.1);
    let frame = compiler
        .compile(&page, Affine::IDENTITY, VIEWPORT)
        .expect("fixture scene compiles");
    let strips = frame.strip_buf();

    group.bench_function("rect_page_200", |b| {
        b.iter(|| {
            let draw = StripDraw {
                payload: 0,
                paint: 0,
                depth_index: 0,
            };
            let mut instances = Vec::with_capacity(strips.len());
            for pair in black_box(strips).windows(2) {
                let span = GpuStrip::from_strip_pair(&pair[0], &pair[1], draw);
                if span.width > 0 {
                    instances.push(span);
                }
                if let Some(gap) = GpuStrip::gap_fill(&pair[0], &pair[1], draw) {
                    instances.push(gap);
                }
            }
            black_box(instances.len());
        });
    });

    group.finish();
}

/// `encode_brush` over a mix of solid and gradient brushes — the CPU-side
/// paint-to-`EncodedPaint` conversion a frame's draws pay for once each.
fn paint_encoding_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("paint_encoding");

    let solid = Brush::Solid(Color::from_rgba8(30, 120, 220, 255));
    let gradient = Brush::Gradient(Gradient {
        kind: linear_kind(),
        stops: stops(),
        interpolation_cs: ColorSpaceTag::Srgb,
        hue_direction: HueDirection::Shorter,
        ..Default::default()
    });
    let brushes = [solid, gradient];

    group.bench_function("solid_and_gradient_x100", |b| {
        b.iter(|| {
            let mut encoded_paints: Vec<EncodedPaint> = Vec::new();
            for _ in 0..100 {
                for brush in black_box(&brushes) {
                    let encoding = encode_brush(brush, Affine::IDENTITY, &mut encoded_paints);
                    black_box(encoding);
                }
            }
            black_box(encoded_paints.len());
        });
    });

    group.finish();
}

fn linear_kind() -> GradientKind {
    LinearGradientPosition {
        start: Point::new(0.0, 0.0),
        end: Point::new(100.0, 0.0),
    }
    .into()
}

fn stops() -> ColorStops {
    ColorStops(
        vec![
            ColorStop {
                offset: 0.0,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(255, 0, 0)),
            },
            ColorStop {
                offset: 0.5,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(0, 255, 0)),
            },
            ColorStop {
                offset: 1.0,
                color: DynamicColor::from_alpha_color(Color::from_rgb8(0, 0, 255)),
            },
        ]
        .into(),
    )
}

// ---------------------------------------------------------------------------
// Phone-shaped scenes (S1-like, S6-like, S5-like) at 1080x2340.
//
// Two kinds of numbers per scene:
// - `scenes_compile/<scene>`: criterion timing of `SceneCompiler::compile`
//   alone (scene recording is untimed setup), on a long-lived compiler.
// - `scenes_encode/<scene>`: criterion timing of `EngineRenderer::encode` +
//   submit + `end_frame` against an offscreen target on the host GPU (CPU wall
//   time of the encode; the GPU wait after it is untimed). Skipped with a
//   note when no adapter exists or `FRUST_BENCH_NO_GPU` is set.
// Under `--features perf-trace` each scene additionally prints the compile
// span medians and the engine's own `frust-perf enc` window medians (upload,
// instances, replay, counts), gathered over a fixed untimed frame run so the
// report does not depend on how many samples criterion's mode takes.
// ---------------------------------------------------------------------------

/// A named scene source: `step` is the iteration index, so a scene that moves
/// each iteration can vary, and a static one ignores it.
struct SceneCase {
    name: &'static str,
    make: Box<dyn FnMut(u32) -> Scene>,
}

fn scene_cases() -> Vec<SceneCase> {
    let mut text = BenchText::new();
    let s1 = S1Parts::new(&mut text);
    println!(
        "scenes: s1_like = {} circles + {} glyph runs",
        PHONE_SCENE_CIRCLES,
        s1.run_count()
    );
    let s6 = S6Parts::new();
    let s6_runs = s6.runs(&mut text, S6_WIDTHS.0);
    let s6_glyphs: usize = s6_runs.iter().map(|run| run.glyphs.len()).sum();
    println!("scenes: s6_like = {s6_glyphs} glyphs");
    let s6_cached = S6Parts::scene(&s6_runs);
    let s5 = s5_like();

    vec![
        SceneCase {
            name: "s1_like",
            make: Box::new(move |step| s1.scene(step)),
        },
        SceneCase {
            name: "s6_like_cached",
            make: Box::new(move |_| s6_cached.clone()),
        },
        SceneCase {
            name: "s6_like_rebreak",
            make: Box::new(move |step| {
                let width = if step % 2 == 0 {
                    S6_WIDTHS.0
                } else {
                    S6_WIDTHS.1
                };
                S6Parts::scene(&s6.runs(&mut text, width))
            }),
        },
        SceneCase {
            name: "s5_like",
            make: Box::new(move |_| s5.clone()),
        },
    ]
}

const PHONE_SCENE_CIRCLES: usize = scenes::S1_CIRCLES;

fn scenes_compile_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("scenes_compile");
    for mut case in scene_cases() {
        group.bench_function(case.name, |b| {
            let mut compiler = SceneCompiler::new(PHONE_VIEWPORT.0, PHONE_VIEWPORT.1);
            let mut step = 0u32;
            b.iter_batched(
                || {
                    step = step.wrapping_add(1);
                    (case.make)(step)
                },
                |scene| {
                    let frame = compiler
                        .compile(black_box(&scene), Affine::IDENTITY, PHONE_VIEWPORT)
                        .expect("fixture scene compiles");
                    black_box(frame.draws().len());
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();

    #[cfg(feature = "perf-trace")]
    for mut case in scene_cases() {
        report_compile_spans(&mut case);
    }
}

/// Frames in the untimed trace runs: three full `frust-perf enc` windows.
#[cfg(feature = "perf-trace")]
const TRACE_FRAMES: u32 = 180;

/// One compile's `(phases total, walk, glyphs, draws, strips)`.
#[cfg(feature = "perf-trace")]
type CompileRow = (
    std::time::Duration,
    std::time::Duration,
    std::time::Duration,
    usize,
    usize,
);

/// Median compile spans and counts over [`TRACE_FRAMES`] compiles, printed.
#[cfg(feature = "perf-trace")]
fn report_compile_spans(case: &mut SceneCase) {
    let mut compiler = SceneCompiler::new(PHONE_VIEWPORT.0, PHONE_VIEWPORT.1);
    let mut rows: Vec<CompileRow> = Vec::new();
    for step in 0..TRACE_FRAMES {
        let scene = (case.make)(step);
        let frame = compiler
            .compile(&scene, Affine::IDENTITY, PHONE_VIEWPORT)
            .expect("fixture scene compiles");
        let spans = frame.compile_spans;
        rows.push((
            spans.total(),
            spans.walk,
            spans.glyphs,
            frame.draws().len(),
            frame.strip_buf().len(),
        ));
    }
    // Skip the first half: cold caches (glyph atlas admission, image upload plan).
    let warm = &rows[rows.len() / 2..];
    let median = |f: &dyn Fn(&CompileRow) -> u128| {
        let mut v: Vec<u128> = warm.iter().map(f).collect();
        v.sort_unstable();
        v[v.len() / 2]
    };
    println!(
        "trace compile {}: phases_us={:.1} walk_us={:.1} glyphs_us={:.1} draws={} strips={}",
        case.name,
        median(&|r| r.0.as_nanos()) as f64 / 1e3,
        median(&|r| r.1.as_nanos()) as f64 / 1e3,
        median(&|r| r.2.as_nanos()) as f64 / 1e3,
        median(&|r| r.3 as u128),
        median(&|r| r.4 as u128),
    );
}

/// Captures the engine's `frust-perf enc` lines (emitted through `log` at
/// info, one per 60-frame window) so the bench can print their medians.
#[cfg(feature = "perf-trace")]
mod capture {
    use std::sync::Mutex;

    pub static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Info
        }
        fn log(&self, record: &log::Record) {
            let text = record.args().to_string();
            if text.starts_with("frust-perf enc")
                && let Ok(mut lines) = LINES.lock()
            {
                lines.push(text);
            }
        }
        fn flush(&self) {}
    }
    pub fn install() {
        static CAPTURE: Capture = Capture;
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Info);
    }
    /// Drains the captured lines and prints the per-field median across the
    /// warm windows (all but the first), under `label`.
    pub fn report(label: &str) {
        let lines: Vec<String> = LINES
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default();
        let warm: Vec<&String> = lines.iter().skip(1).collect();
        if warm.is_empty() {
            println!("trace encode {label}: no complete window captured");
            return;
        }
        let mut fields: Vec<(String, Vec<f64>)> = Vec::new();
        for line in &warm {
            for token in line.split_whitespace().skip(2) {
                if let Some((key, value)) = token.split_once('=')
                    && let Ok(value) = value.parse::<f64>()
                {
                    match fields.iter_mut().find(|(k, _)| k == key) {
                        Some((_, values)) => values.push(value),
                        None => fields.push((key.to_string(), vec![value])),
                    }
                }
            }
        }
        let mut out = format!(
            "trace encode {label} ({} windows of 60 frames, median):",
            warm.len()
        );
        for (key, mut values) in fields {
            values.sort_by(|a, b| a.total_cmp(b));
            out.push_str(&format!(" {key}={}", values[values.len() / 2]));
        }
        println!("{out}");
    }
}

/// Everything a GPU encode needs; `None` when there is no adapter.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    caps: frust_gpu::TierCaps,
}

const GPU_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// The host GPU, or `None` (with a printed reason) when it cannot be used.
fn gpu() -> Option<Gpu> {
    if std::env::var_os("FRUST_BENCH_NO_GPU").is_some() {
        println!("scenes_encode: skipped (FRUST_BENCH_NO_GPU set)");
        return None;
    }
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = block_on(wgpu::util::initialize_adapter_from_env_or_default(
        &instance, None,
    ))
    .ok()?;
    println!("scenes_encode adapter: {:?}", adapter.get_info());
    let caps = frust_gpu::TierCaps::probe(&adapter);
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("frust-engine bench device"),
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu {
        device,
        queue,
        caps,
    })
}

fn scenes_encode_group(c: &mut Criterion) {
    let Some(gpu) = gpu() else {
        println!("scenes_encode: skipped (no usable GPU adapter)");
        return;
    };
    #[cfg(feature = "perf-trace")]
    capture::install();

    let target = frust_gpu::HeadlessTarget::new(
        &gpu.device,
        u32::from(PHONE_VIEWPORT.0),
        u32::from(PHONE_VIEWPORT.1),
        GPU_FORMAT,
    );
    let mut group = c.benchmark_group("scenes_encode");
    for mut case in scene_cases() {
        let mut renderer =
            frust_engine::EngineRenderer::new(&gpu.device, &gpu.caps, GPU_FORMAT, None)
                .expect("engine renderer for a headless target");
        renderer.finish_warm_up(&gpu.device);
        let mut encode_frame = |scene: &Scene| {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            renderer
                .encode(
                    &gpu.device,
                    &gpu.queue,
                    &mut encoder,
                    scene,
                    frust_engine::EngineTarget {
                        view: target.view(),
                        format: GPU_FORMAT,
                        width: u32::from(PHONE_VIEWPORT.0),
                        height: u32::from(PHONE_VIEWPORT.1),
                        depth: None,
                        output: frust_engine::OutputAlpha::Premultiplied,
                    },
                    peniko::color::palette::css::BLACK,
                    Affine::IDENTITY,
                )
                .expect("the engine serves the fixture frame");
            gpu.queue.submit([encoder.finish()]);
            renderer.end_frame(&gpu.queue);
        };
        let mut step = 0u32;
        group.bench_function(case.name, |b| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    step = step.wrapping_add(1);
                    let scene = (case.make)(step);
                    let start = std::time::Instant::now();
                    encode_frame(black_box(&scene));
                    total += start.elapsed();
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                }
                total
            });
        });
        #[cfg(feature = "perf-trace")]
        {
            capture::LINES.lock().map(|mut l| l.clear()).ok();
            for _ in 0..TRACE_FRAMES {
                step = step.wrapping_add(1);
                let scene = (case.make)(step);
                encode_frame(&scene);
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            }
            capture::report(case.name);
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    compile_group,
    strip_generation_group,
    alpha_packing_group,
    paint_encoding_group,
    scenes_compile_group,
    scenes_encode_group,
);
criterion_main!(benches);
