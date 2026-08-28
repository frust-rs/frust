//! A deterministic content fingerprint over a slice of [`Command`]s — the
//! cache key a renderer's [`Command::PushSnapshot`] implementation hashes a
//! recorded body against to decide whether a cached rasterization is still
//! valid.
//!
//! The contract is **pixels relative to the `base` frame**. For each command
//! this module computes `rel = base.inverse() * command.transform` (for
//! [`Command::GlyphRun`]: `base.inverse() * run.transform`) and then:
//!
//! - hashes only `rel`'s LINEAR part (`rel.as_coeffs()[0..4]`, quantized) —
//!   never its translation, which the point-mapping below already absorbs;
//! - hashes every geometric point/corner MAPPED through `rel`
//!   (`quantize((rel * p).x)`/`.y`) rather than hashed raw.
//!
//! That is the load-bearing property a snapshot cache needs to survive an
//! ordinary animation frame-to-frame, and it holds no matter WHERE a body's
//! slide actually lives. A bracket that merely translates/scales/fades as a
//! whole — the slide riding the commands' own `transform`, nothing inside
//! changing — keeps the same fingerprint, exactly as before. But so does a
//! body whose ABSOLUTE geometry moves every frame while its commands'
//! `transform` stays fixed at identity: `frust-core`'s
//! `ChildPod::paint_child` (`crates/frust-core/src/widget.rs`) builds a
//! child's `PaintCtx` at `ctx.origin() + pod.origin`, so an ordinary widget
//! that slides via `pod.set_origin` moves every command's rect/point/glyph
//! it records — the geometry itself carries the motion, not a transform —
//! and `path_at` is the same story for [`Command::Path`] (a path is
//! translated to the widget's origin before it is ever recorded). Both cases
//! describe the identical set of pixels relative to `base` once `base`
//! itself moves by the same amount, so `base.inverse() * command.transform`
//! mapping the RAW geometry — not just hashing the transform relative to
//! `base` — is what makes the two indistinguishable to the cache.
//!
//! Local-space SCALARS — corner radii, stroke width, dash pattern, font
//! size, blur standard deviation, alpha, presentation scale, shader time,
//! colors, cache keys/ids — stay raw: they describe a *length* or an
//! *identity*, not a *position*, so `rel` has nothing to map them through.
//! The hashed linear part of `rel` is what disambiguates an overall scale
//! for a command whose geometry happens to sit at the bracket's own origin
//! (where mapping alone cannot tell a scaled `base` from an unscaled one).
//!
//! Every coefficient/coordinate is quantized before hashing (see
//! [`FINGERPRINT_QUANT`]) so float noise from the `rel` multiply can never
//! flip the hash for two recordings that are mathematically identical.
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

/// Deterministic 64-bit hash of `commands`, with every command's geometry and
/// transform hashed relative to `base` (see the module docs).
pub fn fingerprint_commands(commands: &[Command], base: Affine) -> u64 {
    let base_inv = base.inverse();
    let mut hasher = DefaultHasher::new();
    commands.len().hash(&mut hasher);
    for command in commands {
        hash_command(&mut hasher, command, base_inv);
    }
    hasher.finish()
}

/// Hashes `p` MAPPED through `rel` (`rel * p`) — the pixel `p` lands on
/// relative to `base`, not `p` itself. This is what keeps an absolute
/// coordinate that slides frame-to-frame (see the module docs) fingerprinting
/// equal once `base` slides by the same amount.
fn hash_point_rel(hasher: &mut DefaultHasher, rel: Affine, p: Point) {
    let mapped = rel * p;
    quantize(mapped.x).hash(hasher);
    quantize(mapped.y).hash(hasher);
}

/// Hashes `rect`'s two defining corners `(x0, y0)`/`(x1, y1)`, each MAPPED
/// through `rel`. Combined with the [`hash_linear`] call every caller makes
/// alongside this one, the two mapped corners plus the shared linear part
/// fully determine the parallelogram `rel` carries `rect` to (the linear part
/// applied to `rect`'s edge vectors gives the other two corners), so they
/// need no separate hash.
fn hash_rect_rel(hasher: &mut DefaultHasher, rel: Affine, rect: &Rect) {
    hash_point_rel(hasher, rel, Point::new(rect.x0, rect.y0));
    hash_point_rel(hasher, rel, Point::new(rect.x1, rect.y1));
}

