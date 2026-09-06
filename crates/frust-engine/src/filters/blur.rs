//! The Gaussian blur filter: its GPU parameter block, and the pass sequence a
//! blurred layer renders as.
//!
//! Ported from the sparse-strip reference renderer's `vello_hybrid` 0.2.0
//! `filter.rs`, narrowed to the single-filter path — the multi-primitive filter
//! *graph* planner is unimplemented upstream too (`vello_common`'s
//! `PreparedFilter::new` refuses a graph of more than one primitive), so there
//! is nothing here to port it from. The kernel itself is not re-derived: the
//! decimation plan, the discrete Gaussian weights and the dimension bookkeeping
//! all come from [`vello_common::filter::gaussian_blur`], so this tier and the
//! CPU rasterizer blur with byte-identical kernels.
//!
//! ## Two representations of one kernel
//!
//! `vello_common` hands over a discrete kernel of up to [`MAX_KERNEL_SIZE`]
//! weights, which the CPU rasterizer convolves with directly, one multiply per
//! tap. A GPU has bilinear sampling, so each *pair* of adjacent taps is merged
//! here into one sample at a fractional offset between them, weighted by their
//! sum ([`LinearKernel`]) — the same Gaussian at roughly half the samples. See
//! <https://www.rastergrid.com/blog/2010/09/efficient-gaussian-blur-with-linear-sampling/>.
//!
//! ## Every filter's parameter block is the same size
//!
//! A filter's parameters reach the fragment stage through one texture the whole
//! frame's filters are packed into, addressed by a texel offset. Every kind's
//! block is [`FILTER_SIZE_BYTES`] wide, whatever it carries, which is what lets
//! that offset be a plain multiple rather than a per-filter lookup — the
//! reason [`GpuGaussianBlur`] pads to a footprint it does not need (a drop
//! shadow's is larger) and the reason each block carries a `size_of` assert of
//! its own. A kind added later carries the same assert or the erasure to
//! [`GpuFilterData`] stops being sound.

use bytemuck::{Pod, Zeroable};
use vello_common::filter::gaussian_blur::{DecimationSizer, GaussianBlur};
use vello_common::filter_effects::EdgeMode;
use vello_common::geometry::SizeU16;

pub use vello_common::filter::gaussian_blur::MAX_KERNEL_SIZE;

use crate::filters::{FilterPassKind, FilterStep};

/// Transparent padding a decimated pass overdraws around the region it writes.
///
/// The blur kernels sample bilinearly, so a tap can reach half a kernel past
/// the region it is filtering. Reserving that much transparent border is what
/// lets every kernel skip bounds checks entirely: a tap that lands outside
/// reads transparent black instead of a stale texel, which is exactly what the
/// convolution wants there.
///
/// Keep this in sync with `FILTER_ATLAS_PADDING` in `shaders/filter.wgsl`.
pub const FILTER_ATLAS_PADDING: u16 = MAX_KERNEL_SIZE as u16 / 2;

/// Bytes one filter's parameter block occupies in the filter-data texture,
/// uniform across every filter kind.
///
/// Keep this in sync with `FILTER_SIZE_BYTES` in `shaders/filter.wgsl`.
pub const FILTER_SIZE_BYTES: usize = 48;

/// Words one filter's parameter block occupies.
const FILTER_SIZE_U32: usize = FILTER_SIZE_BYTES / 4;

/// Bytes per texel of the filter-data texture, which is `Rgba32Uint`.
const BYTES_PER_TEXEL: usize = 16;

/// Bit of the packed header a drop shadow sets to ask for the unfiltered layer
/// to be composited back over the shadow.
///
/// Unused by the blur, and reserved here rather than left implicit: the
/// const assertion below is what keeps the blur's own header fields from
/// growing into it.
const COMPOSITE_ORIGINAL_SHIFT: u32 = 13;

/// Mask of [`COMPOSITE_ORIGINAL_SHIFT`].
const COMPOSITE_ORIGINAL_MASK: u32 = 1 << COMPOSITE_ORIGINAL_SHIFT;

const _: () = assert!(
    size_of::<GpuFilterData>() == FILTER_SIZE_BYTES,
    "every filter's parameter block is one uniform size, which is what makes the type-erased \
     block addressable by a plain texel multiple"
);
const _: () = assert!(
    size_of::<GpuGaussianBlur>() == FILTER_SIZE_BYTES,
    "every filter's parameter block is one uniform size, which is what makes the type-erased \
     block addressable by a plain texel multiple"
);
const _: () = assert!(
    FILTER_SIZE_BYTES.is_multiple_of(BYTES_PER_TEXEL),
    "a parameter block that did not fill whole texels could not be addressed by a texel offset"
);
const _: () = assert!(
    pack_blur_header(0x1F, 3, 15, 3) & COMPOSITE_ORIGINAL_MASK == 0,
    "the blur header's fields must not grow into the drop shadow's composite-original bit"
);

