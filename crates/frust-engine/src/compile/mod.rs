//! Scene compilation: a `frust_scene::Scene` becomes sparse strips plus draws.
//!
//! [`SceneCompiler`] is a stateless walk over an already-recorded display list.
//! Unlike an immediate-mode scene recorder, there is no render state to save
//! and restore and no transform stack to unwind — `frust_scene::SceneBuilder`
//! has already composed every command's transform, so each command carries the
//! only transform it needs and the walk composes it with the frame's root once.
//!
//! The compiler owns the retained scratch a
//! [`StripGenerator`] needs (line buffer, tiles, flatten/stroke context) so a
//! steady-state frame reuses those allocations, and the two pieces of state
//! that genuinely span frames — the [`ImageResidency`] that keeps an image's
//! atlas rectangle alive for as long as the scene keeps drawing it, and the
//! [`GlyphPrepCache`] that keeps a glyph's fetched outline and its font's
//! hinting instance alive on the same terms. Everything else is per-frame:
//! [`compile`](SceneCompiler::compile) returns what a frame produced in one
//! [`CompiledFrame`] and keeps nothing of it.
//!
//! Compiled here: the geometry primitives — axis-aligned and rounded
//! rectangles, lines, arbitrary filled/stroked paths — the clip bracket around
//! them, which lowers to a scissor rectangle or a coverage mask and never to an
//! intermediate texture (see [`clip`]), the opacity-layer and snapshot brackets
//! that group them (see [`layers`]), the hole punch that erases what they
//! painted (see [`clear`]), images, whose destination rectangle is
//! rasterized like any other fill and painted by an atlas-backed image paint
//! (see [`paint`] and [`crate::cache::images`]), and blurred rounded
//! rectangles, whose padded bounding rectangle is rasterized the same way and
//! painted by a gaussian-falloff paint the fragment shader evaluates per pixel
//! (see [`blur_rrect`]), and glyph runs, which take one of two routes decided
//! per run by [`crate::text::atlas_policy`] before the walk begins: a settled
//! run is resolved through the glyph atlas and each glyph drawn as one image
//! paint over its slot, while an animating, oversized or refused one has its
//! outlines fetched and scaled by `glifo` and rasterized as any other filled
//! path would be, painted by the run's own brush (see [`crate::text`]).
//! Shader quads are recognised
//! and skipped — the engine grows them in a later pass, and skipping is the
//! conservative behaviour (a frame draws less, never wrong).

pub mod blur_rrect;

pub mod clear;

pub mod clip;

pub mod layers;

pub mod paint;

pub mod draw;

pub mod external;

pub use clear::ClearPunch;
pub use clip::ClipStack;
pub use draw::{DepthCounter, EngineDraw};
pub use external::{ExternalExtents, ExternalSkip};
pub use layers::{GroupStack, LayerLowering, SnapshotStack};

use std::collections::HashSet;
use std::sync::Once;

use kurbo::{
    Affine, BezPath, Cap, Join, Line, PathEl, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke,
};
use peniko::{Brush, Color, Fill, ImageData};

use frust_gpu::TierCaps;
use frust_scene::{Command, CornerRadii, DashPattern, GlyphRun, PathStyle, Scene};

use glifo::{AtlasCacher, GlyphAtlas, GlyphPrepCache, PendingClearRect};

use vello_common::clip::PathDataRef;
use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::paint::ImageId;
use vello_common::record::CommandRecorder;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};
use vello_common::tile::Tile;
use vello_common::util::is_axis_aligned;

use crate::cache::images::{
    AtlasBudget, AtlasRegion, ImageResidency, ImageSkip, ImageUpload, is_mobile_tier,
};
use crate::compile::blur_rrect::{encode_blurred_rounded_rect, inflated_bounds};
use crate::compile::clear::StagedPunch;
use crate::compile::external::encode_scene_texture;
use crate::compile::paint::{LutRequest, encode_brush, encode_image_brush, encode_image_command};
use crate::error::EngineError;
use crate::text::{
    AtlasPolicy, GlyphRunTargets, RunKey, RunRoute, context_paint, font_has_color_glyphs,
    font_is_readable, glyph_atlas_policy, lower_glyph_run,
};

/// Curve-flattening tolerance, in device pixels.
///
/// The value `vello_hybrid`'s own scene recorder flattens at; keeping it
/// identical is what lets the two rasterizers be compared strip-for-strip.
pub(crate) const FLATTEN_TOLERANCE: f64 = 0.1;

/// Raised the first time an image is refused residency, so a scene that draws
/// an unsupported image says so at least once at warning level without the
/// per-frame repetition a per-skip warning would produce.
static IMAGE_SKIP_WARNING: Once = Once::new();

/// Raised the first time a glyph run is refused for an unreadable font, on the
/// same once-per-process terms as [`IMAGE_SKIP_WARNING`].
static FONT_SKIP_WARNING: Once = Once::new();

/// Raised the first time the compiler drops a [`Command::ShaderQuad`], on the
/// same once-per-process terms as [`IMAGE_SKIP_WARNING`].
static SHADER_QUAD_SKIP_WARNING: Once = Once::new();

/// Where one glyph an atlas-routed draw sampled lives in the atlas array.
///
/// The image half of residency travels as an [`ImageUpload`], carrying pixels;
/// a glyph's pixels are produced *on the GPU* by the replay pass, so nothing
/// travels here but the rectangle — which the renderer still needs, because a
/// glyph paint names its slot by [`ImageId`] and only the sink that drew it was
/// ever handed the slot itself.
///
/// Reported per draw rather than per allocation, so a recycled handle can never
/// be resolved against a previous occupant's rectangle: `glifo` returns an
/// evicted slot's id to the shared allocator, and whatever takes it next — a
/// glyph or an image — reports its own rectangle on the frame it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSlot {
    /// The handle the draw's image paint names this slot by.
    pub id: ImageId,
    /// The slot's own rectangle, padding excluded.
    pub region: AtlasRegion,
    /// Transparent padding texels `glifo` keeps around `region`.
    pub padding: u32,
}

/// Everything one compiled frame produced.
///
/// The strips and their alpha coverage share one
/// [`StripStorage`]: `strips.strips` is the frame's whole strip buffer (each
/// [`EngineDraw::strip_range`] indexes into it) and `strips.alphas` the alpha
/// runs those strips reference. They are kept together because a strip's
/// packed alpha index is only meaningful against the alpha buffer generated
/// alongside it.
///
/// The draws themselves live in `recorder.draws` rather than in a field of
/// their own: [`CommandRecorder`] already owns that vector, and its node
/// ranges index into it, so a second parallel copy could only drift out of
/// agreement with the recording. Read them through [`CompiledFrame::draws`].
#[derive(Debug)]
pub struct CompiledFrame {
    /// The frame's strips and the alpha coverage they index.
    pub strips: StripStorage,
    /// The recorded render graph, owning the frame's draws.
    pub recorder: CommandRecorder<EngineDraw>,
    /// Paints too complex to inline into a draw, indexed by
    /// [`Paint::Indexed`](vello_common::paint::Paint::Indexed).
    pub encoded_paints: Vec<EncodedPaint>,
    /// The frame's hole punches, hoisted to the root and issued at the
    /// punch's own painter-order position (see [`clear`]).
    ///
    /// Deliberately not draws: a punch erases rather than paints, and keeping
    /// it out of the recording is what lets a target that disregards alpha
    /// drop the whole pass and read the frame unchanged.
    pub clears: Vec<ClearPunch>,
    /// The colour ramps `encoded_paints` needs made resident before the frame
    /// is drawn, one per gradient entry.
    ///
    /// Deliberately not serviced here: the compiler holds no gradient cache,
    /// so ramp residency is decided once per frame by the renderer rather than
    /// per draw by the walk (see [`paint`]).
    pub lut_requests: Vec<LutRequest>,
    /// How many of this frame's draws wrote their strip coverage directly as a
    /// rectangle, bypassing flattening and tiling (see [`fast_rect`]).
    ///
    /// Purely observational — nothing downstream branches on it. It exists so
    /// the fast path's admission rule is measurable from outside the compiler
    /// rather than inferred from a strip count that both paths can produce.
    pub fast_rect_draws: u32,
    /// How many of this frame's clips lowered to a scissor rectangle, costing
    /// no rasterization at all (see [`clip`]).
    pub scissor_clips: u32,
    /// How many of this frame's clips lowered to a coverage mask.
    pub mask_clips: u32,
    /// Atlas regions whose texels must be cleared before this frame draws,
    /// freed by the residency reap at the head of the frame.
    ///
    /// Serviced **before** [`image_uploads`](Self::image_uploads): a rectangle
    /// freed this frame can be re-allocated in the same frame, so clearing
    /// after writing would erase the image that just moved in.
    pub image_evictions: Vec<AtlasRegion>,
    /// Atlas regions whose texels must be written before this frame draws, one
    /// per image that became resident during it.
    ///
    /// Empty in the steady state: an image drawn on a thousand consecutive
    /// frames appears here exactly once, on the first.
    pub image_uploads: Vec<ImageUpload>,
    /// The atlas array depth this frame's paints address — the layer count the
    /// array texture must have grown to before the uploads are written.
    pub atlas_layers: u32,
    /// How many of this frame's draws painted with an atlas-backed image.
    pub image_draws: u32,
    /// How many image draws were dropped because the image could not be made
    /// resident (unsupported format, oversized, malformed, atlas full, or the
    /// atlas disabled outright).
    ///
    /// Observational, and the counter that makes "an image the engine cannot
    /// hold is a skipped draw, not a panicked frame" measurable rather than
    /// asserted.
    pub skipped_images: u32,
    /// How many of this frame's draws painted with an externally bound
    /// texture.
    pub external_draws: u32,
    /// How many external-texture draws were dropped — an id nothing is
    /// registered under, or a destination the texture cannot be mapped onto.
    ///
    /// Observational, and the external counterpart of
    /// [`skipped_images`](Self::skipped_images): "a texture the engine cannot
    /// resolve is a skipped draw, not a wrongly-sampled one", measured rather
    /// than asserted.
    pub skipped_externals: u32,
    /// How many strips this frame's coverage masks cost.
    ///
    /// Observational, and the counter the clip lowering's whole claim rests on:
    /// a frame whose clips all scissored reports zero here, which is what
    /// "a rectangular clip is free" means measured rather than asserted.
    pub clip_mask_strips: usize,
    /// How many of this frame's draws painted one glyph outline.
    ///
    /// A glyph run costs one draw per glyph that produced coverage, so this is
    /// bounded by — and usually below — the run's own glyph count: a glyph
    /// clipped away or carrying no ink (a space) records nothing.
    pub glyph_draws: u32,
    /// How many glyphs were dropped because the engine has no way to paint
    /// them on this path — a colour (COLR) glyph, a bitmap-strike glyph, or a
    /// stroked outline.
    ///
    /// Observational, and the counter that makes "a glyph the engine cannot
    /// paint goes missing rather than landing wrong" measurable rather than
    /// asserted, the same way [`skipped_images`](Self::skipped_images) does
    /// for images.
    pub skipped_glyphs: u32,
    /// How many of [`glyph_draws`](Self::glyph_draws) sampled the glyph atlas
    /// rather than rasterizing an outline.
    ///
    /// The measure of what the policy is actually buying: a page of settled
    /// text reads all-atlas, an animating size reads zero, and the difference
    /// between them is the frame's rasterization work.
    ///
    /// Observational only. It is **not** the signal for whether the atlas has
    /// pixel work outstanding — a run whose draws were all culled still
    /// inserted entries and dirtied a page while reporting zero here. That
    /// question is [`SceneCompiler::glyph_replay_pending`]'s.
    pub atlas_glyph_draws: u32,
    /// Where each of this frame's atlas-sampled glyphs lives, one entry per
    /// atlas draw (see [`GlyphSlot`]).
    pub glyph_slots: Vec<GlyphSlot>,
    /// Atlas rectangles freed by the *previous* frame's glyph eviction, to be
    /// zeroed before this frame writes anything into the array.
    ///
    /// Carried a frame late deliberately: `glifo` evicts at the end of a frame,
    /// and a rectangle it frees can be handed straight back out on the next
    /// one, so clearing it after that frame's uploads and replay would erase
    /// whatever just moved in. Same ordering, same reason, as
    /// [`image_evictions`](Self::image_evictions).
    ///
    /// Reported rather than consumed, on the same terms as
    /// [`image_evictions`](Self::image_evictions): the same rectangles appear
    /// on every later frame until a caller that really wrote them calls
    /// [`SceneCompiler::acknowledge_glyph_clears`]. A frame compiled and then
    /// refused takes none of them with it.
    pub glyph_clears: Vec<PendingClearRect>,
}

