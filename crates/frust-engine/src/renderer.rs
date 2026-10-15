//! [`EngineRenderer`]: the public seam a host drives one surface's 2D frames
//! through.
//!
//! # The encode contract
//!
//! [`EngineRenderer::encode`] **records into a `wgpu::CommandEncoder` the
//! caller owns and never submits it.** That is the whole point of the seam: a
//! host compositing 2D over its own 3D content records its passes before and
//! after the engine's into one encoder and submits once, and a submit hidden
//! inside the engine would split that into two command buffers with a pipeline
//! flush between them. Two obligations follow, and both are contract rather
//! than preference:
//!
//! - every pass the engine begins is ended before `encode` returns, so the
//!   caller's next `begin_render_pass` on the same encoder is legal; and
//! - the engine issues no `queue.submit` for scene work. It does issue
//!   `queue.write_texture`/`write_buffer` uploads, which are ordered ahead of
//!   the command buffers submitted after them and so land before the passes
//!   that read them.
//!
//! One thing the engine *does* submit, and it is worth naming precisely because
//! the rule above is otherwise absolute: growing the image atlas array submits a
//! command buffer of its own, holding one texture copy and nothing else, on the
//! rare frame that grows it. That is **maintenance, not scene work** — it never
//! touches the caller's encoder, records no pass and no draw, and exists because
//! the queued writes it has to precede would otherwise be flushed ahead of it
//! (see [`crate::gpu::atlas`]'s *Why growth submits a command buffer of its
//! own*). The caller's encoder is still never submitted by the engine, and the
//! frame's own passes still reach the queue only when the caller submits it.
//!
//! # Sharing the depth attachment
//!
//! A caller recording its own depth-writing passes into that encoder hands the
//! same attachment in as `EngineTarget::depth`, and the two renderers then
//! occlude each other correctly in either order. Three rules make that work,
//! and [`crate::gpu::depth`] is where they are stated in full: the shared
//! comparison and which end of the range is near
//! ([`DEPTH_COMPARE`](crate::gpu::depth::DEPTH_COMPARE) over a buffer whose far
//! plane is [`DEPTH_CLEAR`](crate::gpu::depth::DEPTH_CLEAR)); the depth
//! attachment's extent matching the colour target's; and who owns the clear —
//! whichever pass runs first in the encoder, which the caller states through
//! [`EngineRenderer::set_depth_pre_cleared`].
//!
//! What that contract does *not* extend to is colour. The frame's opening
//! below clears the frame's colour target unconditionally, so content painted
//! into that target before `encode` keeps its depth and loses its pixels: a
//! host compositing over its own content records that content after the frame,
//! or into a target of its own the frame composites.
//!
//! # The frame's passes
//!
//! A frame records into that encoder, in this order.
//!
//! 1. **The opening** ([`FramePass::Opening`]), the first pass over the
//!    surface, which does three things in one render pass:
//!    - **clears** the colour target to the frame's base colour, and the depth
//!      attachment to the far plane unless the caller stated it is already
//!      populated (see [`crate::gpu::depth`]) — the pass's own load
//!      operations, so a frame with no draws at all still resolves to a clean
//!      surface;
//!    - draws the **opaque strips**, depth-tested and depth-writing,
//!      unblended: only the fully-covered interior spans of opaque draws
//!      *targeting the surface* — an anti-aliased edge is by definition not
//!      opaque, and a page carries no depth attachment for the split to be
//!      sound against. The depth they establish is what lets every blended
//!      strip after them reject fragments an opaque draw in front of it
//!      already covered;
//!    - then draws the **first surface round's own strips** (pass 2 below),
//!      in order, the pipeline switching between the two inside the pass.
//!
//!    It ends exactly where that round's own pass would have ended — at a
//!    punch cut, at a page or filter round, or at the end of the frame. Page
//!    and filter rounds scheduled ahead of the first surface round run ahead
//!    of the opening: they write pooled pages and read neither the surface nor
//!    its depth, so the opening waits past them. A punch issued before any
//!    surface round draws takes the opening as a pass of its own, ahead of the
//!    erase, and so does the end of a frame whose surface rounds drew nothing
//!    blended.
//!
//!    The opaque strips are drawn once, not once per surface round, and that
//!    is a correctness requirement: the surface can take several rounds (a
//!    [cut](crate::schedule::cut_at) round is how a wide sibling fan is
//!    served), and re-drawing them ahead of each would re-draw opaque coverage
//!    at equal stored depth over composites the round before it had already
//!    blended.
//!
//!    One pass rather than three is what a tile-based GPU is paid for: every
//!    pass over the surface is a whole-framebuffer load into tile memory and a
//!    store back out, and the clear and the opaque strips were two such passes
//!    that drew little or nothing of their own. The pixels are identical —
//!    the same draws in the same order against the same attachments, only the
//!    pass boundaries gone. `FRUST_ENGINE_NO_PASS_MERGE`
//!    ([`EngineRenderer::set_pass_merge`]) records them apart again: a
//!    [`FramePass::Clear`] pass drawing nothing and a [`FramePass::Opaque`]
//!    pass loading it, ahead of every round, and then every surface round in
//!    a pass of its own.
//! 2. **One pass per [round](crate::schedule)**, the first surface round that
//!    draws excepted — it is drawn inside the opening. A page round
//!    ([`FramePass::Page`]) renders one isolated layer into a pooled
//!    intermediate [page](crate::schedule::pages) it clears to transparent, at
//!    the page's own origin — so every instance is shifted by the page's
//!    tile-aligned bounds, and clipped to them, which is what makes a
//!    [banded](crate::schedule::pages::page_bands) layer's column pages tile
//!    their layer instead of each holding a clamped copy of it (see
//!    [`PageWindow`]) — and through the page's own viewport uniform; a round
//!    continuing a page an earlier round of the same layer opened loads it
//!    instead. A surface round ([`FramePass::Surface`]) draws **alpha
//!    strips**, premultiplied-blended, in painter order, loading the surface:
//!    depth-tested but not depth-writing when a depth attachment is in play, a
//!    plain painter's-algorithm pass when it is not. A surface round with
//!    nothing blended to draw records no pass. A round's ops run in the order
//!    [`Schedule::build`] listed them, so a finished child page composites into
//!    its parent exactly where the recording entered it.
//!
//!    A **filter round** is the one round that draws no strip at all: it runs
//!    one pass of a [filter](crate::filters)'s sequence, one instanced quad
//!    through [`EnginePipeline::Filter`], reading the layer's other pooled page
//!    through the engine's only sampler and clearing the page it writes (see
//!    [`FilterResources`]). It is an ordinary round of this walk in every other
//!    respect — recorded into the caller's own encoder, in the order the
//!    scheduler listed it, with its pages handed back the moment its pass ends.
//!    It is *not* an own-encoder exception; the atlas replay remains the only
//!    one of those.
//!
//!    A **region-copy round** ([`FramePass::Copy`]) is a backdrop blur's
//!    first: it reads a region of the frame's own target back into a pooled
//!    page ([`RegionCopyMode`]) for the blur's filter rounds to run over. It is
//!    the one page round that *follows* the opening rather than waiting past
//!    it — it reads the surface, so the clear, the opaque strips and every
//!    surface round ahead of it in painter order must have landed (the
//!    scheduler cuts the surface round there; a punch recorded ahead of it is
//!    erased first) — and, for the same reason, no draw at or past the first
//!    backdrop's depth is routed into the opaque strips. The backdrop's own
//!    rectangle is then drawn in the next surface round as strip instances
//!    reading the blurred page through the composite's layer input, so a clip
//!    open around it masks it like any fill.
//! 3. **The hole punch** ([`FramePass::Punch`]), destination-out, when the
//!    frame recorded a `ClearRect` — see [`crate::compile::clear`] for the
//!    whole contract this pass implements. It is issued at the punch's own painter-order position,
//!    not at the end of the frame: a surface round is *cut* where the punch
//!    was recorded, the punch pass goes into that cut, and the round's
//!    remaining ops resume in a pass of their own after it. Everything drawn
//!    over the slot is therefore recorded after the erase and survives it,
//!    whether or not it wrote depth. A punch past every op of the frame — the
//!    ordinary case, a `ClearRect` recorded last — cuts nothing and lands after
//!    the last round exactly as it always did.
//!
//! With depth unavailable — no attachment, or `FRUST_ENGINE_NO_DEPTH` set —
//! the opening draws no opaque strips (and, unmerged, the opaque pass
//! disappears) and every instance travels through the surface rounds, blended
//! in painter order. That is a correctness requirement rather than a
//! fallback detail: routing the opaque spans into a separate, earlier pass is
//! only sound because the depth buffer re-establishes their ordering against the
//! blended ones.
//!
//! # Compositing a layer
//!
//! A finished page reaches its parent as ONE instanced quad through the strip
//! program every draw goes through (its [`PaintClass::LayerComposite`]
//! specialisation — see *Paint-class runs*), flagged as a whole rectangle and
//! naming the layer colour source: the fragment stage then reads the page
//! bound as `layer_input_texture` at the quad's own texel and scales it by the
//! opacity packed into the instance's low byte. The page is bound through a
//! bind group of its own rather than the frame's shared one, because group 0
//! carries both the pass's viewport uniform and that layer input, and a page
//! round's viewport is its own.
//!
//! A retained page ([`crate::cache::retained`]) is composited by exactly the
//! same quad, bound the same way; only the texture differs — the store's own,
//! named by the retained bracket rather than by a page group. A round
//! rendering a body into one is a page round in every respect above (cleared on
//! its first round, loaded on a continued one, through its own viewport
//! uniform, never taking the surface opening) and is listed as a
//! [`FramePass::Page`]. A `DrawRetained`'s composite carries the painter-order
//! index the command itself was recorded at.
//!
//! A composite carries the deepest painter's-order index of everything inside
//! the layer it composites, nested layers included. That is what keeps a
//! translucent layer correctly ordered against the root round's own draws
//! without giving the scheduler a depth model: every draw recorded before the
//! layer sits behind that index and every draw recorded after it sits in front.
//!
//! # Paint-class runs
//!
//! Every strip pass is drawn in **runs**, each through the pipeline of one
//! [`PaintClass`] — the strip program specialised to the paints the run holds
//! (see [`crate::gpu::pipelines`]'s *The paint-class axis*). The runs are cut
//! when a segment is planned (`Scratch::push_run`, by
//! [`merge_class_runs`]): contiguous, in painter order, covering the segment
//! exactly, so a pass draws the same instances in the same order it always did
//! and only the program shading each run changes. A run shorter than the
//! renderer's threshold ([`EngineRenderer::run_split_min`],
//! [`crate::gpu::strips::RUN_SPLIT_MIN`] by default) is merged and drawn with
//! [`PaintClass::Uber`] rather than paying a pipeline switch of its own; a
//! composite always takes [`PaintClass::LayerComposite`], the opaque pass its
//! own runs, and a punch [`PaintClass::SolidCoverage`]. A pass sets a pipeline
//! only where the class changes between runs.
//!
//! `FRUST_ENGINE_UBER_PROGRAM` ([`EngineRenderer::set_uber_program`]) turns all
//! of it off: no segment is split, every pass draws through its state's one
//! Uber pipeline, and no class program is warmed or built.
//!
//! `FRUST_ENGINE_IMAGE_SAMPLER` ([`EngineRenderer::with_image_sampler`]) picks
//! the program *family* the classes are compiled from, not a class: in the
//! sampler family — the default — every program that reads atlas images (the
//! image classes and Uber) filters a bilinear image through one hardware
//! sampler, which every strip bind group then carries beside the atlas array;
//! its kill switch (`0`) compiles the manual four-tap family instead. The
//! runs, and the pipelines a frame resolves, are the same in both families.
//!
//! # Paint resolution
//!
//! A solid colour travels inside the strip instance itself. Anything else the
//! compiler encoded — a gradient, an image, a blurred rounded rectangle — is
//! resolved once per frame before a single instance is built, in three steps
//! that have to happen in this order:
//!
//! 1. residency is settled for the whole frame: the frame's LUT requests are
//!    serviced through the [`GradientCache`] so every gradient's colour ramp
//!    has an offset into the packed LUT buffer, and every image paint's atlas
//!    rectangle is looked up (or learned, the first time it is drawn) in the
//!    renderer's own image registry — see [`FrameResources::resolve_paints`];
//! 2. each encoded paint is lowered into the [`GpuEncodedPaint`] record the
//!    fragment shader samples, carrying that residency; and
//! 3. the records are serialized back to back, which fixes the texel each one
//!    starts at — the index a strip instance names its paint by.
//!
//! Ramp offsets are only valid within the frame that took them: the cache
//! compacts and rewrites them in [`EngineRenderer::end_frame`], which is why
//! residency is decided here rather than at compile time. An image's atlas
//! rectangle, by contrast, is stable for as long as the image stays resident
//! (the compiler's [`crate::cache::images::ImageResidency`] does not move a
//! live image), so the renderer's own registry only ever forgets an entry
//! when the frame that compiled it reports the entry's region evicted.
//!
//! A paint that still cannot be resolved — an image the atlas has no room
//! for, a gradient whose ramp could not be baked, an external texture (nothing
//! binds one yet) — leaves its draw skipped rather than stamped in a wrong
//! colour: the same "a frame draws less, never wrong" rule the compiler
//! follows for the commands it does not lower.
//!
//! An image the atlas holds a *minified* copy of is the one paint whose lowered
//! record needs a correction here. The compiler composed its natural-to-device
//! transform against the source's declared extent, before residency was
//! consulted and so before the fit was known; the shader samples the resident
//! rectangle. [`ResidentImage::minify_scale`] is the ratio between the two, and
//! folding it into the lowered record's transform is what keeps a downsampled
//! image landing on the destination rectangle the display list asked for.

use core::ops::Range;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Once};
// Durations and the compile's own phase record are read by the `perf-trace`
// encode window and by the tests that pin its arithmetic, and by nothing else.
#[cfg(any(test, feature = "perf-trace"))]
use std::time::Duration;

