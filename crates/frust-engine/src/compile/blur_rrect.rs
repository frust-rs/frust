//! Blurred rounded rectangle encoding: `Command::BlurredRoundedRect` to a
//! `vello_common` [`EncodedPaint::BlurredRoundedRect`] entry, plus the padded
//! bounding rectangle a strip generator rasterizes it through.
//!
//! A gaussian-blurred shadow has no hard edge, so it is compiled in two
//! halves that meet only at the draw the compiler records: the *coverage*
//! this file names — a plain axis-aligned rectangle, inflated past `rect` by
//! the kernel's own falloff distance ([`inflated_bounds`]) — decides which
//! pixels the draw can paint at all, while the *shape* (the rounding, the
//! standard deviation, the falloff curve itself) is baked into the encoded
//! paint ([`encode_blurred_rounded_rect`]) and evaluated per pixel by the
//! fragment shader (`calculate_blurred_rounded_rect` in `helpers.wgsl`,
//! dispatched by the `PaintType::BlurredRoundedRect` branch in `strip.wgsl` —
//! both already carry this primitive's GPU half, ported ahead of this file
//! from `helpers/blurred_rounded_rect.wesl`; there is nothing of theirs left
//! to add here). Generating strip coverage for a plain rectangle rather than
//! for the rounded shape itself is deliberate, not a shortcut: the corners
//! the shader rounds are *inside* that rectangle, and the blur reaches
//! outside the rectangle the un-blurred shadow would occupy, so a strip
//! generator that rasterized the rounded shape instead would clip exactly
//! the pixels the blur exists to reach.
//!
//! ## Radius arity
//!
//! `vello_common` 0.2.0's [`BlurredRoundedRectangle`] takes a single `f32`
//! `radius` — the same arity vello_cpu's `fill_blurred_rounded_rect` carries,
//! which is why the CPU oracle collapses a per-corner shadow through
//! [`CornerRadii::largest`] (see `frust-testing::oracle_cpu`; recorded as the
//! accepted limitation `render-blurred-shadow-corner-collapse`). No vello 0.2 type in
//! this dependency line expresses a per-corner blurred rectangle, so this
//! encoder collapses the same way, through the same method, for the same
//! reason `frust_scene::CornerRadii::largest`'s own doc gives: a shadow
//! rounded *more* than its caster only lightens a square corner, while one
//! rounded *less* pushes a hard wedge out through a rounded corner's notch —
//! collapsing to the largest radius is the direction that costs the least
//! fidelity. There is no fidelity win to report here: this engine tier is
//! bounded by the identical single-radius vocabulary those tiers
//! already are, not by a choice made in this file.
//!
//! ## Invert
//!
//! [`BlurredRoundedRectangle::invert`] lets `vello_hybrid` paint the inverse
//! of the blur coverage for an inset shadow (`vello_hybrid` 0.2.0
//! `scene.rs:639-650`), but [`frust_scene::Command::BlurredRoundedRect`]
//! carries no `invert` field — `frust-scene`'s display list has no
//! inset-shadow command yet — so every shadow this compiler lowers is
//! encoded with `invert: false`. Adding an inset variant is a `frust-scene`
//! change, out of this file's scope.

use peniko::Color;

use vello_common::blurred_rounded_rect::BlurredRoundedRectangle;
use vello_common::encode::{EncodeExt, EncodedPaint};
use vello_common::kurbo::{Affine, Rect};
use vello_common::paint::Paint;

use frust_scene::CornerRadii;

/// How many standard deviations of blur kernel the coverage rectangle in
/// [`inflated_bounds`] is padded by, on every side.
///
/// `vello_hybrid` 0.2.0's own `Scene::fill_blurred_rounded_rect`
/// (`scene.rs:670`) inflates by this same `2.5 * std_dev` before
/// rasterizing, and this compiler matches it exactly rather than deriving a
/// tolerance of its own: past this distance the gaussian falloff
/// `calculate_blurred_rounded_rect` evaluates is close enough to zero that a
/// visible difference from a wider pad is not expected, and a narrower one
/// risks clipping a visible tail — the two render tiers and this one should
/// agree on where a shadow ends.
const BLUR_KERNEL_STD_DEVS: f64 = 2.5;

/// The widest pad [`inflated_bounds`] will apply, in the same pre-transform
/// units as the rectangle it inflates.
///
/// The compiler accepts any finite `std_dev`, and an extreme one inflates
/// the coverage rectangle to coordinates that degrade the strip generator's
/// tile arithmetic into malformed strips outside the tile-snapped viewport.
/// The cap bounds the *pad* — never the standard deviation the shader
/// evaluates — at a distance no addressable surface approaches
/// (`max_texture_size` tops out at 16384), so it cannot change a rendered
/// pixel: coverage past the surface is invisible, and every realistic blur
/// keeps its full falloff.
const MAX_KERNEL_PAD: f64 = 1.0e6;

/// The axis-aligned rectangle a blurred rounded rectangle's strip coverage is
/// generated over, in the same (pre-transform) coordinate space as `rect`.
///
/// Padding by [`BLUR_KERNEL_STD_DEVS`] standard deviations on every side is
/// what keeps the strip generator from clipping the blur's own falloff tail
/// — see the module doc for why this is a plain rectangle pad rather than a
/// rounded one. The pad is capped at [`MAX_KERNEL_PAD`], a coverage-only
/// bound that no visible blur reaches. A non-finite or negative `std_dev`
/// is not guarded here: it cannot reach this function at all, because
/// [`super::check_geometry`] refuses a non-finite `std_dev` before any
/// command is compiled, and `frust_scene::SceneBuilder`'s blurred-rect
/// constructors take a caller-supplied standard deviation on the same terms
/// every other geometry parameter is — a negative one is nonsensical but
/// not this compiler's to reject.
#[must_use]
pub fn inflated_bounds(rect: Rect, std_dev: f64) -> Rect {
    let kernel = (BLUR_KERNEL_STD_DEVS * std_dev).min(MAX_KERNEL_PAD);
    rect.inflate(kernel, kernel)
}