/// Hashes only the LINEAR part of `rel` (`rel.as_coeffs()[0..4]`) — never its
/// translation, which [`hash_point_rel`]/[`hash_rect_rel`] already absorb by
/// mapping the geometry itself. See the module docs for why this is still
/// needed on top of mapped geometry.
fn hash_linear(hasher: &mut DefaultHasher, rel: Affine) {
    for c in &rel.as_coeffs()[0..4] {
        quantize(*c).hash(hasher);
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

/// Hashes every element of `path`, each point MAPPED through `rel` — `path`'s
/// points are absolute (a caller translates the path to the widget's origin
/// before recording it, see `path_at` in `frust-core`'s `widget.rs`), exactly
/// like every other geometry this module hashes.
fn hash_bez_path(hasher: &mut DefaultHasher, path: &BezPath, rel: Affine) {
    for el in path.elements() {
        mem::discriminant(el).hash(hasher);
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => hash_point_rel(hasher, rel, *p),
            PathEl::QuadTo(p0, p1) => {
                hash_point_rel(hasher, rel, *p0);
                hash_point_rel(hasher, rel, *p1);
            }
            PathEl::CurveTo(p0, p1, p2) => {
                hash_point_rel(hasher, rel, *p0);
                hash_point_rel(hasher, rel, *p1);
                hash_point_rel(hasher, rel, *p2);
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

/// Hashes `gradient`'s geometry MAPPED through `rel` (start/end/center
/// points) and its radii/angles/stops raw — a gradient's positions describe
/// pixels the same way any other command geometry does, while its radii and
/// angles are local-space scalars (see the module docs).
fn hash_gradient(hasher: &mut DefaultHasher, gradient: &Gradient, rel: Affine) {
    match &gradient.kind {
        GradientKind::Linear(pos) => {
            0u8.hash(hasher);
            hash_point_rel(hasher, rel, pos.start);
            hash_point_rel(hasher, rel, pos.end);
        }
        GradientKind::Radial(pos) => {
            1u8.hash(hasher);
            hash_point_rel(hasher, rel, pos.start_center);
            quantize_f32(pos.start_radius).hash(hasher);
            hash_point_rel(hasher, rel, pos.end_center);
            quantize_f32(pos.end_radius).hash(hasher);
        }
        GradientKind::Sweep(pos) => {
            2u8.hash(hasher);
            hash_point_rel(hasher, rel, pos.center);
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

fn hash_brush(hasher: &mut DefaultHasher, brush: &Brush, rel: Affine) {
    mem::discriminant(brush).hash(hasher);
    match brush {
        Brush::Solid(color) => hash_color(hasher, color),
        Brush::Gradient(gradient) => hash_gradient(hasher, gradient, rel),
        Brush::Image(image_brush) => {
            hash_image_data(hasher, &image_brush.image);
            hash_image_sampler(hasher, &image_brush.sampler);
        }
    }
}

/// Hashes a glyph run's font/blob id, size, brush and transform (relative to
/// `base_inv`, via `rel = base_inv * run.transform`), and every glyph's id +
/// position MAPPED through that same `rel` — a glyph's `x`/`y` are absolute
/// in exactly the sense every other command's geometry is (see the module
/// docs), so a page whose slide lives in the glyph positions rather than in
/// `run.transform` fingerprints identically once `base` slides with it.
fn hash_glyph_run(hasher: &mut DefaultHasher, run: &GlyphRun, base_inv: Affine) {
    run.font.font().data.id().hash(hasher);
    run.font.font().index.hash(hasher);
    quantize_f32(run.font_size).hash(hasher);
    let rel = base_inv * run.transform;
    hash_brush(hasher, &run.brush, rel);
    hash_linear(hasher, rel);
    run.glyphs.len().hash(hasher);
    for glyph in &run.glyphs {
        glyph.id.hash(hasher);
        hash_point_rel(
            hasher,
            rel,
            Point::new(f64::from(glyph.x), f64::from(glyph.y)),
        );
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
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_brush(hasher, brush, rel);
            hash_linear(hasher, rel);
        }
        Command::RoundedRect {
            rect,
            radii,
            brush,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_corner_radii(hasher, radii);
            hash_brush(hasher, brush, rel);
            hash_linear(hasher, rel);
        }
        Command::Line {
            p0,
            p1,
            width,
            brush,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_point_rel(hasher, rel, *p0);
            hash_point_rel(hasher, rel, *p1);
            quantize(*width).hash(hasher);
            hash_brush(hasher, brush, rel);
            hash_linear(hasher, rel);
        }
        Command::GlyphRun(run) => hash_glyph_run(hasher, run, base_inv),
        Command::PushClip { rect, transform } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_linear(hasher, rel);
        }
        Command::PushClipRounded {
            rect,
            radii,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_corner_radii(hasher, radii);
            hash_linear(hasher, rel);
        }
        Command::PopClip => {}
        Command::Image {
            data,
            dest,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_image_data(hasher, data);
            hash_rect_rel(hasher, rel, dest);
            hash_linear(hasher, rel);
        }
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            color,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_corner_radii(hasher, radii);
            quantize(*std_dev).hash(hasher);
            hash_color(hasher, color);
            hash_linear(hasher, rel);
        }
        Command::PushLayer {
            rect,
            alpha,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            quantize_f32(*alpha).hash(hasher);
            hash_linear(hasher, rel);
        }
        Command::PopLayer => {}
        Command::ClearRect { rect, transform } => {
            let rel = base_inv * *transform;
            hash_rect_rel(hasher, rel, rect);
            hash_linear(hasher, rel);
        }
        Command::Path {
            path,
            style,
            brush,
            transform,
        } => {
            let rel = base_inv * *transform;
            hash_bez_path(hasher, path, rel);
            hash_path_style(hasher, style);
            hash_brush(hasher, brush, rel);
            hash_linear(hasher, rel);
        }
        Command::ShaderQuad {
            program,
            dest,
            transform,
            time,
        } => {
            let rel = base_inv * *transform;
            program.id().hash(hasher);
            hash_rect_rel(hasher, rel, dest);
            hash_linear(hasher, rel);
            quantize_f32(*time).hash(hasher);
        }
        Command::PushSnapshot {
            key,
            rect,
            alpha,
            scale,
            transform,
        } => {
            let rel = base_inv * *transform;
            key.hash(hasher);
            hash_rect_rel(hasher, rel, rect);
            quantize_f32(*alpha).hash(hasher);
            quantize(*scale).hash(hasher);
            hash_linear(hasher, rel);
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

    fn shifted_rect(rect: Rect, dx: f64, dy: f64) -> Rect {
        Rect::new(rect.x0 + dx, rect.y0 + dy, rect.x1 + dx, rect.y1 + dy)
    }

    fn shifted_point(p: Point, dx: f64, dy: f64) -> Point {
        Point::new(p.x + dx, p.y + dy)
    }

    /// A body recorded entirely at ABSOLUTE positions (identity command
    /// transforms throughout, mirroring `ChildPod::paint_child` baking a
    /// widget's origin directly into its geometry) shifted by `(dx, dy)` —
    /// one instance of every geometry-bearing command the module hashes:
    /// a rect, a rounded rect, a line, a path (translated like `path_at`
    /// does), a glyph run with absolute glyph positions, and a linear
    /// gradient fill.
    fn record_absolute_body(font: &FontHandle, dx: f64, dy: f64) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);

        builder.fill_rect(
            shifted_rect(Rect::new(0.0, 0.0, 10.0, 10.0), dx, dy),
            red_brush(),
        );
        builder.fill_rounded_rect(
            shifted_rect(Rect::new(20.0, 0.0, 30.0, 10.0), dx, dy),
            2.0,
            red_brush(),
        );
        builder.stroke_line(
            shifted_point(Point::new(0.0, 20.0), dx, dy),
            shifted_point(Point::new(10.0, 20.0), dx, dy),
            1.5,
            red_brush(),
        );

        let mut path = BezPath::new();
        path.move_to((0.0, 30.0));
        path.line_to((10.0, 30.0));
        path.line_to((5.0, 40.0));
        path.close_path();
        // `Affine::translate * path`, the same shape `path_at` builds.
        builder.fill_path(Affine::translate((dx, dy)) * path, red_brush());

        builder.draw_glyph_run(GlyphRun {
            font: font.clone(),
            font_size: 16.0,
            brush: red_brush(),
            transform: Affine::IDENTITY,
            glyphs: vec![
                Glyph {
                    id: 1,
                    x: (0.0 + dx) as f32,
                    y: (50.0 + dy) as f32,
                },
                Glyph {
                    id: 2,
                    x: (8.0 + dx) as f32,
                    y: (50.0 + dy) as f32,
                },
            ],
        });

        let gradient = Gradient::new_linear(
            shifted_point(Point::new(0.0, 60.0), dx, dy),
            shifted_point(Point::new(10.0, 60.0), dx, dy),
        );
        builder.fill_rect(
            shifted_rect(Rect::new(0.0, 60.0, 10.0, 70.0), dx, dy),
            Brush::Gradient(gradient),
        );

        scene
    }

    /// The exact mechanism the fix targets: a body's geometry sliding by
    /// `(dx, dy)` — absolute coordinates changing every frame, identity
    /// command transforms throughout — fingerprints EQUAL to the unslid body
    /// once `base` slides by the same `(dx, dy)`, and fingerprints DIFFERENT
    /// from it when `base` stays put.
    #[test]
    fn absolute_coordinate_slide_with_an_equally_translated_base_keeps_the_fingerprint() {
        let font = empty_font();
        let base_a = Affine::translate((5.0, 7.0));
        let (dx, dy) = (3.0, -2.0);
        let base_a_plus_delta = Affine::translate((5.0 + dx, 7.0 + dy));

        let a = record_absolute_body(&font, 0.0, 0.0);
        let b = record_absolute_body(&font, dx, dy);

        let fp_a = fingerprint_commands(a.commands(), base_a);
        let fp_b_shifted_base = fingerprint_commands(b.commands(), base_a_plus_delta);
        let fp_b_same_base = fingerprint_commands(b.commands(), base_a);

        assert_eq!(fp_a, fp_b_shifted_base);
        assert_ne!(fp_a, fp_b_same_base);
    }

    /// A body under a scaled `base` must differ from the same body under an
    /// unscaled `base` — even here, where the body's only geometry sits
    /// exactly at the bracket's own origin `(0, 0)` (so mapping alone cannot
    /// tell the two `base`s apart: `rel * (0, 0)` is `(0, 0)` regardless of
    /// `rel`'s linear part). Only the separately-hashed linear part of `rel`
    /// disambiguates the scale here.
    #[test]
    fn scaled_base_changes_the_fingerprint_even_when_the_bodys_geometry_sits_at_the_origin() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.stroke_line(Point::ORIGIN, Point::ORIGIN, 4.0, red_brush());

        assert_ne!(
            fingerprint_commands(scene.commands(), Affine::IDENTITY),
            fingerprint_commands(scene.commands(), Affine::scale(2.0)),
        );
    }

    /// Two glyph runs whose slide lives in different places — one in the
    /// run's own `transform` (composed the way `push_transform` composes
    /// it), the other in the glyphs' own `x`/`y` — fingerprint EQUAL relative
    /// to the same, correspondingly translated `base`.
    #[test]
    fn glyph_slide_via_transform_and_via_glyph_position_hash_equal_relative_to_the_same_base() {
        let font = empty_font();
        let shift = Affine::translate((6.0, -9.0));
        let glyphs_at = |x: f32, y: f32| {
            vec![
                Glyph { id: 1, x, y },
                Glyph {
                    id: 2,
                    x: x + 8.0,
                    y,
                },
            ]
        };

        // Slide lives in the run's own transform; glyph positions stay local.
        let mut via_transform = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut via_transform);
            builder.push_transform(shift);
            builder.draw_glyph_run(GlyphRun {
                font: font.clone(),
                font_size: 16.0,
                brush: red_brush(),
                transform: Affine::IDENTITY,
                glyphs: glyphs_at(0.0, 0.0),
            });
        }

        // Slide lives in the glyph positions themselves (the bug this card
        // fixes); the run's own transform stays identity.
        let mut via_geometry = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut via_geometry);
            builder.draw_glyph_run(GlyphRun {
                font: font.clone(),
                font_size: 16.0,
                brush: red_brush(),
                transform: Affine::IDENTITY,
                glyphs: glyphs_at(6.0, -9.0),
            });
        }

        assert_eq!(
            fingerprint_commands(via_transform.commands(), shift),
            fingerprint_commands(via_geometry.commands(), shift),
        );
    }
}