use frust_gpu::{PipelineCache, PooledTexture, SceneTextureId, ShaderLibrary, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use glifo::{AtlasCommand, AtlasCommandRecorder, AtlasPaint};
use kurbo::Affine;
use peniko::{Brush, Color};
use vello_common::encode::{EncodedImage, EncodedPaint};
use vello_common::fearless_simd::Level;
use vello_common::paint::{ImageId, ImageSource, Paint};
use vello_common::strip::Strip;

use crate::cache::images::{ATLAS_PADDING, AtlasRegion};
use crate::cache::retained::{
    RetainedBudget, RetainedDecision, RetainedEntry, RetainedPages, RetainedPlan,
};
use crate::cache::{
    AtlasBudget, BYTES_PER_TEXEL, CachedRamp, GradientCache, GradientTextureLayout, ResidentImage,
};
#[cfg(any(test, feature = "perf-trace"))]
use crate::compile::CompileSpans;
use crate::compile::paint::resolve_lut_request;
use crate::compile::{
    ClearPunch, CompiledFrame, GlyphQuad, PhaseClock, RetainedKind, SceneCompiler,
};
use crate::config;
use crate::diag::{EngineSpan, FrameTimestamps};
use crate::error::EngineError;
use crate::filters::blur::{FilterInstanceData, GpuFilterData, GpuGaussianBlur};
use crate::filters::drop_shadow::GpuDropShadow;
use crate::filters::{Backdrop, FilterStep, ServedFilter, served_filter};
use crate::gpu::atlas::{
    AtlasPageBuffers, AtlasRenderReport, AtlasRenderer, lower_encoded_image, push_solid_strips,
};
use crate::gpu::bindings::{
    ATLAS_SAMPLER_BINDING, ExternalRuns, ExternalTextures, lower_encoded_external,
};
use crate::gpu::config::resource_widths_agree;
use crate::gpu::depth::DepthAttachment;
use crate::gpu::paint_texture::lower_encoded_paint;
use crate::gpu::pipelines::{
    EnginePipeline, EngineShaders, INTERMEDIATE_FORMAT, atlas_strip_desc, warm_up_descs_for,
};
use crate::gpu::strips::{
    ClassRun, ClassSet, PaintClass, PaintType, merge_class_runs, pack_paint_descriptor,
};
use crate::gpu::targets::{
    IntermediateTargets, IntermediateTexture, filter_data_texture_descriptor,
    filter_data_texture_height, filter_sampler,
};
use crate::gpu::{self, AtlasArray, GpuConfig, GpuEncodedPaint, GpuStrip, StripDraw};
use crate::schedule::pages::{PageConfig, PageSize};
use crate::schedule::{
    Composite, MAX_LIVE_PAGES, PageParity, PageTarget, RetainedLayers, RetainedRole,
    RetainedTarget, Round, RoundOp, Schedule,
};
use crate::{EngineTarget, OutputAlpha};
use vello_common::geometry::RectU16;
use vello_common::geometry::SizeU16;
use vello_common::record::RecordedLayerKind;

/// The packed paint descriptor of an inline premultiplied solid colour.
///
/// A solid paint indexes no encoded-paint record, so its descriptor carries
/// only the colour source and paint type — both of which are zero, which is
/// why this is named rather than written as a bare `0` at its call site: the
/// zero is a coincidence of the layout, not an absence of information.
const SOLID_PAINT: u32 = pack_paint_descriptor(PaintType::Solid, 0);

/// The smallest resource-texture dimension the engine can address.
///
/// A resource texture's row stride is its width times its texel size, and
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` is 256; the narrowest of the three
/// resources is the gradient LUT at 4 bytes per texel, so 64 texels is the
/// point below which an upload's `bytes_per_row` stops being legal.
const MIN_RESOURCE_TEXTURE_DIM: u32 = 64;

/// The smallest strip instance buffer the engine allocates, in instances.
///
/// A first frame with a handful of strips should not force a second allocation
/// on the second frame.
const MIN_INSTANCE_CAPACITY: u64 = 4096;

/// The colour source a composite instance names its finished page by:
/// `COLOR_SOURCE_LAYER` in bits 29-30 of the packed paint descriptor.
///
/// Deliberately not built through
/// [`pack_paint_descriptor`](crate::gpu::strips::pack_paint_descriptor): that
/// helper packs a paint type and a paint-record index into the low bits, and a
/// composite spends the same bits on a constant opacity instead. Two readings
/// of one word, so each is written where its own reading is obvious.
const LAYER_PAINT_SOURCE: u32 = 1 << 29;

/// The premultiplied source colour a hole-punch instance erases with.
///
/// Opaque white. Destination-out weights the erase by the source's own *alpha*
/// and multiplies its colour by zero, so the colour channels never reach the
/// target and only full alpha matters — it is what makes a fully covered pixel
/// read exactly `(0, 0, 0, 0)`.
const PUNCH_SOURCE: u32 = u32::MAX;

/// The label every intermediate page is acquired from the pool under.
const PAGE_LABEL: &str = "frust-engine layer page";

/// The label every filter round's pass is recorded under.
const FILTER_LABEL: &str = "frust-engine filter pass";

/// Vertices one filter pass's quad is built from, the same four-vertex
/// triangle strip every engine program expands an instance into.
const FILTER_QUAD_VERTICES: u32 = 4;

/// The smallest filter instance buffer the engine allocates, in instances.
///
/// A σ-32 blur is ten passes, so this is roughly "one deep blur costs no second
/// allocation"; the buffer is 32 bytes an instance and grows from here.
const MIN_FILTER_INSTANCE_CAPACITY: u64 = 16;

static INDEXED_PAINT_WARNING: Once = Once::new();

/// Raised the first time a frame's backdrop blurs are drawn as nothing because
/// its target can be neither copied nor sampled (see [`RegionCopyMode`]), on
/// the same once-per-process terms as the compiler's skip warnings.
static BACKDROP_UNREADABLE_WARNING: Once = Once::new();

/// Vertices the region-copy program's one quad draws (`copy.wgsl`'s
/// `quad_corner`, a triangle strip).
const COPY_QUAD_VERTICES: u32 = 4;

/// The bytes of one region-copy instance: four `u16` pairs
/// ([`crate::gpu::pipelines::copy_vertex_layout`]).
const COPY_INSTANCE_BYTES: u64 = 16;

/// Raised the first time an atlas region is declined, so a renderer whose
/// budget and array have gone out of agreement says so once rather than every
/// frame.
static ATLAS_REFUSAL_WARNING: Once = Once::new();

/// The CPU phases one [`EngineRenderer::encode_traced`] call splits into.
///
/// The GPU half of a frame is [`EngineSpan`]'s, measured by the timestamp ring
/// at pass boundaries; this is the CPU half, measured by lapping a
/// [`PhaseClock`] between the encode's own steps. The two answer different
/// questions and neither substitutes for the other — a text-heavy scene can
/// cost milliseconds here while its passes cost a fraction of one there.
///
/// The phases partition the call in the order they run, so [`Self::total`] is
/// the whole encode. [`counts`](Self::counts) trails them and is not one of
/// them.
///
/// - [`compile`](Self::compile) — [`SceneCompiler::compile`] end to end,
///   itself split six ways by [`CompileSpans`].
/// - [`schedule`](Self::schedule) — building the frame's pass plan and
///   settling its depth attachment.
/// - [`paints`](Self::paints) — resolving every draw's paint to a texel of the
///   encoded-paint texture.
/// - [`instances`](Self::instances) — building the per-round GPU instance
///   arrays from the recording.
/// - [`resize`](Self::resize) — the fallible capacity checks, the image
///   atlas's own growth, the glyph-rectangle clears an earlier frame's
///   eviction owes, and the three resource-texture resizes.
/// - [`upload`](Self::upload) — the frame's queue writes: alpha coverage,
///   encoded paints, gradient ramps, image texels and the instance arrays.
/// - [`replay`](Self::replay) — the render-to-atlas pass that draws newly
///   cached glyphs into their array layers (zero on a steady frame, which
///   caches none).
/// - [`pipelines`](Self::pipelines) — taking (or building) this frame's
///   pipelines and bind groups, and preparing the filter blocks.
/// - [`record`](Self::record) — recording the frame's passes into the caller's
///   encoder.
///
/// Compiled only under `perf-trace`, with everything that reads it: the whole
/// `frust-perf enc` route is a `#[cfg]` island rather than a runtime branch the
/// optimizer is trusted to fold, so a release-lean binary carries none of its
/// field names, format strings or window (see [`EncodeTrace`]).
#[cfg(feature = "perf-trace")]
#[derive(Debug, Clone, Copy, Default)]
struct EncodeSpans {
    /// Scene compilation, split further by its own six phases.
    compile: CompileSpans,
    /// Whole-call cost of [`SceneCompiler::compile`], call overhead included.
    ///
    /// Deliberately measured from outside rather than summed from
    /// [`Self::compile`]: the difference between the two is the part of the
    /// call the six inner phases do not cover, and a breakdown that could only
    /// report its own sum could never show that gap.
    compile_total: Duration,
    /// Pass planning and depth settlement.
    schedule: Duration,
    /// Paint resolution.
    paints: Duration,
    /// Instance-array building.
    instances: Duration,
    /// Capacity checks, atlas growth, glyph-rectangle clears, resource resizes.
    resize: Duration,
    /// The frame's queue writes.
    upload: Duration,
    /// The render-to-atlas pass for newly cached glyphs.
    replay: Duration,
    /// Pipeline/bind-group acquisition and filter preparation.
    pipelines: Duration,
    /// Recording the frame's passes into the caller's encoder.
    record: Duration,
    /// This frame's own draw, strip, alpha and glyph counts.
    ///
    /// Last because it is not a phase: it is what the phases above were spent
    /// on. Carried here because a phase's cost is only readable against the
    /// work it did — "the walk cost two milliseconds" says nothing on its own,
    /// while "two milliseconds over three thousand strips and three hundred
    /// glyphs" says where a lever would have to bite.
    counts: EncodeCounts,
}

/// The columns one `frust-perf enc` line reports, in row order.
///
/// The first seven are [`CompileSpans`]' own — six phases plus the `glyphs`
/// subset of the walk — and `compile` after them is the whole compile they sit
/// inside (so the six sum to at most it, never past it). The next eight are
/// [`EncodeSpans`]' remaining phases, and `total` closes the line. Pinned as
/// one list because the line's field order is what a capture is graded by —
/// the same contract the `frust-perf img` line keeps.
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_COLUMNS: [&str; 17] = [
    "validate",
    "prepare",
    "classify",
    "admit",
    "walk",
    "glyphs",
    "finish",
    "compile",
    "schedule",
    "paints",
    "instances",
    "resize",
    "upload",
    "replay",
    "pipelines",
    "record",
    "total",
];

/// The unitless per-frame counts the line reports after its phases, in row
/// order — what the phases above were spent on.
///
/// The field order is a capture contract: append only. The first five are
/// per-frame medians over the window. The next five are **sums over the
/// window** (frames' worth of events, not a typical frame):
///
/// - `replays` — frames in which the render-to-atlas replay pass ran.
/// - `dirty_pages` — atlas pages those replays rendered.
/// - `atlas_refused` — pages, clears and uploads the replays refused.
/// - `glyph_misses` — glyphs that took the cache-miss path.
/// - `glyph_evicted` — atlas rectangles the eviction pass freed.
///
/// The last, `run_keys`, is a per-frame median again: the distinct run
/// identities (font, size, hinting) the policy classified in a frame. The
/// replay figures read zero on a frame whose replay did not run.
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_COUNTS: [&str; 11] = [
    "draws",
    "strips",
    "alphas",
    "glyph_draws",
    "atlas_glyphs",
    "replays",
    "dirty_pages",
    "atlas_refused",
    "glyph_misses",
    "glyph_evicted",
    "run_keys",
];

/// Index into [`ENCODE_TRACE_COUNTS`] of the first count reported as a sum over
/// the window rather than its median, up to and excluding
/// [`ENCODE_TRACE_MEDIAN_AGAIN`]: `replays`, `dirty_pages`, `atlas_refused`,
/// `glyph_misses` and `glyph_evicted` are events, mostly zero on a frame, so a
/// median would hide exactly the frames the line exists to count.
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_FIRST_SUMMED: usize = 5;

/// Index into [`ENCODE_TRACE_COUNTS`] of the first count after the summed run
/// (`run_keys`, a per-frame level, reported as its median).
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_MEDIAN_AGAIN: usize = 10;

/// Width of one window row: every phase column followed by every count.
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_ROW: usize = ENCODE_TRACE_COLUMNS.len() + ENCODE_TRACE_COUNTS.len();

/// What one frame drew, carried beside its phases.
///
/// Counts rather than durations, and so reported without a unit suffix.
/// `perf-trace`-only, with [`EncodeSpans`], which is all that carries it.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Clone, Copy, Default)]
struct EncodeCounts {
    /// Recorded draws.
    draws: u32,
    /// Strips generated.
    strips: u32,
    /// Bytes of alpha coverage those strips index — the frame's largest
    /// single queue write.
    alphas: u32,
    /// Draws that painted one glyph outline.
    glyph_draws: u32,
    /// How many of those sampled the glyph atlas rather than rasterizing.
    atlas_glyphs: u32,
    /// 1 when the render-to-atlas replay ran this frame, else 0.
    replays: u32,
    /// Atlas pages the replay rendered this frame.
    dirty_pages: u32,
    /// Atlas pages, clears and uploads the replay refused this frame.
    atlas_refused: u32,
    /// Glyphs that took the cache-miss path while compiling this frame.
    glyph_misses: u32,
    /// Atlas rectangles this frame's eviction freed.
    glyph_evicted: u32,
    /// Distinct run identities classified this frame.
    run_keys: u32,
}

#[cfg(feature = "perf-trace")]
impl EncodeCounts {
    /// What `frame` drew, with the atlas figures the frame's replay and policy
    /// reported.
    fn of(
        frame: &CompiledFrame,
        replayed: bool,
        report: &AtlasRenderReport,
        policy: &crate::text::atlas_policy::AtlasTrace,
    ) -> Self {
        fn count(len: usize) -> u32 {
            u32::try_from(len).unwrap_or(u32::MAX)
        }
        Self {
            draws: count(frame.draws().len()),
            strips: count(frame.strip_buf().len()),
            alphas: count(frame.alphas().len()),
            glyph_draws: frame.glyph_draws,
            atlas_glyphs: frame.atlas_glyph_draws,
            replays: u32::from(replayed),
            dirty_pages: report.pages,
            atlas_refused: report.refused,
            glyph_misses: policy.glyph_misses,
            glyph_evicted: policy.glyph_evicted,
            run_keys: policy.run_keys(),
        }
    }
}

/// Frames one `frust-perf enc` line summarises — one line a second at 60 Hz.
///
/// A window rather than a line per frame: the encode is the very thing being
/// measured, so formatting and logging inside it once per frame would charge
/// the measurement to its own subject. One line per sixty frames keeps that
/// charge under a microsecond a frame while still resolving a scenario's
/// phases (a thirty-second run reports thirty times), and it matches the
/// cadence `frust-shell-common`'s own rate-limited frame summary already
/// emits at.
#[cfg(feature = "perf-trace")]
const ENCODE_TRACE_WINDOW: usize = 60;

#[cfg(feature = "perf-trace")]
impl EncodeSpans {
    /// The whole encode.
    ///
    /// Saturating throughout: a diagnostic sum must not take the frame with it
    /// on overflow (E17).
    fn total(&self) -> Duration {
        // `compile.glyphs` is deliberately absent: it is a subset of the walk
        // inside `compile_total`, and adding it would count text twice.
        [
            self.schedule,
            self.paints,
            self.instances,
            self.resize,
            self.upload,
            self.replay,
            self.pipelines,
            self.record,
        ]
        .iter()
        .fold(self.compile_total, |acc, span| acc.saturating_add(*span))
    }

    /// This frame's row, in [`ENCODE_TRACE_COLUMNS`] order, in nanoseconds.
    ///
    /// Nanoseconds in a `u32` rather than a `Duration` per column: a window of
    /// sixty rows is then three kilobytes of plain integers to sort, and no
    /// single phase of one frame reaches the four-second ceiling that would
    /// saturate one.
    fn row(&self) -> [u32; ENCODE_TRACE_ROW] {
        fn ns(span: Duration) -> u32 {
            u32::try_from(span.as_nanos()).unwrap_or(u32::MAX)
        }
        [
            ns(self.compile.validate),
            ns(self.compile.prepare),
            ns(self.compile.classify),
            ns(self.compile.admit),
            ns(self.compile.walk),
            ns(self.compile.glyphs),
            ns(self.compile.finish),
            ns(self.compile_total),
            ns(self.schedule),
            ns(self.paints),
            ns(self.instances),
            ns(self.resize),
            ns(self.upload),
            ns(self.replay),
            ns(self.pipelines),
            ns(self.record),
            ns(self.total()),
            self.counts.draws,
            self.counts.strips,
            self.counts.alphas,
            self.counts.glyph_draws,
            self.counts.atlas_glyphs,
            self.counts.replays,
            self.counts.dirty_pages,
            self.counts.atlas_refused,
            self.counts.glyph_misses,
            self.counts.glyph_evicted,
            self.counts.run_keys,
        ]
    }
}

/// The rolling window of per-frame [`EncodeSpans`] one `frust-perf enc` line
/// is computed over.
///
/// Absent altogether from a build without `perf-trace`, rather than inert in
/// one. The type, its window, its column names and the `frust-perf enc` literal
/// are all inside a `#[cfg]` island — including the renderer's own field — so
/// "no frame pays a push, a sort or a format" is what the compiler emitted and
/// not what the optimizer was expected to prove about a constant branch. That
/// distinction is the one the release-lean gate measures: it greps a shipping
/// binary for `frust-perf` and expects to find nothing.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Default)]
struct EncodeTrace {
    /// One row per frame in the window, in [`ENCODE_TRACE_COLUMNS`] order.
    window: Vec<[u32; ENCODE_TRACE_ROW]>,
    /// The column being ranked, kept across windows so a steady stream of
    /// them allocates once.
    ranked: Vec<u32>,
    /// Frames recorded since this renderer was built — the line's own `n`,
    /// which is the engine's count and deliberately not the host's raw-line
    /// `n` (a different emitter counting different frames).
    frames: u64,
}

#[cfg(feature = "perf-trace")]
impl EncodeTrace {
    /// Adds one frame's phases to the window, emitting the window's line when
    /// it fills.
    ///
    /// Called at the very end of the encode, so the formatting a full window
    /// costs falls outside every phase the line reports — the numbers describe
    /// the encode, not the encode plus its own accounting.
    fn record(&mut self, spans: &EncodeSpans) {
        // Saturating like every other counter on this path: a renderer that
        // outlived `u64::MAX` frames would report a wrapped `n`, and a wrong
        // number in a capture is worse than a stuck one.
        self.frames = self.frames.saturating_add(1);
        self.window.push(spans.row());
        if self.window.len() < ENCODE_TRACE_WINDOW {
            return;
        }
        let line = self.line();
        self.window.clear();
        log::info!("{line}");
    }

    /// The `frust-perf enc` line the current window reports.
    ///
    /// The counts `replays` through `glyph_evicted` are window sums; every
    /// other column (including `run_keys`) is its own median over the window.
    /// The trailing field is the whole encode's 95th percentile, so a column
    /// answers "what does this phase usually cost" while the tail answers "how
    /// bad is a bad frame". Medians are taken per column and so do not sum to
    /// the `total_us`
    /// median exactly — they are close on a steady scene, and the gap is
    /// itself the signal that the phases are not moving together.
    fn line(&mut self) -> String {
        // Sized for the longest line the loops below write: a
        // `<name>_us=<7 digits>.<1>` field per phase, a `<name>=<10 digits>`
        // field per count, the two `n`/`w` fields and the p95 tail, with room
        // to spare rather than a byte-exact fit.
        let mut line = String::with_capacity(640);
        line.push_str("frust-perf enc n=");
        line.push_str(&self.frames.to_string());
        line.push_str(" w=");
        line.push_str(&self.window.len().to_string());
        for (column, name) in ENCODE_TRACE_COLUMNS.iter().enumerate() {
            let p50 = self.percentile(column, 50);
            line.push(' ');
            line.push_str(name);
            line.push_str("_us=");
            line.push_str(&format_us(p50));
        }
        let tail = self.percentile(ENCODE_TRACE_COLUMNS.len() - 1, 95);
        line.push_str(" total_p95_us=");
        line.push_str(&format_us(tail));
        for (offset, name) in ENCODE_TRACE_COUNTS.iter().enumerate() {
            let column = ENCODE_TRACE_COLUMNS.len() + offset;
            let value = if (ENCODE_TRACE_FIRST_SUMMED..ENCODE_TRACE_MEDIAN_AGAIN).contains(&offset)
            {
                self.window
                    .iter()
                    .filter_map(|row| row.get(column))
                    .fold(0_u64, |acc, v| acc.saturating_add(u64::from(*v)))
            } else {
                u64::from(self.percentile(column, 50))
            };
            line.push(' ');
            line.push_str(name);
            line.push('=');
            line.push_str(&value.to_string());
        }
        line
    }

    /// The `pct`-th percentile of `column` over the window, in nanoseconds.
    ///
    /// Nearest-rank on the sorted column, which needs no interpolation and so
    /// reports a value the window really contains — the same choice
    /// `frust-shell-common`'s frame summary makes, and the one that keeps a
    /// sixty-sample window honest.
    fn percentile(&mut self, column: usize, pct: usize) -> u32 {
        self.ranked.clear();
        self.ranked
            .extend(self.window.iter().filter_map(|row| row.get(column)));
        self.ranked.sort_unstable();
        let len = self.ranked.len();
        if len == 0 {
            return 0;
        }
        let rank = (len * pct).div_ceil(100).clamp(1, len);
        self.ranked.get(rank - 1).copied().unwrap_or(0)
    }
}

/// Nanoseconds as microseconds with one decimal — the unit every
/// `frust-perf enc` field is reported in.
///
/// Fixed-point rather than a float format: a phase can be tens of nanoseconds
/// (an empty punch pass) or milliseconds (a ten-thousand-row table's walk), and
/// one decimal microsecond reads the same either way without a float's
/// locale-dependent formatting reaching a capture the harness greps.
#[cfg(feature = "perf-trace")]
fn format_us(nanos: u32) -> String {
    let tenths = u64::from(nanos).div_ceil(100);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// One surface's 2D render engine: a scene in, recorded passes out.
///
/// Create one per surface and keep it across frames — the retained scene
/// compiler, gradient cache, pipeline cache, resource textures and intermediate
/// pool are the reason a steady-state frame allocates nothing.
#[derive(Debug)]
pub struct EngineRenderer {
    caps: TierCaps,
    format: wgpu::TextureFormat,
    shaders: EngineShaders,
    pipelines: PipelineCache,
    compiler: SceneCompiler,
    gradients: GradientCache,
    depth: DepthAttachment,
    targets: IntermediateTargets,
    /// The bounds an intermediate page is sized between — the policy half of
    /// page sizing, kept beside the pool the extents are requested from.
    pages: PageConfig,
    /// The caller-owned textures a `Command::SceneTexture` resolves against
    /// (see [`crate::gpu::bindings`]). Its extent half lives on the compiler,
    /// written by the same two calls that write this.
    textures: ExternalTextures,
    resources: FrameResources,
    scratch: Scratch,
    /// How each pass's instances are split into paint-class runs — the kill
    /// switch and the run threshold, settled at construction from
    /// [`crate::config`] and overridable per renderer.
    routing: ClassRouting,
    /// Whether the surface opening is one pass — settled at construction from
    /// `FRUST_ENGINE_NO_PASS_MERGE` and overridable per renderer (see
    /// [`Self::set_pass_merge`]).
    pass_merge: bool,
    /// The passes the last encoded frame recorded into the caller's encoder,
    /// in order; refilled every frame, so its allocation is reused.
    frame_passes: Vec<FramePass>,
    /// The strip pipelines resolved so far, by state and class, so a frame
    /// takes each one it draws with by a two-enum lookup rather than by
    /// rebuilding and re-keying its description through the cache.
    resolved: ResolvedPipelines,
    /// The render-to-atlas pass, created by the first frame that caches a
    /// glyph.
    ///
    /// Lazy because it owns a coverage texture, an instance buffer and five
    /// stand-in bindings of its own, and a renderer that never draws text —
    /// or one running with `FRUST_ENGINE_NO_ATLAS` — should pay for none of
    /// them.
    atlas_glyphs: Option<AtlasRenderer>,
    /// The compiler the atlas replay lowers a page's recorded commands
    /// through, sized to the atlas page rather than to the surface.
    ///
    /// A second compiler rather than this renderer's own: the frame's compiler
    /// is mid-frame (its glyph entry map is exactly what the replay is
    /// draining) and its viewport is the surface's, while a page's commands are
    /// in page space. Created on the first replay and kept, so a steady stream
    /// of first-seen glyphs allocates a strip generator once.
    atlas_lowering: Option<SceneCompiler>,
    /// What the last frame's replay serviced, for
    /// [`Self::atlas_render_report`].
    atlas_report: AtlasRenderReport,
    /// The filter-data texture, sampler and instance buffer a filter round is
    /// executed with.
    ///
    /// Lazy for the same reason [`Self::atlas_glyphs`] is: a renderer that
    /// never blurs should own neither a sampler nor a resource texture it will
    /// not read. Created by the first frame that schedules a filter round.
    filters: Option<FilterResources>,
    /// The instance buffer a frame's fragment region copies are drawn from
    /// ([`RegionCopyMode::Fragment`]). Lazy for [`Self::filters`]' reason: a
    /// renderer whose frames never copy a region owns none.
    region_copies: Option<RegionCopyResources>,
    /// The retained pages `PushRetained` bodies render into and
    /// `DrawRetained`s composite from, kept across frames (see
    /// [`crate::cache::retained`]).
    retained: RetainedPages,
    /// The roles the store gave this frame's retained layers, handed to the
    /// scheduler — retained across frames for its allocation alone.
    retained_layers: RetainedLayers,
    /// The scene [`Self::record_range`] projects a range into, kept for its
    /// allocation alone; empty until the first range is recorded.
    range_scene: Scene,
    /// The rolling CPU-phase window `frust-perf enc` lines are emitted from.
    ///
    /// Absent entirely from a build without `perf-trace`, along with the type
    /// itself — see [`EncodeTrace`].
    #[cfg(feature = "perf-trace")]
    encode_trace: EncodeTrace,
}

impl EngineRenderer {
    /// Builds a renderer for `format` targets on `caps`' adapter, warming
    /// every engine pipeline in the background.
    ///
    /// `pipeline_cache` is the host's persisted driver cache when it has one —
    /// `None` on every backend but Vulkan.
    ///
    /// # Errors
    ///
    /// [`EngineError::AtlasError`] when the adapter's `resource_texture_dim`
    /// is not a power of two of at least [`MIN_RESOURCE_TEXTURE_DIM`]. The
    /// strip shader reconstructs a resource texture's width as `1 << bits` and
    /// an upload's row stride must satisfy wgpu's copy alignment, so neither
    /// the shader's addressing nor the uploads would be valid otherwise.
    pub fn new(
        device: &wgpu::Device,
        caps: &TierCaps,
        format: wgpu::TextureFormat,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Result<Self, EngineError> {
        Self::with_image_sampler(
            device,
            caps,
            format,
            pipeline_cache,
            config::image_sampler_enabled(),
        )
    }

    /// [`Self::new`] with the strip program family stated rather than read
    /// from `FRUST_ENGINE_IMAGE_SAMPLER` — the programmatic counterpart of
    /// that switch, which only sets the family [`Self::new`] picks.
    ///
    /// With `image_sampler` set, every strip program that reads atlas images
    /// filters a bilinear one through a hardware sampler instead of four
    /// `textureLoad` taps (see [`crate::gpu::shader_src`]'s *The two program
    /// families*), and every strip bind group carries the one linear,
    /// clamp-to-edge sampler this renderer creates for it. A family is a
    /// construction-time choice rather than a per-frame setter because it
    /// decides which programs are compiled and warmed: a caller comparing the
    /// two builds one renderer of each.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_image_sampler(
        device: &wgpu::Device,
        caps: &TierCaps,
        format: wgpu::TextureFormat,
        pipeline_cache: Option<&wgpu::PipelineCache>,
        image_sampler: bool,
    ) -> Result<Self, EngineError> {
        let dim = caps.resource_texture_dim;
        if !dim.is_power_of_two() || dim < MIN_RESOURCE_TEXTURE_DIM {
            return Err(EngineError::AtlasError);
        }

        let routing = ClassRouting::from_config();
        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register_family(&mut library, device, image_sampler);
        let mut pipelines = PipelineCache::new(Arc::new(library), pipeline_cache.cloned());
        pipelines.warm_up(device, &warm_up_descs_for(&shaders, format, routing.uber));

        let level = Level::try_detect().unwrap_or(Level::baseline());
        let gradients = GradientCache::for_texture(GradientTextureLayout::square(dim), level);

        Ok(Self {
            caps: caps.clone(),
            format,
            shaders,
            pipelines,
            // The viewport is re-asserted on every compile, so the extent is
            // only an initial allocation hint. `caps` is not: it is what fixes
            // the image atlas budget for this renderer's whole life (mobile or
            // desktop tier, the adapter's own ceilings, and any
            // `FRUST_ENGINE_ATLAS_SIZE` override), and the adapter is known
            // exactly here — a compiler built without it would silently keep
            // the mobile budget on every device.
            compiler: {
                let mut compiler = SceneCompiler::for_caps(1, 1, caps);
                // A renderer has a store behind its compiler, so retained
                // brackets are recorded for it — unless the engine-side kill
                // switch says otherwise.
                compiler.set_retained_layers(!config::layers_disabled());
                compiler
            },
            gradients,
            depth: DepthAttachment::new(),
            targets: IntermediateTargets::new(caps),
            pages: PageConfig::default(),
            textures: ExternalTextures::new(),
            resources: FrameResources::new(device, dim, image_sampler),
            scratch: Scratch::default(),
            routing,
            pass_merge: !config::pass_merge_disabled(),
            frame_passes: Vec::new(),
            resolved: ResolvedPipelines::default(),
            atlas_glyphs: None,
            atlas_lowering: None,
            atlas_report: AtlasRenderReport::default(),
            filters: None,
            region_copies: None,
            retained: RetainedPages::for_caps(caps),
            retained_layers: RetainedLayers::new(),
            range_scene: Scene::new(),
            #[cfg(feature = "perf-trace")]
            encode_trace: EncodeTrace::default(),
        })
    }

    /// The target format this renderer warmed its pipelines for.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Whether this renderer's strip programs filter bilinear atlas images
    /// through the hardware sampler (see [`Self::with_image_sampler`]).
    #[must_use]
    pub fn image_sampler(&self) -> bool {
        self.shaders.image_sampler
    }

    /// The adapter capabilities every sizing decision is made against.
    #[must_use]
    pub fn caps(&self) -> &TierCaps {
        &self.caps
    }

    /// The pool the engine's off-screen intermediates come from.
    #[must_use]
    pub fn targets(&self) -> &IntermediateTargets {
        &self.targets
    }

    /// The bounds this renderer sizes intermediate layer pages between.
    #[must_use]
    pub fn page_config(&self) -> PageConfig {
        self.pages
    }

    /// The atlas geometry image residency allocates within.
    ///
    /// Derived from the adapter in [`Self::new`], so this is the tier's budget
    /// narrowed to what the adapter can create — not a constant.
    #[must_use]
    pub fn atlas_budget(&self) -> AtlasBudget {
        self.compiler.images().budget()
    }

    /// Re-budget image residency, dropping every image currently resident and
    /// the atlas array holding them.
    ///
    /// An atlas rectangle only means anything against the geometry it was
    /// allocated in, so a new budget invalidates every one already handed out —
    /// which is why the array, the registry of where each image lives and the
    /// bind groups naming that array all go in the same step, and each image
    /// re-uploads on the next frame that draws it. A start-up or adapter-change
    /// operation, never a per-frame one.
    pub fn set_atlas_budget(&mut self, budget: AtlasBudget) {
        self.compiler.set_atlas_budget(budget);
        self.resources.reset_atlas();
    }

    /// Replace image residency wholesale, on the same invalidation terms as
    /// [`Self::set_atlas_budget`].
    ///
    /// The programmatic counterpart to `FRUST_ENGINE_NO_ATLAS`: an
    /// [`ImageResidency::disabled`] residency takes both atlas classes out of
    /// the frame — images are skipped and every glyph is drawn as outline
    /// strips — without a process-global environment variable, which is what a
    /// caller comparing the two paths on one device needs.
    pub fn set_image_residency(&mut self, images: crate::cache::images::ImageResidency) {
        self.compiler.set_image_residency(images);
        self.resources.reset_atlas();
    }

    /// How many atlas regions this renderer has declined to write or clear.
    ///
    /// Zero on every sound frame: residency allocates inside the budget the
    /// array is created at, and the array is grown to the depth the frame
    /// reports before its regions are written, so a refusal means those two
    /// went out of agreement. The count exists so that disagreement is
    /// measurable rather than silent — a refused write is a region the frame
    /// believed it had filled.
    #[must_use]
    pub fn refused_atlas_regions(&self) -> u64 {
        self.resources.refused_regions
    }

    /// Finishes pipeline warm-up on the calling thread, returning only once
    /// every engine pipeline exists.
    ///
    /// Warm-up is started in the background by [`Self::new`] and normally
    /// needs no attention. Two callers want it forced: a host that must not
    /// let the *first* frame pay for a compile, and anything about to drop the
    /// `wgpu::Device` shortly after building a renderer — the warm-up worker
    /// holds its own handle on that device, and tearing it down while the
    /// worker is mid-compile is a driver-level hazard rather than a clean
    /// cancellation.
    ///
    /// Each variant is requested through the cache, which builds a queued one
    /// inline and waits for one the worker has already started, so nothing is
    /// compiled twice and nothing is left for the worker to claim afterwards.
    ///
    /// "Every" is the warm-up list: the nine Uber states, plus — unless the
    /// Uber program is forced — the warmed paint classes. A class variant
    /// outside that list is created on the first frame that draws a run of it.
    pub fn finish_warm_up(&mut self, device: &wgpu::Device) {
        for desc in warm_up_descs_for(&self.shaders, self.format, self.routing.uber) {
            let _ = self.pipelines.get_or_create(device, &desc);
        }
    }

    /// Forces every strip run through the whole (Uber) strip program, or
    /// lets passes split into paint-class runs again — the programmatic
    /// counterpart of `FRUST_ENGINE_UBER_PROGRAM`, which only sets the value a
    /// renderer starts with.
    ///
    /// Takes effect from the next frame. With it set a frame draws exactly
    /// what it drew before paint classes existed — one pipeline per pass, the
    /// whole `strip.wgsl` program of this renderer's family — which is what a
    /// caller comparing the two needs without a process-global environment
    /// variable.
    pub fn set_uber_program(&mut self, uber: bool) {
        self.routing.uber = uber;
    }

    /// Whether every strip run is drawn with the Uber program.
    #[must_use]
    pub fn uber_program(&self) -> bool {
        self.routing.uber
    }

    /// Draws a glyph run resolved entirely out of the glyph atlas as glyph
    /// quads, or the general way — the programmatic counterpart of
    /// `FRUST_ENGINE_NO_GLYPH_QUADS`, which only sets the value a renderer
    /// starts with (see [`SceneCompiler::set_glyph_quads`]).
    ///
    /// Takes effect from the next frame. Both arms draw the same pixels, which
    /// is what a caller comparing them in one process checks.
    pub fn set_glyph_quads(&mut self, enabled: bool) {
        self.compiler.set_glyph_quads(enabled);
    }

    /// Whether resident glyph runs are drawn as glyph quads.
    #[must_use]
    pub fn glyph_quads(&self) -> bool {
        self.compiler.glyph_quads_enabled()
    }

    /// Sets the shortest paint-class run worth a pipeline switch of its own
    /// (see [`merge_class_runs`]); shorter runs are merged and drawn with the
    /// Uber program. `FRUST_ENGINE_RUN_SPLIT_MIN` only sets the value a
    /// renderer starts with.
    pub fn set_run_split_min(&mut self, min_run: u32) {
        self.routing.min_run = min_run;
    }

    /// The run threshold this renderer splits passes at.
    #[must_use]
    pub fn run_split_min(&self) -> u32 {
        self.routing.min_run
    }

    /// Records the surface opening as one pass, or as the clear and the
    /// opaque strips apart from the first surface round — the programmatic
    /// counterpart of `FRUST_ENGINE_NO_PASS_MERGE`, which only sets the value a
    /// renderer starts with.
    ///
    /// Takes effect from the next frame. Unmerged, a frame records the clear
    /// and the opaque strips as passes of their own ahead of every round (see
    /// the module doc's *The frame's passes*) and draws the same pixels, which
    /// is what a caller comparing the two arms in one process needs.
    pub fn set_pass_merge(&mut self, merge: bool) {
        self.pass_merge = merge;
    }

    /// Whether the surface opening is recorded as one pass.
    #[must_use]
    pub fn pass_merge(&self) -> bool {
        self.pass_merge
    }

    /// The passes the last [`Self::encode`] recorded into the caller's
    /// encoder, in recording order.
    ///
    /// Empty after a refused frame, which records no pass at all. A
    /// diagnostic: how many passes a frame costs is the number a tile-based
    /// GPU pays a whole-framebuffer load and store for, so it is what a test
    /// pins and a capture is read against.
    #[must_use]
    pub fn last_frame_passes(&self) -> &[FramePass] {
        &self.frame_passes
    }

    /// How many render pipelines this renderer has compiled so far.
    ///
    /// Counts *distinct* pipelines, which is why it is a diagnostic rather
    /// than something to wait on: two entries of [`EnginePipeline::ALL`] that
    /// differ only in their colour format describe the same pipeline whenever
    /// the frame's target format happens to equal
    /// [`crate::gpu::pipelines::INTERMEDIATE_FORMAT`], and the cache compiles
    /// that one variant once. Use [`Self::finish_warm_up`] to wait.
    #[must_use]
    pub fn compiled_pipelines(&self) -> u64 {
        self.pipelines.compiled_variants()
    }

    /// States whether a caller-supplied depth attachment already holds the
    /// depth this frame should test against — see
    /// [`DepthAttachment::set_pre_cleared`].
    pub fn set_depth_pre_cleared(&mut self, pre_cleared: bool) {
        self.depth.set_pre_cleared(pre_cleared);
    }

    /// Whether a caller-supplied depth attachment is treated as already
    /// populated.
    ///
    /// The statement is sticky and set once, so a host driving several
    /// surfaces (or re-establishing one after a device loss) can read back
    /// what this renderer is on rather than tracking it a second time.
    #[must_use]
    pub fn depth_pre_cleared(&self) -> bool {
        self.depth.is_pre_cleared()
    }

    /// Registers a caller-owned texture so a
    /// [`Command::SceneTexture`](frust_scene::Command::SceneTexture) naming
    /// `id` draws it, returning whatever was registered under `id` before.
    ///
    /// `id` is the [`SceneTextureId`] the texture minted for itself
    /// (`frust_gpu::Texture::as_scene_texture`), and `size` is its extent in
    /// texels — the rectangle a display list's destination is mapped onto.
    /// `view` must be a non-array 2D view of a float-sampleable texture
    /// carrying `wgpu::TextureUsages::TEXTURE_BINDING`; `wgpu` rejects
    /// anything else when the frame's bind group is built.
    ///
    /// Both halves of the registration land here: the view a pass samples and
    /// the extent the compiler composes a paint transform against. Registering
    /// is idempotent — re-registering the same id replaces the view and drops
    /// the bind groups naming the old one.
    ///
    /// An extent past `u16::MAX` on either axis, or a zero one, registers
    /// nothing and answers `None`: the record the shader reads packs the
    /// source region into `u16` halves, so there is no honest rectangle to
    /// name. Scenes drawing that id go on drawing nothing.
    pub fn bind_texture(
        &mut self,
        id: SceneTextureId,
        size: (u32, u32),
        view: wgpu::TextureView,
    ) -> Option<wgpu::TextureView> {
        self.resources.forget_external(id.get());
        if !self.compiler.bind_external_texture(id.get(), size) {
            return self.textures.unbind(id);
        }
        self.textures.bind(id, view)
    }

    /// Removes the texture registered under `id`, returning its view.
    pub fn unbind_texture(&mut self, id: SceneTextureId) -> Option<wgpu::TextureView> {
        self.compiler.unbind_external_texture(id.get());
        self.resources.forget_external(id.get());
        self.textures.unbind(id)
    }

    /// The view registered under `id`, if any.
    #[must_use]
    pub fn bound_texture(&self, id: SceneTextureId) -> Option<&wgpu::TextureView> {
        self.textures.get(id)
    }

    /// How many external textures are currently bound.
    #[must_use]
    pub fn bound_texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Releases everything sized against the old surface extent and
    /// re-establishes what the next frame needs at the new one.
    ///
    /// Pipelines, shader modules, the gradient cache and the resource textures
    /// are all extent-independent and deliberately survive: a resize must not
    /// cost a pipeline rebuild or a ramp re-bake. What goes is the intermediate
    /// pool's parked entries — every one keyed on an extent nothing will ask
    /// for again — and the engine-owned depth attachment, which has to match
    /// its colour attachment exactly. Reallocating the depth buffer here rather
    /// than on the next frame keeps it off the frame path.
    ///
    /// Every retained page goes too: a page holds its body as it was laid out
    /// for the old extent, and the resident-key ledger published after the
    /// next frame reports the empty store, so every body is painted (and
    /// re-recorded) at the new one.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets.drop_parked();
        self.depth.resize(device, width, height);
        self.retained.clear();
    }

    /// The retained pages this renderer holds resident after its last frame —
    /// the renderer's half of the resident-key ledger (see
    /// [`crate::cache::retained`]), in no particular order.
    pub fn retained_ledger(&self) -> impl Iterator<Item = RetainedEntry> + '_ {
        self.retained.entries()
    }

    /// The retained store's frame clock — how many frames it has planned —
    /// for a ledger's diagnostic frame stamp.
    #[must_use]
    pub fn retained_frame(&self) -> u64 {
        self.retained.frame()
    }

    /// A counter advanced whenever what [`Self::retained_ledger`] reports
    /// changes; a host publishing the ledger only when it moved publishes
    /// every change and nothing else.
    #[must_use]
    pub fn retained_generation(&self) -> u64 {
        self.retained.generation()
    }

    /// Whether a frame was refused with [`EngineError::RetainedKeyMissing`]
    /// (and the store cleared) since the last call, clearing the flag. A host
    /// that sees `true` owes the widget tree one more paint against the
    /// now-empty ledger: the refused frame presented nothing.
    pub fn take_retained_refusal(&mut self) -> bool {
        self.retained.take_refusal()
    }

    /// Drop every retained page — what a surface recreate or a device-scale
    /// change calls for. The next ledger read reports the empty store.
    pub fn clear_retained(&mut self) {
        self.retained.clear();
    }

    /// The byte budget the retained store is held to.
    #[must_use]
    pub fn retained_budget(&self) -> RetainedBudget {
        self.retained.budget()
    }

    /// Re-budget the retained store, evicting least recently used pages until
    /// what it holds fits.
    pub fn set_retained_budget(&mut self, budget: RetainedBudget) {
        self.retained.set_budget(budget);
    }

    /// Set whether this renderer records retained brackets at all — the
    /// programmatic counterpart of `FRUST_ENGINE_NO_LAYERS` (see
    /// [`SceneCompiler::set_retained_layers`]). Turning it off clears the
    /// store, since nothing would ever draw from it again.
    pub fn set_retained_layers(&mut self, enabled: bool) {
        self.compiler.set_retained_layers(enabled);
        if !enabled {
            self.retained.clear();
        }
    }

    /// Whether this renderer records retained brackets.
    #[must_use]
    pub fn retained_layers(&self) -> bool {
        self.compiler.retained_layers_enabled()
    }

    /// Takes the time this renderer's pipeline cache has spent resolving
    /// variants it did not already hold (waiting on the warm-up worker's
    /// in-flight build, or building one inline) since the last call.
    ///
    /// `perf-trace`-only: it feeds the first frame's
    /// `first_submit_pipeline_wait` startup span.
    #[cfg(feature = "perf-trace")]
    pub fn take_pipeline_wait(&mut self) -> Duration {
        self.pipelines.take_resolve_wait()
    }

    /// How many pipelines the warm-up worker has finished building.
    #[must_use]
    pub fn warm_up_finished(&self) -> u64 {
        self.pipelines.warm_up_finished()
    }

    /// Closes the frame out: ages the intermediate pool by one frame and
    /// evicts the gradient cache down to its capacity.
    ///
    /// Call once per frame, after the frame's commands have been submitted and
    /// including frames that drew nothing — those are the frames a parked
    /// intermediate ages on. Gradient eviction compacts the packed LUT buffer
    /// and rewrites the offsets of the survivors, which is why it belongs at
    /// the frame boundary rather than mid-frame, where it would invalidate
    /// offsets the frame's own encoded paints already carry.
    ///
    /// `_queue` is part of the signature because the end-of-frame maintenance
    /// this method owns grows queue writes as the engine does (an atlas region
    /// cleared after the frame that consumed it, in the reference renderer);
    /// it has none of them yet.
    pub fn end_frame(&mut self, _queue: &wgpu::Queue) {
        self.targets.end_frame();
        self.gradients.maintain();
    }

    /// Compiles `scene` and records the frame's passes into `encoder`.
    ///
    /// `root` is applied ahead of every command's own transform and
    /// `base_color` is what the target is cleared to before anything is drawn.
    /// Neither the encoder nor the queue is submitted — see the module header.
    ///
    /// # Errors
    ///
    /// [`EngineError::TargetTooLarge`] for a target outside the `u16` device
    /// grid the strip pipeline addresses, [`EngineError::InvalidTransform`] for
    /// a non-finite transform, [`EngineError::InvalidGeometry`] for non-finite
    /// command geometry (rect extents, radii, path points, stroke or dash
    /// values), [`EngineError::SchedulerEscalation`] for a layer shape the
    /// engine's scheduler does not serve,
    /// [`EngineError::IntermediateTextureTooLarge`] for a layer no page can be
    /// sized to, [`EngineError::AlphaCapacity`] when a frame's coverage
    /// outgrows the alpha texture, and [`EngineError::PaintCapacity`] when its
    /// encoded paints or colour ramps outgrow theirs. Every one of them is
    /// returned before anything is recorded, uploaded, allocated or submitted,
    /// so a refused frame leaves `encoder` exactly as it was found and the
    /// renderer's own resources — the atlas array included — exactly as they
    /// were. That is what lets the caller skip the frame cleanly (nothing is
    /// presented and the previously presented content persists) rather than
    /// present it half-drawn, and what keeps a refused frame's image uploads
    /// alive for the next frame that is not refused.
    #[expect(
        clippy::too_many_arguments,
        reason = "the seam frust-render drives: device, queue, encoder, scene, \
                  target, base colour and root transform are each supplied by a \
                  different owner, so bundling them would only move the \
                  assembly to every call site"
    )]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        target: EngineTarget<'_>,
        base_color: Color,
        root: Affine,
    ) -> Result<(), EngineError> {
        self.encode_traced(
            device,
            queue,
            encoder,
            scene,
            target,
            base_color,
            root,
            FrameTimestamps::inert(),
        )
    }

    /// [`Self::encode`], with each pass's GPU time stamped into `timestamps`.
    ///
    /// The one difference is the sink: every pass this records asks
    /// `timestamps` for its own `timestamp_writes` and takes `None` for an
    /// answer, so a frame encoded with [`FrameTimestamps::inert`] — which is
    /// exactly what [`Self::encode`] passes — records byte-identical work.
    /// Which pass is charged to which span is [`EngineSpan`]'s own
    /// documentation; the host owns the ring behind the sink and reads the
    /// frame's spans back out of it some frames later (see
    /// [`frust_gpu::diag::TimestampRing`]).
    ///
    /// # Errors
    ///
    /// Exactly [`Self::encode`]'s, on exactly its terms — a refused frame has
    /// recorded no pass, so it has taken no timestamp either and the host
    /// abandons the ring's slot rather than mapping it.
    #[expect(
        clippy::too_many_arguments,
        reason = "[`Self::encode`]'s argument list plus the timestamp sink, \
                  each still supplied by a different owner"
    )]
    pub fn encode_traced(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        target: EngineTarget<'_>,
        base_color: Color,
        root: Affine,
        timestamps: FrameTimestamps<'_>,
    ) -> Result<(), EngineError> {
        // The CPU half of the frame's accounting, lapped phase by phase
        // through to the end of the call (see [`EncodeSpans`]). Free without
        // `perf-trace` — no clock is read at all.
        let mut clock = PhaseClock::start();
        // Emptied first, so a frame refused below reports the nothing it
        // recorded rather than the last frame's passes.
        self.frame_passes.clear();
        let size = grid_size(target.width, target.height)?;
        let mut frame = self.compiler.compile(scene, root, size)?;
        let compile_total = clock.lap();

        // The frame's pass plan, settled before anything is allocated or
        // recorded: a layer shape this scheduler does not serve, or one larger
        // than a page can be sized to, refuses the whole frame here so the
        // caller can skip it cleanly rather than present it half-drawn.
        //
        // The retained store decides first, because its answers are roles the
        // scheduler serves: which bodies render into a kept page, where each
        // `DrawRetained` composites — or that one cannot be served, which
        // refuses the frame here and empties the store (see
        // `crate::cache::retained`). Nothing it decides is applied until the
        // frame is past its last fallible step.
        let retained_plan = self
            .retained
            .plan(&frame.retained, &mut frame.recorder.layers)?;
        retained_roles(&frame, &retained_plan, &mut self.retained_layers);
        // Whether the frame's backdrop blurs can be served at all: each one
        // reads its region of this very target back (see `RegionCopyMode`).
        // A target that can be neither copied nor sampled serves none — every
        // backdrop's rectangle is then the transparent draw the compiler
        // recorded, drawing nothing, and is scheduled as the ordinary layer it
        // is — rather than refusing a frame it can otherwise draw whole.
        let copy_mode = RegionCopyMode::of(target.view.texture());
        let serve_backdrops = copy_mode != RegionCopyMode::Unavailable;
        if !frame.backdrops.is_empty() && !serve_backdrops {
            note_backdrop_unreadable(target.view.texture());
        }
        let backdrops: &[Backdrop] = if serve_backdrops {
            &frame.backdrops
        } else {
            &[]
        };
        // The largest region a texture-mode frame transfers, which the
        // staging texture is grown to before the first pass is recorded.
        let staging_extent = backdrops.iter().fold((0_u32, 0_u32), |(w, h), backdrop| {
            (
                w.max(u32::from(backdrop.region.width())),
                h.max(u32::from(backdrop.region.height())),
            )
        });
        let rounds = Schedule::build_frame(
            &frame.recorder,
            &self.caps,
            &self.pages,
            &self.retained_layers,
            backdrops,
        )?;
        let ceiling = self.targets.max_texture_size();
        for round in &rounds {
            if let Some(page) = round.page()
                && (page.size.width > ceiling || page.size.height > ceiling)
            {
                return Err(EngineError::IntermediateTextureTooLarge);
            }
        }

        // Depth is available only when there is an attachment to use and the
        // kill switch is off. Settling that before a single instance is built
        // is what keeps the opaque/alpha split and the pass shape in agreement.
        let depth_enabled = !config::depth_disabled();
        if depth_enabled && target.depth.is_none() {
            self.depth.ensure(device, target.width, target.height);
        }
        // Cloned out of the renderer rather than borrowed from it: an engine-
        // owned attachment lives in `self.depth`, and recording the frame needs
        // `self` mutably (the page pool, the pipeline cache). A `wgpu`
        // texture view is a reference-counted handle, so the clone is a
        // refcount bump once per frame rather than an allocation.
        let depth_view = depth_enabled
            .then(|| target.depth.or_else(|| self.depth.owned_view()))
            .flatten()
            .cloned();
        let depth_view = depth_view.as_ref();

        // Destination-out erases colour as well as alpha, so a target whose
        // alpha is disregarded would take a black rectangle where the display
        // list says nothing changes. A frame cleared to an opaque base colour
        // is exactly that target — every pixel of it presents opaquely — and is
        // the only such statement `encode` is handed, so it is what the skip
        // `compile::clear`'s contract calls for is decided on.
        let punches = !frame.clears.is_empty() && !is_opaque(base_color);
        let schedule_span = clock.lap();

        // Paints are resolved before instances are built: an instance names
        // its paint by the texel its record starts at, which only exists once
        // the frame's ramps are resident and its records are laid out.
        let atlas_budget = self.compiler.images().budget();
        self.resources
            .resolve_paints(&frame, &mut self.gradients, atlas_budget, &self.textures);
        let paints_span = clock.lap();
        self.scratch.routing = self.routing;
        self.scratch.build(
            &frame,
            &rounds,
            depth_view.is_some(),
            punches,
            &self.resources.paint_slots,
            if serve_backdrops {
                &frame.backdrops
            } else {
                &[]
            },
        );
        let instances_span = clock.lap();

        // Everything that can fail does so here, ahead of the first
        // `begin_render_pass` AND ahead of the first thing this frame changes
        // about the renderer's frame-visible state: apart from the engine's
        // own depth attachment (re-sized above, an internal resource no pass
        // has read yet), a frame refused below has allocated nothing, replaced
        // no texture and submitted nothing, which is what lets the caller
        // skip it cleanly.
        let dim = self.caps.resource_texture_dim;
        let alphas_grown = gpu::grow_alpha_texture_height(
            self.resources.alphas.height,
            frame.alphas().len(),
            dim,
        )?;
        let paints_grown = gpu::paint_texture::grow_encoded_paints_texture_height(
            self.resources.paints.height,
            self.resources.paint_texels(),
            dim,
        )?;
        let gradients_grown = self.resources.grown_gradient_height(&self.gradients)?;

        // Past the last fallible step. The retained store applies its plan
        // first — evictions, then a page for every body a round really renders
        // into — so every retained page the frame's rounds name exists before
        // they are recorded. The evictions include every key the frame
        // re-recorded whose page will not hold that recording (refused,
        // nested or unrendered), so the ledger published after this frame
        // never names a stale page.
        let rendered = retained_rendered(&frame, &rounds);
        self.retained.commit(
            device,
            &frame.retained,
            &frame.retained_superseded,
            &retained_plan,
            &rendered,
        );

        // The atlas is created or grown first, so
        // the growth copy's own submit precedes the frame's atlas writes below
        // (see `gpu::atlas`) and the array is deep enough for every region they
        // name.
        self.resources
            .ensure_atlas(device, queue, atlas_budget, frame.atlas_layers);

        // The glyph atlas, in the order [`crate::gpu::atlas`] documents: the
        // rectangles last frame's eviction freed are zeroed first, ahead of
        // every write this frame issues, so a rectangle handed straight back
        // out cannot be erased after its new occupant landed in it.
        //
        // Acknowledged only once the writes were really issued — the same
        // re-offer contract the image plan keeps below. A frame refused before
        // this point leaves every rectangle pending, so the next frame that
        // gets here still zeroes it.
        if self.clear_glyph_rects(device, queue, &frame) {
            self.compiler.acknowledge_glyph_clears();
        }

        self.resources.resize_alphas(device, alphas_grown);
        self.resources.resize_paints(device, paints_grown);
        self.resources.resize_gradients(device, gradients_grown);
        let resize_span = clock.lap();
        let atlas_serviced =
            self.resources
                .upload(queue, &mut frame, &mut self.gradients, size, dim);
        self.resources
            .upload_instances(device, queue, &self.scratch);

        // Residency is committed exactly here: the frame passed every fallible
        // step and its evictions and uploads have reached the array, so the
        // compiler may stop re-offering them. A frame that returned early above
        // never gets here, and its plan is re-offered on the next frame that
        // does (see `cache::images`).
        if atlas_serviced {
            self.compiler.acknowledge_image_plan();
        }
        let upload_span = clock.lap();

        // The glyph pixels themselves, last of the atlas work and strictly
        // before the scene pass: every page `glifo` dirtied this frame is
        // lowered to strips and drawn into its own array layer, on an encoder
        // this call owns and submits (the sanctioned exception to the encode
        // contract — see this module's header and `gpu::atlas`). The queue
        // writes issued above are flushed ahead of that submit, so the pass
        // composites onto a layer whose clears and image uploads have landed.
        //
        // Driven by the atlas's own pending work, never by this frame's
        // surviving draws: `glifo` dirties a page when it *inserts* an entry,
        // so a run culled away behind a clip records fills while drawing
        // nothing, and a draw-gated replay would leave those commands recorded
        // until some later frame happened to run one — by which time eviction
        // may have re-let the rectangles they name.
        //
        // Acknowledged only when the pass really ran, the same way the clears
        // above are: acknowledging is what lifts the eviction deferral, so an
        // acknowledgement for a replay that returned early would let `glifo`
        // free and re-let the very rectangles those commands still name.
        // The report describes only a frame whose replay ran: a frame that
        // skipped it must not carry the last replay's numbers forward.
        self.atlas_report = AtlasRenderReport::default();
        #[cfg(feature = "perf-trace")]
        let mut replayed = false;
        if self.compiler.glyph_replay_pending()
            && self.replay_glyph_pages(device, queue, timestamps)
        {
            self.compiler.acknowledge_glyph_replay();
            #[cfg(feature = "perf-trace")]
            {
                replayed = true;
            }
        }

        let replay_span = clock.lap();

        let format = target.format;
        let pipelines = self.frame_pipelines(device, format, depth_view.is_some(), copy_mode);
        if pipelines.copy.is_some() {
            let copies = self
                .region_copies
                .get_or_insert_with(|| RegionCopyResources::new(device));
            if copy_mode == RegionCopyMode::Texture {
                copies.ensure_staging(device, staging_extent.0, staging_extent.1);
            }
        }

        // The filter rounds' own resources, created by the first frame that
        // schedules one. Only the parameter blocks are uploaded here: a pass's
        // instance names the extent the *pool* quantized its destination page
        // up to, which only the round that acquires it knows, so the instances
        // are written round by round in `record_frame`.
        if let Some(pipeline) = pipelines.filter.as_ref() {
            let scratch = &self.scratch;
            self.filters
                .get_or_insert_with(|| FilterResources::new(device))
                .prepare(
                    device,
                    queue,
                    pipeline,
                    &scratch.filter_blocks,
                    scratch.filter_passes,
                );
        }

        let pipelines_span = clock.lap();

        self.record_frame(
            device, queue, encoder, &target, depth_view, base_color, &pipelines, copy_mode,
            timestamps,
        );

        // The laps above still run without `perf-trace` — `PhaseClock` reads
        // no clock there, so each one is `Duration::ZERO` — but nothing records
        // them, and the trace they would have fed is not compiled at all.
        #[cfg(not(feature = "perf-trace"))]
        let _ = (
            compile_total,
            schedule_span,
            paints_span,
            instances_span,
            resize_span,
            upload_span,
            replay_span,
            pipelines_span,
            clock.lap(),
        );

        // Last, so the window's own formatting is charged to no phase it
        // reports. A frame refused above records nothing: its phases are a
        // partial encode and would drag every percentile toward a frame that
        // was never presented.
        #[cfg(feature = "perf-trace")]
        self.encode_trace.record(&EncodeSpans {
            compile: frame.compile_spans,
            compile_total,
            schedule: schedule_span,
            paints: paints_span,
            instances: instances_span,
            resize: resize_span,
            upload: upload_span,
            replay: replay_span,
            pipelines: pipelines_span,
            record: clock.lap(),
            counts: EncodeCounts::of(
                &frame,
                replayed,
                &self.atlas_report,
                self.compiler.glyph_atlas_trace(),
            ),
        });

        Ok(())
    }

    /// [`Self::encode_traced`] over the commands of `scene` in `range` alone,
    /// cut out by [`project_command_range`] — the overlay spike's way of
    /// drawing one segment of a frame into a target of its own (render-model
    /// Phase 4, decision D10).
    ///
    /// The range is projected into a scene this renderer keeps for the
    /// purpose (its allocation reused from call to call), carrying
    /// `DrawRetained`s only when this renderer retains
    /// ([`Self::retained_layers`]) — a renderer with retention off refuses a
    /// frame carrying one. The bracket rule is [`project_command_range`]'s.
    /// `target`, `base_color` (an overlay passes transparent), `root` and
    /// `timestamps` mean exactly what they mean to [`Self::encode_traced`].
    ///
    /// A range covering the whole scene is still projected (and re-recorded),
    /// so a caller that draws the whole frame calls [`Self::encode_traced`]
    /// instead; this method is never on a path that does.
    ///
    /// # Errors
    ///
    /// Exactly [`Self::encode_traced`]'s, over the projected scene.
    #[expect(
        clippy::too_many_arguments,
        reason = "[`Self::encode_traced`]'s argument list plus the range, each \
                  still supplied by a different owner"
    )]
    pub fn record_range(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        range: Range<usize>,
        target: EngineTarget<'_>,
        base_color: Color,
        root: Affine,
        timestamps: FrameTimestamps<'_>,
    ) -> Result<RangeCut, EngineError> {
        // Taken out for the call (and always put back): `encode_traced`
        // borrows the renderer mutably while it reads the scene.
        let mut projected = std::mem::take(&mut self.range_scene);
        let options = RangeOptions {
            keep_draw_retained: self.retained_layers(),
        };
        let cut = project_command_range(scene, range, options, &mut projected);
        let result = self.encode_traced(
            device, queue, encoder, &projected, target, base_color, root, timestamps,
        );
        self.range_scene = projected;
        result.map(|()| cut)
    }

    /// Creates the render-to-atlas pass on first use.
    fn ensure_atlas_renderer(&mut self, device: &wgpu::Device) {
        if self.atlas_glyphs.is_none() {
            self.atlas_glyphs = Some(AtlasRenderer::new(device, &self.caps));
        }
    }

    /// Zero every atlas rectangle an earlier frame's glyph eviction freed,
    /// answering whether they were serviced.
    ///
    /// Queue writes, issued before this frame's image uploads and before the
    /// replay pass's submit — the first of the three orderings
    /// [`crate::gpu::atlas`] states. A rectangle the array will not take is
    /// counted rather than dropped silently, on the same terms an image region
    /// it refuses is.
    ///
    /// `true` means every rectangle was *offered* to the array — including one
    /// it refused, which no later frame could place either — so the caller may
    /// stop re-offering them. `false` means there was no array to write to at
    /// all, which is the one case where trying again later can succeed. An
    /// empty list is serviced trivially.
    fn clear_glyph_rects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &CompiledFrame,
    ) -> bool {
        if frame.glyph_clears.is_empty() {
            return true;
        }
        if self.resources.atlas.is_none() {
            return false;
        }
        self.ensure_atlas_renderer(device);

        let refused = {
            let Self {
                atlas_glyphs,
                resources,
                ..
            } = self;
            let (Some(glyphs), Some(atlas)) = (atlas_glyphs.as_ref(), resources.atlas.as_ref())
            else {
                return false;
            };
            frame
                .glyph_clears
                .iter()
                .filter(|rect| !glyphs.clear_rect(queue, atlas, **rect))
                .count()
        };

        if refused > 0 {
            self.resources.note_refused_regions(refused as u64);
        }
        true
    }

    /// Draw every atlas page `glifo` dirtied this frame into its own array
    /// layer.
    ///
    /// The pixels of a newly cached glyph, and the last atlas work before the
    /// scene pass. Each page's recorded commands are lowered to strips by
    /// [`lower_atlas_page`] and drawn through the pipeline
    /// [`atlas_strip_desc`] describes; a page the lowering declines is left
    /// undrawn and counted, so a glyph whose shape this tier cannot express
    /// goes *missing* rather than landing half-painted.
    ///
    /// Answers whether the pass really ran, on the same terms
    /// [`clear_glyph_rects`](Self::clear_glyph_rects) does and for the same
    /// reason: `false` means there was no atlas array to draw into at all, so
    /// the recorded commands are still recorded and the caller must go on
    /// offering them. A page the lowering *declined* is not a `false` — it was
    /// offered to the array and counted refused, and no later frame could lower
    /// it either.
    ///
    /// `timestamps` is passed straight through to
    /// [`gpu::atlas::AtlasRenderer::render_pending`], which charges each dirty
    /// page's own pass to [`EngineSpan::Prepass`] — this is the frame's own
    /// Prepass recording site.
    #[must_use]
    fn replay_glyph_pages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        timestamps: FrameTimestamps<'_>,
    ) -> bool {
        if self.resources.atlas.is_none() {
            return false;
        }
        self.ensure_atlas_renderer(device);
        let pipeline = self
            .pipelines
            .get_or_create(device, &atlas_strip_desc(&self.shaders))
            .clone();

        let report = {
            let Self {
                atlas_glyphs,
                atlas_lowering,
                resources,
                compiler,
                ..
            } = self;
            let (Some(glyphs), Some(atlas)) = (atlas_glyphs.as_mut(), resources.atlas.as_ref())
            else {
                return false;
            };

            let (width, height) = atlas.size();
            let page = (
                u16::try_from(width).unwrap_or(u16::MAX),
                u16::try_from(height).unwrap_or(u16::MAX),
            );
            let lowering = atlas_lowering.get_or_insert_with(|| SceneCompiler::new(page.0, page.1));

            glyphs.render_pending(
                device,
                queue,
                &pipeline,
                atlas,
                compiler.glyph_atlas_mut(),
                timestamps,
                |recorder, buffers| lower_atlas_page(recorder, buffers, lowering, page),
            )
        };

        if report.refused > 0 {
            self.resources
                .note_refused_regions(u64::from(report.refused));
        }
        self.atlas_report = report;
        true
    }

    /// What the last frame's render-to-atlas pass serviced.
    ///
    /// Zero across the board on a steady-state frame: text that hit the cache
    /// on every glyph frees no rectangle, queues no pixmap and dirties no page.
    #[must_use]
    pub fn atlas_render_report(&self) -> AtlasRenderReport {
        self.atlas_report
    }

    /// Builds (or takes from the cache) every pipeline this frame's passes
    /// need, and the bind groups each of them will be bound through.
    ///
    /// All of it happens before the first `begin_render_pass`: a pipeline
    /// compiled mid-recording would be the very stall the warm-up exists to
    /// avoid. Each pass resolves exactly the paint classes its runs name
    /// ([`Scratch`]'s class sets), folded to what its state serves; the bind
    /// groups are built once per state, against any one of its class
    /// pipelines — every strip pipeline shares one explicit layout, so a group
    /// built against one binds under all of them.
    fn frame_pipelines(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: bool,
        copy_mode: RegionCopyMode,
    ) -> FramePipelines {
        let alpha_variant = if depth {
            EnginePipeline::StripDepthAlpha
        } else {
            EnginePipeline::StripAlpha
        };
        let punch_variant = if depth {
            EnginePipeline::StripDepthDestOut
        } else {
            EnginePipeline::StripDestOut
        };
        let uber = ClassSet::only(PaintClass::Uber);
        if self.resolved.format != Some(format) {
            self.resolved = ResolvedPipelines {
                format: Some(format),
                ..ResolvedPipelines::default()
            };
        }

        // The surface's own pass is unconditional — it is what the frame's
        // bind groups are built against even when it draws nothing — so a frame
        // with no surface run still resolves one pipeline for it: the Uber one,
        // which the warm-up already built.
        let surface_classes = if self.scratch.surface_classes.is_empty() {
            uber
        } else {
            self.scratch.surface_classes
        };
        let mut frame = FramePipelines {
            alpha: (
                alpha_variant,
                self.class_pipelines(device, format, alpha_variant, surface_classes),
            ),
            opaque: None,
            page: None,
            punch: None,
            filter: None,
            copy: None,
        };
        if depth && !self.scratch.opaque.is_empty() {
            frame.opaque = Some(self.class_pipelines(
                device,
                format,
                EnginePipeline::StripOpaque,
                self.scratch.opaque_classes,
            ));
        }
        if self.scratch.page_rounds() > 0 {
            let page_classes = if self.scratch.page_classes.is_empty() {
                uber
            } else {
                self.scratch.page_classes
            };
            frame.page = Some(self.class_pipelines(
                device,
                format,
                EnginePipeline::StripIntermediate,
                page_classes,
            ));
        }
        if self.scratch.punches() {
            frame.punch = Some((
                punch_variant,
                self.class_pipelines(
                    device,
                    format,
                    punch_variant,
                    ClassSet::only(self.routing.punch_class()),
                ),
            ));
        }
        if self.scratch.filter_passes > 0 {
            // Takes the frame's format like every other variant and ignores
            // it: a filter pass only ever writes a pooled page, so its own
            // description is pinned to `INTERMEDIATE_FORMAT`. One filter
            // pipeline therefore serves a renderer for its whole life,
            // whatever its surface is reconfigured to.
            frame.filter = Some(
                self.pipelines
                    .get_or_create(device, &EnginePipeline::Filter.desc(&self.shaders, format))
                    .clone(),
            );
        }

        if self.scratch.copies > 0 && copy_mode != RegionCopyMode::Unavailable {
            // Pinned to `INTERMEDIATE_FORMAT` like the filter pipeline — a copy
            // only ever writes a pooled page — so one serves the renderer for
            // its whole life whatever the frame's own format.
            frame.copy = Some(
                self.pipelines
                    .get_or_create(device, &EnginePipeline::Copy.desc(&self.shaders, format))
                    .clone(),
            );
        }

        if let Some(pipeline) = frame.alpha.1.any() {
            self.resources
                .ensure_bind_groups(device, frame.alpha.0, pipeline, format);
            self.resources
                .ensure_external_groups(device, frame.alpha.0, pipeline, &self.textures);
        }
        if let Some(pipeline) = frame.opaque.as_ref().and_then(ClassPipelines::any) {
            self.resources.ensure_bind_groups(
                device,
                EnginePipeline::StripOpaque,
                pipeline,
                format,
            );
        }
        if let Some(pipeline) = frame.page.as_ref().and_then(ClassPipelines::any) {
            self.resources.ensure_bind_groups(
                device,
                EnginePipeline::StripIntermediate,
                pipeline,
                format,
            );
            // A layer's own round draws through this variant, so an external
            // texture inside an isolated layer needs its group here too.
            self.resources.ensure_external_groups(
                device,
                EnginePipeline::StripIntermediate,
                pipeline,
                &self.textures,
            );
        }
        if let Some((variant, pipelines)) = frame.punch.as_ref()
            && let Some(pipeline) = pipelines.any()
        {
            self.resources
                .ensure_bind_groups(device, *variant, pipeline, format);
        }
        self.resources
            .ensure_page_configs(device, self.scratch.page_rounds());

        frame
    }

    /// The pipelines `state` draws `classes` with — each class folded to what
    /// the state serves — resolved through this renderer's memo first and the
    /// pipeline cache (warm-up, steal, or an inline compile on a class's first
    /// use) only on a miss.
    fn class_pipelines(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        state: EnginePipeline,
        classes: ClassSet,
    ) -> ClassPipelines {
        let mut out = ClassPipelines::default();
        for class in classes.iter() {
            let class = state.fold_class(class);
            if out.get(class).is_some() {
                continue;
            }
            let slot = ResolvedPipelines::slot(state, class);
            let pipeline = match self.resolved.entries.get(slot).and_then(Option::as_ref) {
                Some(pipeline) => pipeline.clone(),
                None => {
                    let pipeline = self
                        .pipelines
                        .get_or_create(device, &state.class_desc(&self.shaders, format, class))
                        .clone();
                    if let Some(entry) = self.resolved.entries.get_mut(slot) {
                        *entry = Some(pipeline.clone());
                    }
                    pipeline
                }
            };
            out.set(class, pipeline);
        }
        out
    }

    /// Records the frame's surface opening, every round's own pass, and the
    /// hole punch — the pass list the module doc's *The frame's passes*
    /// states — logging each pass it begins into [`Self::frame_passes`].
    ///
    /// Every pass opened here is ended before the method returns, which is the
    /// half of the encode contract a caller cannot check for itself.
    ///
    /// Each pass names the [`EngineSpan`] it is charged to: the frame's own
    /// surface passes are [`EngineSpan::Main`], a layer page round and a
    /// filter pass are [`EngineSpan::Composite`]. Several passes per span is
    /// the ordinary case and they sum. A pass takes its query pair only when
    /// it is really begun, so no pair is ever taken for a pass that was not
    /// recorded.
    #[expect(
        clippy::too_many_arguments,
        reason = "one frame's full recording state, each piece owned by a \
                  different part of the renderer; bundling them would move the \
                  same assembly one call up"
    )]
    fn record_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &EngineTarget<'_>,
        depth_view: Option<&wgpu::TextureView>,
        base_color: Color,
        pipelines: &FramePipelines,
        copy_mode: RegionCopyMode,
        timestamps: FrameTimestamps<'_>,
    ) {
        let merge = self.pass_merge;
        let clear = clear_color(base_color, target.output);
        let depth_load = self.depth.load_op(target.depth.is_some());

        // Under the kill switch the clear is a pass of its own, recorded first
        // and drawing nothing, with the opaque strips in a pass of their own
        // after it. It is timed alongside the frame's other surface
        // passes even though a backend that samples its counters at the
        // vertex/fragment stage boundaries may write nothing for it — an
        // untimed pass is a measurement gap, not a wrong measurement (see
        // `frust_gpu::diag`).
        if !merge {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(FramePass::Clear.label()),
                color_attachments: &[Some(color_attachment(
                    target.view,
                    wgpu::LoadOp::Clear(clear),
                ))],
                depth_stencil_attachment: depth_view.map(|view| depth_attachment(view, depth_load)),
                timestamp_writes: timestamps.writes(EngineSpan::Main),
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            self.frame_passes.push(FramePass::Clear);
        }

        // Every field below is reached through `self.<field>` rather than
        // through a method: the page pool is borrowed mutably for the whole
        // walk while the instance buffer, the bind groups and the plan are
        // borrowed immutably, and only disjoint field borrows let those
        // coexist.
        let Some(instances) = self.resources.instances.as_ref() else {
            // No instance has ever been uploaded, so the frame draws nothing
            // at all — and still resolves to a clean surface: the opening with
            // nothing to draw is exactly the clear.
            if merge {
                SurfaceOpening {
                    view: target.view,
                    depth: depth_view,
                    clear,
                    depth_load,
                    opaque: None,
                }
                .record(
                    encoder,
                    None,
                    timestamps.writes(EngineSpan::Main),
                    &mut self.frame_passes,
                );
            }
            #[cfg(debug_assertions)]
            self.check_pass_list(merge, false, false, false);
            return;
        };
        let dim = self.caps.resource_texture_dim;
        let opaque_count = self.scratch.opaque.len() as u32;
        // The alpha region starts where the opaque one ends, so every segment's
        // own index is relative to that.
        let base = opaque_count;

        // The opaque strips, once for the whole frame and ahead of every
        // surface draw: the depth they write is what every blended instance
        // the frame draws onto its own target, composites included, is then
        // tested against.
        //
        // Once rather than ahead of each surface round, because a frame can
        // take several of those (the scheduler cuts one short to hand a page
        // group back) and a second recording would re-draw opaque coverage at
        // equal stored depth over composites the round before it had already
        // blended. Ahead of the layer rounds or after them makes no difference
        // to those: a layer round writes a pooled page and reads neither this
        // target nor the depth attachment.
        let opaque = if opaque_count > 0
            && let Some(opaque) = pipelines.opaque.as_ref()
            && let Some(opaque_groups) =
                self.resources.bind_groups.get(&EnginePipeline::StripOpaque)
        {
            Some(PassPlan {
                pass: FramePass::Opaque,
                view: target.view,
                load: wgpu::LoadOp::Load,
                depth: depth_view,
                state: EnginePipeline::StripOpaque,
                pipelines: opaque,
                groups: opaque_groups,
                resources: &opaque_groups.resources,
                composites: &[],
                // An external paint is never claimed opaque, so the
                // depth-writing pass never holds a run.
                external_keys: &[],
                instances,
                base: 0,
                // The whole opaque region, `(0, opaque_count)`, cut into its
                // own class runs when the frame was planned.
                segments: &self.scratch.opaque_segments,
                composite_class: self.routing.composite_class(),
            })
        } else {
            None
        };
        let opaque_drawn = opaque.is_some();

        // The surface opening: the clear, then the opaque strips, then — when
        // one reaches the surface before anything else does — the first
        // surface round's own strips, all in ONE pass. Owed until the first
        // pass that touches the surface takes it: the first surface round
        // that draws, or a punch issued ahead of any (which takes it as a
        // pass of its own, drawing no round). A page or filter round never
        // takes it — neither reads nor writes the surface or its depth — so
        // the opening simply waits past them. Whatever is still owed at the
        // end of the walk is recorded there, which is how a frame whose
        // surface draws nothing still resolves to a clean surface.
        //
        // Under the kill switch the clear was recorded above and the opaque
        // strips are recorded here, as passes of their own, and nothing is
        // owed.
        let mut opening = if merge {
            Some(SurfaceOpening {
                view: target.view,
                depth: depth_view,
                clear,
                depth_load,
                opaque,
            })
        } else {
            if let Some(opaque) = opaque.as_ref() {
                record_pass(
                    encoder,
                    opaque,
                    timestamps.writes(EngineSpan::Main),
                    &mut self.frame_passes,
                );
            }
            None
        };

        // The destination-out pass's own recording state, resolved once for the
        // frame: a punch can now land at any of the walk's cuts, and every one
        // of them erases the same target through the same pipeline.
        let punch_pass = pipelines.punch.as_ref().and_then(|(variant, punch)| {
            Some(PunchPass {
                view: target.view,
                depth: depth_view,
                state: *variant,
                pipelines: punch,
                class: self.routing.punch_class(),
                groups: self.resources.bind_groups.get(variant)?,
                instances,
                base,
            })
        });
        let punch_drawn = punch_pass.is_some();

        // The frame's live pages — the two ping-pong groups and the one spill
        // page beside them — each holding the finished page a later round
        // composites (see [`crate::schedule`]).
        let mut live: [Option<PooledTexture>; MAX_LIVE_PAGES] = [const { None }; MAX_LIVE_PAGES];
        let mut page_slot = 0_usize;
        // The fragment region copies' own instance slots, one per copy round.
        let mut copy_slot = 0_u32;

        for plan in &self.scratch.rounds {
            let own = match plan.page {
                None => None,
                // A round continuing a page an earlier round of the same layer
                // opened takes that very texture back out of its group: a fresh
                // one from the pool would hold the previous holder's pixels
                // instead of the half already drawn.
                Some(page) if page.continued => match live[page.parity.index()].take() {
                    Some(pooled) => Some((page.parity, pooled)),
                    // Unreachable: the round that opened the page put it in
                    // this group, and no round between the two releases it.
                    None => continue,
                },
                Some(page) => match self.targets.acquire(
                    device,
                    page.size.width,
                    page.size.height,
                    PAGE_LABEL,
                ) {
                    IntermediateTexture::Texture(pooled) => Some((page.parity, pooled)),
                    // Unreachable: every page extent was checked against this
                    // pool's own ceiling before the first pass was recorded.
                    // Skipping the round draws less rather than taking a device
                    // error mid-frame.
                    IntermediateTexture::TooLarge { .. } => continue,
                },
            };

            // A region-copy round seeds its page with a region of the frame's
            // own target, read back after everything painted before it in
            // painter order — so the opening, which clears the target and
            // draws the opaque strips, is recorded first if it is still owed
            // (the scheduler already cut the surface round ahead of it).
            if let Some(source) = plan.copy {
                if let Some((_, pooled)) = own.as_ref() {
                    record_owed_opening(&mut opening, encoder, timestamps, &mut self.frame_passes);
                    RegionCopyPass {
                        mode: copy_mode,
                        target: target.view,
                        page: pooled,
                        source,
                        pipeline: pipelines.copy.as_ref(),
                        resources: self.region_copies.as_ref(),
                        slot: copy_slot,
                    }
                    .record(
                        device,
                        queue,
                        encoder,
                        timestamps.writes(EngineSpan::Composite),
                    );
                    copy_slot = copy_slot.saturating_add(1);
                    self.frame_passes.push(FramePass::Copy);
                }
                settle_pages(&mut self.targets, &mut live, plan.released, own);
                continue;
            }

            // A filter round is only a filter pass: no strip instance, no
            // composite, no viewport uniform of its own — the pass maps NDC
            // against the destination extent its own instance carries, which is
            // why that instance is written here, where the pool's quantized
            // extent is finally known.
            if let Some(filter) = plan.filter {
                if let Some((_, pooled)) = own.as_ref()
                    && let Some(pipeline) = pipelines.filter.as_ref()
                    && let Some(filters) = self.filters.as_ref()
                {
                    let (width, height) = pooled.size();
                    filters.write_instance(
                        queue,
                        filter.instance,
                        &FilterInstanceData::new(
                            &filter.step,
                            filter.data_offset,
                            // Both pages hold the layer at their own origin, so
                            // neither region is offset within its page.
                            (0, 0),
                            (0, 0),
                            SizeU16::from_wh(
                                u16::try_from(width).unwrap_or(u16::MAX),
                                u16::try_from(height).unwrap_or(u16::MAX),
                            ),
                            filter.original,
                        ),
                    );
                    // A group with no live page samples the transparent
                    // placeholder, which filters nothing — the same "draw less,
                    // never wrong" answer an unresolvable paint gets.
                    // Unreachable: the round that wrote this pass's source is
                    // the one before it, and nothing between the two releases
                    // that group.
                    let source = live[filter.source.index()].as_ref().map_or(
                        &self.resources.placeholders.layer_input,
                        PooledTexture::view,
                    );
                    filters.record_pass_timed(
                        device,
                        encoder,
                        &FilterPassPlan {
                            label: FILTER_LABEL,
                            pipeline,
                            dest: pooled.view(),
                            source,
                            instance: filter.instance,
                        },
                        timestamps.writes(EngineSpan::Composite),
                    );
                    self.frame_passes.push(FramePass::Filter);
                }

                settle_pages(&mut self.targets, &mut live, plan.released, own);
                continue;
            }

            // The off-screen texture this round draws into, when it is not the
            // surface: the pooled page acquired above, or the retained page the
            // store bound to this round's body (`crate::cache::retained`). A
            // retained round whose page is missing — unreachable, the store
            // committed one for every body a round renders — draws nothing
            // rather than drawing onto the surface.
            let retained_page = plan
                .retained
                .and_then(|retained| self.retained.page_for(retained.bracket));
            if plan.retained.is_some() && retained_page.is_none() {
                settle_pages(&mut self.targets, &mut live, plan.released, own);
                continue;
            }
            let offscreen = own
                .as_ref()
                .map(|(_, pooled)| (pooled.view(), pooled.size()))
                .or(retained_page);
            let continued = plan.page.is_some_and(|page| page.continued)
                || plan.retained.is_some_and(|retained| retained.continued);

            // The round's viewport uniform. A page's is written here rather
            // than with the frame's other uploads because only the pool knows
            // the extent it quantized the request up to, and NDC is computed
            // against the attachment's real extent. Distinct buffers, so the
            // write ordering against the frame's own config never matters.
            let config = match offscreen {
                None => &self.resources.config,
                Some((_, (width, height))) => {
                    let Some(config) = self.resources.page_configs.get(page_slot) else {
                        continue;
                    };
                    queue.write_buffer(
                        config,
                        0,
                        bytemuck::bytes_of(&GpuConfig::new(width, height, dim, dim)),
                    );
                    page_slot = page_slot.saturating_add(1);
                    config
                }
            };

            let variant = match offscreen {
                None => pipelines.alpha.0,
                Some(_) => EnginePipeline::StripIntermediate,
            };
            let class_pipelines = match (offscreen, pipelines.page.as_ref()) {
                (None, _) => &pipelines.alpha.1,
                (Some(_), Some(page)) => page,
                (Some(_), None) => continue,
            };
            // The pipeline group 0 is built against: any of the pass's class
            // pipelines will do, since every strip pipeline shares one layout.
            let Some(pipeline) = class_pipelines.any() else {
                continue;
            };
            let Some(groups) = self.resources.bind_groups.get(&variant) else {
                continue;
            };

            // A page round binds its own viewport uniform; the root round's is
            // already the one the shared set carries.
            let page_resources = offscreen.map(|_| {
                resources_bind_group(
                    device,
                    pipeline,
                    &self.resources.alphas.view,
                    config,
                    &self.resources.placeholders.layer_input,
                )
            });
            let resources = page_resources.as_ref().unwrap_or(&groups.resources);

            let segments = self
                .scratch
                .segments
                .get(plan.segments.clone())
                .unwrap_or(&[]);
            let mut composites: Vec<wgpu::BindGroup> = Vec::new();
            for segment in segments {
                // A backdrop's draws sample the page holding its blurred
                // region through the same group-0 layer input a composite
                // reads, so its group is built here, in segment order, beside
                // the composites' own.
                if let Segment::Backdrop(_, _, parity) = *segment {
                    let view = live[parity.index()].as_ref().map_or(
                        &self.resources.placeholders.layer_input,
                        PooledTexture::view,
                    );
                    composites.push(resources_bind_group(
                        device,
                        pipeline,
                        &self.resources.alphas.view,
                        config,
                        view,
                    ));
                }
                if let Segment::Composite(_, source) = *segment {
                    // A group with no live page — or a retained bracket with no
                    // page bound — samples the transparent placeholder, which
                    // composites nothing: the same "draw less, never wrong"
                    // answer an unresolvable paint gets.
                    let view = match source {
                        CompositeSource::Page(parity) => live[parity.index()].as_ref().map_or(
                            &self.resources.placeholders.layer_input,
                            PooledTexture::view,
                        ),
                        CompositeSource::Retained(bracket) => self
                            .retained
                            .page_for(bracket)
                            .map_or(&self.resources.placeholders.layer_input, |(view, _)| view),
                    };
                    composites.push(resources_bind_group(
                        device,
                        pipeline,
                        &self.resources.alphas.view,
                        config,
                        view,
                    ));
                }
            }

            // A page round draws into an off-screen layer page, the frame's own
            // rounds into the surface — the split the two spans name.
            let span = if offscreen.is_some() {
                EngineSpan::Composite
            } else {
                EngineSpan::Main
            };

            let (view, pass, load, depth) = match offscreen {
                Some((view, _)) => (
                    view,
                    FramePass::Page,
                    // A pooled texture holds whatever its last holder left
                    // there, and a retained one whatever this body's last frame
                    // left, so a layer's first round into a page clears it —
                    // and a round continuing that same page loads it, because
                    // clearing again would wipe the half already drawn.
                    if continued {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    },
                    None,
                ),
                None => (
                    target.view,
                    FramePass::Surface,
                    wgpu::LoadOp::Load,
                    depth_view,
                ),
            };

            // A surface plan with nothing of its own to draw records no pass:
            // loading and storing the target unchanged is exactly nothing. A
            // punch cut falling at the very start of a round — a `ClearRect`
            // recorded before anything the round draws — is how one arises, and
            // so is a surface whose every span went to the opaque strips; the
            // opening stays owed past it either way. A *page* plan with no
            // segments still records its pass, because that is what clears the
            // pooled page.
            if !segments.is_empty() || offscreen.is_some() {
                let round = PassPlan {
                    pass,
                    view,
                    load,
                    depth,
                    state: variant,
                    pipelines: class_pipelines,
                    groups,
                    resources,
                    composites: &composites,
                    external_keys: self.resources.external_runs.keys(),
                    instances,
                    base,
                    segments,
                    composite_class: self.routing.composite_class(),
                };
                // The first surface round that draws takes the opening: one
                // pass clears, draws the opaque strips, then this round's own
                // segments in order — the pipeline switching between them
                // inside the pass — and ends where this round's pass would
                // have ended.
                let opening = if offscreen.is_none() {
                    opening.take()
                } else {
                    None
                };
                match opening {
                    Some(opening) => opening.record(
                        encoder,
                        Some(&round),
                        timestamps.writes(span),
                        &mut self.frame_passes,
                    ),
                    None => record_pass(
                        encoder,
                        &round,
                        timestamps.writes(span),
                        &mut self.frame_passes,
                    ),
                }
            }

            settle_pages(&mut self.targets, &mut live, plan.released, own);

            // The cut this plan ends at, erased once the ops before it have
            // been recorded and before the plan after it draws a thing. Inert
            // on every plan the walk did not cut, which is every plan of a
            // frame that punches nothing. A punch issued before the opening
            // was taken has to erase a cleared surface, so the opening is
            // recorded first, as a pass of its own.
            if let Some(punch) = punch_pass.as_ref()
                && plan.punch.1 > 0
            {
                record_owed_opening(&mut opening, encoder, timestamps, &mut self.frame_passes);
                punch.record(encoder, plan.punch, timestamps, &mut self.frame_passes);
            }
        }

        // An opening no surface round took — a frame whose surface draws
        // nothing of its own past the opaque strips, or nothing at all — is
        // recorded here, ahead of the trailing punch it must precede.
        record_owed_opening(&mut opening, encoder, timestamps, &mut self.frame_passes);

        // The punches past every op of the frame, in the position the pass held
        // unconditionally before painter order was restored.
        if let Some(punch) = punch_pass.as_ref() {
            punch.record(
                encoder,
                self.scratch.punch,
                timestamps,
                &mut self.frame_passes,
            );
        }

        for page in live.into_iter().flatten() {
            self.targets.release(page);
        }

        #[cfg(debug_assertions)]
        self.check_pass_list(merge, true, opaque_drawn, punch_drawn);
        #[cfg(not(debug_assertions))]
        let _ = (opaque_drawn, punch_drawn);
    }

    /// Checks that the passes the walk just recorded are exactly the ones
    /// [`Scratch::plan_passes`] says a frame of this shape records — the
    /// host-testable statement of the pass list, held to the recording on
    /// every frame a debug build encodes.
    #[cfg(debug_assertions)]
    fn check_pass_list(&self, merge: bool, instances: bool, opaque: bool, punch: bool) {
        let mut planned = Vec::new();
        self.scratch.plan_passes(
            &PassShape {
                merge,
                instances,
                opaque,
                punch,
            },
            &mut planned,
        );
        debug_assert_eq!(
            planned, self.frame_passes,
            "the recorded pass list disagrees with the planned one"
        );
    }
}

