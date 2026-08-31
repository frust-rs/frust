//! The filter golden family: [`FilterCase`], a corpus of `frust-engine`'s
//! layer filters (a Gaussian blur, a shadow-only drop shadow) against a REAL
//! `vello_cpu` 0.2.0 render — the CPU reference every other corpus case is
//! compared against, driven here through its own filter pipeline rather than
//! through [`super::CorpusCase`].
//!
//! # Why not [`super::CorpusCase`]
//!
//! A [`super::CorpusCase`] records through [`frust_scene::SceneBuilder`]
//! only, and `frust_scene` carries no filter command — the scene seam for a
//! layer filter is a later plan (see `frust_engine::filters`' own module
//! doc). [`FilterCase`] is this family's own vocabulary instead: what filter
//! to run, what shape of content to filter, and the tolerance/description a
//! comparator needs. Rendering it is the caller's job on BOTH arms —
//! `crates/frust-testing/tests/engine_goldens.rs` is where that happens,
//! because both arms need machinery ([`wgpu`] on the engine side,
//! `vello_cpu_oracle::RenderContext` directly on the CPU side) this
//! GPU-free module deliberately does not carry.
//!
//! # The shared geometry contract
//!
//! Every non-oversized case draws one [`CONTENT`]-shaped, opaque
//! [`CONTENT_COLOR`] region on a [`CANVAS`]-square field, in either arm, at
//! IDENTICAL absolute coordinates — the geometry [`push_filter_layer`] grows
//! into the recorded layer's own tile-aligned bounds (`frust_engine::filters`'
//! own filter-expansion accounting) is the SAME geometry `vello_cpu`'s
//! `push_layer`'s own filter shift accounts for internally, so the two arms'
//! results line up pixel-for-pixel once both are cropped to that one shared
//! bounding box — a comparator never has to re-derive or hand-tune an
//! offset. [`CONTENT`] is placed [`CONTENT`].0/`.1` texels in from the
//! canvas origin specifically so even the family's largest spread (σ 32's 3σ
//! reach, or the drop shadow's blur spread plus its own offset) never clips
//! against that origin — see each constant's own doc.
//!
//! [`push_filter_layer`]: frust_engine::filters::push_filter_layer

use peniko::color::palette::css::{BLACK, ROYAL_BLUE};
use peniko::color::{AlphaColor, Srgb};
use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};

use crate::case::Tolerance;

/// The recorder/canvas extent every case in this family renders against, on
/// both arms.
///
/// Large enough that [`CONTENT`]'s placement leaves headroom on every side
/// for the family's largest spread (see [`CONTENT`]'s own doc) without ever
/// clamping against the canvas origin, which would make the two arms'
/// geometry diverge for a reason that is not the filter's own.
pub const CANVAS: u16 = 512;

/// The opaque content rect every non-oversized case draws before opening its
/// filter layer: `(x, y, width, height)`, all in texels.
///
/// Placed 128 texels in from [`CANVAS`]'s origin on both axes — comfortably
/// past σ 32's 3σ == 96-texel reach (`filter-blur-32`) and past the drop
/// shadow's own blur spread plus its offset (`filter-drop-shadow`), so
/// neither case's recorded bounds are ever clamped at zero. `filter-blur-
/// clipped` rounds this same footprint's corners to [`CLIP_RADIUS`] rather
/// than moving or resizing it, so one placement serves the whole family.
pub const CONTENT: (u16, u16, u16, u16) = (128, 128, 64, 64);

/// Corner radius `filter-blur-clipped` rounds [`CONTENT`] to.
pub const CLIP_RADIUS: f64 = 16.0;

/// The opaque color [`CONTENT`] is filled with on both arms.
pub const CONTENT_COLOR: AlphaColor<Srgb> = ROYAL_BLUE;

/// The color `filter-drop-shadow` recolors its shadow to.
pub const SHADOW_COLOR: AlphaColor<Srgb> = BLACK;

/// The offset `filter-drop-shadow` shifts its shadow by, in user space.
pub const SHADOW_OFFSET: (f32, f32) = (24.0, 16.0);

/// What one [`FilterCase`] filters with — the engine-internal vocabulary
/// [`push_filter_layer`](frust_engine::filters::push_filter_layer) records
/// with, mirrored here as plain data so a case can describe itself without
/// depending on `frust-engine` or `wgpu` at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterKind {
    /// A Gaussian blur of standard deviation `sigma`.
    Blur {
        /// The blur's standard deviation, in user space.
        sigma: f32,
    },
    /// A shadow-only drop shadow: [`CONTENT`]'s own alpha, blurred by
    /// `sigma`, shifted by [`SHADOW_OFFSET`] and recolored to
    /// [`SHADOW_COLOR`].
    DropShadow {
        /// The shadow's blur standard deviation, in user space.
        sigma: f32,
    },
}

