//! A deterministic content fingerprint over a slice of [`Command`]s — the
//! cache key a renderer's [`Command::PushSnapshot`] implementation hashes a
//! recorded body against to decide whether a cached rasterization is still
//! valid.
//!
//! Every field that reaches pixels is folded in: geometry, brushes/gradients,
//! radii, dash patterns, stroke widths, glyph runs, image/shader identity.
//! Each [`kurbo::Affine`] is hashed **relative to `base`**
//! (`base.inverse() * transform`) rather than absolutely, so a body that
//! merely slides, scales, or fades as a whole (its own transform moving, but
//! nothing inside it changing) keeps the same fingerprint — the load-bearing
//! property a snapshot cache needs to survive an ordinary animation. Every
//! coefficient/coordinate is quantized before hashing (see
//! [`FINGERPRINT_QUANT`]) so float noise from the `base.inverse()` multiply
//! can never flip the hash for two recordings that are mathematically
//! identical.
//!
//! Hashing uses `std`'s [`DefaultHasher`], constructed with
//! [`DefaultHasher::new`] rather than through a [`std::collections::HashMap`]
//! — that constructor seeds with fixed keys, so the result is deterministic
//! within a process (unlike `RandomState`, which would make two calls with
//! identical input disagree).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::mem;
use std::ops::Range;

use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use peniko::{Brush, Color, Gradient, GradientKind, ImageData};

use crate::glyph::GlyphRun;
use crate::scene::{Command, CornerRadii, DashPattern, PathStyle, Scene};

/// Fixed-point quantization step (per logical pixel / unit) applied to every
/// float before hashing — see the module docs for why.
const FINGERPRINT_QUANT: f64 = 4096.0;

fn quantize(v: f64) -> i64 {
    (v * FINGERPRINT_QUANT).round() as i64
}

fn quantize_f32(v: f32) -> i64 {
    quantize(v as f64)
}

impl Scene {
    /// Fingerprints `self.commands()[range]`, hashed relative to `base` (see
    /// [`fingerprint_commands`]).
    pub fn fingerprint_range(&self, range: Range<usize>, base: Affine) -> u64 {
        fingerprint_commands(&self.commands()[range], base)
    }
}

/// Deterministic 64-bit hash of `commands`, with every [`Affine`] hashed
/// relative to `base` (see the module docs).
pub fn fingerprint_commands(commands: &[Command], base: Affine) -> u64 {
    let base_inv = base.inverse();
    let mut hasher = DefaultHasher::new();
    commands.len().hash(&mut hasher);
    for command in commands {
        hash_command(&mut hasher, command, base_inv);
    }
    hasher.finish()
}

fn hash_point(hasher: &mut DefaultHasher, p: &Point) {
    quantize(p.x).hash(hasher);
    quantize(p.y).hash(hasher);
}

fn hash_rect(hasher: &mut DefaultHasher, rect: &Rect) {
    quantize(rect.x0).hash(hasher);
    quantize(rect.y0).hash(hasher);
    quantize(rect.x1).hash(hasher);
    quantize(rect.y1).hash(hasher);
}

/// Hashes `transform` relative to `base_inv` (`base_inv * transform`), the
/// property that keeps a body's fingerprint stable while its enclosing
/// bracket merely slides/scales/fades as a whole (see the module docs).
fn hash_affine(hasher: &mut DefaultHasher, base_inv: Affine, transform: Affine) {
    for c in (base_inv * transform).as_coeffs() {
        quantize(c).hash(hasher);
    }
}

fn hash_corner_radii(hasher: &mut DefaultHasher, radii: &CornerRadii) {
    quantize(radii.top_left).hash(hasher);
    quantize(radii.top_right).hash(hasher);
    quantize(radii.bottom_right).hash(hasher);
    quantize(radii.bottom_left).hash(hasher);
}

fn hash_dash(hasher: &mut DefaultHasher, dash: &DashPattern) {
    quantize(dash.on).hash(hasher);
    quantize(dash.off).hash(hasher);
    quantize(dash.phase).hash(hasher);
}