/// One render pass a frame recorded into the caller's encoder — what
/// [`EngineRenderer::last_frame_passes`] lists, in recording order.
///
/// The module doc's *The frame's passes* is the contract these name. The
/// glyph-atlas replay's passes are not among them: they are recorded on an
/// encoder of the engine's own, ahead of the frame (see [`crate::gpu::atlas`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FramePass {
    /// The surface opening: clears the colour target (and the depth
    /// attachment, unless pre-cleared), then draws the opaque strips and the
    /// first surface round that draws, whichever of the two the frame has.
    /// The first pass over the surface whenever passes are merged.
    Opening,
    /// The clear alone, drawing nothing — the first pass of a frame recorded
    /// with passes unmerged.
    Clear,
    /// The opaque strips alone, depth-writing, loading what the clear left —
    /// recorded only with passes unmerged.
    Opaque,
    /// A surface round's blended strips, loading the surface.
    Surface,
    /// A layer page round, rendering into a pooled page.
    Page,
    /// One pass of a filter's sequence, between two pooled pages.
    Filter,
    /// A region copy: a region of the frame's own target read back into a
    /// pooled page by one pass that clears the page and draws the region into
    /// it — preceded, on a target that can only be copied, by a transfer into
    /// the renderer's staging texture (see [`RegionCopyMode`]) — ahead of a
    /// backdrop blur's filter passes.
    Copy,
    /// The destination-out hole punch, at a cut or after the last round.
    Punch,
}

impl FramePass {
    /// The label the pass is recorded under, as a GPU capture shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            FramePass::Opening => "frust-engine surface opening",
            FramePass::Clear => "frust-engine clear",
            FramePass::Opaque => "frust-engine opaque strips",
            FramePass::Surface => "frust-engine alpha strips",
            FramePass::Page => PAGE_LABEL,
            FramePass::Filter => FILTER_LABEL,
            FramePass::Copy => "frust-engine region copy",
            FramePass::Punch => "frust-engine hole punch",
        }
    }

    /// Whether the pass renders into the frame's own target rather than into
    /// a pooled page.
    #[must_use]
    pub const fn targets_surface(self) -> bool {
        !matches!(self, FramePass::Page | FramePass::Filter | FramePass::Copy)
    }
}

/// What a frame's pass list depends on beyond its plan, as
/// [`Scratch::plan_passes`] reads it.
#[cfg(any(test, debug_assertions))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PassShape {
    /// Whether the surface opening is merged (the renderer's
    /// [`EngineRenderer::pass_merge`]).
    merge: bool,
    /// Whether the frame has an instance buffer to draw from at all.
    instances: bool,
    /// Whether the frame draws opaque strips — depth in play, opaque spans to
    /// route, and their pipelines resolved.
    opaque: bool,
    /// Whether the frame records its hole punches.
    punch: bool,
}

/// The pipelines one frame's passes are recorded with, resolved once before the
/// first `begin_render_pass`.
///
/// Only `alpha` is unconditional: the rest exist exactly when the frame has
/// work for them, so a plain frame compiles and binds nothing it will not draw.
#[derive(Debug)]
struct FramePipelines {
    /// The blended pass over the frame's own target, with its variant — which
    /// of the two it is depends on whether depth is in play.
    alpha: (EnginePipeline, ClassPipelines),
    /// The depth-writing pass, when depth is available and the frame has
    /// fully covered spans to route into it.
    opaque: Option<ClassPipelines>,
    /// The pass a layer page is rendered through, when the frame has one.
    page: Option<ClassPipelines>,
    /// The destination-out pass, with its variant, when the frame punches.
    punch: Option<(EnginePipeline, ClassPipelines)>,
    /// The pass one filter round runs through, when the frame filters a layer.
    filter: Option<wgpu::RenderPipeline>,
    /// The pass a fragment region copy runs through, when the frame copies a
    /// region of a target it can only sample ([`RegionCopyMode::Fragment`]).
    copy: Option<wgpu::RenderPipeline>,
}

/// How a renderer cuts its passes into paint-class runs.
///
/// Settled from [`crate::config`] when the renderer is built
/// (`FRUST_ENGINE_UBER_PROGRAM`, `FRUST_ENGINE_RUN_SPLIT_MIN`) and overridable
/// per renderer, so a caller can compare the two arms in one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassRouting {
    /// The kill switch: every run is drawn with [`PaintClass::Uber`] and no
    /// segment is split.
    uber: bool,
    /// The shortest run worth a pipeline switch of its own.
    min_run: u32,
}

impl Default for ClassRouting {
    fn default() -> Self {
        Self {
            uber: false,
            min_run: crate::gpu::strips::RUN_SPLIT_MIN,
        }
    }
}

impl ClassRouting {
    /// The routing a renderer starts with, from the process configuration.
    fn from_config() -> Self {
        Self {
            uber: config::uber_program_forced(),
            min_run: config::run_split_min(),
        }
    }

    /// The class a composite quad is drawn with.
    fn composite_class(self) -> PaintClass {
        if self.uber {
            PaintClass::Uber
        } else {
            PaintClass::LayerComposite
        }
    }

    /// The class a hole punch is drawn with.
    fn punch_class(self) -> PaintClass {
        if self.uber {
            PaintClass::Uber
        } else {
            PaintClass::SolidCoverage
        }
    }
}

/// One strip state's pipelines, by paint class — the ones a pass resolved for
/// the classes its runs name.
#[derive(Debug, Default)]
struct ClassPipelines {
    by_class: [Option<wgpu::RenderPipeline>; PaintClass::ALL.len()],
}

