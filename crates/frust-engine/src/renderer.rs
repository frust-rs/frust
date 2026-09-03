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
//! What that contract does *not* extend to is colour. The clear pass below
//! clears the frame's colour target unconditionally, so content painted into
//! that target before `encode` keeps its depth and loses its pixels: a host
//! compositing over its own content records that content after the frame, or
//! into a target of its own the frame composites.
//!
//! # The frame's passes
//!
//! A frame records into that encoder, in this order.
//!
//! 1. **Clear.** Clears the colour target to the frame's base colour, and the
//!    depth attachment to the far plane unless the caller stated it is already
//!    populated (see [`crate::gpu::depth`]). It draws nothing; separating it
//!    from the strip passes is what lets a frame with no draws at all still
//!    resolve to a clean surface.
//! 2. **Opaque strips**, depth-tested and depth-writing, unblended. Once per
//!    frame, ahead of every round: only the fully-covered interior spans of
//!    opaque draws *targeting the surface* reach it — an anti-aliased edge is by
//!    definition not opaque, and a page carries no depth attachment for the
//!    split to be sound against. The depth it establishes is what lets the alpha
//!    passes reject fragments an opaque draw in front of them already covered.
//!
//!    Once, not once per surface round, and that is a correctness requirement:
//!    the surface can take several rounds (a [cut](crate::schedule::cut_at)
//!    round is how a wide sibling fan is served), and re-recording this pass
//!    ahead of each would re-draw opaque coverage at equal stored depth over
//!    composites the round before it had already blended. Running it ahead of
//!    the layer rounds rather than after them changes nothing they do: a layer
//!    round writes a pooled page and reads neither the surface nor the depth
//!    attachment.
//! 3. **One pass per [round](crate::schedule).** A page round renders one
//!    isolated layer into a pooled intermediate
//!    [page](crate::schedule::pages) it clears to transparent, at the page's
//!    own origin — so every instance is shifted by the page's tile-aligned
//!    bounds, and clipped to them, which is what makes a
//!    [banded](crate::schedule::pages::page_bands) layer's column pages tile
//!    their layer instead of each holding a clamped copy of it (see
//!    [`PageWindow`]) — and through the page's own viewport uniform; a round
//!    continuing a page an earlier round of the same layer opened loads it
//!    instead. A surface round draws **alpha strips**, premultiplied-blended,
//!    in painter order: depth-tested but not depth-writing when a depth
//!    attachment is in play, a plain painter's-algorithm pass when it is not.
//!    A round's ops run in the order [`Schedule::build`] listed them, so a
//!    finished child page composites into its parent exactly where the
//!    recording entered it.
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
//! 4. **The hole punch**, destination-out, when the frame recorded a
//!    `ClearRect` — see [`crate::compile::clear`] for the whole contract this
//!    pass implements. It is issued at the punch's own painter-order position,
//!    not at the end of the frame: a surface round is *cut* where the punch
//!    was recorded, the punch pass goes into that cut, and the round's
//!    remaining ops resume in a pass of their own after it. Everything drawn
//!    over the slot is therefore recorded after the erase and survives it,
//!    whether or not it wrote depth. A punch past every op of the frame — the
//!    ordinary case, a `ClearRect` recorded last — cuts nothing and lands after
//!    the last round exactly as it always did.
//!
//! With depth unavailable — no attachment, or `FRUST_ENGINE_NO_DEPTH` set —
//! pass 2 disappears and every instance travels through the surface rounds,
//! blended in painter order. That is a correctness requirement rather than a
//! fallback detail: routing the opaque spans into a separate, earlier pass is
//! only sound because the depth buffer re-establishes their ordering against the
//! blended ones.
//!
//! # Compositing a layer
//!
//! A finished page reaches its parent as ONE instanced quad through the same
//! strip program every draw goes through, flagged as a whole rectangle and
//! naming the layer colour source: the fragment stage then reads the page
//! bound as `layer_input_texture` at the quad's own texel and scales it by the
//! opacity packed into the instance's low byte. The page is bound through a
//! bind group of its own rather than the frame's shared one, because group 0
//! carries both the pass's viewport uniform and that layer input, and a page
//! round's viewport is its own.
//!
//! A composite carries the deepest painter's-order index of everything inside
//! the layer it composites, nested layers included. That is what keeps a
//! translucent layer correctly ordered against the root round's own draws
//! without giving the scheduler a depth model: every draw recorded before the
//! layer sits behind that index and every draw recorded after it sits in front.
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
use std::collections::HashMap;
use std::sync::{Arc, Once};