/// The kind each filter's packed header names, matching the reference's own
/// numbering so a kind added later needs no renumbering.
pub mod filter_type {
    /// An offset filter. Reserved; not served.
    pub const OFFSET: u32 = 0;
    /// A flood filter. Reserved; not served.
    pub const FLOOD: u32 = 1;
    /// A Gaussian blur.
    pub const GAUSSIAN_BLUR: u32 = 2;
    /// A drop shadow. Reserved; not served.
    pub const DROP_SHADOW: u32 = 3;
}

/// How a filter samples past the edge of its input, matching the reference's
/// own numbering.
pub mod edge_mode {
    /// Clamp to the edge texel.
    pub const DUPLICATE: u32 = 0;
    /// Wrap around to the opposite edge.
    pub const WRAP: u32 = 1;
    /// Mirror across the edge.
    pub const MIRROR: u32 = 2;
    /// Read transparent black.
    pub const NONE: u32 = 3;
}

/// `mode` as the fragment stage reads it.
#[must_use]
pub const fn edge_mode_code(mode: EdgeMode) -> u32 {
    match mode {
        EdgeMode::Duplicate => edge_mode::DUPLICATE,
        EdgeMode::Wrap => edge_mode::WRAP,
        EdgeMode::Mirror => edge_mode::MIRROR,
        EdgeMode::None => edge_mode::NONE,
    }
}

/// The most merged bilinear tap pairs per side a kernel can produce.
///
/// Keep this in sync with `MAX_TAPS_PER_SIDE` in `shaders/filters_blur.wgsl`:
/// the shader's weight and offset accessors read a `vec3<f32>` each, which is
/// only enough because the decimation plan bounds the kernel at
/// [`MAX_KERNEL_SIZE`].
pub const MAX_TAPS_PER_SIDE: usize = (MAX_KERNEL_SIZE / 2).div_ceil(2);

const _: () = assert!(
    MAX_TAPS_PER_SIDE == 3,
    "the shader packs the tap weights and offsets into one `vec3<f32>` each"
);

/// A discrete Gaussian kernel re-expressed for bilinear sampling.
///
/// The centre tap is sampled on its own; every other pair of adjacent taps is
/// merged into a single sample placed between them, so the fragment stage runs
/// `1 + 2 * n_taps` samples rather than one per kernel entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearKernel {
    /// Weight of the centre tap.
    pub center_weight: f32,
    /// Merged weight of each tap pair. Only the first `n_taps` are meaningful.
    ///
    /// One side only: the kernel is symmetric, so each entry is applied on both
    /// sides of the centre.
    pub weights: [f32; MAX_TAPS_PER_SIDE],
    /// Fractional offset, in texels from the centre, each tap pair is sampled
    /// at. Only the first `n_taps` are meaningful.
    pub offsets: [f32; MAX_TAPS_PER_SIDE],
    /// How many tap pairs per side are meaningful.
    pub n_taps: u8,
}

impl LinearKernel {
    /// The bilinear form of the first `kernel_size` weights of `kernel`.
    ///
    /// A `kernel_size` past [`MAX_KERNEL_SIZE`] is clamped to it rather than
    /// indexing out of bounds; the decimation plan never produces one, and
    /// clamping keeps that a fact about the plan rather than a precondition on
    /// this function (E17).
    #[must_use]
    pub fn new(kernel: &[f32; MAX_KERNEL_SIZE], kernel_size: u8) -> Self {
        let kernel_size = usize::from(kernel_size).min(MAX_KERNEL_SIZE);
        let radius = kernel_size / 2;
        let center_weight = kernel.get(radius).copied().unwrap_or(1.0);

        let mut weights = [0.0_f32; MAX_TAPS_PER_SIDE];
        let mut offsets = [0.0_f32; MAX_TAPS_PER_SIDE];
        let mut n_taps = 0_usize;

        // Symmetric, so only the positive side is walked.
        let positive_side = kernel
            .get(radius.saturating_add(1)..kernel_size)
            .unwrap_or(&[]);
        let (pairs, remainder) = positive_side.as_chunks::<2>();

        for (k, &[w1, w2]) in pairs.iter().enumerate() {
            let merged_weight = w1 + w2;
            let offset1 = (2 * k + 1) as f32;
            // The merged sample sits at the weighted mean of the two taps it
            // stands in for; with no weight at all there is nothing to average,
            // so it stays on the first of them.
            let merged_offset = if merged_weight > 0.0 {
                (w1 * offset1 + w2 * (offset1 + 1.0)) / merged_weight
            } else {
                offset1
            };
            if let (Some(weight), Some(offset)) = (weights.get_mut(k), offsets.get_mut(k)) {
                *weight = merged_weight;
                *offset = merged_offset;
                n_taps = k.saturating_add(1);
            }
        }

        // An odd number of taps on the positive side leaves one over. It is
        // sampled at whole-texel offset, so bilinear filtering reads it alone.
        if let [leftover] = remainder
            && let (Some(weight), Some(offset)) = (weights.get_mut(n_taps), offsets.get_mut(n_taps))
        {
            *weight = *leftover;
            *offset = radius as f32;
            n_taps = n_taps.saturating_add(1);
        }

        Self {
            center_weight,
            weights,
            offsets,
            n_taps: u8::try_from(n_taps).unwrap_or(0),
        }
    }
}

