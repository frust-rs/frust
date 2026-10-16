//! Steady-state glyph-atlas replay: a screen of slowly drifting labels must
//! stop re-rasterizing once its glyphs have been seen.
//!
//! The scene is a bubble field in the shape of a phone-sized animation
//! benchmark: dozens of bubbles, each carrying two short glyph runs, drifting
//! under a small deterministic physics step. Most bubbles creep: they move by
//! a fraction of a device pixel per frame, so a label lingers in one of the
//! four subpixel buckets glifo keys a glyph on for longer than the cache's age
//! horizon, while its sibling buckets go unused, age out, and are rasterized
//! again when the label drifts back. Each such miss is a frame with atlas
//! fills to replay, so the replay cost is paid on frames that draw nothing new.
//!
//! The test drives the real compiler on the host (no GPU) and counts the
//! frames that leave fills to replay in each 60-frame window. After a warm-up
//! the count must be (nearly) zero.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Blob, Brush, Color, FontData};

/// Noto Sans, subsetted to Latin plus combining marks.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// Glyph ids of `Hello` in [`LATIN_FONT`] and a disjoint set.
const HELLO: [u32; 5] = [5, 6, 7, 7, 8];
const OTHERS: [u32; 6] = [9, 10, 11, 12, 6, 7];

/// Frames per counted window.
const WINDOW: usize = 60;

/// Frames before counting starts: every label has been seen in the buckets
/// its first moves reach, and the age horizon has turned over several times.
const WARM_UP: usize = 600;

/// Frames simulated.
const FRAMES: usize = 1800;

/// The seed the bubble field is laid out from.
const SEED: u64 = 42;

fn latin_font() -> FontHandle {
    static FONT: std::sync::OnceLock<FontHandle> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)))
        .clone()
}

/// A device: its pixel scale and logical play area.
struct Device {
    scale: f64,
    width: f64,
    height: f64,
    top: f64,
    bottom: f64,
}

/// A 1080 x 2340 phone at 2.75x.
fn tall_phone() -> Device {
    Device { scale: 2.75, width: 1080.0 / 2.75, height: 2340.0 / 2.75, top: 24.0, bottom: 48.0 }
}

/// A 750 x 1334 phone at 2x.
fn small_phone() -> Device {
    Device { scale: 2.0, width: 375.0, height: 667.0, top: 20.0, bottom: 0.0 }
}

// ---------------------------------------------------------------------------
// The bubble field.
// ---------------------------------------------------------------------------

const FRICTION: f64 = 0.90;
const CENTER_PULL: f64 = 0.001;
const COLLISION_STRENGTH: f64 = 0.08;
const WALL_BOUNCE: f64 = 0.35;
const RADIUS_MIN_FRAC: f64 = 0.05;
const RADIUS_SPAN_FRAC: f64 = 0.10;
const CLUSTER_SPAN_FRAC: f64 = 0.5;
const COUNT_CAP: usize = 60;

struct Bubble {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    radius: f64,
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Field {
    bubbles: Vec<Bubble>,
    width: f64,
    height: f64,
}

impl Field {
    fn new(width: f64, height: f64, seed: u64) -> Self {
        let s = width.min(height);
        let a = RADIUS_MIN_FRAC;
        let span = RADIUS_SPAN_FRAC;
        let mean_area = std::f64::consts::PI * s * s * (a * a + a * span + span * span / 3.0);
        let count = ((0.5 * width * height / mean_area).floor() as usize).min(COUNT_CAP);
        let mut rng = SplitMix64(seed);
        let bubbles = (0..count)
            .map(|_| {
                let _performance = rng.next_f64();
                let radius = (RADIUS_MIN_FRAC + rng.next_f64() * RADIUS_SPAN_FRAC) * s;
                let x = width / 2.0 + (rng.next_f64() * CLUSTER_SPAN_FRAC - CLUSTER_SPAN_FRAC / 2.0) * s;
                let y = height / 2.0 + (rng.next_f64() * CLUSTER_SPAN_FRAC - CLUSTER_SPAN_FRAC / 2.0) * s;
                Bubble { x, y, vx: 0.0, vy: 0.0, radius }
            })
            .collect();
        Self { bubbles, width, height }
    }