impl CompiledFrame {
    /// The frame's draws, in paint order (back-most first).
    pub fn draws(&self) -> &[EngineDraw] {
        &self.recorder.draws
    }

    /// The frame's whole strip buffer; a draw's `strip_range` indexes into it.
    pub fn strip_buf(&self) -> &[vello_common::strip::Strip] {
        &self.strips.strips
    }

    /// The alpha coverage the frame's strips reference.
    pub fn alphas(&self) -> &[u8] {
        &self.strips.alphas
    }
}

/// Compiles a `frust_scene::Scene` into strips and draws.
///
/// Create one per surface and reuse it across frames — the retained
/// [`StripGenerator`] is the point.
#[derive(Debug)]
pub struct SceneCompiler {
    generator: StripGenerator,
    clips: ClipStack,
    groups: GroupStack,
    snapshots: SnapshotStack,
    punches: Vec<StagedPunch>,
    images: ImageResidency,
    glyphs: GlyphPrepCache,
    /// Which glyphs earn an atlas slot, and the entry map they live in.
    ///
    /// A sibling field of [`Self::images`] rather than a member of it: the two
    /// share one allocator but decide different things, and every call that
    /// needs both takes them as disjoint borrows of this struct (see
    /// [`crate::text::atlas_policy`] for why the allocator is the residency's).
    glyph_atlas: AtlasPolicy,
    /// This frame's per-run routing decisions, in the order the scene records
    /// its glyph runs.
    ///
    /// Filled by the collect walk at the head of [`Self::compile`] and consumed
    /// by the draw walk one run at a time. Retained across frames only for its
    /// allocation.
    run_routes: Vec<RunRoute>,
    /// How many of [`Self::run_routes`] the draw walk has consumed.
    next_run: usize,
    /// Scratch for the collect walk's distinct-glyph count, retained across
    /// runs and frames for its allocation alone. A run's admission is charged
    /// against the atlas budget at that count (see
    /// [`crate::text::RunKey::distinct_glyphs`]), and counting it needs a set;
    /// one owned here is one not allocated per run. It carries nothing between
    /// calls — `RunKey::for_run` clears it before it counts.
    run_glyph_ids: HashSet<u32>,
    /// Whether a glyph run's outline is hinted before it is rasterized (see
    /// [`crate::text`]'s module doc for the split this half of the policy
    /// answers). Mobile-safe by default — `false`, the same "known nothing
    /// about the device yet" reasoning [`Self::new`] gives
    /// [`AtlasBudget::MOBILE`] — and set from the adapter's own class by
    /// [`Self::for_caps`], or directly by [`Self::set_hint_text`] for a test
    /// that wants either answer without a `TierCaps` in hand.
    hint_text: bool,
    /// The texel extent of every externally bound texture, so a
    /// [`Command::SceneTexture`] can be lowered without this crate's compile
    /// half knowing anything about `wgpu` (see
    /// [`crate::compile::external`]). Written through
    /// [`Self::bind_external_texture`]/[`Self::unbind_external_texture`],
    /// which the renderer calls alongside its own view registry so the two
    /// halves are always registered together.
    externals: ExternalExtents,
}

