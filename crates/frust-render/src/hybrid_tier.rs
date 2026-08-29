//! Experimental hybrid render tier: translation of a [`frust_scene::Scene`]
//! into a [`vello_hybrid`] `Scene`, rendered by `vello_hybrid`'s wgpu
//! `Renderer` into a caller-owned target view.
//!
//! Compiled only behind the non-default `hybrid-tier` feature (see
//! `frust-render/Cargo.toml`). The GPU (vello 0.9) path is unaffected when the
//! feature is off — this whole module, and every `vello_hybrid`/`vello_common`/
//! `glifo` type it names, then vanishes from the build.
//!
//! # Why this exists
//!
//! `vello_hybrid` rasterizes paths into sparse strips on the CPU and composites
//! them on the GPU, instead of vello's compute pipeline. Driving it from the
//! *same* command walk the GPU and CPU tiers already share
//! ([`SceneSink`](crate::convert::SceneSink)) is what lets the two be measured
//! against each other on real frust scenes rather than on a synthetic
//! benchmark: there is one command-coverage list, not three.
//!
//! # Structure
//!
//! Three layers, split so the mapping is testable without a GPU:
//!
//! - [`HybridSink`] implements [`SceneSink`](crate::convert::SceneSink) and
//!   holds *all* the lowering decisions (rounded rect → path, hole punch →
//!   destination-out blend layer, …), expressed against the primitive
//!   [`HybridOps`] seam.
//! - [`SceneOps`] is the production [`HybridOps`]: every primitive goes
//!   straight into a real `vello_hybrid::Scene`, which needs no device and so
//!   runs in the host tests below.
//! - [`HybridResources`] is the narrow seam for the two commands that *do*
//!   need renderer-owned GPU state — a glyph run (the renderer's glyph atlas)
//!   and an image (its image atlas). [`GpuResources`] is the real one;
//!   the tests substitute a host fake with an upload counter.
//!
//! # Command coverage vs. the GPU path
//!
//! Every `Command` variant routes through the shared walk, so coverage matches
//! the vello path by construction. Where `vello_hybrid` 0.2.0's API cannot
//! reproduce a GPU-path effect exactly, the downgrade is documented inline and
//! collected here:
//!
//! - **Image *brushes*** (a `Brush::Image` handed to a fill, as opposed to the
//!   dedicated `Command::Image`) are painted transparent, exactly as the CPU
//!   tier does — no shipping widget emits one.
//! - **`Command::Image`** goes through the residency shim below rather than a
//!   per-frame pixel upload: `ImageSource::Pixmap` is `unimplemented!()` in
//!   `vello_hybrid`'s paint packing, so the only usable source is an
//!   `ImageId` handle into the renderer's atlas.
//! - **Per-corner blurred shadows** collapse to one radius in the shared walk,
//!   same as both other tiers.
//! - **A clip is non-isolated** (`push_clip_path`, not `push_clip_layer`): no
//!   intermediate texture is allocated per clip, which is the whole point of
//!   the strip pipeline. An opacity layer still isolates, because opacity has
//!   to composite.
//!
//! # Image residency
//!
//! [`ImageResidency`] keys the renderer-assigned
//! [`ImageId`](vello_common::paint::ImageId) on the peniko `Blob` id of the
//! image's pixel data, which is stable for as long as a widget holds its
//! decoded `ImageSource`. An image therefore uploads on its first frame and
//! reuses its atlas slot on every later one; a handle unseen for
//! [`IMAGE_EVICTION_FRAMES`] frames is destroyed, releasing its atlas region.
//!
//! # Atlas capacity
//!
//! `Renderer::upload_image` is infallible in its signature and `unwrap`s the
//! allocation underneath, so an atlas that cannot fit one more image aborts the
//! process (the workspace builds with `panic = "abort"`). Nothing upstream is
//! either fallible or observable here — `Resources` keeps its `ImageCache`
//! private, so neither the occupancy nor the atlas count can be read back — so
//! [`ImageResidency`] keeps its own account of the pixels it has admitted and
//! refuses an upload that would take it past [`atlas_pixel_budget`]. A refused
//! image is skipped exactly like an oversized one (warn once, no draw); an
//! evicted one returns its pixels to the account.
//!
//! What that account can and cannot promise, precisely:
//!
//! - The allocator's one *guarantee* is the fresh-atlas one: with `auto_grow`
//!   on and the first-fit strategy, an image already inside the per-image
//!   dimension clamp always fits a newly created atlas, so an allocation made
//!   while the manager can still create one succeeds.
//! - That guarantee cannot be leaned on for long. The atlas count only grows —
//!   atlases are never destroyed — and is invisible from here, so past the first
//!   few uploads there is nothing left to prove.
//! - Beyond it the budget is *conservative, not a proof*. The backing allocator
//!   is guillotiere's guillotine tree: it refuses on shape rather than on area
//!   and documents its own coalescing as imperfect, so free area is not the same
//!   as a free rectangle. [`ATLAS_BUDGET_DIVISOR`] answers that with headroom
//!   rather than with a claim.
//! - The headroom is shared, not spare: with `FRUST_HYBRID_ATLAS_CACHE` on the
//!   glyph atlas allocates its slots from the *same* `ImageCache`.
//!
//! Glyph runs get no caller-side equivalent, because at this pin there is
//! nothing for one to do: glifo already degrades on a full atlas (its slot
//! allocation is an `ok()?`, and the run falls back to the uncached path), and
//! `vello_hybrid`'s one glyph-atlas `expect` sits behind
//! `FRUST_HYBRID_ATLAS_CACHE` — with the knob off the glyph atlas is never even
//! created. It fires on a render error from the atlas pass, not on exhaustion,
//! and upstream exposes no way to pre-check it. That residual is accepted and
//! knob-gated.
//!
//! # Instrumentation
//!
//! Two knobs, both resolved once per process:
//!
//! - `FRUST_HYBRID_ATLAS_CACHE` (`1`/`true`) turns on glifo's experimental
//!   glyph atlas cache for every glyph run this tier draws — upstream calls it
//!   "not recommended for external use", which is exactly why it is a knob and
//!   not a default. The effective value is logged once in any build.
//! - Under the `perf-trace` feature AND `FRUST_TRACE`, [`HybridTierRenderer::render`]
//!   emits one `frust-perf hybrid strip_us=<n> record_us=<n>` line per frame,
//!   splitting the frame into the CPU half (scene reset → sparse-strip
//!   rasterization through the shared command walk) and the GPU half (the
//!   renderer's own pass recording). The image-eviction pass between them
//!   belongs to neither window.
//!
//! # Base color
//!
//! `Renderer::render` *does* clear the target view — to transparent black, from
//! its own pass, before the strip passes load over it — and it owns no
//! `base_color` render parameter, so nothing in it can paint an app background.
//! The ground draw is therefore load-bearing rather than belt-and-braces:
//! [`HybridTierRenderer`] fills the whole scene with the frame's base color as
//! its first draw, and without it an opaque background would composite over
//! transparent black. The CPU tier draws its own ground for the same reason.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use frust_scene::{GlyphRun, Scene};
use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke};
use peniko::{BlendMode, Brush, Color, Fill, ImageData, ImageFormat};
use vello_common::paint::{ImageId, ImageSource, PaintType};
use vello_common::pixmap::Pixmap;
use vello_hybrid::{
    AtlasConfig, RenderError, RenderSize, RenderTargetConfig, Renderer, Resources, TextureBindings,
};

use crate::convert::{SceneSink, encode_into};

/// Flattening tolerance (logical px) for the shapes handed to `vello_hybrid` as
/// `BezPath`s. Its rect fill is a concrete fast path, but rounded rects, lines
/// and clips all want a path, so we pre-flatten — the same tolerance the CPU
/// tier uses, so the two tiers' curves match.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// The transparent fallback paint used where `vello_hybrid` has no equivalent
/// for a scene brush (currently only `Brush::Image` handed to a fill; see the
/// module docs' downgrade note).
const FALLBACK_PAINT: Color = Color::new([0.0, 0.0, 0.0, 0.0]);

/// How many consecutive frames an uploaded image may go undrawn before its
/// atlas slot is released.
///
/// A frame count, not a duration, and the difference is real: it buys 1.00 s at
/// 60 Hz, 0.67 s at 90 Hz and 0.50 s at 120 Hz, so the horizon *shrinks* as the
/// panel gets faster. An image carried off-screen and back over a full second on
/// a 120 Hz panel is re-uploaded, which is the accepted cost — re-uploading an
/// already-decoded pixmap — against an unconditional per-frame `Instant::now()`
/// that a wall-clock horizon would put on the frame path, which the
/// Instrumentation conventions in `docs/CODE_STANDARDS.md` keep behind a trace
/// gate. Sized so the common case survives on the 60 Hz panels this tier is
/// being measured on: a fling that carries an image just past the edge and back.
const IMAGE_EVICTION_FRAMES: u64 = 60;

/// The share of the image atlas array's nominal pixel capacity this tier lets
/// live uploads occupy: one `ATLAS_BUDGET_DIVISOR`th of it.
///
/// Halving it is headroom, not waste. A guillotine allocator refuses on the
/// shape of its free rectangles rather than on their total area, so the account
/// in [`ImageResidency`] has to leave room for fragmentation it cannot see; and
/// the glyph atlas draws slots from the same `ImageCache` whenever
/// `FRUST_HYBRID_ATLAS_CACHE` is on. Even halved the ceiling is far past any
/// real scene — 8 x 4096 x 4096 / 2 is 67 Mpx of resident image — so the guard
/// bites on a leak or an adversarial scene, not on a photo gallery.
const ATLAS_BUDGET_DIVISOR: u64 = 2;

/// Whether glifo's experimental glyph atlas cache is enabled for this
/// process's glyph runs, resolved once from `FRUST_HYBRID_ATLAS_CACHE`
/// (compile-time `option_env!` or runtime env var, see
/// [`crate::context::env_str`]) and cached — a change requires a fresh
/// process, matching every other `FRUST_*` knob.
///
/// `1` and `true` (case-insensitive, surrounding whitespace trimmed) enable
/// it; every other value, including unset, leaves glifo's own default, which
/// is off. There is deliberately no warn-on-unrecognised arm: unlike
/// `FRUST_AA_MODE`'s named modes this is a plain flag, and the effective value
/// is logged either way (see [`HybridTierRenderer::new`]).
fn atlas_cache_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        parse_atlas_cache(crate::context::env_str(
            option_env!("FRUST_HYBRID_ATLAS_CACHE"),
            std::env::var("FRUST_HYBRID_ATLAS_CACHE").ok(),
        ))
    })
}