fn hash_path_style(hasher: &mut DefaultHasher, style: &PathStyle) {
    mem::discriminant(style).hash(hasher);
    if let PathStyle::Stroke { width, dash } = style {
        quantize(*width).hash(hasher);
        match dash {
            Some(dash) => {
                true.hash(hasher);
                hash_dash(hasher, dash);
            }
            None => false.hash(hasher),
        }
    }
}

fn hash_bez_path(hasher: &mut DefaultHasher, path: &BezPath) {
    for el in path.elements() {
        mem::discriminant(el).hash(hasher);
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => hash_point(hasher, p),
            PathEl::QuadTo(p0, p1) => {
                hash_point(hasher, p0);
                hash_point(hasher, p1);
            }
            PathEl::CurveTo(p0, p1, p2) => {
                hash_point(hasher, p0);
                hash_point(hasher, p1);
                hash_point(hasher, p2);
            }
            PathEl::ClosePath => {}
        }
    }
}

fn hash_color(hasher: &mut DefaultHasher, color: &Color) {
    for c in color.components {
        c.to_bits().hash(hasher);
    }
}

fn hash_dynamic_color(hasher: &mut DefaultHasher, color: &peniko::color::DynamicColor) {
    color.cs.hash(hasher);
    color.flags.hash(hasher);
    for c in color.components {
        c.to_bits().hash(hasher);
    }
}

fn hash_gradient(hasher: &mut DefaultHasher, gradient: &Gradient) {
    match &gradient.kind {
        GradientKind::Linear(pos) => {
            0u8.hash(hasher);
            hash_point(hasher, &pos.start);
            hash_point(hasher, &pos.end);
        }
        GradientKind::Radial(pos) => {
            1u8.hash(hasher);
            hash_point(hasher, &pos.start_center);
            quantize_f32(pos.start_radius).hash(hasher);
            hash_point(hasher, &pos.end_center);
            quantize_f32(pos.end_radius).hash(hasher);
        }
        GradientKind::Sweep(pos) => {
            2u8.hash(hasher);
            hash_point(hasher, &pos.center);
            quantize_f32(pos.start_angle).hash(hasher);
            quantize_f32(pos.end_angle).hash(hasher);
        }
    }
    (gradient.extend as u8).hash(hasher);
    gradient.interpolation_cs.hash(hasher);
    (gradient.hue_direction as u8).hash(hasher);
    gradient.interpolation_alpha_space.hash(hasher);
    for stop in gradient.stops.iter() {
        quantize_f32(stop.offset).hash(hasher);
        hash_dynamic_color(hasher, &stop.color);
    }
}

fn hash_image_data(hasher: &mut DefaultHasher, data: &ImageData) {
    // `Blob::id()` (peniko's reference-counted resource handle) is the
    // identity a snapshot cache actually needs — content changing always
    // mints a new blob, so the id alone tracks "did the pixels change".
    data.data.id().hash(hasher);
}

fn hash_image_sampler(hasher: &mut DefaultHasher, sampler: &peniko::ImageSampler) {
    (sampler.x_extend as u8).hash(hasher);
    (sampler.y_extend as u8).hash(hasher);
    (sampler.quality as u8).hash(hasher);
    quantize_f32(sampler.alpha).hash(hasher);
}

fn hash_brush(hasher: &mut DefaultHasher, brush: &Brush) {
    mem::discriminant(brush).hash(hasher);
    match brush {
        Brush::Solid(color) => hash_color(hasher, color),
        Brush::Gradient(gradient) => hash_gradient(hasher, gradient),
        Brush::Image(image_brush) => {
            hash_image_data(hasher, &image_brush.image);
            hash_image_sampler(hasher, &image_brush.sampler);
        }
    }
}

