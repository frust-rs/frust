//! Scene plan — the pure, GPU-free description of the bench's two content
//! scenes, plus the offscreen-texture geometry/memory helpers.
//!
//! THROWAWAY LAB (see `lib.rs`'s top-of-file note). This module holds no
//! `frust-render` state and paints nothing — it is the deterministic,
//! unit-testable plan the [`crate::bench_view`] widget rasterizes at paint
//! time. Keeping it pure lets the T0 tests assert command counts and layer
//! geometry with no `RenderRoot`, font, or GPU dependency.
//!
//! ## Calibration (scene A ≈ the glyph-catalog Feedback page)
//!
//! [`plan`] generates a Feedback-screen-equivalent vector scene: badges, tags,
//! alerts, toast buttons, and loaders, sized to comparable path/glyph counts
//! as `examples/glyph-catalog/src/pages/feedback.rs` (see that page's
//! badges/tags/alerts/toasts/loaders sections). The exact per-section counts
//! are asserted in `tests/scene.rs` and reported by [`ScenePlan::counts`].

use frust::authoring::{BezPath, Point, Rect, Size, Color, ImageData};
use frust::peniko::{Blob, ImageAlphaType, ImageFormat};

/// The three bench modes (cycled by tapping; the initial mode is the
/// compile-time/runtime `LAYER_BENCH_SCENE` define — see [`crate::initial_mode`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Scene A: the full vector scene, re-rasterized every frame.
    Vector,
    /// Scene B: the same content pre-rendered into [`NUM_LAYERS`] offscreen
    /// image quads, composited every frame with **zero** re-render, plus one
    /// small live vector region (the spinner).
    Composite,
    /// Scene B + one live layer: like [`Mode::Composite`], but one of the
    /// layers is a `ShaderQuad` re-rendered into its own dedicated
    /// `Rgba8Unorm` target **every frame** (the `render_to_texture`-style
    /// per-layer dispatch the spike isolates), the other layers staying static
    /// image quads.
    CompositeRelayer,
}

impl Mode {
    /// The short on-screen/label form (`A` / `B` / `B+1relayer`).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Vector => "A (vector)",
            Mode::Composite => "B (composite)",
            Mode::CompositeRelayer => "B+1relayer",
        }
    }

    /// Next mode in the tap cycle: A -> B -> B+1relayer -> A.
    pub fn next(self) -> Mode {
        match self {
            Mode::Vector => Mode::Composite,
            Mode::Composite => Mode::CompositeRelayer,
            Mode::CompositeRelayer => Mode::Vector,
        }
    }

    /// Parse the `LAYER_BENCH_SCENE` value (`a`/`b`/`c`, case-insensitive; `c`
    /// and its `b+1`/`relayer` aliases select [`Mode::CompositeRelayer`]).
    /// Any unrecognized value falls back to [`Mode::Vector`].
    pub fn parse(raw: &str) -> Mode {
        match raw.trim().to_ascii_lowercase().as_str() {
            "b" => Mode::Composite,
            "c" | "b1" | "b+1" | "relayer" => Mode::CompositeRelayer,
            _ => Mode::Vector,
        }
    }
}

/// How many offscreen layers the composite scenes (B / B+1relayer) split the
/// content into — within a 3-6 offscreen-textures window. Four
/// full-width horizontal bands tile the screen exactly once, so the total
/// composite texture footprint is ~one full-screen layer (see
/// [`layer_memory_bytes`]) — approximately 10.4MB.
pub const NUM_LAYERS: usize = 4;

/// A rounded-rectangle fill (chips, cards, buttons, progress track/fill,
/// skeleton) — positions are relative to the widget origin.
#[derive(Clone, Debug)]
pub struct RectSpec {
    pub rect: Rect,
    pub radius: f64,
    pub color: Color,
}

