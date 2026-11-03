//! The glyph-atlas insert for `hvgl`-only faces, done frust-side because
//! `glifo` cannot read their outlines (see [`super::hvgl`]).
//!
//! `glifo` caches an outline glyph in four steps: key it, measure its raster
//! extent from the outline's bounds, record a black fill of the outline into
//! its slot's page recorder, and draw the slot as a tinted coverage mask. The
//! keys and the allocation are the policy's ([`AtlasPolicy::collect_run`] keys
//! glyphs exactly as `glifo` keys an unhinted outline, [`AtlasPolicy::build`]
//! places them); the other three steps are private to `glifo` and restated
//! here over its public API, each pointing at its source: [`raster_metrics`],
//! [`record_outline`] and [`draw_from_atlas`]. The entries are therefore
//! indistinguishable from `glifo`'s to everything downstream — the resident-run
//! cache, glyph quads, eviction, clears and the page replay. Unhinted, at the
//! run's absorbed device size and variation coordinates, with neither embolden
//! nor skew (the provider's synthesis asymmetry); faces keyed by [`face_id`].
//!
//! # Pin-bound
//!
//! The restatements are correct for the pinned `glifo` 0.3.0 only. A bump that
//! moves one would put these glyphs on other pixels than `glifo`'s own without
//! failing to compile; the Metal parity golden for this route (PingFang atlas
//! against outlines, with a CFF Hiragino control `glifo` inserts itself) is the
//! tripwire, re-run with any `glifo` pin move.

use frust_scene::{FontHandle, GlyphRun};
use glifo::{AtlasCommandRecorder, AtlasConfig, AtlasSlot, DrawSink as _, GlyphAtlas};
use glifo::{GlyphCacheKey, GlyphRenderer as _, RasterMetrics};
use kurbo::{Affine, BezPath, Rect, Shape as _, Vec2};
use peniko::color::palette::css::BLACK;
use peniko::color::{AlphaColor, Srgb};
use peniko::{Extend, ImageQuality, ImageSampler};
use vello_common::paint::{Image, Tint, TintMode};

use super::ResidentPlan;
use super::atlas_policy::{AtlasPolicy, GlyphUpload, KeyedSlot, subpixel_bucket};
use super::backend::EngineGlyphSink;
use super::hvgl::{self, GlyphOutline, HvglOutlines};

/// The face identity an `hvgl` run's atlas keys carry in place of a blob id:
/// the blob id, the collection index and the PostScript name the provider opens
/// CoreText by, hashed (FNV-1a) and tagged with the top bit. The name is part
/// of it because the outlines come from the face that name opens, not from the
/// bytes; the tag keeps it clear of every blob id `glifo` keys its own entries
/// on, which count up from zero.
#[must_use]
pub(crate) fn face_id(handle: &FontHandle) -> u64 {
    let font = handle.font();
    let name = handle.postscript_name().unwrap_or_default();
    let bytes = font
        .data
        .id()
        .to_le_bytes()
        .into_iter()
        .chain(font.index.to_le_bytes())
        .chain(name.bytes());
    let hash = bytes.fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    hash | (1 << 63)
}

/// The faces the collect walk sent to the insert this frame, so the measuring
/// callback and the upload pass can find the handle a key was built from.
///
/// One entry per face and variation instance — a frame draws a handful — found
/// by a scan. Cleared at the head of every frame; kept for its allocation.
#[derive(Debug, Default)]
pub(crate) struct CollectedFaces {
    /// `(face_id, handle)` per face and coordinates.
    faces: Vec<(u64, FontHandle)>,
}

impl CollectedFaces {
    /// Forget the previous frame's faces.
    pub(crate) fn clear(&mut self) {
        self.faces.clear();
    }

    /// Note that a run of `handle`, keyed under `id`, was collected.
    pub(crate) fn note(&mut self, id: u64, handle: &FontHandle) {
        let coords = handle.normalized_coords();
        if !self
            .faces
            .iter()
            .any(|(known, face)| *known == id && face.normalized_coords() == coords)
        {
            self.faces.push((id, handle.clone()));
        }
    }

    /// The handle `key` was built from, if a collected run carried it.
    fn handle(&self, key: &GlyphCacheKey) -> Option<&FontHandle> {
        let coords: &[i16] = bytemuck::cast_slice(&key.var_coords[..]);
        self.faces
            .iter()
            .find(|(id, face)| *id == key.font_id && face.normalized_coords() == coords)
            .map(|(_, face)| face)
    }
}