impl ClassPipelines {
    /// The pipeline drawing `class`, if the frame resolved one.
    fn get(&self, class: PaintClass) -> Option<&wgpu::RenderPipeline> {
        self.by_class.get(class.index()).and_then(Option::as_ref)
    }

    fn set(&mut self, class: PaintClass, pipeline: wgpu::RenderPipeline) {
        if let Some(slot) = self.by_class.get_mut(class.index()) {
            *slot = Some(pipeline);
        }
    }

    /// Any one of the pipelines — what a bind group is built against, every
    /// strip pipeline sharing one explicit layout.
    fn any(&self) -> Option<&wgpu::RenderPipeline> {
        self.by_class.iter().flatten().next()
    }
}

/// The strip pipelines a renderer has resolved so far, by `(state, class)`,
/// for the target format they were built against.
///
/// A memo in front of the pipeline cache: a steady-state frame takes every
/// pipeline it draws with from here, by indexing a dense `state x class` table,
/// rather than rebuilding each description (a vertex-layout allocation) and
/// packing its cache key — which keeps the frame's `pipelines` phase from
/// growing with the number of classes it draws.
#[derive(Debug)]
struct ResolvedPipelines {
    /// The format every entry was built for; a change drops them all.
    format: Option<wgpu::TextureFormat>,
    /// Indexed by [`Self::slot`].
    entries: [Option<wgpu::RenderPipeline>; RESOLVED_SLOTS],
}

/// One [`ResolvedPipelines`] entry per render state and paint class.
const RESOLVED_SLOTS: usize = EnginePipeline::ALL.len() * PaintClass::ALL.len();

impl Default for ResolvedPipelines {
    fn default() -> Self {
        Self {
            format: None,
            entries: [const { None }; RESOLVED_SLOTS],
        }
    }
}

impl ResolvedPipelines {
    /// The table slot of `state` drawing `class`.
    const fn slot(state: EnginePipeline, class: PaintClass) -> usize {
        state.index() * PaintClass::ALL.len() + class.index()
    }
}

/// One pass's full recording state, assembled before the pass is begun.
///
/// A struct rather than an argument list because the composite groups have to
/// be built (and so borrowed) before `begin_render_pass` takes the encoder, and
/// naming them together is what makes that ordering obvious at the call site.
///
/// The attachment half (`pass`, `view`, `load`, `depth`) describes the pass the
/// plan begins when it is recorded on its own; the rest is what it draws, which
/// is all the [surface opening](SurfaceOpening) reads of the plans it draws
/// inside its own pass.
struct PassPlan<'a> {
    /// Which of the frame's passes this is — its label, and its entry in
    /// [`EngineRenderer::last_frame_passes`].
    pass: FramePass,
    view: &'a wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    depth: Option<&'a wgpu::TextureView>,
    /// The render state the pass draws with, which folds each run's class to
    /// one it serves.
    state: EnginePipeline,
    /// The state's pipeline per paint class — at least every class the pass's
    /// segments name, folded.
    pipelines: &'a ClassPipelines,
    /// The pipeline variant's own groups 1-3.
    groups: &'a StripBindGroups,
    /// Group 0 for the pass's ordinary strip segments.
    resources: &'a wgpu::BindGroup,
    /// Group 0 per composite segment, in the order the segments name them.
    composites: &'a [wgpu::BindGroup],
    /// The externally bound texture each [`Segment::External`] slot names,
    /// indexed by slot — the frame's own
    /// [`ExternalRuns::keys`](crate::gpu::bindings::ExternalRuns::keys).
    /// Empty for a pass that draws no external texture, which is every pass of
    /// every frame that records no `SceneTexture`.
    external_keys: &'a [u64],
    instances: &'a wgpu::Buffer,
    /// The instance index every segment's own index is relative to.
    base: u32,
    segments: &'a [Segment],
    /// The class a [`Segment::Composite`] is drawn with —
    /// [`PaintClass::LayerComposite`], or Uber under the kill switch.
    composite_class: PaintClass,
}

/// Records one pass: load the colour target, then draw each segment in order.
///
/// `timestamps` is the query pair the pass's GPU time is stamped into, `None`
/// for an untimed pass — which is every pass of every frame encoded without a
/// timestamp sink (see [`crate::diag`]). The pass is logged into `log` as it
/// is begun.
fn record_pass(
    encoder: &mut wgpu::CommandEncoder,
    plan: &PassPlan<'_>,
    timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    log: &mut Vec<FramePass>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(plan.pass.label()),
        color_attachments: &[Some(color_attachment(plan.view, plan.load))],
        depth_stencil_attachment: plan
            .depth
            .map(|view| depth_attachment(view, wgpu::LoadOp::Load)),
        timestamp_writes: timestamps,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    log.push(plan.pass);
    draw_segments(&mut pass, plan);
}

/// Draws `plan`'s segments, in order, into an already begun `pass`.
///
/// Two groups are re-bound per segment. Group 0, because a composite reads its
/// page through it while an ordinary strip reads the placeholder; and group 1,
/// because a run of instances sampling an externally bound texture needs that
/// texture bound where the atlas placeholder otherwise sits. Groups 2-3 are the
/// variant's own and never change within a plan. A plan with one segment —
/// every frame that records no layer and no external texture — sets them all
/// exactly once.
///
/// A segment naming a slot with no bind group behind it draws nothing rather
/// than drawing with whatever group 1 last held: the texture was unbound
/// between the frame's paint resolution and its recording, and a wrongly
/// sampled run is worse than a missing one.
///
/// Each segment is one paint-class run, and the pipeline is set only where
/// the class changes from the segment before — so a plan of one class (every
/// plan under the Uber kill switch) sets its pipeline exactly once. A segment
/// whose class has no resolved pipeline draws nothing, for the same reason; it
/// cannot occur, since the frame resolves every class its segments name before
/// recording.
///
/// Everything a plan draws with is set here, its first pipeline included, so
/// two plans drawn back to back into one pass — the opening's opaque strips
/// and then its round's — each start from their own state rather than from
/// whatever the plan before them left bound.
fn draw_segments(pass: &mut wgpu::RenderPass<'_>, plan: &PassPlan<'_>) {
    pass.set_vertex_buffer(0, plan.instances.slice(..));

    let mut composite = 0_usize;
    let mut bound: Option<PaintClass> = None;
    for segment in plan.segments {
        // Claimed before the class lookup, so a composite whose pipeline is
        // missing still consumes its own group and the next one pairs up.
        let class = plan.state.fold_class(match *segment {
            Segment::Strips(_, _, class) | Segment::External(_, _, _, class) => class,
            Segment::Composite(..) | Segment::Backdrop(..) => plan.composite_class,
        });
        let (group, images, range) = match *segment {
            Segment::Strips(first, count, _) => {
                if count == 0 {
                    continue;
                }
                (
                    plan.resources,
                    None,
                    GpuStrip::instance_range(plan.base.saturating_add(first), count),
                )
            }
            Segment::External(first, count, slot, _) => {
                if count == 0 {
                    continue;
                }
                let Some(images) = plan
                    .external_keys
                    .get(slot as usize)
                    .and_then(|key| plan.groups.externals.get(key))
                else {
                    continue;
                };
                (
                    plan.resources,
                    Some(images),
                    GpuStrip::instance_range(plan.base.saturating_add(first), count),
                )
            }
            Segment::Composite(first, _) => {
                let Some(group) = plan.composites.get(composite) else {
                    continue;
                };
                composite = composite.saturating_add(1);
                (
                    group,
                    None,
                    GpuStrip::instance_range(plan.base.saturating_add(first), 1),
                )
            }
            // Its group is claimed before its count is read, like a
            // composite's, so the groups after it stay paired.
            Segment::Backdrop(first, count, _) => {
                let Some(group) = plan.composites.get(composite) else {
                    continue;
                };
                composite = composite.saturating_add(1);
                if count == 0 {
                    continue;
                }
                (
                    group,
                    None,
                    GpuStrip::instance_range(plan.base.saturating_add(first), count),
                )
            }
        };
        if bound != Some(class) {
            let Some(pipeline) = plan.pipelines.get(class) else {
                continue;
            };
            pass.set_pipeline(pipeline);
            bound = Some(class);
        }
        plan.groups.bind_with(pass, group, images);
        pass.draw(GpuStrip::vertex_range(), range);
    }
}

/// The pass that opens the frame's own surface: the clear, the opaque strips,
/// and the first surface round that draws, in one render pass.
///
/// Merging is pixel-identical to recording the three apart, because nothing
/// between them differs but the pass boundary: the clear becomes the pass's
/// `LoadOp::Clear` on colour (and on depth, unless the caller stated its
/// attachment is already populated), and the two strip plans draw in the order
/// their passes ran, each through its own pipelines and bind groups, against
/// the one colour and depth attachment both already shared. What it saves is
/// the pass boundary itself: on a tile-based GPU every pass over the surface
/// is a whole-framebuffer load into tile memory and a store back out, and the
/// clear and the opaque pass were two of those that drew little or nothing.
///
/// Recorded exactly once per frame, by whichever comes first of the first
/// surface round that draws (which it then carries) and the first punch
/// (ahead of which it is recorded alone) — or at the end of the walk when
/// neither came. See [`EngineRenderer::set_pass_merge`] for the switch that
/// records the three apart again.
struct SurfaceOpening<'a> {
    view: &'a wgpu::TextureView,
    depth: Option<&'a wgpu::TextureView>,
    /// The frame's base colour, already in the target's alpha convention.
    clear: wgpu::Color,
    /// The depth clear, or a load when the caller's attachment is pre-cleared
    /// (see [`crate::gpu::depth`]).
    depth_load: wgpu::LoadOp<f32>,
    /// The opaque strips, when the frame has any to draw.
    opaque: Option<PassPlan<'a>>,
}

impl SurfaceOpening<'_> {
    /// Records the opening, drawing `round`'s segments after the opaque strips
    /// when a surface round is folded into it.
    fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        round: Option<&PassPlan<'_>>,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
        log: &mut Vec<FramePass>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(FramePass::Opening.label()),
            color_attachments: &[Some(color_attachment(
                self.view,
                wgpu::LoadOp::Clear(self.clear),
            ))],
            depth_stencil_attachment: self
                .depth
                .map(|view| depth_attachment(view, self.depth_load)),
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        log.push(FramePass::Opening);
        if let Some(opaque) = self.opaque.as_ref() {
            draw_segments(&mut pass, opaque);
        }
        if let Some(round) = round {
            draw_segments(&mut pass, round);
        }
    }
}

/// Records the surface opening as a pass of its own if it is still owed, and
/// marks it taken.
fn record_owed_opening(
    opening: &mut Option<SurfaceOpening<'_>>,
    encoder: &mut wgpu::CommandEncoder,
    timestamps: FrameTimestamps<'_>,
    log: &mut Vec<FramePass>,
) {
    if let Some(opening) = opening.take() {
        opening.record(encoder, None, timestamps.writes(EngineSpan::Main), log);
    }
}

/// Everything the frame's hole-punch passes are recorded with, resolved once
/// before the round walk begins.
///
/// A frame can now record several: the punches are issued at the painter-order
/// positions they were hoisted from, so a surface round carrying two of them is
/// cut twice and each cut erases through this same pipeline and these same
/// groups (see [`crate::compile::clear`]). Resolving them once is what keeps
/// that from becoming a per-cut lookup, and holding the borrows in one value is
/// what keeps them out of the page pool's way — the walk holds that mutably
/// throughout.
struct PunchPass<'a> {
    view: &'a wgpu::TextureView,
    depth: Option<&'a wgpu::TextureView>,
    /// Which destination-out state the punch draws with.
    state: EnginePipeline,
    /// That state's pipelines, holding at least [`Self::class`].
    pipelines: &'a ClassPipelines,
    /// The class every punch run is drawn with: the solid program, or Uber
    /// under the kill switch.
    class: PaintClass,
    /// The destination-out variant's own groups 1-3.
    groups: &'a StripBindGroups,
    instances: &'a wgpu::Buffer,
    /// The instance index the punch's own `(first, count)` is relative to — the
    /// alpha region's start, the same one every round's segments use.
    base: u32,
}

impl PunchPass<'_> {
    /// Records `punch`'s instances as one destination-out pass, or nothing at
    /// all when the cut issued none — which is every cut of every frame that
    /// records no `ClearRect`. A pass that is not recorded takes no query pair.
    fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        punch: (u32, u32),
        timestamps: FrameTimestamps<'_>,
        log: &mut Vec<FramePass>,
    ) {
        let (first, count) = punch;
        if count == 0 {
            return;
        }
        record_pass(
            encoder,
            &PassPlan {
                pass: FramePass::Punch,
                view: self.view,
                load: wgpu::LoadOp::Load,
                depth: self.depth,
                state: self.state,
                pipelines: self.pipelines,
                groups: self.groups,
                resources: &self.groups.resources,
                composites: &[],
                // A punch erases with a solid source; it samples nothing.
                external_keys: &[],
                instances: self.instances,
                base: self.base,
                // Every punch instance is the same solid source, so the run
                // is one class whole — no split to plan.
                segments: &[Segment::Strips(first, count, self.class)],
                composite_class: self.class,
            },
            timestamps.writes(EngineSpan::Main),
            log,
        );
    }
}

/// Hands back the page groups a finished round consumed, and parks the page it
/// wrote in its own group.
///
/// The same bookkeeping after every round, strip and filter alike: a page is
/// free the moment the pass that sampled it ends, which is what bounds a chain
/// of any depth — and a filter layer's own pair of pages — to the two ping-pong
/// groups, and every shape this scheduler serves to
/// [`MAX_LIVE_PAGES`](crate::schedule::MAX_LIVE_PAGES) live intermediates.
///
/// A round that is not continuing a page of its own takes a *fresh* texture out
/// of the pool rather than the group's current occupant, and the occupant it
/// displaces goes back here. That is what keeps a filter pass from ever holding
/// one texture as both its attachment and its source: a filter round never
/// continues a page — it clears — so what it writes is always a different
/// texture from the one the pass before it wrote and this pass reads.
fn settle_pages(
    targets: &mut IntermediateTargets,
    live: &mut [Option<PooledTexture>; MAX_LIVE_PAGES],
    released: [bool; MAX_LIVE_PAGES],
    own: Option<(PageParity, PooledTexture)>,
) {
    for (index, slot) in live.iter_mut().enumerate() {
        if released.get(index).copied().unwrap_or(false)
            && let Some(page) = slot.take()
        {
            targets.release(page);
        }
    }
    if let Some((parity, pooled)) = own
        && let Some(previous) = live[parity.index()].replace(pooled)
    {
        targets.release(previous);
    }
}

/// The GPU resources a frame's [filter](crate::filters) rounds are executed
/// with, beyond the two pooled pages they ping-pong between.
///
/// Three of them, and each is the engine's only one of its kind: the
/// filter-data texture holding every filter in the frame's 48-byte parameter
/// block, the bilinear sampler
/// ([`filter_sampler`](crate::gpu::targets::filter_sampler)) the blur kernels
/// read their source page through, and the instance buffer one quad per pass is
/// drawn from.
///
/// Public because this *is* executing a filter pass — the renderer holds one
/// and drives it over the pages the scheduler named, and `tests/filters.rs`
/// drives the same type over pages of its own on real hardware. That second
/// caller is not a convenience: `frust_scene` carries no *layer*-filter
/// command, so a filter layer cannot reach [`EngineRenderer::encode`] through a
/// `Scene` at all, and driving this type directly is how the drop shadow's
/// WGSL is exercised on a device. The blur's is also reached through `encode`
/// itself, by a backdrop blur's pass rounds.
#[derive(Debug)]
pub struct FilterResources {
    /// Every filter in the frame's parameter block, back to back.
    data: ResourceTexture,
    /// Group 0, naming [`Self::data`]'s view. Rebuilt whenever that texture is,
    /// and valid for the renderer's whole life otherwise: a filter pipeline's
    /// description does not depend on the frame's target format, so there is
    /// only ever one layout to have derived it from.
    data_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    instances: Option<wgpu::Buffer>,
    instance_capacity: u64,
    /// Reusable staging for the parameter-block upload, padded to the
    /// texture's own footprint.
    staging: Vec<u8>,
}

impl FilterResources {
    /// A renderer's filter resources, with nothing uploaded yet.
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            data: ResourceTexture::new(
                device,
                &filter_data_texture_descriptor(gpu::MIN_RESOURCE_TEXTURE_HEIGHT),
            ),
            data_group: None,
            sampler: filter_sampler(device),
            instances: None,
            instance_capacity: 0,
            staging: Vec::new(),
        }
    }

    /// Uploads this frame's parameter `blocks` and reserves room for `passes`
    /// pass instances, growing either resource if the frame outgrew it.
    ///
    /// `pipeline` is the one [`EnginePipeline::Filter`] describes; it is needed
    /// because wgpu derives a pipeline's bind-group layouts from its shader
    /// module, so the group naming the filter-data texture can only be built
    /// against the pipeline that will bind it.
    ///
    /// A block count no filter-data texture could hold (see
    /// [`filter_data_texture_height`]) leaves the group unbuilt, which leaves
    /// every filter pass of the frame issuing no draw — the page is still
    /// cleared, so the layer composites as transparent rather than as whatever
    /// its page last held. Unreachable in practice, and "draw less, never
    /// wrong" when it is not.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::RenderPipeline,
        blocks: &[GpuFilterData],
        passes: u32,
    ) {
        let Some(height) = filter_data_texture_height(blocks.len()) else {
            self.data_group = None;
            return;
        };
        if height > self.data.height {
            self.data = ResourceTexture::new(device, &filter_data_texture_descriptor(height));
            self.data_group = None;
        }
        if self.data_group.is_none() {
            self.data_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frust-engine filter data"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.data.view),
                }],
            }));
        }

        // A queue write covers the whole texture extent, so the blocks are
        // padded out to its footprint; the trailing texels are never addressed,
        // because a pass names its own block by a texel offset the host handed
        // it.
        let footprint = gpu::resource_texture_bytes(self.data.width, self.data.height);
        self.staging.clear();
        self.staging
            .resize(usize::try_from(footprint).unwrap_or(usize::MAX), 0);
        let bytes: &[u8] = bytemuck::cast_slice(blocks);
        if let Some(head) = self.staging.get_mut(..bytes.len()) {
            head.copy_from_slice(bytes);
        }
        queue.write_texture(
            self.data.copy_target(),
            &self.staging,
            resource_layout(
                gpu::resource_bytes_per_row(self.data.width),
                self.data.height,
            ),
            self.data.extent(),
        );

        self.reserve_instances(device, passes);
    }

    /// Grows the instance buffer if this frame's pass count outgrew it.
    fn reserve_instances(&mut self, device: &wgpu::Device, passes: u32) {
        let stride = size_of::<FilterInstanceData>() as u64;
        let required = u64::from(passes).saturating_mul(stride).max(stride);
        if self.instances.is_some() && self.instance_capacity >= required {
            return;
        }
        let capacity = required
            .checked_next_power_of_two()
            .unwrap_or(required)
            .max(MIN_FILTER_INSTANCE_CAPACITY.saturating_mul(stride));
        self.instance_capacity = capacity;
        self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-engine filter instances"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    /// Writes one pass's instance into slot `index` of the instance buffer.
    ///
    /// Separate from [`Self::prepare`] because a pass's `dest_texture_size` is
    /// the extent the texture pool *quantized* its destination page up to, and
    /// NDC is computed against the attachment's real extent — only the round
    /// that acquires the page knows it. A queue write issued while the frame is
    /// being recorded still lands ahead of the command buffers it is submitted
    /// with, which is the same ordering a page's viewport uniform already
    /// relies on.
    ///
    /// A slot past the reserved capacity is dropped rather than written, which
    /// leaves that pass drawing whatever the slot last held; unreachable, since
    /// `prepare` reserved one slot per pass of this very frame.
    pub fn write_instance(&self, queue: &wgpu::Queue, index: u32, instance: &FilterInstanceData) {
        let Some(buffer) = self.instances.as_ref() else {
            return;
        };
        let stride = size_of::<FilterInstanceData>() as u64;
        let offset = u64::from(index).saturating_mul(stride);
        if offset.saturating_add(stride) > self.instance_capacity {
            return;
        }
        queue.write_buffer(buffer, offset, bytemuck::bytes_of(instance));
    }

    /// Records one filter pass into `encoder`: clear the destination page, then
    /// draw the one instanced quad that filters `plan`'s source into it.
    ///
    /// The clear is unconditional and the draw is not. A filter pass writes only
    /// the region its step names — a decimated one a quarter of the texels the
    /// pass before it did — and the kernels sample past that region without
    /// bounds checks, so whatever surrounds it has to be transparent rather than
    /// a previous holder's pixels. That has to hold even on the path where the
    /// pass itself cannot be issued, or the layer's composite would sample the
    /// page's previous tenant instead of nothing.
    ///
    /// The pass is opened and closed here, on the caller's own encoder: a filter
    /// round is not an exception to [`EngineRenderer::encode`]'s contract.
    pub fn record_pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        plan: &FilterPassPlan<'_>,
    ) {
        self.record_pass_timed(device, encoder, plan, None);
    }

    /// [`Self::record_pass`], stamping the pass's GPU time into `timestamps`.
    ///
    /// A separate method rather than a field on [`FilterPassPlan`]: the plan is
    /// public and built by struct literal outside this crate, so a new required
    /// field would break every one of those call sites to serve a diagnostic
    /// they do not use. `None` records exactly what [`Self::record_pass`] does.
    pub fn record_pass_timed(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        plan: &FilterPassPlan<'_>,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(plan.label),
            color_attachments: &[Some(color_attachment(
                plan.dest,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ))],
            // A pooled page carries no depth attachment, which is also why the
            // filter pipeline declares no depth state.
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        let (Some(data_group), Some(instances)) =
            (self.data_group.as_ref(), self.instances.as_ref())
        else {
            return;
        };

        let source = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine filter source"),
            layout: &plan.pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(plan.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        pass.set_pipeline(plan.pipeline);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.set_bind_group(0, data_group, &[]);
        pass.set_bind_group(1, &source, &[]);
        pass.draw(
            0..FILTER_QUAD_VERTICES,
            plan.instance..plan.instance.saturating_add(1),
        );
    }
}

/// One filter pass's full recording state.
///
/// The two pages are named as views rather than as parities because
/// [`FilterResources`] holds no pool: which texture each group is holding is the
/// caller's bookkeeping, whether that caller is the frame path or a test.
#[derive(Debug)]
pub struct FilterPassPlan<'a> {
    /// Label for captures and validation messages.
    pub label: &'a str,
    /// The pipeline [`EnginePipeline::Filter`] describes.
    pub pipeline: &'a wgpu::RenderPipeline,
    /// The page this pass writes. Cleared to transparent before it is written.
    pub dest: &'a wgpu::TextureView,
    /// The page this pass reads — the one the pass before it wrote.
    pub source: &'a wgpu::TextureView,
    /// Which instance of the filter instance buffer this pass draws.
    pub instance: u32,
}

/// A colour attachment over `view` with `load`, keeping what it stores.
fn color_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

/// A depth attachment over `view` with `load`, keeping what it stores so the
/// pass after it tests against the same buffer.
fn depth_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<f32>,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    }
}

/// The frame's base colour as a clear value.
///
/// The strip pipelines blend premultiplied and the shader emits premultiplied
/// colour, so a [`OutputAlpha::Premultiplied`] target's clear value has to be
/// premultiplied too — otherwise the background sits in a different alpha
/// convention from everything drawn over it. `Straight` is honoured here and
/// only here: the pipelines themselves are fixed premultiplied, so a straight
/// target's *drawn* content is premultiplied regardless.
fn clear_color(base_color: Color, output: OutputAlpha) -> wgpu::Color {
    let components = match output {
        OutputAlpha::Premultiplied => base_color.premultiply().components,
        OutputAlpha::Straight => base_color.components,
    };
    wgpu::Color {
        r: f64::from(components[0]),
        g: f64::from(components[1]),
        b: f64::from(components[2]),
        a: f64::from(components[3]),
    }
}

/// Whether a frame cleared to `base_color` presents opaquely, and so whether
/// its alpha channel carries anything a hole punch could reveal.
///
/// The engine's only statement about the target's own alpha handling:
/// [`EngineTarget`] describes how the alpha it produces is *interpreted*
/// (premultiplied or straight), never whether it is used at all. A base colour
/// at full alpha seals every pixel of the surface, which is exactly the
/// presentation [`crate::compile::clear`]'s contract says to skip the
/// destination-out pass on.
fn is_opaque(base_color: Color) -> bool {
    base_color.components[3] >= 1.0
}

/// The target extent on the `u16` device grid the strip pipeline addresses.
fn grid_size(width: u32, height: u32) -> Result<(u16, u16), EngineError> {
    let width = u16::try_from(width).map_err(|_| EngineError::TargetTooLarge)?;
    let height = u16::try_from(height).map_err(|_| EngineError::TargetTooLarge)?;
    Ok((width, height))
}

/// One unit of drawing inside a round's pass, in execution order.
///
/// Instance indices are relative to the alpha region of the shared instance
/// buffer, which is why [`PassPlan::base`] exists rather than the indices being
/// absolute: the opaque region is laid out first and its own segment addresses
/// from zero.
///
/// Every strip segment is also one paint-class run: it names the
/// [`PaintClass`] whose program draws it, and a pass sets that class's
/// pipeline where the class changes (see the module doc's *Paint-class runs*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    /// Ordinary strip instances, as `(first, count, class)`, drawn with the
    /// frame's own atlas binding.
    Strips(u32, u32, PaintClass),
    /// Strip instances sampling an externally bound texture, as
    /// `(first, count, slot, class)` — a *run*, in the sense
    /// [`crate::gpu::bindings::ExternalRuns`] gives the word: the maximal span
    /// of consecutive instances that share one texture, drawn with that
    /// texture's own group 1 bound.
    External(u32, u32, u32, PaintClass),
    /// One composite quad at `first`, sampling the finished page it names.
    Composite(u32, CompositeSource),
    /// A backdrop blur's own strip instances, as `(first, count, parity)`:
    /// its rectangle's coverage, each instance sampling the blurred page in
    /// `parity`'s group at its own position — drawn through the composite
    /// class with that page bound as group 0's layer input.
    Backdrop(u32, u32, PageParity),
}

/// The page a [`Segment::Composite`] samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositeSource {
    /// The live pooled page in this group.
    Page(PageParity),
    /// The retained page of this retained bracket (see
    /// [`crate::schedule::RetainedSource`]).
    Retained(u32),
}

/// The retained page one round renders a body into.
#[derive(Debug, Clone, Copy)]
struct RetainedRound {
    /// The retained bracket the page belongs to.
    bracket: u32,
    /// Whether an earlier round of the same body already rendered into the
    /// page this frame, so this one loads it rather than clearing it.
    continued: bool,
}

/// One scheduled round, resolved to the instances and target it draws with.
///
/// One *plan* rather than one scheduled round: a surface round carrying a hole
/// punch is cut into a plan per span between its punches, so the punch pass can
/// be recorded at the painter-order position it was hoisted from (see
/// [`Scratch::cut_for_punches`]).
#[derive(Debug, Clone)]
struct RoundPlan {
    /// The page this round renders into, or `None` for the frame's own target
    /// and for a retained page.
    page: Option<PagePlan>,
    /// The retained page this round renders a body into, if it does — an
    /// off-screen round like a page one, into the store's texture.
    retained: Option<RetainedRound>,
    /// This round's slice of [`Scratch::segments`], in execution order.
    segments: Range<usize>,
    /// Page groups this round consumed, indexed by
    /// [`PageParity::index`]; each returns to the pool once the pass ends.
    released: [bool; MAX_LIVE_PAGES],
    /// The filter pass this round runs, on a filter round — which draws no
    /// strip and composites nothing, so its `segments` range is empty.
    filter: Option<FilterPlan>,
    /// The region of the frame's own target this round copies into its page,
    /// on a region-copy round — which, like a filter round, draws no strip.
    copy: Option<RectU16>,
    /// The hole punches to erase with once this plan's own pass has been
    /// recorded, as `(first, count)` into the alpha region — the cut this plan
    /// ends at.
    ///
    /// Only a *surface* plan ever carries one: a punch recorded inside an
    /// isolated layer was hoisted out of it to the frame root (see
    /// [`crate::compile::clear`]), so a page round is never cut and a filter
    /// round — which draws nothing of the frame's own — never is either.
    punch: (u32, u32),
}

/// One filter pass, resolved to the instance slot it draws and the page group
/// it reads.
#[derive(Debug, Clone, Copy)]
struct FilterPlan {
    /// Which pass of the filter's sequence this is, and at what extents.
    step: FilterStep,
    /// The group holding the page this pass reads.
    source: PageParity,
    /// The texel this filter's parameter block starts at in the filter-data
    /// texture — where the fragment stage reads its kernel from.
    data_offset: u32,
    /// The filter layer's own extent, before any decimation; it bounds the
    /// transparent border a decimated pass overdraws.
    original: SizeU16,
    /// This pass's slot in the frame's filter instance buffer.
    instance: u32,
}

/// The pooled page one round renders into.
#[derive(Debug, Clone, Copy)]
struct PagePlan {
    parity: PageParity,
    size: PageSize,
    /// Whether an earlier round of the same layer already rendered into this
    /// page, so this round takes that texture back rather than acquiring a
    /// fresh one, and loads it rather than clearing it.
    continued: bool,
}

/// The frame's instance buffers and pass plan, retained so a steady-state frame
/// refills them rather than reallocating them.
///
/// The segments of every round live in one flat vector rather than a vector per
/// round, so a frame with layers costs no allocation once the first one has
/// grown these buffers.
#[derive(Debug, Default)]
struct Scratch {
    /// Fully-covered spans of the opaque draws targeting the frame's own
    /// surface, drawn unblended with depth write in one pass ahead of every
    /// round — including the surface's own, of which a frame may have several.
    opaque: Vec<GpuStrip>,
    /// Everything else, drawn premultiplied-blended in painter order and laid
    /// out round by round in execution order.
    alpha: Vec<GpuStrip>,
    /// Every round's segments, back to back; [`RoundPlan::segments`] slices it.
    segments: Vec<Segment>,
    /// The frame's rounds, each layer's before the round that composites it and
    /// the surface's last round last.
    rounds: Vec<RoundPlan>,
    /// The instances of the punches no cut reached — those recorded past every
    /// op of the frame — as `(first, count)` into the alpha region.
    ///
    /// Their pass is the last thing the frame records, which is where a
    /// `ClearRect` recorded last belongs in painter order anyway. Punches the
    /// walk *did* cut at are held on their own [`RoundPlan::punch`] instead.
    ///
    /// No round's segments name a punch instance, wherever it sits in the
    /// buffer, so dropping the punch passes leaves the frame exactly as the
    /// display list would read without the clear.
    punch: (u32, u32),
    /// The deepest painter's-order index inside each recorded layer, which is
    /// the depth its composite carries. Filled as the rounds are walked, which
    /// is sound because a layer's own round always precedes the round that
    /// composites it.
    layer_depth: Vec<u32>,
    /// One parameter block per *filter layer* of the frame, in the order the
    /// rounds first named them — which is the order the texel offsets in
    /// [`FilterPlan::data_offset`] were taken from.
    filter_blocks: Vec<GpuFilterData>,
    /// The layers `filter_blocks` holds, parallel to it, so a filter's second
    /// and later passes reuse the block its first one packed rather than
    /// repacking one per pass.
    filter_layers: Vec<u32>,
    /// How many filter passes this frame runs, and so how many instances its
    /// filter instance buffer has to hold.
    filter_passes: u32,
    /// How many region-copy rounds this frame runs — one per backdrop blur
    /// the scheduler served.
    copies: u32,
    /// How segments are cut into paint-class runs this frame — copied in from
    /// the renderer ahead of [`Self::build`].
    routing: ClassRouting,
    /// The opaque region's own segments: `(0, opaque.len())` cut into class
    /// runs, drawn by the one depth-writing pass.
    opaque_segments: Vec<Segment>,
    /// The classes the opaque pass's runs name.
    opaque_classes: ClassSet,
    /// The classes the frame's surface rounds name — composites included.
    surface_classes: ClassSet,
    /// The classes the frame's page rounds name — composites included.
    page_classes: ClassSet,
    /// Whether the round being planned renders into a page, which decides the
    /// class set [`Self::push_run`] records a run's class in.
    planning_page: bool,
    /// Reusable output of [`merge_class_runs`].
    class_runs: Vec<ClassRun>,
    /// Where the paint class changes in [`Self::alpha`], as `(first instance,
    /// class)` in ascending order — one entry per draw whose class differs
    /// from the draw before it. Every instance of a draw shares its paint, so
    /// these are the only places a class run can start, and a segment is split
    /// by walking them instead of re-reading every instance.
    alpha_marks: Vec<(u32, PaintClass)>,
    /// The same marks for [`Self::opaque`].
    opaque_marks: Vec<(u32, PaintClass)>,
}

