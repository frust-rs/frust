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

use crate::gpu::atlas::natural_to_dest;

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
    extents: HashMap<u64, (u16, u16)>,
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
        let (Ok(width), Ok(height)) = (u16::try_from(size.0), u16::try_from(size.1)) else {
            self.extents.remove(&id);
            return false;
        };
        if width == 0 || height == 0 {
            self.extents.remove(&id);
            return false;
        }
        self.extents.insert(id, (width, height));
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
        self.extents.get(&id).copied()
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
/// sampler is the display list's own default — [`ImageQuality::Medium`]
/// (bilinear) with padded extends — since the command carries no sampling
/// parameters of its own and a scaled texture filtered at nearest would be
/// visibly worse.
///
/// The entry is marked as possibly transparent unconditionally. The command
/// carries no opacity statement and the engine never reads the caller's texels,
/// so every such draw goes to the frame's blended pass; claiming opacity would
/// route it through the depth-writing pass and let it occlude what it should
/// have blended over.
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
    let Some((width, height)) = extents.get(id) else {
        note_unregistered(id, extents.take_warning(id));
        return Err(ExternalSkip::Unregistered { id });
    };

    let paint_transform = natural_to_dest(transform, (u32::from(width), u32::from(height)), dest)
        .ok_or(ExternalSkip::DegenerateDest)?;
    let inverse = paint_transform.inverse();
    if !inverse.as_coeffs().iter().all(|coeff| coeff.is_finite()) {
        return Err(ExternalSkip::SingularTransform);
    }

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
            quality: ImageQuality::Medium,
            ..ImageSampler::default()
        },
        may_have_transparency: true,
        transform: inverse,
        tint: None,
    }));

    Ok(ExternalEncoding {
        paint: Paint::Indexed(IndexedPaint::new(paint_index)),
        paint_index,
    })
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
                    "a caller's texels are never claimed opaque"
                );
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
}