    /// One frame's step: centre pull, pairwise collisions, friction, walls,
    /// integration, and zeroing of tiny velocities.
    fn step(&mut self) {
        let (cx, cy) = (self.width / 2.0, self.height / 2.0);
        for b in &mut self.bubbles {
            b.vx += (cx - b.x) * CENTER_PULL;
            b.vy += (cy - b.y) * CENTER_PULL;
        }
        for i in 0..self.bubbles.len() {
            for j in (i + 1)..self.bubbles.len() {
                let (dx, dy, min) = {
                    let (a, b) = (&self.bubbles[i], &self.bubbles[j]);
                    (b.x - a.x, b.y - a.y, a.radius + b.radius)
                };
                let distance = (dx * dx + dy * dy).sqrt();
                if distance < min && distance > 0.0 {
                    let force = (min - distance) * COLLISION_STRENGTH;
                    let (nx, ny) = (dx / distance, dy / distance);
                    self.bubbles[i].vx -= nx * force;
                    self.bubbles[i].vy -= ny * force;
                    self.bubbles[j].vx += nx * force;
                    self.bubbles[j].vy += ny * force;
                }
            }
        }
        for b in &mut self.bubbles {
            b.vx *= FRICTION;
            b.vy *= FRICTION;
        }
        let (w, h) = (self.width, self.height);
        for b in &mut self.bubbles {
            if b.x - b.radius < 0.0 {
                b.x = b.radius;
                b.vx = b.vx.abs() * WALL_BOUNCE;
            }
            if b.x + b.radius > w {
                b.x = w - b.radius;
                b.vx = -b.vx.abs() * WALL_BOUNCE;
            }
            if b.y - b.radius < 0.0 {
                b.y = b.radius;
                b.vy = b.vy.abs() * WALL_BOUNCE;
            }
            if b.y + b.radius > h {
                b.y = h - b.radius;
                b.vy = -b.vy.abs() * WALL_BOUNCE;
            }
        }
        for b in &mut self.bubbles {
            b.x += b.vx;
            b.y += b.vy;
            if b.vx.abs() < 0.05 {
                b.vx = 0.0;
            }
            if b.vy.abs() < 0.05 {
                b.vy = 0.0;
            }
        }
    }
}

/// A label's font size from its bubble's radius, fixed per bubble.
fn label_size(radius: f64) -> f32 {
    const MIN_R: f64 = 18.0;
    const MAX_R: f64 = 60.0;
    if radius <= MIN_R {
        8.0
    } else if radius >= MAX_R {
        24.0
    } else {
        (8.0 + ((radius - MIN_R) / (MAX_R - MIN_R)).sqrt() * 16.0) as f32
    }
}

/// A run of `ids` at `size`, its glyphs `0.55 em` apart, drawn at `origin`.
fn label(ids: &[u32], size: f32, origin: (f64, f64), color: Color) -> GlyphRun {
    let advance = size * 0.55;
    GlyphRun {
        font: latin_font(),
        font_size: size,
        brush: Brush::Solid(color),
        transform: Affine::translate(origin),
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph { id: *id, x: index as f32 * advance, y: 0.0 })
            .collect(),
    }
}

/// What the replay count of each window looks like.
struct Report {
    /// Frames that left fills to replay, per window, after the warm-up.
    replays: Vec<usize>,
    /// Glyph draws on the last frame, so a fix cannot win by drawing nothing.
    glyph_draws: u32,
}

/// Drive the field for [`FRAMES`] frames through the real compiler.
fn drive(device: &Device) -> Report {
    let play_height = device.height - device.top - device.bottom;
    let mut field = Field::new(device.width, play_height, SEED);
    let dims = (
        (device.width * device.scale).round() as u16,
        (device.height * device.scale).round() as u16,
    );
    let mut compiler = SceneCompiler::with_atlas_budget(dims.0, dims.1, AtlasBudget::MOBILE);
    compiler.set_hint_text(false);
    compiler.set_glyph_quads(true);
    let root = Affine::scale(device.scale);
    let sizes: Vec<f32> = field.bubbles.iter().map(|b| label_size(b.radius)).collect();

    let mut replays = vec![0usize; (FRAMES - WARM_UP) / WINDOW];
    let mut glyph_draws = 0;
    for frame in 0..FRAMES {
        field.step();
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(
                Rect::new(0.0, 0.0, device.width, device.height),
                Brush::Solid(Color::from_rgb8(13, 20, 33)),
            );
            for (index, (bubble, size)) in field.bubbles.iter().zip(&sizes).enumerate() {
                let ids = &HELLO[..3 + index % 3];
                let width = f64::from(*size) * 0.55 * ids.len() as f64;
                let y = device.top + bubble.y;
                builder.draw_glyph_run(label(
                    ids,
                    *size,
                    (bubble.x - width / 2.0, y - f64::from(*size) * 0.6),
                    Color::WHITE,
                ));
                let small = size * 0.7;
                let percent_width = f64::from(small) * 0.55 * OTHERS.len() as f64;
                builder.draw_glyph_run(label(
                    &OTHERS,
                    small,
                    (bubble.x - percent_width / 2.0, y + bubble.radius * 0.1),
                    Color::from_rgba8(255, 255, 255, 230),
                ));
            }
        }
        let compiled = compiler.compile(&scene, root, dims).expect("an in-range scene compiles");
        let rasterized = compiler.glyph_replay_pending();
        compiler.acknowledge_glyph_clears();
        compiler.acknowledge_glyph_replay();
        compiler.acknowledge_image_plan();
        glyph_draws = compiled.glyph_draws;
        if frame >= WARM_UP && rasterized {
            let window = (frame - WARM_UP) / WINDOW;
            if let Some(count) = replays.get_mut(window) {
                *count += 1;
            }
        }
    }
    Report { replays, glyph_draws }
}

fn assert_quiet(name: &str, device: &Device) {
    let report = drive(device);
    let total: usize = report.replays.iter().sum();
    let worst = report.replays.iter().copied().max().unwrap_or(0);
    eprintln!(
        "{name}: replay frames per {WINDOW}-frame window after warm-up: {:?} (glyph draws {})",
        report.replays, report.glyph_draws
    );
    assert!(report.glyph_draws > 0, "{name}: the scene draws glyphs");
    // A steady-state window may carry the odd first sighting of a bucket; it
    // may not replay on most frames.
    assert!(
        worst <= 3 && total <= report.replays.len() * 2,
        "{name}: the labels keep re-rasterizing in steady state: {:?}",
        report.replays
    );
}

#[test]
fn a_drifting_bubble_field_stops_replaying_on_a_tall_phone() {
    assert_quiet("tall phone", &tall_phone());
}

#[test]
fn a_drifting_bubble_field_stops_replaying_on_a_small_phone() {
    assert_quiet("small phone", &small_phone());
}