/// What one collected glyph measures.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Measured {
    /// Its raster extent and bearings.
    Ink(RasterMetrics),
    /// It has no ink: no slot, and remembered as inkless.
    Inkless,
    /// The provider has no outline for it; it is drawn (and counted skipped)
    /// on the outline route.
    Unavailable,
}

/// `key`'s glyph measured from its provider outline at the key's size —
/// `glifo`'s own measurement of an outline it caches: the outline's bounds
/// scaled from units-per-em to the size, then [`raster_metrics`].
pub(crate) fn measure(
    key: &GlyphCacheKey,
    faces: &CollectedFaces,
    outlines: &mut HvglOutlines,
) -> Measured {
    let Some(handle) = faces.handle(key) else {
        return Measured::Unavailable;
    };
    match outlines.outline(handle, key.glyph_id) {
        GlyphOutline::Path { path, units_per_em } => {
            let scale = outline_scale(key, units_per_em);
            Measured::Ink(raster_metrics(
                &path.bounding_box().scale_from_origin(scale),
            ))
        }
        GlyphOutline::Empty => Measured::Inkless,
        GlyphOutline::Unavailable => Measured::Unavailable,
    }
}

/// Font units to pixels at `key`'s size.
fn outline_scale(key: &GlyphCacheKey, units_per_em: f64) -> f64 {
    f64::from(f32::from_bits(key.size_bits)) / units_per_em
}

/// Record every upload of the frame's atlas pass into its page's recorder,
/// answering how many were recorded. An upload whose outline cannot be found
/// again records nothing; its slot stays blank until the entry ages out.
pub(crate) fn rasterize(
    uploads: &[GlyphUpload],
    faces: &CollectedFaces,
    outlines: &mut HvglOutlines,
    atlas: &mut GlyphAtlas,
    pages: AtlasConfig,
) -> u32 {
    let (width, height) = pages.atlas_size;
    let (width, height) = (
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(height).unwrap_or(u16::MAX),
    );
    let mut recorded = 0_u32;
    for upload in uploads {
        let Some(handle) = faces.handle(&upload.key) else {
            continue;
        };
        let GlyphOutline::Path { path, units_per_em } =
            outlines.outline(handle, upload.key.glyph_id)
        else {
            continue;
        };
        let recorder = atlas.recorder_for_page(upload.slot.page_index, width, height);
        record_outline(
            recorder,
            path,
            outline_scale(&upload.key, units_per_em),
            glifo::atlas::key::subpixel_offset(upload.key.subpixel_x),
            upload.slot,
        );
        recorded = recorded.saturating_add(1);
    }
    recorded
}

/// `glifo`'s `calculate_raster_metrics` (glifo 0.3.0 renderer.rs:591): the
/// whole-pixel extent of `bounds` (pixels, y up), one extra column for the
/// subpixel offset rasterized into the slot, y flipped to the atlas's y down.
#[must_use]
pub(crate) fn raster_metrics(bounds: &Rect) -> RasterMetrics {
    let min_x = bounds.x0.floor() as i32;
    let max_x = bounds.x1.ceil() as i32 + 1;
    let flipped_min_y = (-bounds.y1).floor() as i32;
    let flipped_max_y = (-bounds.y0).ceil() as i32;
    RasterMetrics {
        width: (max_x - min_x) as u16,
        height: (flipped_max_y - flipped_min_y) as u16,
        bearing_x: min_x as i16,
        bearing_y: flipped_min_y as i16,
    }
}

/// The transform [`record_outline`] sets (glifo 0.3.0 renderer.rs:342): font
/// units scaled by `scale` and flipped to y down, then translated to the slot's
/// atlas corner `atlas` less its `bearing`, plus the bucket's `subpixel` in x.
fn outline_transform(scale: f64, atlas: (u16, u16), bearing: (i16, i16), subpixel: f32) -> Affine {
    Affine::scale_non_uniform(scale, -scale).then_translate(Vec2::new(
        f64::from(atlas.0) - f64::from(bearing.0) + f64::from(subpixel),
        f64::from(atlas.1) - f64::from(bearing.1),
    ))
}

