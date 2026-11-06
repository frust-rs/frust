//! Lowering for [`Command::SceneTexture`](frust_scene::Command::SceneTexture):
//! a caller-owned GPU texture drawn over a destination rectangle.
//!
//! The command is the scene layer's one reference to a texture the engine did
//! not produce. It carries an opaque `u64` and a destination rectangle and
//! nothing else — no pixels, no extent, no sampler — because the scene layer
//! depends on no GPU crate (see `frust_scene::Command::SceneTexture`). So the
//! two halves of the lowering sit on opposite sides of the frame:
//!
//! 1. the *extent* is registry state, learned when the host registers the
//!    texture and kept here in [`ExternalExtents`] so the walk can compose the
//!    same natural-pixels-onto-`dest` mapping [`Command::Image`] uses; and
//! 2. the *view* is the renderer's, resolved after compiling against the same
//!    registry (see [`crate::gpu::bindings`]).
//!
//! The split is what keeps this module free of `wgpu`, like the rest of
//! `compile`. It is also why an unregistered id is decided here rather than at
//! encode time: with no extent there is no mapping to compose, so the draw
//! never reaches the frame's paint table at all.
//!
//! Unlike an atlas-backed image, nothing is made resident and nothing is
//! uploaded — the texels are the caller's, already on the device. The encoded
//! entry is therefore a plain [`EncodedExternalTexture`] naming the id, the
//! source region and the inverse device-to-texel transform the shader applies.

use std::collections::{HashMap, HashSet};

use peniko::{ImageQuality, ImageSampler};
use vello_common::TextureId;
use vello_common::encode::{EncodedExternalTexture, EncodedPaint};
use vello_common::geometry::RectU16;
use vello_common::kurbo::{Affine, Rect};
use vello_common::paint::{IndexedPaint, Paint};

use crate::compile::paint::resolved_quality;
use crate::gpu::atlas::natural_to_dest;

/// How a texture is filtered when it is drawn at a scale other than 1:1.
///
/// An unscaled whole-pixel placement is always sampled exactly (nearest),
/// whatever this says — the same downgrade an image takes, because the four
/// taps a bilinear filter blends are one texel there.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SceneTextureFilter {
    /// Bilinear wherever the placement is scaled or fractional.
    #[default]
    Linear,
    /// Nearest at every scale: magnified texels stay hard-edged blocks.
    Nearest,
    /// Nearest while the texture is magnified on both axes (fewer than one
    /// texel per device pixel), bilinear otherwise.
    NearestWhenMagnified,
}

/// Registration-side hints for an externally bound texture.
///
/// The scene command carries neither; they are state of the registration, set
/// when the host binds the texture.
///
/// `opaque` is a caller assertion. The engine never reads the texels, so
/// claiming opacity for a texture holding alpha below one draws it wrongly: it
/// occludes what it should blend over. Opaque draws take the same route an
/// opaque image takes.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneTextureHints {
    /// The sampling filter.
    pub filter: SceneTextureFilter,
    /// Whether the caller asserts every texel is fully opaque.
    pub opaque: bool,
}

impl SceneTextureHints {
    /// The defaults: linear filtering, possibly transparent.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            filter: SceneTextureFilter::Linear,
            opaque: false,
        }
    }

    /// These hints with `filter` as the sampling filter.
    #[must_use]
    pub const fn with_filter(mut self, filter: SceneTextureFilter) -> Self {
        self.filter = filter;
        self
    }

    /// These hints with the caller's opacity assertion set to `opaque`.
    #[must_use]
    pub const fn assume_opaque(mut self, opaque: bool) -> Self {
        self.opaque = opaque;
        self
    }
}

/// The texel extent of every externally bound texture, keyed by the opaque id
/// a [`Command::SceneTexture`](frust_scene::Command::SceneTexture) names it by.
///
/// A projection of the renderer's own registry, maintained by the one call that
/// owns both halves (`EngineRenderer::bind_texture`/`unbind_texture`), so the
/// two can never disagree about which ids are live. Only the extent crosses
/// over: the view stays on the GPU side, which is what keeps `compile` free of
/// `wgpu`.
///
/// A compiler built standalone starts empty, which reads as "nothing is bound"
/// — every `SceneTexture` then draws nothing, exactly as an unregistered id
/// does.
#[derive(Debug, Default)]
pub struct ExternalExtents {
    extents: HashMap<u64, ((u16, u16), SceneTextureHints)>,
    /// Ids already reported as unregistered, so a scene that draws one every
    /// frame says so once rather than once per frame.
    warned: HashSet<u64>,
}