impl SceneCompiler {
    /// A compiler sized for a `width` x `height` viewport.
    ///
    /// The size is re-asserted on every [`compile`](Self::compile) call, so
    /// this is only the initial allocation hint; pass the surface's current
    /// size to avoid an immediate resize.
    ///
    /// Image residency starts on [`AtlasBudget::MOBILE`], the smaller of the
    /// two tiers. A compiler built without an adapter in hand knows nothing
    /// about the device it will end up on, and over-budgeting a phone costs
    /// real memory while under-budgeting a desktop costs only an extra atlas
    /// layer — call [`for_caps`](Self::for_caps) or
    /// [`set_atlas_budget`](Self::set_atlas_budget) once the adapter is known.
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_atlas_budget(width, height, AtlasBudget::MOBILE)
    }

    /// A compiler sized for a `width` x `height` viewport, with image
    /// residency budgeted for `caps`' adapter and glyph hinting decided by
    /// `caps`' device class.
    ///
    /// Hinting is turned on for a desktop-class adapter and left off for a
    /// mobile one — the same `!`[`is_mobile_tier`] split
    /// [`AtlasBudget::for_caps`] draws its own tier from, so a caller with an
    /// adapter in hand only ever answers the mobile-or-desktop question once.
    /// See [`crate::text`]'s module doc for why hinting defaults off and what
    /// the other half of the policy — the transform predicate `glifo` applies
    /// on top of this — is not this crate's to make.
    pub fn for_caps(width: u16, height: u16, caps: &TierCaps) -> Self {
        let mut compiler = Self::with_atlas_budget(width, height, AtlasBudget::for_caps(caps));
        compiler.hint_text = !is_mobile_tier(caps);
        compiler
    }

    /// Set whether a glyph run's outline is hinted before it is rasterized,
    /// bypassing [`Self::for_caps`]' `TierCaps` reading.
    ///
    /// For a test that wants a chosen answer without building a `TierCaps` —
    /// [`Self::new`] and [`Self::with_atlas_budget`] already default to the
    /// mobile-safe `false`, so this is also how a caller that built one of
    /// those turns hinting on.
    pub fn set_hint_text(&mut self, hint_text: bool) {
        self.hint_text = hint_text;
    }

    /// A compiler sized for a `width` x `height` viewport, with image
    /// residency budgeted explicitly.
    pub fn with_atlas_budget(width: u16, height: u16, budget: AtlasBudget) -> Self {
        let level = Level::try_detect().unwrap_or(Level::baseline());
        let images = ImageResidency::new(budget);
        Self {
            generator: StripGenerator::new(width, height, level),
            clips: ClipStack::new(),
            groups: GroupStack::new(),
            snapshots: SnapshotStack::new(),
            punches: Vec::new(),
            // Built from the residency, so the policy's page geometry is read
            // off the allocator it will pack into rather than derived a second
            // time from the same budget.
            glyph_atlas: glyph_atlas_policy(&images),
            images,
            glyphs: GlyphPrepCache::default(),
            run_routes: Vec::new(),
            next_run: 0,
            run_glyph_ids: HashSet::new(),
            hint_text: false,
            externals: ExternalExtents::new(),
        }
    }

    /// Records an externally owned texture as bound under `id` at `size`
    /// texels, answering whether the extent is one a paint can be composed
    /// against at all (see [`ExternalExtents::bind`]).
    ///
    /// Only the extent: the view the frame's passes sample is the renderer's
    /// (see [`crate::gpu::bindings`]). A caller that registers one half without
    /// the other gets a texture that draws nothing, which is why the renderer's
    /// own `bind_texture` writes both.
    pub fn bind_external_texture(&mut self, id: u64, size: (u32, u32)) -> bool {
        self.externals.bind(id, size)
    }

    /// Forgets the extent recorded for `id`, so a `SceneTexture` naming it
    /// draws nothing again.
    pub fn unbind_external_texture(&mut self, id: u64) {
        self.externals.unbind(id);
    }

    /// The externally bound extents this compiler resolves against.
    #[must_use]
    pub fn externals(&self) -> &ExternalExtents {
        &self.externals
    }

    /// The glyph entry map, for the caller that has to drain the pages this
    /// compiler's last frame dirtied.
    ///
    /// The engine produces no glyph pixels itself: `glifo` records the fills
    /// that rasterize a newly cached glyph into a per-page recorder, and
    /// [`crate::gpu::atlas::AtlasRenderer::render_pending`] replays them into
    /// the atlas array before the frame's scene pass. That replay needs the map
    /// itself, which is what this hands over.
    pub fn glyph_atlas_mut(&mut self) -> &mut GlyphAtlas {
        self.glyph_atlas.atlas_mut()
    }

    /// How many glyphs this compiler currently holds resident in the atlas.
    ///
    /// Observational, and the counter the policy's whole claim rests on: a page
    /// of static text reaches a fixed number here and stays there, while an
    /// animating size never contributes at all.
    #[must_use]
    pub fn glyph_atlas_entries(&self) -> usize {
        self.glyph_atlas.entry_count()
    }

    /// Whether any glyph may be cached at all — `false` under
    /// `FRUST_ENGINE_NO_ATLAS`.
    #[must_use]
    pub fn glyph_atlas_enabled(&self) -> bool {
        self.glyph_atlas.is_enabled()
    }

    /// The images this compiler currently holds resident.
    pub fn images(&self) -> &ImageResidency {
        &self.images
    }

    /// Record that a compiled frame's
    /// [`image_evictions`](CompiledFrame::image_evictions) and
    /// [`image_uploads`](CompiledFrame::image_uploads) have been serviced
    /// against a live atlas array.
    ///
    /// The other half of the plan seam: [`compile`](Self::compile) reports the
    /// plan without consuming it, and it goes on being reported — identically,
    /// never duplicated — until this is called. Call it only once the regions
    /// have really been written, so a frame refused after compiling keeps its
    /// uploads for the next frame that is not (see [`crate::cache::images`]'s
    /// module doc).
    pub fn acknowledge_image_plan(&mut self) {
        self.images.acknowledge_plan();
    }

    /// Whether `glifo` still holds recorded page commands, bitmap uploads or
    /// freed rectangles that have not reached the atlas array.
    ///
    /// The gate a caller drives
    /// [`crate::gpu::atlas::AtlasRenderer::render_pending`] from. Deliberately
    /// *not* [`CompiledFrame::atlas_glyph_draws`]: `glifo` dirties a page when
    /// it inserts an entry, not when a draw survives, so a run scrolled behind
    /// a clip inserts entries and records fills while contributing no draw at
    /// all. Gating on draws leaves those commands recorded — and a recorded
    /// command outliving the slot it names is old ink replayed into whichever
    /// glyph was let that rectangle next.
    ///
    /// Stays `true` across a frame the caller refuses, exactly as the image
    /// plan does, until [`acknowledge_glyph_replay`](Self::acknowledge_glyph_replay).
    #[must_use]
    pub fn glyph_replay_pending(&self) -> bool {
        self.glyph_atlas.replay_pending()
    }

    /// Record that the recorded page commands were replayed into the atlas
    /// array.
    ///
    /// Also what lets `glifo`'s eviction pass resume: while a replay is
    /// outstanding the policy defers ageing, so that no rectangle a recorded
    /// command still names can be freed and re-let underneath it (see
    /// [`crate::text::atlas_policy`]).
    pub fn acknowledge_glyph_replay(&mut self) {
        self.glyph_atlas.acknowledge_replay();
    }

    /// Whether any rectangle freed by glyph eviction is still waiting to be
    /// zeroed.
    #[must_use]
    pub fn glyph_clears_pending(&self) -> bool {
        self.glyph_atlas.has_pending_clears()
    }

    /// Record that this frame's [`CompiledFrame::glyph_clears`] were written to
    /// the atlas array.
    ///
    /// The glyph half of the same re-offer contract
    /// [`acknowledge_image_plan`](Self::acknowledge_image_plan) closes for
    /// images: [`compile`](Self::compile) reports the clears without consuming
    /// them, and goes on reporting the same ones, until a caller that really
    /// issued the writes says so. A frame compiled and then dropped therefore
    /// leaves no rectangle holding an evicted glyph's pixels.
    pub fn acknowledge_glyph_clears(&mut self) {
        self.glyph_atlas.acknowledge_clears();
    }

    /// Re-budget image residency, dropping every image currently resident.
    ///
    /// The atlas geometry is what an allocation's coordinates mean, so a change
    /// to it invalidates every rectangle already handed out: residency starts
    /// over and each image re-uploads on the next frame that draws it. A caller
    /// that owns the atlas texture must recreate it at the new extent in the
    /// same step — this is an adapter-change or start-up operation, never a
    /// per-frame one.
    pub fn set_atlas_budget(&mut self, budget: AtlasBudget) {
        self.set_image_residency(ImageResidency::new(budget));
    }

    /// Replace this compiler's image residency wholesale, dropping every image
    /// currently resident.
    ///
    /// The same invalidation [`set_atlas_budget`](Self::set_atlas_budget)
    /// carries, exposed for the residencies a budget alone cannot express — a
    /// deliberately [disabled](ImageResidency::disabled) one, or one a caller
    /// built against an adapter's own capabilities.
    ///
    /// The glyph policy is rebuilt alongside it, and for the same reason: its
    /// slots came out of the allocator being replaced, so every one of them
    /// names a rectangle of a geometry that no longer exists. Text re-caches on
    /// the next frame that draws it, exactly as an image re-uploads.
    pub fn set_image_residency(&mut self, images: ImageResidency) {
        self.images = images;
        self.glyph_atlas = glyph_atlas_policy(&self.images);
    }

    /// Compile `scene` for a `size` viewport, with `root` applied ahead of
    /// every command's own transform.
    ///
    /// # Errors
    ///
    /// [`EngineError::TargetTooLarge`] when `size` cannot be rounded up to
    /// whole tiles inside `u16`; [`EngineError::InvalidTransform`] when a
    /// composed transform is non-finite and so maps geometry to coordinates no
    /// `u16` pixel can hold; and [`EngineError::InvalidGeometry`] when a
    /// command the compiler lowers carries non-finite geometry of its own (see
    /// [`check_geometry`]). All three are refused before any strip is
    /// generated — the frame path returns errors and never panics.
    pub fn compile(
        &mut self,
        scene: &Scene,
        root: Affine,
        size: (u16, u16),
    ) -> Result<CompiledFrame, EngineError> {
        let (width, height) = size;
        check_tile_addressable(width, height)?;
        check_finite(root)?;

        // The whole scene is refused up front rather than mid-walk, so a
        // rejected frame never leaves half its draws recorded.
        for command in scene.commands() {
            if let Some(transform) = command_transform(command) {
                check_finite(root * transform)?;
            }
            check_geometry(command)?;
        }

        self.generator.reset(width, height);
        self.clips.reset();
        self.groups.reset();
        self.snapshots.reset();
        self.punches.clear();
        // Ahead of the walk, so a rectangle this frame's reap frees is
        // available to this frame's own allocations and its clear is ordered
        // ahead of their uploads.
        self.images.begin_frame();
        // Once per compiled frame, which is the cadence `glifo` ages its
        // outline entries by. Ahead of the walk rather than after it for the
        // same reason as the reap above: the glyphs this frame is about to
        // draw should be stamped as used *after* the ageing pass, not before
        // it.
        self.glyphs.maintain();

        // Phase one of the frame: every glyph run is *routed* before any of
        // them is drawn. Opened here, beside the residency's own frame, because
        // the two age against the same clock.
        // Nothing is rasterized in that phase and no slot is allocated — the
        // walk only asks the policy which runs may be cached, which is what
        // records their sizes against the animation guard before a single glyph
        // reaches `glifo`. Closing the phase hands back the rectangles last
        // frame's eviction freed, to be zeroed ahead of anything this frame
        // writes (see [`CompiledFrame::glyph_clears`]).
        self.glyph_atlas.begin_frame();
        self.classify_runs(scene, root);
        let glyph_clears = self
            .glyph_atlas
            .build(self.images.allocator_mut(), |_| {
                // Unreachable: the collect walk claims no glyph, because the
                // allocation and the rasterization of a cached glyph are
                // `glifo`'s own — it keys, packs and records every one of them
                // itself once a run reaches it with the cacher enabled. So the
                // pass this closes carries clears and nothing else.
                None
            })
            .clears;

        let mut frame = CompiledFrame {
            strips: StripStorage::new(GenerationMode::Append),
            recorder: CommandRecorder::new(width, height),
            encoded_paints: Vec::new(),
            clears: Vec::new(),
            lut_requests: Vec::new(),
            fast_rect_draws: 0,
            scissor_clips: 0,
            mask_clips: 0,
            clip_mask_strips: 0,
            image_evictions: Vec::new(),
            image_uploads: Vec::new(),
            atlas_layers: 0,
            image_draws: 0,
            skipped_images: 0,
            external_draws: 0,
            skipped_externals: 0,
            glyph_draws: 0,
            skipped_glyphs: 0,
            atlas_glyph_draws: 0,
            glyph_slots: Vec::new(),
            glyph_clears,
        };
        let mut depth = DepthCounter::new();

        for command in scene.commands() {
            self.compile_command(command, root, &mut frame, &mut depth);
        }

        self.close_open_groups(&mut frame);
        self.generate_punches(&mut frame);

        // Closes the frame the policy opened: ages `glifo`'s entry map, frees
        // whatever aged out back to the shared allocator, and takes the clear
        // rects that eviction produced — which belong to the *next* frame's
        // pass, not this one's.
        self.glyph_atlas.end_frame(self.images.allocator_mut());

        frame.scissor_clips = self.clips.scissor_clips();
        frame.mask_clips = self.clips.mask_clips();
        frame.clip_mask_strips = self.clips.mask_strips();
        // Copied rather than drained. Compiling is not the moment residency
        // becomes true — this frame can still be refused by the caller after it
        // returns, and a refused frame never reaches the atlas. The plan stays
        // pending in the residency, re-offered on every later frame, until the
        // consumer that actually wrote the regions acknowledges it through
        // [`acknowledge_image_plan`](SceneCompiler::acknowledge_image_plan).
        let (evictions, uploads) = self.images.plan();
        frame.image_evictions = evictions;
        frame.image_uploads = uploads;
        frame.atlas_layers = self.images.layers();

        Ok(frame)
    }

    fn compile_command(
        &mut self,
        command: &Command,
        root: Affine,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
    ) {
        // The frame root with any open snapshot bracket's presentation scale
        // composed ahead of it (see [`layers`]). The identity outside a
        // bracket, so this is the plain frame root for every frame that
        // records none.
        let combined = root * self.snapshots.correction();

        // Taken here rather than inside the glyph arm, and taken for every
        // glyph run whether or not it goes on to be drawn: the collect walk
        // classified one run per `Command::GlyphRun` in this same order, so
        // consuming one per `Command::GlyphRun` is what keeps the two walks in
        // step through every early return below.
        let route = match command {
            Command::GlyphRun(_) => self.take_run_route(),
            _ => None,
        };

        // A correction composes a transform the up-front walk never saw, and
        // the product can leave the finite device grid even though both
        // factors are on it. Such a command draws nothing rather than refusing
        // the frame: the refusal is the up-front walk's to make over the
        // numbers a scene actually carries, and a bracket's presentation scale
        // is not one of them. Only a command *inside* a snapshot bracket can
        // land here at all — outside one the composition is the frame root's,
        // which that walk already checked.
        //
        // Drawing nothing is not the same as doing nothing: a command that
        // opens a bracket still has to open one, or its pop would close the
        // bracket around it instead. So a bracket lands blocked rather than
        // absent, which draws nothing inside it and balances its own pop.
        let on_grid = command_on_grid(command, combined);
        if !on_grid {
            match command {
                Command::PushClip { .. }
                | Command::PushClipRounded { .. }
                | Command::PushLayer { .. } => self.open_blocked_group(),
                // The correction is the outermost bracket's, so a bracket
                // reaching here is a nested one, whose presentation is ignored
                // anyway; only its depth has to be counted. The substitution is
                // still made through [`snapshot_entry`] rather than inline,
                // because it is the collect walk's to make identically (see
                // [`Self::classify_runs`]).
                Command::PushSnapshot {
                    rect,
                    scale,
                    transform,
                    ..
                } => {
                    let (scale, transform) = snapshot_entry(on_grid, *scale, *transform);
                    self.snapshots.enter(*rect, scale, transform);
                }
                _ => {}
            }
            return;
        }

        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => {
                let transform = combined * *transform;

                if let Some(device_rect) = fast_rect(*rect, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        PaintSource::Brush(brush),
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        PaintSource::Brush(brush),
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                rect.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            Command::RoundedRect {
                rect,
                radii,
                brush,
                transform,
            } => {
                let transform = combined * *transform;
                let shape = RoundedRect::from_rect(*rect, rounded_rect_radii(*radii));

                self.record(
                    frame,
                    depth,
                    PaintSource::Brush(brush),
                    transform,
                    |generator, storage, clip| {
                        generator.generate_filled_path(
                            shape.path_elements(FLATTEN_TOLERANCE),
                            Fill::NonZero,
                            transform,
                            None,
                            storage,
                            clip,
                        );
                    },
                );
            }
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => {
                let transform = combined * *transform;
                let line = Line::new(*p0, *p1);
                let stroke = round_stroke(*width);

                self.record(
                    frame,
                    depth,
                    PaintSource::Brush(brush),
                    transform,
                    |generator, storage, clip| {
                        generator.generate_stroked_path(
                            line.path_elements(FLATTEN_TOLERANCE),
                            &stroke,
                            transform,
                            None,
                            storage,
                            clip,
                        );
                    },
                );
            }
            Command::Path {
                path,
                style,
                brush,
                transform,
            } => {
                let transform = combined * *transform;

                match style {
                    PathStyle::Fill => {
                        self.record(
                            frame,
                            depth,
                            PaintSource::Brush(brush),
                            transform,
                            |generator, storage, clip| {
                                generator.generate_filled_path(
                                    path.iter(),
                                    Fill::NonZero,
                                    transform,
                                    None,
                                    storage,
                                    clip,
                                );
                            },
                        );
                    }
                    PathStyle::Stroke { width, dash } => {
                        let stroke = round_stroke(*width);
                        // A dash pattern is expanded into its own sub-paths
                        // before the stroker runs, the same lowering the
                        // display list's other consumers apply: the pattern
                        // never reaches a backend's own dash support, so every
                        // rasterizer sees the identical geometry.
                        let dashed = match dash {
                            Some(dash) if dash.is_effective() => Some(dash_path(path, *dash)),
                            _ => None,
                        };

                        match &dashed {
                            Some(dashed) => {
                                self.record(
                                    frame,
                                    depth,
                                    PaintSource::Brush(brush),
                                    transform,
                                    |generator, storage, clip| {
                                        generator.generate_stroked_path(
                                            dashed.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            clip,
                                        );
                                    },
                                );
                            }
                            None => {
                                self.record(
                                    frame,
                                    depth,
                                    PaintSource::Brush(brush),
                                    transform,
                                    |generator, storage, clip| {
                                        generator.generate_stroked_path(
                                            path.iter(),
                                            &stroke,
                                            transform,
                                            None,
                                            storage,
                                            clip,
                                        );
                                    },
                                );
                            }
                        }
                    }
                }
            }
            Command::PushClip { rect, transform } => {
                let transform = combined * *transform;
                self.clips.push_rect(*rect, transform, &mut self.generator);
                self.groups.push_clip(transform.transform_rect_bbox(*rect));
            }
            Command::PushClipRounded {
                rect,
                radii,
                transform,
            } => {
                let transform = combined * *transform;
                self.clips.push_rounded(
                    *rect,
                    rounded_rect_radii(*radii),
                    transform,
                    &mut self.generator,
                );
                self.groups.push_clip(transform.transform_rect_bbox(*rect));
            }
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => {
                self.open_layer(frame, *rect, *alpha, combined * *transform);
            }
            // One bracket stack serves all three kinds, so whichever pop
            // arrives closes the innermost open bracket (see [`layers`]). A pop
            // with nothing open is ignored: an unbalanced widget tree must not
            // be able to lift a bracket a sibling still relies on.
            Command::PopClip | Command::PopLayer => self.close_group(frame),
            Command::ClearRect { rect, transform } => {
                let transform = combined * *transform;
                // Hoisted here rather than at the end of the frame because
                // this is the only point the brackets confining it are still
                // open; its coverage is generated once the frame's draws are
                // done (see [`clear`]).
                let punch = clear::punch_rect(*rect, transform, self.groups.bounds());
                if let Some(device) = punch {
                    self.punches.push(StagedPunch {
                        device,
                        depth: depth.advance(),
                    });
                }
            }
            Command::PushSnapshot {
                rect,
                alpha,
                scale,
                transform,
                ..
            } => {
                if self.snapshots.enter(*rect, *scale, *transform) {
                    // The bracket's own correction is the one that applies to
                    // the layer it opens, so the transform is recomposed here
                    // rather than reusing `combined` from before the entry.
                    let corrected = root * self.snapshots.correction() * *transform;
                    if *alpha < 1.0 && check_finite(corrected).is_ok() {
                        self.open_layer(frame, *rect, *alpha, corrected);
                        self.snapshots.record_layer(self.groups.depth());
                    }
                }
            }
            Command::PopSnapshot => {
                if self.snapshots.leave(self.groups.depth()) {
                    self.close_group(frame);
                }
            }
            Command::Image {
                data,
                dest,
                transform,
            } => {
                let transform = combined * *transform;
                let source = PaintSource::Image { data, dest: *dest };

                // An image is its destination rectangle's coverage under an
                // image paint — the same two rectangle paths a solid fill
                // takes, so a pixel-aligned image costs no flattening either.
                if let Some(device_rect) = fast_rect(*dest, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                dest.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
                transform,
            } => {
                let transform = combined * *transform;
                let source = PaintSource::BlurredRect {
                    rect: *rect,
                    radii: *radii,
                    std_dev: *std_dev,
                    color: *color,
                };
                // The strip generator rasterizes the padded bounding
                // rectangle, not `rect` itself and not a rounded shape — see
                // [`blur_rrect`]'s module doc for why. It takes the same fast
                // rectangle path a fill or an image does whenever that padded
                // rectangle lands pixel-aligned under `transform`.
                let bounds = inflated_bounds(*rect, *std_dev);

                if let Some(device_rect) = fast_rect(bounds, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                bounds.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
            Command::GlyphRun(run) => {
                self.compile_glyph_run(run, combined * run.transform, route, frame, depth);
            }
            // Recognised but not yet compiled. Named rather than caught by a
            // wildcard so a command added to the display list fails to compile
            // here instead of silently vanishing from every frame. Tracked as
            // `engine-shader-quad-unwired` in docs/LIMITATIONS.md until the
            // GPU-seam work wires this command through; the drop itself is
            // unchanged, just no longer silent.
            Command::ShaderQuad { .. } => note_shader_quad_skip(),
            Command::SceneTexture {
                id,
                dest,
                transform,
            } => {
                let transform = combined * *transform;
                let source = PaintSource::SceneTexture {
                    id: *id,
                    dest: *dest,
                };

                // An externally owned texture is its destination rectangle's
                // coverage under an image paint that samples the caller's
                // texture rather than the atlas — the same two rectangle paths
                // `Command::Image` takes, so a pixel-aligned one costs no
                // flattening either.
                if let Some(device_rect) = fast_rect(*dest, transform) {
                    let recorded = self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_rect_fast(&device_rect, storage, clip);
                        },
                    );
                    if recorded {
                        frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
                    }
                } else {
                    self.record(
                        frame,
                        depth,
                        source,
                        transform,
                        |generator, storage, clip| {
                            generator.generate_filled_path(
                                dest.path_elements(FLATTEN_TOLERANCE),
                                Fill::NonZero,
                                transform,
                                None,
                                storage,
                                clip,
                            );
                        },
                    );
                }
            }
        }
    }

    /// Route every glyph run the scene records, in recording order.
    ///
    /// The whole of the frame's collect phase. It is a walk of its own rather
    /// than a question asked inside the draw walk because the answer for one
    /// run depends on what the *font* has been drawn at recently, and a policy
    /// that learned a size only as it drew it would route the first run of a
    /// changing frame as settled and the second as animating — the two halves
    /// of one line of text taking different paths.
    ///
    /// Every run is classified, including ones the draw walk will refuse: the
    /// refusals it makes (an empty run, a blocked clip, an unreadable face)
    /// are not size observations, and a size drawn on a frame is a size drawn
    /// on that frame whatever else happens to it.
    ///
    /// The walk carries a [`SnapshotStack`] of its own for one reason: a run's
    /// route depends on the scale in its *device* transform (see
    /// [`crate::text::atlas_policy`]), and inside a `PushSnapshot` bracket that
    /// transform carries the bracket's presentation scale as well as the frame
    /// root. Classifying against `root * run.transform` alone would answer for
    /// a size the run is not drawn at. Only the correction is tracked here —
    /// the bracket's *layers* are the draw walk's to open, and this walk opens
    /// nothing.
    ///
    /// A bracket is entered on the draw walk's exact terms, through the same
    /// [`command_on_grid`] check and the same [`snapshot_entry`] substitution
    /// it uses, so the correction the two walks carry is one decision made
    /// twice rather than two decisions that happen to agree.
    fn classify_runs(&mut self, scene: &Scene, root: Affine) {
        self.run_routes.clear();
        self.next_run = 0;

        let mut snapshots = SnapshotStack::new();
        for command in scene.commands() {
            match command {
                Command::PushSnapshot {
                    rect,
                    scale,
                    transform,
                    ..
                } => {
                    // The draw walk's own entry, made here on exactly its
                    // terms: [`command_on_grid`] is the check it asks and
                    // [`snapshot_entry`] is the substitution it makes. A
                    // bracket it enters neutrally installs no correction, so a
                    // walk that entered it with the recorded pair would
                    // classify every run inside against a device transform
                    // nothing is ever drawn through — a route decided for one
                    // magnitude and a draw made at another.
                    let combined = root * snapshots.correction();
                    let on_grid = command_on_grid(command, combined);
                    let (scale, transform) = snapshot_entry(on_grid, *scale, *transform);
                    snapshots.enter(*rect, scale, transform);
                }
                Command::PopSnapshot => {
                    // The group depth a real close would be tested against is
                    // the draw walk's; nothing here closes a group, so zero is
                    // the honest answer and the return value is unused.
                    snapshots.leave(0);
                }
                Command::GlyphRun(run) => {
                    // The context colour `glifo` would resolve a COLR layer
                    // against — the run's own brush when it is solid, black
                    // otherwise, which is the same answer
                    // `EngineGlyphSink::get_context_color` gives it.
                    let context_color = match context_paint(&run.brush) {
                        vello_common::paint::PaintType::Solid(color) => color,
                        _ => peniko::color::palette::css::BLACK,
                    };
                    let key = RunKey::for_run(
                        run,
                        root * snapshots.correction() * run.transform,
                        self.hint_text,
                        font_has_color_glyphs(run.font.font()),
                        context_color,
                        &mut self.run_glyph_ids,
                    );
                    let route = self.glyph_atlas.classify_run(&key);
                    self.run_routes.push(route);
                }
                _ => {}
            }
        }
    }

    /// The next run's route, or `None` once the collect walk's answers are
    /// exhausted.
    ///
    /// `None` is the conservative answer rather than an error: a run with no
    /// recorded route is drawn as outlines, which is correct pixels by the path
    /// the engine has always used.
    ///
    /// The route is re-tested against *live* glyph residency on the way out
    /// (see [`crate::text::atlas_policy::AtlasPolicy::admit_run`]). The collect
    /// walk answered every run of this frame from the population the frame
    /// opened with, because `glifo` inserts nothing until the draw walk reaches
    /// the run; without this second test a frame one entry below the budget
    /// would admit every run it carries and overshoot by as much as one frame's
    /// whole text. Here the population is the real one — every earlier run of
    /// this same frame has already inserted, and the re-test charges those
    /// insertions before it answers — so the bound holds within a frame and not
    /// merely across frames. It can only ever *narrow* an answer, which is the
    /// outline path: correct pixels, and the only direction that is safe to
    /// decide late.
    fn take_run_route(&mut self) -> Option<RunRoute> {
        let route = self.run_routes.get(self.next_run).copied();
        self.next_run = self.next_run.saturating_add(1);
        route.map(|route| self.glyph_atlas.admit_run(route))
    }

    /// Draw one glyph run: its brush encoded once, then every glyph's outline
    /// rasterized under the active clip (see [`crate::text`]).
    ///
    /// `transform` is the run's own transform composed with the frame root.
    /// The brush is encoded against it once for the whole run rather than once
    /// per glyph, because that transform *is* the paint's placement — a glyph
    /// moves the outline, never the paint behind it — so a gradient-brushed
    /// line of text costs one encoded entry and one colour ramp.
    ///
    /// A run whose brush cannot be encoded draws nothing, on the same terms an
    /// image draw the atlas refuses does: the refusal is already counted in
    /// [`CompiledFrame::skipped_images`] by the encoding, and every glyph in
    /// the run simply goes missing rather than being painted with a
    /// substitute. An image-brushed run is likewise counted as the one image
    /// draw its single encoding is, not as one per glyph.
    ///
    /// A run whose font cannot be read is refused the same way and for a
    /// harder reason: the text backend's font gate is what keeps a blob that
    /// is not a font off the frame path at all (see [`crate::text`]).
    fn compile_glyph_run(
        &mut self,
        run: &GlyphRun,
        transform: Affine,
        route: Option<RunRoute>,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
    ) {
        // All three checked before the brush is encoded, so a run that can
        // draw nothing leaves no orphan entry in the frame's encoded-paint
        // table and no ramp request for a gradient nothing paints with — the
        // same rule [`record`](Self::record) keeps for a shape.
        if run.glyphs.is_empty() || self.clips.blocks_everything() {
            return;
        }
        if !font_is_readable(run.font.font()) {
            note_font_skip();
            let glyphs = u32::try_from(run.glyphs.len()).unwrap_or(u32::MAX);
            frame.skipped_glyphs = frame.skipped_glyphs.saturating_add(glyphs);
            return;
        }

        let Some(paint) = self.encode_paint(PaintSource::Brush(&run.brush), transform, frame)
        else {
            return;
        };

        // Read out ahead of the destructure below: `hint_text` is `Copy`, and
        // reading it through `self` after the destructure moved out its other
        // fields would fight the borrow checker for no reason.
        let hint_text = self.hint_text;
        // Destructured rather than passed as `self`, because the sink borrows
        // the generator and the clip stack mutably while the glyph caches, the
        // entry map and the shared allocator are borrowed mutably alongside
        // them — five disjoint fields of one struct.
        let Self {
            generator,
            clips,
            glyphs,
            images,
            glyph_atlas,
            ..
        } = self;
        // The policy's decision, turned into the borrow `glifo` caches
        // through. A run it refused reaches `glifo` with no cache at all, which
        // is the outline path unchanged rather than a cache that declines every
        // lookup — those are the same pixels but not the same work.
        let cacher = match route {
            Some(RunRoute::Atlas(_)) => {
                AtlasCacher::Enabled(glyph_atlas.atlas_mut(), images.allocator_mut())
            }
            Some(RunRoute::Outline(_)) | None => AtlasCacher::Disabled,
        };
        let outcome = lower_glyph_run(
            run,
            transform,
            paint,
            &run.brush,
            hint_text,
            cacher,
            GlyphRunTargets {
                generator,
                clips,
                prep: glyphs,
                frame,
                depth,
            },
        );

        frame.glyph_draws = frame.glyph_draws.saturating_add(outcome.drawn);
        frame.skipped_glyphs = frame.skipped_glyphs.saturating_add(outcome.skipped);
    }

    /// Open a layer bracket: its rectangle's clip, and — below full opacity —
    /// a recorded layer for the scheduler to give a page of its own.
    ///
    /// The clip and the bracket are pushed together and unconditionally, which
    /// is what keeps the two stacks in step for [`close_group`](Self::close_group).
    fn open_layer(&mut self, frame: &mut CompiledFrame, rect: Rect, alpha: f32, transform: Affine) {
        self.clips.push_rect(rect, transform, &mut self.generator);

        let isolated = layers::lower_layer(alpha) == LayerLowering::Isolated;
        if isolated {
            frame.recorder.push_layer(layers::layer_props(alpha), None);
        }
        self.groups
            .push_layer(transform.transform_rect_bbox(rect), isolated);
    }

    /// Open a bracket that admits nothing, for a push whose transform does not
    /// land on the device grid.
    ///
    /// A degenerate rectangle under the identity, rather than the push's own
    /// geometry under its own transform: the point is to reach an empty
    /// scissor without handing the flattener a transform it cannot subdivide
    /// against, which is the very thing that made this push unusable.
    fn open_blocked_group(&mut self) {
        self.clips
            .push_rect(Rect::ZERO, Affine::IDENTITY, &mut self.generator);
        self.groups.push_clip(Rect::ZERO);
    }

    /// Close the innermost open bracket, undoing exactly what opened it.
    ///
    /// The recording is only popped when this bracket is the one that pushed
    /// it *and* the recording agrees a layer is open — the recorder's own pop
    /// panics on an empty layer stack, and the frame path returns errors
    /// rather than panicking (E17).
    fn close_group(&mut self, frame: &mut CompiledFrame) {
        let Some(group) = self.groups.pop() else {
            return;
        };
        if group.closes_clip() {
            self.clips.pop();
        }
        if group.closes_layer() && frame.recorder.has_layers() {
            frame.recorder.pop_layer();
        }
    }

    /// Close every bracket the display list left open at the end of the frame.
    ///
    /// A recorded layer that is never popped has no bounds — the recorder
    /// computes them at the pop — so an unbalanced push would otherwise leave
    /// the scheduler a layer it cannot place. Closing here is the same policy
    /// an unbalanced pop gets, applied at the other end.
    fn close_open_groups(&mut self, frame: &mut CompiledFrame) {
        while !self.groups.is_empty() {
            self.close_group(frame);
        }
        self.snapshots.reset();
    }

    /// Generate the coverage for every punch the frame hoisted.
    ///
    /// Runs after the walk, so the strips land past every draw's own range and
    /// no draw references them. The punch is rasterized at the frame root
    /// under no clip at all — being hoisted out of its brackets is exactly
    /// what the confinement in [`clear::punch_rect`] already accounted for —
    /// and takes the fast rectangle path whenever its edges fall on whole
    /// pixels, which is what makes a pixel-aligned punch pixel-exact however
    /// its edges fall inside a tile.
    fn generate_punches(&mut self, frame: &mut CompiledFrame) {
        for index in 0..self.punches.len() {
            let Some(punch) = self.punches.get(index).copied() else {
                continue;
            };

            let start = frame.strips.strips.len();
            match fast_rect(punch.device, Affine::IDENTITY) {
                Some(device) => {
                    self.generator
                        .generate_filled_rect_fast(&device, &mut frame.strips, None);
                }
                None => {
                    self.generator.generate_filled_path(
                        punch.device.path_elements(FLATTEN_TOLERANCE),
                        Fill::NonZero,
                        Affine::IDENTITY,
                        None,
                        &mut frame.strips,
                        None,
                    );
                }
            }

            let strip_range = start..frame.strips.strips.len();
            if strip_range.is_empty() {
                continue;
            }

            frame.clears.push(ClearPunch {
                strip_range,
                bounds: clear::device_bounds(punch.device),
                depth: punch.depth,
            });
        }
    }

    /// Run `generate` under the active clip, then record whatever strips
    /// survived as one draw painted from `source` under `transform`.
    ///
    /// `generate` is handed the clip stack's coverage mask to pass on to the
    /// strip generator, which is what intersects a mask clip while the draw's
    /// own coverage is produced; the scissor is applied afterwards, to the run
    /// the generator appended. A scissor admitting nothing skips generation
    /// entirely rather than generating coverage to throw away.
    ///
    /// Returns whether a draw was recorded. A generator call that produced no
    /// strips (fully culled, clipped away, degenerate, or empty geometry)
    /// records nothing and consumes no depth, so a frame's depths stay dense
    /// over the draws that actually exist.
    ///
    /// The paint is encoded only once the strips are known to be non-empty, so
    /// a culled draw leaves no orphan entry in the frame's encoded-paint table
    /// and no ramp request for a gradient nothing paints with. An image the
    /// atlas refuses arrives *after* that point, so its coverage is rolled back
    /// to where the generator started rather than left behind as strips no draw
    /// references.
    fn record<F>(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        source: PaintSource<'_>,
        transform: Affine,
        generate: F,
    ) -> bool
    where
        F: FnOnce(&mut StripGenerator, &mut StripStorage, Option<PathDataRef<'_>>),
    {
        if self.clips.blocks_everything() {
            return false;
        }

        let start = frame.strips.strips.len();
        let alpha_start = frame.strips.alphas.len();
        generate(&mut self.generator, &mut frame.strips, self.clips.mask());
        self.clips.clip_run(&mut frame.strips, start, alpha_start);
        let strip_range = start..frame.strips.strips.len();

        if strip_range.is_empty() {
            return false;
        }

        let Some(paint) = self.encode_paint(source, transform, frame) else {
            frame.strips.strips.truncate(start);
            frame.strips.alphas.truncate(alpha_start);
            return false;
        };

        let draw = EngineDraw::new(paint, depth.advance(), strip_range.clone());
        frame
            .recorder
            .push_draw(draw, &frame.strips.strips[strip_range]);
        true
    }

    /// Encode `source` into the paint a draw carries, or `None` when the paint
    /// cannot be resolved and the draw is to be dropped.
    ///
    /// A solid and a gradient are always encodable (a degenerate gradient falls
    /// back to a solid). The two that can answer `None` are an image, which
    /// needs atlas space the residency may refuse, and an externally bound
    /// texture, whose id may name nothing registered.
    fn encode_paint(
        &mut self,
        source: PaintSource<'_>,
        transform: Affine,
        frame: &mut CompiledFrame,
    ) -> Option<vello_common::paint::Paint> {
        let encoded = match source {
            PaintSource::Brush(Brush::Image(brush)) => encode_image_brush(
                brush,
                transform,
                &mut frame.encoded_paints,
                &mut self.images,
            ),
            PaintSource::Brush(brush) => {
                let encoding = encode_brush(brush, transform, &mut frame.encoded_paints);
                frame.lut_requests.extend(encoding.lut_request);
                return Some(encoding.paint);
            }
            PaintSource::Image { data, dest } => encode_image_command(
                data,
                dest,
                transform,
                &mut frame.encoded_paints,
                &mut self.images,
            ),
            PaintSource::SceneTexture { id, dest } => {
                // Its own error type and its own counters, so it returns here
                // rather than joining the atlas-residency match below: nothing
                // is made resident and nothing is uploaded — the texels are
                // the caller's and are already on the device.
                return match encode_scene_texture(
                    id,
                    dest,
                    transform,
                    &mut self.externals,
                    &mut frame.encoded_paints,
                ) {
                    Ok(encoding) => {
                        frame.external_draws = frame.external_draws.saturating_add(1);
                        Some(encoding.paint)
                    }
                    Err(_) => {
                        frame.skipped_externals = frame.skipped_externals.saturating_add(1);
                        None
                    }
                };
            }
            PaintSource::BlurredRect {
                rect,
                radii,
                std_dev,
                color,
            } => {
                // Unlike an image, this can never be refused (see
                // [`encode_blurred_rounded_rect`]'s doc), so it returns
                // straight away rather than joining the fallible match below.
                let paint = encode_blurred_rounded_rect(
                    rect,
                    radii,
                    std_dev,
                    color,
                    transform,
                    &mut frame.encoded_paints,
                );
                return Some(paint);
            }
        };

        match encoded {
            Ok(encoding) => {
                frame.image_draws = frame.image_draws.saturating_add(1);
                Some(encoding.paint)
            }
            Err(skip) => {
                frame.skipped_images = frame.skipped_images.saturating_add(1);
                note_image_skip(skip);
                None
            }
        }
    }
}

/// What a draw is painted with, as the walk hands it to
/// [`SceneCompiler::record`].
///
/// An image is not a [`Brush`] in the display list — [`Command::Image`] carries
/// its pixels and a destination rectangle directly — so the two arrive by
/// different routes and are distinguished here rather than by forcing one into
/// the shape of the other.
enum PaintSource<'a> {
    /// A solid, gradient or image brush recorded on a shape command.
    Brush(&'a Brush),
    /// A [`Command::Image`]'s pixels scaled to fill `dest`.
    Image {
        /// The decoded image to make resident.
        data: &'a ImageData,
        /// The destination rectangle, in the command's own coordinate space.
        dest: Rect,
    },
    /// A [`Command::SceneTexture`]'s externally bound texture scaled to fill
    /// `dest`.
    SceneTexture {
        /// The opaque id the display list names the texture by.
        id: u64,
        /// The destination rectangle, in the command's own coordinate space.
        dest: Rect,
    },
    /// A [`Command::BlurredRoundedRect`]'s shadow parameters, in the
    /// command's own (pre-transform) coordinate space.
    BlurredRect {
        /// The un-padded rectangle the shadow is cast from.
        rect: Rect,
        /// Per-corner radii, collapsed to their largest at encode time (see
        /// [`blur_rrect`]).
        radii: CornerRadii,
        /// The blur's standard deviation.
        std_dev: f64,
        /// The shadow's base colour.
        color: Color,
    },
}

/// Report an image the atlas refused.
///
/// The first refusal in a process is a warning, because a blank image where one
/// was expected is otherwise invisible; the rest are debug, because a scene
/// that keeps drawing a refused image would repeat the message every frame.
fn note_image_skip(skip: ImageSkip) {
    IMAGE_SKIP_WARNING.call_once(|| {
        log::warn!("image draw skipped: {skip} (further skips are logged at debug level)");
    });
    log::debug!("image draw skipped: {skip}");
}

/// Report a glyph run whose font could not be read.
///
/// The same once-warning-then-debug shape [`note_image_skip`] uses, and for
/// the same reason: a missing line of text is otherwise invisible, while a
/// scene that keeps drawing against an unloaded font would repeat the message
/// every frame.
fn note_font_skip() {
    FONT_SKIP_WARNING.call_once(|| {
        log::warn!(
            "glyph run skipped: its font blob is not a readable face \
             (further skips are logged at debug level)"
        );
    });
    log::debug!("glyph run skipped: its font blob is not a readable face");
}

/// Report that the compiler dropped a [`Command::ShaderQuad`].
///
/// Latched to once per process rather than following [`note_image_skip`] and
/// [`note_font_skip`]'s warn-then-debug shape: a shader quad's absence is a
/// standing, known gap (see `engine-shader-quad-unwired` in
/// docs/LIMITATIONS.md) rather than a per-frame refusal worth re-reporting at
/// debug level on every later drop.
fn note_shader_quad_skip() {
    SHADER_QUAD_SKIP_WARNING.call_once(|| {
        log::warn!(
            "ShaderQuad command dropped: the engine compiler does not draw shader quads yet \
             (see docs/LIMITATIONS.md `engine-shader-quad-unwired`; logged once per process)"
        );
    });
}

/// The transform a command carries, or `None` for one that carries none.
fn command_transform(command: &Command) -> Option<Affine> {
    match command {
        Command::FillRect { transform, .. }
        | Command::RoundedRect { transform, .. }
        | Command::Line { transform, .. }
        | Command::PushClip { transform, .. }
        | Command::PushClipRounded { transform, .. }
        | Command::Image { transform, .. }
        | Command::BlurredRoundedRect { transform, .. }
        | Command::PushLayer { transform, .. }
        | Command::ClearRect { transform, .. }
        | Command::Path { transform, .. }
        | Command::ShaderQuad { transform, .. }
        | Command::SceneTexture { transform, .. }
        | Command::PushSnapshot { transform, .. } => Some(*transform),
        Command::GlyphRun(run) => Some(run.transform),
        Command::PopClip | Command::PopLayer | Command::PopSnapshot => None,
    }
}

/// Whether `command`'s own transform still lands on the finite device grid once
/// `combined` — the frame root with any open snapshot bracket's correction — is
/// composed ahead of it.
///
/// Asked by both of the frame's walks, from one place, because they have to ask
/// it the same way. The draw walk draws nothing for a command that answers
/// `false` and enters a `PushSnapshot` neutrally instead (see
/// [`snapshot_entry`]); the collect walk classifies against the transform that
/// entry implies. Two copies of this expression could drift a coefficient
/// apart and route a run for a device size it is never drawn at.
///
/// A command carrying no transform of its own is on the grid trivially: there
/// is nothing to compose.
fn command_on_grid(command: &Command, combined: Affine) -> bool {
    command_transform(command).is_none_or(|transform| check_finite(combined * transform).is_ok())
}

/// The presentation scale and transform a `PushSnapshot` bracket is entered
/// with: the recorded pair on the grid, and the neutral pair off it.
///
/// The neutral pair is what makes an off-grid bracket *inert* rather than
/// absent — it still has a depth to count and a pop to balance, but it installs
/// no correction, so nothing inside it is drawn through a transform the frame
/// refused. Both walks substitute through this one function so that the route
/// a run is given and the transform it is drawn through can never be decided
/// from different magnitudes.
fn snapshot_entry(on_grid: bool, scale: f64, transform: Affine) -> (f64, Affine) {
    if on_grid {
        (scale, transform)
    } else {
        (1.0, Affine::IDENTITY)
    }
}

/// Refuse a viewport whose tile-snapped extent would not fit in `u16`.
///
/// The recorder snaps the scene size up to whole tiles, and that rounding is
/// checked arithmetic upstream — an extent within three pixels of `u16::MAX`
/// has no representable tile-aligned bound. Refusing it here is what keeps the
/// frame path free of that panic.
fn check_tile_addressable(width: u16, height: u16) -> Result<(), EngineError> {
    let addressable = width.checked_next_multiple_of(Tile::WIDTH).is_some()
        && height.checked_next_multiple_of(Tile::HEIGHT).is_some();

    if addressable {
        Ok(())
    } else {
        Err(EngineError::TargetTooLarge)
    }
}

/// Refuse a transform that maps geometry off the finite device grid.
///
/// A non-finite coefficient (`NaN` from a degenerate inverse, an infinity from
/// an overflowed scale) sends every coordinate it touches outside the `u16`
/// pixel range the strip pipeline addresses, so the frame is refused rather
/// than rasterized into whatever the downstream float-to-integer conversions
/// happen to saturate to.
fn check_finite(transform: Affine) -> Result<(), EngineError> {
    if transform.as_coeffs().iter().all(|c| c.is_finite()) {
        Ok(())
    } else {
        Err(EngineError::InvalidTransform)
    }
}

/// Refuse a command whose own geometry is non-finite.
///
/// A finite transform is not enough on its own: a `NaN` corner radius, an
/// infinite rectangle extent, a `NaN` control point or stroke width all reach
/// the flattener and the stroker as they were recorded, and neither of those
/// bails on a non-finite number. They subdivide against it — a rounded rect of
/// unbounded extent with a `NaN` radius never finishes at all, and a `NaN`
/// stroke width buys hundreds of milliseconds and megabytes of scratch to emit
/// no coverage whatsoever. Refusing here, in the same up-front walk the
/// transforms are checked in, is what bounds the frame path's work by the
/// scene rather than by the arithmetic.
///
/// Only the commands the compiler actually lowers are checked. A command it
/// recognises and skips contributes no geometry to the frame, so refusing the
/// whole frame over one would draw *nothing* where skipping draws less — the
/// weaker outcome. A clip is checked because it *is* lowered: its rectangle and
/// radii decide whether the clip scissors or masks and where its edges land, so
/// a non-finite one is refused on the same terms as a fill's, matching the
/// refusal its transform already drew. A layer and a snapshot bracket are
/// checked on those same terms, and for the same reason: each lowers its
/// rectangle through the clip stack, so a non-finite one reaches the flattener
/// exactly as a clip's would. Their `alpha` and `scale` are checked alongside
/// it because neither is decoration — an alpha decides whether the layer
/// isolates, and a scale composes a transform every command inside the bracket
/// is drawn under. A clear is checked because its rectangle *is* the coverage
/// it erases with. An image's destination rectangle is checked on those same
/// terms — it is both the coverage the image paints through and the scale its
/// natural-to-destination transform is derived from, so a non-finite one would
/// reach the flattener and the paint encoding alike. A blurred rounded
/// rectangle's rectangle, corner radii and standard deviation are checked on
/// those same terms — together they decide the padded rectangle the strip
/// generator rasterizes ([`blur_rrect::inflated_bounds`]) and the falloff the
/// fragment shader evaluates from the encoded paint
/// ([`blur_rrect::encode_blurred_rounded_rect`]), so a non-finite one would
/// reach the flattener and the paint encoding exactly as a non-finite rounded
/// rect's radii already do. A lowered command carrying no geometry at all
/// ([`Command::PopClip`] and its two siblings) has nothing to check and sits
/// with the skipped group. A glyph run's font size and per-glyph positions are
/// checked for the same reason a stroke width is: they are not decoration
/// either, but the numbers every glyph's own draw transform is derived from
/// ([`crate::text`]), so a non-finite one reaches the flattener as a transform
/// no subdivision converges against. The scan is per glyph and therefore the
/// one check here whose cost grows with a command's contents — bounded by the
/// glyph count the run already carries, and paid once per frame rather than
/// once per glyph drawn.
///
/// The match is exhaustive over every [`Command`] variant, the same as
/// [`SceneCompiler::compile_command`]'s: a variant added to the enum fails to
/// compile here until it is placed in the checked group or the unchecked one.
/// Moving a variant *between* those two groups is not itself compiler-enforced
/// — the match stays exhaustive either way — so that half of the discipline
/// still has to be kept by hand alongside `compile_command`.
fn check_geometry(command: &Command) -> Result<(), EngineError> {
    let finite = match command {
        Command::FillRect { rect, .. } => rect.is_finite(),
        Command::RoundedRect { rect, radii, .. } => rect.is_finite() && radii_are_finite(*radii),
        Command::Line { p0, p1, width, .. } => {
            p0.is_finite() && p1.is_finite() && width.is_finite()
        }
        Command::Path { path, style, .. } => path.is_finite() && style_is_finite(style),
        Command::PushClip { rect, .. } => rect.is_finite(),
        Command::PushClipRounded { rect, radii, .. } => {
            rect.is_finite() && radii_are_finite(*radii)
        }
        Command::PushLayer { rect, alpha, .. } => rect.is_finite() && alpha.is_finite(),
        Command::ClearRect { rect, .. } => rect.is_finite(),
        Command::PushSnapshot {
            rect, alpha, scale, ..
        } => rect.is_finite() && alpha.is_finite() && scale.is_finite(),
        Command::Image { dest, .. } => dest.is_finite(),
        Command::SceneTexture { dest, .. } => dest.is_finite(),
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            ..
        } => rect.is_finite() && radii_are_finite(*radii) && std_dev.is_finite(),
        Command::GlyphRun(run) => glyph_run_is_finite(run),
        // Carrying no geometry of their own — see above.
        Command::PopClip
        | Command::PopLayer
        | Command::ShaderQuad { .. }
        | Command::PopSnapshot => true,
    };

    if finite {
        Ok(())
    } else {
        Err(EngineError::InvalidGeometry)
    }
}

/// Whether a glyph run's own numbers are finite.
///
/// The font size and every glyph position, because those are exactly the run's
/// numbers that end up inside a transform: `glifo` absorbs the font size into
/// each glyph's draw transform and translates that transform by the glyph's
/// position, so either one non-finite produces a transform the flattener
/// subdivides against forever. The font itself is not checked — a malformed or
/// unreadable face yields no outline and draws nothing, which is a missing
/// glyph rather than an unbounded loop.
fn glyph_run_is_finite(run: &GlyphRun) -> bool {
    run.font_size.is_finite()
        && run
            .glyphs
            .iter()
            .all(|glyph| glyph.x.is_finite() && glyph.y.is_finite())
}

/// Whether every corner radius is finite.
fn radii_are_finite(radii: CornerRadii) -> bool {
    radii.top_left.is_finite()
        && radii.top_right.is_finite()
        && radii.bottom_right.is_finite()
        && radii.bottom_left.is_finite()
}

/// Whether a path style's own numbers are finite.
///
/// The dash lengths and phase are checked even though
/// [`DashPattern::is_effective`] would strike a non-finite pattern out and
/// stroke solid: a frame the compiler refuses for a `NaN` stroke width would
/// otherwise be accepted for a `NaN` dash phase, and one contract over every
/// number a *lowered* command carries is the one a caller can hold in their
/// head — not a claim about a command [`check_geometry`] skips rather than
/// lowers, whose numbers this function never sees.
///
/// An effective dash pattern is checked further, past its own fields: see
/// [`dash_cycle_is_normalizable`].
fn style_is_finite(style: &PathStyle) -> bool {
    match style {
        PathStyle::Fill => true,
        PathStyle::Stroke { width, dash } => {
            let dash_finite = match dash {
                Some(dash) => {
                    dash.on.is_finite()
                        && dash.off.is_finite()
                        && dash.phase.is_finite()
                        && dash_cycle_is_normalizable(dash)
                }
                None => true,
            };
            width.is_finite() && dash_finite
        }
    }
}

/// Whether a dash pattern's derived cycle survives kurbo's own normalization
/// arithmetic, so `kurbo::dash` terminates instead of spinning forever.
///
/// [`DashPattern::is_effective`] already screens out a non-positive or
/// sub-epsilon pattern in favour of a solid stroke, but its own period check —
/// `on + off >= DASH_PERIOD_EPSILON` — can itself be fooled: two individually
/// finite lengths can sum past `f64::MAX` into `+inf`, and `+inf >=
/// DASH_PERIOD_EPSILON` still reads as effective. kurbo doubles this crate's
/// on/off pair into its own length-2 dash array, so the period it derives is
/// always `on + off`; once that overflows, `phase.rem_euclid(period)`
/// overflows with it, and the catch-up loop `kurbo::dash` runs before it ever
/// pulls a `PathEl` adds an infinite step to a value that never converges —
/// `on = off = f64::MAX`, `phase = -1.0` hangs this way. Refusing here, where
/// `on`, `off` and `phase` already passed their own finiteness checks, is what
/// keeps that unbounded loop out of the frame path.
///
/// Public for the same reason [`dash_path`] and [`well_formed`] are: the CPU
/// oracle carries an identical copy, and a cross-crate test pins the two
/// against each other so they cannot drift apart silently.
pub fn dash_cycle_is_normalizable(dash: &DashPattern) -> bool {
    if !dash.is_effective() {
        // A degenerate pattern never reaches `dash_path`: `is_effective` is
        // what routes it to a solid stroke instead, so its derived period is
        // moot here.
        return true;
    }
    let period = dash.on + dash.off;
    period.is_finite() && period > 0.0 && dash.phase.rem_euclid(period).is_finite()
}

/// The device-space rectangle to hand the fast rectangle path, or `None` when
/// this rectangle has to go through full path processing.
///
/// The fast path writes strip coverage for a rectangle directly, skipping
/// flattening and tiling entirely, and is taken only when the result is
/// indistinguishable from the general path: the composed transform must keep
/// the rectangle axis-aligned (no rotation or skew), and the transformed
/// rectangle must land on whole pixels, so no edge needs partial coverage.
///
/// [`clip`] admits a rectangular clip to its scissor path by the same rule and
/// through this same function: a rectangle whose coverage can be written
/// exactly is a rectangle whose *clip* can be applied exactly, so the two share
/// one admission rule rather than two that could drift apart.
fn fast_rect(rect: Rect, transform: Affine) -> Option<Rect> {
    if !is_axis_aligned(&transform) {
        return None;
    }

    let device = transform.transform_rect_bbox(rect);
    is_pixel_aligned(device).then_some(device)
}

/// Whether every edge of `rect` falls on a whole pixel.
fn is_pixel_aligned(rect: Rect) -> bool {
    [rect.x0, rect.y0, rect.x1, rect.y1]
        .iter()
        .all(|v| v.is_finite() && v.fract() == 0.0)
}

/// A stroke of `width` with round caps and joins — the only stroke style the
/// display list can express.
fn round_stroke(width: f64) -> Stroke {
    Stroke::new(width)
        .with_caps(Cap::Round)
        .with_join(Join::Round)
}

/// `path` expanded into the sub-paths `dash` breaks it into.
///
/// Both ends of the expansion go through [`well_formed`]. The input needs it
/// because `kurbo::dash` mishandles a subpath that closes without ever
/// producing a segment: it emits that subpath's closing element ahead of the
/// `MoveTo` meant to open the output, so a path whose *first* subpath is a
/// zero-length closed one (a dashed arc at zero sweep records exactly that)
/// dashes to a sequence beginning with `ClosePath`. Such a sequence is not a
/// path any consumer can read — `BezPath`'s own "begins with `MoveTo`"
/// invariant is asserted in a debug build and silently strokes malformed
/// geometry in a release one. Normalizing those subpaths away first removes
/// the input the iterator gets wrong; normalizing the result as well makes the
/// well-formedness of what this returns a property of this function rather
/// than of the dash iterator's internal states.
///
/// Callers inside this crate only ever reach `dash` here once
/// [`dash_cycle_is_normalizable`] has passed it, since `kurbo::dash` itself
/// does not bound its catch-up loop against a non-normalizable cycle; a caller
/// outside the up-front walk carries that same obligation. Made `pub` (rather
/// than `pub(crate)`) so `frust-testing`'s CPU oracle, which keeps its own
/// independent copy of this lowering (see that crate's `oracle_cpu` module
/// docs for why), can pin its output against this one directly rather than
/// only through a rendered image.
pub fn dash_path(path: &BezPath, dash: DashPattern) -> BezPath {
    let source = well_formed(path.iter());
    well_formed(kurbo::dash(source.iter(), dash.phase, &[dash.on, dash.off]))
}

/// `elements` as a path every consumer can read: opened by a `MoveTo`, and
/// carrying no `ClosePath` that closes a subpath with no segments in it.
///
/// Both rules drop elements that describe no geometry — an element before the
/// first `MoveTo` has no start point to be drawn from, and closing a subpath
/// that never left its start point adds no segment — so a well-formed path in
/// yields itself back unchanged.
///
/// `pub` for the same cross-crate-parity reason as [`dash_path`].
pub fn well_formed(elements: impl Iterator<Item = PathEl>) -> BezPath {
    let mut out = BezPath::new();
    // Tracked rather than read back off `out`: `BezPath::is_empty` asks whether
    // a path holds any SEGMENT, which a path holding only its opening `MoveTo`
    // does not.
    let mut opened = false;
    let mut segments_in_subpath = 0_usize;

    for element in elements {
        match element {
            PathEl::MoveTo(_) => {
                opened = true;
                segments_in_subpath = 0;
                out.push(element);
            }
            PathEl::ClosePath => {
                if segments_in_subpath > 0 {
                    segments_in_subpath = 0;
                    out.push(element);
                }
            }
            PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                if opened {
                    segments_in_subpath += 1;
                    out.push(element);
                }
            }
        }
    }
    out
}