/// A small filled dot (badge status dot, loader dot) — rendered as a
/// radius-clamped rounded rect. Relative to the widget origin.
#[derive(Clone, Debug)]
pub struct DotSpec {
    pub center: Point,
    pub radius: f64,
    pub color: Color,
}

/// A shaped-text run: the string, its baseline origin (relative to the widget
/// origin), size, and color. Shaped into real glyph runs by the widget.
#[derive(Clone, Debug)]
pub struct LabelSpec {
    pub text: String,
    pub origin: Point,
    pub size: f32,
    pub color: Color,
}

/// A small vector icon path (alert icons). Relative to the widget origin.
#[derive(Clone, Debug)]
pub struct IconSpec {
    pub origin: Point,
    pub path: BezPath,
    pub color: Color,
}

/// The pure command counts, asserted by the T0 tests and reported on the
/// on-screen HUD's calibration display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SceneCounts {
    pub rounded_rects: usize,
    pub dots: usize,
    pub glyph_labels: usize,
    pub icon_paths: usize,
}

/// The full vector scene plan (scene A's content).
#[derive(Clone, Debug, Default)]
pub struct ScenePlan {
    pub rects: Vec<RectSpec>,
    pub dots: Vec<DotSpec>,
    pub labels: Vec<LabelSpec>,
    pub icons: Vec<IconSpec>,
}

impl ScenePlan {
    /// Command counts (for the T0 assertions + the on-screen HUD's calibration display).
    pub fn counts(&self) -> SceneCounts {
        SceneCounts {
            rounded_rects: self.rects.len(),
            dots: self.dots.len(),
            glyph_labels: self.labels.len(),
            icon_paths: self.icons.len(),
        }
    }
}

// A small, arbitrary palette — colors are cosmetic here (the spike measures GPU
// cost, not appearance), chosen only to be visible against the dark background.
const SURFACE_CARD: Color = Color::from_rgb8(0x22, 0x26, 0x2e);
const CHIP: Color = Color::from_rgb8(0x2f, 0x35, 0x40);
const ACCENT: Color = Color::from_rgb8(0x7c, 0x9c, 0xf0);
const SUCCESS: Color = Color::from_rgb8(0x4c, 0xc3, 0x8a);
const WARNING: Color = Color::from_rgb8(0xe0, 0xa8, 0x3e);
const ERROR: Color = Color::from_rgb8(0xe0, 0x6c, 0x6c);
const INK: Color = Color::from_rgb8(0xe6, 0xe9, 0xef);
const INK_DIM: Color = Color::from_rgb8(0x9a, 0xa2, 0xb0);

/// The dark full-screen background fill both scenes paint first (representing
/// the app surface + baseline overdraw).
pub const BACKGROUND: Color = Color::from_rgb8(0x14, 0x17, 0x1c);