/// `glifo`'s `render_outline_to_atlas` (glifo 0.3.0 renderer.rs:342): the
/// outline (font units, y up) scaled and flipped into `slot`, shifted by the
/// bucket's `subpixel` offset, filled black.
fn record_outline(
    recorder: &mut AtlasCommandRecorder,
    path: &BezPath,
    scale: f64,
    subpixel: f32,
    slot: AtlasSlot,
) {
    recorder.set_transform(outline_transform(
        scale,
        (slot.x, slot.y),
        (slot.bearing_x, slot.bearing_y),
        subpixel,
    ));
    recorder.set_paint(BLACK.into());
    recorder.fill_path(path);
}

/// The placement [`draw_from_atlas`] sets (glifo 0.3.0 renderer.rs:521): the
/// origin's whole-pixel floor plus the slot's `bearing`, so a negative origin
/// rounds toward minus infinity rather than toward zero.
fn glyph_rect_transform(origin: (f64, f64), bearing: (i16, i16)) -> Affine {
    Affine::translate((
        origin.0.floor() + f64::from(bearing.0),
        origin.1.floor() + f64::from(bearing.1),
    ))
}

/// `glifo`'s `render_outline_glyph_from_atlas` over `render_from_atlas`
/// (glifo 0.3.0 renderer.rs:521 and :311): `slot`'s rectangle placed at
/// `floor(origin) + bearing`, sampled nearest, tinted by `color` as an alpha
/// mask.
fn draw_from_atlas(
    sink: &mut EngineGlyphSink<'_>,
    slot: AtlasSlot,
    origin: (f64, f64),
    color: AlphaColor<Srgb>,
) {
    let rect_transform = glyph_rect_transform(origin, (slot.bearing_x, slot.bearing_y));
    let area = Rect::new(0.0, 0.0, f64::from(slot.width), f64::from(slot.height));
    let paint_transform = sink.atlas_paint_transform(&slot);
    let image = Image {
        image: sink.atlas_image_source(&slot),
        sampler: ImageSampler {
            x_extend: Extend::Pad,
            y_extend: Extend::Pad,
            quality: ImageQuality::Low,
            alpha: 1.0,
        },
    };
    let state = sink.save_state();
    sink.set_tint(Some(Tint {
        color,
        mode: TintMode::AlphaMask,
    }));
    sink.set_transform(rect_transform);
    sink.set_paint_image(image);
    sink.set_paint_transform(paint_transform);
    sink.fill_rect(&area);
    sink.set_tint(None);
    sink.restore_state(state);
}