/// The packed header of a blur's parameter block.
///
/// See the bit layout documented in `shaders/filter.wgsl`.
const fn pack_blur_header(
    filter_type: u32,
    edge_mode: u32,
    n_decimations: u32,
    n_linear_taps: u32,
) -> u32 {
    (filter_type & 0x1F)
        | ((edge_mode & 0x3) << 5)
        | ((n_decimations & 0xF) << 7)
        | ((n_linear_taps & 0x3) << 11)
}

/// A Gaussian blur's parameter block, as the fragment stage reads it.
///
/// `_padding` carries the block to [`FILTER_SIZE_BYTES`]; the blur needs none
/// of it, but a drop shadow's block is larger and every kind shares one stride.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, Zeroable, Pod)]
pub struct GpuGaussianBlur {
    /// Packed filter kind, edge mode, decimation count and tap count.
    pub header: u32,
    /// Weight of the kernel's centre tap.
    pub center_weight: f32,
    /// Merged weight of each bilinear tap pair.
    pub linear_weights: [f32; MAX_TAPS_PER_SIDE],
    /// Fractional offset of each bilinear tap pair.
    pub linear_offsets: [f32; MAX_TAPS_PER_SIDE],
    /// Unused by the blur; present so every filter kind is one stride wide.
    pub _padding: [u32; 4],
}

impl From<&GaussianBlur> for GpuGaussianBlur {
    fn from(blur: &GaussianBlur) -> Self {
        let kernel = LinearKernel::new(&blur.kernel, blur.kernel_size);

        Self {
            header: pack_blur_header(
                filter_type::GAUSSIAN_BLUR,
                edge_mode_code(blur.edge_mode),
                // Four bits is room for fifteen halvings, which is more than a
                // page-sized layer has axes to halve; a plan past that is
                // truncated by the mask rather than corrupting the fields above
                // it, and the pass sequence is planned from the plan itself
                // rather than from the header.
                u32::try_from(blur.n_decimations).unwrap_or(u32::MAX),
                u32::from(kernel.n_taps),
            ),
            center_weight: kernel.center_weight,
            linear_weights: kernel.weights,
            linear_offsets: kernel.offsets,
            _padding: [0; 4],
        }
    }
}

/// A filter's parameter block with its kind erased, as it is serialized into
/// the filter-data texture.
///
/// Every kind casts to this because every kind is [`FILTER_SIZE_BYTES`] wide;
/// the fragment stage reads the kind back out of the header.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, Zeroable, Pod)]
pub struct GpuFilterData {
    data: [u32; FILTER_SIZE_U32],
}

impl GpuFilterData {
    /// Texels one parameter block occupies in the filter-data texture — the
    /// stride a filter's own texel offset is a multiple of.
    pub const SIZE_TEXELS: u32 = (FILTER_SIZE_BYTES / BYTES_PER_TEXEL) as u32;

    /// The filter kind the header names, one of [`filter_type`]'s constants.
    #[must_use]
    pub fn filter_type(&self) -> u32 {
        self.data.first().copied().unwrap_or_default() & 0x1F
    }

    /// How many 2x decimation levels the header names, meaningful for a blur.
    #[must_use]
    pub fn n_decimations(&self) -> usize {
        ((self.data.first().copied().unwrap_or_default() >> 7) & 0xF) as usize
    }

    /// The block's words, in the order they are uploaded.
    #[must_use]
    pub fn words(&self) -> &[u32; FILTER_SIZE_U32] {
        &self.data
    }
}

impl From<GpuGaussianBlur> for GpuFilterData {
    fn from(blur: GpuGaussianBlur) -> Self {
        bytemuck::cast(blur)
    }
}

/// One filter pass's per-instance vertex data.
///
/// Must stay byte-compatible with `FilterInstanceData` in
/// `shaders/filter.wgsl`. Every extent is a `u16` pair packed into one word,
/// the same packing the strip and copy instances use.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FilterInstanceData {
    /// Origin of the source region inside the source page.
    pub source_origin: u32,
    /// Extent of the source region.
    pub source_size: u32,
    /// Origin of the destination region inside the destination page.
    pub dest_origin: u32,
    /// Extent of the destination region.
    pub dest_size: u32,
    /// Extent of the whole destination page.
    pub dest_texture_size: u32,
    /// Texel offset of this filter's parameter block in the filter-data
    /// texture.
    pub filter_data_offset: u32,
    /// Extent of the filter layer before any decimation, which bounds the
    /// transparent border a decimated pass overdraws.
    pub original_size: u32,
    /// Which pass of the filter's sequence this instance runs, one of
    /// [`FilterPassKind::code`]'s values.
    pub filter_pass_kind: u32,
}