impl Scratch {
    /// Turns a scheduled frame into instances and a per-round pass plan,
    /// routing each instance to the pass that can draw it.
    ///
    /// A draw's anti-aliased spans always land in the blended buffer: partial
    /// coverage is not opaque however opaque the paint is. Its fully-covered
    /// spans land in the opaque buffer only when the paint is opaque, depth is
    /// available to re-establish their ordering against the blended ones, *and*
    /// the draw targets the frame's own surface — a page has no depth
    /// attachment, so a page round is a plain painter's-algorithm walk.
    ///
    /// `backdrops` is the frame's served backdrop blurs — `frame.backdrops`,
    /// or none at all on a target no region can be read back from. A draw the
    /// scheduler placed at a backdrop's own depth is that backdrop's rectangle
    /// and samples its blurred page instead of its recorded paint; and no draw
    /// at or past the first backdrop's depth takes the depth-writing pass,
    /// which is recorded ahead of every surface round — so ahead of the copy
    /// that reads the target back, which must not see what is painted over the
    /// backdrop.
    fn build(
        &mut self,
        frame: &CompiledFrame,
        rounds: &[Round],
        depth_active: bool,
        punches: bool,
        paint_slots: &[Option<ResolvedPaint>],
        backdrops: &[Backdrop],
    ) {
        self.opaque.clear();
        self.alpha.clear();
        self.segments.clear();
        self.rounds.clear();
        self.punch = (0, 0);
        self.layer_depth.clear();
        self.layer_depth.resize(frame.recorder.layers.len(), 0);
        // A `DrawRetained`'s layer holds no draw, so its composite's depth is
        // the slot the command itself was recorded at rather than the deepest
        // index inside it.
        for bracket in &frame.retained {
            if let RetainedKind::Draw { depth, .. } = bracket.kind
                && let Some(slot) = self.layer_depth.get_mut(bracket.layer as usize)
            {
                *slot = depth;
            }
        }
        self.filter_blocks.clear();
        self.filter_layers.clear();
        self.filter_passes = 0;
        self.copies = 0;
        self.opaque_segments.clear();
        self.alpha_marks.clear();
        self.opaque_marks.clear();
        self.opaque_classes = ClassSet::EMPTY;
        self.surface_classes = ClassSet::EMPTY;
        self.page_classes = ClassSet::EMPTY;

        let draws = frame.draws();
        let strips = frame.strip_buf();
        // The punches still to be issued, in the order the compiler hoisted
        // them — which is depth order, since each took its own index from the
        // frame's monotonic painter-order counter. Empty on a frame with no
        // clear and on a target that disregards alpha (`punches`), and then
        // nothing below ever cuts: a non-punching frame is planned exactly as
        // it always was.
        let mut pending: &[ClearPunch] = if punches { &frame.clears } else { &[] };
        // Every draw before the first backdrop may take the depth-writing pass;
        // none from it on. `u32::MAX` on a frame with no backdrop, which is
        // then routed exactly as it always was.
        let opaque_limit = backdrops
            .first()
            .map_or(u32::MAX, |backdrop| backdrop.depth);

        for round in rounds {
            let page = round.page();
            let retained = round.retained();
            let offscreen = page.is_some() || retained.is_some();

            // A region-copy round draws nothing: it is planned as its page and
            // the region it copies. Any punch recorded ahead of the backdrop
            // it serves is erased first, as a cut of its own, so the copy reads
            // the target the display list says lies beneath the backdrop.
            if let Some(copy) = round.region_copy() {
                if let Some(page) = page {
                    if !pending.is_empty()
                        && let Some(backdrop) = backdrops
                            .iter()
                            .find(|backdrop| backdrop.layer == page.layer)
                    {
                        let mut first_segment = self.segments.len();
                        let mut run_start = self.alpha.len() as u32;
                        let mut run_external = None;
                        self.cut_for_punches(
                            &mut pending,
                            strips,
                            backdrop.depth,
                            &mut first_segment,
                            &mut run_start,
                            &mut run_external,
                        );
                    }
                    self.copies = self.copies.saturating_add(1);
                    self.rounds.push(RoundPlan {
                        page: Some(PagePlan {
                            parity: page.parity,
                            size: page.size,
                            continued: false,
                        }),
                        retained: None,
                        segments: self.segments.len()..self.segments.len(),
                        released: released_groups(&round.released),
                        filter: None,
                        copy: Some(copy.source),
                        punch: (0, 0),
                    });
                }
                continue;
            }

            let filter = self.plan_filter(frame, round, backdrops);
            // A page holds its layer — or one column band of it — at the page's
            // own origin, so every instance of the round is shifted into that
            // column and clipped to it (see `PageWindow`). A retained page holds
            // its body the same way.
            let window = match (page, retained) {
                (Some(page), _) => PageWindow::of(page),
                (None, Some(retained)) => PageWindow::of_retained(retained),
                (None, None) => PageWindow::ROOT,
            };
            let split_opaque = !offscreen && depth_active;
            self.planning_page = offscreen;
            // Only the frame's own target is punched — a punch inside an
            // isolated layer was hoisted out of it — so only a surface round is
            // ever cut.
            let cuts = !offscreen;

            let mut first_segment = self.segments.len();
            let mut deepest = 0_u32;
            let mut run_start = self.alpha.len() as u32;
            // The external texture the open run is drawn with, `None` while it
            // is drawn with the frame's own atlas binding. A draw that names a
            // different one closes the run: there is a single external binding
            // to set (see [`crate::gpu::bindings`]).
            let mut run_external: Option<u32> = None;

            for op in &round.ops {
                match op {
                    RoundOp::Draws(range) => {
                        let batch = draws
                            .get(range.start as usize..range.end as usize)
                            .unwrap_or(&[]);
                        for draw in batch {
                            deepest = deepest.max(draw.depth);
                            // Ahead of the paint lookup rather than after it, so
                            // the cut follows the order the display list was
                            // recorded in rather than the subset of it that
                            // survived lowering.
                            if cuts && !pending.is_empty() {
                                self.cut_for_punches(
                                    &mut pending,
                                    strips,
                                    draw.depth,
                                    &mut first_segment,
                                    &mut run_start,
                                    &mut run_external,
                                );
                            }
                            // A backdrop's rectangle: drawn from the blurred
                            // page this round samples rather than from the
                            // transparent paint it was recorded with.
                            if let Ok(at) = backdrops
                                .binary_search_by_key(&draw.depth, |backdrop| backdrop.depth)
                            {
                                if let Some(backdrop) = backdrops.get(at)
                                    && let Some(sample) = round
                                        .samples
                                        .iter()
                                        .find(|sample| sample.layer == backdrop.layer)
                                    && let Some(run) = strips.get(draw.strip_range.clone())
                                {
                                    let end = self.alpha.len() as u32;
                                    self.push_run(run_start, end, run_external);
                                    run_external = None;
                                    self.push_backdrop(
                                        backdrop,
                                        sample.parity,
                                        draw.depth,
                                        run,
                                        window,
                                    );
                                    run_start = self.alpha.len() as u32;
                                }
                                continue;
                            }
                            let Some(paint) = pack_paint(&draw.paint, draw.depth, paint_slots)
                            else {
                                continue;
                            };
                            if paint.glyph_quad {
                                // One rect instance, from the quad alone: the
                                // draw's strips are its rectangle's coverage,
                                // which the instance covers whole.
                                let Some(quad) = glyph_quad(&frame.glyph_quads, draw.depth) else {
                                    continue;
                                };
                                if paint.external != run_external {
                                    let end = self.alpha.len() as u32;
                                    self.push_run(run_start, end, run_external);
                                    run_start = end;
                                    run_external = paint.external;
                                }
                                mark_class(
                                    &mut self.alpha_marks,
                                    self.alpha.len() as u32,
                                    paint.class,
                                );
                                if let Some(instance) = window.place_quad(quad, paint) {
                                    self.alpha.push(instance);
                                }
                                continue;
                            }
                            let Some(run) = strips.get(draw.strip_range.clone()) else {
                                continue;
                            };
                            // Before the first of this draw's instances lands,
                            // so the run that closes holds exactly the
                            // instances drawn with the texture it names.
                            if paint.external != run_external {
                                let end = self.alpha.len() as u32;
                                self.push_run(run_start, end, run_external);
                                run_start = end;
                                run_external = paint.external;
                            }
                            let to_opaque =
                                paint.opaque && split_opaque && draw.depth < opaque_limit;
                            let class = paint.class;
                            mark_class(&mut self.alpha_marks, self.alpha.len() as u32, class);
                            if to_opaque {
                                mark_class(&mut self.opaque_marks, self.opaque.len() as u32, class);
                            }

                            // A generation's last strip is its sentinel, which
                            // is what carries the preceding strip's extent — so
                            // every instance comes from a pair, and the
                            // sentinel itself never becomes one.
                            for pair in run.windows(2) {
                                self.push_span(&pair[0], &pair[1], paint, to_opaque, window);
                            }
                        }
                    }
                    RoundOp::Composite(composite) => {
                        let depth = self
                            .layer_depth
                            .get(composite.layer as usize)
                            .copied()
                            .unwrap_or(0);
                        // A composite carries the deepest index inside its
                        // layer, so a layer recorded after a punch is cut
                        // against it exactly like a draw would be — which is
                        // what keeps a translucent layer over the slot from
                        // being erased by it.
                        if cuts && !pending.is_empty() {
                            self.cut_for_punches(
                                &mut pending,
                                strips,
                                depth,
                                &mut first_segment,
                                &mut run_start,
                                &mut run_external,
                            );
                        }

                        // Consecutive draw batches merge into one segment; a
                        // composite is what breaks the run, because it binds a
                        // different page as its colour source.
                        let end = self.alpha.len() as u32;
                        self.push_run(run_start, end, run_external);

                        deepest = deepest.max(depth);
                        let source = match composite.retained {
                            Some(retained) => CompositeSource::Retained(retained.bracket),
                            None => CompositeSource::Page(composite.parity),
                        };
                        self.segments.push(Segment::Composite(end, source));
                        self.note_class(self.routing.composite_class());
                        self.alpha
                            .push(composite_instance(composite, window.origin(), depth));
                        run_start = self.alpha.len() as u32;
                        // A composite draws through group 1's own atlas
                        // binding, so the run after it starts un-bound again.
                        run_external = None;
                    }
                }
            }

            let end = self.alpha.len() as u32;
            self.push_run(run_start, end, run_external);

            // Accumulated rather than assigned: a layer whose round was cut
            // renders in several rounds, and the depth its composite carries is
            // the deepest index across all of them.
            let own_layer = page
                .map(|page| page.layer)
                .or(retained.map(|retained| retained.layer));
            if let Some(layer) = own_layer
                && let Some(slot) = self.layer_depth.get_mut(layer as usize)
            {
                *slot = (*slot).max(deepest);
            }

            let released = released_groups(&round.released);

            self.rounds.push(RoundPlan {
                page: page.map(|page| PagePlan {
                    parity: page.parity,
                    size: page.size,
                    continued: page.continued,
                }),
                retained: retained.map(|retained| RetainedRound {
                    bracket: retained.bracket,
                    continued: retained.continued,
                }),
                segments: first_segment..self.segments.len(),
                released,
                filter,
                copy: None,
                // The round's last plan draws to its own end; a punch past
                // every op of the frame is issued after the whole walk instead.
                punch: (0, 0),
            });
        }

        // Whatever no op was recorded after: a `ClearRect` recorded last, which
        // is the ordinary shape of a platform-view slot.
        if !pending.is_empty() {
            self.punch = self.push_punches(pending, strips);
        }

        // The opaque region is one segment of the one depth-writing pass, cut
        // into class runs like any other.
        self.class_runs.clear();
        merge_class_runs(
            marked_runs(&self.opaque_marks, 0, self.opaque.len() as u32),
            self.routing.min_run,
            self.routing.uber,
            &mut self.class_runs,
        );
        debug_assert!(
            runs_draw(&self.class_runs, &self.opaque),
            "the opaque marks disagree with the instances they mark"
        );
        for run in &self.class_runs {
            self.opaque_segments
                .push(Segment::Strips(run.first, run.count, run.class));
            self.opaque_classes.insert(run.class);
        }
    }

    /// Records that the round being planned draws a run of `class`.
    fn note_class(&mut self, class: PaintClass) {
        if self.planning_page {
            self.page_classes.insert(class);
        } else {
            self.surface_classes.insert(class);
        }
    }

    /// Closes the open span of a surface round at `depth`, issuing every punch
    /// recorded before it as a pass of its own.
    ///
    /// This is the whole painter-order restoration: the ops recorded before the
    /// punch become a plan that ends here, the punch's own instances follow
    /// them in the buffer, and the ops recorded after it start a plan of their
    /// own — so the erase lands between the two rather than after both. A punch
    /// pass is a pass of its own because it draws through a different pipeline
    /// (destination-out) than the strips around it, not because of what it
    /// covers.
    ///
    /// Does nothing when no pending punch is shallower than `depth`, which is
    /// every op of every frame that records no clear.
    fn cut_for_punches(
        &mut self,
        pending: &mut &[ClearPunch],
        strips: &[Strip],
        depth: u32,
        first_segment: &mut usize,
        run_start: &mut u32,
        run_external: &mut Option<u32>,
    ) {
        let cut = pending
            .iter()
            .take_while(|punch| punch.depth < depth)
            .count();
        if cut == 0 {
            return;
        }
        let (issued, rest) = pending.split_at(cut);
        *pending = rest;

        // Close the run this cut interrupts, so the plan's segments name only
        // what was recorded before the punch.
        let end = self.alpha.len() as u32;
        self.push_run(*run_start, end, *run_external);
        // The punch's own instances go in next and are drawn solid, so the run
        // resumed after the cut starts with nothing bound.
        *run_external = None;

        let punch = self.push_punches(issued, strips);
        self.rounds.push(RoundPlan {
            page: None,
            retained: None,
            segments: *first_segment..self.segments.len(),
            // The pages a cut round consumed are handed back when its LAST plan
            // ends: nothing between two plans of one round acquires a page, and
            // a composite in an earlier plan still has to sample the page it
            // names.
            released: [false; MAX_LIVE_PAGES],
            filter: None,
            copy: None,
            punch,
        });
        *first_segment = self.segments.len();
        *run_start = self.alpha.len() as u32;
    }

    /// Records the open run of instances `first..end` as segments drawn with
    /// the texture in `external` bound — one per paint-class run the span
    /// splits into — or nothing at all when the run is empty.
    ///
    /// The one place a strip segment is created, so both run-breaking rules —
    /// a segment holds instances sharing one group-1 binding, and one paint
    /// class ([`merge_class_runs`]) — are stated once rather than at each of
    /// the four points a run can close. The class runs tile the span in order,
    /// so the segments still draw `first..end` front to back exactly as one
    /// segment did.
    fn push_run(&mut self, first: u32, end: u32, external: Option<u32>) {
        if end.checked_sub(first).filter(|count| *count > 0).is_none() {
            return;
        }
        let Some(span) = self.alpha.get(first as usize..end as usize) else {
            return;
        };
        self.class_runs.clear();
        merge_class_runs(
            marked_runs(&self.alpha_marks, first, end),
            self.routing.min_run,
            self.routing.uber,
            &mut self.class_runs,
        );
        debug_assert!(
            runs_draw(&self.class_runs, span),
            "the alpha marks disagree with the instances they mark"
        );
        for index in 0..self.class_runs.len() {
            let run = self.class_runs[index];
            let at = first.saturating_add(run.first);
            self.segments.push(match external {
                Some(slot) => Segment::External(at, run.count, slot, run.class),
                None => Segment::Strips(at, run.count, run.class),
            });
            self.note_class(run.class);
        }
    }

    /// Appends a backdrop blur's rectangle — the strip run `run` of its one
    /// draw, at painter-order `depth` — as instances sampling the blurred page
    /// in `parity`'s group, and the [`Segment::Backdrop`] drawing them.
    ///
    /// Each instance reads the page at its own position relative to the
    /// backdrop's region, which the page holds at its origin, so every pixel
    /// of the rectangle takes the blur of the backdrop under that very pixel;
    /// the run's own coverage — the rectangle's edges, and any coverage-mask
    /// clip it was generated under — weights it exactly as it would a fill.
    /// Never opaque: the blur of a backdrop reaching transparent edges is not.
    fn push_backdrop(
        &mut self,
        backdrop: &Backdrop,
        parity: PageParity,
        depth: u32,
        run: &[Strip],
        window: PageWindow,
    ) {
        let class = self.routing.composite_class();
        let paint = PackedPaint {
            payload: PaintPayload::Layer(backdrop.region.x0, backdrop.region.y0),
            paint: LAYER_PAINT_SOURCE | u32::from(u8::MAX),
            depth_index: depth,
            opaque: false,
            external: None,
            class,
            glyph_quad: false,
        };
        let first = self.alpha.len() as u32;
        for pair in run.windows(2) {
            self.push_span(&pair[0], &pair[1], paint, false, window);
        }
        let count = (self.alpha.len() as u32).saturating_sub(first);
        if count > 0 {
            self.segments.push(Segment::Backdrop(first, count, parity));
            self.note_class(class);
        }
    }

    /// Turns `punches` into destination-out instances, appended to the alpha
    /// region and named by no round's segments, as `(first, count)`.
    ///
    /// Each punch is a strip run like any other, drawn with an opaque source so
    /// the blend state's `1 − src.a` reaches zero exactly where the coverage is
    /// full, and carrying the painter-order depth it was hoisted from — which
    /// still orders it against the frame's one depth-writing pass, whose opaque
    /// coverage is recorded once ahead of every round and so cannot be cut.
    fn push_punches(&mut self, punches: &[ClearPunch], strips: &[Strip]) -> (u32, u32) {
        let first = self.alpha.len() as u32;

        for punch in punches {
            let paint = PackedPaint {
                payload: PaintPayload::Solid(PUNCH_SOURCE),
                paint: SOLID_PAINT,
                depth_index: punch.depth,
                // An erase never joins the depth-writing pass: it establishes
                // no colour for a later fragment to be rejected against.
                opaque: false,
                external: None,
                class: PaintClass::SolidCoverage,
                glyph_quad: false,
            };
            let Some(run) = strips.get(punch.strip_range.clone()) else {
                continue;
            };
            for pair in run.windows(2) {
                // Only the frame's own target is punched, so a punch's
                // instances are never shifted and never clipped.
                self.push_span(&pair[0], &pair[1], paint, false, PageWindow::ROOT);
            }
        }

        (first, (self.alpha.len() as u32).saturating_sub(first))
    }

    /// Emits the instances the `strip`/`next` pair describes: the strip's own
    /// alpha-sampled span, plus the solid span filling the gap to `next` when
    /// the winding between them says there is one.
    ///
    /// `window` shifts each instance's *geometry* into the round's target and
    /// clips it to the column that target holds, while the paint is still
    /// sampled at the scene position the strip was rasterized at — a layer's
    /// contents move into its page, the gradient or image painting them does
    /// not. An instance the window culls entirely is not emitted at all.
    fn push_span(
        &mut self,
        strip: &Strip,
        next: &Strip,
        paint: PackedPaint,
        to_opaque: bool,
        window: PageWindow,
    ) {
        let values = paint.values_at(strip.x, strip.y);
        let mut span = GpuStrip::from_strip_pair(strip, next, values);
        if window.place(&mut span, paint) {
            self.alpha.push(span);
        }
        // A gap starts where the strip ends rather than where it begins, so a
        // position-sampled paint is re-evaluated at the gap's own origin — the
        // gap is a different piece of the scene, not a continuation of the
        // span's sampling. `place` re-evaluates it once more if the window's
        // own left edge moves the instance again.
        if let Some(mut gap) = GpuStrip::gap_fill(strip, next, values) {
            gap.payload = paint.payload_at(gap.x, gap.y);
            if window.place(&mut gap, paint) {
                if to_opaque {
                    self.opaque.push(gap);
                } else {
                    self.alpha.push(gap);
                }
            }
        }
    }

    /// Resolves `round`'s filter pass, if it has one, packing the layer's
    /// parameter block on first sight and claiming the pass's instance slot.
    ///
    /// `None` for an ordinary round, and also for the two shapes that cannot
    /// occur: a filter round with no page (the scheduler always gives one a
    /// page) and a recorded kind [`served_filter`] does not recognise (the
    /// scheduler refuses one before it plans a round). Both answer by leaving
    /// the round's pass unissued — its
    /// page is still cleared — rather than by asserting (E17).
    fn plan_filter(
        &mut self,
        frame: &CompiledFrame,
        round: &Round,
        backdrops: &[Backdrop],
    ) -> Option<FilterPlan> {
        let pass = round.filter_pass()?;
        let page = round.page()?;
        let recorded = frame.recorder.layers.get(pass.layer as usize)?;
        // A backdrop's layer is a regular one: its blur is the backdrop
        // record's, not the recording's.
        let data_offset = match backdrops
            .iter()
            .find(|backdrop| backdrop.layer == pass.layer)
        {
            Some(backdrop) => self.pack_filter_block(pass.layer, || {
                Some(GpuFilterData::from(GpuGaussianBlur::from(&backdrop.blur())))
            })?,
            None => self.filter_block(pass.layer, &recorded.kind)?,
        };

        let instance = self.filter_passes;
        self.filter_passes = self.filter_passes.saturating_add(1);
        Some(FilterPlan {
            step: pass.step,
            source: pass.source,
            data_offset,
            original: SizeU16::from(page.bounds),
            instance,
        })
    }

    /// The texel offset of `layer`'s parameter block, packing the block on
    /// first sight.
    ///
    /// A linear scan rather than a map: a frame's filter layers are counted in
    /// ones (a filter layer is served only directly under the surface), and one
    /// allocation-free vector beats a hash map that would have to be cleared
    /// every frame.
    fn filter_block(&mut self, layer: u32, kind: &RecordedLayerKind) -> Option<u32> {
        // The one dispatch every filter-recognising site in this crate shares
        // (`schedule::layer_role`/`filter_rounds`), so the block packed here is
        // for the same filter the scheduler planned the passes of.
        self.pack_filter_block(layer, || {
            Some(match served_filter(layer, kind).ok()? {
                ServedFilter::Blur(blur) => GpuFilterData::from(GpuGaussianBlur::from(&blur)),
                ServedFilter::DropShadow(shadow) => {
                    GpuFilterData::from(GpuDropShadow::from(&shadow))
                }
            })
        })
    }

    /// The texel offset of `layer`'s parameter block, packing the one `make`
    /// answers on first sight — [`Self::filter_block`]'s bookkeeping, shared
    /// with a backdrop blur, whose block comes from its own record.
    fn pack_filter_block(
        &mut self,
        layer: u32,
        make: impl FnOnce() -> Option<GpuFilterData>,
    ) -> Option<u32> {
        let index = match self.filter_layers.iter().position(|id| *id == layer) {
            Some(index) => index,
            None => {
                let block = make()?;
                self.filter_layers.push(layer);
                self.filter_blocks.push(block);
                self.filter_layers.len().saturating_sub(1)
            }
        };

        u32::try_from(index)
            .ok()?
            .checked_mul(GpuFilterData::SIZE_TEXELS)
    }

    /// Whether this frame records a hole-punch pass at all — at a cut inside a
    /// surface round, or after every round of the frame.
    ///
    /// Derived rather than counted alongside the instances: the two places a
    /// punch can land are the two places its instances are recorded from, and
    /// one answer read off both is one fewer field to keep in step.
    fn punches(&self) -> bool {
        self.punch.1 > 0 || self.rounds.iter().any(|plan| plan.punch.1 > 0)
    }

    /// How many of this frame's rounds render *strips* into a pooled page, and
    /// so how many viewport uniforms of their own the frame needs.
    ///
    /// A filter round targets a page too and is deliberately not counted: it
    /// binds no viewport uniform at all, mapping NDC against the destination
    /// extent its own instance carries.
    fn page_rounds(&self) -> usize {
        self.rounds
            .iter()
            .filter(|plan| {
                (plan.page.is_some() || plan.retained.is_some())
                    && plan.filter.is_none()
                    && plan.copy.is_none()
            })
            .count()
    }

    /// The passes a frame planned like this one records, in order, into
    /// `out` — the pass list of the module doc's *The frame's passes*, stated
    /// over the plan alone so it can be checked without a device.
    ///
    /// `record_frame` is what actually records them, and a debug build holds
    /// every frame it records to this list (`EngineRenderer::check_pass_list`).
    /// The two agree by construction on every shape the walk serves; the
    /// unreachable skips the walk guards against (a page the pool refuses, a
    /// pipeline the frame never resolved) are the only places they could part.
    #[cfg(any(test, debug_assertions))]
    fn plan_passes(&self, shape: &PassShape, out: &mut Vec<FramePass>) {
        out.clear();
        if !shape.merge {
            out.push(FramePass::Clear);
        }
        if !shape.instances {
            if shape.merge {
                out.push(FramePass::Opening);
            }
            return;
        }
        if !shape.merge && shape.opaque {
            out.push(FramePass::Opaque);
        }

        let mut owed = shape.merge;
        for plan in &self.rounds {
            // A copy reads the surface back, so the opening — the clear and
            // the opaque strips — is recorded ahead of it.
            if plan.copy.is_some() {
                if owed {
                    out.push(FramePass::Opening);
                    owed = false;
                }
                out.push(FramePass::Copy);
                continue;
            }
            if plan.filter.is_some() {
                out.push(FramePass::Filter);
                continue;
            }
            if plan.page.is_some() || plan.retained.is_some() {
                out.push(FramePass::Page);
            } else if !plan.segments.is_empty() {
                out.push(if owed {
                    FramePass::Opening
                } else {
                    FramePass::Surface
                });
                owed = false;
            }
            if shape.punch && plan.punch.1 > 0 {
                if owed {
                    out.push(FramePass::Opening);
                    owed = false;
                }
                out.push(FramePass::Punch);
            }
        }
        if owed {
            out.push(FramePass::Opening);
        }
        if shape.punch && self.punch.1 > 0 {
            out.push(FramePass::Punch);
        }
    }

    /// The instance bytes, opaque buffer first, so one vertex buffer serves
    /// every pass and each is a contiguous instance range into it.
    fn instance_bytes(&self) -> (&[u8], &[u8]) {
        (
            bytemuck::cast_slice(&self.opaque),
            bytemuck::cast_slice(&self.alpha),
        )
    }
}

/// Records that the instances from `at` on carry `class`, unless the marks
/// already say so.
///
/// A draw that emitted nothing leaves its mark at the same index as the next
/// draw's, which then overwrites it — and drops it altogether when that leaves
/// two neighbouring marks of one class.
fn mark_class(marks: &mut Vec<(u32, PaintClass)>, at: u32, class: PaintClass) {
    match marks.last_mut() {
        Some(last) if last.1 == class => {}
        Some(last) if last.0 == at => {
            last.1 = class;
            let len = marks.len();
            if len >= 2 && marks[len - 2].1 == class {
                marks.pop();
            }
        }
        _ => marks.push((at, class)),
    }
}

/// The class runs `marks` says cover instances `first..end`, relative to
/// `first` — the per-draw input [`merge_class_runs`] merges.
///
/// Every instance of a planned segment was emitted after its draw's mark, so
/// the mark at or before `first` names its class; a composite or punch
/// instance between two marks is never inside a segment's range.
fn marked_runs(
    marks: &[(u32, PaintClass)],
    first: u32,
    end: u32,
) -> impl Iterator<Item = ClassRun> + '_ {
    let from = marks
        .partition_point(|(at, _)| *at <= first)
        .saturating_sub(1);
    let tail = marks.get(from..).unwrap_or(&[]);
    tail.iter()
        .enumerate()
        .take_while(move |(_, (at, _))| *at < end)
        .filter_map(move |(index, (at, class))| {
            let start = (*at).max(first);
            let stop = tail.get(index + 1).map_or(end, |next| next.0.min(end));
            (stop > start).then(|| ClassRun {
                first: start - first,
                count: stop - start,
                class: *class,
            })
        })
}

/// Whether `runs` tile `instances` front to back and each run's program draws
/// every instance in it, as far as the instance's own flag can tell
/// ([`PaintClass::admits_flag`]) — what the marks must agree with, checked in
/// debug builds.
fn runs_draw(runs: &[ClassRun], instances: &[GpuStrip]) -> bool {
    let mut next = 0_u32;
    for run in runs {
        if run.first != next || run.count == 0 {
            return false;
        }
        let Some(span) = instances.get(run.first as usize..(run.first + run.count) as usize) else {
            return false;
        };
        if !span
            .iter()
            .all(|instance| run.class.admits_flag(instance.paint_and_rect_flag))
        {
            return false;
        }
        next += run.count;
    }
    next as usize == instances.len()
}

/// The column of device space one round's target holds: the origin its
/// instances are shifted to, and the edges they are clipped against.
///
/// A page holds its layer at the page's own origin, and its composite samples
/// exactly `(0, 0)`-to-its-own-extent back out
/// ([`Composite::source`](crate::schedule::Composite::source)). For a layer
/// [banded](crate::schedule::pages::page_bands) into column pages that makes
/// the band's own rectangle two things at once: the shift, and the *clip*. The
/// scheduler replays the layer's whole op list into every band — a band differs
/// only in which page it writes and where that page lands — so this is the site
/// that decides which part of each replayed strip belongs to the band at hand.
/// Emission is where the decision lives because it is the one place that knows
/// both the origin and the width; the alternative, clipping the ops
/// scheduler-side, would have to re-rasterize geometry the scheduler only holds
/// as draw ranges.
///
/// Two rules, and both halves of each matter:
///
/// - a span entirely outside the column contributes **no instance** — shifting
///   it by a saturating subtraction instead would clamp it onto the page's own
///   edge, stretching a span the scene drew elsewhere across content this band
///   really holds;
/// - a span straddling an edge has its geometry **and** its first alpha column
///   ([`GpuStrip::col_idx_or_rect_frac`]) advanced by the same amount, because
///   the shader reads that column unshifted and steps one column per pixel of
///   the instance (`shaders/strip.wgsl`'s `col_offset`): advancing the x
///   without the column would sample another pixel's coverage.
///
/// The frame's own surface is [`ROOT`](Self::ROOT), a window over the whole
/// device grid that shifts nothing and clips nothing. A layer that fits one
/// page is one band covering all of it, and a layer's bounds are the
/// tile-aligned union of its own draws' bounds, so the clip is a no-op for
/// every layer that is not banded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PageWindow {
    /// Left edge of the column in device space — what every instance's `x` is
    /// shifted by, and what one left of it is culled against.
    x0: u16,
    /// Right edge of the column in device space, exclusive.
    x1: u16,
    /// Top edge of the column in device space — what every instance's `y` is
    /// shifted by. A band spans its layer's whole height, so the vertical axis
    /// is shifted and never clipped.
    y0: u16,
}

impl PageWindow {
    /// The whole device grid: the window a surface round carries.
    const ROOT: Self = Self {
        x0: 0,
        x1: u16::MAX,
        y0: 0,
    };

    /// The window `page` renders through — its own tile-aligned bounds, which
    /// are one column band's for a banded layer and the whole layer's for
    /// every other.
    fn of(page: &PageTarget) -> Self {
        Self {
            x0: page.bounds.x0,
            x1: page.bounds.x1,
            y0: page.bounds.y0,
        }
    }

    /// The window a retained page's round renders through: the body's own
    /// tile-aligned bounds, never banded.
    fn of_retained(target: &RetainedTarget) -> Self {
        Self {
            x0: target.bounds.x0,
            x1: target.bounds.x1,
            y0: target.bounds.y0,
        }
    }

    /// The origin instances are shifted by.
    fn origin(self) -> (u16, u16) {
        (self.x0, self.y0)
    }

    /// Clips `span` — a strip instance still carrying its scene coordinates —
    /// to this window and shifts it into the target, answering whether any of
    /// it survived.
    ///
    /// `paint` is the draw's own paint, needed because a left-clipped instance
    /// starts at a different scene position than the one it was rasterized at:
    /// a position-sampled paint (a gradient, an image) is re-evaluated there,
    /// exactly as a gap fill is at its own origin, so the paint keeps landing
    /// where the scene put it rather than being squeezed into the clipped span.
    fn place(self, span: &mut GpuStrip, paint: PackedPaint) -> bool {
        let end = span.x.saturating_add(span.width);
        if span.width == 0 || end <= self.x0 || span.x >= self.x1 {
            return false;
        }

        let cut = self.x0.saturating_sub(span.x);
        if cut > 0 {
            span.x = self.x0;
            span.width = span.width.saturating_sub(cut);
            // The dense part of an instance starts at its left edge, so the
            // columns cut off the geometry are cut off the coverage too.
            let dense = cut.min(span.dense_width_or_rect_height);
            span.dense_width_or_rect_height = span.dense_width_or_rect_height.saturating_sub(dense);
            // A sparse instance names no column at all: the fragment stage
            // reads `col + dense_width` as "does this instance sample
            // coverage", so leaving a non-zero column on one whose dense part
            // is gone would send it to the alpha texture for coverage it never
            // wrote.
            span.col_idx_or_rect_frac = if span.dense_width_or_rect_height == 0 {
                0
            } else {
                span.col_idx_or_rect_frac.saturating_add(u32::from(dense))
            };
            span.payload = paint.payload_at(span.x, span.y);
        }

        // Past the right edge is outside the region the composite samples, so
        // it reaches no pixel of the parent either way; clipping it keeps a
        // band's instances inside the band's own rectangle rather than relying
        // on the page's quantized extent to swallow the overhang.
        let over = end.saturating_sub(self.x1);
        if over > 0 {
            span.width = span.width.saturating_sub(over);
            span.dense_width_or_rect_height = span.dense_width_or_rect_height.min(span.width);
        }

        span.x = span.x.saturating_sub(self.x0);
        span.y = span.y.saturating_sub(self.y0);
        span.width > 0
    }

    /// The rect instance drawing `quad` with `paint`, clipped to this window
    /// and shifted into its target, or `None` when none of it survives.
    ///
    /// The same two rules [`Self::place`] keeps, for a whole rectangle: a quad
    /// outside the column contributes nothing, and one straddling an edge is
    /// trimmed — its atlas texel advanced by exactly the columns cut off its
    /// left edge, so every surviving pixel still samples the texel it did. A
    /// quad's coverage is its rectangle whole (it lands on whole pixels), so
    /// there is no coverage column to advance and the instance carries no edge
    /// fractions at all.
    fn place_quad(self, quad: &GlyphQuad, paint: PackedPaint) -> Option<GpuStrip> {
        let end = quad.x.saturating_add(quad.width);
        if quad.width == 0 || quad.height == 0 || end <= self.x0 || quad.x >= self.x1 {
            return None;
        }
        let cut = self.x0.saturating_sub(quad.x);
        let x = quad.x.max(self.x0);
        let width = end.min(self.x1).saturating_sub(x);
        let instance = GpuStrip::from_rect(
            x.saturating_sub(self.x0),
            quad.y.saturating_sub(self.y0),
            width,
            quad.height,
            0,
            StripDraw {
                payload: pack_u16_pair(quad.atlas_x.saturating_add(cut), quad.atlas_y),
                paint: paint.paint,
                depth_index: paint.depth_index,
            },
        );
        (width > 0).then_some(instance)
    }
}

/// The glyph quad the draw at `depth` is, out of a frame's quads in depth
/// order — `None` only for a frame whose quads disagree with its draws, which
/// then draws nothing for that glyph rather than something wrong.
fn glyph_quad(quads: &[GlyphQuad], depth: u32) -> Option<&GlyphQuad> {
    let found = quads
        .binary_search_by_key(&depth, |quad| quad.depth)
        .ok()
        .and_then(|index| quads.get(index));
    debug_assert!(
        found.is_some(),
        "a glyph-quad draw with no quad at depth {depth}"
    );
    found
}

/// The scheduler roles `plan` gives `frame`'s retained layers, written into
/// `roles` (cleared first). A body the store refused gets none and is
/// scheduled as the ordinary layer it lowered to.
fn retained_roles(frame: &CompiledFrame, plan: &RetainedPlan, roles: &mut RetainedLayers) {
    roles.clear();
    for (index, (bracket, decision)) in frame.retained.iter().zip(&plan.decisions).enumerate() {
        let id = u32::try_from(index).unwrap_or(u32::MAX);
        let role = match *decision {
            RetainedDecision::Record { .. } => RetainedRole::Record { bracket: id },
            RetainedDecision::Draw { dest, origin } => RetainedRole::Draw {
                bracket: id,
                dest,
                origin,
            },
            RetainedDecision::Hidden => RetainedRole::Hidden,
            RetainedDecision::Inline(_) => continue,
        };
        roles.set(bracket.layer, role);
    }
}

/// Which of `frame`'s retained brackets the schedule renders into a retained
/// page — the bodies whose pages the store may commit. A body the store
/// admitted can still render nothing, nested inside a layer that composites
/// nothing.
fn retained_rendered(frame: &CompiledFrame, rounds: &[Round]) -> Vec<bool> {
    let mut rendered = vec![false; frame.retained.len()];
    for round in rounds {
        if let Some(target) = round.retained()
            && let Some(slot) = rendered.get_mut(target.bracket as usize)
        {
            *slot = true;
        }
    }
    rendered
}

/// The single quad that composites a finished page onto `origin`-shifted
/// target.
///
/// A whole-rectangle instance with no fractional edges: a layer's bounds are
/// tile-aligned, so the quad covers whole pixels and the fragment stage leaves
/// its coverage at one. The payload is the page texel the quad's top-left
/// corner samples — the page holds the layer at its own origin, so that is
/// `Composite::source`'s origin and not the layer's device position.
fn composite_instance(composite: &Composite, origin: (u16, u16), depth: u32) -> GpuStrip {
    let bounds = composite.bounds;
    let source = composite.source();

    GpuStrip::from_rect(
        bounds.x0.saturating_sub(origin.0),
        bounds.y0.saturating_sub(origin.1),
        bounds.width(),
        bounds.height(),
        0,
        StripDraw {
            payload: pack_u16_pair(source.x0, source.y0),
            paint: LAYER_PAINT_SOURCE | u32::from(pack_opacity(composite.opacity)),
            depth_index: depth,
        },
    )
}