/// Parses a raw `FRUST_HYBRID_ATLAS_CACHE` value (already resolved by
/// [`crate::context::env_str`]). Split out from [`atlas_cache_enabled`]'s
/// `OnceLock` so the parsing is testable without a process-wide env.
fn parse_atlas_cache(raw: Option<String>) -> bool {
    raw.as_deref()
        .map(str::trim)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true"))
}

/// Whether the per-frame `frust-perf hybrid` span line is enabled: the
/// process-wide `FRUST_TRACE` flag, compile-time-or-runtime, resolved once and
/// cached.
///
/// Mirrors `context::perf_tracing_enabled`, which is private to that module —
/// the two must answer identically, so a change to `FRUST_TRACE`'s parsing
/// belongs in both. Only compiled under the `perf-trace` feature, so a build
/// without it carries neither this env read nor the `"frust-perf …"` literal
/// it guards; that is the same inert-counterpart shape `context.rs`'s own
/// probes use.
#[cfg(feature = "perf-trace")]
fn frame_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        fn is_set_non_zero(value: Option<&str>) -> bool {
            matches!(value, Some(v) if v != "0")
        }
        is_set_non_zero(option_env!("FRUST_TRACE"))
            || is_set_non_zero(std::env::var("FRUST_TRACE").ok().as_deref())
    })
}

/// A [`vello_hybrid`]-backed render tier: owns the reusable hybrid scene, the
/// renderer and its persistent resources, and the image-residency map, so a
/// frame allocates nothing of its own (mirroring the GPU path's reused
/// `vello::Scene`).
///
/// Dimensions are `u16` (the hybrid scene's native size type); larger surfaces
/// are clamped to `u16::MAX` — far beyond any real window, so the clamp is a
/// safety net, not an expected path.
pub(crate) struct HybridTierRenderer {
    scene: vello_hybrid::Scene,
    renderer: Renderer,
    resources: Resources,
    /// Bindings for externally-owned textures sampled by texture-rect draws.
    /// The scene walk emits none, so this stays empty; it is held rather than
    /// built per frame because `render` takes it by reference.
    bindings: TextureBindings,
    images: ImageResidency,
    /// Whether every glyph run this renderer draws asks glifo for its
    /// atlas-backed glyph cache — resolved once at construction from
    /// [`atlas_cache_enabled`] and held here so the per-frame path reads a
    /// plain `bool` rather than a `OnceLock`.
    atlas_cache: bool,
    width: u16,
    height: u16,
}

/// Clamps a `(u32, u32)` surface size to the `(u16, u16)` the hybrid scene
/// wants, with a floor of 1 (a zero-sized scene is invalid).
fn clamp_dims(width: u32, height: u32) -> (u16, u16) {
    let w = width.clamp(1, u16::MAX as u32) as u16;
    let h = height.clamp(1, u16::MAX as u32) as u16;
    (w, h)
}

impl HybridTierRenderer {
    /// Creates a renderer targeting `format` at `width`x`height` device pixels.
    ///
    /// `format` is baked into the renderer's pipelines, so a target whose
    /// format changes needs a new `HybridTierRenderer`; a size change does not
    /// (see [`resize`](Self::resize)).
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let (w, h) = clamp_dims(width, height);
        let (renderer, resources) = Renderer::new(
            device,
            &RenderTargetConfig {
                format,
                width: u32::from(w),
                height: u32::from(h),
            },
        );
        // One line per process naming the knob's effective value, beside the
        // tier/aa-mode/render-scale lines and in any build (no `perf-trace`
        // needed) — a capture read hours later should not have to infer which
        // glyph path produced its numbers. A non-default value also warns
        // once, the same escalation the other measurement knobs use, because
        // upstream calls this cache experimental and not recommended.
        let atlas_cache = atlas_cache_enabled();
        static ATLAS_CACHE_LOGGED: OnceLock<()> = OnceLock::new();
        ATLAS_CACHE_LOGGED.get_or_init(|| {
            log::info!(
                "frust-render hybrid atlas-cache={}",
                if atlas_cache { "on" } else { "off" }
            );
            if atlas_cache {
                log::warn!(
                    "frust-render measurement knob in effect: FRUST_HYBRID_ATLAS_CACHE=1 \
                     (glifo's experimental glyph atlas cache, upstream-flagged as not \
                     recommended for external use)"
                );
            }
        });
        Self {
            scene: vello_hybrid::Scene::new(w, h),
            renderer,
            resources,
            bindings: TextureBindings::new(),
            images: ImageResidency::new(
                max_atlas_image_dimension(device),
                atlas_pixel_budget(device),
            ),
            atlas_cache,
            width: w,
            height: h,
        }
    }

    /// Resizes the render target if `width`x`height` differs from the current
    /// size. The hybrid scene resizes in place; the renderer re-derives its own
    /// depth texture and config buffer from the per-frame `RenderSize`, so
    /// there is nothing to rebuild here.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = clamp_dims(width, height);
        if w == self.width && h == self.height {
            return;
        }
        self.scene.reset_and_resize(w, h);
        self.width = w;
        self.height = h;
    }

    /// Encodes `scene` into the hybrid scene (over a `base_color` ground) and
    /// records its GPU work into `encoder`, targeting `target_view`.
    ///
    /// The encoder is borrowed, not submitted: image uploads, atlas writes and
    /// the render passes all land in the caller's command buffer, so the caller
    /// keeps control of submission order (and of the surface texture the view
    /// belongs to).
    ///
    /// Errors are returned, never panicked — this runs on the frame path.
    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        base_color: Color,
        target_view: &wgpu::TextureView,
    ) -> Result<(), RenderError> {
        // Window one opens here: scene reset, base-color ground and the whole
        // shared command walk, i.e. every sparse strip `vello_hybrid`
        // rasterizes on the CPU for this frame. One clock read, and only when
        // tracing is on (`bool::then`, the frame-path convention in
        // `docs/CODE_STANDARDS.md`'s Instrumentation section).
        #[cfg(feature = "perf-trace")]
        let strip_start = frame_trace_enabled().then(std::time::Instant::now);

        self.scene.reset();
        self.images.begin_frame();

        // `vello_hybrid` clears the target view to transparent black and has no
        // `base_color` render parameter (unlike vello's `RenderParams`), so the
        // frame's background has to be a scene draw: the ground goes first.
        self.scene.set_transform(Affine::IDENTITY);
        self.scene.set_fill_rule(Fill::NonZero);
        self.scene.set_paint(PaintType::Solid(base_color));
        self.scene.fill_rect(&Rect::new(
            0.0,
            0.0,
            f64::from(self.width),
            f64::from(self.height),
        ));

        {
            let Self {
                scene: hybrid_scene,
                renderer,
                resources,
                images,
                atlas_cache,
                ..
            } = self;
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: hybrid_scene,
                    resources: GpuResources {
                        renderer,
                        resources,
                        images,
                        device,
                        queue,
                        encoder,
                        atlas_cache: *atlas_cache,
                    },
                },
            };
            // Reuse the *same* command walk the GPU path uses (`convert`), so
            // command coverage stays single-sourced.
            encode_into(scene, &mut sink);
        }
        // Window one closes with the walk: the eviction pass below is neither
        // CPU strip work nor the composite record, so it is charged to neither
        // window (it is also a no-op on the overwhelmingly common frame).
        #[cfg(feature = "perf-trace")]
        let strip_us = strip_start.map(|start| start.elapsed().as_micros());

        // Release atlas slots whose images have gone unused. Collected first so
        // the residency map is no longer borrowed while the renderer clears the
        // regions; the vector stays unallocated on the overwhelmingly common
        // frame that evicts nothing.
        let mut expired = Vec::new();
        self.images.evict_stale(|id| expired.push(id));
        for id in expired {
            self.renderer
                .destroy_image(&mut self.resources, encoder, id);
        }

        // Window two: the renderer's own recording of the composite passes
        // into the caller's encoder. GPU execution is NOT in it — the caller
        // owns submission — so this is record cost, which is what the name
        // says.
        #[cfg(feature = "perf-trace")]
        let record_start = strip_us.map(|_| std::time::Instant::now());

        let recorded = self.renderer.render(
            &self.scene,
            &mut self.resources,
            device,
            queue,
            encoder,
            &RenderSize {
                width: u32::from(self.width),
                height: u32::from(self.height),
            },
            target_view,
            &self.bindings,
        );

        // One line per frame, both windows on it, through the same
        // `log::info!` path every other `frust-perf` line uses. A failed
        // record is not reported: its windows time an abandoned frame, and a
        // stats reader has no way to tell that from a real one.
        #[cfg(feature = "perf-trace")]
        if recorded.is_ok()
            && let (Some(strip_us), Some(record_start)) = (strip_us, record_start)
        {
            log::info!(
                "frust-perf hybrid strip_us={strip_us} record_us={}",
                record_start.elapsed().as_micros()
            );
        }

        recorded
    }

    /// The current target width (device px). Test-only.
    #[cfg(test)]
    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    /// The current target height (device px). Test-only.
    #[cfg(test)]
    pub(crate) fn height(&self) -> u16 {
        self.height
    }
}

/// The largest image edge the renderer's image atlas can hold: its configured
/// atlas size, clamped to what the device allows (`vello_hybrid` applies the
/// same clamp to the atlas it actually creates). Anything larger cannot be
/// allocated, and `Renderer::upload_image` panics rather than reporting that,
/// so [`ImageResidency`] refuses it up front.
fn max_atlas_image_dimension(device: &wgpu::Device) -> u32 {
    let (atlas_width, atlas_height) = AtlasConfig::default().atlas_size;
    atlas_width
        .min(atlas_height)
        .min(device.limits().max_texture_dimension_2d)
}

