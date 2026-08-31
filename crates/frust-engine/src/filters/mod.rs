//! Layer filters: a layer rendered in isolation, run through a sequence of
//! filter passes, and composited from the page the last of them wrote.
//!
//! An opacity layer needs one page and one extra pass ([`schedule`]); a
//! filtered layer needs a *second* page as well, because a filter pass reads a
//! whole image and writes a whole image and a render pass cannot do both to one
//! texture. The two pages the scheduler already ping-pongs between are exactly
//! that pair: the layer's contents land in one, each pass writes the one it did
//! not read, and the sequence is arranged so the result ends back in the page
//! the parent's composite samples.
//!
//! [`schedule`]: crate::schedule
//!
//! ## What is served
//!
//! One filter: [`LayerFilter::Blur`], the Gaussian blur that the theme layer's
//! `GlassMaterial` backdrop needs and that nothing in the engine could express
//! before. Every other filter a recording can carry — a flood, an offset, a
//! drop shadow, or a graph of more than one primitive — is refused by
//! [`served_blur`] with a reason naming what was found, and the frame is
//! skipped rather than rendered without its filter. That refusal is not a
//! placeholder for a fallback: the engine tier carries no second renderer (see
//! the [`schedule`] module header).
//!
//! The scene seam is deliberately absent. `frust_scene` has no filter command
//! and gains none here — [`push_filter_layer`] is how the engine's own compiler
//! will open a filtered layer once a later plan gives the display list
//! something to lower from. This module proves the engine can render one.
//!
//! ## What is pure, and what is not
//!
//! Everything here is a decision over plain values, in the crate's usual split:
//! which passes a blur costs and at what extents ([`blur::blur_passes`]), what
//! the fragment stage reads out of the filter-data texture
//! ([`blur::GpuGaussianBlur`]), and how one pass's instance is packed
//! ([`blur::FilterInstanceData`]). No device is touched and no texture is
//! allocated here.
//!
//! The three device-side halves live where every other engine pipeline's do:
//! the WGSL program is assembled by [`crate::gpu::shader_src::FILTER`], the
//! pipeline it is drawn through is
//! [`crate::gpu::pipelines::EnginePipeline::Filter`], and the texture holding
//! the frame's parameter blocks plus the bilinear sampler the kernels read
//! through are [`crate::gpu::targets`]'. The renderer binds the pages the
//! scheduler named and issues one instanced quad per pass
//! ([`crate::renderer::FilterResources`]).

pub mod blur;

use kurbo::Affine;
use vello_common::filter::gaussian_blur::GaussianBlur;
use vello_common::filter::{FilterData, PreparedFilter};
use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_common::geometry::SizeU16;
use vello_common::record::{CommandRecorder, LayerProps, RecordedLayerKind};

use crate::error::EngineError;

/// The largest standard deviation a blur layer is served at.
///
/// A blur expands its layer by 3σ on every side, snapped up to the tile grid
/// and carried as a `u16`; a σ past this bound would wrap that padding instead
/// of expanding the layer. It sits an order of magnitude above the largest page
/// the scheduler will ever size (the default ceiling is 4096 texels per axis),
/// so nothing a frame could have rendered is refused by it.
pub const MAX_BLUR_SIGMA: f32 = 4096.0;

/// A filter the engine can apply to an isolated layer.
///
/// Engine-internal on purpose: this is the vocabulary
/// [`push_filter_layer`] records with, not a `frust_scene` command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerFilter {
    /// A Gaussian blur of standard deviation `sigma`, in the layer's own user
    /// space — the transform in force when the layer is opened scales it into
    /// device space.
    Blur {
        /// The blur's standard deviation. Zero blurs nothing.
        sigma: f32,
    },
}

impl LayerFilter {
    /// The recorder-side description of this filter under `transform`.
    ///
    /// This is what carries the filter's *expansion* into the recording: a
    /// blurred layer's tile-aligned bounds grow by 3σ on every side, because
    /// the blur paints outside the contents that produced it, and the page the
    /// layer is rendered into is sized from those grown bounds.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidGeometry`] for a σ that is not finite, is
    /// negative, or is past [`MAX_BLUR_SIGMA`], and
    /// [`EngineError::InvalidTransform`] for a transform that is not finite —
    /// the same terms the compiler already refuses a draw's own geometry and
    /// transform on, checked here because every quantity below is derived from
    /// both.
    pub fn filter_data(self, transform: Affine) -> Result<FilterData, EngineError> {
        if !transform.as_coeffs().iter().all(|coeff| coeff.is_finite()) {
            return Err(EngineError::InvalidTransform);
        }

        match self {
            Self::Blur { sigma } => {
                // A range check rather than a chain of comparisons, which also
                // settles the non-finite cases: neither a NaN nor an infinity
                // is contained by it.
                if !(0.0..=MAX_BLUR_SIGMA).contains(&sigma) {
                    return Err(EngineError::InvalidGeometry);
                }

                let filter = Filter::from_primitive(FilterPrimitive::GaussianBlur {
                    std_deviation: sigma,
                    edge_mode: EdgeMode::default(),
                });

                Ok(FilterData::new(filter, transform))
            }
        }
    }
}