/// Build the Feedback-equivalent vector scene plan for a content area of
/// `size`. Deterministic: same `size` always yields the same plan (so the T0
/// count assertions are stable).
pub fn plan(size: Size) -> ScenePlan {
    let mut p = ScenePlan::default();
    let pad = 16.0;
    let w = size.width;
    let mut y = pad + 8.0;

    // --- Badges: 5 chips (3 with a status dot) + a section title. ---
    p.labels
        .push(label("Badges", Point::new(pad, y), 15.0, INK));
    y += 24.0;
    let badge_labels = [
        ("connected", SUCCESS, true),
        ("degraded", WARNING, true),
        ("offline", ERROR, true),
        ("read-only", INK_DIM, false),
        ("v0.44.1", ACCENT, false),
    ];
    let mut x = pad;
    for (text, color, dot) in badge_labels {
        let chip_w = 92.0;
        p.rects.push(rrect(x, y, chip_w, 26.0, 13.0, CHIP));
        if dot {
            p.dots.push(DotSpec {
                center: Point::new(x + 14.0, y + 13.0),
                radius: 4.0,
                color,
            });
            p.labels
                .push(label(text, Point::new(x + 24.0, y + 8.0), 11.0, INK));
        } else {
            p.labels
                .push(label(text, Point::new(x + 12.0, y + 8.0), 11.0, INK));
        }
        x += chip_w + 8.0;
    }
    y += 26.0 + 28.0;

    // --- Tags: 3 removable chips (label + an "x" glyph each). ---
    p.labels.push(label("Tags", Point::new(pad, y), 15.0, INK));
    y += 24.0;
    let mut x = pad;
    for tag in ["rust", "gpu", "reactive"] {
        let chip_w = 84.0;
        p.rects.push(rrect(x, y, chip_w, 26.0, 13.0, CHIP));
        p.labels
            .push(label(tag, Point::new(x + 12.0, y + 8.0), 11.0, INK));
        p.labels.push(label(
            "x",
            Point::new(x + chip_w - 16.0, y + 8.0),
            11.0,
            INK_DIM,
        ));
        x += chip_w + 8.0;
    }
    y += 26.0 + 28.0;

    // --- Alerts: 4 full-width cards (icon path + title + body). Card fills are
    // the scene's main overdraw source (text/icons drawn over them). ---
    p.labels
        .push(label("Alerts", Point::new(pad, y), 15.0, INK));
    y += 24.0;
    let alerts = [
        ("Heads up", "A new workspace layout is available.", ACCENT),
        ("Saved", "Your changes have been saved.", SUCCESS),
        (
            "Low disk space",
            "Free up space soon to avoid issues.",
            WARNING,
        ),
        ("Sync failed", "Check your connection and try again.", ERROR),
    ];
    for (title, body, color) in alerts {
        p.rects
            .push(rrect(pad, y, w - 2.0 * pad, 54.0, 10.0, SURFACE_CARD));
        p.icons
            .push(alert_icon(Point::new(pad + 14.0, y + 16.0), color));
        p.labels
            .push(label(title, Point::new(pad + 40.0, y + 10.0), 13.0, INK));
        p.labels
            .push(label(body, Point::new(pad + 40.0, y + 30.0), 11.0, INK_DIM));
        y += 54.0 + 8.0;
    }
    y += 20.0;

    // --- Toasts: 5 trigger buttons + a caption. ---
    p.labels
        .push(label("Toasts", Point::new(pad, y), 15.0, INK));
    y += 24.0;
    let mut x = pad;
    for name in ["Plain", "Info", "Success", "Warning", "Error"] {
        let btn_w = 78.0;
        p.rects.push(rrect(x, y, btn_w, 34.0, 8.0, CHIP));
        p.labels
            .push(label(name, Point::new(x + 12.0, y + 11.0), 12.0, INK));
        x += btn_w + 8.0;
    }
    y += 34.0 + 8.0;
    p.labels.push(label(
        "Toasts play one at a time (FIFO) and auto-dismiss.",
        Point::new(pad, y),
        11.0,
        INK_DIM,
    ));
    y += 28.0;

    // --- Loaders: progress bar (track + fill), skeleton, dots + captions. ---
    p.labels
        .push(label("Loaders", Point::new(pad, y), 15.0, INK));
    y += 24.0;
    p.labels
        .push(label("Progress (65%)", Point::new(pad, y), 12.0, INK_DIM));
    y += 18.0;
    let bar_w = w - 2.0 * pad;
    p.rects.push(rrect(pad, y, bar_w, 8.0, 4.0, CHIP));
    p.rects.push(rrect(pad, y, bar_w * 0.65, 8.0, 4.0, ACCENT));
    y += 22.0;
    p.labels
        .push(label("Skeleton", Point::new(pad, y), 12.0, INK_DIM));
    y += 18.0;
    p.rects.push(rrect(pad, y, 220.0, 16.0, 6.0, CHIP));
    y += 26.0;
    p.labels
        .push(label("Dots loader", Point::new(pad, y), 12.0, INK_DIM));
    y += 18.0;
    for i in 0..3 {
        p.dots.push(DotSpec {
            center: Point::new(pad + 6.0 + i as f64 * 16.0, y + 6.0),
            radius: 4.0,
            color: ACCENT,
        });
    }

    p
}