/// A layer's constant opacity as the eight bits a composite instance carries.
///
/// Clamped rather than refused: the scheduler already admits only opacities
/// strictly between zero and one, and rounding is what keeps a 0.5 layer at
/// exactly the 128 the reference renderer's own packing produces.
fn pack_opacity(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Where a strip instance's payload comes from.
///
/// A solid paint carries its colour there; every other paint reads its colour
/// from a record instead, and spends the payload on the scene position the
/// paint is sampled at — which is why the payload is per instance rather than
/// per draw.
#[derive(Debug, Clone, Copy)]
enum PaintPayload {
    /// A premultiplied RGBA8 colour, the same for every instance of the draw.
    Solid(u32),
    /// The instance's own scene-space origin, packed as a `u16` pair.
    Position,
    /// The page texel under the instance's own origin, for a page holding the
    /// device region starting at `(x, y)` at its own origin: a backdrop blur's
    /// rectangle sampling its blurred page.
    Layer(u16, u16),
}

/// One draw's paint, resolved to the values its instances repeat.
#[derive(Debug, Clone, Copy)]
struct PackedPaint {
    payload: PaintPayload,
    /// The packed paint descriptor, without [`RECT_STRIP_FLAG`](crate::gpu::strips::RECT_STRIP_FLAG).
    paint: u32,
    depth_index: u32,
    /// Whether every pixel this paint produces is opaque, and so whether its
    /// fully-covered spans may take the depth-writing pass.
    opaque: bool,
    /// The external-texture slot this paint's instances have to be drawn with
    /// bound, or `None` for every paint the frame's atlas binding serves.
    ///
    /// What breaks a pass's instances into runs: the strip pipelines have one
    /// external binding, so consecutive instances may share a segment only
    /// while this stays equal (see [`crate::gpu::bindings::ExternalRuns`]).
    external: Option<u32>,
    /// The strip program class this paint's instances are drawn with — from
    /// the resolved record, so an image is told apart by the path it samples
    /// through (see [`PaintClass::of_image`]).
    class: PaintClass,
    /// Whether this is a glyph run's shared record, whose draws are each one
    /// glyph quad ([`crate::compile::GlyphQuad`]) rather than a strip run.
    glyph_quad: bool,
}

impl PackedPaint {
    /// The payload an instance whose scene origin is `(x, y)` carries.
    fn payload_at(self, x: u16, y: u16) -> u32 {
        match self.payload {
            PaintPayload::Solid(rgba) => rgba,
            PaintPayload::Position => pack_u16_pair(x, y),
            PaintPayload::Layer(x0, y0) => {
                pack_u16_pair(x.saturating_sub(x0), y.saturating_sub(y0))
            }
        }
    }

    /// The per-instance values for an instance whose scene origin is `(x, y)`.
    fn values_at(self, x: u16, y: u16) -> StripDraw {
        StripDraw {
            payload: self.payload_at(x, y),
            paint: self.paint,
            depth_index: self.depth_index,
        }
    }
}

/// Two `u16`s in one word, low half first — the packing the shader's
/// `unpack_u16_pair` reverses.
fn pack_u16_pair(x: u16, y: u16) -> u32 {
    u32::from(x) | (u32::from(y) << 16)
}

/// The shader values for `paint`, resolved against the frame's paint slots.
///
/// Answers `None` for an indexed paint the frame could not resolve — one whose
/// entry is missing from `paint_slots` entirely, or one the lowering refused
/// because the engine has nowhere to hold its pixels yet (see the module
/// header). The draw is then skipped rather than stamped in a wrong colour.
fn pack_paint(
    paint: &Paint,
    depth: u32,
    paint_slots: &[Option<ResolvedPaint>],
) -> Option<PackedPaint> {
    match paint {
        Paint::Solid(color) => Some(PackedPaint {
            payload: PaintPayload::Solid(color.as_premul_rgba8().to_u32()),
            paint: SOLID_PAINT,
            depth_index: depth,
            opaque: color.is_opaque(),
            external: None,
            class: PaintClass::SolidCoverage,
            glyph_quad: false,
        }),
        Paint::Indexed(indexed) => {
            let Some(Some(resolved)) = paint_slots.get(indexed.index()) else {
                INDEXED_PAINT_WARNING.call_once(|| {
                    log::warn!(
                        "an indexed paint could not be lowered to a GPU record; those draws are \
                         skipped (logged once)"
                    );
                });
                return None;
            };
            Some(PackedPaint {
                payload: PaintPayload::Position,
                paint: pack_paint_descriptor(resolved.paint_type, resolved.texel_offset),
                depth_index: depth,
                opaque: resolved.opaque,
                external: resolved.external,
                class: resolved.class,
                glyph_quad: resolved.paint_type == PaintType::Glyph,
            })
        }
    }
}

/// One encoded paint after lowering: where its record landed and how a strip
/// instance names it.
#[derive(Debug, Clone, Copy)]
struct ResolvedPaint {
    /// How the fragment shader reads the record.
    paint_type: PaintType,
    /// The texel the record starts at in the encoded-paint texture.
    texel_offset: u32,
    /// Whether every pixel the paint produces is opaque.
    opaque: bool,
    /// The external-texture slot this paint samples, for a paint that reads a
    /// caller-owned texture rather than the atlas (see
    /// [`crate::gpu::bindings`]). `None` — the ordinary case — means the
    /// frame's atlas binding serves it.
    external: Option<u32>,
    /// The strip program class the paint is drawn with ([`class_of_record`]).
    class: PaintClass,
}

/// The strip program class a lowered record is drawn with.
///
/// The paint type decides it for every record but an image, whose record also
/// names the path it samples through: nearest or bilinear from the atlas is
/// [`PaintClass::Image`], bicubic or the external texture
/// [`PaintClass::ImageFiltered`]. The bits are the ones the shader reads —
/// quality in bits 0-1 of `image_params`, the source kind in bit 14.
fn class_of_record(record: &GpuEncodedPaint) -> PaintClass {
    match record {
        GpuEncodedPaint::Image(image) => PaintClass::of_image(
            image.image_params & IMAGE_QUALITY_BITS,
            (image.image_params >> IMAGE_SOURCE_KIND_SHIFT) & 1 == 1,
        ),
        _ => PaintClass::of_flag(pack_paint_descriptor(record.paint_type(), 0)),
    }
}

/// Bits 0-1 of an image record's `image_params`: the sampling quality.
const IMAGE_QUALITY_BITS: u32 = 0b11;

/// The bit of an image record's `image_params` marking an external texture.
const IMAGE_SOURCE_KIND_SHIFT: u32 = 14;

/// One resource texture and the extent it currently holds.
#[derive(Debug)]
struct ResourceTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl ResourceTexture {
    fn new(device: &wgpu::Device, descriptor: &wgpu::TextureDescriptor<'_>) -> Self {
        let texture = device.create_texture(descriptor);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width: descriptor.size.width,
            height: descriptor.size.height,
        }
    }

    /// The whole texture as a copy destination.
    fn copy_target(&self) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &self.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }
    }

    /// The extent one full-texture upload covers.
    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }
}

/// The GPU resources one renderer holds across frames.
#[derive(Debug)]
struct FrameResources {
    alphas: ResourceTexture,
    paints: ResourceTexture,
    gradients: ResourceTexture,
    /// Stand-ins for the strip shader's bindings a given pass has nothing real
    /// for: the layer input (a real page when a composite is being drawn, this
    /// otherwise), the glyph/image atlas array until the first image is
    /// resident, and an externally bound texture, which nothing writes yet.
    /// Every declared binding has to be bound for a pass to validate, whether
    /// or not an instance samples it.
    placeholders: Placeholders,
    /// The linear, clamp-to-edge sampler the image-sampler program family
    /// filters bilinear atlas images through, bound beside the atlas array in
    /// every strip group 1; `None` for the manual family, whose layout
    /// declares no sampler (see [`EngineRenderer::with_image_sampler`]).
    atlas_sampler: Option<wgpu::Sampler>,
    config: wgpu::Buffer,
    /// One viewport uniform per page round of the busiest frame so far.
    ///
    /// A round's NDC mapping is against the extent of the attachment it writes,
    /// and a page's extent is neither the frame's nor the same from one round
    /// to the next, so each needs a buffer of its own — a bind group holds the
    /// whole buffer, not an offset into one. Grown only, like every other
    /// retained resource here.
    page_configs: Vec<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    instance_capacity: u64,
    /// The GPU records this frame's indexed paints resolve against, in
    /// serialization order — which is the order the texel offsets in
    /// `paint_slots` were taken from.
    paints_data: Vec<GpuEncodedPaint>,
    /// One slot per encoded paint of the frame, indexed by
    /// [`Paint::Indexed`](vello_common::paint::Paint::Indexed): `None` for one
    /// the engine could not lower. Kept parallel to the *encoded* paints
    /// rather than compacted to the lowered ones, because a draw names its
    /// paint by the compiler's index.
    paint_slots: Vec<Option<ResolvedPaint>>,
    /// Reusable per-paint ramp residency, filled from a frame's LUT requests
    /// before its paints are lowered.
    paint_ramps: Vec<Option<CachedRamp>>,
    /// Reusable staging for the encoded-paint upload, padded to the texture's
    /// footprint.
    paint_staging: Vec<u8>,
    /// The distinct external textures this frame's paints sample, in the order
    /// they were first named — the slot numbering the pass segments and the
    /// group-1 bind groups both address by.
    external_runs: ExternalRuns,
    /// One set of bind groups per strip render state, built against any one
    /// of that state's class pipelines. Every strip pipeline — each state,
    /// each paint class — is built against one explicit layout
    /// ([`crate::gpu::bindings::strip_pipeline_layout`]), so a state's set binds
    /// under all of its classes; keeping one set per state is what the map
    /// already did when each pipeline derived a layout of its own.
    bind_groups: HashMap<EnginePipeline, StripBindGroups>,
    /// The target format the live bind groups were built against; a change
    /// means different pipelines, so the whole map is dropped rather than
    /// accumulating a set per format ever rendered to.
    bind_group_format: Option<wgpu::TextureFormat>,
    /// The real image atlas array, created lazily by the first frame that
    /// makes an image resident and grown from then on — `None` is exactly
    /// [`Placeholders::atlas_array`]'s domain, a renderer that has never
    /// drawn an image.
    atlas: Option<AtlasArray>,
    /// The atlas rectangle every image the renderer has ever drawn currently
    /// holds, keyed by the stable [`ImageId`] the compiler's own residency
    /// minted it. See [`FrameResources::resolve_paints`] for how this is
    /// kept in step with residency across frames without reaching into the
    /// compiler's own cache.
    image_registry: HashMap<ImageId, ResidentImage>,
    /// How many atlas regions have been declined — a write or clear the array
    /// refused, or one the budget says the array could not hold. Counted rather
    /// than dropped silently, because each one is a region the frame believed
    /// it had filled.
    refused_regions: u64,
}

impl FrameResources {
    fn new(device: &wgpu::Device, dim: u32, image_sampler: bool) -> Self {
        let min = gpu::MIN_RESOURCE_TEXTURE_HEIGHT;
        Self {
            alphas: ResourceTexture::new(device, &gpu::alpha_texture_descriptor(dim, min)),
            paints: ResourceTexture::new(
                device,
                &gpu::paint_texture::encoded_paints_texture_descriptor(dim, min),
            ),
            gradients: ResourceTexture::new(device, &gradient_texture_descriptor(dim, min)),
            placeholders: Placeholders::new(device),
            atlas_sampler: image_sampler.then(|| atlas_sampler(device)),
            config: device.create_buffer(&config_descriptor("frust-engine config uniform")),
            page_configs: Vec::new(),
            instances: None,
            instance_capacity: 0,
            paints_data: Vec::new(),
            paint_slots: Vec::new(),
            paint_ramps: Vec::new(),
            paint_staging: Vec::new(),
            external_runs: ExternalRuns::new(),
            bind_groups: HashMap::new(),
            bind_group_format: None,
            atlas: None,
            image_registry: HashMap::new(),
            refused_regions: 0,
        }
    }

    /// Drops the atlas array, everything recorded about what lives in it, and
    /// the bind groups naming it.
    ///
    /// What a re-budget needs: the rectangles the registry holds were allocated
    /// in a geometry that no longer exists, so keeping any of them would point
    /// a paint at a rectangle of a texture that is gone.
    fn reset_atlas(&mut self) {
        self.atlas = None;
        self.image_registry.clear();
        self.bind_groups.clear();
    }

    /// Count `regions` atlas regions as declined, saying so once.
    ///
    /// Once, not per region: a budget and an array that disagree disagree about
    /// every region, and a per-frame line would bury the fact under itself. The
    /// count on [`EngineRenderer::refused_atlas_regions`] is the measure.
    fn note_refused_regions(&mut self, regions: u64) {
        self.refused_regions = self.refused_regions.saturating_add(regions);
        ATLAS_REFUSAL_WARNING.call_once(|| {
            log::warn!(
                "an atlas region was refused by the image atlas array; those images are skipped \
                 and their uploads re-offered on a later frame (logged once — see \
                 EngineRenderer::refused_atlas_regions for the count)"
            );
        });
    }

    /// Services `frame`'s LUT and image residency, then lowers its encoded
    /// paints into the records the shader samples.
    ///
    /// Leaves `paints_data` holding the lowered records in serialization order
    /// and `paint_slots` naming, per *encoded* paint, the texel its record
    /// starts at — or `None` where the paint could not be lowered.
    ///
    /// A solid-only frame leaves both empty and touches neither the cache nor
    /// the paint texture, so it costs exactly what it did before paints were
    /// wired up. Image residency is still serviced even then, since an image
    /// can be evicted on a frame that draws nothing at all (see
    /// [`Self::update_image_registry`]).
    fn resolve_paints(
        &mut self,
        frame: &CompiledFrame,
        cache: &mut GradientCache,
        budget: AtlasBudget,
        externals: &ExternalTextures,
    ) {
        self.paints_data.clear();
        self.paint_slots.clear();
        self.external_runs.clear();

        // Kept in step every frame, not only when this frame's own paints
        // need it: an image reaped by the compiler's age-based eviction while
        // nothing draws it must still be forgotten here, or a later draw that
        // reuses its freed rectangle's `ImageId` would read the stale entry.
        self.update_image_registry(frame, budget);

        if frame.encoded_paints.is_empty() {
            return;
        }

        // Ramp residency next, for the whole frame: a ramp's offset is only
        // meaningful once the cache has finished baking this frame's misses,
        // and a record built before that would name a ramp that had not been
        // packed yet.
        self.paint_ramps.clear();
        self.paint_ramps.resize(frame.encoded_paints.len(), None);
        for request in &frame.lut_requests {
            let ramp = resolve_lut_request(*request, &frame.encoded_paints, cache);
            if let Some(slot) = self.paint_ramps.get_mut(request.paint_index) {
                *slot = ramp;
            }
        }

        let mut texel_offset = 0;
        // Ascending, like the walk below, so one cursor marks them.
        let mut glyph_records = frame.glyph_run_paints.iter().copied().peekable();
        for (index, paint) in frame.encoded_paints.iter().enumerate() {
            // A glyph run's shared record: lowered like any atlas image, but
            // drawn by glyph quads (see `crate::compile::GlyphQuad`).
            let glyph_record = glyph_records.next_if_eq(&index).is_some();
            // An external texture's slot travels with its record: the record
            // itself only says "sample the external binding", and which
            // texture that binding holds is settled per run when the pass is
            // recorded rather than per paint.
            let mut external = None;
            let lowered = match paint {
                EncodedPaint::Image(image) => image_id(image)
                    .and_then(|id| self.image_registry.get(&id))
                    .and_then(|resident| {
                        lower_encoded_image(image, resident)
                            .map(|record| fit_minified(record, resident))
                    }),
                // A paint naming a texture nothing is bound under lowers to
                // nothing, so its draws are skipped rather than sampling
                // whichever texture the binding happens to hold.
                EncodedPaint::ExternalTexture(entry) => externals
                    .view(entry.texture_id.0)
                    .and_then(|_| self.external_runs.slot_of(entry.texture_id.0))
                    .map(|slot| {
                        external = Some(slot);
                        lower_encoded_external(entry)
                    }),
                _ => {
                    let ramp = self.paint_ramps.get(index).copied().flatten();
                    lower_encoded_paint(paint, ramp)
                }
            };
            let slot = lowered.map(|record| {
                let resolved = ResolvedPaint {
                    paint_type: if glyph_record {
                        PaintType::Glyph
                    } else {
                        record.paint_type()
                    },
                    texel_offset,
                    opaque: !paint.may_have_transparency(),
                    external,
                    class: if glyph_record {
                        PaintClass::Glyph
                    } else {
                        class_of_record(&record)
                    },
                };
                texel_offset += record.texel_len();
                self.paints_data.push(record);
                resolved
            });
            self.paint_slots.push(slot);
        }
    }

    /// Keeps [`Self::image_registry`] in step with the compiler's own image
    /// residency, without reaching into it: the residency's rectangles are
    /// not reachable from here (see [`crate::gpu::paint_texture::lower_encoded_paint`]'s
    /// doc for why), so this reconstructs the same information from what a
    /// compiled frame already reports.
    ///
    /// Two passes, and each reads the frame's plan directly rather than
    /// inferring anything from draw order. First, every region this frame's
    /// residency reaped is forgotten — `frame.image_evictions` names it by
    /// rectangle, and a rectangle uniquely identifies the one image that held it
    /// (padding is always zero in this engine, so the reported and the stored
    /// rectangle are the same value; see [`crate::cache::images`]'s module doc).
    /// Second, every entry of `frame.image_uploads` is registered under the
    /// [`ImageId`] it carries.
    ///
    /// The order matters and the id does. Evictions run first so a same-frame
    /// evict-then-reallocate that reuses a rectangle registers the new tenant
    /// rather than having it removed again. And the upload naming its own id is
    /// what makes this sound under a *re-offered* plan: an upload the previous
    /// frame did not service is reported again alongside no new draw of its own,
    /// so a walk pairing uploads positionally against this frame's encoded
    /// paints would hand a fresh image the stale upload's rectangle.
    ///
    /// A region the atlas array could not hold at `budget`'s geometry and this
    /// frame's depth is counted and left unregistered instead. Its draws are
    /// then skipped, which is the whole point: a registered rectangle nothing
    /// wrote would be sampled as whatever the texture happened to contain.
    fn update_image_registry(&mut self, frame: &CompiledFrame, budget: AtlasBudget) {
        if !frame.image_evictions.is_empty() {
            // A set rather than `Vec::contains`: both sides of this scan scale
            // with content — the registry with how many images and glyph slots
            // are resident, the plan with how hard the atlas is churning — and
            // their product is the frame path's, not a report's.
            let cleared: HashSet<AtlasRegion> = frame.image_evictions.iter().copied().collect();
            self.image_registry
                .retain(|_, resident| !cleared.contains(&resident.region));
        }

        let mut refused = 0_u64;
        for upload in &frame.image_uploads {
            if !budget.contains(upload.region, frame.atlas_layers) {
                refused = refused.saturating_add(1);
                continue;
            }
            self.image_registry.insert(
                upload.id,
                ResidentImage {
                    id: upload.id,
                    region: upload.region,
                    natural: upload.natural,
                    padding: u32::from(ATLAS_PADDING),
                    may_have_transparency: upload.may_have_transparency,
                },
            );
        }
        // The glyph half of the same registry, and the reason it is a second
        // loop rather than a branch inside the first: a glyph slot carries no
        // pixels and is *not* re-offered across frames, because the pixels are
        // produced by the replay pass rather than uploaded from here. Its
        // rectangle is reported by every draw that names it (see
        // [`crate::compile::GlyphSlot`]), so registering it here is what makes
        // a handle `glifo` recycled resolve against its current occupant.
        for slot in &frame.glyph_slots {
            if !budget.contains(slot.region, frame.atlas_layers) {
                refused = refused.saturating_add(1);
                continue;
            }
            self.image_registry.insert(
                slot.id,
                ResidentImage {
                    id: slot.id,
                    region: slot.region,
                    // Never minified: a glyph is rasterized straight into the
                    // rectangle it was allocated, so the natural extent and the
                    // resident one are the same value by construction.
                    natural: slot.region.size,
                    padding: slot.padding,
                    may_have_transparency: true,
                },
            );
        }

        if refused > 0 {
            self.note_refused_regions(refused);
        }
    }

    /// Grows or creates the image atlas array to hold `layers` layers at
    /// `budget`'s per-layer extent, clearing the live bind groups when it
    /// does — a bind group built against the old (or absent) atlas view would
    /// otherwise sample nothing, or a freed texture.
    ///
    /// A frame that has never made an image resident (`layers == 0`) leaves
    /// the atlas unset, so the strip shader's binding stays on
    /// [`Placeholders::atlas_array`] until the first one is.
    ///
    /// Growth submits a maintenance command buffer of its own rather than
    /// recording into the frame's encoder — see [`crate::gpu::atlas`]. So this
    /// must be called after the frame's last fallible step (a refused frame
    /// must submit nothing) and before its atlas writes are issued (the copy has
    /// to precede them, and a submit flushes whatever is already queued).
    fn ensure_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        budget: AtlasBudget,
        layers: u32,
    ) -> bool {
        let grew = match &mut self.atlas {
            None if layers == 0 => false,
            None => {
                self.atlas = Some(AtlasArray::with_layers(
                    device,
                    budget.atlas_size.0,
                    budget.atlas_size.1,
                    layers,
                ));
                true
            }
            Some(atlas) => atlas.ensure_layers(device, queue, layers),
        };
        if grew {
            self.bind_groups.clear();
        }
        grew
    }

    /// Flushes `frame`'s atlas evictions and uploads against the live atlas
    /// array, evictions first — a rectangle this frame's residency freed may
    /// already hold a fresh upload by the time this runs (see
    /// [`crate::cache::images`]'s module doc), so clearing after writing
    /// would erase the image that just moved in.
    ///
    /// Answers whether the whole plan reached the array — which is what tells
    /// the compiler it may stop re-offering it. A region the array declined is
    /// counted (see [`Self::note_refused_regions`]) and the plan stays pending,
    /// so a later frame that has grown the array writes it rather than the
    /// image being lost.
    ///
    /// An absent array with a plan to service is that same disagreement rather
    /// than a quiet no-op: both are driven by the same `frame.atlas_layers`, so
    /// an empty plan and an absent atlas normally agree.
    fn upload_atlas(&mut self, queue: &wgpu::Queue, frame: &CompiledFrame) -> bool {
        let refused = match self.atlas.as_ref() {
            None => (frame.image_evictions.len() + frame.image_uploads.len()) as u64,
            Some(atlas) => {
                let mut refused = 0_u64;
                for region in &frame.image_evictions {
                    if !atlas.clear_region(queue, *region) {
                        refused = refused.saturating_add(1);
                    }
                }
                for upload in &frame.image_uploads {
                    if !atlas.write_region(queue, upload.region, upload.pixels.data_as_u8_slice()) {
                        refused = refused.saturating_add(1);
                    }
                }
                refused
            }
        };

        if refused == 0 {
            return true;
        }
        self.note_refused_regions(refused);
        false
    }

    /// Makes sure there is one viewport uniform per page round of this frame.
    ///
    /// Grown only: the buffers are 32 bytes each and a frame that once needed
    /// four keeps them rather than reallocating on the next frame that does.
    fn ensure_page_configs(&mut self, device: &wgpu::Device, rounds: usize) {
        while self.page_configs.len() < rounds {
            self.page_configs
                .push(device.create_buffer(&config_descriptor("frust-engine page config uniform")));
        }
    }

    /// The texels this frame's encoded paints occupy.
    fn paint_texels(&self) -> u32 {
        self.paints_data
            .iter()
            .map(GpuEncodedPaint::texel_len)
            .sum()
    }

    /// The height the gradient LUT texture must grow to for every ramp the
    /// cache has packed, or `None` when it already fits.
    ///
    /// # Errors
    ///
    /// [`EngineError::PaintCapacity`]: a LUT set past the resource dimension
    /// squared has nowhere to live, and the frame path reports it rather than
    /// asserting.
    fn grown_gradient_height(&self, cache: &GradientCache) -> Result<Option<u32>, EngineError> {
        let width = self.gradients.width;
        let texels = u32::try_from(cache.luts_size() / BYTES_PER_TEXEL as usize)
            .map_err(|_| EngineError::PaintCapacity)?;
        let required = texels.div_ceil(width).max(gpu::MIN_RESOURCE_TEXTURE_HEIGHT);
        if required > width {
            return Err(EngineError::PaintCapacity);
        }
        Ok((required > self.gradients.height).then_some(required))
    }

    fn resize_alphas(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.alphas = ResourceTexture::new(
                device,
                &gpu::alpha_texture_descriptor(self.alphas.width, height),
            );
            self.bind_groups.clear();
        }
    }

    fn resize_paints(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.paints = ResourceTexture::new(
                device,
                &gpu::paint_texture::encoded_paints_texture_descriptor(self.paints.width, height),
            );
            self.bind_groups.clear();
        }
    }

    fn resize_gradients(&mut self, device: &wgpu::Device, height: Option<u32>) {
        if let Some(height) = height {
            self.gradients = ResourceTexture::new(
                device,
                &gradient_texture_descriptor(self.gradients.width, height),
            );
            self.bind_groups.clear();
        }
    }

    /// Uploads the frame's coverage, paint records, colour ramps, atlas
    /// evictions/uploads and config, answering whether the atlas plan was
    /// serviced in full (see [`Self::upload_atlas`]).
    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        frame: &mut CompiledFrame,
        cache: &mut GradientCache,
        size: (u16, u16),
        dim: u32,
    ) -> bool {
        let atlas_serviced = self.upload_atlas(queue, frame);

        let alphas = &self.alphas;
        gpu::with_padded_alphas(
            &mut frame.strips.alphas,
            alphas.width,
            alphas.height,
            |bytes| {
                queue.write_texture(
                    alphas.copy_target(),
                    bytes,
                    resource_layout(gpu::resource_bytes_per_row(alphas.width), alphas.height),
                    alphas.extent(),
                );
            },
        );

        if !self.paints_data.is_empty() {
            let footprint = gpu::resource_texture_bytes(self.paints.width, self.paints.height);
            self.paint_staging
                .resize(usize::try_from(footprint).unwrap_or(usize::MAX), 0);
            // The height was checked to fit before any pass was recorded, so a
            // buffer too short for the records is unreachable; skipping the
            // upload rather than unwrapping keeps the frame path total anyway.
            if GpuEncodedPaint::serialize_to_buffer(&self.paints_data, &mut self.paint_staging)
                .is_ok()
            {
                queue.write_texture(
                    self.paints.copy_target(),
                    &self.paint_staging,
                    resource_layout(
                        gpu::resource_bytes_per_row(self.paints.width),
                        self.paints.height,
                    ),
                    self.paints.extent(),
                );
            }
        }

        if cache.has_changed() {
            let layout = GradientTextureLayout {
                width: self.gradients.width,
                height: self.gradients.height,
            };
            if let Some(upload) = cache.begin_upload(layout) {
                queue.write_texture(
                    self.gradients.copy_target(),
                    &upload,
                    resource_layout(upload.bytes_per_row(), self.gradients.height),
                    self.gradients.extent(),
                );
            }
            cache.mark_synced();
        }

        // The strip program addresses all three resource textures by the
        // config's width shifts alone — the LUT by the paint texture's — so
        // the config built from `dim` is only sound while each of them really
        // is `dim` wide (see `gpu::config`'s module doc). Every resize above
        // grows height only, so this holds by construction; the assertion is
        // what notices a change that breaks it.
        debug_assert!(
            resource_widths_agree(
                dim,
                self.alphas.width,
                self.paints.width,
                self.gradients.width
            ),
            "strip resource textures must all be resource_texture_dim ({dim}) wide: \
             alphas {}, paints {}, gradients {}",
            self.alphas.width,
            self.paints.width,
            self.gradients.width,
        );
        let config = GpuConfig::new(u32::from(size.0), u32::from(size.1), dim, dim);
        queue.write_buffer(&self.config, 0, bytemuck::bytes_of(&config));

        atlas_serviced
    }

    /// Grows the instance buffer if this frame outgrew it, then uploads the
    /// opaque and alpha instances back to back.
    fn upload_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scratch: &Scratch) {
        let (opaque, alpha) = scratch.instance_bytes();
        let required = (opaque.len() + alpha.len()) as u64;
        if required == 0 {
            return;
        }

        let buffer = match &mut self.instances {
            Some(buffer) if self.instance_capacity >= required => buffer,
            slot => {
                let floor = MIN_INSTANCE_CAPACITY * size_of::<GpuStrip>() as u64;
                let capacity = required
                    .checked_next_power_of_two()
                    .unwrap_or(required)
                    .max(floor);
                self.instance_capacity = capacity;
                slot.insert(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frust-engine strip instances"),
                    size: capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }))
            }
        };
        if !opaque.is_empty() {
            queue.write_buffer(buffer, 0, opaque);
        }
        if !alpha.is_empty() {
            queue.write_buffer(buffer, opaque.len() as u64, alpha);
        }
    }

    /// Builds `variant`'s bind groups if it has none, first dropping every
    /// set when the target format changed under them.
    fn ensure_bind_groups(
        &mut self,
        device: &wgpu::Device,
        variant: EnginePipeline,
        pipeline: &wgpu::RenderPipeline,
        format: wgpu::TextureFormat,
    ) {
        if self.bind_group_format != Some(format) {
            self.bind_groups.clear();
            self.bind_group_format = Some(format);
        }
        if self.bind_groups.contains_key(&variant) {
            return;
        }
        let atlas_view = self
            .atlas
            .as_ref()
            .map(AtlasArray::view)
            .unwrap_or(&self.placeholders.atlas_array);
        let groups = StripBindGroups::new(
            device,
            pipeline,
            &self.alphas.view,
            &self.config,
            &self.placeholders,
            atlas_view,
            self.atlas_sampler.as_ref(),
            &self.paints.view,
            &self.gradients.view,
        );
        self.bind_groups.insert(variant, groups);
    }

    /// Builds `variant`'s group 1 for every external texture this frame draws
    /// that it has none for yet.
    ///
    /// Called after [`Self::ensure_bind_groups`] has built the variant's own
    /// set, and only for the variants the frame records with, so a frame that
    /// draws no external texture builds nothing. What is built is retained
    /// alongside the rest of the variant's groups and dropped with them — on a
    /// format change, an atlas growth or a re-budget, all of which change the
    /// atlas view this group also holds.
    ///
    /// A texture with no registered view is skipped rather than substituted:
    /// its paint did not resolve either, so nothing in the frame names its
    /// slot.
    fn ensure_external_groups(
        &mut self,
        device: &wgpu::Device,
        variant: EnginePipeline,
        pipeline: &wgpu::RenderPipeline,
        externals: &ExternalTextures,
    ) {
        if self.external_runs.is_empty() {
            return;
        }
        // Destructured rather than reached through `self`: the group map is
        // borrowed mutably while the atlas view and the placeholders are read.
        let Self {
            bind_groups,
            external_runs,
            atlas,
            placeholders,
            atlas_sampler,
            ..
        } = self;
        let atlas_view = atlas
            .as_ref()
            .map(AtlasArray::view)
            .unwrap_or(&placeholders.atlas_array);
        let Some(groups) = bind_groups.get_mut(&variant) else {
            return;
        };
        for key in external_runs.keys() {
            if groups.externals.contains_key(key) {
                continue;
            }
            let Some(view) = externals.view(*key) else {
                continue;
            };
            groups.externals.insert(
                *key,
                images_bind_group(device, pipeline, atlas_view, view, atlas_sampler.as_ref()),
            );
        }
    }

    /// Drops every bind group naming the texture registered under `key`.
    ///
    /// Called when that registration changes, because a group holds its view
    /// by value: keeping one past a re-bind would go on sampling the texture
    /// the caller replaced, and keeping one past an unbind would hold the
    /// caller's texture alive for as long as this renderer lives.
    fn forget_external(&mut self, key: u64) {
        for groups in self.bind_groups.values_mut() {
            groups.externals.remove(&key);
        }
    }
}

/// Lower one atlas page's recorded commands into the strips that draw it,
/// answering whether the whole page could be expressed.
///
/// The caller-supplied half of the render-to-atlas seam: `gpu::atlas` owns the
/// pass, the orderings and the submit, while turning a command stream into
/// strips is compiler work and stays on this side of the edge — `compile`
/// already depends on `gpu::atlas`, so taking the reverse dependency would make
/// the two mutually recursive.
///
/// The stream is replayed as a scene in *page* space and compiled by
/// `lowering`, which is why that compiler is sized to the page rather than to
/// the surface. Only the four commands an outline glyph produces are lowered;
/// anything else — a clip path, a blend layer, a gradient paint, which is to
/// say every COLR shape — refuses the page whole rather than drawing part of
/// it. An indexed paint coming back out of the compile means the same thing:
/// the atlas pass binds no paint texture, so a record it would have to sample
/// cannot be drawn.
///
/// Refusing a page is a *last* line rather than the design, because refusal
/// cannot be made harmless here: `glifo` clears a recorder's commands whether
/// or not this answered `true`, and it offers no way to withdraw the entries
/// whose pixels those commands were going to be. Nothing that would reach this
/// refusal is therefore admitted to the atlas in the first place — a colour
/// face never takes the atlas route at all (`crate::text::atlas_policy`), so
/// what arrives here is the solid outline stream this lowers.
fn lower_atlas_page(
    recorder: &AtlasCommandRecorder,
    buffers: &mut AtlasPageBuffers,
    lowering: &mut SceneCompiler,
    page: (u16, u16),
) -> bool {
    let mut scene = Scene::new();
    {
        let mut builder = SceneBuilder::new(&mut scene);
        let mut transform = Affine::IDENTITY;
        let mut brush = Brush::Solid(Color::BLACK);

        for command in &recorder.commands {
            match command {
                AtlasCommand::SetTransform(next) => transform = *next,
                AtlasCommand::SetPaint(AtlasPaint::Solid(color)) => brush = Brush::Solid(*color),
                AtlasCommand::FillPath(path) => {
                    builder.push_transform(transform);
                    builder.fill_path((**path).clone(), brush.clone());
                    builder.pop_transform();
                }
                AtlasCommand::FillRect(rect) => {
                    builder.push_transform(transform);
                    builder.fill_rect(*rect, brush.clone());
                    builder.pop_transform();
                }
                _ => return false,
            }
        }
    }

    let Ok(frame) = lowering.compile(&scene, Affine::IDENTITY, page) else {
        return false;
    };

    let strips = frame.strip_buf();
    for draw in frame.draws() {
        let Paint::Solid(color) = &draw.paint else {
            return false;
        };
        let Some(run) = strips.get(draw.strip_range.clone()) else {
            continue;
        };
        push_solid_strips(
            run,
            color.as_premul_rgba8().to_u32(),
            draw.depth,
            &mut buffers.instances,
        );
    }
    buffers.alphas.extend_from_slice(frame.alphas());
    true
}

/// The [`ImageId`] an encoded image paint names, or `None` for the one
/// [`ImageSource`] variant no residency ever mints — the paint carrying its
/// pixels inline as a [`vello_common::pixmap::Pixmap`] rather than through a
/// handle. The compiler's own image encoding always produces the handle form
/// (see [`crate::compile::paint::encode_image`]), so a compiled frame never
/// exercises the `None` arm; it exists because the type itself admits both.
fn image_id(image: &EncodedImage) -> Option<ImageId> {
    match image.source {
        ImageSource::OpaqueId { id, .. } => Some(id),
        ImageSource::Pixmap(_) => None,
    }
}

/// A lowered image record corrected for an atlas rectangle that holds a
/// *minified* copy of the source.
///
/// The record's transform maps a device position back onto the image's own
/// texels, and the compiler composed it against the source's declared extent —
/// it had to, since that is all it knows before residency is consulted. Scaling
/// its output by [`ResidentImage::minify_scale`] retargets it at the smaller
/// rectangle actually uploaded, which is the whole correction a downsampled
/// image needs: the record's `image_size` and `image_offset` already describe
/// the resident rectangle.
///
/// A record stored at full size is returned untouched, which is every image but
/// one larger than an atlas layer.
fn fit_minified(record: GpuEncodedPaint, resident: &ResidentImage) -> GpuEncodedPaint {
    let Some((x, y)) = resident.minify_scale() else {
        return record;
    };
    let GpuEncodedPaint::Image(mut image) = record else {
        return record;
    };

    // `[a, b, c, d, tx, ty]`, mapping `(u, v)` to `(a·u + c·v + tx, b·u + d·v +
    // ty)`: the x row is scaled by one factor and the y row by the other.
    image.transform[0] *= x;
    image.transform[2] *= x;
    image.transform[4] *= x;
    image.transform[1] *= y;
    image.transform[3] *= y;
    image.transform[5] *= y;

    GpuEncodedPaint::Image(image)
}

/// The descriptor every `Config` uniform buffer is created with.
fn config_descriptor(label: &str) -> wgpu::BufferDescriptor<'_> {
    wgpu::BufferDescriptor {
        label: Some(label),
        size: GpuConfig::SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }
}

/// The texel copy layout of a full resource-texture upload.
fn resource_layout(bytes_per_row: u32, rows: u32) -> wgpu::TexelCopyBufferLayout {
    wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(bytes_per_row),
        rows_per_image: Some(rows),
    }
}

/// The descriptor for a gradient LUT texture of `width` x `height` texels.
fn gradient_texture_descriptor(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("frust-engine gradient texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: GradientTextureLayout::FORMAT,
        usage: gpu::RESOURCE_TEXTURE_USAGES,
        view_formats: &[],
    }
}

/// The 1x1 stand-ins for the strip shader's not-yet-written bindings.
#[derive(Debug)]
struct Placeholders {
    layer_input: wgpu::TextureView,
    atlas_array: wgpu::TextureView,
    external: wgpu::TextureView,
}

impl Placeholders {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            layer_input: placeholder_view(device, "frust-engine layer input placeholder", false),
            atlas_array: placeholder_view(device, "frust-engine atlas placeholder", true),
            external: placeholder_view(device, "frust-engine external placeholder", false),
        }
    }
}

/// A 1x1 transparent `Rgba8Unorm` texture's view, as a plain 2D texture or as
/// a 2D array.
///
/// The array variant allocates **two** layers, not one, matching
/// [`gpu::atlas::atlas_texture_descriptor`]'s own floor: wgpu-hal 30.0.1's
/// GLES backend derives the GL target from the descriptor's layer count
/// alone, and a one-layer array descriptor binds as `GL_TEXTURE_2D` rather
/// than `GL_TEXTURE_2D_ARRAY` (see that function's doc comment for the
/// file:line and upstream issue refs). Only layer zero of the two is ever
/// sampled here.
fn placeholder_view(device: &wgpu::Device, label: &str, array: bool) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: if array { 2 } else { 1 },
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(if array {
            wgpu::TextureViewDimension::D2Array
        } else {
            wgpu::TextureViewDimension::D2
        }),
        ..Default::default()
    })
}