/// Opens a filtered layer on `recorder` — the engine-internal counterpart of
/// [`CommandRecorder::push_layer`] for a layer that carries a filter.
///
/// Close it with [`CommandRecorder::pop_layer`], exactly like a regular layer.
/// Popping is what computes the layer's bounds, and for a filtered layer those
/// bounds are the *expanded* ones the filter paints into, which is what the
/// scheduler sizes the layer's pages from.
///
/// # Errors
///
/// Whatever [`LayerFilter::filter_data`] refuses; nothing is recorded when it
/// does, so a refused filter leaves the recording exactly as it was rather than
/// opening a layer no `pop_layer` will balance.
pub fn push_filter_layer<D>(
    recorder: &mut CommandRecorder<D>,
    props: LayerProps,
    filter: LayerFilter,
    transform: Affine,
) -> Result<(), EngineError> {
    let filter_data = filter.filter_data(transform)?;
    recorder.push_layer(props, Some(filter_data));
    Ok(())
}

/// The blur that layer `id`'s recorded filter describes, or a reason naming
/// what was found instead.
///
/// The reason is prose rather than an error variant because it reaches a log
/// through [`EngineError::SchedulerEscalation`], where a caller reading it is
/// trying to find out which recorded shape froze a surface.
///
/// # Errors
///
/// A `kind` that is not a filter at all, a filter graph of anything but one
/// primitive, a primitive that is not a Gaussian blur, or a blur whose
/// prepared kernel is not a Gaussian blur after all.
pub fn served_blur(id: u32, kind: &RecordedLayerKind) -> Result<GaussianBlur, String> {
    let RecordedLayerKind::Filter { filter_data, .. } = kind else {
        return Err(format!("layer {id} carries no filter"));
    };

    // Checked before preparing rather than after: `PreparedFilter::new` panics
    // on a graph the reference has not implemented, and a recording is not
    // trusted to hold only the shapes the engine's own compiler records (E17).
    let primitives = filter_data.filter.graph.primitives.as_slice();
    let [FilterPrimitive::GaussianBlur { std_deviation, .. }] = primitives else {
        return Err(format!(
            "layer {id} carries a filter graph of {} primitive(s) the engine does not serve; the \
             only filter it renders is a single Gaussian blur",
            primitives.len()
        ));
    };

    if !(0.0..=MAX_BLUR_SIGMA).contains(std_deviation) {
        return Err(format!(
            "layer {id} carries a Gaussian blur of σ {std_deviation}, which is not a standard \
             deviation this engine blurs at (0 to {MAX_BLUR_SIGMA})"
        ));
    }

    match PreparedFilter::new(&filter_data.filter, &filter_data.transform) {
        PreparedFilter::GaussianBlur(blur) => Ok(blur),
        _ => Err(format!(
            "layer {id} carries a Gaussian blur that prepared as another filter"
        )),
    }
}

/// One pass of a filter's sequence.
///
/// The numbering is the wire format the fragment stage switches on; it matches
/// the reference renderer's own so the kinds this engine does not serve —
/// flood (1), offset (2) and the two drop-shadow composites (7, 8) — can be
/// added later without renumbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterPassKind {
    /// Copy the source region through unchanged.
    Copy,
    /// Halve both axes.
    Downscale,
    /// Convolve horizontally with the blur kernel.
    BlurH,
    /// Convolve vertically with the blur kernel.
    BlurV,
    /// Double both axes.
    Upscale,
}

impl FilterPassKind {
    /// The value `filter_pass_kind` carries into the fragment stage.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Copy => 0,
            Self::Downscale => 3,
            Self::BlurH => 4,
            Self::BlurV => 5,
            Self::Upscale => 6,
        }
    }
}

/// One pass of a filter's sequence, with the extents it reads and writes.
///
/// A pass reads the page the pass before it wrote and writes the other one, so
/// `source` is the previous step's `dest`; the two differ only for the
/// rescaling passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterStep {
    /// Which pass this is.
    pub kind: FilterPassKind,
    /// Extent of the region read, at the source page's origin.
    pub source: SizeU16,
    /// Extent of the region written, at the destination page's origin.
    pub dest: SizeU16,
}