/// The [`NUM_LAYERS`] full-width horizontal band rects tiling `size` — one per
/// composite layer (scene B / B+1relayer). Relative to the widget origin.
pub fn layer_bands(size: Size) -> Vec<Rect> {
    let band_h = size.height / NUM_LAYERS as f64;
    (0..NUM_LAYERS)
        .map(|i| {
            let y0 = i as f64 * band_h;
            Rect::new(0.0, y0, size.width, y0 + band_h)
        })
        .collect()
}

/// Physical pixel dimensions of one band's offscreen texture at device-pixel
/// ratio `dpr`, clamped to vello's 8192² atlas cap (mirroring
/// `frust-render`'s `shader_effects::clamp_size` policy).
pub fn layer_texture_dims(band: Rect, dpr: f64) -> (u32, u32) {
    let w = ((band.width() * dpr).round() as u32).clamp(1, 8192);
    let h = ((band.height() * dpr).round() as u32).clamp(1, 8192);
    (w, h)
}

/// Total offscreen texture memory (bytes) for all [`NUM_LAYERS`] band textures
/// at `dpr` — the "texture memory" figure the spike records. RGBA8 = 4 bytes/px.
pub fn layer_memory_bytes(size: Size, dpr: f64) -> u64 {
    layer_bands(size)
        .into_iter()
        .map(|band| {
            let (w, h) = layer_texture_dims(band, dpr);
            w as u64 * h as u64 * 4
        })
        .sum()
}

/// Build one band's offscreen `Rgba8Unorm` [`ImageData`] at physical
/// resolution (`dpr`), filled with a cheap procedural gradient.
///
/// The texture **content** is a gradient, not a pixel-accurate raster of the
/// band's vector primitives — deliberately. Vello's per-frame composite cost
/// (the quad fill + texture-sample bandwidth this spike measures) is
/// independent of what the texels hold; the one-time/per-frame *re-render* cost
/// is measured separately by [`Mode::CompositeRelayer`]'s live shader layer. A
/// gradient keeps construction O(pixels) with no software rasterizer, while
/// still exercising a real, full-size RGBA8 upload.
pub fn build_layer_image(band: Rect, dpr: f64, seed: u8) -> ImageData {
    let (w, h) = layer_texture_dims(band, dpr);
    let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
    for py in 0..h {
        let gy = ((py * 255) / h.max(1)) as u8;
        for px in 0..w {
            let gx = ((px * 255) / w.max(1)) as u8;
            let idx = ((py as usize) * (w as usize) + (px as usize)) * 4;
            pixels[idx] = gx.wrapping_add(seed);
            pixels[idx + 1] = gy;
            pixels[idx + 2] = seed.wrapping_mul(37);
            pixels[idx + 3] = 0xff;
        }
    }
    ImageData {
        data: Blob::from(pixels),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: w,
        height: h,
    }
}

fn label(text: &str, origin: Point, size: f32, color: Color) -> LabelSpec {
    LabelSpec {
        text: text.to_string(),
        origin,
        size,
        color,
    }
}

fn rrect(x: f64, y: f64, w: f64, h: f64, radius: f64, color: Color) -> RectSpec {
    RectSpec {
        rect: Rect::new(x, y, x + w, y + h),
        radius,
        color,
    }
}

/// A small triangular "alert" glyph path (a filled warning triangle), ~16px.
fn alert_icon(origin: Point, color: Color) -> IconSpec {
    let mut path = BezPath::new();
    path.move_to((origin.x + 8.0, origin.y));
    path.line_to((origin.x + 16.0, origin.y + 14.0));
    path.line_to((origin.x, origin.y + 14.0));
    path.close_path();
    IconSpec {
        origin: Point::ZERO,
        path,
        color,
    }
}