impl ExternalExtents {
    /// An empty map — nothing bound.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `id` as bound at `size` texels, returning whether the extent is
    /// usable at all.
    ///
    /// A dimension past `u16::MAX` is refused rather than truncated: the record
    /// the shader reads packs the source region into `u16` halves, so a
    /// truncated extent would sample a rectangle the caller never named. A
    /// zero dimension is refused for the same reason a zero-sized image is —
    /// there is no scale to map it onto `dest` with.
    pub fn bind(&mut self, id: u64, size: (u32, u32)) -> bool {
        self.bind_with(id, size, SceneTextureHints::default())
    }

    /// [`Self::bind`] with registration-side `hints`; rebinding replaces them.
    pub fn bind_with(&mut self, id: u64, size: (u32, u32), hints: SceneTextureHints) -> bool {
        let (Ok(width), Ok(height)) = (u16::try_from(size.0), u16::try_from(size.1)) else {
            self.extents.remove(&id);
            return false;
        };
        if width == 0 || height == 0 {
            self.extents.remove(&id);
            return false;
        }
        self.extents.insert(id, ((width, height), hints));
        // A freshly bound id is worth reporting again if it is later unbound
        // while a scene still draws it.
        self.warned.remove(&id);
        true
    }

    /// Forgets `id`, if it was bound.
    pub fn unbind(&mut self, id: u64) {
        self.extents.remove(&id);
    }

    /// The texel extent bound under `id`, if any.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<(u16, u16)> {
        self.extents.get(&id).map(|(extent, _)| *extent)
    }

    /// The hints bound under `id`, if any.
    #[must_use]
    pub fn hints(&self, id: u64) -> Option<SceneTextureHints> {
        self.extents.get(&id).map(|(_, hints)| *hints)
    }

    /// How many textures are bound.
    #[must_use]
    pub fn len(&self) -> usize {
        self.extents.len()
    }

    /// Whether nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.extents.is_empty()
    }

    /// Whether `id` has not been reported unregistered yet, latching it so the
    /// next ask answers `false`.
    fn take_warning(&mut self, id: u64) -> bool {
        self.warned.insert(id)
    }
}

/// Why a [`Command::SceneTexture`](frust_scene::Command::SceneTexture) recorded
/// no draw.
///
/// Every variant is a skipped draw, never a refused frame: the engine's
/// standing rule is that a frame draws less rather than wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalSkip {
    /// No texture is bound under the id the command named.
    Unregistered {
        /// The id the display list asked for.
        id: u64,
    },
    /// The destination rectangle has no area to scale the texture onto.
    DegenerateDest,
    /// The composed transform has no finite inverse, so device space cannot be
    /// mapped back to texels.
    SingularTransform,
}

/// One encoded external-texture paint: the paint a draw references, and where
/// its entry landed in the frame's paint table.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalEncoding {
    /// The paint to record against the draw.
    pub paint: Paint,
    /// Index of the entry in the encoded-paint side table.
    pub paint_index: usize,
}

/// Encode a `Command::SceneTexture`: the whole of the texture bound under `id`
/// scaled to fill `dest`, under `transform`.
///
/// The natural-to-`dest` composition is [`natural_to_dest`], the same one
/// [`Command::Image`](frust_scene::Command::Image) takes, so a texture and an
/// image drawn over the same rectangle land on identical device pixels. The
/// sampler is the registration's [`SceneTextureHints`]: bilinear
/// ([`ImageQuality::Medium`]) by default, resolved through the same
/// whole-pixel downgrade images take, or nearest as the filter hint asks.
///
/// The entry is marked as possibly transparent unless the registration asserts
/// opacity. The engine never reads the caller's texels, so an unasserted
/// texture goes to the blended pass; an asserted one takes the route an opaque
/// image takes.
///
/// # Errors
///
/// Returns the [`ExternalSkip`] the draw is dropped for. An unregistered id is
/// reported once per id at warning level and at debug level thereafter — the
/// same shape the image residency's own refusals take.
pub fn encode_scene_texture(
    id: u64,
    dest: Rect,
    transform: Affine,
    extents: &mut ExternalExtents,
    encoded_paints: &mut Vec<EncodedPaint>,
) -> Result<ExternalEncoding, ExternalSkip> {
    let (Some((width, height)), Some(hints)) = (extents.get(id), extents.hints(id)) else {
        note_unregistered(id, extents.take_warning(id));
        return Err(ExternalSkip::Unregistered { id });
    };

    let paint_transform = natural_to_dest(transform, (u32::from(width), u32::from(height)), dest)
        .ok_or(ExternalSkip::DegenerateDest)?;
    let inverse = paint_transform.inverse();
    if !inverse.as_coeffs().iter().all(|coeff| coeff.is_finite()) {
        return Err(ExternalSkip::SingularTransform);
    }

    let quality = match hints.filter {
        SceneTextureFilter::Linear => resolved_quality(ImageQuality::Medium, paint_transform),
        SceneTextureFilter::Nearest => ImageQuality::Low,
        SceneTextureFilter::NearestWhenMagnified if magnifies(paint_transform) => ImageQuality::Low,
        SceneTextureFilter::NearestWhenMagnified => {
            resolved_quality(ImageQuality::Medium, paint_transform)
        }
    };

    let paint_index = encoded_paints.len();
    encoded_paints.push(EncodedPaint::ExternalTexture(EncodedExternalTexture {
        texture_id: TextureId(id),
        source_region: RectU16 {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        },
        sampler: ImageSampler {
            quality,
            ..ImageSampler::default()
        },
        may_have_transparency: !hints.opaque,
        transform: inverse,
        tint: None,
    }));

    Ok(ExternalEncoding {
        paint: Paint::Indexed(IndexedPaint::new(paint_index)),
        paint_index,
    })
}