/// Draw every glyph of `run` that the resident-run cache did not: resident
/// glyphs from their slots, inkless ones as nothing, and the rest — a glyph
/// the atlas had no room for, or one the provider could not measure — filled
/// from the provider's outline under the run's device `transform`, as the
/// outline route draws them. Answers how many glyphs could not be drawn.
pub(crate) fn draw_run(
    run: &GlyphRun,
    transform: Affine,
    plan: &ResidentPlan<'_>,
    sink: &mut EngineGlyphSink<'_>,
    policy: &mut AtlasPolicy,
    outlines: &mut HvglOutlines,
) -> u32 {
    let identity = plan.identity();
    let color = plan.color();
    let mut unavailable = 0_u32;
    for glyph in plan.glyphs() {
        let origin = plan.origin(glyph);
        // `glifo` keys the fraction of the f64 origin, narrowed.
        let bucket = subpixel_bucket(origin.0.fract() as f32);
        let key = policy.glyph_key(&identity, glyph.id, bucket);
        match policy.keyed_slot(&key) {
            KeyedSlot::Resident(slot) => draw_from_atlas(sink, slot, origin, color),
            KeyedSlot::Inkless => {}
            KeyedSlot::Absent => {
                if !hvgl::draw_glyph(run, transform, glyph, sink, outlines) {
                    unavailable = unavailable.saturating_add(1);
                }
            }
        }
    }
    unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The restated extent is `glifo`'s on the cases its doc names: a pixel of
    /// subpixel room on the right, y flipped, outward rounding.
    #[test]
    fn raster_metrics_round_outward_with_a_subpixel_column() {
        let metrics = raster_metrics(&Rect::new(0.25, -2.5, 9.75, 11.2));
        assert_eq!(
            (
                metrics.width,
                metrics.height,
                metrics.bearing_x,
                metrics.bearing_y
            ),
            (11, 15, 0, -12)
        );
    }

    /// Two handles over one blob naming different faces never share a key,
    /// and no face id can equal a small blob id.
    #[test]
    fn face_ids_separate_names_and_stay_clear_of_blob_ids() {
        let blob = peniko::Blob::new(std::sync::Arc::new(vec![0_u8; 16]));
        let face = |name: &str| {
            FontHandle::new(peniko::FontData::new(blob.clone(), 0))
                .with_identity(Some(std::sync::Arc::from(name)), false)
        };
        let medium = face_id(&face("PingFangSC-Medium"));
        let regular = face_id(&face("PingFangSC-Regular"));
        assert_ne!(medium, regular);
        assert_eq!(medium, face_id(&face("PingFangSC-Medium")));
        assert!(medium >> 63 == 1 && regular >> 63 == 1);
    }

    /// `(width, height, bearing_x, bearing_y)` of a [`raster_metrics`] answer.
    fn extent(bounds: &Rect) -> (u16, u16, i16, i16) {
        let metrics = raster_metrics(bounds);
        (
            metrics.width,
            metrics.height,
            metrics.bearing_x,
            metrics.bearing_y,
        )
    }

    /// `glifo`'s `calculate_raster_metrics` (glifo 0.3.0 renderer.rs:591),
    /// worked by hand: x is `floor(x0)` to `ceil(x1) + 1`; y is `floor(-y1)`
    /// to `ceil(-y0)`, the flipped extent.
    #[test]
    fn raster_metrics_golden_negative_origin() {
        // x: floor(-3.5) = -4, ceil(-0.5) + 1 = 1, width 5.
        // y: floor(-2.0) = -2, ceil(4.25) = 5, height 7.
        assert_eq!(extent(&Rect::new(-3.5, -4.25, -0.5, 2.0)), (5, 7, -4, -2));
        // Entirely below the baseline and left of the origin: bearing_y > 0.
        // x: floor(-2) = -2, ceil(-1) + 1 = 0, width 2.
        // y: floor(1) = 1, ceil(2) = 2, height 1.
        assert_eq!(extent(&Rect::new(-2.0, -2.0, -1.0, -1.0)), (2, 1, -2, 1));
    }

    /// Fractional extents round outward on every side, straddling the origin.
    #[test]
    fn raster_metrics_golden_fractional_extent() {
        // x: floor(0.9) = 0, ceil(1.1) + 1 = 3, width 3.
        // y: floor(-0.9) = -1, ceil(0.1) = 1, height 2.
        assert_eq!(extent(&Rect::new(0.9, -0.1, 1.1, 0.9)), (3, 2, 0, -1));
        // A quarter-pixel glyph still takes the subpixel column and one row.
        // x: floor(0.25) = 0, ceil(0.75) + 1 = 2, width 2.
        // y: floor(-0.5) = -1, ceil(0) = 0, height 1.
        assert_eq!(extent(&Rect::new(0.25, 0.0, 0.75, 0.5)), (2, 1, 0, -1));
    }

    /// Whole-pixel bounds: no rounding, but the extra subpixel column stays.
    #[test]
    fn raster_metrics_golden_integer_bounds() {
        // x: floor(1) = 1, ceil(4) + 1 = 5, width 4.
        // y: floor(-6) = -6, ceil(-2) = -2, height 4.
        assert_eq!(extent(&Rect::new(1.0, 2.0, 4.0, 6.0)), (4, 4, 1, -6));
    }

    /// Zero-area bounds: a point keeps the one subpixel column and no rows;
    /// the empty rectangle at the origin is the same shape at zero.
    #[test]
    fn raster_metrics_golden_zero_area() {
        // x: floor(5) = 5, ceil(5) + 1 = 6, width 1.
        // y: floor(-5) = -5, ceil(-5) = -5, height 0.
        assert_eq!(extent(&Rect::new(5.0, 5.0, 5.0, 5.0)), (1, 0, 5, -5));
        assert_eq!(extent(&Rect::ZERO), (1, 0, 0, 0));
    }

    /// A 30000 px square still fits `u16` extents and `i16` bearings.
    #[test]
    fn raster_metrics_golden_very_large() {
        // x: floor(0) = 0, ceil(30000) + 1 = 30001, width 30001 (< 65535).
        // y: floor(-30000) = -30000, ceil(0) = 0, height 30000; -30000 > i16::MIN.
        assert_eq!(
            extent(&Rect::new(0.0, 0.0, 30000.0, 30000.0)),
            (30001, 30000, 0, -30000)
        );
    }

    /// `glifo`'s `quantize_subpixel` (glifo 0.3.0 key.rs:196-203) with its
    /// 4 buckets (key.rs:24), worked by hand: the fraction is wrapped into
    /// `[0, 1)`, scaled by 4, rounded, and clamped to bucket 3.
    #[test]
    fn subpixel_bucket_golden_matches_glifo_quantization() {
        let cases: [(f32, u8); 11] = [
            (0.0, 0),
            (0.1, 0),   // 0.4 -> 0
            (0.2, 1),   // 0.8 -> 1
            (0.4, 2),   // 1.6 -> 2
            (0.5, 2),   // 2.0 -> 2
            (0.7, 3),   // 2.8 -> 3
            (0.9, 3),   // 3.6 -> 4, clamped to 3
            (1.0, 0),   // fract 0.0
            (1.5, 2),   // fract 0.5 -> 2
            (-0.25, 3), // wraps to 0.75 -> 3
            (-0.9, 0),  // wraps to 0.1 -> 0.4 -> 0
        ];
        for (fraction, bucket) in cases {
            assert_eq!(subpixel_bucket(fraction), bucket, "fraction {fraction}");
        }
    }

    /// The bucket's offset, as `glifo`'s `subpixel_offset` (key.rs:208-210)
    /// returns it: `bucket / 4`. A 0.9 fraction lands on the 0.75 offset.
    #[test]
    fn subpixel_offset_golden_for_a_bucketed_fraction() {
        assert_eq!(
            glifo::atlas::key::subpixel_offset(subpixel_bucket(0.9)),
            0.75
        );
        assert_eq!(
            glifo::atlas::key::subpixel_offset(subpixel_bucket(0.5)),
            0.5
        );
    }

    /// The transform `record_outline` sets (glifo 0.3.0 renderer.rs:342), by
    /// hand: x' = x * s + (slot.x - bearing_x + subpixel), y' = -y * s + (slot.y - bearing_y).
    #[test]
    fn outline_transform_golden_placement() {
        // scale 0.5, slot (10, 20), bearing (-3, -7), subpixel 0.25:
        // tx = 10 + 3 + 0.25 = 13.25; ty = 20 + 7 = 27.
        assert_eq!(
            outline_transform(0.5, (10, 20), (-3, -7), 0.25).as_coeffs(),
            [0.5, 0.0, 0.0, -0.5, 13.25, 27.0]
        );
        // The point (100, 200) font units: x' = 50 + 13.25 = 63.25, y' = -100 + 27 = -73.
        let point =
            outline_transform(0.5, (10, 20), (-3, -7), 0.25) * kurbo::Point::new(100.0, 200.0);
        assert_eq!((point.x, point.y), (63.25, -73.0));
        // Unit scale at the atlas origin, positive bearing, half-pixel subpixel:
        // tx = 0 - 2 + 0.5 = -1.5; ty = 0 - 4 = -4.
        assert_eq!(
            outline_transform(1.0, (0, 0), (2, 4), 0.5).as_coeffs(),
            [1.0, 0.0, 0.0, -1.0, -1.5, -4.0]
        );
    }

    /// The placement `draw_from_atlas` sets (glifo 0.3.0 renderer.rs:521):
    /// tx = floor(origin.x) + bearing_x, ty = floor(origin.y) + bearing_y.
    #[test]
    fn glyph_rect_transform_golden_floors_the_origin() {
        // floor(10.7) + 2 = 12; floor(-3.2) + -4 = -8.
        assert_eq!(
            glyph_rect_transform((10.7, -3.2), (2, -4)).as_coeffs(),
            [1.0, 0.0, 0.0, 1.0, 12.0, -8.0]
        );
        // A negative fraction floors toward minus infinity: floor(-0.5) = -1.
        assert_eq!(
            glyph_rect_transform((-0.5, 0.0), (0, 0)).as_coeffs(),
            [1.0, 0.0, 0.0, 1.0, -1.0, 0.0]
        );
    }
}
