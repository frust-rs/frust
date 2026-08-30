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
//!
//! An image is the one paint that cannot follow that deferred shape. A
//! gradient's encoded entry carries its own cache key, so the ramp can be baked
//! afterwards and matched back up; an image's encoded entry has to carry the
//! [`ImageId`](vello_common::paint::ImageId) *itself*, which only an allocation
//! against the atlas can mint. Image residency is therefore resolved inline
//! ([`encode_image`] and its two callers), against a residency the compiler
//! owns — cheap, because allocation is a rectangle packer over plain values and
//! only the *upload* it schedules costs anything, and that upload happens once
//! per image rather than once per frame.

use std::sync::Once;

use peniko::{Brush, Color, ImageBrush, ImageData, ImageQuality, ImageSampler};
use vello_common::encode::{EncodeExt, EncodedImage, EncodedPaint};
use vello_common::kurbo::{Affine, Rect};
use vello_common::paint::{IndexedPaint, Paint, Tint, TintMode};

use crate::cache::images::{ImageResidency, ImageSkip, ResidentImage};
use crate::cache::{CachedRamp, GradientCache};
use crate::gpu::atlas::{natural_to_dest, x_y_advances};

/// The colour an image brush paints with when it is encoded with no residency
/// in hand.
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

/// One encoded image paint: the paint a draw references, plus where the atlas
/// made its texels resident.
///
/// The residency travels back out because the record the shader reads is built
/// from *both* halves — the encoded entry's sampler and transform, and the
/// atlas layer, offset and extent — and only this call has both in hand at
/// once (see [`crate::gpu::atlas::lower_encoded_image`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageEncoding {
    /// The paint to record against the draw.
    pub paint: Paint,
    /// Index of the image entry in the encoded-paint side table.
    pub paint_index: usize,
    /// Where the image's texels are resident in the atlas array.
    pub resident: ResidentImage,
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
                    "an image brush encoded with no residency paints transparent \
                     (logged once); the compiler routes image brushes through \
                     `encode_image_brush` instead"
                );
            });

            BrushEncoding {
                paint: IMAGE_PLACEHOLDER.into(),
                lut_request: None,
            }
        }
    }
}

/// Encode an image brush drawn at its natural pixel size under `transform`.
///
/// The vello contract an image brush carries: the pixels land one-for-one on
/// the device grid under the paint transform, with the sampler's extend modes
/// covering everything the shape reaches beyond them.
///
/// # Errors
///
/// Returns the [`ImageSkip`] the residency refused the image with, or
/// [`ImageSkip::SingularTransform`] for a paint transform with no inverse.
pub fn encode_image_brush(
    brush: &ImageBrush,
    transform: Affine,
    encoded_paints: &mut Vec<EncodedPaint>,
    images: &mut ImageResidency,
) -> Result<ImageEncoding, ImageSkip> {
    encode_image(
        &brush.image,
        brush.sampler,
        transform,
        encoded_paints,
        images,
    )
}

/// Encode a `Command::Image`: `data`'s natural pixel rectangle scaled to fill
/// `dest`, under `transform`.
///
/// The natural-to-`dest` composition is
/// [`crate::gpu::atlas::natural_to_dest`], the same one `frust-render`'s
/// CPU-tier lowering applies, so an image lands on identical device pixels
/// whichever tier drew it. The sampler is the display list's own default —
/// [`ImageQuality::Medium`] (bilinear) with padded extends, since a
/// `Command::Image` carries no sampling parameters of its own and a scaled
/// image filtered at nearest would be visibly worse than the CPU tier's
/// output.
///
/// # Errors
///
/// Returns the [`ImageSkip`] the residency refused the image with,
/// [`ImageSkip::SingularTransform`] for a transform with no inverse, or
/// [`ImageSkip::DegenerateExtent`] when `data` has no natural area to scale
/// from.
pub fn encode_image_command(
    data: &ImageData,
    dest: Rect,
    transform: Affine,
    encoded_paints: &mut Vec<EncodedPaint>,
    images: &mut ImageResidency,
) -> Result<ImageEncoding, ImageSkip> {
    let paint_transform = natural_to_dest(transform, (data.width, data.height), dest).ok_or(
        ImageSkip::DegenerateExtent {
            width: data.width,
            height: data.height,
        },
    )?;

    encode_image(
        data,
        ImageSampler::default(),
        paint_transform,
        encoded_paints,
        images,
    )
}

