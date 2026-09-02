//! The drop-shadow filter: its GPU parameter block, and the pass sequence a
//! shadowed layer renders as.
//!
//! Ported from the sparse-strip reference renderer's `vello_hybrid` 0.2.0
//! `filter.rs` (the `GpuDropShadow` half) and `shaders/filters/drop_shadow.wesl`
//! — narrowed the same way [`crate::filters::blur`] narrows the reference's
//! Gaussian blur: this engine serves only the *shadow-only* shape
//! (`vello_common`'s `DropShadow::new_shadow_only`, `composite_original` always
//! `false`). The reference's other shape composites the layer's own unfiltered
//! content back over the shadow, which needs a third live texture beyond the
//! two the scheduler's ping-pong ever hands a layer (see
//! [`crate::schedule`]'s *Filter rounds* section) — a shape nothing in frust's
//! widget set asks for yet, so it is left unserved rather than half-built.
//!
//! ## What a shadow costs
//!
//! A shadow is the same blur pass sequence [`crate::filters::blur::blur_passes`]
//! already plans, plus one pass before it and one after: a shift by the
//! shadow's own device-space offset ([`FilterPassKind::Offset`]), and a
//! recolour of the blurred, offset alpha mask into the shadow's own
//! premultiplied colour ([`FilterPassKind::Colorize`]). Both cost one pass each
//! and neither changes extent, so wrapping the blur's own even-length sequence
//! in one more pass on each side keeps the whole thing even — the same
//! even-sequence invariant [`crate::filters::blur::blur_passes`] documents,
//! checked here rather than assumed.
//!
//! ## The kernel these passes read
//!
//! The taps and weights [`GpuDropShadow`] packs are read by [`filter.wgsl`]'s
//! existing `PASS_BLUR_H`/`PASS_BLUR_V` cases unmodified: [`GpuDropShadow`]
//! places its header, centre weight, linear weights and linear offsets at the
//! identical texel offsets [`crate::filters::blur::GpuGaussianBlur`] does, so a
//! drop shadow's blur passes are indistinguishable, to the fragment stage, from
//! a plain blur's. Only the offset and the colour — carried in the block's
//! third texel, which a blur's own leaves as padding — are read by code this
//! module's own [`shaders/filters_drop_shadow.wgsl`] prelude adds.
//!
//! ## Two shadow paths, and which one to reach for
//!
//! Frust now has two ways to paint a drop shadow, and they are not
//! interchangeable — each is the right tool for a different shape of caller.
//!
//! [`frust_scene::Command::BlurredRoundedRect`] is a *scene command*: a widget
//! records one, the engine compiler (`compile::blur_rrect`) encodes it as a
//! `vello_common` blurred-rounded-rect paint the strip shader evaluates per
//! pixel (the CPU oracle mirrors it through vello_cpu's
//! `fill_blurred_rounded_rect`), and the shadow of a rectangle with a single
//! corner radius (per-corner radii collapse to
//! [`frust_scene::CornerRadii::largest`] at encode time — see that command's
//! own doc) is painted analytically, in one draw, with no intermediate
//! texture and no extra render pass. That is the whole of what it can shadow:
//! one rectangle, gaussian-blurred by an error-function approximation
//! (`erf7`) baked into the strip shader's own kernel, never arbitrary content.
//!
//! A drop-shadow [filter](crate::filters) layer is the opposite trade. It
//! shadows *whatever a layer's contents turn out to be* — text, an image, a
//! stack of widgets, anything the layer's own draws paint — because it works
//! from the layer's rasterized alpha rather than from one rectangle's
//! geometry. That generality costs a page for the layer's own contents, a
//! second page for the pass sequence to ping-pong into, and `2 +
//! 2·n_decimations` render passes ([`drop_shadow_passes`]) instead of the one
//! draw a `BlurredRoundedRect` costs — and, on this tier, it is reachable only
//! through [`push_filter_layer`](crate::filters::push_filter_layer), since
//! `frust_scene` carries no filter command yet (see [`crate::filters`]'s own
//! doc for why).
//!
//! **The recommendation this module exists to let a docs card cite:** a
//! widget shadowing its own rectangle — a card, a button, a sheet — keeps
//! using `BlurredRoundedRect`; nothing here should replace it, and nothing
//! about this filter makes that command obsolete. A drop-shadow filter layer
//! is for the case `BlurredRoundedRect` cannot serve at all: shadowing
//! content whose shape is not a single rectangle, or is not known until the
//! layer's own contents are rasterized.
//!
//! [`filter.wgsl`]: ../../../shaders/filter.wgsl
//! [`shaders/filters_drop_shadow.wgsl`]: ../../../shaders/filters_drop_shadow.wgsl

use bytemuck::{Pod, Zeroable};
use vello_common::filter::drop_shadow::DropShadow;
use vello_common::filter::gaussian_blur::GaussianBlur;
use vello_common::geometry::SizeU16;

use crate::filters::blur::{
    self, FILTER_SIZE_BYTES, GpuFilterData, LinearKernel, MAX_TAPS_PER_SIDE, edge_mode_code,
    filter_type,
};
use crate::filters::{FilterPassKind, FilterStep};

const _: () = assert!(
    size_of::<GpuDropShadow>() == FILTER_SIZE_BYTES,
    "every filter's parameter block is one uniform size, which is what makes the type-erased \
     block addressable by a plain texel multiple"
);