/// How many atlas pixels [`ImageResidency`] may hold live at once on `device`:
/// [`ATLAS_BUDGET_DIVISOR`]'s share of the capacity `vello_hybrid` will actually
/// configure there. See the module docs' Atlas capacity section for what the
/// budget does and does not promise.
fn atlas_pixel_budget(device: &wgpu::Device) -> u64 {
    let limits = device.limits();
    atlas_pixel_budget_from(
        AtlasConfig::default(),
        limits.max_texture_dimension_2d,
        limits.max_texture_array_layers,
    )
}

/// The pixel ceiling `config` implies once a device allowing
/// `max_texture_dimension_2d`-wide textures and `max_texture_array_layers`
/// atlases has had its say.
///
/// Mirrors `vello_hybrid`'s own `MemorySettings::normalize`, which clamps the
/// config against exactly those two limits before the renderer's `ImageCache`
/// is built — reading them here is what attaches the account to the atlas that
/// exists rather than to the default on paper. Split out from
/// [`atlas_pixel_budget`] so the arithmetic is testable without a device, the
/// same split [`parse_atlas_cache`] uses.
fn atlas_pixel_budget_from(
    config: AtlasConfig,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
) -> u64 {
    let (atlas_width, atlas_height) = config.atlas_size;
    let width = u64::from(atlas_width.min(max_texture_dimension_2d));
    let height = u64::from(atlas_height.min(max_texture_dimension_2d));
    let atlases = u64::try_from(config.max_atlases)
        .unwrap_or(u64::MAX)
        .min(u64::from(max_texture_array_layers));
    // A device reporting no atlas layers at all yields a budget of zero, which
    // refuses every upload — the conservative answer, since there would be no
    // atlas to allocate from either.
    width.saturating_mul(height).saturating_mul(atlases) / ATLAS_BUDGET_DIVISOR
}

/// The atlas pixels one upload of `pixmap` occupies.
///
/// `vello_hybrid`'s wgpu path allocates with zero padding at this pin, so the
/// padded footprint the allocator is asked for is exactly the image's own pixel
/// count. That constant is private upstream, so a pin that gave it a non-zero
/// value would turn this into an under-count — one more thing
/// [`ATLAS_BUDGET_DIVISOR`]'s headroom absorbs.
fn atlas_pixels(pixmap: &Pixmap) -> u64 {
    u64::from(pixmap.width()) * u64::from(pixmap.height())
}

/// Renderer-owned GPU state the sink reaches for on the only two commands that
/// need it, behind a seam so the rest of the mapping runs on the host.
pub(crate) trait HybridResources {
    /// Fill `run`'s glyphs into `scene`, which needs the renderer's glyph
    /// atlas. The caller has already applied the run's transform and brush to
    /// `scene`'s paint state — the glyph-run builder reads both from there.
    fn fill_glyphs(&mut self, scene: &mut vello_hybrid::Scene, run: &GlyphRun);

    /// Resolve `data` to an atlas-resident image source, uploading it on first
    /// sight. `None` when the image cannot be made resident (see
    /// [`ImageResidency::source`]), in which case the draw is skipped.
    fn image_source(&mut self, data: &ImageData) -> Option<ImageSource>;
}

/// The real [`HybridResources`]: uploads through the renderer into its image
/// atlas and draws glyph runs through its glyph atlas.
struct GpuResources<'a> {
    renderer: &'a mut Renderer,
    resources: &'a mut Resources,
    images: &'a mut ImageResidency,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    encoder: &'a mut wgpu::CommandEncoder,
    /// The renderer's resolved `FRUST_HYBRID_ATLAS_CACHE` value, lent per
    /// frame — glifo takes it per glyph run, not per renderer.
    atlas_cache: bool,
}

impl HybridResources for GpuResources<'_> {
    fn fill_glyphs(&mut self, scene: &mut vello_hybrid::Scene, run: &GlyphRun) {
        scene
            .glyph_run(self.resources, run.font.font())
            .font_size(run.font_size)
            .hint(false)
            // Per-run, not per-renderer: `vello_hybrid` builds each glyph run
            // with the cache off and glifo exposes the choice only on the
            // builder, so the renderer's resolved knob is re-applied here on
            // every run rather than once at construction.
            .atlas_cache(self.atlas_cache)
            .fill_glyphs(run.glyphs.iter().map(|g| glifo::Glyph {
                id: g.id,
                x: g.x,
                y: g.y,
            }));
    }

    fn image_source(&mut self, data: &ImageData) -> Option<ImageSource> {
        // Reborrowed field by field so the upload closure and the residency map
        // borrow disjoint parts of `self`.
        let renderer = &mut *self.renderer;
        let resources = &mut *self.resources;
        let encoder = &mut *self.encoder;
        let device = self.device;
        let queue = self.queue;
        self.images.source(data, |pixmap| {
            Some(renderer.upload_image(resources, device, queue, encoder, pixmap))
        })
    }
}

/// Lets a caller lend its resources to a sink rather than move them in — the
/// host tests drive several frames through one set.
impl<T: HybridResources + ?Sized> HybridResources for &mut T {
    fn fill_glyphs(&mut self, scene: &mut vello_hybrid::Scene, run: &GlyphRun) {
        (**self).fill_glyphs(scene, run);
    }

    fn image_source(&mut self, data: &ImageData) -> Option<ImageSource> {
        (**self).image_source(data)
    }
}

/// One image resident in the renderer's atlas.
struct Resident {
    id: ImageId,
    /// Cached from the uploaded pixmap: re-deriving it would mean re-decoding
    /// the image every frame, and the opaque case lets the renderer take its
    /// opaque draw path.
    may_have_transparency: bool,
    /// The atlas pixels this upload was charged, held so eviction returns
    /// exactly what admission took rather than a re-derived guess.
    atlas_pixels: u64,
    /// The frame counter value when this image was last drawn.
    last_seen: u64,
}

/// The image-residency shim: maps a peniko `Blob` id to the renderer-assigned
/// [`ImageId`] of its atlas upload, so a widget's cached image uploads once
/// rather than once per frame.
///
/// The `Blob` id is the right key: it is stable for the lifetime of the decoded
/// pixel buffer a widget holds, and two `ImageData` values sharing a blob share
/// pixels by construction. (Re-wrapping the same bytes in a *new* blob is a new
/// id and so a second upload — that is a widget-side cache miss, not something
/// this map can or should paper over.)
struct ImageResidency {
    resident: HashMap<u64, Resident>,
    /// Monotonic frame counter driven by [`begin_frame`](Self::begin_frame);
    /// eviction is expressed in terms of it rather than wall-clock time.
    frame: u64,
    /// Largest image edge the atlas can hold (see
    /// [`max_atlas_image_dimension`]).
    max_dimension: u32,
    /// Atlas pixels currently admitted: the sum of `atlas_pixels` over
    /// [`resident`](Self::resident), maintained incrementally so admitting an
    /// image stays O(1) on the frame path.
    live_pixels: u64,
    /// The ceiling `live_pixels` may not cross (see [`atlas_pixel_budget`], and
    /// the module docs' Atlas capacity section for what it promises).
    pixel_budget: u64,
}

impl ImageResidency {
    fn new(max_dimension: u32, pixel_budget: u64) -> Self {
        Self {
            resident: HashMap::new(),
            frame: 0,
            max_dimension,
            live_pixels: 0,
            pixel_budget,
        }
    }

    /// Opens a frame. Every [`source`](Self::source) hit after this marks its
    /// image as used in the new frame.
    fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The atlas-resident source for `data`, uploading through `upload` on the
    /// first frame the image is seen and reusing the returned handle after.
    ///
    /// `None` — the draw is skipped, never panicked — when the image cannot be
    /// made resident: a degenerate size, an edge larger than the atlas, a pixel
    /// format `vello_common` does not convert, a buffer whose length disagrees
    /// with its declared size, no room left in the atlas budget, or a refused
    /// upload. Each of those is a panic inside `vello_common`'s own conversion
    /// or the renderer's allocation, so they are checked here rather than
    /// discovered on the frame path.
    fn source(
        &mut self,
        data: &ImageData,
        upload: impl FnOnce(&Arc<Pixmap>) -> Option<ImageId>,
    ) -> Option<ImageSource> {
        let key = data.data.id();
        if let Some(resident) = self.resident.get_mut(&key) {
            resident.last_seen = self.frame;
            return Some(ImageSource::opaque_id_with_transparency_hint(
                resident.id,
                resident.may_have_transparency,
            ));
        }

        let pixmap = to_pixmap(data, self.max_dimension)?;
        // Charged before the upload, never after: `upload_image` cannot report a
        // refusal, so the only place this image can still be turned away is
        // here.
        let atlas_pixels = atlas_pixels(&pixmap);
        if self.live_pixels.saturating_add(atlas_pixels) > self.pixel_budget {
            warn_once(
                &IMAGE_ATLAS_FULL,
                "hybrid tier: image atlas budget is full; images will not be drawn until \
                 resident ones are evicted",
            );
            return None;
        }
        let may_have_transparency = pixmap.may_have_transparency();
        let id = upload(&pixmap)?;
        self.live_pixels += atlas_pixels;
        self.resident.insert(
            key,
            Resident {
                id,
                may_have_transparency,
                atlas_pixels,
                last_seen: self.frame,
            },
        );
        Some(ImageSource::opaque_id_with_transparency_hint(
            id,
            may_have_transparency,
        ))
    }

    /// Drops every handle unseen for [`IMAGE_EVICTION_FRAMES`] frames, handing
    /// each to `destroy` so its atlas region can be released and returning its
    /// pixels to the capacity account.
    fn evict_stale(&mut self, mut destroy: impl FnMut(ImageId)) {
        let frame = self.frame;
        // Summed into a local because the account lives beside the map `retain`
        // is holding.
        let mut reclaimed = 0;
        self.resident.retain(|_, resident| {
            if frame.saturating_sub(resident.last_seen) < IMAGE_EVICTION_FRAMES {
                return true;
            }
            reclaimed += resident.atlas_pixels;
            destroy(resident.id);
            false
        });
        self.live_pixels = self.live_pixels.saturating_sub(reclaimed);
    }

    /// How many images are currently resident. Test-only.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.resident.len()
    }
}