/// The four bind groups every strip pass sets.
///
/// The grouping is the shader's, not this module's: coverage plus config plus
/// layer input, atlas array plus external texture, encoded paints, gradient
/// ramps — four groups exactly, which is the downlevel ceiling with no
/// headroom left.
#[derive(Debug)]
struct StripBindGroups {
    resources: wgpu::BindGroup,
    images: wgpu::BindGroup,
    paints: wgpu::BindGroup,
    gradients: wgpu::BindGroup,
    /// Group 1 again, once per externally bound texture this variant has
    /// drawn: the same atlas array beside that texture's view instead of the
    /// placeholder. Keyed by the id a display list names the texture by.
    ///
    /// A second group rather than a fifth: the four-group ceiling has no
    /// headroom (see [`crate::gpu::pipelines`]), so an external texture is
    /// bound by re-setting the group the atlas already occupies — which is why
    /// a pass's instances are split into runs at all.
    externals: HashMap<u64, wgpu::BindGroup>,
}

impl StripBindGroups {
    /// Takes the eight resources one by one rather than a `&FrameResources`
    /// so the caller can build a set while holding the map it lands in
    /// mutably.
    ///
    /// `atlas` is the live [`AtlasArray`] view once the renderer has one, or
    /// [`Placeholders::atlas_array`] until then — the caller picks, since
    /// only it knows which the frame's own `atlas_layers` calls for. `sampler`
    /// is [`FrameResources::atlas_sampler`].
    #[expect(
        clippy::too_many_arguments,
        reason = "one bind-group build's full resource list; a struct would \
                  only rename the same borrows the caller already holds \
                  mutably in `FrameResources`"
    )]
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        alphas: &wgpu::TextureView,
        config: &wgpu::Buffer,
        placeholders: &Placeholders,
        atlas: &wgpu::TextureView,
        sampler: Option<&wgpu::Sampler>,
        paints: &wgpu::TextureView,
        gradients: &wgpu::TextureView,
    ) -> Self {
        let resources_group =
            resources_bind_group(device, pipeline, alphas, config, &placeholders.layer_input);
        let images = images_bind_group(device, pipeline, atlas, &placeholders.external, sampler);
        let paints = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip paints"),
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(paints),
            }],
        });
        let gradients = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine strip gradients"),
            layout: &pipeline.get_bind_group_layout(3),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(gradients),
            }],
        });

        Self {
            resources: resources_group,
            images,
            paints,
            gradients,
            externals: HashMap::new(),
        }
    }

    /// Sets all four groups on `pass`, taking group 0 from `resources` and
    /// group 1 from `images` when a segment names one, rather than from this
    /// set.
    ///
    /// Two of the four vary within a pass. Group 0 carries both the pass's
    /// viewport uniform and the layer input a composite samples, so a page
    /// round and every composite in it substitute their own. Group 1 carries
    /// the external texture a run is drawn with, so a segment that samples one
    /// substitutes the group holding it; every other segment takes this set's
    /// own, which pairs the atlas array with a placeholder nothing reads.
    /// Groups 2-3 are frame-wide and belong to the pipeline variant.
    fn bind_with(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        resources: &wgpu::BindGroup,
        images: Option<&wgpu::BindGroup>,
    ) {
        pass.set_bind_group(0, resources, &[]);
        pass.set_bind_group(1, images.unwrap_or(&self.images), &[]);
        pass.set_bind_group(2, &self.paints, &[]);
        pass.set_bind_group(3, &self.gradients, &[]);
    }
}

/// Group 1 of a strip pass: the image atlas array, the caller-owned texture
/// an external image paint samples, and — in the image-sampler program family
/// only — the sampler the atlas array is filtered through.
///
/// One function for both shapes the group takes — `external` is
/// [`Placeholders::external`] for the frame-wide group, or a registered view
/// for the group a run of external instances is drawn with (see
/// [`crate::gpu::bindings`]). Built against one pipeline's group layout, as
/// [`resources_bind_group`] is; `sampler` is present exactly when that layout
/// declares one, which [`FrameResources::atlas_sampler`] already decides.
fn images_bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    atlas: &wgpu::TextureView,
    external: &wgpu::TextureView,
    sampler: Option<&wgpu::Sampler>,
) -> wgpu::BindGroup {
    let textures = [
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(atlas),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(external),
        },
    ];
    let with_sampler;
    let entries: &[wgpu::BindGroupEntry<'_>] = match sampler {
        Some(sampler) => {
            let [atlas, external] = textures;
            with_sampler = [
                atlas,
                external,
                wgpu::BindGroupEntry {
                    binding: ATLAS_SAMPLER_BINDING,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ];
            &with_sampler
        }
        None => &textures,
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frust-engine strip images"),
        layout: &pipeline.get_bind_group_layout(1),
        entries,
    })
}

/// The sampler the image-sampler program family filters bilinear atlas
/// images through: linear in both directions, clamped to the edge, one mip.
///
/// Clamp-to-edge is a backstop rather than the mechanism: the shader already
/// clamps every sample position to the centres of the image's own edge texels
/// (`shaders/helpers.wgsl`'s `sampled_bilinear`), so the filter never reaches
/// past the image's rectangle in the first place.
fn atlas_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("frust-engine atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}

/// The groups a round's `released` list names, as the per-group flags a
/// [`RoundPlan`] carries.
fn released_groups(released: &[PageParity]) -> [bool; MAX_LIVE_PAGES] {
    let mut groups = [false; MAX_LIVE_PAGES];
    for parity in released {
        if let Some(slot) = groups.get_mut(parity.index()) {
            *slot = true;
        }
    }
    groups
}

/// How a region of a frame's own target is read back into a pooled page —
/// the mechanism every effect sampling what the frame has already painted
/// shares (a backdrop blur today; see [`crate::filters`]' *The backdrop
/// blur*).
///
/// Either way the region reaches the page through one pass of
/// [`EnginePipeline::Copy`] that clears the page and draws the region at its
/// origin: a pooled page is a render target and a sampled texture, never a
/// copy destination, so a texture copy cannot land in one directly. The modes
/// differ only in what that pass reads.
///
/// Decided per frame from the target texture itself, because the caller
/// decides what it renders into: a swapchain texture is `RENDER_ATTACHMENT`
/// only on this tier and is never widened, so `frust-render` renders a frame
/// carrying a backdrop into an intermediate of its own (in the swapchain's own
/// format, sampled and copyable) and presents it with one copy pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionCopyMode {
    /// The target is `COPY_SRC` in the pages' own [`INTERMEDIATE_FORMAT`]: a
    /// `copy_texture_to_texture` lands the region in the renderer's own
    /// staging texture, unconverted, and the copy pass reads it from there —
    /// the route a target that cannot be sampled (a headless readback target)
    /// still has.
    Texture,
    /// The target is `TEXTURE_BINDING`: the copy pass reads the region
    /// straight out of it — any format the target is rendered in, converted on
    /// the way into the page's own.
    Fragment,
    /// The target can be neither copied nor sampled: no region is read back,
    /// and a backdrop blur draws nothing.
    Unavailable,
}

impl RegionCopyMode {
    /// The mode a frame rendering into `target` reads regions back with.
    #[must_use]
    pub fn of(target: &wgpu::Texture) -> Self {
        Self::for_usage(target.usage(), target.format(), target.sample_count())
    }

    /// [`Self::of`] over the three facts it reads, so the decision is
    /// host-testable without a device.
    ///
    /// Sampling is preferred whenever the target allows it — it costs the one
    /// pass alone, where a texture copy costs a transfer as well.
    #[must_use]
    pub fn for_usage(
        usage: wgpu::TextureUsages,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        if sample_count != 1 {
            Self::Unavailable
        } else if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
            Self::Fragment
        } else if format == INTERMEDIATE_FORMAT && usage.contains(wgpu::TextureUsages::COPY_SRC) {
            Self::Texture
        } else {
            Self::Unavailable
        }
    }
}

/// Reports a frame whose backdrop blurs are drawn as nothing because `target`
/// can be neither copied nor sampled, once per process.
fn note_backdrop_unreadable(target: &wgpu::Texture) {
    BACKDROP_UNREADABLE_WARNING.call_once(|| {
        log::warn!(
            "backdrop blur drawn as nothing: the frame's target ({:?}, {:?}) is neither \
             TEXTURE_BINDING nor COPY_SRC in {INTERMEDIATE_FORMAT:?}, so its backdrop cannot be \
             read back (logged once)",
            target.format(),
            target.usage()
        );
    });
}

/// What a frame's region copies are recorded with beyond the copy pipeline:
/// the instance buffer — one 16-byte slot per copy a frame can run
/// ([`crate::filters::MAX_BACKDROPS_PER_FRAME`]), written as each copy is
/// recorded — and, for a [`RegionCopyMode::Texture`] frame, the staging
/// texture a region is transferred into before the copy pass reads it.
#[derive(Debug)]
struct RegionCopyResources {
    instances: wgpu::Buffer,
    /// `COPY_DST | TEXTURE_BINDING` in [`INTERMEDIATE_FORMAT`], grown to the
    /// largest region a frame has copied and kept, like the page pool's
    /// textures, for the renderer's life. Created by the first texture-mode
    /// frame.
    staging: Option<ResourceTexture>,
}

impl RegionCopyResources {
    fn new(device: &wgpu::Device) -> Self {
        let slots = crate::filters::MAX_BACKDROPS_PER_FRAME as u64;
        Self {
            instances: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust-engine region copy instances"),
                size: slots.saturating_mul(COPY_INSTANCE_BYTES),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            staging: None,
        }
    }

    /// Grows the staging texture to hold a `width` x `height` region, if it
    /// does not already.
    fn ensure_staging(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let (held_width, held_height) = self
            .staging
            .as_ref()
            .map_or((0, 0), |staging| (staging.width, staging.height));
        if held_width >= width && held_height >= height {
            return;
        }
        self.staging = Some(ResourceTexture::new(
            device,
            &wgpu::TextureDescriptor {
                label: Some("frust-engine region copy staging"),
                size: wgpu::Extent3d {
                    width: width.max(held_width),
                    height: height.max(held_height),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: INTERMEDIATE_FORMAT,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        ));
    }
}

/// One region copy's full recording state: a region of the frame's own
/// target, read back into a pooled page at the page's origin.
struct RegionCopyPass<'a> {
    mode: RegionCopyMode,
    /// The frame's own target.
    target: &'a wgpu::TextureView,
    /// The page the region lands in.
    page: &'a PooledTexture,
    /// The device region copied — inside the target by construction
    /// ([`crate::filters::backdrop_region`] clamps it).
    source: RectU16,
    /// [`EnginePipeline::Copy`], on every frame that copies.
    pipeline: Option<&'a wgpu::RenderPipeline>,
    /// The copies' instance buffer and staging texture, on the same frames.
    resources: Option<&'a RegionCopyResources>,
    /// This copy's instance slot.
    slot: u32,
}

impl RegionCopyPass<'_> {
    /// Records the copy into `encoder`: on a texture-mode frame the region is
    /// transferred into the staging texture first, then one pass clears the
    /// page and draws the region into it at the page's origin.
    ///
    /// The page holds the region and transparency around it. A copy whose
    /// pipeline, instance buffer or staging texture is missing — which cannot
    /// happen, `encode` resolves all three for every frame that copies — still
    /// clears the page, and the backdrop blurs nothing into nothing: "draw
    /// less, never wrong".
    fn record(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let width = self.source.width();
        let height = self.source.height();
        // The texture the pass reads, and where in it the region starts.
        let source = match (self.mode, self.resources) {
            (RegionCopyMode::Fragment, Some(_)) => {
                Some((self.target, (self.source.x0, self.source.y0)))
            }
            (RegionCopyMode::Texture, Some(resources)) => match resources.staging.as_ref() {
                Some(staging)
                    if staging.width >= u32::from(width) && staging.height >= u32::from(height) =>
                {
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: self.target.texture(),
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: u32::from(self.source.x0),
                                y: u32::from(self.source.y0),
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        staging.copy_target(),
                        wgpu::Extent3d {
                            width: u32::from(width),
                            height: u32::from(height),
                            depth_or_array_layers: 1,
                        },
                    );
                    Some((&staging.view, (0, 0)))
                }
                _ => None,
            },
            _ => None,
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(FramePass::Copy.label()),
            color_attachments: &[Some(color_attachment(
                self.page.view(),
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            ))],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let (Some((view, origin)), Some(pipeline), Some(resources)) =
            (source, self.pipeline, self.resources)
        else {
            return;
        };
        let offset = u64::from(self.slot).saturating_mul(COPY_INSTANCE_BYTES);
        if offset.saturating_add(COPY_INSTANCE_BYTES) > resources.instances.size() {
            return;
        }
        let (page_width, page_height) = self.page.size();
        let instance = [
            pack_u16_pair(0, 0),
            pack_u16_pair(origin.0, origin.1),
            pack_u16_pair(width, height),
            pack_u16_pair(
                u16::try_from(page_width).unwrap_or(u16::MAX),
                u16::try_from(page_height).unwrap_or(u16::MAX),
            ),
        ];
        queue.write_buffer(
            &resources.instances,
            offset,
            bytemuck::cast_slice(&instance),
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-engine region copy source"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        });
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, resources.instances.slice(..));
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(
            0..COPY_QUAD_VERTICES,
            self.slot..self.slot.saturating_add(1),
        );
    }
}

/// Group 0 of a strip pass: the frame's coverage, the pass's own viewport
/// uniform, and the texture a composite reads its finished page from.
///
/// Built against one pipeline's group layout. Every strip pipeline names the
/// one explicit strip layout ([`crate::gpu::bindings::strip_pipeline_layout`]),
/// so the group binds under every strip state and paint class alike — which is
/// what lets one pass switch class pipelines between runs without rebinding.
fn resources_bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    alphas: &wgpu::TextureView,
    config: &wgpu::Buffer,
    layer_input: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frust-engine strip resources"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(alphas),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: config.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(layer_input),
            },
        ],
    })
}

// ---------------------------------------------------------------------------
// Command ranges (render-model Phase 4 overlay spike)
// ---------------------------------------------------------------------------

/// How [`project_command_range`] treats the commands it is asked to carry
/// into a range — the one thing that differs between the renderer a range is
/// projected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeOptions {
    /// Keep a `DrawRetained` inside the range. A renderer with a retained
    /// store behind it keeps them; one without (an overlay renderer built with
    /// retention off, whose compiler refuses a frame carrying one) has them
    /// dropped and counted in [`RangeCut::dropped_draw_retained`] instead.
    pub keep_draw_retained: bool,
}

/// What cutting a command range out of a scene did — the bracket rule of the
/// overlay spike, reported so a caller (and a test) can see what a cut cost.
///
/// See [`project_command_range`] for the rule itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RangeCut {
    /// The first command index the projection carried, after clamping.
    pub start: usize,
    /// One past the last command index the projection carried, after
    /// clamping.
    pub end: usize,
    /// Brackets open at `start` that were reopened ahead of the range.
    pub reopened: usize,
    /// Brackets still open at `end` that were closed after the range.
    pub closed: usize,
    /// `PushRetained` brackets spanning a cut, painted inline (their
    /// presentation scale folded into the body's transforms and their alpha
    /// as a plain layer) rather than recorded as a page.
    pub demoted_retained: usize,
    /// `DrawRetained`s dropped under `keep_draw_retained: false`.
    pub dropped_draw_retained: usize,
    /// Pops inside the range with no open bracket of their own kind — already
    /// ignored by every renderer, so dropped rather than carried.
    pub dropped_pops: usize,
}

/// The bracket families the display list closes with a pop of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BracketKind {
    Clip,
    Layer,
    Retained,
}

/// The bracket kind `command` opens, if it opens one.
fn opens(command: &frust_scene::Command) -> Option<BracketKind> {
    use frust_scene::Command;
    match command {
        Command::PushClip { .. }
        | Command::PushClipRounded { .. }
        | Command::PushClipPath { .. } => Some(BracketKind::Clip),
        Command::PushLayer { .. } => Some(BracketKind::Layer),
        Command::PushRetained { .. } => Some(BracketKind::Retained),
        _ => None,
    }
}

/// The bracket kind `command` closes, if it is a pop.
fn closes(command: &frust_scene::Command) -> Option<BracketKind> {
    use frust_scene::Command;
    match command {
        Command::PopClip => Some(BracketKind::Clip),
        Command::PopLayer => Some(BracketKind::Layer),
        Command::PopRetained => Some(BracketKind::Retained),
        _ => None,
    }
}

/// One bracket the projection holds open, and how it will be closed.
#[derive(Debug, Clone, Copy)]
enum OpenBracket {
    /// A clip or layer carried as recorded; closed with its own pop.
    Plain(BracketKind),
    /// A `PushRetained` carried as recorded; closed with `PopRetained`.
    Retained,
    /// A `PushRetained` spanning a cut, painted inline. `layer` says whether
    /// its alpha opened a layer to close; `prefix` is the correction in force
    /// before it, restored when it closes.
    Demoted { layer: bool, prefix: Affine },
    /// A `PushRetained` nested inside a demoted bracket: its presentation is
    /// ignored (only the outermost bracket's is honoured), so it opened
    /// nothing and closes nothing.
    Absorbed,
}

impl OpenBracket {
    fn kind(self) -> BracketKind {
        match self {
            OpenBracket::Plain(kind) => kind,
            OpenBracket::Retained | OpenBracket::Demoted { .. } | OpenBracket::Absorbed => {
                BracketKind::Retained
            }
        }
    }
}

/// Records `command` into `builder` with `prefix` composed ahead of its own
/// transform — the identity outside a demoted retained bracket, its
/// presentation correction inside one.
///
/// The builder's own transform stack is left as found: every arm pushes the
/// command's transform, records, and pops, so the recorded transform is
/// exactly `prefix * transform`.
fn replay(builder: &mut SceneBuilder<'_>, prefix: Affine, command: &frust_scene::Command) {
    use frust_scene::{Command, PathStyle};
    fn under(
        builder: &mut SceneBuilder<'_>,
        transform: Affine,
        record: impl FnOnce(&mut SceneBuilder<'_>),
    ) {
        builder.push_transform(transform);
        record(builder);
        builder.pop_transform();
    }
    match command {
        Command::FillRect {
            rect,
            brush,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.fill_rect(*rect, brush.clone())
        }),
        Command::RoundedRect {
            rect,
            radii,
            brush,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.fill_rounded_rect_radii(*rect, *radii, brush.clone());
        }),
        Command::Line {
            p0,
            p1,
            width,
            brush,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.stroke_line(*p0, *p1, *width, brush.clone());
        }),
        // A glyph run composes the builder's transform ahead of its own.
        Command::GlyphRun(run) => under(builder, prefix, |b| b.draw_glyph_run(run.clone())),
        Command::PushClip { rect, transform } => {
            under(builder, prefix * *transform, |b| b.push_clip(*rect));
        }
        Command::PushClipRounded {
            rect,
            radii,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.push_clip_rounded_radii(*rect, *radii);
        }),
        Command::PushClipPath { path, transform } => {
            under(builder, prefix * *transform, |b| {
                b.push_clip_path(path.clone())
            });
        }
        Command::PopClip => builder.pop_clip(),
        Command::Image {
            data,
            dest,
            transform,
        } => under(builder, prefix * *transform, |b| b.draw_image(data, *dest)),
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            color,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.draw_blurred_rounded_rect_radii(*rect, *radii, *std_dev, *color);
        }),
        Command::PushLayer {
            rect,
            alpha,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.push_layer(*rect, *alpha)
        }),
        Command::PopLayer => builder.pop_layer(),
        Command::ClearRect { rect, transform } => {
            under(builder, prefix * *transform, |b| b.clear_rect(*rect));
        }
        Command::Path {
            path,
            style,
            brush,
            transform,
        } => under(builder, prefix * *transform, |b| match style {
            PathStyle::Fill => b.fill_path(path.clone(), brush.clone()),
            PathStyle::Stroke { width, dash: None } => {
                b.stroke_path(path.clone(), *width, brush.clone());
            }
            PathStyle::Stroke {
                width,
                dash: Some(dash),
            } => b.stroke_path_dashed(path.clone(), *width, *dash, brush.clone()),
        }),
        Command::ShaderQuad {
            program,
            dest,
            transform,
            time,
        } => under(builder, prefix * *transform, |b| {
            b.draw_shader(program, *dest, *time);
        }),
        Command::PushRetained {
            key,
            rect,
            alpha,
            scale,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.push_retained(*key, *rect, *alpha, *scale);
        }),
        Command::PopRetained => builder.pop_retained(),
        Command::DrawRetained {
            key,
            rect,
            transform,
            alpha,
            scale,
        } => under(builder, prefix * *transform, |b| {
            b.draw_retained(*key, *rect, *alpha, *scale);
        }),
        Command::SceneTexture {
            id,
            dest,
            transform,
        } => {
            under(builder, prefix * *transform, |b| {
                b.scene_texture(*id, *dest)
            });
        }
        Command::BackdropBlur {
            rect,
            radius,
            transform,
        } => under(builder, prefix * *transform, |b| {
            b.backdrop_blur(*rect, *radius)
        }),
    }
}