use frust_gpu::{PipelineCache, PooledTexture, SceneTextureId, ShaderLibrary, TierCaps};
use frust_scene::{Scene, SceneBuilder};
use glifo::{AtlasCommand, AtlasCommandRecorder, AtlasPaint};
use kurbo::Affine;
use peniko::{Brush, Color};
use vello_common::encode::{EncodedImage, EncodedPaint};
use vello_common::fearless_simd::Level;
use vello_common::paint::{ImageId, ImageSource, Paint};
use vello_common::strip::Strip;

use crate::cache::images::ATLAS_PADDING;
use crate::cache::{
    AtlasBudget, BYTES_PER_TEXEL, CachedRamp, GradientCache, GradientTextureLayout, ResidentImage,
};
use crate::compile::paint::resolve_lut_request;
use crate::compile::{ClearPunch, CompiledFrame, SceneCompiler};
use crate::config;
use crate::diag::{EngineSpan, FrameTimestamps};
use crate::error::EngineError;
use crate::filters::blur::{FilterInstanceData, GpuFilterData, GpuGaussianBlur};
use crate::filters::drop_shadow::GpuDropShadow;
use crate::filters::{FilterStep, ServedFilter, served_filter};
use crate::gpu::atlas::{
    AtlasPageBuffers, AtlasRenderReport, AtlasRenderer, lower_encoded_image, push_solid_strips,
};
use crate::gpu::bindings::{ExternalRuns, ExternalTextures, lower_encoded_external};
use crate::gpu::depth::DepthAttachment;
use crate::gpu::paint_texture::lower_encoded_paint;
use crate::gpu::pipelines::{EnginePipeline, EngineShaders, atlas_strip_desc, warm_up_descs};
use crate::gpu::strips::{PaintType, pack_paint_descriptor};
use crate::gpu::targets::{
    IntermediateTargets, IntermediateTexture, filter_data_texture_descriptor,
    filter_data_texture_height, filter_sampler,
};
use crate::gpu::{self, AtlasArray, GpuConfig, GpuEncodedPaint, GpuStrip, StripDraw};
use crate::schedule::pages::{PageConfig, PageSize};
use crate::schedule::{
    Composite, MAX_LIVE_PAGES, PageParity, PageTarget, Round, RoundOp, Schedule,
};
use crate::{EngineTarget, OutputAlpha};
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