/// Encode a blurred rounded rectangle into `encoded_paints`, returning the
/// paint the draw that rasterizes [`inflated_bounds`] references.
///
/// `radii` collapses through [`CornerRadii::largest`] — see the module doc's
/// "Radius arity" section for why. `transform` is the paint transform in
/// effect for the draw, the same composed transform the coverage rectangle
/// is rasterized under; `BlurredRoundedRectangle::encode_into` folds its
/// inverse into the encoded entry, which is the direction the shader samples
/// in.
///
/// Unlike an image, this can never fail: every field a
/// [`Command::BlurredRoundedRect`](frust_scene::Command::BlurredRoundedRect)
/// carries reaches here already checked finite by
/// [`super::check_geometry`], and `vello_common`'s own encoding clamps a
/// degenerate radius or a vanishing standard deviation internally rather
/// than refusing them — so, unlike [`crate::compile::paint::encode_brush`]'s
/// image arm, there is no skip case for a caller here to route around.
#[must_use]
pub fn encode_blurred_rounded_rect(
    rect: Rect,
    radii: CornerRadii,
    std_dev: f64,
    color: Color,
    transform: Affine,
    encoded_paints: &mut Vec<EncodedPaint>,
) -> Paint {
    let shadow = BlurredRoundedRectangle {
        rect,
        color,
        radius: radii.largest() as f32,
        std_dev: std_dev as f32,
        // See the module doc's "Invert" section: the display list carries no
        // inset-shadow flag to plumb through.
        invert: false,
    };

    shadow.encode_into(encoded_paints, transform, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::color::palette::css::RED;

    #[test]
    fn inflated_bounds_pads_every_side_by_the_kernel_distance() {
        let rect = Rect::new(10.0, 10.0, 50.0, 40.0);
        let inflated = inflated_bounds(rect, 4.0);

        let kernel = BLUR_KERNEL_STD_DEVS * 4.0;
        assert_eq!(inflated.x0, rect.x0 - kernel);
        assert_eq!(inflated.y0, rect.y0 - kernel);
        assert_eq!(inflated.x1, rect.x1 + kernel);
        assert_eq!(inflated.y1, rect.y1 + kernel);
    }

    #[test]
    fn inflated_bounds_is_the_identity_at_zero_std_dev() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        assert_eq!(inflated_bounds(rect, 0.0), rect);
    }

    #[test]
    fn encoding_appends_exactly_one_entry_and_indexes_it() {
        let mut encoded_paints = Vec::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 30.0);

        let paint = encode_blurred_rounded_rect(
            rect,
            CornerRadii::uniform(6.0),
            3.0,
            RED,
            Affine::IDENTITY,
            &mut encoded_paints,
        );

        assert_eq!(encoded_paints.len(), 1);
        match paint {
            Paint::Indexed(indexed) => assert_eq!(indexed.index(), 0),
            Paint::Solid(_) => panic!("a blurred rounded rectangle always encodes indexed"),
        }
        match &encoded_paints[0] {
            EncodedPaint::BlurredRoundedRect(entry) => {
                assert_eq!(
                    entry.color,
                    vello_common::paint::PremulColor::from_alpha_color(RED)
                );
                assert!(
                    !entry.invert,
                    "the display list carries no inset-shadow flag"
                );
            }
            other => panic!("expected an encoded blurred rounded rectangle, got {other:?}"),
        }
    }

    #[test]
    fn per_corner_radii_collapse_to_the_largest_corner() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);

        let _ = encode_blurred_rounded_rect(
            rect,
            CornerRadii::new(12.0, 0.0, 0.0, 0.0),
            2.0,
            RED,
            Affine::IDENTITY,
            &mut a,
        );
        let _ = encode_blurred_rounded_rect(
            rect,
            CornerRadii::uniform(12.0),
            2.0,
            RED,
            Affine::IDENTITY,
            &mut b,
        );

        let radius = |paints: &[EncodedPaint]| match &paints[0] {
            EncodedPaint::BlurredRoundedRect(entry) => entry.r1,
            other => panic!("expected an encoded blurred rounded rectangle, got {other:?}"),
        };
        assert_eq!(
            radius(&a),
            radius(&b),
            "a shadow rounded on one corner only must encode the same outer radius as one \
             rounded on every corner to the same value, since only the largest corner survives"
        );
    }

    #[test]
    fn appending_a_second_entry_does_not_disturb_the_first() {
        let mut encoded_paints = Vec::new();
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);

        let first = encode_blurred_rounded_rect(
            rect,
            CornerRadii::uniform(4.0),
            1.0,
            RED,
            Affine::IDENTITY,
            &mut encoded_paints,
        );
        let second = encode_blurred_rounded_rect(
            rect,
            CornerRadii::uniform(8.0),
            2.0,
            RED,
            Affine::IDENTITY,
            &mut encoded_paints,
        );

        assert_eq!(encoded_paints.len(), 2);
        match (first, second) {
            (Paint::Indexed(a), Paint::Indexed(b)) => {
                assert_eq!(a.index(), 0);
                assert_eq!(b.index(), 1);
            }
            _ => panic!("both entries must encode indexed"),
        }
    }
}