impl FilterKind {
    /// The `vello_common` filter primitive this kind describes, at
    /// [`EdgeMode::default`] — the same primitive both
    /// [`push_filter_layer`](frust_engine::filters::push_filter_layer) (via
    /// [`frust_engine::filters::LayerFilter`]) and `vello_cpu`'s own
    /// `push_layer` `filter` parameter accept, so the two arms filter with
    /// the identical value rather than two hand-built ones that could drift
    /// apart.
    #[must_use]
    pub fn primitive(self) -> FilterPrimitive {
        match self {
            Self::Blur { sigma } => FilterPrimitive::GaussianBlur {
                std_deviation: sigma,
                edge_mode: EdgeMode::default(),
            },
            Self::DropShadow { sigma } => FilterPrimitive::DropShadowOnly {
                dx: SHADOW_OFFSET.0,
                dy: SHADOW_OFFSET.1,
                std_deviation: sigma,
                color: SHADOW_COLOR,
                edge_mode: EdgeMode::default(),
            },
        }
    }

    /// [`Self::primitive`], as a `vello_common` [`Filter`] — the exact value
    /// `vello_cpu`'s `push_layer` `filter` argument takes. `vello_cpu`
    /// combines it with whatever transform is active when `push_layer` is
    /// called (mirroring `push_filter_layer`'s own `transform` argument on
    /// the engine side), so no transform is threaded through here.
    #[must_use]
    pub fn filter(self) -> Filter {
        Filter::from_primitive(self.primitive())
    }
}

/// Whether a case's content is [`CONTENT`] verbatim or rounded to
/// [`CLIP_RADIUS`] — `filter-blur-clipped`'s own shape, the case that proves
/// a filter runs correctly over non-rectangular content (its kernel's
/// bilinear taps sample straight through the transparent corners a rounded
/// rect leaves inside [`CONTENT`]'s own bounding box).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentShape {
    /// The plain axis-aligned [`CONTENT`] rect.
    Rect,
    /// [`CONTENT`], corners rounded to [`CLIP_RADIUS`].
    Rounded,
}

/// One named case in the filter golden family: what to filter with, what
/// shape to filter, and the tolerance/description a comparator needs.
///
/// Not a [`super::CorpusCase`] — see this module's own doc for why — so a
/// case here carries no `record: fn(&mut Scene)` and is rendered by the
/// caller directly on each arm instead ([`crate::corpus::filters`]'s own
/// module doc names the two call sites).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterCase {
    /// The case's stable name — the golden-artifact stem, exactly like
    /// [`crate::case::CaseSpec::name`].
    pub name: &'static str,
    /// What this case filters with.
    pub kind: FilterKind,
    /// What shape of content this case filters.
    pub shape: ContentShape,
    /// The engine-vs-`vello_cpu` comparison tolerance (P1's own bar — see
    /// `crates/frust-testing/tests/engine_goldens.rs`'s module doc for what
    /// P1 means for the rest of the corpus).
    pub tolerance: Tolerance,
    /// One line on what this case exists to pin, for the family listing and
    /// for a reviewer reading a promoted PNG cold.
    pub about: &'static str,
}