/// Converts `data` into the premultiplied pixmap the atlas uploader takes,
/// or `None` when the conversion would be invalid — see
/// [`ImageResidency::source`] for why each case is checked here.
fn to_pixmap(data: &ImageData, max_dimension: u32) -> Option<Arc<Pixmap>> {
    if data.width == 0 || data.height == 0 {
        return None;
    }
    if data.width > max_dimension || data.height > max_dimension {
        warn_once(
            &IMAGE_TOO_LARGE,
            "hybrid tier: image exceeds the renderer's atlas dimensions and will not be drawn",
        );
        return None;
    }
    if !matches!(data.format, ImageFormat::Rgba8 | ImageFormat::Bgra8) {
        warn_once(
            &UNSUPPORTED_IMAGE_FORMAT,
            "hybrid tier: unsupported image pixel format; the image will not be drawn",
        );
        return None;
    }
    // `from_peniko_image_data` walks the buffer in 4-byte chunks and asserts the
    // resulting pixel count matches `width * height`.
    if data.format.size_in_bytes(data.width, data.height)? != data.data.data().len() {
        return None;
    }
    match ImageSource::from_peniko_image_data(data) {
        ImageSource::Pixmap(pixmap) => Some(pixmap),
        // The conversion above always yields pixels; an id-backed source cannot
        // be uploaded and would mean the upstream conversion changed shape.
        ImageSource::OpaqueId { .. } => None,
    }
}

static IMAGE_TOO_LARGE: OnceLock<()> = OnceLock::new();
static UNSUPPORTED_IMAGE_FORMAT: OnceLock<()> = OnceLock::new();
static IMAGE_ATLAS_FULL: OnceLock<()> = OnceLock::new();

/// Logs `message` the first time its `slot` is reached — a rejected image
/// repeats every frame, and a per-frame warn would be its own performance
/// problem on the path being measured.
fn warn_once(slot: &OnceLock<()>, message: &str) {
    slot.get_or_init(|| log::warn!("{message}"));
}

/// The primitive `vello_hybrid` operations every scene command lowers into.
///
/// A seam rather than direct calls on `vello_hybrid::Scene`, so the lowering in
/// [`HybridSink`] can be asserted per command variant with no GPU and no
/// renderer (mirroring how [`SceneSink`](crate::convert::SceneSink) itself is
/// tested in `convert.rs`).
trait HybridOps {
    fn set_transform(&mut self, transform: Affine);
    fn set_fill_rule(&mut self, fill_rule: Fill);
    fn set_paint(&mut self, paint: PaintType);
    fn set_paint_transform(&mut self, transform: Affine);
    fn reset_paint_transform(&mut self);
    fn set_stroke_width(&mut self, width: f64);
    fn fill_rect(&mut self, rect: &Rect);
    fn fill_path(&mut self, path: &BezPath);
    fn stroke_path(&mut self, path: &BezPath);
    fn fill_blurred_rounded_rect(&mut self, rect: &Rect, radius: f32, std_dev: f32);
    fn push_clip_path(&mut self, path: &BezPath);
    fn pop_clip_path(&mut self);
    /// An isolated layer clipped to `clip` and composited at `alpha`.
    fn push_opacity_layer(&mut self, clip: &BezPath, alpha: f32);
    /// An isolated layer composited with `blend`.
    fn push_blend_layer(&mut self, blend: BlendMode);
    fn pop_layer(&mut self);
    fn glyph_run(&mut self, run: &GlyphRun);
    fn image_source(&mut self, data: &ImageData) -> Option<ImageSource>;
}

/// The production [`HybridOps`]: drives a real `vello_hybrid::Scene`, reaching
/// through `R` for the two renderer-backed operations.
struct SceneOps<'a, R: HybridResources> {
    scene: &'a mut vello_hybrid::Scene,
    resources: R,
}

impl<R: HybridResources> HybridOps for SceneOps<'_, R> {
    fn set_transform(&mut self, transform: Affine) {
        self.scene.set_transform(transform);
    }

    fn set_fill_rule(&mut self, fill_rule: Fill) {
        self.scene.set_fill_rule(fill_rule);
    }

    fn set_paint(&mut self, paint: PaintType) {
        self.scene.set_paint(paint);
    }

    fn set_paint_transform(&mut self, transform: Affine) {
        self.scene.set_paint_transform(transform);
    }

    fn reset_paint_transform(&mut self) {
        self.scene.reset_paint_transform();
    }

    fn set_stroke_width(&mut self, width: f64) {
        self.scene.set_stroke(Stroke::new(width));
    }

    fn fill_rect(&mut self, rect: &Rect) {
        self.scene.fill_rect(rect);
    }

    fn fill_path(&mut self, path: &BezPath) {
        self.scene.fill_path(path);
    }

    fn stroke_path(&mut self, path: &BezPath) {
        self.scene.stroke_path(path);
    }

    fn fill_blurred_rounded_rect(&mut self, rect: &Rect, radius: f32, std_dev: f32) {
        // `invert: false` — an ordinary drop shadow; the inverted form is the
        // inset-shadow primitive, which no scene command lowers to.
        self.scene
            .fill_blurred_rounded_rect(rect, radius, std_dev, false);
    }

    fn push_clip_path(&mut self, path: &BezPath) {
        // Deliberately the clip *stack*, not a clip layer: a clip layer would
        // allocate an intermediate texture per clip, and frust scenes nest
        // clips freely (every scroll viewport is one).
        self.scene.push_clip_path(path);
    }

    fn pop_clip_path(&mut self) {
        self.scene.pop_clip_path();
    }

    fn push_opacity_layer(&mut self, clip: &BezPath, alpha: f32) {
        // Clip and opacity in ONE layer: the vello path's `push_layer` clips to
        // the layer rect as well as fading it, and an opacity layer has to
        // isolate anyway, so carrying the clip here costs nothing extra and
        // keeps the two tiers' output comparable.
        self.scene
            .push_layer(Some(clip), None, Some(alpha), None, None);
    }

    fn push_blend_layer(&mut self, blend: BlendMode) {
        // `Scene::set_blend_mode` asserts on destructive modes; a blend *layer*
        // is the sanctioned route for them.
        self.scene.push_blend_layer(blend);
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn glyph_run(&mut self, run: &GlyphRun) {
        self.resources.fill_glyphs(self.scene, run);
    }

    fn image_source(&mut self, data: &ImageData) -> Option<ImageSource> {
        self.resources.image_source(data)
    }
}

/// A [`SceneSink`](crate::convert::SceneSink) that lowers each scene command
/// into [`HybridOps`] primitives.
///
/// `vello_hybrid` is a state machine (set paint/transform/stroke, then draw),
/// like `vello_cpu` and unlike vello's per-call parameters, so each method sets
/// the state it depends on fresh before drawing.
struct HybridSink<O: HybridOps> {
    ops: O,
}

/// The hybrid paint for a scene [`Brush`]. `Solid`/`Gradient` map straight
/// across (`vello_common`'s paint type is the same `peniko::Brush` enum over a
/// different image type); an image brush handed to a fill falls back to
/// transparent (see the module docs' downgrade note).
fn paint_of(brush: &Brush) -> PaintType {
    match brush {
        Brush::Solid(color) => PaintType::Solid(*color),
        Brush::Gradient(gradient) => PaintType::Gradient(gradient.clone()),
        Brush::Image(_) => PaintType::Solid(FALLBACK_PAINT),
    }
}