/// Writes the commands of `scene` in `range` into `out` (reset first) as a
/// self-contained display list, and reports what the cut did — the overlay
/// spike's range projection (render-model Phase 4, decision D10).
///
/// `range` is clamped to the scene: an `end` past the last command stops at
/// it, and a `start` past `end` projects nothing. The projection is a scene,
/// not a compiled-frame filter: the compiled frame's draws are ordered by
/// depth and grouped into rounds, while a command index is the one paint-order
/// position every producer of a cut (a platform view's place in the display
/// list) can name.
///
/// # The bracket rule
///
/// A cut can fall inside a bracket. The rule this spike chose:
///
/// - **Clips and layers spanning a cut are closed at it and reopened across
///   it.** Every clip or layer open at `start` is re-recorded, in its original
///   push order and with its original geometry, ahead of the range; every one
///   still open at `end` is popped after it, innermost first. Both halves of a
///   split clip therefore clip to the same shape, and both halves of a split
///   layer composite at the same alpha — which is exact for a clip, and for a
///   layer differs from the uncut frame only where content on both sides of
///   the cut overlaps inside it (each half is composited on its own).
/// - **A retained bracket spanning a cut is demoted to inline paint on both
///   sides.** A page recorded from half a body would be a stale page the
///   ledger then advertises, so neither side records one: the bracket's
///   presentation scale is folded into every body command's transform (the
///   same conjugation `crate::compile::layers::retained_correction` composes
///   ahead of the frame root) and its alpha below one becomes a plain layer —
///   exactly the lowering `frust_scene` prescribes for a renderer that does
///   not retain. A retained bracket nested inside a demoted one contributes
///   nothing, as a nested bracket's presentation never does. A retained
///   bracket wholly inside the range is carried as recorded.
/// - **A pop with no open bracket of its kind is dropped** (every renderer
///   ignores one anyway), and a `DrawRetained` is dropped when
///   `options.keep_draw_retained` is off.
///
/// Pops match the most recently opened bracket of their own kind, which is
/// the stack discipline of every well-nested display list.
///
/// Every carried command is re-recorded through a [`SceneBuilder`] with its
/// transform composed under the identity (or a demoted bracket's correction),
/// so geometry and paints are carried verbatim; the cost is one clone of each
/// carried command's payload, paid only by a caller that projects.
pub fn project_command_range(
    scene: &Scene,
    range: Range<usize>,
    options: RangeOptions,
    out: &mut Scene,
) -> RangeCut {
    let commands = scene.commands();
    let len = commands.len();
    let end = range.end.min(len);
    let start = range.start.min(end);
    let mut cut = RangeCut {
        start,
        end,
        ..RangeCut::default()
    };

    // Pass one, over the whole scene: where each bracket closes, and which
    // brackets are open at `start` (in push order).
    let mut closed_at: Vec<Option<usize>> = vec![None; len];
    let mut stack: Vec<(BracketKind, usize)> = Vec::new();
    let mut open_at_start: Vec<usize> = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        if index == start {
            open_at_start.extend(stack.iter().map(|&(_, at)| at));
        }
        if let Some(kind) = opens(command) {
            stack.push((kind, index));
        } else if let Some(kind) = closes(command)
            && let Some(position) = stack.iter().rposition(|&(open, _)| open == kind)
        {
            let (_, at) = stack.remove(position);
            closed_at[at] = Some(index);
        }
    }
    if start == len {
        open_at_start.extend(stack.iter().map(|&(_, at)| at));
    }
    // A retained bracket spans a cut when it opened before the range, or is
    // still open where the range ends short of the scene's own end.
    let spans = |at: usize| at < start || (end < len && closed_at[at].is_none_or(|c| c >= end));

    out.reset();
    let mut builder = SceneBuilder::new(out);
    let mut open: Vec<OpenBracket> = Vec::new();
    let mut prefix = Affine::IDENTITY;

    let push = |at: usize,
                builder: &mut SceneBuilder<'_>,
                open: &mut Vec<OpenBracket>,
                prefix: &mut Affine,
                cut: &mut RangeCut| {
        let command = &commands[at];
        match command {
            frust_scene::Command::PushRetained {
                rect,
                alpha,
                scale,
                transform,
                ..
            } => {
                if open
                    .iter()
                    .any(|bracket| matches!(bracket, OpenBracket::Demoted { .. }))
                {
                    open.push(OpenBracket::Absorbed);
                } else if spans(at) {
                    cut.demoted_retained += 1;
                    let correction =
                        crate::compile::layers::retained_correction(*rect, *scale, *transform);
                    let layer = *alpha < 1.0;
                    if layer {
                        let corrected = *prefix * correction * *transform;
                        builder.push_transform(corrected);
                        builder.push_layer(*rect, *alpha);
                        builder.pop_transform();
                    }
                    open.push(OpenBracket::Demoted {
                        layer,
                        prefix: *prefix,
                    });
                    *prefix *= correction;
                } else {
                    replay(builder, *prefix, command);
                    open.push(OpenBracket::Retained);
                }
            }
            _ => {
                if let Some(kind) = opens(command) {
                    replay(builder, *prefix, command);
                    open.push(OpenBracket::Plain(kind));
                }
            }
        }
    };
    let close =
        |bracket: OpenBracket, builder: &mut SceneBuilder<'_>, prefix: &mut Affine| match bracket {
            OpenBracket::Plain(BracketKind::Clip) => builder.pop_clip(),
            OpenBracket::Plain(BracketKind::Layer) => builder.pop_layer(),
            OpenBracket::Plain(BracketKind::Retained) | OpenBracket::Retained => {
                builder.pop_retained();
            }
            OpenBracket::Demoted {
                layer,
                prefix: before,
            } => {
                if layer {
                    builder.pop_layer();
                }
                *prefix = before;
            }
            OpenBracket::Absorbed => {}
        };

    for &at in &open_at_start {
        cut.reopened += 1;
        push(at, &mut builder, &mut open, &mut prefix, &mut cut);
    }
    for (index, command) in commands.iter().enumerate().take(end).skip(start) {
        if opens(command).is_some() {
            push(index, &mut builder, &mut open, &mut prefix, &mut cut);
        } else if let Some(kind) = closes(command) {
            match open.iter().rposition(|bracket| bracket.kind() == kind) {
                Some(position) => {
                    let bracket = open.remove(position);
                    close(bracket, &mut builder, &mut prefix);
                }
                None => cut.dropped_pops += 1,
            }
        } else if matches!(command, frust_scene::Command::DrawRetained { .. })
            && !options.keep_draw_retained
        {
            cut.dropped_draw_retained += 1;
        } else {
            replay(&mut builder, prefix, command);
        }
    }
    while let Some(bracket) = open.pop() {
        cut.closed += 1;
        close(bracket, &mut builder, &mut prefix);
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_gpu::DownlevelProfile;
    use kurbo::Rect;
    use peniko::color::palette::css::{BLUE, RED};
    use std::collections::BTreeMap;
    use vello_common::geometry::RectU16;
    use vello_common::paint::IndexedPaint;

    /// The `frust-perf enc` line is a contract: a capture is graded by
    /// grepping its fields, so the field order and the names are pinned here
    /// rather than only by the formatter. `perf-trace`-only, with the line —
    /// `cargo test -p frust-engine --features perf-trace` is where it runs.
    #[cfg(feature = "perf-trace")]
    #[test]
    fn an_encode_trace_line_reports_every_column_in_order() {
        let mut trace = EncodeTrace {
            frames: 7,
            ..EncodeTrace::default()
        };
        // One row per column, each carrying a distinct value in its own
        // column, so a transposed or dropped field shows up as a wrong number
        // rather than only as a wrong name.
        let mut row = [0_u32; ENCODE_TRACE_ROW];
        for (column, slot) in row.iter_mut().enumerate() {
            *slot = (column as u32 + 1) * 1_000;
        }
        trace.window.push(row);

        let line = trace.line();
        let phases: Vec<String> = ENCODE_TRACE_COLUMNS
            .iter()
            .enumerate()
            .map(|(column, name)| format!("{name}_us={}.0", column + 1))
            .collect();
        // The counts carry no unit suffix and are reported as the raw values
        // the row holds, not scaled to microseconds like the phases above.
        let counts: Vec<String> = ENCODE_TRACE_COUNTS
            .iter()
            .enumerate()
            .map(|(offset, name)| {
                format!(
                    "{name}={}",
                    (ENCODE_TRACE_COLUMNS.len() + offset + 1) * 1_000
                )
            })
            .collect();
        assert_eq!(
            line,
            format!(
                "frust-perf enc n=7 w=1 {} total_p95_us={}.0 {}",
                phases.join(" "),
                ENCODE_TRACE_COLUMNS.len(),
                counts.join(" "),
            ),
        );
    }

    /// The event counts are sums over the window, the level counts medians,
    /// and the pinned order puts them where the contract says.
    #[cfg(feature = "perf-trace")]
    #[test]
    fn an_encode_trace_line_sums_events_and_takes_the_median_of_levels() {
        let mut trace = EncodeTrace::default();
        let base = ENCODE_TRACE_COLUMNS.len();
        for frame in 0..3_u32 {
            let mut row = [0_u32; ENCODE_TRACE_ROW];
            row[base] = 10 + frame; // draws: median 11
            row[base + 5] = 1; // replays: sum 3
            row[base + 6] = 2; // dirty_pages: sum 6
            row[base + 8] = frame; // glyph_misses: sum 3
            row[base + 10] = 4 + frame; // run_keys: median 5
            trace.window.push(row);
        }
        let line = trace.line();
        let tail = line.split(" total_p95_us=").nth(1).unwrap();
        let counts = tail.split_once(' ').unwrap().1;
        assert_eq!(
            counts,
            "draws=11 strips=0 alphas=0 glyph_draws=0 atlas_glyphs=0 replays=3 \
             dirty_pages=6 atlas_refused=0 glyph_misses=3 glyph_evicted=0 run_keys=5"
        );
    }

    /// A window's percentile is nearest-rank over the column it names, so the
    /// value reported is one the window really contains.
    #[cfg(feature = "perf-trace")]
    #[test]
    fn an_encode_trace_percentile_is_nearest_rank_per_column() {
        let mut trace = EncodeTrace::default();
        for value in [4_000_u32, 1_000, 3_000, 2_000] {
            let mut row = [0_u32; ENCODE_TRACE_ROW];
            row[0] = value;
            trace.window.push(row);
        }
        assert_eq!(trace.percentile(0, 50), 2_000);
        assert_eq!(trace.percentile(0, 95), 4_000);
        // An empty window reports zero rather than reaching past its end.
        trace.window.clear();
        assert_eq!(trace.percentile(0, 50), 0);
    }

    /// Nanoseconds round up to a tenth of a microsecond, so a phase that cost
    /// anything at all never reports as free.
    #[cfg(feature = "perf-trace")]
    #[test]
    fn a_span_that_cost_anything_never_formats_as_zero() {
        assert_eq!(format_us(0), "0.0");
        assert_eq!(format_us(1), "0.1");
        assert_eq!(format_us(4_170_000), "4170.0");
    }

    /// The compile phases partition the compile they sit inside, so their sum
    /// is the whole call minus the overhead the renderer measures around it.
    #[test]
    fn compile_phases_sum_without_overflowing() {
        let spans = CompileSpans {
            validate: Duration::from_micros(10),
            prepare: Duration::from_micros(20),
            classify: Duration::from_micros(30),
            admit: Duration::from_micros(40),
            walk: Duration::from_micros(50),
            // A subset of `walk`, so the sum must not grow by it.
            glyphs: Duration::from_micros(45),
            finish: Duration::from_micros(60),
        };
        assert_eq!(spans.total(), Duration::from_micros(210));
        assert_eq!(
            CompileSpans {
                validate: Duration::MAX,
                walk: Duration::MAX,
                ..CompileSpans::default()
            }
            .total(),
            Duration::MAX,
        );
    }

    #[test]
    fn a_target_past_the_u16_grid_is_refused() {
        assert_eq!(
            grid_size(1920, 1080).expect("a normal surface"),
            (1920, 1080)
        );
        assert_eq!(
            grid_size(u32::from(u16::MAX), 16).expect("the grid ceiling itself"),
            (u16::MAX, 16)
        );
        assert!(matches!(
            grid_size(u32::from(u16::MAX) + 1, 16),
            Err(EngineError::TargetTooLarge)
        ));
        assert!(matches!(
            grid_size(16, u32::from(u16::MAX) + 1),
            Err(EngineError::TargetTooLarge)
        ));
    }

    #[test]
    fn a_premultiplied_clear_folds_alpha_into_the_colour() {
        let half = RED.with_alpha(0.5);
        let premultiplied = clear_color(half, OutputAlpha::Premultiplied);
        let straight = clear_color(half, OutputAlpha::Straight);

        assert!((premultiplied.r - 0.5).abs() < 1e-6);
        assert!((premultiplied.a - 0.5).abs() < 1e-6);
        assert!((straight.r - 1.0).abs() < 1e-6);
        assert!((straight.a - 0.5).abs() < 1e-6);

        // An opaque colour is identical under both conventions.
        assert_eq!(
            clear_color(BLUE, OutputAlpha::Premultiplied),
            clear_color(BLUE, OutputAlpha::Straight)
        );
    }

    #[test]
    fn a_solid_paint_travels_premultiplied_in_the_instance_payload() {
        let paint = pack_paint(&Paint::from(RED), 3, &[]).expect("a solid paint always resolves");
        let values = paint.values_at(40, 12);
        assert_eq!(values.paint, SOLID_PAINT);
        assert_eq!(values.depth_index, 3);
        assert_eq!(values.payload, RED.premultiply().to_rgba8().to_u32());
        assert!(paint.opaque);
        assert_eq!(
            paint.values_at(0, 0).payload,
            values.payload,
            "a solid colour is the same wherever it is stamped"
        );

        let paint = pack_paint(&Paint::from(RED.with_alpha(0.5)), 0, &[])
            .expect("a translucent solid paint still resolves");
        assert!(
            !paint.opaque,
            "a translucent paint never reaches the opaque pass"
        );
    }

    #[test]
    fn an_unresolvable_indexed_paint_skips_its_draw() {
        let indexed = Paint::Indexed(IndexedPaint::new(0));
        assert!(
            pack_paint(&indexed, 0, &[]).is_none(),
            "an index past the frame's slots resolves to nothing"
        );
        assert!(
            pack_paint(&indexed, 0, &[None]).is_none(),
            "so does a slot the lowering refused"
        );
    }

    /// An image record is classed by the path it samples through — the one
    /// split an instance's flag cannot make — and every other record by its
    /// paint type.
    #[test]
    fn a_lowered_record_names_its_strip_program_class() {
        use crate::gpu::atlas::pack_image_params;
        use crate::gpu::paint_texture::GpuEncodedImage;

        let image = |quality: u32, external: bool| {
            GpuEncodedPaint::Image(GpuEncodedImage {
                image_params: pack_image_params(quality, 0, 0, 3, external),
                image_size: 0,
                image_offset: 0,
                transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                tint: 0,
                tint_mode: 0,
                image_padding: 0,
            })
        };
        assert_eq!(class_of_record(&image(0, false)), PaintClass::Image);
        assert_eq!(class_of_record(&image(1, false)), PaintClass::Image);
        assert_eq!(class_of_record(&image(2, false)), PaintClass::ImageFiltered);
        assert_eq!(class_of_record(&image(1, true)), PaintClass::ImageFiltered);
    }

    #[test]
    fn a_resolved_indexed_paint_samples_at_each_instances_own_position() {
        let slots = [Some(ResolvedPaint {
            paint_type: PaintType::LinearGradient,
            texel_offset: 6,
            opaque: true,
            external: None,
            class: PaintClass::Gradient,
        })];
        let paint = pack_paint(&Paint::Indexed(IndexedPaint::new(0)), 2, &slots)
            .expect("a lowered paint resolves");

        assert_eq!(
            paint.paint,
            pack_paint_descriptor(PaintType::LinearGradient, 6)
        );
        assert_eq!(paint.depth_index, 2);
        // The payload is the instance's scene origin, not a colour, so two
        // instances of the same draw carry different payloads.
        assert_eq!(paint.payload_at(8, 4), 8 | (4 << 16));
        assert_eq!(paint.payload_at(40, 4), 40 | (4 << 16));
    }

    #[test]
    fn a_u16_pair_packs_low_half_first() {
        assert_eq!(pack_u16_pair(0, 0), 0);
        assert_eq!(pack_u16_pair(1, 0), 1);
        assert_eq!(pack_u16_pair(0, 1), 1 << 16);
        assert_eq!(pack_u16_pair(u16::MAX, u16::MAX), u32::MAX);
    }

    #[test]
    fn the_minimum_resource_dimension_keeps_every_upload_row_legal() {
        // The gradient LUT is the narrowest resource at four bytes per texel,
        // so it is the one that fixes the floor.
        assert_eq!(
            MIN_RESOURCE_TEXTURE_DIM * BYTES_PER_TEXEL,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
        );
        assert!(MIN_RESOURCE_TEXTURE_DIM.is_power_of_two());
    }

    // -----------------------------------------------------------------
    // Where the hole punch lands in the pass plan
    //
    // What a frame's pass plan COSTS, and where the erase sits inside it,
    // is a decision over the recording's shape — no device, no pixels.
    // The pixel half of the same contract is
    // `frust-testing`'s `tests/aa_over_punch.rs`, against `vello_cpu`.
    // -----------------------------------------------------------------

    /// Viewport every plan below is built against.
    const PLAN_VIEWPORT: (u16, u16) = (64, 48);

    /// The slot, and a chip straddling its right edge — so a chip drawn over
    /// the punch is half inside it and half over the backdrop.
    const PLAN_SLOT: Rect = Rect::new(4.0, 4.0, 32.0, 44.0);
    const PLAN_CHIP: Rect = Rect::new(16.0, 12.0, 56.0, 32.0);

    /// The plan `Scratch::build` produces for `scene`, alongside the rounds it
    /// was scheduled from, under a target that carries alpha.
    fn plan_of(scene: &Scene) -> (Scratch, Vec<Round>) {
        let frame = SceneCompiler::new(PLAN_VIEWPORT.0, PLAN_VIEWPORT.1)
            .compile(scene, Affine::IDENTITY, PLAN_VIEWPORT)
            .expect("an in-range scene compiles");
        let rounds = Schedule::build(
            &frame.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            &PageConfig::default(),
        )
        .expect("a scene of solid fills schedules");

        let mut scratch = Scratch::default();
        // Depth on and punching on: the shape a translucent presentation
        // carrying a platform-view slot is planned under.
        scratch.build(&frame, &rounds, true, !frame.clears.is_empty(), &[], &[]);
        (scratch, rounds)
    }

    /// A scene recorded through the public builder.
    fn plan_scene(record: impl FnOnce(&mut SceneBuilder<'_>)) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        record(&mut builder);
        scene
    }

    fn plan_backdrop(builder: &mut SceneBuilder<'_>) {
        builder.fill_rect(
            Rect::new(
                0.0,
                0.0,
                f64::from(PLAN_VIEWPORT.0),
                f64::from(PLAN_VIEWPORT.1),
            ),
            Brush::Solid(RED),
        );
    }

    #[test]
    fn a_frame_that_punches_nothing_is_planned_exactly_as_its_rounds() {
        let (scratch, rounds) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE));
        }));

        assert_eq!(
            scratch.rounds.len(),
            rounds.len(),
            "a frame with no clear is cut nowhere, so it costs exactly its \
             scheduled rounds' passes"
        );
        assert!(!scratch.punches(), "and records no punch pass at all");
    }

    #[test]
    fn a_clear_recorded_last_keeps_its_pass_after_every_round() {
        let (scratch, rounds) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.clear_rect(PLAN_SLOT);
        }));

        assert_eq!(
            scratch.rounds.len(),
            rounds.len(),
            "nothing is recorded after the clear, so nothing is cut"
        );
        assert!(
            scratch.rounds.iter().all(|plan| plan.punch.1 == 0),
            "no round carries the punch"
        );
        assert!(
            scratch.punch.1 > 0,
            "it is issued after the last round instead — the position an \
             unconditionally-trailing pass would have put it in, which is why \
             a frame shaped like this renders byte-identically"
        );
    }

    #[test]
    fn a_draw_over_a_clear_cuts_the_surface_round_and_the_punch_goes_in_the_cut() {
        let (scratch, rounds) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.clear_rect(PLAN_SLOT);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE));
        }));

        assert_eq!(
            scratch.rounds.len(),
            rounds.len() + 1,
            "the surface round is cut in two — one extra pass, and one only"
        );
        assert_eq!(
            scratch.punch,
            (0, 0),
            "with nothing left over for a trailing pass"
        );

        let cut = scratch
            .rounds
            .iter()
            .position(|plan| plan.punch.1 > 0)
            .expect("the cut plan carries the punch");
        assert_eq!(cut, scratch.rounds.len() - 2, "and it is the cut plan");

        // The buffer says the same thing the plan does: the backdrop's
        // instances precede the punch's, and the chip's follow them.
        let before = scratch.rounds[cut].segments.clone();
        let after = scratch.rounds[cut + 1].segments.clone();
        let end_of = |range: Range<usize>| {
            scratch.segments[range]
                .iter()
                .map(|segment| match *segment {
                    Segment::Strips(first, count, _)
                    | Segment::External(first, count, _, _)
                    | Segment::Backdrop(first, count, _) => first + count,
                    Segment::Composite(first, _) => first + 1,
                })
                .max()
                .expect("a plan of this frame draws something")
        };
        let (punch_first, punch_count) = scratch.rounds[cut].punch;
        assert!(
            end_of(before) <= punch_first,
            "everything recorded before the clear is erased by the punch"
        );
        assert!(
            scratch.segments[after]
                .iter()
                .all(|segment| match *segment {
                    Segment::Strips(first, _, _)
                    | Segment::External(first, _, _, _)
                    | Segment::Backdrop(first, _, _)
                    | Segment::Composite(first, _) => first >= punch_first + punch_count,
                }),
            "and everything recorded after it lands on top of the erase"
        );
    }

    // -----------------------------------------------------------------
    // The pass list: what a frame of each shape records, merged and apart
    //
    // Host-only: `Scratch::plan_passes` states the list over the plan, and
    // a debug build holds every frame `record_frame` really records to it,
    // so the device suites (`tests/encode_contract.rs` among them) check the
    // same list against a real encoder. The filter shape is pinned only here:
    // no `Scene` command records a filter layer, so no `encode` call reaches
    // a filter round.
    // -----------------------------------------------------------------

    /// A rectangle off the pixel grid, so its edges are anti-aliased and the
    /// draw always reaches a surface round's blended strips — never only the
    /// opaque ones.
    const PLAN_SOFT: Rect = Rect::new(8.5, 8.5, 24.5, 40.5);

    /// The passes `scratch` records with the opening merged, and apart.
    fn pass_lists(scratch: &Scratch) -> (Vec<FramePass>, Vec<FramePass>) {
        let shape = |merge| PassShape {
            merge,
            instances: true,
            opaque: !scratch.opaque.is_empty(),
            punch: scratch.punches(),
        };
        let mut merged = Vec::new();
        scratch.plan_passes(&shape(true), &mut merged);
        let mut apart = Vec::new();
        scratch.plan_passes(&shape(false), &mut apart);
        assert_opened_once(&merged);
        assert!(
            !apart.contains(&FramePass::Opening),
            "an unmerged frame records no opening: {apart:?}"
        );
        (merged, apart)
    }

    /// A merged list opens its surface exactly once, before every other pass
    /// over it, and never records the clear or the opaque strips apart.
    fn assert_opened_once(passes: &[FramePass]) {
        let openings = passes
            .iter()
            .filter(|pass| **pass == FramePass::Opening)
            .count();
        assert_eq!(openings, 1, "one opening per frame: {passes:?}");
        let first_surface = passes.iter().position(|pass| pass.targets_surface());
        assert_eq!(
            first_surface,
            passes.iter().position(|pass| *pass == FramePass::Opening),
            "the opening is the first pass over the surface: {passes:?}"
        );
        assert!(
            !passes.contains(&FramePass::Clear) && !passes.contains(&FramePass::Opaque),
            "and the clear and opaque strips live inside it: {passes:?}"
        );
    }

    #[test]
    fn an_empty_frame_is_the_opening_alone() {
        let (scratch, _) = plan_of(&plan_scene(|_| {}));
        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(merged, [FramePass::Opening]);
        assert_eq!(apart, [FramePass::Clear]);

        // And the same before any instance buffer exists at all.
        for merge in [true, false] {
            let mut passes = Vec::new();
            scratch.plan_passes(
                &PassShape {
                    merge,
                    instances: false,
                    opaque: false,
                    punch: false,
                },
                &mut passes,
            );
            assert_eq!(passes.len(), 1, "one clean clear: {passes:?}");
        }
    }

    #[test]
    fn a_surface_only_frame_folds_its_clear_and_opaque_strips_into_one_pass() {
        let (scratch, _) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.fill_rect(PLAN_SOFT, Brush::Solid(BLUE));
        }));
        assert!(!scratch.opaque.is_empty(), "the backdrop is opaque strips");

        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(merged, [FramePass::Opening]);
        assert_eq!(
            apart,
            [FramePass::Clear, FramePass::Opaque, FramePass::Surface]
        );
    }

    #[test]
    fn without_depth_the_opening_is_the_clear_and_the_surface_round() {
        let scene = plan_scene(|builder| {
            plan_backdrop(builder);
            builder.fill_rect(PLAN_SOFT, Brush::Solid(BLUE));
        });
        let frame = SceneCompiler::new(PLAN_VIEWPORT.0, PLAN_VIEWPORT.1)
            .compile(&scene, Affine::IDENTITY, PLAN_VIEWPORT)
            .expect("an in-range scene compiles");
        let rounds = Schedule::build(
            &frame.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            &PageConfig::default(),
        )
        .expect("a scene of solid fills schedules");
        let mut scratch = Scratch::default();
        scratch.build(&frame, &rounds, false, false, &[], &[]);
        assert!(scratch.opaque.is_empty(), "no depth, no opaque strips");

        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(merged, [FramePass::Opening]);
        assert_eq!(apart, [FramePass::Clear, FramePass::Surface]);
    }

    #[test]
    fn a_page_round_waits_for_the_opening_rather_than_splitting_it() {
        let (scratch, _) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.push_layer(PLAN_CHIP, 0.5);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE));
            builder.pop_layer();
        }));
        let (merged, apart) = pass_lists(&scratch);
        // The page is rendered first either way — a layer round writes a
        // pooled page and reads neither the surface nor its depth — and the
        // surface round that composites it carries the opening.
        assert_eq!(merged, [FramePass::Page, FramePass::Opening]);
        assert_eq!(
            apart,
            [
                FramePass::Clear,
                FramePass::Opaque,
                FramePass::Page,
                FramePass::Surface
            ]
        );
    }

    #[test]
    fn a_punch_cut_ends_the_opening_where_the_first_surface_pass_ended() {
        let (scratch, _) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.fill_rect(PLAN_SOFT, Brush::Solid(BLUE));
            builder.clear_rect(PLAN_SLOT);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE.with_alpha(0.5)));
        }));
        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(
            merged,
            [FramePass::Opening, FramePass::Punch, FramePass::Surface]
        );
        assert_eq!(
            apart,
            [
                FramePass::Clear,
                FramePass::Opaque,
                FramePass::Surface,
                FramePass::Punch,
                FramePass::Surface
            ]
        );
    }

    #[test]
    fn a_punch_ahead_of_every_surface_draw_takes_the_opening_alone() {
        // Nothing is recorded before the clear, so the plan the punch cuts
        // draws nothing: the opening — the backdrop's opaque strips inside it —
        // is recorded ahead of the erase on its own, and everything blended
        // after the cut loads.
        let (scratch, _) = plan_of(&plan_scene(|builder| {
            builder.clear_rect(PLAN_SLOT);
            plan_backdrop(builder);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE.with_alpha(0.5)));
        }));
        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(
            merged,
            [FramePass::Opening, FramePass::Punch, FramePass::Surface]
        );
        assert_eq!(
            apart,
            [
                FramePass::Clear,
                FramePass::Opaque,
                FramePass::Punch,
                FramePass::Surface
            ]
        );
    }

    #[test]
    fn a_punch_recorded_last_follows_the_opening() {
        let (scratch, _) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.fill_rect(PLAN_SOFT, Brush::Solid(BLUE));
            builder.clear_rect(PLAN_SLOT);
        }));
        let (merged, apart) = pass_lists(&scratch);
        assert_eq!(merged, [FramePass::Opening, FramePass::Punch]);
        assert_eq!(
            apart,
            [
                FramePass::Clear,
                FramePass::Opaque,
                FramePass::Surface,
                FramePass::Punch
            ]
        );
    }

    #[test]
    fn a_filter_frame_records_its_filter_passes_before_the_opening() {
        use crate::filters::{LayerFilter, push_filter_layer};
        use vello_common::peniko::BlendMode;
        use vello_common::record::{CommandRecorder, LayerProps};

        // Compiled as a backdrop and a chip, then re-recorded with the chip
        // inside a blurred layer under the surface: no `Scene` command
        // records a filter layer, so this is the only way to plan one.
        let mut frame = SceneCompiler::new(PLAN_VIEWPORT.0, PLAN_VIEWPORT.1)
            .compile(
                &plan_scene(|builder| {
                    plan_backdrop(builder);
                    builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE));
                }),
                Affine::IDENTITY,
                PLAN_VIEWPORT,
            )
            .expect("an in-range scene compiles");
        let draws = frame.recorder.draws.clone();
        assert_eq!(draws.len(), 2, "the backdrop and the chip");
        let mut recorder = CommandRecorder::new(PLAN_VIEWPORT.0, PLAN_VIEWPORT.1);
        let strips_of = |draw: &crate::EngineDraw| &frame.strip_buf()[draw.strip_range.clone()];
        recorder.push_draw(draws[0].clone(), strips_of(&draws[0]));
        push_filter_layer(
            &mut recorder,
            LayerProps {
                blend_mode: BlendMode::default(),
                opacity: 1.0,
                mask: None,
                clip_path: None,
            },
            LayerFilter::Blur { sigma: 2.0 },
            Affine::IDENTITY,
        )
        .expect("a finite σ under the ceiling records");
        recorder.push_draw(draws[1].clone(), strips_of(&draws[1]));
        recorder.pop_layer();
        frame.recorder = recorder;

        let rounds = Schedule::build(
            &frame.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            &PageConfig::default(),
        )
        .expect("a blurred layer under the surface schedules");
        let mut scratch = Scratch::default();
        scratch.build(&frame, &rounds, true, false, &[], &[]);
        assert!(scratch.filter_passes > 0, "the layer is filtered");

        let (merged, apart) = pass_lists(&scratch);
        let filters = |passes: &[FramePass]| {
            passes
                .iter()
                .filter(|pass| **pass == FramePass::Filter)
                .count()
        };
        assert_eq!(filters(&merged), scratch.filter_passes as usize);
        assert_eq!(filters(&apart), filters(&merged));
        assert_eq!(
            merged.len() + 2,
            apart.len(),
            "the clear and the opaque strips fold into the surface round: \
             {merged:?} against {apart:?}"
        );
        assert_eq!(merged.first(), Some(&FramePass::Page));
        assert_eq!(
            merged.last(),
            Some(&FramePass::Opening),
            "every filter pass runs before the surface opens: {merged:?}"
        );
    }

    // -----------------------------------------------------------------
    // A banded layer's column pages hold their own column, and only it
    //
    // The scheduler hands every band of a layer the same op list, so the
    // instances a band emits are where a column split is made or lost. Every
    // case here is host-only: the plan is built with no device, and the pixel
    // half of the same contract is `tests/desktop_stress.rs`'s 5K cases.
    // -----------------------------------------------------------------

    /// Viewport the band plans below are built against: wide enough that one
    /// layer over all of it needs several column pages under [`BAND_PAGES`],
    /// and one tile row tall, so a draw costs one strip row per column.
    const BAND_VIEWPORT: (u16, u16) = (200, 8);

    /// A page ceiling small enough to band a test-sized layer, so no case here
    /// has to allocate — or even name — a 5K one.
    const BAND_PAGES: PageConfig = PageConfig {
        min_page_size: 64,
        max_page_size: 64,
    };

    /// The plan `scene` produces under `config`, alongside the rounds it was
    /// scheduled into.
    ///
    /// Depth is off, so every instance of the frame lands in the one blended
    /// buffer in painter order and a case can read the whole plan out of it;
    /// the split into the depth-writing pass is a surface-round decision and a
    /// page round never takes it.
    fn band_plan_of(scene: &Scene, config: &PageConfig) -> (Scratch, Vec<Round>) {
        let frame = SceneCompiler::new(BAND_VIEWPORT.0, BAND_VIEWPORT.1)
            .compile(scene, Affine::IDENTITY, BAND_VIEWPORT)
            .expect("an in-range scene compiles");
        let rounds = Schedule::build(
            &frame.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            config,
        )
        .expect("a layer wider than the ceiling bands rather than refusing");

        let mut scratch = Scratch::default();
        scratch.build(&frame, &rounds, false, false, &[], &[]);
        assert_eq!(
            scratch.rounds.len(),
            rounds.len(),
            "a frame with no clear is cut nowhere, so the plans and the rounds \
             line up one for one"
        );
        (scratch, rounds)
    }

    /// A layer at half opacity — so it cannot be inlined and has to take a page
    /// — holding one solid rectangle per entry of `rects`.
    fn band_scene(rects: &[(Rect, Color)]) -> Scene {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_layer(
            Rect::new(
                0.0,
                0.0,
                f64::from(BAND_VIEWPORT.0),
                f64::from(BAND_VIEWPORT.1),
            ),
            0.5,
        );
        for (rect, color) in rects {
            builder.fill_rect(*rect, Brush::Solid(*color));
        }
        builder.pop_layer();
        scene
    }

    /// The bounds of every page round, in order — one band's column each.
    fn band_bounds(rounds: &[Round]) -> Vec<RectU16> {
        rounds
            .iter()
            .filter_map(|round| round.page().map(|page| page.bounds))
            .collect()
    }

    /// The strip instances one round plan issues, in execution order.
    fn plan_strips(scratch: &Scratch, plan: &RoundPlan) -> Vec<GpuStrip> {
        scratch.segments[plan.segments.clone()]
            .iter()
            .filter_map(|segment| match *segment {
                Segment::Strips(first, count, _)
                | Segment::External(first, count, _, _)
                | Segment::Backdrop(first, count, _) => Some((first as usize, count as usize)),
                Segment::Composite(..) => None,
            })
            .flat_map(|(first, count)| scratch.alpha[first..first + count].iter().copied())
            .collect()
    }

    /// Every pixel column the frame's *page* rounds paint, mapped back out of
    /// the pages they were shifted into, and valued by what the shader reads
    /// there: the instance's paint payload, plus the exact alpha column that
    /// pixel samples (`None` for a sparse instance, which samples none).
    ///
    /// Keyed by the draw's own depth as well as the position, so a page painted
    /// by two different draws is not conflated — and so a *second* instance of
    /// one draw covering a pixel it already covered, which is precisely what a
    /// clamped band replay produces, is caught here rather than silently
    /// overwriting the first.
    fn painted_pixels(
        scratch: &Scratch,
        rounds: &[Round],
    ) -> BTreeMap<(u32, u16, u16), (u32, Option<u32>)> {
        let mut painted = BTreeMap::new();

        for (plan, round) in scratch.rounds.iter().zip(rounds) {
            let Some(page) = round.page() else {
                continue;
            };
            for span in plan_strips(scratch, plan) {
                for offset in 0..span.width {
                    let x = page.bounds.x0 + span.x + offset;
                    let y = page.bounds.y0 + span.y;
                    let column = (offset < span.dense_width_or_rect_height)
                        .then(|| span.col_idx_or_rect_frac + u32::from(offset));
                    assert!(
                        painted
                            .insert((span.depth_index, y, x), (span.payload, column))
                            .is_none(),
                        "one draw covers a device pixel at most once, however the \
                         layer holding it was split"
                    );
                }
            }
        }

        painted
    }

    #[test]
    fn a_banded_layer_paints_exactly_what_one_page_would_have() {
        // The whole point of the split, asserted as an equality rather than as
        // a rectangle property: the same scene planned onto one page and onto
        // column bands has to paint the same device pixels, from the same
        // paints, sampling the same alpha columns. A band replay that clamped
        // a strip left of its own column onto the page's edge fails here twice
        // over — once on the ghost pixel, once on the column it would sample.
        let scene = band_scene(&[
            (Rect::new(0.0, 0.0, 40.0, 8.0), RED),
            // Straddles a band edge on a tile that is only partly covered, so
            // the case exercises an alpha-sampled instance cut in two, not
            // just a solid one.
            (Rect::new(49.0, 0.0, 99.0, 8.0), BLUE),
            (Rect::new(160.0, 0.0, 200.0, 8.0), RED),
        ]);

        let (one_page, one_page_rounds) = band_plan_of(&scene, &PageConfig::default());
        let (banded, banded_rounds) = band_plan_of(&scene, &BAND_PAGES);

        assert_eq!(
            band_bounds(&one_page_rounds).len(),
            1,
            "the reference plan really does hold the layer on one page"
        );
        assert!(
            band_bounds(&banded_rounds).len() > 1,
            "and the case only means anything while the other one is banded"
        );

        assert_eq!(
            painted_pixels(&banded, &banded_rounds),
            painted_pixels(&one_page, &one_page_rounds),
            "a banded layer paints what one whole-layer page would have"
        );
    }

    #[test]
    fn a_band_holds_no_instance_of_a_strip_outside_its_own_column() {
        // The counterexample the equality above generalizes: content confined
        // to the outer columns, and nothing at all in the middle. A band whose
        // column the scene never drew in must render nothing — under a
        // saturating shift it would render the leftmost content clamped onto
        // its own edge instead.
        let scene = band_scene(&[
            (Rect::new(0.0, 0.0, 40.0, 8.0), RED),
            (Rect::new(160.0, 0.0, 200.0, 8.0), BLUE),
        ]);
        let (scratch, rounds) = band_plan_of(&scene, &BAND_PAGES);
        let bands = band_bounds(&rounds);
        assert!(bands.len() > 2, "the layer bands: {bands:?}");

        let mut empty = 0_usize;
        for (plan, band) in scratch
            .rounds
            .iter()
            .zip(&rounds)
            .filter_map(|(plan, round)| round.page().map(|page| (plan, page.bounds)))
        {
            let strips = plan_strips(&scratch, plan);
            if strips.is_empty() {
                empty += 1;
            }
            for span in strips {
                let end = span.x + span.width;
                assert!(
                    end <= band.width(),
                    "an instance of {span:?} reaches past the {band:?} band's own \
                     column, which its composite never samples"
                );
                // Mapped back to the scene, every instance lands where one of
                // the two rectangles was actually drawn.
                let x = band.x0 + span.x;
                assert!(
                    x < 44 || band.x0 + end > 160,
                    "an instance covers device x {x}..{}, which neither \
                     rectangle reaches",
                    band.x0 + end
                );
            }
        }

        assert!(
            empty > 0,
            "a band whose column holds nothing renders nothing: {bands:?}"
        );
    }

    #[test]
    fn a_strip_straddling_a_band_edge_advances_its_alpha_column_with_its_geometry() {
        // A pixel-aligned rectangle's left edge lands on a tile of its own, so
        // a rectangle starting at 49 puts an alpha-sampled instance across
        // 48..52 — and a band edge falls inside it. The two pieces together
        // have to read the same coverage the whole instance would: neighbouring
        // pixels of one strip row sampling neighbouring alpha columns, with no
        // column repeated. The rectangles either side of it are what carries
        // the layer past the ceiling, so the middle one is banded at all.
        let scene = band_scene(&[
            (Rect::new(0.0, 0.0, 40.0, 8.0), RED),
            (Rect::new(49.0, 0.0, 99.0, 8.0), BLUE),
            (Rect::new(160.0, 0.0, 200.0, 8.0), RED),
        ]);
        let (scratch, rounds) = band_plan_of(&scene, &BAND_PAGES);
        let edges: Vec<u16> = band_bounds(&rounds)
            .iter()
            .map(|bounds| bounds.x0)
            .collect();

        // Every alpha-sampled pixel of every band, as (strip row, device x,
        // the alpha column it samples) — a row at a time, because two rows of
        // one draw sample different columns at the same x by construction.
        let mut dense: Vec<(u16, u16, u32)> = Vec::new();
        for (plan, band) in scratch
            .rounds
            .iter()
            .zip(&rounds)
            .filter_map(|(plan, round)| round.page().map(|page| (plan, page.bounds)))
        {
            for span in plan_strips(&scratch, plan) {
                for offset in 0..span.dense_width_or_rect_height {
                    dense.push((
                        band.y0 + span.y,
                        band.x0 + span.x + offset,
                        span.col_idx_or_rect_frac + u32::from(offset),
                    ));
                }
            }
        }
        dense.sort_unstable();

        let neighbours = || {
            dense
                .windows(2)
                .map(|pair| (pair[0], pair[1]))
                .filter(|((row, x, _), (next_row, next_x, _))| row == next_row && x + 1 == *next_x)
        };
        assert!(
            neighbours().any(|(_, (_, x, _))| edges.contains(&x)),
            "the case only means anything while an alpha-sampled instance \
             really is cut by a band edge {edges:?}: {dense:?}"
        );

        for ((_, _, column), (_, x, next_column)) in neighbours() {
            assert_eq!(
                next_column,
                column + 1,
                "neighbouring pixels of one strip row sample neighbouring alpha \
                 columns, band edge at {x} or not: {dense:?}"
            );
        }
    }

    #[test]
    fn a_translucent_layer_over_a_clear_is_composited_after_the_punch() {
        let (scratch, rounds) = plan_of(&plan_scene(|builder| {
            plan_backdrop(builder);
            builder.clear_rect(PLAN_SLOT);
            builder.push_layer(PLAN_CHIP, 0.5);
            builder.fill_rect(PLAN_CHIP, Brush::Solid(BLUE));
            builder.pop_layer();
        }));

        assert_eq!(
            scratch.rounds.len(),
            rounds.len() + 1,
            "the layer's page round, then the surface round cut in two"
        );
        let cut = scratch
            .rounds
            .iter()
            .position(|plan| plan.punch.1 > 0)
            .expect("the cut plan carries the punch");
        // The composite is what the cut has to fall before: a layer recorded
        // after the clear carries a deeper index than the punch, and the
        // composite writes no depth for the test to save it by.
        let composited = scratch.segments[scratch.rounds[cut + 1].segments.clone()]
            .iter()
            .any(|segment| matches!(segment, Segment::Composite(..)));
        assert!(
            composited,
            "the layer composites in the plan AFTER the punch, not before it"
        );
        assert!(
            !scratch.segments[scratch.rounds[cut].segments.clone()]
                .iter()
                .any(|segment| matches!(segment, Segment::Composite(..))),
            "and nothing composites into the plan the punch closes"
        );
    }

    // -----------------------------------------------------------------
    // Backdrop blurs: the copy round, the sampled page, the opaque limit
    //
    // Host-only, over the same plan the frame path records from: a
    // backdrop's region copy is the one round besides a punch that must
    // follow the opening, and its rectangle the one strip run that samples
    // a page rather than a paint.
    // -----------------------------------------------------------------

    /// The rectangle every backdrop case below blurs behind.
    const PLAN_PANEL: Rect = Rect::new(8.0, 8.0, 40.0, 36.0);

    /// [`plan_of`] for a frame whose backdrops are served — scheduled through
    /// [`Schedule::build_frame`] and planned with the frame's own backdrop
    /// records, as `encode` does on a readable target.
    fn backdrop_plan_of(scene: &Scene, punches: bool) -> (Scratch, Vec<Round>, CompiledFrame) {
        let frame = SceneCompiler::new(PLAN_VIEWPORT.0, PLAN_VIEWPORT.1)
            .compile(scene, Affine::IDENTITY, PLAN_VIEWPORT)
            .expect("an in-range scene compiles");
        let rounds = Schedule::build_frame(
            &frame.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            &PageConfig::default(),
            &RetainedLayers::new(),
            &frame.backdrops,
        )
        .expect("a backdrop under the surface schedules");
        let mut scratch = Scratch::default();
        scratch.build(&frame, &rounds, true, punches, &[], &frame.backdrops);
        (scratch, rounds, frame)
    }

    /// An opaque field, the backdrop, and a chip painted over the panel.
    fn backdrop_scene() -> Scene {
        plan_scene(|builder| {
            builder.fill_rect(
                Rect::new(
                    0.0,
                    0.0,
                    f64::from(PLAN_VIEWPORT.0),
                    f64::from(PLAN_VIEWPORT.1),
                ),
                Brush::Solid(BLUE),
            );
            builder.backdrop_blur(PLAN_PANEL, 2.0);
            builder.fill_rect(Rect::new(12.0, 12.0, 20.0, 20.0), Brush::Solid(RED));
        })
    }

    #[test]
    fn a_backdrop_ends_the_opening_before_its_copy_and_draws_after_its_passes() {
        let (scratch, _, frame) = backdrop_plan_of(&backdrop_scene(), false);
        assert_eq!(frame.backdrops.len(), 1);
        let (merged, apart) = pass_lists(&scratch);
        // σ 2 convolves directly: one horizontal and one vertical pass.
        assert_eq!(
            merged,
            [
                FramePass::Opening,
                FramePass::Copy,
                FramePass::Filter,
                FramePass::Filter,
                FramePass::Surface,
            ],
            "the opening — clear, opaque strips and the field — lands before the copy reads \
             the surface back, and the panel and chip draw after the blur"
        );
        assert_eq!(
            apart,
            [
                FramePass::Clear,
                FramePass::Opaque,
                FramePass::Surface,
                FramePass::Copy,
                FramePass::Filter,
                FramePass::Filter,
                FramePass::Surface,
            ],
            "unmerged, the same order with the clear and the opaque strips apart"
        );
    }

    #[test]
    fn a_backdrops_rectangle_samples_the_page_its_round_releases() {
        let (scratch, rounds, frame) = backdrop_plan_of(&backdrop_scene(), false);
        let backdrop = frame.backdrops[0];
        let sampling = rounds
            .iter()
            .position(|round| !round.samples.is_empty())
            .expect("one surface round samples the blurred page");
        let sample = rounds[sampling].samples[0];
        assert_eq!(sample.layer, backdrop.layer);
        assert!(
            rounds[sampling].released.contains(&sample.parity),
            "the round that samples the page hands it back"
        );

        let plan = &scratch.rounds[sampling];
        let segments = &scratch.segments[plan.segments.clone()];
        let (first, count, parity) = segments
            .iter()
            .find_map(|segment| match *segment {
                Segment::Backdrop(first, count, parity) => Some((first, count, parity)),
                _ => None,
            })
            .expect("the panel is drawn as a backdrop segment");
        assert_eq!(parity, sample.parity);
        assert!(count > 0);
        for instance in &scratch.alpha[first as usize..(first + count) as usize] {
            assert_eq!(
                instance.paint_and_rect_flag & !crate::gpu::strips::RECT_STRIP_FLAG,
                LAYER_PAINT_SOURCE | u32::from(u8::MAX),
                "every panel instance reads the layer input at full opacity"
            );
            let (page_x, page_y) = (instance.payload & 0xffff, instance.payload >> 16);
            assert_eq!(
                (page_x, page_y),
                (
                    u32::from(instance.x - backdrop.region.x0),
                    u32::from(instance.y - backdrop.region.y0)
                ),
                "and samples the page texel under its own device origin"
            );
        }
    }

    #[test]
    fn draws_after_a_backdrop_never_take_the_depth_writing_pass() {
        // The field recorded *after* the backdrop: its opaque interior must
        // not be drawn in the opening, ahead of the copy that reads the
        // surface back.
        let after = plan_scene(|builder| {
            builder.backdrop_blur(PLAN_PANEL, 2.0);
            builder.fill_rect(
                Rect::new(
                    0.0,
                    0.0,
                    f64::from(PLAN_VIEWPORT.0),
                    f64::from(PLAN_VIEWPORT.1),
                ),
                Brush::Solid(BLUE),
            );
        });
        let (scratch, _, _) = backdrop_plan_of(&after, false);
        assert!(
            scratch.opaque.is_empty(),
            "nothing at or past the backdrop's depth is routed opaque"
        );

        // The same field before the backdrop keeps its opaque spans.
        let (before, _, _) = backdrop_plan_of(&backdrop_scene(), false);
        assert!(!before.opaque.is_empty());
    }

    #[test]
    fn a_punch_ahead_of_a_backdrop_is_erased_before_the_copy() {
        let scene = plan_scene(|builder| {
            builder.fill_rect(PLAN_SLOT, Brush::Solid(BLUE));
            builder.clear_rect(PLAN_SLOT);
            builder.backdrop_blur(PLAN_PANEL, 2.0);
        });
        let (scratch, _, _) = backdrop_plan_of(&scene, true);
        let (merged, _) = pass_lists(&scratch);
        let punch = merged
            .iter()
            .position(|pass| *pass == FramePass::Punch)
            .expect("the punch is recorded");
        let copy = merged
            .iter()
            .position(|pass| *pass == FramePass::Copy)
            .expect("the copy is recorded");
        assert!(
            punch < copy,
            "the backdrop reads the erased surface, not the field the punch erases: {merged:?}"
        );
    }

    #[test]
    fn a_target_that_cannot_be_read_back_serves_no_backdrop() {
        // `encode` hands the plan no backdrop records on such a target: the
        // panel is the transparent draw it was recorded as, scheduled as the
        // ordinary layer it is — no copy, no filter pass, no sampled page.
        let (scratch, rounds) = plan_of(&backdrop_scene());
        assert!(
            rounds
                .iter()
                .all(|round| round.copy.is_none() && round.samples.is_empty())
        );
        let (merged, _) = pass_lists(&scratch);
        assert_eq!(merged, [FramePass::Opening]);
        assert_eq!(scratch.copies, 0);
    }

    #[test]
    fn the_region_copy_mode_prefers_sampling_and_needs_a_single_sample() {
        use wgpu::TextureFormat::{Bgra8Unorm, Rgba8Unorm, Rgba8UnormSrgb};
        use wgpu::TextureUsages as U;
        let attach = U::RENDER_ATTACHMENT;
        assert_eq!(
            RegionCopyMode::for_usage(attach | U::COPY_SRC, Rgba8Unorm, 1),
            RegionCopyMode::Texture,
            "a headless readback target in the pages' own format is copied"
        );
        assert_eq!(
            RegionCopyMode::for_usage(attach | U::COPY_SRC | U::TEXTURE_BINDING, Rgba8Unorm, 1),
            RegionCopyMode::Fragment,
            "sampling costs the one pass alone, so it wins whenever it is allowed"
        );
        assert_eq!(
            RegionCopyMode::for_usage(attach | U::TEXTURE_BINDING, Bgra8Unorm, 1),
            RegionCopyMode::Fragment,
            "any format can be sampled across into a page"
        );
        assert_eq!(
            RegionCopyMode::for_usage(attach | U::COPY_SRC, Rgba8UnormSrgb, 1),
            RegionCopyMode::Unavailable,
            "a copy would move encoded bytes into a linear page"
        );
        assert_eq!(
            RegionCopyMode::for_usage(attach, Bgra8Unorm, 1),
            RegionCopyMode::Unavailable,
            "a swapchain is RENDER_ATTACHMENT only"
        );
        assert_eq!(
            RegionCopyMode::for_usage(attach | U::TEXTURE_BINDING | U::COPY_SRC, Rgba8Unorm, 4),
            RegionCopyMode::Unavailable,
            "a multisampled target is neither"
        );
    }
}