/// Raised the first time an atlas region is declined, so a renderer whose
/// budget and array have gone out of agreement says so once rather than every
/// frame.
static ATLAS_REFUSAL_WARNING: Once = Once::new();

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
        let dim = caps.resource_texture_dim;
        if !dim.is_power_of_two() || dim < MIN_RESOURCE_TEXTURE_DIM {
            return Err(EngineError::AtlasError);
        }

        let mut library = ShaderLibrary::new();
        let shaders = EngineShaders::register(&mut library, device);
        let mut pipelines = PipelineCache::new(Arc::new(library), pipeline_cache.cloned());
        pipelines.warm_up(device, &warm_up_descs(&shaders, format));

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
            compiler: SceneCompiler::for_caps(1, 1, caps),
            gradients,
            depth: DepthAttachment::new(),
            targets: IntermediateTargets::new(caps),
            pages: PageConfig::default(),
            textures: ExternalTextures::new(),
            resources: FrameResources::new(device, dim),
            scratch: Scratch::default(),
            atlas_glyphs: None,
            atlas_lowering: None,
            atlas_report: AtlasRenderReport::default(),
            filters: None,
        })
    }

    /// The target format this renderer warmed its pipelines for.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
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
    pub fn finish_warm_up(&mut self, device: &wgpu::Device) {
        for pipeline in EnginePipeline::ALL {
            let _ = self
                .pipelines
                .get_or_create(device, &pipeline.desc(&self.shaders, self.format));
        }
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
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets.drop_parked();
        self.depth.resize(device, width, height);
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
        let size = grid_size(target.width, target.height)?;
        let mut frame = self.compiler.compile(scene, root, size)?;

        // The frame's pass plan, settled before anything is allocated or
        // recorded: a layer shape this scheduler does not serve, or one larger
        // than a page can be sized to, refuses the whole frame here so the
        // caller can skip it cleanly rather than present it half-drawn.
        let rounds = Schedule::build(&frame.recorder, &self.caps, &self.pages)?;
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

        // Paints are resolved before instances are built: an instance names
        // its paint by the texel its record starts at, which only exists once
        // the frame's ramps are resident and its records are laid out.
        let atlas_budget = self.compiler.images().budget();
        self.resources
            .resolve_paints(&frame, &mut self.gradients, atlas_budget, &self.textures);
        self.scratch.build(
            &frame,
            &rounds,
            depth_view.is_some(),
            punches,
            &self.resources.paint_slots,
        );

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

        // Past the last fallible step. The atlas is created or grown first, so
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
        if self.compiler.glyph_replay_pending()
            && self.replay_glyph_pages(device, queue, timestamps)
        {
            self.compiler.acknowledge_glyph_replay();
        }

        let format = target.format;
        let pipelines = self.frame_pipelines(device, format, depth_view.is_some());

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

        self.record_frame(
            device, queue, encoder, &target, depth_view, base_color, &pipelines, timestamps,
        );

        Ok(())
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
    /// avoid, and a bind group is only valid against the pipeline that derived
    /// its layout.
    fn frame_pipelines(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: bool,
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

        let mut frame = FramePipelines {
            alpha: (
                alpha_variant,
                self.pipelines
                    .get_or_create(device, &alpha_variant.desc(&self.shaders, format))
                    .clone(),
            ),
            opaque: None,
            page: None,
            punch: None,
            filter: None,
        };
        if depth && !self.scratch.opaque.is_empty() {
            frame.opaque = Some(
                self.pipelines
                    .get_or_create(
                        device,
                        &EnginePipeline::StripOpaque.desc(&self.shaders, format),
                    )
                    .clone(),
            );
        }
        if self.scratch.page_rounds() > 0 {
            frame.page = Some(
                self.pipelines
                    .get_or_create(
                        device,
                        &EnginePipeline::StripIntermediate.desc(&self.shaders, format),
                    )
                    .clone(),
            );
        }
        if self.scratch.punches() {
            frame.punch = Some((
                punch_variant,
                self.pipelines
                    .get_or_create(device, &punch_variant.desc(&self.shaders, format))
                    .clone(),
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

        self.resources
            .ensure_bind_groups(device, frame.alpha.0, &frame.alpha.1, format);
        self.resources.ensure_external_groups(
            device,
            frame.alpha.0,
            &frame.alpha.1,
            &self.textures,
        );
        if let Some(pipeline) = frame.opaque.as_ref() {
            self.resources.ensure_bind_groups(
                device,
                EnginePipeline::StripOpaque,
                pipeline,
                format,
            );
        }
        if let Some(pipeline) = frame.page.as_ref() {
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
        if let Some((variant, pipeline)) = frame.punch.as_ref() {
            self.resources
                .ensure_bind_groups(device, *variant, pipeline, format);
        }
        self.resources
            .ensure_page_configs(device, self.scratch.page_rounds());

        frame
    }

    /// Records the clear pass, every round's own pass, and the hole punch.
    ///
    /// Every pass opened here is ended before the method returns, which is the
    /// half of the encode contract a caller cannot check for itself.
    ///
    /// Each pass names the [`EngineSpan`] it is charged to: the frame's own
    /// surface passes are [`EngineSpan::Main`], a layer page round and a
    /// filter pass are [`EngineSpan::Composite`]. Several passes per span is
    /// the ordinary case and they sum.
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
        timestamps: FrameTimestamps<'_>,
    ) {
        let depth_load = self.depth.load_op(target.depth.is_some());

        // Drawing nothing is the point: this pass exists so a frame with no
        // instances at all still resolves to a clean surface. It is timed
        // alongside the frame's other surface passes even though a backend
        // that samples its counters at the vertex/fragment stage boundaries
        // may write nothing for it — an untimed pass is a measurement gap, not
        // a wrong measurement (see `frust_gpu::diag`).
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frust-engine clear"),
            color_attachments: &[Some(color_attachment(
                target.view,
                wgpu::LoadOp::Clear(clear_color(base_color, target.output)),
            ))],
            depth_stencil_attachment: depth_view.map(|view| depth_attachment(view, depth_load)),
            timestamp_writes: timestamps.writes(EngineSpan::Main),
            occlusion_query_set: None,
            multiview_mask: None,
        }));

        // Every field below is reached through `self.<field>` rather than
        // through a method: the page pool is borrowed mutably for the whole
        // walk while the instance buffer, the bind groups and the plan are
        // borrowed immutably, and only disjoint field borrows let those
        // coexist.
        let Some(instances) = self.resources.instances.as_ref() else {
            return;
        };
        let dim = self.caps.resource_texture_dim;
        let opaque_count = self.scratch.opaque.len() as u32;
        // The alpha region starts where the opaque one ends, so every segment's
        // own index is relative to that.
        let base = opaque_count;

        // The opaque pass, once for the whole frame and ahead of every round:
        // the depth it writes is what every blended instance the frame draws
        // onto its own target, composites included, is then tested against.
        //
        // Hoisted out of the round walk rather than recorded ahead of each
        // surface round, because a frame can take several of those (the
        // scheduler cuts one short to hand a page group back) and a second
        // recording of this pass would re-draw opaque coverage at equal stored
        // depth over composites the round before it had already blended. Ahead
        // of the layer rounds costs them nothing: a layer round writes a pooled
        // page and reads neither this target nor the depth attachment.
        if opaque_count > 0
            && let Some(opaque) = pipelines.opaque.as_ref()
            && let Some(opaque_groups) =
                self.resources.bind_groups.get(&EnginePipeline::StripOpaque)
        {
            record_pass(
                encoder,
                &PassPlan {
                    label: "frust-engine opaque strips",
                    view: target.view,
                    load: wgpu::LoadOp::Load,
                    depth: depth_view,
                    pipeline: opaque,
                    groups: opaque_groups,
                    resources: &opaque_groups.resources,
                    composites: &[],
                    // An external paint is never claimed opaque, so the
                    // depth-writing pass never holds a run.
                    external_keys: &[],
                    instances,
                    base: 0,
                    segments: &[Segment::Strips(0, opaque_count)],
                    timestamps: timestamps.writes(EngineSpan::Main),
                },
            );
        }

        // The destination-out pass's own recording state, resolved once for the
        // frame: a punch can now land at any of the walk's cuts, and every one
        // of them erases the same target through the same pipeline.
        let punch_pass = pipelines.punch.as_ref().and_then(|(variant, pipeline)| {
            Some(PunchPass {
                view: target.view,
                depth: depth_view,
                pipeline,
                groups: self.resources.bind_groups.get(variant)?,
                instances,
                base,
            })
        });

        // The frame's live pages — the two ping-pong groups and the one spill
        // page beside them — each holding the finished page a later round
        // composites (see [`crate::schedule`]).
        let mut live: [Option<PooledTexture>; MAX_LIVE_PAGES] = [const { None }; MAX_LIVE_PAGES];
        let mut page_slot = 0_usize;

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
                }

                settle_pages(&mut self.targets, &mut live, plan.released, own);
                continue;
            }

            // The round's viewport uniform. A page's is written here rather
            // than with the frame's other uploads because only the pool knows
            // the extent it quantized the request up to, and NDC is computed
            // against the attachment's real extent. Distinct buffers, so the
            // write ordering against the frame's own config never matters.
            let config = match &own {
                None => &self.resources.config,
                Some((_, pooled)) => {
                    let Some(config) = self.resources.page_configs.get(page_slot) else {
                        continue;
                    };
                    let (width, height) = pooled.size();
                    queue.write_buffer(
                        config,
                        0,
                        bytemuck::bytes_of(&GpuConfig::new(width, height, dim, dim)),
                    );
                    page_slot = page_slot.saturating_add(1);
                    config
                }
            };

            let variant = match &own {
                None => pipelines.alpha.0,
                Some(_) => EnginePipeline::StripIntermediate,
            };
            let pipeline = match (&own, pipelines.page.as_ref()) {
                (None, _) => &pipelines.alpha.1,
                (Some(_), Some(page)) => page,
                (Some(_), None) => continue,
            };
            let Some(groups) = self.resources.bind_groups.get(&variant) else {
                continue;
            };

            // A page round binds its own viewport uniform; the root round's is
            // already the one the shared set carries.
            let page_resources = own.as_ref().map(|_| {
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
                if let Segment::Composite(_, parity) = *segment {
                    // A group with no live page samples the transparent
                    // placeholder, which composites nothing — the same "draw
                    // less, never wrong" answer an unresolvable paint gets.
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
            }

            // A page round draws into an off-screen layer page, the frame's own
            // rounds into the surface — the split the two spans name.
            let span = if own.is_some() {
                EngineSpan::Composite
            } else {
                EngineSpan::Main
            };

            let (view, label, load, depth) = match &own {
                Some((_, pooled)) => (
                    pooled.view(),
                    PAGE_LABEL,
                    // A pooled texture holds whatever its last holder left
                    // there, so a layer's first round into a page clears it —
                    // and a round continuing that same page loads it, because
                    // clearing again would wipe the half already drawn.
                    if plan.page.is_some_and(|page| page.continued) {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    },
                    None,
                ),
                None => (
                    target.view,
                    "frust-engine alpha strips",
                    wgpu::LoadOp::Load,
                    depth_view,
                ),
            };

            // A surface plan with nothing of its own to draw records no pass:
            // loading and storing the target unchanged is exactly nothing. A
            // punch cut falling at the very start of a round — a `ClearRect`
            // recorded before anything the round draws — is how one arises. A
            // *page* plan with no segments still records its pass, because that
            // is what clears the pooled page.
            if !segments.is_empty() || own.is_some() {
                record_pass(
                    encoder,
                    &PassPlan {
                        label,
                        view,
                        load,
                        depth,
                        pipeline,
                        groups,
                        resources,
                        composites: &composites,
                        external_keys: self.resources.external_runs.keys(),
                        instances,
                        base,
                        segments,
                        timestamps: timestamps.writes(span),
                    },
                );
            }

            settle_pages(&mut self.targets, &mut live, plan.released, own);

            // The cut this plan ends at, erased once the ops before it have
            // been recorded and before the plan after it draws a thing. Inert
            // on every plan the walk did not cut, which is every plan of a
            // frame that punches nothing.
            if let Some(punch) = punch_pass.as_ref() {
                punch.record(encoder, plan.punch, timestamps.writes(EngineSpan::Main));
            }
        }

        // The punches past every op of the frame, in the position the pass held
        // unconditionally before painter order was restored.
        if let Some(punch) = punch_pass.as_ref() {
            punch.record(
                encoder,
                self.scratch.punch,
                timestamps.writes(EngineSpan::Main),
            );
        }

        for page in live.into_iter().flatten() {
            self.targets.release(page);
        }
    }
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
    alpha: (EnginePipeline, wgpu::RenderPipeline),
    /// The depth-writing pass, when depth is available and the frame has
    /// fully covered spans to route into it.
    opaque: Option<wgpu::RenderPipeline>,
    /// The pass a layer page is rendered through, when the frame has one.
    page: Option<wgpu::RenderPipeline>,
    /// The destination-out pass, with its variant, when the frame punches.
    punch: Option<(EnginePipeline, wgpu::RenderPipeline)>,
    /// The pass one filter round runs through, when the frame filters a layer.
    filter: Option<wgpu::RenderPipeline>,
}