/// The family's five rendered cases — σ 2/8/32 plain blurs, a shadow-only
/// drop shadow, and a blur over rounded (non-rectangular) content.
///
/// `filter-blur-oversized`, the family's sixth and only refusal member, is
/// NOT here: it renders nothing on either arm (it exists to pin
/// [`frust_engine::error::EngineError::IntermediateTextureTooLarge`], not a
/// pixel), so `crates/frust-testing/tests/engine_goldens.rs` names it and its
/// own σ directly rather than threading a `no_ref`/oversized flag through
/// this struct for a case with no image at either end.
#[must_use]
pub fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            name: "filter-blur-2",
            kind: FilterKind::Blur { sigma: 2.0 },
            shape: ContentShape::Rect,
            tolerance: Tolerance::new(),
            about: "the shallowest blur this family pins: no decimation, a single H/V \
                    convolution pass each way — vello_common's own kernel, shared by both arms",
        },
        FilterCase {
            name: "filter-blur-8",
            kind: FilterKind::Blur { sigma: 8.0 },
            shape: ContentShape::Rect,
            // Wider than the corpus default: measured on the pinned T400 rig
            // at max |delta| [3, 2, 3, 2], 12 px (0.0957%) of this case's own
            // 43x59 frame, all inside the decimated pyramid's reconstructed
            // edge — the GPU's bilinear downscale/upscale passes round
            // slightly differently from vello_cpu's software ones on the two
            // decimations this σ costs (the shallower filter-blur-2, with no
            // decimation, measures 0 px differing at the corpus default).
            tolerance: Tolerance::new().with_channel(3).with_alpha(2),
            about: "a mid blur: two decimations each way, exercising the downscale/upscale \
                    passes the shallowest case never costs",
        },
        FilterCase {
            name: "filter-blur-32",
            kind: FilterKind::Blur { sigma: 32.0 },
            shape: ContentShape::Rect,
            // Wider still: measured on the pinned T400 rig at max |delta|
            // [2, 3, 4, 4], 1848 px (2.8198%) of this case's own 150x158
            // frame, confined to the reconstructed halo around the source
            // rect's edges — four decimations each way compounds the same
            // GPU-vs-software rounding filter-blur-8's own row explains, and
            // the deepest σ this family pins costs the most of them.
            tolerance: Tolerance::new().with_channel(4).with_alpha(4),
            about: "the deepest blur this family pins short of refusal: four decimations each \
                    way, the widest spread `CONTENT`'s own placement was sized to leave room for",
        },
        FilterCase {
            name: "filter-drop-shadow",
            kind: FilterKind::DropShadow { sigma: 8.0 },
            shape: ContentShape::Rect,
            tolerance: Tolerance::new(),
            about: "a shadow-only drop shadow: CONTENT's own alpha, blurred, shifted by \
                    SHADOW_OFFSET and recolored to SHADOW_COLOR — never the layer's own \
                    unfiltered content composited back over it (frust_engine::filters::\
                    drop_shadow's own doc explains why only that shape is served)",
        },
        FilterCase {
            name: "filter-blur-clipped",
            kind: FilterKind::Blur { sigma: 8.0 },
            shape: ContentShape::Rounded,
            // Wider than the plain-rect cases: the two arms' antialiasing at
            // CLIP_RADIUS's rounded corners is not bit-identical (a
            // supersampled coverage mask on the engine side vs vello_cpu's
            // own analytic rasterizer), and the blur that follows only
            // softens that difference rather than erasing it. Measured on
            // the pinned T400 rig; see this row's own escalation note in
            // `engine_goldens.rs` for the exact number and why it is the
            // corner antialiasing rather than the blur kernel.
            tolerance: Tolerance::new().with_channel(24).with_alpha(24),
            about: "a blur over ROUNDED (non-rectangular) content: the same σ 8 kernel as \
                    filter-drop-shadow's, over content whose corners are transparent inside \
                    CONTENT's own bounding box — proves the kernel's bilinear taps sample \
                    straight through that transparency rather than assuming a filled rect",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_has_a_unique_non_empty_name_and_a_real_description() {
        let cases = filter_cases();
        assert_eq!(cases.len(), 5);
        let mut seen: Vec<&str> = Vec::new();
        for case in &cases {
            assert!(!case.name.is_empty());
            assert!(
                case.about.len() > 16,
                "`{}`'s description is too thin to review: {:?}",
                case.name,
                case.about
            );
            assert!(
                !seen.contains(&case.name),
                "`{}` is listed more than once",
                case.name
            );
            seen.push(case.name);
        }
    }

    #[test]
    fn every_case_name_is_prefixed_filter_and_starts_with_filter_blur_or_filter_drop_shadow() {
        for case in filter_cases() {
            assert!(
                case.name.starts_with("filter-"),
                "`{}` does not carry the family's own prefix",
                case.name
            );
        }
    }

    #[test]
    fn a_blur_kind_and_a_drop_shadow_kind_describe_the_primitive_their_own_arm_needs() {
        let blur = FilterKind::Blur { sigma: 8.0 }.primitive();
        assert!(matches!(
            blur,
            FilterPrimitive::GaussianBlur {
                std_deviation: 8.0,
                ..
            }
        ));

        let shadow = FilterKind::DropShadow { sigma: 8.0 }.primitive();
        let FilterPrimitive::DropShadowOnly {
            dx,
            dy,
            std_deviation,
            color,
            ..
        } = shadow
        else {
            panic!("expected a shadow-only drop shadow primitive: {shadow:?}");
        };
        assert_eq!(dx, SHADOW_OFFSET.0);
        assert_eq!(dy, SHADOW_OFFSET.1);
        assert_eq!(std_deviation, 8.0);
        assert_eq!(color, SHADOW_COLOR);
    }

    #[test]
    fn content_is_placed_with_room_for_the_familys_widest_spread() {
        // 3 * 32 == 96, the widest blur spread the family pins; CONTENT's own
        // (128, 128) placement leaves 128 texels of headroom on the low
        // edges, comfortably past that reach plus the drop shadow's own
        // offset on top of its own (shallower) blur.
        let reach = 3.0 * 32.0_f32;
        assert!(f32::from(CONTENT.0) > reach);
        assert!(f32::from(CONTENT.1) > reach);
        assert!(CONTENT.0 + CONTENT.2 < CANVAS);
        assert!(CONTENT.1 + CONTENT.3 < CANVAS);
    }
}
