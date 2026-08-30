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

use scenes::{VIEWPORT, glyph_run_placeholder, icon_set, rect_page};

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

criterion_group!(
    benches,
    compile_group,
    strip_generation_group,
    alpha_packing_group,
    paint_encoding_group,
);
criterion_main!(benches);