impl<O: HybridOps> SceneSink for HybridSink<O> {
    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect) {
        self.ops.set_transform(transform);
        self.ops.set_fill_rule(style);
        self.ops.set_paint(paint_of(brush));
        // The concrete rect fast path: axis-aligned rects skip strip generation
        // entirely and are drawn as GPU rectangles.
        self.ops.fill_rect(rect);
    }

    fn fill_rounded_rect(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radii: RoundedRectRadii,
    ) {
        let path = RoundedRect::from_rect(*rect, radii).to_path(FLATTEN_TOLERANCE);
        self.ops.set_transform(transform);
        self.ops.set_fill_rule(style);
        self.ops.set_paint(paint_of(brush));
        self.ops.fill_path(&path);
    }

    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64) {
        let path = Line::new(p0, p1).to_path(FLATTEN_TOLERANCE);
        self.ops.set_transform(transform);
        self.ops.set_paint(paint_of(brush));
        self.ops.set_stroke_width(width);
        self.ops.stroke_path(&path);
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun) {
        // The glyph-run builder reads the scene's current transform and paint,
        // so both are set before the run rather than passed to it.
        self.ops.set_transform(run.transform);
        self.ops.set_paint(paint_of(&run.brush));
        self.ops.glyph_run(run);
    }

    fn push_clip(&mut self, transform: Affine, rect: &Rect) {
        // The clip path is generated under the transform current at push time,
        // so it is set first — same ordering contract as a draw.
        self.ops.set_transform(transform);
        self.ops.push_clip_path(&rect.to_path(FLATTEN_TOLERANCE));
    }

    fn push_clip_rounded(&mut self, transform: Affine, rect: &Rect, radii: RoundedRectRadii) {
        let path = RoundedRect::from_rect(*rect, radii).to_path(FLATTEN_TOLERANCE);
        self.ops.set_transform(transform);
        self.ops.push_clip_path(&path);
    }

    fn pop_clip(&mut self) {
        self.ops.pop_clip_path();
    }

    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
        let natural_w = f64::from(data.width);
        let natural_h = f64::from(data.height);
        if natural_w <= 0.0 || natural_h <= 0.0 || dest.width() <= 0.0 || dest.height() <= 0.0 {
            return;
        }
        // Resolve residency BEFORE touching paint state, so a refused image
        // leaves the scene exactly as it was.
        let Some(source) = self.ops.image_source(data) else {
            return;
        };
        let image = vello_common::paint::Image {
            image: source,
            sampler: peniko::ImageSampler::default(),
        };
        // The widget transform maps the local dest rect onto the screen and
        // applies to the fill shape. The *paint* transform maps the image's
        // natural pixel space onto that same local dest, and the image is
        // sampled through `transform * paint_transform`.
        let paint_transform = Affine::translate((dest.x0, dest.y0))
            * Affine::scale_non_uniform(dest.width() / natural_w, dest.height() / natural_h);
        self.ops.set_transform(transform);
        self.ops.set_fill_rule(Fill::NonZero);
        self.ops.set_paint(PaintType::Image(image));
        self.ops.set_paint_transform(paint_transform);
        self.ops.fill_rect(dest);
        self.ops.reset_paint_transform();
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: &Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.ops.set_transform(transform);
        // The blurred-rect primitive reads a solid paint only (a non-solid one
        // falls back to black inside it), which is all the scene command
        // carries anyway.
        self.ops.set_paint(PaintType::Solid(color));
        self.ops
            .fill_blurred_rounded_rect(rect, radius as f32, std_dev as f32);
    }

    fn push_layer(&mut self, transform: Affine, rect: &Rect, alpha: f32) {
        self.ops.set_transform(transform);
        self.ops
            .push_opacity_layer(&rect.to_path(FLATTEN_TOLERANCE), alpha);
    }

    fn pop_layer(&mut self) {
        self.ops.pop_layer();
    }

    fn clear_rect(&mut self, transform: Affine, rect: &Rect) {
        // The hole-punch: a layer composited with `Compose::DestOut` and an
        // OPAQUE fill inside erases the destination exactly where the fill
        // covers — `dst' = dst*(1-src.a)` — so the fill's own antialiased
        // coverage keeps the erase pixel-exact at the edges, and pixels the
        // layer never paints are untouched. Deliberately NOT `Compose::Clear`,
        // matching both other tiers (see `convert.rs`'s vello impl for the
        // tile-granularity bleed that ruled Clear out).
        //
        // A blend LAYER rather than `set_blend_mode`, which asserts on
        // destructive modes.
        self.ops.set_transform(transform);
        self.ops.push_blend_layer(BlendMode::new(
            peniko::Mix::Normal,
            peniko::Compose::DestOut,
        ));
        self.ops.set_fill_rule(Fill::NonZero);
        self.ops.set_paint(PaintType::Solid(Color::BLACK));
        self.ops.fill_rect(rect);
        self.ops.pop_layer();
    }

    fn fill_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath) {
        self.ops.set_transform(transform);
        self.ops.set_fill_rule(Fill::NonZero);
        self.ops.set_paint(paint_of(brush));
        self.ops.fill_path(path);
    }

    fn stroke_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath, width: f64) {
        self.ops.set_transform(transform);
        self.ops.set_paint(paint_of(brush));
        self.ops.set_stroke_width(width);
        self.ops.stroke_path(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::{CornerRadii, DashPattern, FontHandle, Glyph, SceneBuilder};
    use peniko::color::palette::css::{BLUE, GREEN, RED};
    use peniko::{Blob, FontData};

    /// One recorded [`HybridOps`] call. Records exactly what the routing
    /// assertions below read — the point is which primitive a command lowered
    /// to, not a byte-for-byte replay.
    #[derive(Debug, Clone, PartialEq)]
    enum Op {
        Transform(Affine),
        FillRule(Fill),
        Paint(PaintKind),
        PaintTransform(Affine),
        ResetPaintTransform,
        StrokeWidth(f64),
        FillRect(Rect),
        FillPath(BezPath),
        StrokePath(BezPath),
        BlurredRoundedRect {
            rect: Rect,
            radius: f32,
            std_dev: f32,
        },
        PushClipPath(BezPath),
        PopClipPath,
        PushOpacityLayer {
            clip: BezPath,
            alpha: f32,
        },
        PushBlendLayer(BlendMode),
        PopLayer,
        GlyphRun {
            font_size: f32,
            glyphs: usize,
        },
        ImageSource {
            blob: u64,
            resolved: bool,
        },
    }

    /// The paint kind a lowering set, without the gradient/image payload.
    #[derive(Debug, Clone, PartialEq)]
    enum PaintKind {
        Solid(Color),
        Gradient,
        Image,
    }

    fn paint_kind(paint: &PaintType) -> PaintKind {
        match paint {
            PaintType::Solid(color) => PaintKind::Solid(*color),
            PaintType::Gradient(_) => PaintKind::Gradient,
            PaintType::Image(_) => PaintKind::Image,
        }
    }

    /// A [`HybridOps`] that records into a plain `Vec` instead of touching a
    /// hybrid scene — the GPU-free way to assert *which* primitive each command
    /// lowered to. Drives a real [`ImageResidency`] so image commands exercise
    /// the shim rather than a stub.
    struct RecordingOps {
        ops: Vec<Op>,
        images: ImageResidency,
        uploads: u32,
        next_id: u32,
    }

    impl RecordingOps {
        fn new() -> Self {
            Self::with_budget(UNBOUNDED_ATLAS_PIXELS)
        }

        fn with_budget(pixel_budget: u64) -> Self {
            Self {
                ops: Vec::new(),
                images: ImageResidency::new(TEST_MAX_DIMENSION, pixel_budget),
                uploads: 0,
                next_id: 0,
            }
        }
    }

    impl HybridOps for RecordingOps {
        fn set_transform(&mut self, transform: Affine) {
            self.ops.push(Op::Transform(transform));
        }
        fn set_fill_rule(&mut self, fill_rule: Fill) {
            self.ops.push(Op::FillRule(fill_rule));
        }
        fn set_paint(&mut self, paint: PaintType) {
            self.ops.push(Op::Paint(paint_kind(&paint)));
        }
        fn set_paint_transform(&mut self, transform: Affine) {
            self.ops.push(Op::PaintTransform(transform));
        }
        fn reset_paint_transform(&mut self) {
            self.ops.push(Op::ResetPaintTransform);
        }
        fn set_stroke_width(&mut self, width: f64) {
            self.ops.push(Op::StrokeWidth(width));
        }
        fn fill_rect(&mut self, rect: &Rect) {
            self.ops.push(Op::FillRect(*rect));
        }
        fn fill_path(&mut self, path: &BezPath) {
            self.ops.push(Op::FillPath(path.clone()));
        }
        fn stroke_path(&mut self, path: &BezPath) {
            self.ops.push(Op::StrokePath(path.clone()));
        }
        fn fill_blurred_rounded_rect(&mut self, rect: &Rect, radius: f32, std_dev: f32) {
            self.ops.push(Op::BlurredRoundedRect {
                rect: *rect,
                radius,
                std_dev,
            });
        }
        fn push_clip_path(&mut self, path: &BezPath) {
            self.ops.push(Op::PushClipPath(path.clone()));
        }
        fn pop_clip_path(&mut self) {
            self.ops.push(Op::PopClipPath);
        }
        fn push_opacity_layer(&mut self, clip: &BezPath, alpha: f32) {
            self.ops.push(Op::PushOpacityLayer {
                clip: clip.clone(),
                alpha,
            });
        }
        fn push_blend_layer(&mut self, blend: BlendMode) {
            self.ops.push(Op::PushBlendLayer(blend));
        }
        fn pop_layer(&mut self) {
            self.ops.push(Op::PopLayer);
        }
        fn glyph_run(&mut self, run: &GlyphRun) {
            self.ops.push(Op::GlyphRun {
                font_size: run.font_size,
                glyphs: run.glyphs.len(),
            });
        }
        fn image_source(&mut self, data: &ImageData) -> Option<ImageSource> {
            let Self {
                images,
                uploads,
                next_id,
                ops,
            } = self;
            let source = images.source(data, |_pixmap| {
                *uploads += 1;
                *next_id += 1;
                Some(ImageId::new(*next_id))
            });
            ops.push(Op::ImageSource {
                blob: data.data.id(),
                resolved: source.is_some(),
            });
            source
        }
    }

    /// A host [`HybridResources`] for driving a REAL `vello_hybrid::Scene`
    /// without a device: glyph runs are counted rather than shaped (they need
    /// the renderer's atlas), and images resolve through a real
    /// [`ImageResidency`] with a counting uploader.
    struct HostResources {
        images: ImageResidency,
        uploads: u32,
        next_id: u32,
        glyph_runs: usize,
    }

    impl HostResources {
        fn new() -> Self {
            Self::with_budget(UNBOUNDED_ATLAS_PIXELS)
        }

        fn with_budget(pixel_budget: u64) -> Self {
            Self {
                images: ImageResidency::new(TEST_MAX_DIMENSION, pixel_budget),
                uploads: 0,
                next_id: 0,
                glyph_runs: 0,
            }
        }
    }

    impl HybridResources for HostResources {
        fn fill_glyphs(&mut self, _scene: &mut vello_hybrid::Scene, _run: &GlyphRun) {
            self.glyph_runs += 1;
        }

        fn image_source(&mut self, data: &ImageData) -> Option<ImageSource> {
            let Self {
                images,
                uploads,
                next_id,
                ..
            } = self;
            images.source(data, |_pixmap| {
                *uploads += 1;
                *next_id += 1;
                Some(ImageId::new(*next_id))
            })
        }
    }

    /// The per-image edge clamp the production renderer derives from the
    /// default atlas config on any device that allows the full 4096.
    const TEST_MAX_DIMENSION: u32 = 4096;

    /// A capacity budget no test upload can reach, so the tests that are not
    /// about the capacity guard never trip it.
    const UNBOUNDED_ATLAS_PIXELS: u64 = u64::MAX;

    /// Runs `scene` through the sink over a recording backend and returns the
    /// primitives it lowered to.
    fn record(scene: &Scene) -> Vec<Op> {
        let mut sink = HybridSink {
            ops: RecordingOps::new(),
        };
        encode_into(scene, &mut sink);
        sink.ops.ops
    }

    fn solid_red() -> Brush {
        Brush::Solid(RED)
    }

    /// An `ImageData` of a given natural pixel size, with a fresh blob (and so
    /// a fresh residency key) per call.
    fn image_of_size(w: u32, h: u32) -> ImageData {
        ImageData {
            data: Blob::from(vec![255u8; (w * h * 4) as usize]),
            format: ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: w,
            height: h,
        }
    }

    fn triangle() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((3.0, 3.0));
        path.line_to((10.0, 3.0));
        path.line_to((6.0, 11.0));
        path.close_path();
        path
    }

    fn glyph_run() -> GlyphRun {
        GlyphRun {
            font: FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0)),
            font_size: 16.0,
            brush: solid_red(),
            transform: Affine::translate((3.0, 4.0)),
            glyphs: vec![
                Glyph {
                    id: 1,
                    x: 0.0,
                    y: 0.0,
                },
                Glyph {
                    id: 2,
                    x: 8.0,
                    y: 0.0,
                },
            ],
        }
    }

    #[test]
    fn fill_rect_takes_the_rect_fast_path() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), solid_red());
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::FillRule(Fill::NonZero),
                Op::Paint(PaintKind::Solid(RED)),
                Op::FillRect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            ]
        );
    }

    #[test]
    fn rounded_rect_lowers_to_a_filled_path() {
        let mut scene = Scene::new();
        let rect = Rect::new(2.0, 2.0, 16.0, 16.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rounded_rect_radii(
                rect,
                CornerRadii::new(6.0, 0.0, 3.0, 1.0),
                solid_red(),
            );
        }

        let expected = RoundedRect::from_rect(rect, RoundedRectRadii::new(6.0, 0.0, 3.0, 1.0))
            .to_path(FLATTEN_TOLERANCE);
        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::FillRule(Fill::NonZero),
                Op::Paint(PaintKind::Solid(RED)),
                Op::FillPath(expected),
            ]
        );
    }

    #[test]
    fn stroke_line_sets_the_stroke_width_then_strokes_a_path() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.stroke_line(
                Point::new(0.0, 0.0),
                Point::new(20.0, 20.0),
                1.5,
                Brush::Solid(BLUE),
            );
        }

        let expected =
            Line::new(Point::new(0.0, 0.0), Point::new(20.0, 20.0)).to_path(FLATTEN_TOLERANCE);
        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::Paint(PaintKind::Solid(BLUE)),
                Op::StrokeWidth(1.5),
                Op::StrokePath(expected),
            ]
        );
    }

    #[test]
    fn clips_use_the_non_isolated_clip_stack() {
        // A clip must NOT become a clip layer: that would allocate an
        // intermediate texture for every scroll viewport in the tree.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 10.0, 20.0));
            builder.pop_clip();
        }

        let ops = record(&scene);
        assert_eq!(
            ops,
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::PushClipPath(Rect::new(0.0, 0.0, 10.0, 20.0).to_path(FLATTEN_TOLERANCE)),
                Op::PopClipPath,
            ]
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::PushOpacityLayer { .. })),
            "a rectangular clip must not open an isolated layer"
        );
    }

    #[test]
    fn rounded_clip_carries_its_corners_into_the_clip_path() {
        let mut scene = Scene::new();
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let radii = CornerRadii::new(6.0, 0.0, 3.0, 1.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip_rounded_radii(rect, radii);
            builder.pop_clip();
        }

        let expected = RoundedRect::from_rect(rect, RoundedRectRadii::new(6.0, 0.0, 3.0, 1.0))
            .to_path(FLATTEN_TOLERANCE);
        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::PushClipPath(expected),
                Op::PopClipPath,
            ]
        );
    }

    #[test]
    fn opacity_layer_carries_the_layer_rect_as_its_clip() {
        // The vello path's `push_layer` clips to the layer rect as well as
        // fading it; dropping the clip here would make the two tiers disagree.
        let mut scene = Scene::new();
        let rect = Rect::new(2.0, 2.0, 12.0, 12.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_layer(rect, 0.5);
            builder.pop_layer();
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::PushOpacityLayer {
                    clip: rect.to_path(FLATTEN_TOLERANCE),
                    alpha: 0.5,
                },
                Op::PopLayer,
            ]
        );
    }

    #[test]
    fn clear_rect_punches_through_a_destination_out_blend_layer() {
        let mut scene = Scene::new();
        let rect = Rect::new(1.0, 1.0, 9.0, 9.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.clear_rect(rect);
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::PushBlendLayer(BlendMode::new(
                    peniko::Mix::Normal,
                    peniko::Compose::DestOut
                )),
                Op::FillRule(Fill::NonZero),
                Op::Paint(PaintKind::Solid(Color::BLACK)),
                Op::FillRect(rect),
                Op::PopLayer,
            ]
        );
    }

    #[test]
    fn blurred_shadow_collapses_per_corner_radii_to_one_radius() {
        let mut scene = Scene::new();
        let rect = Rect::new(1.0, 1.0, 18.0, 18.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_blurred_rounded_rect_radii(
                rect,
                CornerRadii::new(6.0, 0.0, 3.0, 1.0),
                2.0,
                Color::BLACK,
            );
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::IDENTITY),
                Op::Paint(PaintKind::Solid(Color::BLACK)),
                Op::BlurredRoundedRect {
                    rect,
                    radius: 6.0,
                    std_dev: 2.0,
                },
            ]
        );
    }

    #[test]
    fn dashed_stroke_arrives_pre_expanded_from_the_shared_walk() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            let mut path = BezPath::new();
            path.move_to((0.0, 10.0));
            path.line_to((20.0, 10.0));
            builder.stroke_path_dashed(
                path,
                1.0,
                DashPattern::new(3.0, 2.0).with_phase(1.0),
                Brush::Solid(BLUE),
            );
        }

        let ops = record(&scene);
        let Some(Op::StrokePath(dashed)) = ops.iter().find(|op| matches!(op, Op::StrokePath(_)))
        else {
            panic!("expected a stroked path, got {ops:?}");
        };
        assert!(
            dashed.elements().len() > 2,
            "the dash pattern should have expanded into several sub-paths, got {dashed:?}"
        );
    }

    #[test]
    fn glyph_run_sets_transform_and_paint_before_the_run() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_glyph_run(glyph_run());
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::Transform(Affine::translate((3.0, 4.0))),
                Op::Paint(PaintKind::Solid(RED)),
                Op::GlyphRun {
                    font_size: 16.0,
                    glyphs: 2,
                },
            ]
        );
    }

    #[test]
    fn image_paints_through_the_residency_shim() {
        let mut scene = Scene::new();
        let dest = Rect::new(0.0, 0.0, 8.0, 4.0);
        let data = image_of_size(2, 2);
        let blob = data.data.id();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(&data, dest);
        }

        assert_eq!(
            record(&scene),
            vec![
                Op::ImageSource {
                    blob,
                    resolved: true,
                },
                Op::Transform(Affine::IDENTITY),
                Op::FillRule(Fill::NonZero),
                Op::Paint(PaintKind::Image),
                Op::PaintTransform(Affine::scale_non_uniform(4.0, 2.0)),
                Op::FillRect(dest),
                Op::ResetPaintTransform,
            ]
        );
    }

    #[test]
    fn an_image_brush_handed_to_a_fill_falls_back_to_transparent() {
        // The documented downgrade: no shipping widget emits one, and the
        // dedicated image command is the supported route.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(
                Rect::new(0.0, 0.0, 4.0, 4.0),
                Brush::Image(peniko::ImageBrush {
                    image: image_of_size(2, 2),
                    sampler: peniko::ImageSampler::default(),
                }),
            );
        }

        assert!(
            record(&scene).contains(&Op::Paint(PaintKind::Solid(FALLBACK_PAINT))),
            "an image brush should paint transparent"
        );
    }

    #[test]
    fn shader_quad_lowers_to_its_placeholder_fill() {
        let mut scene = Scene::new();
        let dest = Rect::new(0.0, 0.0, 6.0, 6.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_shader(&frust_scene::ShaderProgram::new("fn main() {}"), dest, 0.0);
        }

        // No shader pre-pass runs for this tier, so the quad takes the shared
        // walk's miss path: an opaque placeholder rect.
        let ops = record(&scene);
        assert!(
            ops.contains(&Op::FillRect(dest)),
            "expected a placeholder fill, got {ops:?}"
        );
    }

    #[test]
    fn snapshot_bracket_emulates_its_alpha_layer_inline() {
        // No snapshot cache backs this tier, so a bracket takes the shared
        // walk's inline-emulation path: an opacity layer around its body.
        let mut scene = Scene::new();
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(7, rect, 0.5, 1.0);
            builder.fill_rect(rect, solid_red());
            builder.pop_snapshot();
        }

        let ops = record(&scene);
        assert_eq!(
            &ops[..2],
            [
                Op::Transform(Affine::IDENTITY),
                Op::PushOpacityLayer {
                    clip: rect.to_path(FLATTEN_TOLERANCE),
                    alpha: 0.5,
                },
            ]
        );
        assert_eq!(ops.last(), Some(&Op::PopLayer));
    }

    #[test]
    fn every_command_variant_routes_to_a_hybrid_primitive() {
        // The whole `SceneSink` surface in one scene, mirroring the CPU tier's
        // equivalent: every variant must reach a primitive, not silently drop.
        let mut scene = Scene::new();
        let data = image_of_size(2, 2);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), solid_red());
            builder.fill_rounded_rect(Rect::new(2.0, 2.0, 16.0, 16.0), 4.0, solid_red());
            builder.stroke_line(
                Point::new(0.0, 0.0),
                Point::new(20.0, 20.0),
                1.5,
                Brush::Solid(BLUE),
            );
            builder.draw_glyph_run(glyph_run());
            builder.draw_image(&data, Rect::new(0.0, 0.0, 8.0, 8.0));
            builder.draw_blurred_rounded_rect(
                Rect::new(1.0, 1.0, 18.0, 18.0),
                3.0,
                2.0,
                Color::BLACK,
            );
            builder.push_clip(Rect::new(0.0, 0.0, 15.0, 15.0));
            builder.push_clip_rounded(Rect::new(0.0, 0.0, 14.0, 14.0), 2.0);
            builder.push_layer(Rect::new(2.0, 2.0, 12.0, 12.0), 0.5);
            builder.fill_path(triangle(), Brush::Solid(GREEN));
            builder.stroke_path(triangle(), 1.0, solid_red());
            builder.pop_layer();
            builder.pop_clip();
            builder.pop_clip();
            builder.clear_rect(Rect::new(1.0, 1.0, 5.0, 5.0));
        }

        let ops = record(&scene);
        let has = |pred: fn(&Op) -> bool| ops.iter().any(pred);
        assert!(has(|op| matches!(op, Op::FillRect(_))), "fill rect");
        assert!(has(|op| matches!(op, Op::FillPath(_))), "fill path");
        assert!(has(|op| matches!(op, Op::StrokePath(_))), "stroke path");
        assert!(has(|op| matches!(op, Op::StrokeWidth(_))), "stroke width");
        assert!(
            has(|op| matches!(op, Op::BlurredRoundedRect { .. })),
            "blurred rounded rect"
        );
        assert!(has(|op| matches!(op, Op::PushClipPath(_))), "clip push");
        assert!(has(|op| matches!(op, Op::PopClipPath)), "clip pop");
        assert!(
            has(|op| matches!(op, Op::PushOpacityLayer { .. })),
            "opacity layer"
        );
        assert!(
            has(|op| matches!(op, Op::PushBlendLayer(_))),
            "hole-punch blend layer"
        );
        assert!(has(|op| matches!(op, Op::PopLayer)), "layer pop");
        assert!(has(|op| matches!(op, Op::GlyphRun { .. })), "glyph run");
        assert!(
            has(|op| matches!(op, Op::ImageSource { resolved: true, .. })),
            "image residency"
        );
        assert!(
            has(|op| matches!(op, Op::Paint(PaintKind::Image))),
            "image paint"
        );
    }

    #[test]
    fn one_image_uploads_once_across_sixty_frames() {
        // The residency contract: a widget redrawing its cached image every
        // frame must pay the atlas upload exactly once.
        let data = image_of_size(4, 4);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(&data, Rect::new(0.0, 0.0, 4.0, 4.0));
        }

        let mut sink = HybridSink {
            ops: RecordingOps::new(),
        };
        for _ in 0..60 {
            sink.ops.images.begin_frame();
            encode_into(&scene, &mut sink);
        }

        assert_eq!(sink.ops.uploads, 1, "the image should upload exactly once");
        assert_eq!(sink.ops.images.len(), 1);
    }

    #[test]
    fn two_distinct_images_upload_once_each() {
        let first = image_of_size(4, 4);
        let second = image_of_size(4, 4);
        assert_ne!(first.data.id(), second.data.id());

        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(&first, Rect::new(0.0, 0.0, 4.0, 4.0));
            builder.draw_image(&second, Rect::new(4.0, 0.0, 8.0, 4.0));
        }

        let mut sink = HybridSink {
            ops: RecordingOps::new(),
        };
        for _ in 0..10 {
            sink.ops.images.begin_frame();
            encode_into(&scene, &mut sink);
        }

        assert_eq!(sink.ops.uploads, 2);
        assert_eq!(sink.ops.images.len(), 2);
    }

    #[test]
    fn an_image_unseen_for_sixty_frames_is_evicted() {
        let data = image_of_size(4, 4);
        let mut residency = ImageResidency::new(TEST_MAX_DIMENSION, UNBOUNDED_ATLAS_PIXELS);
        residency.begin_frame();
        let mut uploads = 0;
        assert!(
            residency
                .source(&data, |_| {
                    uploads += 1;
                    Some(ImageId::new(1))
                })
                .is_some()
        );

        // Just short of the horizon: still resident, nothing destroyed.
        for _ in 0..(IMAGE_EVICTION_FRAMES - 1) {
            residency.begin_frame();
        }
        let mut destroyed = Vec::new();
        residency.evict_stale(|id| destroyed.push(id));
        assert!(destroyed.is_empty(), "evicted too early");
        assert_eq!(residency.len(), 1);

        // One frame further and the handle is released.
        residency.begin_frame();
        residency.evict_stale(|id| destroyed.push(id));
        assert_eq!(destroyed, vec![ImageId::new(1)]);
        assert_eq!(residency.len(), 0);
        assert_eq!(uploads, 1);
    }

    #[test]
    fn a_redrawn_image_is_never_evicted() {
        let data = image_of_size(4, 4);
        let mut residency = ImageResidency::new(TEST_MAX_DIMENSION, UNBOUNDED_ATLAS_PIXELS);
        let mut uploads = 0;
        for _ in 0..(IMAGE_EVICTION_FRAMES * 3) {
            residency.begin_frame();
            residency.source(&data, |_| {
                uploads += 1;
                Some(ImageId::new(1))
            });
            let mut destroyed = Vec::new();
            residency.evict_stale(|id| destroyed.push(id));
            assert!(destroyed.is_empty(), "a drawn image must stay resident");
        }
        assert_eq!(uploads, 1);
    }

    #[test]
    fn an_unuploadable_image_is_refused_rather_than_panicking() {
        let mut residency = ImageResidency::new(64, UNBOUNDED_ATLAS_PIXELS);
        let mut uploads = 0;
        let mut try_upload = |data: &ImageData, residency: &mut ImageResidency| {
            residency.source(data, |_| {
                uploads += 1;
                Some(ImageId::new(1))
            })
        };

        // Degenerate size.
        assert!(try_upload(&image_of_size(0, 0), &mut residency).is_none());
        // Larger than the atlas can hold.
        assert!(try_upload(&image_of_size(128, 8), &mut residency).is_none());
        // A pixel buffer that disagrees with the declared size.
        let mut short = image_of_size(4, 4);
        short.data = Blob::from(vec![0u8; 8]);
        assert!(try_upload(&short, &mut residency).is_none());
        // A refused upload.
        let data = image_of_size(4, 4);
        assert!(residency.source(&data, |_| None).is_none());

        assert_eq!(uploads, 0);
        assert_eq!(residency.len(), 0);
    }

    #[test]
    fn a_refused_image_leaves_no_paint_state_behind() {
        let mut scene = Scene::new();
        let data = image_of_size(0, 0);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(&data, Rect::new(0.0, 0.0, 4.0, 4.0));
        }

        assert!(
            record(&scene).is_empty(),
            "a degenerate image must not touch the scene"
        );
    }

    /// The atlas budget derivation, against the two device limits
    /// `vello_hybrid`'s own `MemorySettings::normalize` clamps the config with.
    #[test]
    fn the_atlas_budget_mirrors_the_config_the_device_will_allow() {
        let config = AtlasConfig::default();
        // The numbers the module docs quote, pinned against the dependency.
        assert_eq!(config.max_atlases, 8);
        assert_eq!(config.atlas_size, (4096, 4096));

        // A device that allows the whole configured atlas array.
        assert_eq!(
            atlas_pixel_budget_from(config, 4096, 256),
            8 * 4096 * 4096 / 2
        );
        // One that clamps the edge and the layer count both.
        assert_eq!(
            atlas_pixel_budget_from(config, 2048, 4),
            4 * 2048 * 2048 / 2
        );
        // A roomier device does not grow the atlas past its own config.
        assert_eq!(
            atlas_pixel_budget_from(config, 16384, 2048),
            8 * 4096 * 4096 / 2
        );
        // No array layers at all: nothing to allocate from, nothing admitted.
        assert_eq!(atlas_pixel_budget_from(config, 4096, 0), 0);
    }

    /// Past the budget the draw is skipped and warned about, exactly as an
    /// oversized image is — the arm that was unreachable while
    /// `Renderer::upload_image` was the only thing that could refuse, and
    /// `upload_image` cannot refuse, it aborts.
    #[test]
    fn an_exhausted_atlas_budget_skips_the_draw_instead_of_uploading() {
        const EDGE: u32 = 8;
        // Room for exactly two of them.
        const BUDGET: u64 = (EDGE * EDGE * 2) as u64;

        let images: Vec<ImageData> = (0..3).map(|_| image_of_size(EDGE, EDGE)).collect();
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            for image in &images {
                builder.draw_image(image, Rect::new(0.0, 0.0, 8.0, 8.0));
            }
        }

        let mut sink = HybridSink {
            ops: RecordingOps::with_budget(BUDGET),
        };
        encode_into(&scene, &mut sink);
        let ops = sink.ops.ops;

        let resolved = ops
            .iter()
            .filter(|op| matches!(op, Op::ImageSource { resolved: true, .. }))
            .count();
        let refused = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    Op::ImageSource {
                        resolved: false,
                        ..
                    }
                )
            })
            .count();
        assert_eq!((resolved, refused), (2, 1));
        // Two draws, not three: the refused image paints nothing at all, and
        // leaves the scene where the second one left it.
        assert_eq!(
            ops.iter()
                .filter(|op| matches!(op, Op::FillRect(_)))
                .count(),
            2
        );
        assert!(matches!(
            ops.last(),
            Some(Op::ImageSource {
                resolved: false,
                ..
            })
        ));
        assert_eq!(sink.ops.uploads, 2);
        assert!(
            IMAGE_ATLAS_FULL.get().is_some(),
            "a budget refusal must warn (once per process)"
        );
    }

    /// The same exhaustion driven through the production `SceneOps` over a real
    /// hybrid scene, across the eviction horizon: distinct images inside one
    /// window fill the budget and are skipped, then eviction hands the pixels
    /// back and the next image uploads again.
    #[test]
    fn eviction_returns_atlas_budget_and_re_admits_uploads() {
        const EDGE: u32 = 8;
        const BUDGET: u64 = (EDGE * EDGE * 2) as u64;

        let images: Vec<ImageData> = (0..4).map(|_| image_of_size(EDGE, EDGE)).collect();
        let mut crowded = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut crowded);
            for image in &images {
                builder.draw_image(image, Rect::new(0.0, 0.0, 8.0, 8.0));
            }
        }

        let mut hybrid = vello_hybrid::Scene::new(16, 16);
        let mut resources = HostResources::with_budget(BUDGET);

        // All four inside one frame, so all four are inside the eviction
        // window: two are admitted, two are turned away, and the real scene
        // takes the result without a panic.
        hybrid.reset();
        resources.images.begin_frame();
        {
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: &mut hybrid,
                    resources: &mut resources,
                },
            };
            encode_into(&crowded, &mut sink);
        }
        assert_eq!(resources.uploads, 2);
        assert_eq!(resources.images.len(), 2);
        assert_eq!(resources.images.live_pixels, BUDGET);

        // Redrawing the same crowded scene inside the window changes nothing:
        // the two residents are reused, the two refusals repeat.
        for _ in 0..(IMAGE_EVICTION_FRAMES - 1) {
            hybrid.reset();
            resources.images.begin_frame();
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: &mut hybrid,
                    resources: &mut resources,
                },
            };
            encode_into(&crowded, &mut sink);
        }
        assert_eq!(resources.uploads, 2);

        // Now stop drawing them and cross the horizon.
        for _ in 0..IMAGE_EVICTION_FRAMES {
            resources.images.begin_frame();
        }
        let mut destroyed = Vec::new();
        resources.images.evict_stale(|id| destroyed.push(id));
        assert_eq!(destroyed.len(), 2);
        assert_eq!(resources.images.len(), 0);
        assert_eq!(resources.images.live_pixels, 0);

        // The reclaimed pixels are spendable again.
        let fresh = image_of_size(EDGE, EDGE);
        let mut later = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut later);
            builder.draw_image(&fresh, Rect::new(0.0, 0.0, 8.0, 8.0));
        }
        hybrid.reset();
        resources.images.begin_frame();
        {
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: &mut hybrid,
                    resources: &mut resources,
                },
            };
            encode_into(&later, &mut sink);
        }
        assert_eq!(resources.uploads, 3);
        assert_eq!(resources.images.len(), 1);
    }

    #[test]
    fn every_command_variant_drives_a_real_hybrid_scene() {
        // The recording tests above prove WHICH primitive each command reaches;
        // this one proves the real `vello_hybrid::Scene` accepts the resulting
        // sequence — its strip generation, paint encoding and layer stack all
        // run here, on the host, with no device. Glyph runs are excluded: they
        // need the renderer's glyph atlas, which only exists with a device.
        let mut scene = Scene::new();
        let data = image_of_size(2, 2);
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), solid_red());
            builder.fill_rounded_rect(Rect::new(2.0, 2.0, 16.0, 16.0), 4.0, solid_red());
            builder.stroke_line(
                Point::new(0.0, 0.0),
                Point::new(20.0, 20.0),
                1.5,
                Brush::Solid(BLUE),
            );
            builder.draw_image(&data, Rect::new(0.0, 0.0, 8.0, 8.0));
            builder.draw_blurred_rounded_rect(
                Rect::new(1.0, 1.0, 18.0, 18.0),
                3.0,
                2.0,
                Color::BLACK,
            );
            builder.push_clip(Rect::new(0.0, 0.0, 15.0, 15.0));
            builder.push_layer(Rect::new(2.0, 2.0, 12.0, 12.0), 0.5);
            builder.fill_path(triangle(), Brush::Solid(GREEN));
            builder.stroke_path(triangle(), 1.0, solid_red());
            builder.pop_layer();
            builder.pop_clip();
            builder.clear_rect(Rect::new(1.0, 1.0, 5.0, 5.0));
        }

        let mut hybrid = vello_hybrid::Scene::new(20, 20);
        let mut resources = HostResources::new();
        {
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: &mut hybrid,
                    resources: &mut resources,
                },
            };
            encode_into(&scene, &mut sink);
        }

        assert_eq!(resources.uploads, 1);
        assert_eq!((hybrid.width(), hybrid.height()), (20, 20));
    }

    #[test]
    fn a_real_hybrid_scene_uploads_one_image_once_across_sixty_frames() {
        // The residency contract again, this time with the production
        // `SceneOps` driving a real hybrid scene end to end.
        let data = image_of_size(4, 4);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(&data, Rect::new(0.0, 0.0, 4.0, 4.0));
        }

        let mut hybrid = vello_hybrid::Scene::new(8, 8);
        let mut resources = HostResources::new();
        for _ in 0..60 {
            hybrid.reset();
            resources.images.begin_frame();
            let mut sink = HybridSink {
                ops: SceneOps {
                    scene: &mut hybrid,
                    resources: &mut resources,
                },
            };
            encode_into(&scene, &mut sink);
        }

        assert_eq!(resources.uploads, 1);
        assert_eq!(resources.glyph_runs, 0);
    }

    #[test]
    fn zero_dimensions_are_clamped_to_one() {
        assert_eq!(clamp_dims(0, 0), (1, 1));
    }

    #[test]
    fn oversized_dimensions_are_clamped_to_u16_max() {
        assert_eq!(clamp_dims(100_000, 70_000), (u16::MAX, u16::MAX));
    }

    /// The GPU leg of the tier (the counterpart of the CPU tier's upload
    /// smoke test): build a real `HybridTierRenderer`, render a scene into an
    /// `Rgba8Unorm` texture through it, and read the result back. Needs a real
    /// device, so it is `#[ignore]`d and run manually on hardware.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render --features hybrid-tier -- --ignored`"]
    fn hybrid_tier_renders_a_scene_on_a_real_device() {
        pollster::block_on(async {
            // 64 px wide is 256 bytes per row — wgpu's
            // `COPY_BYTES_PER_ROW_ALIGNMENT` for the readback copy below.
            const SIZE: u32 = 64;
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust hybrid_tier smoke"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust hybrid_tier target"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            let mut scene = Scene::new();
            {
                let mut builder = SceneBuilder::new(&mut scene);
                builder.fill_rect(
                    Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
                    solid_red(),
                );
            }

            let mut renderer =
                HybridTierRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, SIZE, SIZE);
            assert_eq!((renderer.width(), renderer.height()), (64, 64));

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            renderer
                .render(&device, &queue, &mut encoder, &scene, Color::WHITE, &view)
                .expect("hybrid render failed");

            let bytes_per_row = SIZE * 4;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust hybrid_tier readback"),
                size: u64::from(bytes_per_row * SIZE),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(SIZE),
                    },
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);

            let slice = buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll failed");
            rx.recv()
                .expect("map channel closed")
                .expect("buffer map failed");

            let mapped = slice.get_mapped_range();
            let centre = ((SIZE / 2) * bytes_per_row + (SIZE / 2) * 4) as usize;
            let (r, g, b) = (mapped[centre], mapped[centre + 1], mapped[centre + 2]);
            assert!(
                r > g && r > b,
                "centre pixel should be red-dominant, got rgb=({r},{g},{b})"
            );
        });
    }

    /// A `log`ger that keeps every record's rendered message, so a test can
    /// assert on what the frame path actually emitted rather than on a
    /// re-derivation of it. Installed process-wide (the `log` crate allows
    /// exactly one), which is why the assertion below tolerates a losing race
    /// with an already-installed logger.
    #[cfg(feature = "perf-trace")]
    struct CapturingLogger(std::sync::Mutex<Vec<String>>);

    #[cfg(feature = "perf-trace")]
    static CAPTURED: CapturingLogger = CapturingLogger(std::sync::Mutex::new(Vec::new()));

    #[cfg(feature = "perf-trace")]
    impl log::Log for CapturingLogger {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            if let Ok(mut lines) = self.0.lock() {
                lines.push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }

    /// The per-frame span line is really emitted by a real render, in the
    /// documented `frust-perf hybrid strip_us=<n> record_us=<n>` shape — the
    /// contract a capture reader parses. Needs both halves of the gate: the
    /// `perf-trace` feature (compile time) and `FRUST_TRACE` (this process's
    /// environment), which is why the run command sets it rather than the test
    /// mutating a process-global env var mid-suite.
    #[cfg(feature = "perf-trace")]
    #[test]
    #[ignore = "requires a GPU and FRUST_TRACE; run locally with `FRUST_TRACE=1 cargo test -p frust-render --features hybrid-tier,perf-trace -- --ignored`"]
    fn hybrid_render_emits_one_strip_us_span_line_per_frame() {
        assert!(
            frame_trace_enabled(),
            "run this test with FRUST_TRACE=1 in the environment"
        );
        let _ = log::set_logger(&CAPTURED);
        log::set_max_level(log::LevelFilter::Info);

        pollster::block_on(async {
            const SIZE: u32 = 64;
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust hybrid_tier span smoke"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust hybrid_tier span target"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            let mut scene = Scene::new();
            {
                let mut builder = SceneBuilder::new(&mut scene);
                builder.fill_rect(
                    Rect::new(0.0, 0.0, f64::from(SIZE), f64::from(SIZE)),
                    solid_red(),
                );
            }

            let mut renderer =
                HybridTierRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, SIZE, SIZE);
            // Three frames, three lines: the line is per-frame, not per-process
            // like the tier/atlas-cache startup lines.
            for _ in 0..3 {
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                renderer
                    .render(&device, &queue, &mut encoder, &scene, Color::WHITE, &view)
                    .expect("hybrid render failed");
                queue.submit([encoder.finish()]);
            }

            let lines = CAPTURED.0.lock().expect("capture lock").clone();
            let spans: Vec<&String> = lines
                .iter()
                .filter(|line| line.starts_with("frust-perf hybrid strip_us="))
                .collect();
            assert_eq!(spans.len(), 3, "one line per frame, got {lines:?}");
            for span in spans {
                let rest = span
                    .strip_prefix("frust-perf hybrid strip_us=")
                    .expect("filtered on this prefix");
                let (strip, record) = rest
                    .split_once(" record_us=")
                    .unwrap_or_else(|| panic!("missing record_us field: {span:?}"));
                strip
                    .parse::<u128>()
                    .unwrap_or_else(|_| panic!("strip_us not an integer: {span:?}"));
                record
                    .parse::<u128>()
                    .unwrap_or_else(|_| panic!("record_us not an integer: {span:?}"));
            }
        });
    }

    /// `FRUST_HYBRID_ATLAS_CACHE` is a flag, not a mode list: only `1`/`true`
    /// (case- and whitespace-insensitive) turn the experimental cache on, and
    /// every other value — including an unrecognised one — leaves glifo's own
    /// default off, since a typo must never silently enable an
    /// upstream-experimental path.
    #[test]
    fn atlas_cache_knob_accepts_only_one_and_true() {
        for raw in ["1", "true", "TRUE", " True ", "\ttrue\n"] {
            assert!(parse_atlas_cache(Some(raw.to_string())), "raw {raw:?}");
        }
        for raw in ["0", "false", "on", "yes", "", "  ", "hybrid"] {
            assert!(!parse_atlas_cache(Some(raw.to_string())), "raw {raw:?}");
        }
        assert!(!parse_atlas_cache(None), "unset must stay off");
    }
}
