//! Scene fixtures shared by frust-engine's diagnostic microbenches.
//!
//! Each builder returns a freshly recorded `frust_scene::Scene`. A bench
//! builds the scene once and reuses it across iterations, mirroring the
//! steady-state shape a real frame compiles: the display list is retained,
//! only compilation happens per frame.

use std::sync::Arc;

use kurbo::{Affine, Circle, Point, Rect, Shape};
use peniko::color::DynamicColor;
use peniko::{
    Blob, Brush, Color, ColorStop, FontData, Gradient, ImageAlphaType, ImageData, ImageFormat,
};

use frust_text::{FontFamily, TextContext, TextStyle};

use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};

/// The viewport every fixture is recorded (and compiled) against.
pub const VIEWPORT: (u16, u16) = (1024, 768);

/// A page of `count` small filled rectangles tiled across the viewport — the
/// flatten/tile-heavy shape a scrollable list or grid paints.
pub fn rect_page(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let columns = 20usize;
    for i in 0..count {
        let col = (i % columns) as f64;
        let row = (i / columns) as f64;
        let x0 = col * 48.0;
        let y0 = row * 32.0;
        builder.fill_rect(
            Rect::new(x0, y0, x0 + 40.0, y0 + 24.0),
            Brush::Solid(Color::from_rgba8(30, 120, 220, 255)),
        );
    }

    scene
}

/// A set of `count` filled circular paths — a vector-drawn icon set, as
/// distinct from a glyph run.
pub fn icon_set(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let columns = 25usize;
    for i in 0..count {
        let col = (i % columns) as f64;
        let row = (i / columns) as f64;
        let center = Point::new(20.0 + col * 36.0, 20.0 + row * 36.0);
        let circle = kurbo::Circle::new(center, 12.0);
        builder.fill_path(
            circle.to_path(0.1),
            Brush::Solid(Color::from_rgba8(240, 240, 240, 255)),
        );
    }

    scene
}

/// A single glyph run of `count` glyphs, placed at identity.
///
/// A placeholder: `SceneCompiler` recognizes `Command::GlyphRun` and skips
/// it (glyph compilation is not wired up yet), so compiling this scene only
/// measures the walk's cost of recognizing and stepping over the command —
/// never real shaping or rasterization. Reserved for the group that will
/// exercise the real path once glyphs compile.
pub fn glyph_run_placeholder(count: usize) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);

    let font = FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0));
    let glyphs = (0..count)
        .map(|i| Glyph {
            id: i as u32,
            x: i as f32 * 10.0,
            y: 0.0,
        })
        .collect();

    builder.draw_glyph_run(GlyphRun {
        font,
        font_size: 16.0,
        brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 255)),
        transform: Affine::IDENTITY,
        glyphs,
    });

    scene
}

// ---------------------------------------------------------------------------
// Phone-shaped scenes: the CPU half of the benchmark app's S1 / S5 / S6
// scenarios, recorded straight through `SceneBuilder` (the app's own
// `benchmarks/` tree is read-only for the plan that added these).
// ---------------------------------------------------------------------------

/// The viewport the S1/S5/S6-like scenes compile against: a 1080x2340 phone.
pub const PHONE_VIEWPORT: (u16, u16) = (1080, 2340);

/// Bundled Latin face, the same one `frust-testing`'s deterministic goldens use.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// A text context with only the bundled Latin face registered — no system font.
pub struct BenchText {
    cx: TextContext,
    family: FontFamily,
}

impl BenchText {
    pub fn new() -> Self {
        let mut cx = TextContext::new();
        let registered = cx
            .register_fonts(LATIN_FONT.to_vec())
            .expect("the bundled Latin face registers");
        let family = FontFamily::stack(registered.iter().map(|family| family.name.clone()));
        Self { cx, family }
    }

    fn style(&self, size: f32, color: Color) -> TextStyle {
        TextStyle {
            family: self.family.clone(),
            ..TextStyle::new(size, color)
        }
    }

    /// Shapes `text` once at `size`, returning its runs at the origin.
    fn runs(&mut self, text: &str, size: f32, color: Color, width: Option<f32>) -> Vec<GlyphRun> {
        let style = self.style(size, color);
        self.cx
            .layout(text, &style, width)
            .to_scene_runs(Point::ORIGIN)
    }
}

/// Label strings. The bundled face is a subset covering only `H e l o`, space,
/// comma and `é`; anything else would fall back to a host font and make the
/// bench machine-dependent, so every string here is spelled from those.
const SYMBOLS: [&str; 4] = ["Hello", "Hole", "Hell", "Hoe"];
const PERCENTS: [&str; 5] = ["lo, ell", "eel", "Hole,", "ole", "Hello,"];

/// Circles in the S1-like scene (the benchmark app's bubble count is of this order).
pub const S1_CIRCLES: usize = 60;

/// The pre-shaped, reusable pieces of the S1-like scene.
pub struct S1Parts {
    /// Per circle: the symbol run(s) then the percent run(s), shaped once.
    labels: Vec<Vec<GlyphRun>>,
    radii: Vec<f64>,
}

