//! Paint encoding: `peniko::Brush` to a `vello_common` [`Paint`] plus its
//! [`EncodedPaint`] side table.
//!
//! A scene's brushes are split into the two things the renderer needs. A solid
//! colour is self-contained and travels premultiplied inside the [`Paint`] itself,
//! costing no side-table entry. Anything else is encoded once into a caller-owned
//! `Vec<EncodedPaint>` and referenced by index, so per-draw state stays small and
//! two draws sharing a brush share one encoded entry.
//!
//! Encoding a gradient also *bakes nothing*: the colour ramp is a separate,
//! bounded GPU resource, so a gradient encoding yields a [`LutRequest`] naming the
//! entry whose ramp must be made resident in the [`GradientCache`] before the
//! frame is drawn. Requests are deliberately not serviced here — a caller
//! batches them so ramp residency is decided once per frame rather than per draw.

use std::sync::Once;

use peniko::{Brush, Color};
use vello_common::encode::{EncodeExt, EncodedPaint};
use vello_common::kurbo::Affine;
use vello_common::paint::Paint;

use crate::cache::{CachedRamp, GradientCache};

/// The colour an image brush paints with until image paints are supported.
///
/// Transparent rather than an arbitrary opaque colour: an unsupported paint should
/// leave the surface untouched, not stamp a wrong-coloured shape over it.
const IMAGE_PLACEHOLDER: Color = Color::TRANSPARENT;

static IMAGE_BRUSH_WARNING: Once = Once::new();

/// A gradient whose colour ramp is not yet resident in the [`GradientCache`].
///
/// Names the [`EncodedPaint`] entry to bake, not the gradient itself, because the
/// encoded entry is what carries the cache key the ramp is stored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LutRequest {
    /// Index of the gradient entry in the encoded-paint side table.
    pub paint_index: usize,
}

/// One encoded brush: the paint a draw references, plus any ramp it still needs.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushEncoding {
    /// The paint to record against the draw.
    pub paint: Paint,
    /// Set when the paint is a gradient whose ramp must be made resident.
    pub lut_request: Option<LutRequest>,
}

/// Encode `brush` into `paint`'s renderer-facing form, appending to `encoded_paints`.
///
/// `transform` is the paint transform in effect for the draw; a gradient's encoded
/// form folds its inverse in, so the same gradient under two transforms yields two
/// encoded entries but still shares one colour ramp.
///
/// A gradient that is degenerate or malformed — fewer than two stops, unsorted or
/// out-of-range offsets, a zero-length line, coincident circles, a non-increasing
/// sweep — is not an error: `vello_common` substitutes a solid fallback, which
/// arrives here as a plain [`Paint::Solid`] with no side-table entry and therefore
/// no [`LutRequest`].
///
/// Frust's scene layer only ever constructs `Extend::Pad` gradients; other extend
/// modes are carried through to the encoded entry untouched for the renderer to
/// honour.
pub fn encode_brush(
    brush: &Brush,
    transform: Affine,
    encoded_paints: &mut Vec<EncodedPaint>,
) -> BrushEncoding {
    match brush {
        Brush::Solid(color) => BrushEncoding {
            paint: (*color).into(),
            lut_request: None,
        },
        Brush::Gradient(gradient) => {
            let paint = gradient.encode_into(encoded_paints, transform, None);
            // A degenerate gradient falls back to a solid colour and pushes no
            // entry, so the request follows the encoding result rather than the
            // brush kind.
            let lut_request = match &paint {
                Paint::Indexed(indexed) => Some(LutRequest {
                    paint_index: indexed.index(),
                }),
                Paint::Solid(_) => None,
            };

            BrushEncoding { paint, lut_request }
        }
        Brush::Image(_) => {
            IMAGE_BRUSH_WARNING.call_once(|| {
                log::warn!(
                    "image brushes are not encoded yet; painting them transparent (logged once)"
                );
            });

            BrushEncoding {
                paint: IMAGE_PLACEHOLDER.into(),
                lut_request: None,
            }
        }
    }
}

/// Make the ramp named by `request` resident, returning where it landed.
///
/// Returns `None` when the request does not name a gradient entry, which can only
/// happen if `encoded_paints` is not the table the request was produced against.
pub fn resolve_lut_request(
    request: LutRequest,
    encoded_paints: &[EncodedPaint],
    cache: &mut GradientCache,
) -> Option<CachedRamp> {
    match encoded_paints.get(request.paint_index) {
        Some(EncodedPaint::Gradient(gradient)) => Some(cache.get_or_create_ramp(gradient)),
        _ => None,
    }
}