/// The display list's per-corner radii as kurbo's, in its clockwise-from-top-left
/// argument order.
fn rounded_rect_radii(radii: CornerRadii) -> RoundedRectRadii {
    RoundedRectRadii::new(
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewport_within_three_pixels_of_the_u16_ceiling_is_refused() {
        assert!(check_tile_addressable(65532, 65532).is_ok());
        assert!(matches!(
            check_tile_addressable(65533, 16),
            Err(EngineError::TargetTooLarge)
        ));
        assert!(matches!(
            check_tile_addressable(16, u16::MAX),
            Err(EngineError::TargetTooLarge)
        ));
    }

    /// The substitution both walks make for an off-grid `PushSnapshot`, and why
    /// making it in only one of them would matter.
    ///
    /// A bracket entered with the recorded presentation installs a correction;
    /// one entered neutrally installs none. That correction is precisely the
    /// affine a run inside the bracket is *classified* against, so a walk that
    /// substituted and a walk that did not would decide a run's route from one
    /// magnitude and draw it at another — which is the atlas route handed to a
    /// transform `glifo` will not absorb.
    #[test]
    fn an_off_grid_snapshot_bracket_is_entered_neutrally() {
        // A frame root already carrying an outer bracket's correction, and an
        // inner bracket whose own transform overflows against it. Both factors
        // are finite; only the composition is not.
        let combined = Affine::scale(1e200);
        let transform = Affine::scale(1e200);
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let command = Command::PushSnapshot {
            key: 0,
            rect,
            alpha: 1.0,
            scale: 2.0,
            transform,
        };

        assert!(!command_on_grid(&command, combined));
        assert!(command_on_grid(&command, Affine::IDENTITY));

        let (scale, entered) = snapshot_entry(false, 2.0, transform);
        assert_eq!(scale, 1.0);
        assert_eq!(entered.as_coeffs(), Affine::IDENTITY.as_coeffs());
        let (scale, entered) = snapshot_entry(true, 2.0, transform);
        assert_eq!(scale, 2.0);
        assert_eq!(entered.as_coeffs(), transform.as_coeffs());

        // And the difference reaches the quantity that is classified: an
        // outermost bracket entered with the recorded pair corrects, one
        // entered neutrally does not.
        let mut recorded = SnapshotStack::new();
        recorded.enter(rect, 2.0, transform);
        assert_ne!(
            recorded.correction().as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );

        let mut neutral = SnapshotStack::new();
        let (scale, entered) = snapshot_entry(false, 2.0, transform);
        neutral.enter(rect, scale, entered);
        assert_eq!(
            neutral.correction().as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );
    }

    #[test]
    fn pixel_alignment_rejects_fractional_edges() {
        assert!(is_pixel_aligned(Rect::new(0.0, 0.0, 4.0, 4.0)));
        assert!(!is_pixel_aligned(Rect::new(0.0, 0.5, 4.0, 4.0)));
        assert!(!is_pixel_aligned(Rect::new(0.0, 0.0, 4.0, f64::INFINITY)));
    }

    /// [`SHADER_QUAD_SKIP_WARNING`] is a process-global [`Once`], so this
    /// proves the half of "exactly once" a test can still observe once
    /// another test in the same binary may already have tripped it: the
    /// latch never un-completes, whatever else in this binary called
    /// [`note_shader_quad_skip`] first. `Once::call_once` itself is the
    /// standard-library guarantee behind the other half — that the closure
    /// inside it runs at most once ever — so calling the reporting function
    /// twice here and observing the latch hold is a structural stand-in for
    /// capturing and counting the actual log line.
    #[test]
    fn a_dropped_shader_quad_is_latched_to_once_per_process() {
        note_shader_quad_skip();
        assert!(SHADER_QUAD_SKIP_WARNING.is_completed());
        note_shader_quad_skip();
        assert!(SHADER_QUAD_SKIP_WARNING.is_completed());
    }
}