impl S1Parts {
    pub fn new(text: &mut BenchText) -> Self {
        let mut labels = Vec::with_capacity(S1_CIRCLES);
        let mut radii = Vec::with_capacity(S1_CIRCLES);
        for i in 0..S1_CIRCLES {
            radii.push(40.0 + (i * 7 % 21) as f64);
            let symbol = SYMBOLS[i % SYMBOLS.len()];
            let percent = PERCENTS[i % PERCENTS.len()];
            let mut runs = text.runs(symbol, 18.0, Color::WHITE, None);
            runs.extend(text.runs(percent, 12.6, Color::from_rgba8(255, 255, 255, 230), None));
            labels.push(runs);
        }
        Self { labels, radii }
    }

    /// Total glyph runs across the scene (two per circle).
    pub fn run_count(&self) -> usize {
        self.labels.iter().map(Vec::len).sum()
    }

    /// The scene for iteration `step`: every circle at a new position.
    pub fn scene(&self, step: u32) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 1080.0, 2340.0),
            Brush::Solid(Color::from_rgba8(18, 18, 24, 255)),
        );
        let t = f64::from(step) * 0.05;
        for (i, radius) in self.radii.iter().enumerate() {
            let phase = i as f64 * 0.37;
            let cx = 540.0 + 400.0 * (t + phase).sin() * ((i % 5) as f64 / 5.0 + 0.2);
            let cy = 1170.0 + 1000.0 * (t * 0.7 + phase * 1.3).cos() * ((i % 7) as f64 / 7.0 + 0.2);
            let center = Point::new(cx, cy);
            let base = Color::from_rgba8(
                (60 + i * 37 % 180) as u8,
                (90 + i * 53 % 150) as u8,
                (200 - i * 11 % 120) as u8,
                255,
            );
            let path = Circle::new(center, *radius).to_path(0.1);
            let gradient_center = Point::new(center.x - 0.2 * radius, center.y - 0.2 * radius);
            let fill = Gradient::new_radial(gradient_center, (0.95 * radius) as f32).with_stops([
                ColorStop {
                    offset: 0.0,
                    color: DynamicColor::from_alpha_color(base.with_alpha(0.7)),
                },
                ColorStop {
                    offset: 1.0,
                    color: DynamicColor::from_alpha_color(base.with_alpha(0.3)),
                },
            ]);
            builder.fill_path(path.clone(), Brush::Gradient(fill));
            builder.stroke_path(path, 2.0, Brush::Solid(base.with_alpha(0.6)));
            if let Some(runs) = self.labels.get(i) {
                builder.push_transform(Affine::translate((center.x - 20.0, center.y)));
                for run in runs {
                    builder.draw_glyph_run(run.clone());
                }
                builder.pop_transform();
            }
        }
        scene
    }
}

/// Words the S6-like paragraph is built from (spelled from the bundled subset's glyphs only — see `SYMBOLS`).
const WORDS: [&str; 12] = [
    "frame",
    "scroll",
    "glyph",
    "atlas",
    "paragraph",
    "render",
    "strip",
    "layout",
    "cache",
    "text",
    "widget",
    "signal",
];

/// The S6-like paragraph: ~2000 glyphs at 18 px.
pub struct S6Parts {
    text: String,
}

/// The two wrap widths the re-break variant alternates between.
pub const S6_WIDTHS: (f32, f32) = (1000.0, 760.0);

impl S6Parts {
    pub fn new() -> Self {
        let mut text = String::new();
        let mut i = 0usize;
        while text.len() < 2000 {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(WORDS[(i * 5 + i / 3) % WORDS.len()]);
            i += 1;
        }
        Self { text }
    }

    /// The paragraph laid out once at `width`, as runs.
    pub fn runs(&self, text: &mut BenchText, width: f32) -> Vec<GlyphRun> {
        text.runs(
            &self.text,
            18.0,
            Color::from_rgba8(235, 235, 235, 255),
            Some(width),
        )
    }

    /// A scene emitting `runs` unchanged.
    pub fn scene(runs: &[GlyphRun]) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(
            Rect::new(0.0, 0.0, 1080.0, 2340.0),
            Brush::Solid(Color::from_rgba8(18, 18, 24, 255)),
        );
        builder.push_transform(Affine::translate((40.0, 60.0)));
        for run in runs {
            builder.draw_glyph_run(run.clone());
        }
        builder.pop_transform();
        scene
    }
}

/// The S5-like scene: nine 256x256 RGBA images magnified into 1064x256 rows
/// under a rect clip. Built once; every iteration compiles the same images.
pub fn s5_like() -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    builder.fill_rect(
        Rect::new(0.0, 0.0, 1080.0, 2340.0),
        Brush::Solid(Color::from_rgba8(18, 18, 24, 255)),
    );
    builder.push_clip(Rect::new(0.0, 0.0, 1080.0, 2340.0));
    for i in 0..9u32 {
        let mut pixels = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256u32 {
            for x in 0..256u32 {
                pixels.extend_from_slice(&[
                    ((x * 255) / 255) as u8,
                    ((y * 255) / 255) as u8,
                    (i * 28) as u8,
                    255,
                ]);
            }
        }
        let image = ImageData {
            data: Blob::new(Arc::new(pixels)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 256,
            height: 256,
        };
        let y0 = f64::from(i) * 262.0;
        builder.draw_image(&image, Rect::new(8.0, y0, 8.0 + 1064.0, y0 + 256.0));
    }
    builder.pop_clip();
    scene
}