impl FilterInstanceData {
    /// The instance running `step` of a filter whose parameters start at
    /// `filter_data_offset`, from `source_origin` of the source page into
    /// `dest_origin` of a destination page of `dest_texture_size`.
    ///
    /// `original_size` is the filter layer's own extent, before decimation.
    #[must_use]
    pub fn new(
        step: &FilterStep,
        filter_data_offset: u32,
        source_origin: (u16, u16),
        dest_origin: (u16, u16),
        dest_texture_size: SizeU16,
        original_size: SizeU16,
    ) -> Self {
        Self {
            source_origin: pack_u16_pair(source_origin.0, source_origin.1),
            source_size: pack_u16_pair(step.source.width(), step.source.height()),
            dest_origin: pack_u16_pair(dest_origin.0, dest_origin.1),
            dest_size: pack_u16_pair(step.dest.width(), step.dest.height()),
            dest_texture_size: pack_u16_pair(dest_texture_size.width(), dest_texture_size.height()),
            filter_data_offset,
            original_size: pack_u16_pair(original_size.width(), original_size.height()),
            filter_pass_kind: step.kind.code(),
        }
    }
}

/// Two `u16`s in one word, low half first — the packing every engine instance
/// layout uses and `unpack_u16_pair` in `shaders/helpers.wgsl` reads.
#[must_use]
pub const fn pack_u16_pair(low: u16, high: u16) -> u32 {
    (low as u32) | ((high as u32) << 16)
}

/// The passes a blur of `blur` over a layer of `size` renders as, in execution
/// order.
///
/// The shape is the reference's `emit_blur_sequence` followed by its
/// `ensure_result_in_original`: `n_decimations` halvings, one horizontal and
/// one vertical convolution at the decimated resolution, then the matching
/// doublings back. Decimating is what keeps the convolution kernel bounded —
/// the plan halves until the remaining variance is small enough for a kernel of
/// at most [`MAX_KERNEL_SIZE`] taps, so a σ of 32 costs more passes rather than
/// a wider kernel.
///
/// Each step names the extent it reads and the extent it writes; a step's
/// destination is the next step's source, on the opposite page.
#[must_use]
pub fn blur_passes(blur: &GaussianBlur, size: SizeU16) -> Vec<FilterStep> {
    let mut sizer = DecimationSizer::new(size.width(), size.height());
    let mut steps: Vec<FilterStep> = Vec::new();

    // Counted rather than inferred: `DecimationSizer::upscale` pops the stack
    // its downscales pushed, so an upscale emitted without a matching downscale
    // would panic inside it (E17). Pairing them through this counter is what
    // makes that unreachable by construction rather than by argument.
    let mut pending_upscales = 0_usize;
    for _ in 0..blur.n_decimations {
        let source = current(&sizer);
        let (width, height) = sizer.downscale();
        steps.push(FilterStep {
            kind: FilterPassKind::Downscale,
            source,
            dest: SizeU16::from_wh(width, height),
        });
        pending_upscales = pending_upscales.saturating_add(1);
    }

    let decimated = current(&sizer);
    steps.push(FilterStep {
        kind: FilterPassKind::BlurH,
        source: decimated,
        dest: decimated,
    });
    steps.push(FilterStep {
        kind: FilterPassKind::BlurV,
        source: decimated,
        dest: decimated,
    });

    while pending_upscales > 0 {
        let source = current(&sizer);
        let (width, height) = sizer.upscale();
        steps.push(FilterStep {
            kind: FilterPassKind::Upscale,
            source,
            dest: SizeU16::from_wh(width, height),
        });
        pending_upscales = pending_upscales.saturating_sub(1);
    }

    // The reference's `ensure_result_in_original`. A pass writes the page it
    // did not read, so an odd-length sequence would leave the result in the
    // scratch page rather than the one the parent's composite samples. A blur's
    // sequence is `2 * n_decimations + 2` passes and so always even; this is
    // what keeps that a checked property rather than an assumed one.
    if !steps.len().is_multiple_of(2) {
        let size = current(&sizer);
        steps.push(FilterStep {
            kind: FilterPassKind::Copy,
            source: size,
            dest: size,
        });
    }

    steps
}

/// The sizer's current extent.
fn current(sizer: &DecimationSizer) -> SizeU16 {
    let (width, height) = sizer.current();
    SizeU16::from_wh(width, height)
}