/// Make `data` resident and append its encoded entry, returning the paint that
/// references it.
///
/// `paint_transform` is the *forward* mapping from the image's natural pixel
/// rectangle onto device space; the entry stores its inverse, because that is
/// the direction the shader applies it in (device fragment to image texel).
///
/// A sampler alpha below one is folded into the entry's tint rather than
/// carried on the sampler: `vello_common`'s own image encoding
/// `unimplemented!()`s on a non-unit sampler alpha, and the shader has no alpha
/// field to read one from — but it always multiplies by a tint, so an identity
/// tint scaled by the alpha produces exactly the same result with nothing left
/// unimplemented on the frame path (E17).
///
/// # Errors
///
/// Returns the [`ImageSkip`] the residency refused the image with, or
/// [`ImageSkip::SingularTransform`] when `paint_transform` has no finite
/// inverse.
pub fn encode_image(
    data: &ImageData,
    sampler: ImageSampler,
    paint_transform: Affine,
    encoded_paints: &mut Vec<EncodedPaint>,
    images: &mut ImageResidency,
) -> Result<ImageEncoding, ImageSkip> {
    let transform = paint_transform.inverse();
    if !transform.as_coeffs().iter().all(|coeff| coeff.is_finite()) {
        return Err(ImageSkip::SingularTransform);
    }

    let resident = images.resolve(data)?;

    let (x_advance, y_advance) = x_y_advances(transform);
    let tint = alpha_tint(sampler.alpha);
    let sampler = ImageSampler {
        quality: resolved_quality(sampler.quality, paint_transform),
        alpha: 1.0,
        ..sampler
    };

    let paint_index = encoded_paints.len();
    encoded_paints.push(EncodedPaint::Image(EncodedImage {
        may_have_transparency: resident.may_have_transparency || tint.is_some(),
        source: resident.source(),
        sampler,
        transform,
        x_advance,
        y_advance,
        tint,
    }));

    Ok(ImageEncoding {
        paint: Paint::Indexed(IndexedPaint::new(paint_index)),
        paint_index,
        resident,
    })
}

/// A sampler alpha below one expressed as the identity tint scaled by it, or
/// `None` for a fully opaque (or malformed) alpha.
fn alpha_tint(alpha: f32) -> Option<Tint> {
    if !alpha.is_finite() || alpha >= 1.0 {
        return None;
    }

    Some(Tint {
        color: Color::WHITE.multiply_alpha(alpha.max(0.0)),
        mode: TintMode::Multiply,
    })
}

/// The sampling quality an image is actually drawn at.
///
/// A bilinear request under a whole-pixel translation is downgraded to nearest,
/// which is the same optimization `vello_common`'s own image encoding applies:
/// the four taps a bilinear filter blends are the same texel when nothing
/// subpixel is happening, so the downgrade is exact rather than a quality
/// trade, and it is what keeps this tier's output bit-identical to the CPU
/// reference for an unscaled image.
fn resolved_quality(quality: ImageQuality, paint_transform: Affine) -> ImageQuality {
    if quality != ImageQuality::Medium {
        return quality;
    }

    let c = paint_transform.as_coeffs();
    let unit_translation = (c[0] - 1.0).abs() < NEARLY_ZERO
        && c[1].abs() < NEARLY_ZERO
        && c[2].abs() < NEARLY_ZERO
        && (c[3] - 1.0).abs() < NEARLY_ZERO
        && (c[4] - c[4].floor()).abs() < NEARLY_ZERO
        && (c[5] - c[5].floor()).abs() < NEARLY_ZERO;

    if unit_translation {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    }
}

/// The near-zero tolerance `vello_common`'s `is_nearly_zero` compares against,
/// restated because that trait is not exported.
const NEARLY_ZERO: f64 = 1.0 / 4096.0;

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