/// Hashes a glyph run's font/blob id, size, brush ("style bits"), transform
/// (relative to `base_inv`), and every glyph's id + position.
fn hash_glyph_run(hasher: &mut DefaultHasher, run: &GlyphRun, base_inv: Affine) {
    run.font.font().data.id().hash(hasher);
    run.font.font().index.hash(hasher);
    quantize_f32(run.font_size).hash(hasher);
    hash_brush(hasher, &run.brush);
    hash_affine(hasher, base_inv, run.transform);
    run.glyphs.len().hash(hasher);
    for glyph in &run.glyphs {
        glyph.id.hash(hasher);
        quantize_f32(glyph.x).hash(hasher);
        quantize_f32(glyph.y).hash(hasher);
    }
}

fn hash_command(hasher: &mut DefaultHasher, command: &Command, base_inv: Affine) {
    mem::discriminant(command).hash(hasher);
    match command {
        Command::FillRect {
            rect,
            brush,
            transform,
        } => {
            hash_rect(hasher, rect);
            hash_brush(hasher, brush);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::RoundedRect {
            rect,
            radii,
            brush,
            transform,
        } => {
            hash_rect(hasher, rect);
            hash_corner_radii(hasher, radii);
            hash_brush(hasher, brush);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::Line {
            p0,
            p1,
            width,
            brush,
            transform,
        } => {
            hash_point(hasher, p0);
            hash_point(hasher, p1);
            quantize(*width).hash(hasher);
            hash_brush(hasher, brush);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::GlyphRun(run) => hash_glyph_run(hasher, run, base_inv),
        Command::PushClip { rect, transform } => {
            hash_rect(hasher, rect);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::PushClipRounded {
            rect,
            radii,
            transform,
        } => {
            hash_rect(hasher, rect);
            hash_corner_radii(hasher, radii);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::PopClip => {}
        Command::Image {
            data,
            dest,
            transform,
        } => {
            hash_image_data(hasher, data);
            hash_rect(hasher, dest);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            color,
            transform,
        } => {
            hash_rect(hasher, rect);
            hash_corner_radii(hasher, radii);
            quantize(*std_dev).hash(hasher);
            hash_color(hasher, color);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::PushLayer {
            rect,
            alpha,
            transform,
        } => {
            hash_rect(hasher, rect);
            quantize_f32(*alpha).hash(hasher);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::PopLayer => {}
        Command::ClearRect { rect, transform } => {
            hash_rect(hasher, rect);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::Path {
            path,
            style,
            brush,
            transform,
        } => {
            hash_bez_path(hasher, path);
            hash_path_style(hasher, style);
            hash_brush(hasher, brush);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::ShaderQuad {
            program,
            dest,
            transform,
            time,
        } => {
            program.id().hash(hasher);
            hash_rect(hasher, dest);
            hash_affine(hasher, base_inv, *transform);
            quantize_f32(*time).hash(hasher);
        }
        Command::PushSnapshot {
            key,
            rect,
            alpha,
            scale,
            transform,
        } => {
            key.hash(hasher);
            hash_rect(hasher, rect);
            quantize_f32(*alpha).hash(hasher);
            quantize(*scale).hash(hasher);
            hash_affine(hasher, base_inv, *transform);
        }
        Command::PopSnapshot => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::SceneBuilder;
    use crate::glyph::{FontHandle, Glyph};
    use peniko::FontData;
    use peniko::color::palette::css::{BLUE, RED};

    fn red_brush() -> Brush {
        Brush::Solid(RED)
    }

    fn empty_font() -> FontHandle {
        FontHandle::new(FontData::new(peniko::Blob::from(vec![1u8]), 0))
    }

    fn two_by_two_image() -> ImageData {
        image_of_bytes(vec![0u8; 2 * 2 * 4])
    }

    fn image_of_bytes(bytes: Vec<u8>) -> ImageData {
        ImageData {
            data: peniko::Blob::from(bytes),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn identical_recordings_fingerprint_equal() {
        // Cloned into both recordings — `peniko::Blob::id()` mints a fresh id
        // per *construction*, not per content (mirrors `Command::Image`'s own
        // doc comment: a widget hands the same cached handle to every frame),
        // so two independently-constructed images would never fingerprint
        // equal even with byte-identical contents.
        let image = two_by_two_image();

        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), red_brush());
        builder.draw_image(&image, Rect::new(0.0, 0.0, 4.0, 4.0));

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), red_brush());
        builder.draw_image(&image, Rect::new(0.0, 0.0, 4.0, 4.0));

        assert_eq!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_rect_coordinate_changes_the_fingerprint() {
        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), red_brush());

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.fill_rect(Rect::new(0.0, 0.0, 11.0, 10.0), red_brush());

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_color_changes_the_fingerprint() {
        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), red_brush());

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Brush::Solid(BLUE));

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_glyph_position_changes_the_fingerprint() {
        let run = |x: f32| GlyphRun {
            font: empty_font(),
            font_size: 16.0,
            brush: red_brush(),
            transform: Affine::IDENTITY,
            glyphs: vec![Glyph { id: 1, x, y: 0.0 }],
        };

        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.draw_glyph_run(run(0.0));

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.draw_glyph_run(run(5.0));

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_image_blob_changes_the_fingerprint() {
        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.draw_image(
            &image_of_bytes(vec![0u8; 16]),
            Rect::new(0.0, 0.0, 4.0, 4.0),
        );

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.draw_image(
            &image_of_bytes(vec![1u8; 16]),
            Rect::new(0.0, 0.0, 4.0, 4.0),
        );

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_snapshot_scale_changes_the_fingerprint() {
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.push_snapshot(1, rect, 1.0, 1.0);
        builder.pop_snapshot();

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.push_snapshot(1, rect, 1.0, 0.9);
        builder.pop_snapshot();

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    #[test]
    fn different_snapshot_alpha_changes_the_fingerprint() {
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut scene_a = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_a);
        builder.push_snapshot(1, rect, 1.0, 1.0);
        builder.pop_snapshot();

        let mut scene_b = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene_b);
        builder.push_snapshot(1, rect, 0.5, 1.0);
        builder.pop_snapshot();

        assert_ne!(
            fingerprint_commands(scene_a.commands(), Affine::IDENTITY),
            fingerprint_commands(scene_b.commands(), Affine::IDENTITY),
        );
    }

    /// The load-bearing property: a body re-recorded after an *outer*
    /// translate, and again after an outer scale, fingerprints EQUAL when
    /// hashed relative to the bracket's own transform — a page that merely
    /// slides/scales/fades as a whole keeps its cached fingerprint.
    #[test]
    fn fingerprint_is_stable_under_the_bracket_own_translate_and_scale() {
        fn record_body(outer: Affine) -> (Scene, Affine) {
            let mut scene = Scene::new();
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_transform(outer);
            let bracket_transform = builder.current_transform();
            builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), red_brush());
            builder.fill_rounded_rect(Rect::new(2.0, 2.0, 8.0, 8.0), 1.5, red_brush());
            (scene, bracket_transform)
        }

        let (translated, base_t) = record_body(Affine::translate((37.0, -12.0)));
        let (scaled, base_s) = record_body(Affine::scale(2.25));
        let (identity, base_i) = record_body(Affine::IDENTITY);

        let fp_translated = translated.fingerprint_range(0..translated.commands().len(), base_t);
        let fp_scaled = scaled.fingerprint_range(0..scaled.commands().len(), base_s);
        let fp_identity = identity.fingerprint_range(0..identity.commands().len(), base_i);

        assert_eq!(fp_translated, fp_identity);
        assert_eq!(fp_scaled, fp_identity);
    }

    #[test]
    fn fingerprint_range_matches_fingerprint_commands_over_the_same_slice() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), red_brush());
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), red_brush());
        builder.fill_rect(Rect::new(2.0, 2.0, 3.0, 3.0), red_brush());

        let via_range = scene.fingerprint_range(1..3, Affine::IDENTITY);
        let via_commands = fingerprint_commands(&scene.commands()[1..3], Affine::IDENTITY);
        assert_eq!(via_range, via_commands);
    }
}
