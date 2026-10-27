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
//! intermediate texture (see [`clip`]), the opacity-layer and retained brackets
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
//! conservative behaviour (a frame draws less, never wrong). A backdrop blur
//! is a layer of its own holding one draw — its rectangle's coverage under
//! whatever clip is open, painted by nothing the compiler resolves — listed in
//! [`CompiledFrame::backdrops`] for the scheduler and the renderer to cut the
//! frame at and to paint from the blurred page (see
//! [`crate::filters`]' *The backdrop blur*).

pub mod blur_rrect;

pub mod clear;

pub mod clip;

pub(crate) mod cull;

pub mod layers;

pub mod paint;

pub mod draw;

pub mod external;

pub(crate) mod parallel;

pub use clear::ClearPunch;
pub use clip::ClipStack;
pub use draw::{DepthCounter, EngineDraw};
pub use external::{ExternalExtents, ExternalSkip};
pub use layers::{
    GroupStack, LayerLowering, RetainedBracket, RetainedKind, RetainedPlacement, RetainedStack,
};
pub use parallel::{DEFAULT_PARALLEL_MIN_COST, DEFAULT_PARALLEL_WORKERS, MAX_PARALLEL_WORKERS};

/// The most glyph runs a compiler's resident-run cache remembers at once (see
/// [`SceneCompiler::resident_glyph_run_count`]). A byte budget bounds the same
/// records alongside it, so a scene of very long runs is held to fewer.
pub const MAX_RESIDENT_GLYPH_RUNS: usize = crate::text::atlas_policy::MAX_RESIDENT_RUNS;

use std::sync::Once;
use std::time::Duration;

use kurbo::{Affine, BezPath, Cap, Join, PathEl, Rect, RoundedRectRadii, Shape, Stroke};
use peniko::{Brush, Color, Fill, ImageData};

use frust_gpu::{SceneTextureId, TierCaps};
use frust_scene::{Command, CornerRadii, DashPattern, GlyphRun, PathStyle, Scene};

use glifo::{GlyphAtlas, GlyphPrepCache, PendingClearRect};

use vello_common::encode::EncodedPaint;
use vello_common::fearless_simd::Level;
use vello_common::geometry::RectU16;
use vello_common::paint::ImageId;
use vello_common::record::CommandRecorder;
use vello_common::strip_generator::{GenerationMode, StripGenerator, StripStorage};
use vello_common::tile::Tile;
use vello_common::util::is_axis_aligned;

use crate::cache::images::{
    AtlasBudget, AtlasRegion, ImageResidency, ImageSkip, ImageUpload, is_mobile_tier,
};
use crate::compile::blur_rrect::encode_blurred_rounded_rect;
use crate::compile::clear::StagedPunch;
use crate::compile::external::encode_scene_texture;
use crate::compile::paint::{LutRequest, encode_brush, encode_image_brush, encode_image_command};
use crate::compile::parallel::{Claim, Geometry, Merge, Pool};
use crate::config;
use crate::error::EngineError;
use crate::filters::{Backdrop, MAX_BACKDROPS_PER_FRAME, backdrop_region, backdrop_sigma};
use crate::text::hvgl::HvglOutlines;
use crate::text::{
    AtlasPolicy, FontFacts, GlyphQuadScratch, GlyphQuadTargets, GlyphRunRouting, GlyphRunTargets,
    OutlineReason, RunKey, RunRoute, context_paint, glyph_atlas_policy, lower_glyph_run,
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

/// Raised the first time the compiler drops a [`Command::ShaderQuad`] whose
/// program has no rendered target, on the same once-per-process terms as
/// [`IMAGE_SKIP_WARNING`].
static SHADER_QUAD_SKIP_WARNING: Once = Once::new();

/// Raised the first time a [`Command::ShaderQuad`] is dropped because the
/// shader-effect kill switch is set, on the same once-per-process terms as
/// [`IMAGE_SKIP_WARNING`]. Separate from [`SHADER_QUAD_SKIP_WARNING`] because
/// it reports a deliberate configuration rather than a missing pre-pass, and
/// conflating the two would tell an operator who set the switch that something
/// went wrong.
static SHADER_EFFECTS_DISABLED_WARNING: Once = Once::new();

/// A stopwatch for the CPU phases one frame's encode splits into, compiled
/// away entirely without `perf-trace`.
///
/// The engine's own CPU profile is measured by lapping this once per phase
/// rather than by sampling: a phase is tens to hundreds of microseconds and no
/// sampling profiler rides along on a phone under a benchmark harness, while a
/// lap is two clock reads. Under `perf-trace` a lap reads
/// [`web_time::Instant`]; without it the type is zero-sized, [`Self::lap`]
/// answers [`Duration::ZERO`] and no clock is read at all — the "zero clock
/// reads in a disabled build" terms `docs/RENDER_DEVELOPMENT.md`'s perf-trace
/// convention and `docs/PERFORMANCE_BASELINES.md`'s Release-lean note set for an
/// FFI-sensitive path, met at compile time rather than by a runtime branch.
///
/// Laps are cumulative by construction: each one both reports the span since
/// the previous lap and opens the next, so a phase can never be double-counted
/// or silently skipped the way two independent `Instant` pairs could.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhaseClock {
    /// When the phase now being timed began.
    #[cfg(feature = "perf-trace")]
    last: web_time::Instant,
}

impl PhaseClock {
    /// Opens the first phase at "now".
    #[must_use]
    pub(crate) fn start() -> Self {
        Self {
            #[cfg(feature = "perf-trace")]
            last: web_time::Instant::now(),
        }
    }

    /// Closes the phase in flight, answering what it cost, and opens the next.
    #[must_use]
    pub(crate) fn lap(&mut self) -> Duration {
        #[cfg(feature = "perf-trace")]
        {
            let now = web_time::Instant::now();
            // Saturating rather than `-`: a clock that went backwards across a
            // lap is a measurement artefact, and a zero span reports it far
            // better than a panicked frame would (E17).
            let span = now.saturating_duration_since(self.last);
            self.last = now;
            span
        }
        #[cfg(not(feature = "perf-trace"))]
        Duration::ZERO
    }
}

/// What one [`SceneCompiler::compile`] call spent, phase by phase.
///
/// Zero across the board in a build without `perf-trace` — see [`PhaseClock`].
/// The phases partition the call in the order they run, so their sum is the
/// whole compile minus call overhead. Two edges are worth naming rather than
/// leaving to be inferred: the frame record and its depth counter are built
/// after the `admit` lap, so `walk` spans their construction as well as the
/// command walk itself; and the `frust-perf img` line a `perf-trace` build
/// emits is written *after* the last lap, so no phase is charged the cost of
/// reporting on one.
///
/// 1. [`validate`](Self::validate) — the up-front finiteness and geometry
///    sweep over every command. A frame is refused whole or not at all, so
///    this sweep runs before the walk records anything and is paid on every
///    frame in the command count.
/// 2. [`prepare`](Self::prepare) — resetting the per-frame scratch (strip
///    generator, clip/group/retained stacks, punches) and ageing the two
///    caches that span frames (image residency, `glifo`'s prep cache).
/// 3. [`classify`](Self::classify) — routing every glyph run through
///    [`crate::text::atlas_policy`] before any of them is drawn.
/// 4. [`admit`](Self::admit) — closing the glyph atlas's own frame, which is
///    where admission packs what the routing pass asked for.
/// 5. [`walk`](Self::walk) — building the frame record the walk fills, then
///    the command walk itself: strip generation and paint encoding, and on a
///    text-heavy scene the bulk of the call. The part of it spent inside glyph
///    runs is reported separately by [`glyphs`](Self::glyphs), which is a
///    subset of this rather than a phase of its own. It also carries the
///    parallel walk's overhead: the planning pass that picks the shape
///    commands helper threads generate, spawning the helpers and joining them
///    (see the `parallel` module).
/// 6. [`finish`](Self::finish) — closing open groups, generating the hole
///    punches, ageing the glyph atlas and taking the frame's image plan.
///
/// Observational only: nothing downstream branches on any of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompileSpans {
    /// The up-front finiteness and geometry sweep over every command.
    pub validate: Duration,
    /// Per-frame scratch resets and the two cross-frame caches' ageing.
    pub prepare: Duration,
    /// Glyph-run routing, ahead of the walk.
    pub classify: Duration,
    /// Glyph atlas admission, closing the routing pass.
    pub admit: Duration,
    /// The command walk: strip generation and paint encoding, with the
    /// parallel walk's planning, spawn and join.
    pub walk: Duration,
    /// The part of [`walk`](Self::walk) spent inside glyph runs.
    ///
    /// A **subset** of `walk`, not a seventh phase beside it, and so
    /// deliberately excluded from [`Self::total`]: adding it would count the
    /// glyph work twice. It exists because "the walk dominates" is not on its
    /// own an actionable measurement on a text-heavy scene — whether the cost
    /// is the text or everything drawn around it is the question that decides
    /// where a lever could go.
    ///
    /// Accumulated per [`Command::GlyphRun`] rather than per glyph: a run is
    /// the unit the cache and the atlas policy both work in, and two clock
    /// reads a glyph would cost more than the phase being measured.
    pub glyphs: Duration,
    /// Closing groups, punch generation, atlas ageing, the image plan.
    pub finish: Duration,
}

impl CompileSpans {
    /// What the six phases sum to.
    ///
    /// Saturating rather than `+`: a sum is only ever read by a diagnostic
    /// line, and overflowing one must not take the frame with it (E17).
    #[must_use]
    pub fn total(&self) -> Duration {
        [
            self.prepare,
            self.classify,
            self.admit,
            self.walk,
            self.finish,
        ]
        .iter()
        .fold(self.validate, |acc, span| acc.saturating_add(*span))
    }
}

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