/// Whether `paint_transform` (natural texels to device) magnifies on both axes:
/// fewer than one texel per device pixel along each of its two basis vectors.
fn magnifies(paint_transform: Affine) -> bool {
    let c = paint_transform.as_coeffs();
    let x_scale = c[0].hypot(c[1]);
    let y_scale = c[2].hypot(c[3]);
    x_scale > 1.0 && y_scale > 1.0
}

/// Report a `SceneTexture` naming an id nothing is bound under.
///
/// Warning on the first sighting of each id, debug on every later one: a
/// texture registered a frame late is an ordinary start-up shape and would
/// otherwise flood the log, while a texture never registered at all is an
/// invisible blank the caller has no other signal for.
fn note_unregistered(id: u64, first: bool) {
    if first {
        log::warn!(
            "SceneTexture {id} draws nothing: no texture is registered under that id \
             (further sightings of this id are logged at debug level)"
        );
    } else {
        log::debug!("SceneTexture {id} draws nothing: no texture is registered under that id");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEST: Rect = Rect::new(0.0, 0.0, 8.0, 4.0);

    fn bound(size: (u32, u32)) -> ExternalExtents {
        let mut extents = ExternalExtents::new();
        assert!(extents.bind(7, size));
        extents
    }

    #[test]
    fn an_unregistered_id_encodes_nothing() {
        let mut extents = ExternalExtents::new();
        let mut paints = Vec::new();

        let skip = encode_scene_texture(7, DEST, Affine::IDENTITY, &mut extents, &mut paints);

        assert_eq!(skip, Err(ExternalSkip::Unregistered { id: 7 }));
        assert!(paints.is_empty(), "a skipped draw leaves no orphan entry");
    }

    #[test]
    fn an_unregistered_id_is_reported_once() {
        let mut extents = ExternalExtents::new();

        assert!(extents.take_warning(7), "the first sighting reports");
        assert!(!extents.take_warning(7), "a later sighting does not");
        assert!(extents.take_warning(8), "a different id reports on its own");
    }

    #[test]
    fn rebinding_an_id_lets_it_report_again() {
        let mut extents = ExternalExtents::new();
        assert!(!extents.take_warning(7) || !extents.take_warning(7));

        assert!(extents.bind(7, (4, 4)));

        assert!(
            extents.take_warning(7),
            "a rebound id is a new fact about that id"
        );
    }

    #[test]
    fn a_bound_texture_encodes_its_whole_extent_as_the_source_region() {
        let mut extents = bound((4, 2));
        let mut paints = Vec::new();

        let encoding = encode_scene_texture(7, DEST, Affine::IDENTITY, &mut extents, &mut paints)
            .expect("a bound texture encodes");

        assert_eq!(encoding.paint_index, 0);
        match paints.as_slice() {
            [EncodedPaint::ExternalTexture(entry)] => {
                assert_eq!(entry.texture_id, TextureId(7));
                assert_eq!(
                    entry.source_region,
                    RectU16 {
                        x0: 0,
                        y0: 0,
                        x1: 4,
                        y1: 2
                    }
                );
                assert!(
                    entry.may_have_transparency,
                    "a caller's texels are not claimed opaque unasserted"
                );
                assert_eq!(entry.sampler.quality, ImageQuality::Medium);
            }
            other => panic!("expected one external-texture entry, got {other:?}"),
        }
    }

    #[test]
    fn the_encoded_transform_maps_the_destination_back_onto_the_texels() {
        let mut extents = bound((4, 2));
        let mut paints = Vec::new();

        encode_scene_texture(7, DEST, Affine::IDENTITY, &mut extents, &mut paints)
            .expect("a bound texture encodes");

        let EncodedPaint::ExternalTexture(entry) = &paints[0] else {
            panic!("expected an external-texture entry");
        };
        // `dest` is twice the texture's extent on both axes, so the inverse
        // maps a device point back to half its coordinate in texels.
        let far = entry.transform * kurbo::Point::new(8.0, 4.0);
        assert!((far.x - 4.0).abs() < 1e-9, "x mapped to {}", far.x);
        assert!((far.y - 2.0).abs() < 1e-9, "y mapped to {}", far.y);
    }

    #[test]
    fn a_degenerate_destination_encodes_nothing() {
        let mut extents = bound((4, 2));
        let mut paints = Vec::new();

        let skip = encode_scene_texture(
            7,
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Affine::scale(0.0),
            &mut extents,
            &mut paints,
        );

        assert_eq!(skip, Err(ExternalSkip::SingularTransform));
        assert!(paints.is_empty());
    }

    #[test]
    fn an_extent_past_the_packed_halves_is_refused_rather_than_truncated() {
        let mut extents = ExternalExtents::new();

        assert!(!extents.bind(7, (70_000, 8)));
        assert!(extents.get(7).is_none());
    }

    #[test]
    fn a_zero_extent_is_refused() {
        let mut extents = ExternalExtents::new();

        assert!(!extents.bind(7, (0, 8)));
        assert!(extents.is_empty());
    }

    #[test]
    fn unbinding_forgets_the_extent() {
        let mut extents = bound((4, 2));
        assert_eq!(extents.len(), 1);

        extents.unbind(7);

        assert!(extents.is_empty());
        assert!(extents.get(7).is_none());
    }

    fn encode_with(
        hints: SceneTextureHints,
        size: (u32, u32),
        dest: Rect,
    ) -> EncodedExternalTexture {
        let mut extents = ExternalExtents::new();
        assert!(extents.bind_with(7, size, hints));
        let mut paints = Vec::new();
        encode_scene_texture(7, dest, Affine::IDENTITY, &mut extents, &mut paints)
            .expect("a bound texture encodes");
        match paints.pop() {
            Some(EncodedPaint::ExternalTexture(entry)) => entry,
            other => panic!("expected an external-texture entry, got {other:?}"),
        }
    }

    fn quality(hints: SceneTextureHints, scale: f64) -> ImageQuality {
        // An 8x8 texture over a dest `scale` times its size.
        let side = 8.0 * scale;
        encode_with(hints, (8, 8), Rect::new(0.0, 0.0, side, side))
            .sampler
            .quality
    }

    #[test]
    fn default_hints_resolve_through_the_shared_image_resolver() {
        let hints = SceneTextureHints::default();
        assert_eq!(quality(hints, 1.0), ImageQuality::Low, "1:1 is exact");
        assert_eq!(quality(hints, 2.0), ImageQuality::Medium);
        assert_eq!(quality(hints, 0.5), ImageQuality::Medium);
        assert!(encode_with(hints, (8, 8), DEST).may_have_transparency);
    }

    #[test]
    fn a_nearest_hint_encodes_low_under_scale() {
        let hints = SceneTextureHints::new().with_filter(SceneTextureFilter::Nearest);
        assert_eq!(quality(hints, 4.0), ImageQuality::Low);
        assert_eq!(quality(hints, 0.5), ImageQuality::Low);
    }

    #[test]
    fn nearest_when_magnified_follows_the_scale() {
        let hints = SceneTextureHints::new().with_filter(SceneTextureFilter::NearestWhenMagnified);
        assert_eq!(quality(hints, 4.0), ImageQuality::Low);
        assert_eq!(quality(hints, 0.5), ImageQuality::Medium);
    }

    #[test]
    fn an_opaque_hint_clears_the_transparency_flag() {
        let hints = SceneTextureHints::new().assume_opaque(true);
        assert!(!encode_with(hints, (8, 8), DEST).may_have_transparency);
    }

    #[test]
    fn rebinding_replaces_hints_and_unbinding_forgets_them() {
        let mut extents = ExternalExtents::new();
        assert!(extents.bind_with(7, (4, 4), SceneTextureHints::new().assume_opaque(true)));
        assert_eq!(extents.hints(7).map(|h| h.opaque), Some(true));

        assert!(extents.bind_with(
            7,
            (4, 4),
            SceneTextureHints::new().with_filter(SceneTextureFilter::Nearest)
        ));
        let hints = extents.hints(7).expect("still bound");
        assert!(!hints.opaque);
        assert_eq!(hints.filter, SceneTextureFilter::Nearest);

        extents.unbind(7);
        assert!(extents.hints(7).is_none());
    }
}