/// A drop shadow's parameter block, as the fragment stage reads it.
///
/// `header`, `center_weight`, `linear_weights` and `linear_offsets` sit at the
/// same offsets [`crate::filters::blur::GpuGaussianBlur`] gives them, so the
/// blur passes of a shadow's own sequence read this block through the exact
/// accessors a plain blur's does; `dx`, `dy` and `color` are what a blur's own
/// block leaves as padding, and only the offset and colourize passes this
/// module adds ever read them.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, Zeroable, Pod)]
pub struct GpuDropShadow {
    /// Packed filter kind, edge mode, decimation count and tap count — the
    /// composite-original bit is never set, since this engine serves only the
    /// shadow-only shape.
    pub header: u32,
    /// Weight of the kernel's centre tap.
    pub center_weight: f32,
    /// Merged weight of each bilinear tap pair.
    pub linear_weights: [f32; MAX_TAPS_PER_SIDE],
    /// Fractional offset of each bilinear tap pair.
    pub linear_offsets: [f32; MAX_TAPS_PER_SIDE],
    /// Horizontal device-space offset of the shadow.
    pub dx: f32,
    /// Vertical device-space offset of the shadow.
    pub dy: f32,
    /// The shadow's own premultiplied colour, packed as RGBA8.
    pub color: u32,
    /// Unused; present so every filter kind is one stride wide.
    pub _padding: [u32; 1],
}

impl From<&DropShadow> for GpuDropShadow {
    fn from(shadow: &DropShadow) -> Self {
        let kernel = LinearKernel::new(&shadow.kernel, shadow.kernel_size);

        Self {
            header: pack_drop_shadow_header(
                edge_mode_code(shadow.edge_mode),
                u32::try_from(shadow.n_decimations).unwrap_or(u32::MAX),
                u32::from(kernel.n_taps),
            ),
            center_weight: kernel.center_weight,
            linear_weights: kernel.weights,
            linear_offsets: kernel.offsets,
            dx: shadow.dx,
            dy: shadow.dy,
            color: shadow.color.premultiply().to_rgba8().to_u32(),
            _padding: [0; 1],
        }
    }
}

impl From<GpuDropShadow> for GpuFilterData {
    fn from(shadow: GpuDropShadow) -> Self {
        bytemuck::cast(shadow)
    }
}

/// The packed header of a drop shadow's parameter block.
///
/// See the bit layout documented in `shaders/filter.wgsl`; bit 13
/// (`composite_original`) is always left `0` here — this engine never composes
/// the reference's original-compositing shape (see this module's own doc).
const fn pack_drop_shadow_header(edge_mode: u32, n_decimations: u32, n_linear_taps: u32) -> u32 {
    (filter_type::DROP_SHADOW & 0x1F)
        | ((edge_mode & 0x3) << 5)
        | ((n_decimations & 0xF) << 7)
        | ((n_linear_taps & 0x3) << 11)
}

/// The passes a drop shadow of `shadow` over a layer of `size` renders as, in
/// execution order.
///
/// The shape is the reference's own drop-shadow plan narrowed to the
/// shadow-only case: an [`FilterPassKind::Offset`] shifting the layer by the
/// shadow's own device-space `(dx, dy)`, then exactly the pass sequence
/// [`blur::blur_passes`] plans for the shadow's blur, then a
/// [`FilterPassKind::Colorize`] recolouring the result into the shadow's own
/// premultiplied colour.
///
/// Neither the offset nor the colourize pass rescales, so both read and write
/// `size` — the layer's own, undecimated extent, exactly as the blur
/// sequence's own first and last steps do. Wrapping an always-even sequence in
/// one more pass on each side keeps the whole thing even, so the result lands
/// back in the page the layer's own contents were rendered into; the check
/// below is what keeps that a checked property rather than an assumed one, the
/// same way [`blur::blur_passes`] checks its own.
#[must_use]
pub fn drop_shadow_passes(shadow: &DropShadow, size: SizeU16) -> Vec<FilterStep> {
    let blur = GaussianBlur {
        std_deviation: shadow.std_deviation,
        n_decimations: shadow.n_decimations,
        kernel: shadow.kernel,
        kernel_size: shadow.kernel_size,
        edge_mode: shadow.edge_mode,
    };

    let mut steps: Vec<FilterStep> = Vec::new();
    steps.push(FilterStep {
        kind: FilterPassKind::Offset,
        source: size,
        dest: size,
    });
    steps.extend(blur::blur_passes(&blur, size));
    steps.push(FilterStep {
        kind: FilterPassKind::Colorize,
        source: size,
        dest: size,
    });

    // The reference's `ensure_result_in_original`, carried over from
    // `blur::blur_passes` for the same reason: a pass writes the page it did
    // not read, so an odd-length sequence would strand the result in the
    // scratch page. Offset and colourize add exactly two passes to the blur's
    // own always-even sequence, so this never fires today; it is what keeps
    // that a checked fact rather than an assumption a future change to either
    // wrapper pass could quietly break.
    if !steps.len().is_multiple_of(2) {
        let size = steps.last().map_or(size, |step| step.dest);
        steps.push(FilterStep {
            kind: FilterPassKind::Copy,
            source: size,
            dest: size,
        });
    }

    steps
}