/// One glyph drawn as a *glyph quad*: the whole-pixel rectangle its single
/// rect instance covers, and the atlas texel under that rectangle's top-left
/// corner (see [`crate::text`]'s backend for when a glyph is one).
///
/// The draw it belongs to is an [`EngineDraw`] with the run's shared record as
/// its paint, found by its painter-order depth, which no two draws of a frame
/// share. Its strip range is empty unless the glyph was drawn inside an
/// isolated layer, where it carries the rectangle's own coverage so the
/// recorder can fold its bounds into the layer's: the renderer draws a quad
/// from this alone — one rect instance, no edge coverage (the rectangle lands
/// on whole pixels), its payload this atlas texel — and never from strips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphQuad {
    /// The depth of the draw this quad is.
    pub depth: u32,
    /// Left edge in device pixels, already trimmed to the viewport and the
    /// clip stack's scissor.
    pub x: u16,
    /// Top edge in device pixels, trimmed likewise.
    pub y: u16,
    /// Width in device pixels; never zero.
    pub width: u16,
    /// Height in device pixels; never zero.
    pub height: u16,
    /// Atlas texel column under the rectangle's left edge.
    pub atlas_x: u16,
    /// Atlas texel row under the rectangle's top edge.
    pub atlas_y: u16,
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
    /// freed by the residency reap at the head of the frame — less every
    /// rectangle an upload of the same plan rewrites in full (see
    /// [`image_forgets`](Self::image_forgets) for the unfiltered set).
    ///
    /// Serviced **before** [`image_uploads`](Self::image_uploads): a rectangle
    /// freed this frame can be re-allocated in the same frame, so clearing
    /// after writing would erase the image that just moved in.
    pub image_evictions: Vec<AtlasRegion>,
    /// Every atlas rectangle whose resident image was reaped, for the
    /// consumer's registry to forget.
    ///
    /// A superset of [`image_evictions`](Self::image_evictions): a clear is
    /// skipped when an upload in the same plan rewrites the identical
    /// rectangle, but the evicted tenant is gone either way and must stop
    /// resolving. Same pending-until-acknowledged lifetime.
    pub image_forgets: Vec<AtlasRegion>,
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
    /// resident (unsupported format, oversized, malformed, atlas full, the
    /// same blob already resolved this frame at another extent, or the atlas
    /// disabled outright).
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
    /// How many glyph runs were drawn as outlines because they are
    /// faux-obliqued (`OutlineReason::SyntheticOblique`).
    ///
    /// Observational, and the measure of what faux-italic text costs: such a
    /// run is re-tessellated every frame, never cached and never resident.
    /// Counted only when the atlas policy is active (a process with the atlas
    /// switched off routes every run as disabled first).
    pub synthetic_oblique_runs: u32,
    /// How many glyph runs were drawn as outlines because their face has more
    /// variation axes than the cache key holds (`OutlineReason::VariationAxes`,
    /// more than `MAX_KEYED_AXES`).
    ///
    /// Observational, like [`synthetic_oblique_runs`](Self::synthetic_oblique_runs).
    pub variation_axes_outline_runs: u32,
    /// Where each of this frame's atlas-sampled glyphs lives, one entry per
    /// atlas draw (see [`GlyphSlot`]).
    pub glyph_slots: Vec<GlyphSlot>,
    /// The frame's glyph quads, one per draw a fully resident glyph run drew
    /// as one, in depth order (see [`GlyphQuad`]). A quad's draw is the one
    /// kind of draw whose strip range may be empty: outside an isolated layer
    /// nothing reads its coverage, so none is generated.
    ///
    /// A subset of [`atlas_glyph_draws`](Self::atlas_glyph_draws), and empty
    /// whenever quads are off (`FRUST_ENGINE_NO_GLYPH_QUADS`,
    /// [`SceneCompiler::set_glyph_quads`]).
    pub glyph_quads: Vec<GlyphQuad>,
    /// The indices into [`encoded_paints`](Self::encoded_paints) of the
    /// records glyph quads are painted with — one per glyph run and atlas
    /// layer — in ascending order.
    ///
    /// Each is an image record naming one of the run's slots, read for its
    /// atlas layer and tint alone; a draw painted with one is drawn from its
    /// [`GlyphQuad`], never from the record's own placement.
    pub glyph_run_paints: Vec<usize>,
    /// How many glyph runs this frame drew out of the resident-run cache —
    /// every glyph resolved from slots remembered on an earlier frame, without
    /// the run reaching `glifo` (see [`crate::text::ResidentPlan`]).
    ///
    /// Observational: a resident run draws exactly the quads, records and
    /// slots `glifo`'s own fully resident run would, so nothing downstream
    /// tells the two apart. Zero whenever the cache is off
    /// (`FRUST_ENGINE_NO_GLYPH_RUN_CACHE`, [`SceneCompiler::set_glyph_run_cache`])
    /// or quads are.
    pub resident_glyph_runs: u32,
    /// How many of this frame's draws took strips a helper thread generated
    /// rather than strips the walk generated itself (see the `parallel` module).
    ///
    /// Observational: a helper's run is byte-identical to the one the walk
    /// would have generated, so nothing downstream tells the two apart. Zero
    /// whenever the frame spawned no helper — one worker
    /// (`FRUST_ENGINE_NO_PARALLEL`, [`SceneCompiler::set_parallel_workers`]),
    /// planned work below [`SceneCompiler::parallel_min_cost`], or a walk that
    /// reached every planned command before a helper did.
    pub parallel_draws: u32,
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
    /// The retained brackets this frame recorded, in recording order — empty
    /// unless retention is on ([`SceneCompiler::set_retained_layers`]).
    ///
    /// Each names the recorded layer standing for it; the renderer's
    /// retained-page store (`crate::cache::retained`) resolves every one to a
    /// page, a composite or the inline lowering before the frame is scheduled.
    pub retained: Vec<RetainedBracket>,
    /// The keys of every `PushRetained` this frame carried that is *not* in
    /// [`retained`](Self::retained), in recording order — empty unless
    /// retention is on.
    ///
    /// Only the outermost bracket is recorded; one nested inside it (or one
    /// whose transform left the device grid) is painted as part of the
    /// enclosing body and its own key's page receives nothing this frame. The
    /// retained-page store evicts every key listed here, so the ledger never
    /// names a page older than the frame's own recording of the body (see
    /// `crate::cache::retained`'s *A page holds this frame's recording, or
    /// goes*).
    pub retained_superseded: Vec<u64>,
    /// The frame's backdrop blurs, in recording order — and so in ascending
    /// [`Backdrop::depth`] — one per `Command::BackdropBlur` whose rectangle
    /// produced coverage (see [`crate::filters`]' *The backdrop blur*).
    ///
    /// Empty on every frame that records none, and then nothing downstream
    /// plans, cuts or draws anything differently.
    pub backdrops: Vec<Backdrop>,
    /// What compiling this frame cost, phase by phase — all zero without
    /// `perf-trace` (see [`CompileSpans`]).
    ///
    /// Carried on the frame rather than kept on the compiler because the
    /// consumer is the renderer's own encode-phase accounting, which already
    /// holds the frame and would otherwise have to reach back into the
    /// compiler for a number belonging to this frame alone.
    pub compile_spans: CompileSpans,
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
    /// The SIMD level the walk's generator was built at, so a helper's
    /// generator is built at the same one (see [`parallel`]).
    level: Level,
    /// How many threads generate a frame's strips, the calling thread
    /// included; one is the sequential walk. Starts at
    /// [`parallel::default_workers`]; [`Self::set_parallel_workers`] decides
    /// it per compiler.
    parallel_workers: usize,
    /// The estimated cost a frame's planned helper work must reach before a
    /// helper is spawned for it (see [`DEFAULT_PARALLEL_MIN_COST`]). Starts at
    /// that default, or `FRUST_ENGINE_PARALLEL_MIN_COST`;
    /// [`Self::set_parallel_min_cost`] decides it per compiler.
    parallel_min_cost: u64,
    /// The helpers' generators and the per-job slots their runs wait in,
    /// retained across frames for their allocations alone.
    pool: Pool,
    clips: ClipStack,
    groups: GroupStack,
    retained_stack: RetainedStack,
    /// Whether a `PushRetained` bracket is recorded for the retained-page
    /// store and a `DrawRetained` lowered rather than refused (see
    /// [`Self::set_retained_layers`]). Off for a compiler built on its own.
    retain: bool,
    /// The [`CompiledFrame::retained`] entry of the outermost `PushRetained`
    /// body the walk is inside, while it is inside one — what a hole punch or
    /// an external texture drawn in the body marks unretainable.
    retained_open: Option<usize>,
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
    /// [`crate::text::RunKey::distinct_glyphs`]), counted by sorting the run's
    /// ids and dropping repeats; one buffer owned here is one not allocated
    /// per run. It carries nothing between calls — `RunKey::for_run` empties it
    /// before it counts.
    run_glyph_ids: Vec<u32>,
    /// Whether a glyph run resolved entirely out of the glyph atlas is drawn
    /// as glyph quads (see [`crate::text`]). On unless
    /// `FRUST_ENGINE_NO_GLYPH_QUADS` is set; [`Self::set_glyph_quads`] decides
    /// it per compiler.
    glyph_quads: bool,
    /// Where a run's glyph quads wait for the run to finish, retained for its
    /// allocations alone.
    glyph_quad_scratch: GlyphQuadScratch,
    /// Whether a run drawn as glyph quads may be drawn out of the glyph-atlas
    /// policy's resident-run cache rather than through `glifo` (see
    /// [`crate::text::ResidentPlan`]). On unless
    /// `FRUST_ENGINE_NO_GLYPH_RUN_CACHE` is set; [`Self::set_glyph_run_cache`]
    /// decides it per compiler.
    glyph_run_cache: bool,
    /// The font gate's answers for the faces drawn most recently, so a frame
    /// of many short runs reads each face's table directory once (see
    /// [`crate::text::FontFacts`]).
    font_facts: FontFacts,
    /// The outline provider for `hvgl`-only faces, which `glifo` cannot draw:
    /// the faces it opened in CoreText and its bounded outline cache (see
    /// [`crate::text::hvgl`]).
    hvgl_outlines: HvglOutlines,
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
    /// The frame's own target extent — the `(width, height)` most recently
    /// passed to [`Self::compile`] (or, before the first call, this
    /// compiler's own construction size). Kept only so the `ShaderQuad` arm
    /// can tell a quad deliberately culled by the shader-quad pre-pass's own
    /// target-extent check (`crate::effects::shader_quad`) apart from one
    /// whose pre-pass genuinely never ran — see [`shader_quad_is_culled`] and
    /// [`note_shader_quad_unrendered`]/[`note_shader_quad_culled`].
    frame_extent: (u16, u16),
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

    /// Set whether a glyph run resolved entirely out of the glyph atlas is
    /// drawn as glyph quads — one rect instance per glyph, one record per run
    /// and atlas layer — or the general way, one image record and one
    /// generated rectangle per glyph (see [`crate::text`]).
    ///
    /// The per-compiler counterpart of `FRUST_ENGINE_NO_GLYPH_QUADS`, which
    /// only sets the value a compiler starts with: a caller comparing the two
    /// lowerings in one process sets it here rather than through a
    /// process-global environment variable. Takes effect from the next run.
    pub fn set_glyph_quads(&mut self, enabled: bool) {
        self.glyph_quads = enabled;
    }

    /// Whether resident glyph runs are drawn as glyph quads.
    #[must_use]
    pub fn glyph_quads_enabled(&self) -> bool {
        self.glyph_quads
    }

    /// Set whether a glyph run drawn as glyph quads may be drawn out of the
    /// resident-run cache — its glyphs' slots remembered from an earlier
    /// frame — instead of being resolved glyph by glyph through `glifo` (see
    /// [`crate::text::ResidentPlan`]).
    ///
    /// The per-compiler counterpart of `FRUST_ENGINE_NO_GLYPH_RUN_CACHE`, on
    /// the same terms as [`Self::set_glyph_quads`]: the variable sets the
    /// value a compiler starts with, this decides it for one compiler. Takes
    /// effect from the next run. The cache only ever serves quads, so turning
    /// quads off turns it off too.
    pub fn set_glyph_run_cache(&mut self, enabled: bool) {
        self.glyph_run_cache = enabled;
    }

    /// Whether glyph runs may be drawn out of the resident-run cache.
    #[must_use]
    pub fn glyph_run_cache_enabled(&self) -> bool {
        self.glyph_run_cache
    }

    /// Set whether retained brackets are recorded for a retained-page store.
    ///
    /// Off by default: a compiler built on its own has no store behind it, so
    /// it lowers every `PushRetained` inline exactly as `frust_scene` tells a
    /// renderer that does not retain to, and refuses a `DrawRetained` with
    /// [`EngineError::RetainedKeyMissing`]. The renderer turns it on for the
    /// compiler it owns (unless `FRUST_ENGINE_NO_LAYERS` is set): each
    /// outermost bracket's body then becomes a recorded layer whatever its
    /// alpha and is listed in [`CompiledFrame::retained`], and a `DrawRetained`
    /// becomes an empty layer at its painter-order position (see
    /// [`layers`]). Takes effect from the next frame.
    pub fn set_retained_layers(&mut self, enabled: bool) {
        self.retain = enabled;
    }

    /// Whether retained brackets are recorded for a retained-page store.
    #[must_use]
    pub fn retained_layers_enabled(&self) -> bool {
        self.retain
    }

    /// How many glyph runs the resident-run cache currently remembers.
    ///
    /// Bounded by [`MAX_RESIDENT_GLYPH_RUNS`] records (and a byte budget
    /// beside it), however many distinct runs have been drawn.
    #[must_use]
    pub fn resident_glyph_run_count(&self) -> usize {
        self.glyph_atlas.resident_runs()
    }

    /// How many glyph outlines of `hvgl`-only faces (Apple's PingFang, whose
    /// outlines `glifo` cannot read) the compiler currently keeps, fetched
    /// from CoreText.
    ///
    /// Bounded by [`Self::hvgl_outline_capacity`] however many distinct
    /// glyphs have been drawn; always zero on a target without CoreText.
    #[must_use]
    pub fn hvgl_cached_outlines(&self) -> usize {
        self.hvgl_outlines.cached_outlines()
    }

    /// The most `hvgl` glyph outlines the compiler keeps; past it the least
    /// recently drawn one is evicted.
    #[must_use]
    pub fn hvgl_outline_capacity(&self) -> usize {
        self.hvgl_outlines.capacity()
    }

    /// How many `hvgl` faces CoreText resolved to a different PostScript name
    /// than the run's handle carried, since the compiler was built.
    ///
    /// Each such face is refused (its glyphs are counted skipped) and warned
    /// about once.
    #[must_use]
    pub fn hvgl_identity_mismatches(&self) -> u32 {
        self.hvgl_outlines.identity_mismatches()
    }

    /// How many `hvgl` faces were refused because the run's handle carried no
    /// PostScript name, since the compiler was built.
    #[must_use]
    pub fn hvgl_nameless_faces(&self) -> u32 {
        self.hvgl_outlines.nameless_faces()
    }

    /// Set how many threads generate a frame's strips, the calling thread
    /// included (see the `parallel` module). One — or zero, read as one — is the
    /// sequential walk: no planning pass, no helper thread.
    ///
    /// The per-compiler counterpart of `FRUST_ENGINE_NO_PARALLEL`, on the same
    /// terms as [`Self::set_glyph_quads`]: the variable sets the value a
    /// compiler starts with ([`DEFAULT_PARALLEL_WORKERS`] capped by the
    /// machine, or one with it set), this decides it for one compiler. Takes
    /// effect from the next frame. The frame a compiler produces is the same
    /// at every setting. Capped at [`MAX_PARALLEL_WORKERS`] — every
    /// helper is a thread spawned per frame and a strip generator kept across
    /// them, so a count far past any machine would only pay for threads that
    /// cannot run; the variable's path is capped by the machine's own thread
    /// count instead, so that a test can ask for the helper count it means to
    /// exercise on any host.
    pub fn set_parallel_workers(&mut self, workers: usize) {
        self.parallel_workers = workers.clamp(1, parallel::MAX_PARALLEL_WORKERS);
    }

    /// How many threads generate a frame's strips, the calling thread
    /// included.
    #[must_use]
    pub fn parallel_workers(&self) -> usize {
        self.parallel_workers
    }

    /// Set the estimated cost a frame's planned helper work must reach before
    /// a helper is spawned for it (see [`DEFAULT_PARALLEL_MIN_COST`] for the
    /// unit). Zero spawns for any frame with a single planned command, which
    /// is how a test drives the helpers over a scene too small to earn them.
    pub fn set_parallel_min_cost(&mut self, cost: u64) {
        self.parallel_min_cost = cost;
    }

    /// The estimated cost a frame's planned work must reach before a helper
    /// is spawned.
    #[must_use]
    pub fn parallel_min_cost(&self) -> u64 {
        self.parallel_min_cost
    }

    /// A compiler sized for a `width` x `height` viewport, with image
    /// residency budgeted explicitly.
    pub fn with_atlas_budget(width: u16, height: u16, budget: AtlasBudget) -> Self {
        let level = Level::try_detect().unwrap_or(Level::baseline());
        let images = ImageResidency::new(budget);
        Self {
            generator: StripGenerator::new(width, height, level),
            level,
            parallel_workers: parallel::default_workers(),
            parallel_min_cost: config::parallel_min_cost().unwrap_or(DEFAULT_PARALLEL_MIN_COST),
            pool: Pool::default(),
            clips: ClipStack::new(),
            groups: GroupStack::new(),
            retained_stack: RetainedStack::new(),
            retain: false,
            retained_open: None,
            punches: Vec::new(),
            // Built from the residency, so the policy's page geometry is read
            // off the allocator it will pack into rather than derived a second
            // time from the same budget.
            glyph_atlas: glyph_atlas_policy(&images),
            images,
            glyphs: GlyphPrepCache::default(),
            run_routes: Vec::new(),
            next_run: 0,
            run_glyph_ids: Vec::new(),
            glyph_quads: !config::glyph_quads_disabled(),
            glyph_quad_scratch: GlyphQuadScratch::default(),
            glyph_run_cache: !config::glyph_run_cache_disabled(),
            font_facts: FontFacts::default(),
            hvgl_outlines: HvglOutlines::default(),
            hint_text: false,
            externals: ExternalExtents::new(),
            frame_extent: (width, height),
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

    /// The glyph-atlas policy's counters for the last compiled frame — what
    /// the `frust-perf enc` line's miss, eviction and run-key counts read.
    #[cfg(feature = "perf-trace")]
    pub(crate) fn glyph_atlas_trace(&self) -> &crate::text::atlas_policy::AtlasTrace {
        self.glyph_atlas.trace()
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
        // The compiler's half of the encode's CPU accounting; the renderer
        // laps the rest of the call around it (see [`CompileSpans`]). Free
        // without `perf-trace`.
        let mut clock = PhaseClock::start();
        let (width, height) = size;
        check_tile_addressable(width, height)?;
        check_finite(root)?;

        // The whole scene is refused up front rather than mid-walk, so a
        // rejected frame never leaves half its draws recorded.
        let mut backdrops = 0_usize;
        for command in scene.commands() {
            if matches!(command, Command::BackdropBlur { .. }) {
                backdrops = backdrops.saturating_add(1);
            }
            if let Some(transform) = command_transform(command) {
                check_finite(root * transform)?;
            }
            if let Command::DrawRetained { key, .. } = command
                && !self.retain
            {
                // This compiler retains nothing, so no ledger can have
                // declared a key resident and a `DrawRetained` here is a
                // recorder error. Refused with the whole frame, before any
                // strip is generated; the caller logs the reason and skips
                // the frame. With retention on, whether the key is resident is
                // the renderer's store's to answer, after the walk.
                return Err(EngineError::RetainedKeyMissing { key: *key });
            }
            check_geometry(command)?;
        }
        // Counted over the commands the scene records, not over the ones that
        // end up drawing: the bound is a statement about the display list a
        // widget tree produced, refused whole before a strip is generated.
        if backdrops > MAX_BACKDROPS_PER_FRAME {
            return Err(EngineError::TooManyBackdrops {
                recorded: backdrops,
                max: MAX_BACKDROPS_PER_FRAME,
            });
        }

        let validate = clock.lap();

        self.generator.reset(width, height);
        self.frame_extent = (width, height);
        self.clips.reset();
        self.groups.reset();
        self.retained_stack.reset();
        self.retained_open = None;
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
        let prepare = clock.lap();

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
        let classify = clock.lap();

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
        let admit = clock.lap();

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
            image_forgets: Vec::new(),
            image_uploads: Vec::new(),
            atlas_layers: 0,
            image_draws: 0,
            skipped_images: 0,
            external_draws: 0,
            skipped_externals: 0,
            glyph_draws: 0,
            skipped_glyphs: 0,
            atlas_glyph_draws: 0,
            synthetic_oblique_runs: 0,
            variation_axes_outline_runs: 0,
            glyph_slots: Vec::new(),
            glyph_quads: Vec::new(),
            glyph_run_paints: Vec::new(),
            resident_glyph_runs: 0,
            parallel_draws: 0,
            glyph_clears,
            retained: Vec::new(),
            retained_superseded: Vec::new(),
            backdrops: Vec::new(),
            compile_spans: CompileSpans::default(),
        };
        let mut depth = DepthCounter::new();

        // The walk proper, with its shape commands' strips generated by
        // helper threads behind it whenever the plan earns one (see
        // [`parallel`]). Planned and spawned inside the walk's lap, and the
        // helpers joined before it closes, so the parallel walk's overhead is
        // charged to the phase it shortens.
        let helpers = self.parallel_workers.saturating_sub(1);
        let plan = if helpers > 0 {
            parallel::plan_with(
                scene,
                root,
                Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
                self.retain,
            )
        } else {
            parallel::Plan::default()
        };
        if helpers > 0 && !plan.is_empty() && plan.cost() >= self.parallel_min_cost {
            // Moved out for the walk's duration: the helpers borrow it while
            // the walk borrows the compiler. It is put back on the normal
            // return only: a helper panic propagates out of the scope, which
            // aborts a release build (`panic = "abort"` in the release profile)
            // and, in a dev, test or downstream build that unwinds, leaves the
            // compiler with an empty pool — the sequential walk plus a fresh
            // pool's allocations on the next frame, slower and never wrong.
            let mut pool = std::mem::take(&mut self.pool);
            let level = self.level;
            pool.run(&plan, helpers, level, (width, height), |merge| {
                for (index, command) in scene.commands().iter().enumerate() {
                    self.compile_command(index, command, root, &mut frame, &mut depth, Some(merge));
                }
            });
            self.pool = pool;
        } else {
            for (index, command) in scene.commands().iter().enumerate() {
                self.compile_command(index, command, root, &mut frame, &mut depth, None);
            }
        }
        let walk = clock.lap();

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
        frame.image_forgets = self.images.evictions().to_vec();
        frame.image_uploads = uploads;
        frame.atlas_layers = self.images.layers();

        // Field by field rather than as a whole struct, so the glyph subset
        // the walk accumulated into `frame` survives. Last, so `finish` covers
        // every phase above it and the six partition the call rather than
        // sampling parts of it.
        frame.compile_spans.validate = validate;
        frame.compile_spans.prepare = prepare;
        frame.compile_spans.classify = classify;
        frame.compile_spans.admit = admit;
        frame.compile_spans.walk = walk;
        frame.compile_spans.finish = clock.lap();

        // After the last lap, deliberately: the line reports this frame's
        // residency, and a phase that included the cost of reporting on itself
        // would be measuring the instrumentation rather than the compile.
        #[cfg(feature = "perf-trace")]
        note_image_pressure(&frame, &self.images);

        Ok(frame)
    }

    fn compile_command(
        &mut self,
        index: usize,
        command: &Command,
        root: Affine,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        merge: Option<&Merge<'_, '_>>,
    ) {
        // The frame root with any open retained bracket's presentation scale
        // composed ahead of it (see [`layers`]). The identity outside a
        // bracket, so this is the plain frame root for every frame that
        // records none.
        let combined = root * self.retained_stack.correction();

        // Claimed for every command, drawn or not, ahead of every early return
        // below: claiming is what moves the walk's position past a planned
        // command, which is what stops a helper from starting work the walk
        // has already gone by (see [`parallel`]).
        let claim = merge.and_then(|merge| merge.claim(index, combined));

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
        // is not one of them. Only a command *inside* a retained bracket can
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
                | Command::PushClipPath { .. }
                | Command::PushLayer { .. } => self.open_blocked_group(),
                // The correction is the outermost bracket's, so a bracket
                // reaching here is a nested one, whose presentation is ignored
                // anyway; only its depth has to be counted. The substitution is
                // still made through [`retained_entry`] rather than inline,
                // because it is the collect walk's to make identically (see
                // [`Self::classify_runs`]).
                Command::PushRetained {
                    key,
                    rect,
                    scale,
                    transform,
                    ..
                } => {
                    let (scale, transform) = retained_entry(on_grid, *scale, *transform);
                    self.retained_stack.enter(*rect, scale, transform);
                    self.supersede_retained(frame, *key);
                }
                _ => {}
            }
            return;
        }

        match command {
            // A shape is its geometry's coverage under its brush; the
            // geometry is resolved by the one function a helper resolves it by
            // too, so a helper's run and the walk's own can never differ.
            Command::FillRect {
                brush, transform, ..
            }
            | Command::RoundedRect {
                brush, transform, ..
            }
            | Command::Line {
                brush, transform, ..
            }
            | Command::Path {
                brush, transform, ..
            } => {
                if let Some(geometry) = Geometry::of(command, combined) {
                    self.draw_shape(
                        frame,
                        depth,
                        PaintSource::Brush(brush),
                        combined * *transform,
                        geometry,
                        claim,
                    );
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
            // An outline is always a coverage mask, never a layer page — so a
            // `PushRetained` body opened under it finds the mask open and is
            // marked unretainable on exactly a rounded clip's terms (see
            // [`Self::open_retained`]). The group is confined to the outline's
            // device bounding box, which is what a clear hoisted past it is
            // limited to.
            Command::PushClipPath { path, transform } => {
                let transform = combined * *transform;
                self.clips.push_path(path, transform, &mut self.generator);
                self.groups
                    .push_clip(transform.transform_rect_bbox(path.bounding_box()));
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
                // A punch is hoisted out of every bracket to the frame root, so
                // a page could never carry it: a body that punches is drawn
                // inline every frame.
                self.mark_retained_body_unretainable(frame);
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
            Command::PushRetained {
                key,
                rect,
                alpha,
                scale,
                transform,
            } => {
                let mut listed = false;
                if self.retained_stack.enter(*rect, *scale, *transform) {
                    // The bracket's own correction is the one that applies to
                    // the layer it opens, so the transform is recomposed here
                    // rather than reusing `combined` from before the entry.
                    let corrected = root * self.retained_stack.correction() * *transform;
                    if check_finite(corrected).is_ok() {
                        if self.retain {
                            self.open_retained(frame, *key, *rect, *alpha, corrected);
                            listed = true;
                        } else if *alpha < 1.0 {
                            self.open_layer(frame, *rect, *alpha, corrected);
                            self.retained_stack.record_layer(self.groups.depth());
                        }
                    }
                }
                // A nested bracket is painted as part of the outermost body,
                // so its own key's page receives nothing this frame.
                if !listed {
                    self.supersede_retained(frame, *key);
                }
            }
            Command::PopRetained => {
                let closes = self.retained_stack.leave(self.groups.depth());
                if closes {
                    self.close_group(frame);
                }
                // The outermost bracket just ended. A body whose own group a
                // stray pop inside it already closed drew its tail outside the
                // layer, so the layer is not the body and is never retained.
                if !self.retained_stack.is_open()
                    && let Some(index) = self.retained_open.take()
                    && !closes
                {
                    mark_unretainable(frame, index);
                }
            }
            // Refused by the up-front walk in [`Self::compile`] when retention
            // is off, so it reaches the lowering only with a store behind it.
            Command::DrawRetained {
                key,
                rect,
                transform,
                alpha,
                scale,
            } => {
                if self.retain {
                    let mapped = combined * *transform * Affine::scale_about(*scale, rect.center());
                    self.record_draw_retained(frame, depth, *key, *rect, *alpha, mapped);
                }
            }
            // An image is its destination rectangle's coverage under an image
            // paint — the same two rectangle paths a solid fill takes, so a
            // pixel-aligned image costs no flattening either.
            Command::Image {
                data,
                dest,
                transform,
            } => {
                if let Some(geometry) = Geometry::of(command, combined) {
                    let source = PaintSource::Image { data, dest: *dest };
                    self.draw_shape(frame, depth, source, combined * *transform, geometry, claim);
                }
            }
            // The strip generator rasterizes the padded bounding rectangle,
            // not `rect` itself and not a rounded shape — see [`blur_rrect`]'s
            // module doc for why. It takes the same fast rectangle path a fill
            // or an image does whenever that padded rectangle lands
            // pixel-aligned under the composed transform.
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
                transform,
            } => {
                if let Some(geometry) = Geometry::of(command, combined) {
                    let source = PaintSource::BlurredRect {
                        rect: *rect,
                        radii: *radii,
                        std_dev: *std_dev,
                        color: *color,
                    };
                    self.draw_shape(frame, depth, source, combined * *transform, geometry, claim);
                }
            }
            Command::GlyphRun(run) => {
                // Two clock reads a run, free without `perf-trace` (see
                // [`PhaseClock`]); the accumulated total is a subset of the
                // walk this arm runs inside.
                let mut clock = PhaseClock::start();
                self.compile_glyph_run(run, combined * run.transform, route, frame, depth);
                frame.compile_spans.glyphs = frame.compile_spans.glyphs.saturating_add(clock.lap());
            }
            // A fragment program's own pixels were produced before the frame
            // was compiled, into a texture registered under an id derived from
            // the program alone (`crate::effects::shader_quad`). From here on
            // the quad is an external texture like any other — the pre-pass is
            // the only thing that distinguishes it.
            Command::ShaderQuad {
                program,
                dest,
                transform,
                ..
            } => {
                // A fragment program's pixels change with no command changing,
                // so a body drawing one is never retained.
                self.mark_retained_body_unretainable(frame);
                if config::shader_effects_disabled() {
                    note_shader_effects_disabled();
                    return;
                }
                let id = shader_quad_texture_id(program.id());
                if self.externals.get(id).is_none() {
                    // No pre-pass ran for this program, its shader failed to
                    // compile, or this exact quad was deliberately culled by
                    // the pre-pass's own target-extent check (see
                    // `shader_quad_is_culled`) — an expected, routine outcome
                    // told apart from the other two so it is never reported
                    // through the missing-pre-pass warning below.
                    if shader_quad_is_culled(*dest, combined * *transform, self.frame_extent) {
                        note_shader_quad_culled(program.id());
                    } else {
                        // Reported in its own words rather than as an
                        // unregistered scene texture, whose id would name
                        // nothing a reader could look up.
                        note_shader_quad_unrendered(program.id());
                    }
                    frame.skipped_externals = frame.skipped_externals.saturating_add(1);
                    return;
                }
                self.draw_external_texture(id, *dest, combined * *transform, frame, depth);
            }
            Command::SceneTexture {
                id,
                dest,
                transform,
            } => {
                // An externally bound texture's pixels change with no command
                // changing (a camera, a video, a 3D pass), so a body drawing
                // one is never retained.
                self.mark_retained_body_unretainable(frame);
                self.draw_external_texture(*id, *dest, combined * *transform, frame, depth);
            }
            Command::BackdropBlur {
                rect,
                radius,
                transform,
            } => {
                // A page is rasterized offscreen, where there is no frame
                // beneath the body to sample, so a body blurring its backdrop
                // is drawn inline every frame — on exactly a punch's terms.
                self.mark_retained_body_unretainable(frame);
                self.record_backdrop(frame, depth, *rect, *radius, combined * *transform);
            }
        }
    }

    /// Record a `Command::BackdropBlur`: a layer of its own holding the
    /// rectangle's coverage, listed in [`CompiledFrame::backdrops`].
    ///
    /// The layer is what places the backdrop at its painter-order position in
    /// the recording — the node the scheduler cuts the frame's surface round
    /// at — and it opens no clip and composites at full opacity, so a
    /// scheduler that served it as an ordinary layer would inline it. Its one
    /// draw is the rectangle generated under the clip stack exactly like a
    /// fill's, so a scissor trims it and a coverage mask (a rounded or path
    /// clip) masks it; it is painted transparent, because what it really
    /// paints — the blurred page — is the renderer's to bind once the page
    /// exists. A rectangle producing no coverage records the empty layer and
    /// no backdrop, which then costs nothing anywhere.
    fn record_backdrop(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        rect: Rect,
        radius: f64,
        transform: Affine,
    ) {
        let Some(sigma) = backdrop_sigma(radius, transform) else {
            return;
        };
        let (width, height) = self.frame_extent;
        let Some(region) =
            backdrop_region(transform.transform_rect_bbox(rect), sigma, (width, height))
        else {
            return;
        };

        let layer = u32::try_from(frame.recorder.layers.len()).unwrap_or(u32::MAX);
        frame.recorder.push_layer(layers::layer_props(1.0), None);
        let transparent = Brush::Solid(Color::TRANSPARENT);
        let recorded = self.record(
            frame,
            depth,
            PaintSource::Brush(&transparent),
            transform,
            Geometry::rect(rect, transform),
            None,
        );
        frame.recorder.pop_layer();

        if recorded && let Some(draw) = frame.draws().last() {
            frame.backdrops.push(Backdrop {
                layer,
                depth: draw.depth,
                region,
                sigma,
            });
        }
    }

    /// Record the texture bound under `id` scaled to fill `dest` under
    /// `transform` — the body both [`Command::SceneTexture`] and
    /// [`Command::ShaderQuad`] lower to, since the only thing separating them
    /// is where the texels came from.
    ///
    /// An externally owned texture is its destination rectangle's coverage
    /// under an image paint that samples the caller's texture rather than the
    /// atlas — the same two rectangle paths [`Command::Image`] takes, so a
    /// pixel-aligned one costs no flattening either.
    fn draw_external_texture(
        &mut self,
        id: u64,
        dest: Rect,
        transform: Affine,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
    ) {
        let source = PaintSource::SceneTexture { id, dest };
        let geometry = Geometry::rect(dest, transform);
        self.draw_shape(frame, depth, source, transform, geometry, None);
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
    /// The walk carries a [`RetainedStack`] of its own for one reason: a run's
    /// route depends on the scale in its *device* transform (see
    /// [`crate::text::atlas_policy`]), and inside a `PushRetained` bracket that
    /// transform carries the bracket's presentation scale as well as the frame
    /// root. Classifying against `root * run.transform` alone would answer for
    /// a size the run is not drawn at. Only the correction is tracked here —
    /// the bracket's *layers* are the draw walk's to open, and this walk opens
    /// nothing.
    ///
    /// A bracket is entered on the draw walk's exact terms, through the same
    /// [`command_on_grid`] check and the same [`retained_entry`] substitution
    /// it uses, so the correction the two walks carry is one decision made
    /// twice rather than two decisions that happen to agree.
    fn classify_runs(&mut self, scene: &Scene, root: Affine) {
        self.run_routes.clear();
        self.next_run = 0;

        let mut retained_stack = RetainedStack::new();
        for command in scene.commands() {
            match command {
                Command::PushRetained {
                    rect,
                    scale,
                    transform,
                    ..
                } => {
                    // The draw walk's own entry, made here on exactly its
                    // terms: [`command_on_grid`] is the check it asks and
                    // [`retained_entry`] is the substitution it makes. A
                    // bracket it enters neutrally installs no correction, so a
                    // walk that entered it with the recorded pair would
                    // classify every run inside against a device transform
                    // nothing is ever drawn through — a route decided for one
                    // magnitude and a draw made at another.
                    let combined = root * retained_stack.correction();
                    let on_grid = command_on_grid(command, combined);
                    let (scale, transform) = retained_entry(on_grid, *scale, *transform);
                    retained_stack.enter(*rect, scale, transform);
                }
                Command::PopRetained => {
                    // The group depth a real close would be tested against is
                    // the draw walk's; nothing here closes a group, so zero is
                    // the honest answer and the return value is unused.
                    retained_stack.leave(0);
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
                    // An `hvgl`-only face is routed to outlines exactly as a
                    // colour face is: its glyphs are drawn by the `hvgl`
                    // provider, never cached by `glifo`.
                    let facts = self.font_facts.of(run.font.font());
                    let key = RunKey::for_run(
                        run,
                        root * retained_stack.correction() * run.transform,
                        self.hint_text,
                        facts.color || facts.hvgl,
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
        let facts = self.font_facts.of(run.font.font());
        if !facts.readable {
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
        let glyph_run_cache = self.glyph_run_cache;
        let hvgl_face = facts.hvgl;
        // Destructured rather than passed as `self`, because the sink borrows
        // the generator and the clip stack mutably while the glyph caches, the
        // entry map and the shared allocator are borrowed mutably alongside
        // them — five disjoint fields of one struct.
        let Self {
            generator,
            clips,
            groups,
            glyphs,
            images,
            glyph_atlas,
            glyph_quads,
            glyph_quad_scratch,
            hvgl_outlines,
            ..
        } = self;
        // The policy's decision, turned into the borrow `glifo` caches
        // through. A run it refused reaches `glifo` with no cache at all, which
        // is the outline path unchanged rather than a cache that declines every
        // lookup — those are the same pixels but not the same work.
        match route {
            Some(RunRoute::Outline(OutlineReason::SyntheticOblique)) => {
                frame.synthetic_oblique_runs = frame.synthetic_oblique_runs.saturating_add(1);
            }
            Some(RunRoute::Outline(OutlineReason::VariationAxes)) => {
                frame.variation_axes_outline_runs =
                    frame.variation_axes_outline_runs.saturating_add(1);
            }
            _ => {}
        }
        let atlas = match route {
            Some(RunRoute::Atlas(_)) => Some((glyph_atlas, images.allocator_mut())),
            Some(RunRoute::Outline(_)) | None => None,
        };
        // Quads only for a run the atlas serves, and never under a coverage
        // mask: a quad's rectangle can be trimmed to the scissor but cannot
        // carry a mask (see [`crate::text`]'s backend).
        let quads =
            (*glyph_quads && atlas.is_some() && clips.mask().is_none()).then(|| GlyphQuadTargets {
                scratch: glyph_quad_scratch,
                window: quad_window(generator, groups),
            });
        let outcome = lower_glyph_run(
            run,
            transform,
            paint,
            &run.brush,
            hint_text,
            GlyphRunRouting {
                atlas,
                resident: glyph_run_cache,
                quads,
                hvgl_face,
            },
            GlyphRunTargets {
                generator,
                clips,
                prep: glyphs,
                frame,
                depth,
                hvgl: hvgl_outlines,
            },
        );

        frame.glyph_draws = frame.glyph_draws.saturating_add(outcome.drawn);
        frame.skipped_glyphs = frame.skipped_glyphs.saturating_add(outcome.skipped);
        if outcome.resident {
            frame.resident_glyph_runs = frame.resident_glyph_runs.saturating_add(1);
        }
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

    /// Open the outermost `PushRetained` bracket of a compiler with retention
    /// on: the same lowering the inline emulation gives it, recorded as a
    /// layer whatever its alpha, and listed for the retained-page store.
    ///
    /// Below full opacity that is exactly [`Self::open_layer`]. At full opacity
    /// the inline emulation opens no layer and clips nothing, so the layer
    /// opened here carries no clip either ([`GroupStack::push_retained`]) — the
    /// body's draws are the inline lowering's draws, gathered — and the store
    /// that refuses it leaves the scheduler a full-opacity layer it inlines.
    ///
    /// Whether the body can be retained at all is measured here, before the
    /// bracket's own group narrows anything: a page can only hold what the
    /// body really drew, so a body reaching past the viewport or a scissor
    /// around it, or under a coverage mask, would be retained missing what was
    /// clipped away — and composited somewhere else on a later frame, the loss
    /// would show.
    fn open_retained(
        &mut self,
        frame: &mut CompiledFrame,
        key: u64,
        rect: Rect,
        alpha: f32,
        transform: Affine,
    ) {
        let window = quad_window(&self.generator, &self.groups);
        let device = transform.transform_rect_bbox(rect);
        let retainable = self.clips.mask().is_none()
            && !self.clips.blocks_everything()
            && rect_within(device, window);
        // The id `push_layer` is about to hand the layer, in either branch.
        let layer = u32::try_from(frame.recorder.layers.len()).unwrap_or(u32::MAX);

        if alpha < 1.0 {
            self.open_layer(frame, rect, alpha, transform);
        } else {
            frame.recorder.push_layer(layers::layer_props(alpha), None);
            self.groups.push_retained();
        }
        self.retained_stack.record_layer(self.groups.depth());

        let placement = RetainedPlacement::of(transform);
        frame.retained.push(RetainedBracket {
            key,
            layer,
            opacity: alpha,
            placement,
            extent_px: placement.extent(rect),
            kind: RetainedKind::Record {
                rect: tile_bounds(device),
                retainable,
            },
        });
        self.retained_open = Some(frame.retained.len() - 1);
    }

    /// Record a `DrawRetained` of a compiler with retention on: an empty layer
    /// at the command's painter-order position, standing for the composite the
    /// retained-page store resolves it to, plus what the store needs to place
    /// that composite.
    ///
    /// `mapped` is the body's local space mapped onto the device grid with the
    /// presentation scale folded in — the same composition a `PushRetained`
    /// bracket's correction produces for its body. A non-finite one draws
    /// nothing, the same answer every off-grid command gets.
    fn record_draw_retained(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        key: u64,
        rect: Rect,
        alpha: f32,
        mapped: Affine,
    ) {
        if check_finite(mapped).is_err() {
            return;
        }
        let window = quad_window(&self.generator, &self.groups);
        let masked = self.clips.mask().is_some();
        let layer = u32::try_from(frame.recorder.layers.len()).unwrap_or(u32::MAX);
        // Pushed and popped at once: a layer with no draws of its own, whose
        // node is what puts the composite at this point of its stream.
        frame.recorder.push_layer(layers::layer_props(alpha), None);
        frame.recorder.pop_layer();

        let placement = RetainedPlacement::of(mapped);
        frame.retained.push(RetainedBracket {
            key,
            layer,
            opacity: alpha,
            placement,
            extent_px: placement.extent(rect),
            kind: RetainedKind::Draw {
                depth: depth.advance(),
                window: if self.clips.blocks_everything() {
                    RectU16::new(0, 0, 0, 0)
                } else {
                    window
                },
                masked,
            },
        });
    }

    /// List `key`, a `PushRetained` this frame carries but records no bracket
    /// for, in [`CompiledFrame::retained_superseded`] — with retention on;
    /// without it there is no store to evict from.
    fn supersede_retained(&self, frame: &mut CompiledFrame, key: u64) {
        if self.retain {
            frame.retained_superseded.push(key);
        }
    }

    /// Mark the retained body the walk is inside, if any, as one no page may
    /// hold.
    fn mark_retained_body_unretainable(&mut self, frame: &mut CompiledFrame) {
        if let Some(index) = self.retained_open {
            mark_unretainable(frame, index);
        }
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
        self.retained_stack.reset();
        // A body still open at the end of the frame never reached its own
        // `PopRetained`, so where it would have ended is unknown.
        if let Some(index) = self.retained_open.take() {
            mark_unretainable(frame, index);
        }
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

    /// [`record`](Self::record) `geometry`, counting the draw as a fast
    /// rectangle when it took that path (see [`fast_rect`]).
    fn draw_shape(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        source: PaintSource<'_>,
        transform: Affine,
        geometry: Geometry<'_>,
        claim: Option<Claim<'_>>,
    ) {
        let fast = geometry.is_fast_rect();
        if self.record(frame, depth, source, transform, geometry, claim) && fast {
            frame.fast_rect_draws = frame.fast_rect_draws.saturating_add(1);
        }
    }

    /// Generate `geometry` under the active clip, then record whatever strips
    /// survived as one draw painted from `source` under `transform`.
    ///
    /// The clip stack's coverage mask is handed to the strip generator, which
    /// is what intersects a mask clip while the draw's own coverage is
    /// produced; the scissor is applied afterwards, to the run the generator
    /// appended. A scissor admitting nothing skips generation entirely rather
    /// than generating coverage to throw away.
    ///
    /// `claim` is the command's helper-generated run, when one was planned
    /// (see [`parallel`]). It stands in for generating only while no mask is
    /// open — a helper generates unclipped — and only when a helper actually
    /// got to it; otherwise the run is generated here exactly as it would be
    /// without one. Everything after generation is the same either way.
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
    fn record(
        &mut self,
        frame: &mut CompiledFrame,
        depth: &mut DepthCounter,
        source: PaintSource<'_>,
        transform: Affine,
        geometry: Geometry<'_>,
        claim: Option<Claim<'_>>,
    ) -> bool {
        if self.clips.blocks_everything() {
            return false;
        }

        let start = frame.strips.strips.len();
        let alpha_start = frame.strips.alphas.len();
        let mask = self.clips.mask();
        let merged =
            mask.is_none() && claim.is_some_and(|claim| claim.append_to(&mut frame.strips));
        if !merged {
            geometry.generate(&mut self.generator, &mut frame.strips, mask);
        }
        self.clips.clip_run(&mut frame.strips, start, alpha_start);
        let strip_range = start..frame.strips.strips.len();

        // A run is only a draw when it carries content. Under a mask clip
        // `vello_common::clip::intersect_impl` gates its trailing sentinel on
        // the *whole* target buffer being non-empty, not on what this call
        // added — and `frame.strips` is one `Append`-mode buffer shared by
        // every draw of the frame, so a masked path that contributed no rows
        // (zero-area geometry, coverage the mask removed entirely) after an
        // earlier draw's content still gets a sentinel: a *lone* sentinel.
        // The renderer's pairwise walk reads each span's extent off the strip
        // after it, so that is not a run at all — roll it back exactly like an
        // empty one. Every generation path pairs a content strip with its own
        // sentinel in the same call, so a one-strip run can only be that
        // sentinel; the assertion keeps a future generator change from being
        // swallowed here as "nothing to draw".
        if strip_range.len() < 2 {
            debug_assert!(
                strip_range.is_empty() || frame.strips.strips[start].is_sentinel(),
                "a one-strip run must be a lone sentinel, not an unterminated content strip"
            );
            frame.strips.strips.truncate(start);
            frame.strips.alphas.truncate(alpha_start);
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
        if merged {
            frame.parallel_draws = frame.parallel_draws.saturating_add(1);
        }
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

/// Report what this frame's image residency cost, on a frame where it cost
/// anything.
///
/// The counterpart to [`note_image_skip`]'s once-per-process warning, which
/// says *that* an image was refused and then goes quiet: this says how many
/// draws a given frame lost and how hard the atlas is being churned to avoid
/// losing more. `frust-perf`-prefixed and at info level, which is what carries
/// it into a benchmark capture — the harness keeps every line with that prefix
/// and drops the rest, so a run's own log answers "did the atlas hold this
/// scene?" without a parallel system-log capture beside it.
///
/// Silent on a frame that skipped nothing and evicted nothing, which is every
/// frame of a steady scene: the common path pays two comparisons and writes no
/// line. `skipped` is the frame's own count of image draws that painted
/// nothing, so it includes the few refusals decided before residency is even
/// consulted (a singular paint transform, a destination with no area) as well
/// as the atlas's own — every one of them is a draw the display list asked for
/// and the frame did not paint, which is the question the line answers.
///
/// `perf-trace`-only, like every other `frust-perf` line in this workspace
/// (`frust_render::context::log_render_path`, [`PhaseClock`], the encode
/// window): the release-lean gate asserts a shipping binary contains no
/// `frust-perf` bytes at all, and a `#[cfg]` is what makes that the compiler's
/// answer rather than a hope about the optimizer.
#[cfg(feature = "perf-trace")]
fn note_image_pressure(frame: &CompiledFrame, images: &ImageResidency) {
    if !image_pressure_reported(frame, images) {
        return;
    }
    // The text is built *inside* the macro's argument list, so the log ceiling
    // covers the `format!` and not merely the emission. `evicted>0` is the
    // expected steady state of a working set larger than the atlas, and a
    // build whose ceiling drops info lines must not pay a `String` per frame
    // for one it will never record.
    log::info!("{}", image_pressure_line(frame, images));
}

/// Whether `frame` has any image-residency cost to report against `images`.
///
/// Split from the line itself so the text can be built where the log macro can
/// elide it — see [`note_image_pressure`] — and so "a frame the atlas held
/// reports nothing at all" stays a property a test can name.
#[cfg(feature = "perf-trace")]
#[must_use]
pub fn image_pressure_reported(frame: &CompiledFrame, images: &ImageResidency) -> bool {
    frame.skipped_images != 0 || images.frame_pressure_evictions() != 0
}

/// The `frust-perf img` line `frame` reports against `images`.
///
/// Separate from the logging above because the *text* is a contract: a
/// benchmark capture is graded by grepping these fields, so the field order and
/// the names are pinned by a test rather than only by this module. Ask
/// [`image_pressure_reported`] first — on a quiet frame this still formats a
/// line, it is simply one nothing asks for.
#[cfg(feature = "perf-trace")]
#[must_use]
pub fn image_pressure_line(frame: &CompiledFrame, images: &ImageResidency) -> String {
    let budget = images.budget();
    format!(
        "frust-perf img skipped={} evicted={} resident={} budget={}x{}x{}",
        frame.skipped_images,
        images.frame_pressure_evictions(),
        images.entry_count(),
        budget.atlas_size.0,
        budget.atlas_size.1,
        budget.max_atlases,
    )
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

/// The id the offscreen target of fragment program `program_id` is registered
/// under — the compiler's half of the agreement
/// [`crate::effects::shader_quad`] makes with the pre-pass that renders it.
///
/// A pure function of the program id, computed independently on both sides
/// rather than exchanged through a side table, because the walk holds nothing
/// else: a [`Command::ShaderQuad`] carries its `ShaderProgram` and no texture
/// handle. `SceneTextureId::for_shader_program` is what keeps the derived
/// value out of the range host textures mint from.
#[must_use]
pub fn shader_quad_texture_id(program_id: u64) -> u64 {
    SceneTextureId::for_shader_program(program_id).get()
}

/// Report a [`Command::ShaderQuad`] dropped for want of a rendered target.
///
/// Latched to once per process rather than following [`note_image_skip`] and
/// [`note_font_skip`]'s warn-then-debug shape: a compiler driven without the
/// pre-pass (a host encoding frames straight through
/// [`crate::EngineRenderer`], or a program whose shader failed to compile)
/// produces this on every frame forever, and the second report says nothing
/// the first did not. Never raised for a quad [`shader_quad_is_culled`]
/// reports deliberately culled — that case is [`note_shader_quad_culled`]'s.
fn note_shader_quad_unrendered(program_id: u64) {
    SHADER_QUAD_SKIP_WARNING.call_once(|| {
        log::warn!(
            "ShaderQuad draws nothing: fragment program {program_id} has no rendered target \
             — either the frame's shader pre-pass did not run before this compile, or the \
             program failed to compile (logged once per process)"
        );
    });
}

/// Whether a [`Command::ShaderQuad`] at `dest` under `transform` (the
/// command's own transform composed with the frame's) is entirely outside
/// `frame_extent` — the same target-extent overlap test
/// `crate::effects::shader_quad::frame_demands`/`culled_program_ids` apply to
/// decide whether the pre-pass renders this program's quad at all this frame,
/// restated independently here (the compiler and the pre-pass share no
/// channel for "this id was culled, not missing") so an id nothing is
/// registered under can be told apart from one whose pre-pass genuinely never
/// ran — see [`note_shader_quad_unrendered`]/[`note_shader_quad_culled`].
///
/// Conservative on the same terms as the pre-pass's own check: a non-finite
/// `bbox` answers `false` (never treated as culled — some other refusal
/// accounts for it), and a shared edge counts as overlapping via
/// [`kurbo::Rect::overlaps`]. Unlike the pre-pass's walk, this has no
/// [`Command::PushRetained`]-bracket exemption to make: it is asked only
/// about the one quad instance actually being compiled right now, under its
/// own already-composed `transform` (which, inside a bracket, already
/// includes the enclosing retained bracket's own presentation correction — see
/// `compile_command`'s `combined`) — there is no separate "the bracket might
/// still move it" case to guard against here, only the geometry this exact
/// draw is about to be attempted at.
fn shader_quad_is_culled(dest: Rect, transform: Affine, frame_extent: (u16, u16)) -> bool {
    let frame_rect = Rect::new(
        0.0,
        0.0,
        f64::from(frame_extent.0),
        f64::from(frame_extent.1),
    );
    let bbox = transform.transform_rect_bbox(dest);
    bbox.is_finite() && !bbox.overlaps(frame_rect)
}

/// Report a [`Command::ShaderQuad`] dropped because the shader-quad pre-pass
/// (`crate::effects::shader_quad`) deliberately culled every one of the
/// program's quads this frame — its device rectangle does not intersect the
/// frame's own target at all — rather than because no pre-pass ran for it at
/// all.
///
/// Debug-level and unlatched (unlike [`note_shader_quad_unrendered`]): a
/// culled quad is an expected, routine outcome of a scene drawing an
/// off-screen or fully clipped program, not a signal that something is
/// missing, so it does not need the once-per-process rate limit a genuine
/// "nothing rendered this" warning does.
fn note_shader_quad_culled(program_id: u64) {
    log::debug!(
        "ShaderQuad draws nothing: fragment program {program_id} was culled by the \
         shader-quad pre-pass — its destination does not intersect this frame's target \
         (expected; not a missing pre-pass)"
    );
}

/// Report a [`Command::ShaderQuad`] dropped because
/// `FRUST_ENGINE_NO_SHADER_EFFECTS` is set.
///
/// Once per process, like the sighting above: the switch is read once and
/// cannot change under a running process, so the fact is stated once.
fn note_shader_effects_disabled() {
    SHADER_EFFECTS_DISABLED_WARNING.call_once(|| {
        log::warn!(
            "ShaderQuad commands draw nothing: FRUST_ENGINE_NO_SHADER_EFFECTS is set \
             (logged once per process)"
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
        | Command::PushClipPath { transform, .. }
        | Command::Image { transform, .. }
        | Command::BlurredRoundedRect { transform, .. }
        | Command::PushLayer { transform, .. }
        | Command::ClearRect { transform, .. }
        | Command::Path { transform, .. }
        | Command::ShaderQuad { transform, .. }
        | Command::SceneTexture { transform, .. }
        | Command::PushRetained { transform, .. }
        | Command::DrawRetained { transform, .. }
        | Command::BackdropBlur { transform, .. } => Some(*transform),
        Command::GlyphRun(run) => Some(run.transform),
        Command::PopClip | Command::PopLayer | Command::PopRetained => None,
    }
}

/// Whether `command`'s own transform still lands on the finite device grid once
/// `combined` — the frame root with any open retained bracket's correction — is
/// composed ahead of it.
///
/// Asked by both of the frame's walks, from one place, because they have to ask
/// it the same way. The draw walk draws nothing for a command that answers
/// `false` and enters a `PushRetained` neutrally instead (see
/// [`retained_entry`]); the collect walk classifies against the transform that
/// entry implies. Two copies of this expression could drift a coefficient
/// apart and route a run for a device size it is never drawn at.
///
/// A command carrying no transform of its own is on the grid trivially: there
/// is nothing to compose.
fn command_on_grid(command: &Command, combined: Affine) -> bool {
    command_transform(command).is_none_or(|transform| check_finite(combined * transform).is_ok())
}

/// The presentation scale and transform a `PushRetained` bracket is entered
/// with: the recorded pair on the grid, and the neutral pair off it.
///
/// The neutral pair is what makes an off-grid bracket *inert* rather than
/// absent — it still has a depth to count and a pop to balance, but it installs
/// no correction, so nothing inside it is drawn through a transform the frame
/// refused. Both walks substitute through this one function so that the route
/// a run is given and the transform it is drawn through can never be decided
/// from different magnitudes.
fn retained_entry(on_grid: bool, scale: f64, transform: Affine) -> (f64, Affine) {
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
/// refusal its transform already drew. A layer and a retained bracket are
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
        // An outline is the mask's geometry outright, so a non-finite point
        // reaches the flattener exactly as a filled path's would.
        Command::PushClipPath { path, .. } => path.is_finite(),
        Command::PushLayer { rect, alpha, .. } => rect.is_finite() && alpha.is_finite(),
        Command::ClearRect { rect, .. } => rect.is_finite(),
        Command::PushRetained {
            rect, alpha, scale, ..
        }
        | Command::DrawRetained {
            rect, alpha, scale, ..
        } => rect.is_finite() && alpha.is_finite() && scale.is_finite(),
        Command::Image { dest, .. } => dest.is_finite(),
        Command::SceneTexture { dest, .. } => dest.is_finite(),
        // Carries a destination rectangle like the two above, and lowers
        // through the same external-texture path, so the same check applies.
        Command::ShaderQuad { dest, .. } => dest.is_finite(),
        Command::BlurredRoundedRect {
            rect,
            radii,
            std_dev,
            ..
        } => rect.is_finite() && radii_are_finite(*radii) && std_dev.is_finite(),
        Command::GlyphRun(run) => glyph_run_is_finite(run),
        // Its rectangle is the coverage the blurred backdrop is drawn through
        // and the region the blur reads is grown from it, and its radius is
        // the σ the planner's decimation loop runs over — a non-finite one of
        // either reaches the flattener or that loop as it was recorded.
        Command::BackdropBlur { rect, radius, .. } => rect.is_finite() && radius.is_finite(),
        // Carrying no geometry of their own — see above.
        Command::PopClip | Command::PopLayer | Command::PopRetained => true,
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

/// The device window a glyph quad drawn now is trimmed to: the generator's
/// viewport intersected with the scissor every open bracket's clip applies.
///
/// Only asked while no coverage mask is on the clip stack, and then every open
/// bracket's clip is a scissor whose rectangle is exactly the bracket's
/// recorded bounds: a bracket and its clip are pushed together from one
/// rectangle under one transform ([`GroupStack`]'s invariant), and a clip
/// scissors only when [`fast_rect`] admits that rectangle — the same
/// `transform_rect_bbox` the bounds are recorded from, on whole pixels — clamped
/// onto the `u16` grid the same way. So this is the rectangle the general
/// draw's coverage is culled and scissored to, without a second clip stack.
fn quad_window(generator: &StripGenerator, groups: &GroupStack) -> RectU16 {
    let coordinate = |value: f64| value.clamp(0.0, f64::from(u16::MAX)) as u16;
    groups.bounds().fold(
        RectU16::new(0, 0, generator.width(), generator.height()),
        |window, bounds| {
            window.intersect(RectU16::new(
                coordinate(bounds.x0),
                coordinate(bounds.y0),
                coordinate(bounds.x1),
                coordinate(bounds.y1),
            ))
        },
    )
}

/// Mark `frame`'s retained bracket `index`, a `PushRetained` body, as one no
/// page may hold.
fn mark_unretainable(frame: &mut CompiledFrame, index: usize) {
    if let Some(RetainedBracket {
        kind: RetainedKind::Record { retainable, .. },
        ..
    }) = frame.retained.get_mut(index)
    {
        *retainable = false;
    }
}

/// Whether `device` lies wholly inside `window`.
fn rect_within(device: Rect, window: RectU16) -> bool {
    device.is_finite()
        && !window.is_empty()
        && device.x0 >= f64::from(window.x0)
        && device.y0 >= f64::from(window.y0)
        && device.x1 <= f64::from(window.x1)
        && device.y1 <= f64::from(window.y1)
}

/// `device` rounded out to whole tiles and clamped onto the `u16` grid — the
/// alignment a recorded layer's own bounds take.
fn tile_bounds(device: Rect) -> RectU16 {
    let snap = |value: f64, step: u16, up: bool| {
        let step = f64::from(step);
        let snapped = if up {
            (value / step).ceil() * step
        } else {
            (value / step).floor() * step
        };
        if snapped.is_finite() {
            snapped.clamp(0.0, f64::from(u16::MAX)) as u16
        } else {
            0
        }
    };
    RectU16::new(
        snap(device.x0, Tile::WIDTH, false),
        snap(device.y0, Tile::HEIGHT, false),
        snap(device.x1, Tile::WIDTH, true),
        snap(device.y1, Tile::HEIGHT, true),
    )
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

    /// The substitution both walks make for an off-grid `PushRetained`, and why
    /// making it in only one of them would matter.
    ///
    /// A bracket entered with the recorded presentation installs a correction;
    /// one entered neutrally installs none. That correction is precisely the
    /// affine a run inside the bracket is *classified* against, so a walk that
    /// substituted and a walk that did not would decide a run's route from one
    /// magnitude and draw it at another — which is the atlas route handed to a
    /// transform `glifo` will not absorb.
    #[test]
    fn an_off_grid_retained_bracket_is_entered_neutrally() {
        // A frame root already carrying an outer bracket's correction, and an
        // inner bracket whose own transform overflows against it. Both factors
        // are finite; only the composition is not.
        let combined = Affine::scale(1e200);
        let transform = Affine::scale(1e200);
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let command = Command::PushRetained {
            key: 0,
            rect,
            alpha: 1.0,
            scale: 2.0,
            transform,
        };

        assert!(!command_on_grid(&command, combined));
        assert!(command_on_grid(&command, Affine::IDENTITY));

        let (scale, entered) = retained_entry(false, 2.0, transform);
        assert_eq!(scale, 1.0);
        assert_eq!(entered.as_coeffs(), Affine::IDENTITY.as_coeffs());
        let (scale, entered) = retained_entry(true, 2.0, transform);
        assert_eq!(scale, 2.0);
        assert_eq!(entered.as_coeffs(), transform.as_coeffs());

        // And the difference reaches the quantity that is classified: an
        // outermost bracket entered with the recorded pair corrects, one
        // entered neutrally does not.
        let mut recorded = RetainedStack::new();
        recorded.enter(rect, 2.0, transform);
        assert_ne!(
            recorded.correction().as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );

        let mut neutral = RetainedStack::new();
        let (scale, entered) = retained_entry(false, 2.0, transform);
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
    /// [`note_shader_quad_unrendered`] first. `Once::call_once` itself is the
    /// standard-library guarantee behind the other half — that the closure
    /// inside it runs at most once ever — so calling the reporting function
    /// twice here and observing the latch hold is a structural stand-in for
    /// capturing and counting the actual log line.
    #[test]
    fn an_unrendered_shader_quad_is_latched_to_once_per_process() {
        note_shader_quad_unrendered(1);
        assert!(SHADER_QUAD_SKIP_WARNING.is_completed());
        note_shader_quad_unrendered(1);
        assert!(SHADER_QUAD_SKIP_WARNING.is_completed());
    }

    /// The kill switch's own report latches independently of the one above —
    /// two distinct facts about why a quad drew nothing, each stated once.
    #[test]
    fn a_disabled_shader_quad_is_latched_to_once_per_process() {
        note_shader_effects_disabled();
        assert!(SHADER_EFFECTS_DISABLED_WARNING.is_completed());
        note_shader_effects_disabled();
        assert!(SHADER_EFFECTS_DISABLED_WARNING.is_completed());
    }

    /// A shader quad's target id is derived, never minted, and lands in the
    /// half of the id space `SceneTextureId::mint` cannot reach — the whole
    /// reason the compiler can name a texture it never saw registered.
    #[test]
    fn a_shader_quad_target_id_is_derived_from_its_program() {
        assert_eq!(shader_quad_texture_id(7), shader_quad_texture_id(7));
        assert_ne!(shader_quad_texture_id(7), shader_quad_texture_id(8));
        assert_ne!(shader_quad_texture_id(7), 7);
        assert_eq!(
            shader_quad_texture_id(7),
            SceneTextureId::for_shader_program(7).get()
        );
    }

    #[test]
    fn shader_quad_is_culled_true_when_entirely_outside_the_frame() {
        assert!(shader_quad_is_culled(
            Rect::new(1000.0, 1000.0, 1008.0, 1008.0),
            Affine::IDENTITY,
            (64, 64)
        ));
    }

    #[test]
    fn shader_quad_is_culled_false_when_overlapping_the_frame() {
        assert!(!shader_quad_is_culled(
            Rect::new(0.0, 0.0, 8.0, 8.0),
            Affine::IDENTITY,
            (64, 64)
        ));
    }

    #[test]
    fn shader_quad_is_culled_false_touching_the_frame_edge() {
        // A shared edge counts as overlapping (kurbo::Rect::overlaps), so a
        // quad exactly abutting the frame boundary is never wrongly reported
        // culled.
        assert!(!shader_quad_is_culled(
            Rect::new(64.0, 0.0, 80.0, 16.0),
            Affine::IDENTITY,
            (64, 64)
        ));
    }

    #[test]
    fn shader_quad_is_culled_false_for_a_non_finite_bbox() {
        assert!(!shader_quad_is_culled(
            Rect::new(0.0, 0.0, f64::NAN, 8.0),
            Affine::IDENTITY,
            (64, 64)
        ));
    }

    #[test]
    fn a_culled_shader_quad_is_reported_at_debug_level_not_through_the_unrendered_warning() {
        // `note_shader_quad_culled` carries no process-latch to observe the
        // way `SHADER_QUAD_SKIP_WARNING` does above — it is meant to fire
        // every time, unlike the once-per-process missing-pre-pass report.
        // What is asserted here is the compile-time distinction itself: a
        // quad `shader_quad_is_culled` reports true for must never also read
        // as "un-rendered" by the same geometry.
        let frame_extent = (64, 64);
        let culled_dest = Rect::new(1000.0, 1000.0, 1008.0, 1008.0);
        let unrendered_dest = Rect::new(0.0, 0.0, 8.0, 8.0);

        assert!(shader_quad_is_culled(
            culled_dest,
            Affine::IDENTITY,
            frame_extent
        ));
        assert!(!shader_quad_is_culled(
            unrendered_dest,
            Affine::IDENTITY,
            frame_extent
        ));
    }
}