/// One pass's full recording state, assembled before the pass is begun.
///
/// A struct rather than an argument list because the composite groups have to
/// be built (and so borrowed) before `begin_render_pass` takes the encoder, and
/// naming them together is what makes that ordering obvious at the call site.
struct PassPlan<'a> {
    label: &'a str,
    view: &'a wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    depth: Option<&'a wgpu::TextureView>,
    pipeline: &'a wgpu::RenderPipeline,
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
    /// The query pair this pass's GPU time is stamped into, `None` for an
    /// untimed pass — which is every pass of every frame encoded without a
    /// timestamp sink (see [`crate::diag`]).
    timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
}

/// Records one pass: load the colour target, then draw each segment in order.
///
/// Two groups are re-bound per segment. Group 0, because a composite reads its
/// page through it while an ordinary strip reads the placeholder; and group 1,
/// because a run of instances sampling an externally bound texture needs that
/// texture bound where the atlas placeholder otherwise sits. Groups 2-3 are the
/// variant's own and never change within a pass. A pass with one segment —
/// every frame that records no layer and no external texture — sets them all
/// exactly once.
///
/// A segment naming a slot with no bind group behind it draws nothing rather
/// than drawing with whatever group 1 last held: the texture was unbound
/// between the frame's paint resolution and its recording, and a wrongly
/// sampled run is worse than a missing one.
fn record_pass(encoder: &mut wgpu::CommandEncoder, plan: &PassPlan<'_>) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(plan.label),
        color_attachments: &[Some(color_attachment(plan.view, plan.load))],
        depth_stencil_attachment: plan
            .depth
            .map(|view| depth_attachment(view, wgpu::LoadOp::Load)),
        timestamp_writes: plan.timestamps.clone(),
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(plan.pipeline);
    pass.set_vertex_buffer(0, plan.instances.slice(..));

    let mut composite = 0_usize;
    for segment in plan.segments {
        let (group, images, range) = match *segment {
            Segment::Strips(first, count) => {
                if count == 0 {
                    continue;
                }
                (
                    plan.resources,
                    None,
                    GpuStrip::instance_range(plan.base.saturating_add(first), count),
                )
            }
            Segment::External(first, count, slot) => {
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
        };
        plan.groups.bind_with(&mut pass, group, images);
        pass.draw(GpuStrip::vertex_range(), range);
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
    pipeline: &'a wgpu::RenderPipeline,
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
    /// records no `ClearRect`.
    fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        punch: (u32, u32),
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let (first, count) = punch;
        if count == 0 {
            return;
        }
        record_pass(
            encoder,
            &PassPlan {
                label: "frust-engine hole punch",
                view: self.view,
                load: wgpu::LoadOp::Load,
                depth: self.depth,
                pipeline: self.pipeline,
                groups: self.groups,
                resources: &self.groups.resources,
                composites: &[],
                // A punch erases with a solid source; it samples nothing.
                external_keys: &[],
                instances: self.instances,
                base: self.base,
                segments: &[Segment::Strips(first, count)],
                timestamps,
            },
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
/// caller is not a convenience: `frust_scene` carries no filter command yet
/// (the scene seam is a later plan), so a filter layer cannot reach
/// [`EngineRenderer::encode`] through a `Scene` at all, and driving this type
/// directly is the only way the ported WGSL is exercised on a device.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    /// Ordinary strip instances, as `(first, count)`, drawn with the frame's
    /// own atlas binding.
    Strips(u32, u32),
    /// Strip instances sampling an externally bound texture, as
    /// `(first, count, slot)` — a *run*, in the sense
    /// [`crate::gpu::bindings::ExternalRuns`] gives the word: the maximal span
    /// of consecutive instances that share one texture, drawn with that
    /// texture's own group 1 bound.
    External(u32, u32, u32),
    /// One composite quad at `first`, sampling the finished page in the named
    /// group.
    Composite(u32, PageParity),
}

/// One scheduled round, resolved to the instances and target it draws with.
///
/// One *plan* rather than one scheduled round: a surface round carrying a hole
/// punch is cut into a plan per span between its punches, so the punch pass can
/// be recorded at the painter-order position it was hoisted from (see
/// [`Scratch::cut_for_punches`]).
#[derive(Debug, Clone)]
struct RoundPlan {
    /// The page this round renders into, or `None` for the frame's own target.
    page: Option<PagePlan>,
    /// This round's slice of [`Scratch::segments`], in execution order.
    segments: Range<usize>,
    /// Page groups this round consumed, indexed by
    /// [`PageParity::index`]; each returns to the pool once the pass ends.
    released: [bool; MAX_LIVE_PAGES],
    /// The filter pass this round runs, on a filter round — which draws no
    /// strip and composites nothing, so its `segments` range is empty.
    filter: Option<FilterPlan>,
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
    fn build(
        &mut self,
        frame: &CompiledFrame,
        rounds: &[Round],
        depth_active: bool,
        punches: bool,
        paint_slots: &[Option<ResolvedPaint>],
    ) {
        self.opaque.clear();
        self.alpha.clear();
        self.segments.clear();
        self.rounds.clear();
        self.punch = (0, 0);
        self.layer_depth.clear();
        self.layer_depth.resize(frame.recorder.layers.len(), 0);
        self.filter_blocks.clear();
        self.filter_layers.clear();
        self.filter_passes = 0;

        let draws = frame.draws();
        let strips = frame.strip_buf();
        // The punches still to be issued, in the order the compiler hoisted
        // them — which is depth order, since each took its own index from the
        // frame's monotonic painter-order counter. Empty on a frame with no
        // clear and on a target that disregards alpha (`punches`), and then
        // nothing below ever cuts: a non-punching frame is planned exactly as
        // it always was.
        let mut pending: &[ClearPunch] = if punches { &frame.clears } else { &[] };

        for round in rounds {
            let page = round.page();
            let filter = self.plan_filter(frame, round);
            // A page holds its layer — or one column band of it — at the page's
            // own origin, so every instance of the round is shifted into that
            // column and clipped to it (see `PageWindow`).
            let window = page.map_or(PageWindow::ROOT, PageWindow::of);
            let split_opaque = page.is_none() && depth_active;
            // Only the frame's own target is punched — a punch inside an
            // isolated layer was hoisted out of it — so only a surface round is
            // ever cut.
            let cuts = page.is_none();

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
                            let Some(paint) = pack_paint(&draw.paint, draw.depth, paint_slots)
                            else {
                                continue;
                            };
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
                            let to_opaque = paint.opaque && split_opaque;

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
                        self.segments
                            .push(Segment::Composite(end, composite.parity));
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
            if let Some(page) = page
                && let Some(slot) = self.layer_depth.get_mut(page.layer as usize)
            {
                *slot = (*slot).max(deepest);
            }

            let mut released = [false; MAX_LIVE_PAGES];
            for parity in &round.released {
                if let Some(slot) = released.get_mut(parity.index()) {
                    *slot = true;
                }
            }

            self.rounds.push(RoundPlan {
                page: page.map(|page| PagePlan {
                    parity: page.parity,
                    size: page.size,
                    continued: page.continued,
                }),
                segments: first_segment..self.segments.len(),
                released,
                filter,
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
            segments: *first_segment..self.segments.len(),
            // The pages a cut round consumed are handed back when its LAST plan
            // ends: nothing between two plans of one round acquires a page, and
            // a composite in an earlier plan still has to sample the page it
            // names.
            released: [false; MAX_LIVE_PAGES],
            filter: None,
            punch,
        });
        *first_segment = self.segments.len();
        *run_start = self.alpha.len() as u32;
    }

    /// Records the open run of instances `first..end` as one segment, drawn
    /// with the texture in `external` bound, or nothing at all when the run is
    /// empty.
    ///
    /// The one place a strip segment is created, so the run-breaking rule —
    /// a segment holds instances sharing one group-1 binding — is stated once
    /// rather than at each of the four points a run can close.
    fn push_run(&mut self, first: u32, end: u32, external: Option<u32>) {
        let Some(count) = end.checked_sub(first).filter(|count| *count > 0) else {
            return;
        };
        self.segments.push(match external {
            Some(slot) => Segment::External(first, count, slot),
            None => Segment::Strips(first, count),
        });
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
    fn plan_filter(&mut self, frame: &CompiledFrame, round: &Round) -> Option<FilterPlan> {
        let pass = round.filter_pass()?;
        let page = round.page()?;
        let recorded = frame.recorder.layers.get(pass.layer as usize)?;
        let data_offset = self.filter_block(pass.layer, &recorded.kind)?;

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
        let index = match self.filter_layers.iter().position(|id| *id == layer) {
            Some(index) => index,
            None => {
                // The one dispatch every filter-recognising site in this crate
                // shares (`schedule::layer_role`/`filter_rounds`), so the block
                // packed here is for the same filter the scheduler planned the
                // passes of.
                let block = match served_filter(layer, kind).ok()? {
                    ServedFilter::Blur(blur) => GpuFilterData::from(GpuGaussianBlur::from(&blur)),
                    ServedFilter::DropShadow(shadow) => {
                        GpuFilterData::from(GpuDropShadow::from(&shadow))
                    }
                };
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
            .filter(|plan| plan.page.is_some() && plan.filter.is_none())
            .count()
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
}

impl PackedPaint {
    /// The payload an instance whose scene origin is `(x, y)` carries.
    fn payload_at(self, x: u16, y: u16) -> u32 {
        match self.payload {
            PaintPayload::Solid(rgba) => rgba,
            PaintPayload::Position => pack_u16_pair(x, y),
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
}

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
    /// One set of bind groups per strip pipeline variant. Not one shared set:
    /// every engine pipeline uses wgpu's derived layout, and a derived layout
    /// is exclusive to the pipeline that derived it.
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
    fn new(device: &wgpu::Device, dim: u32) -> Self {
        let min = gpu::MIN_RESOURCE_TEXTURE_HEIGHT;
        Self {
            alphas: ResourceTexture::new(device, &gpu::alpha_texture_descriptor(dim, min)),
            paints: ResourceTexture::new(
                device,
                &gpu::paint_texture::encoded_paints_texture_descriptor(dim, min),
            ),
            gradients: ResourceTexture::new(device, &gradient_texture_descriptor(dim, min)),
            placeholders: Placeholders::new(device),
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
        for (index, paint) in frame.encoded_paints.iter().enumerate() {
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
                    paint_type: record.paint_type(),
                    texel_offset,
                    opaque: !paint.may_have_transparency(),
                    external,
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
            self.image_registry
                .retain(|_, resident| !frame.image_evictions.contains(&resident.region));
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
            groups
                .externals
                .insert(*key, images_bind_group(device, pipeline, atlas_view, view));
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
/// a single-layer 2D array.
fn placeholder_view(device: &wgpu::Device, label: &str, array: bool) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
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
    /// only it knows which the frame's own `atlas_layers` calls for.
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
        paints: &wgpu::TextureView,
        gradients: &wgpu::TextureView,
    ) -> Self {
        let resources_group =
            resources_bind_group(device, pipeline, alphas, config, &placeholders.layer_input);
        let images = images_bind_group(device, pipeline, atlas, &placeholders.external);
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

/// Group 1 of a strip pass: the image atlas array, and the caller-owned
/// texture an external image paint samples.
///
/// One function for both shapes the group takes — `external` is
/// [`Placeholders::external`] for the frame-wide group, or a registered view
/// for the group a run of external instances is drawn with (see
/// [`crate::gpu::bindings`]). Built against one pipeline for the same reason
/// [`resources_bind_group`] is: every engine pipeline uses wgpu's derived
/// layout, which is exclusive to the pipeline that derived it.
fn images_bind_group(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    atlas: &wgpu::TextureView,
    external: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frust-engine strip images"),
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(atlas),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(external),
            },
        ],
    })
}

/// Group 0 of a strip pass: the frame's coverage, the pass's own viewport
/// uniform, and the texture a composite reads its finished page from.
///
/// Built against one pipeline rather than shared, because every engine pipeline
/// uses wgpu's *derived* layout — a layout derived from a shader module is
/// exclusive to the pipeline that derived it, so a bind group built against one
/// variant's layout is rejected by another's even when the two layouts are
/// structurally identical.
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust_gpu::DownlevelProfile;
    use kurbo::Rect;
    use peniko::color::palette::css::{BLUE, RED};
    use std::collections::BTreeMap;
    use vello_common::geometry::RectU16;
    use vello_common::paint::IndexedPaint;

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

    #[test]
    fn a_resolved_indexed_paint_samples_at_each_instances_own_position() {
        let slots = [Some(ResolvedPaint {
            paint_type: PaintType::LinearGradient,
            texel_offset: 6,
            opaque: true,
            external: None,
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
        scratch.build(&frame, &rounds, true, !frame.clears.is_empty(), &[]);
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
                    Segment::Strips(first, count) | Segment::External(first, count, _) => {
                        first + count
                    }
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
                    Segment::Strips(first, _)
                    | Segment::External(first, _, _)
                    | Segment::Composite(first, _) => first >= punch_first + punch_count,
                }),
            "and everything recorded after it lands on top of the erase"
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
        scratch.build(&frame, &rounds, false, false, &[]);
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
                Segment::Strips(first, count) | Segment::External(first, count, _) => {
                    Some((first as usize, count as usize))
                }
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
}
