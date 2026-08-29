//! The deterministic, GPU-free reference renderer every golden case is
//! compared against: [`CpuOracle`], a [`SceneRenderer`] backed by `vello_cpu`
//! 0.2.0.
//!
//! # Why *this* CPU renderer
//!
//! The oracle deliberately rasterizes with the ENGINE'S OWN core — `vello_cpu`
//! 0.2.0, which is built on the same `vello_common` 0.2.0 + `glifo` 0.3.0 the
//! engine tier uses — rather than with the `vello_cpu = "=0.0.9"` pin
//! `frust-render`'s legacy `cpu-tier` fallback carries. A golden diff is then a
//! statement about *Frust's* lowering of a [`frust_scene::Scene`], not about
//! the difference between two unrelated rasterizer generations.
//!
//! Both versions therefore live in the graph at once, which means two
//! `vello_common` identities (0.0.9 and 0.2.0) and two `glifo` identities
//! (0.1.1 and 0.3.0). That duplication is deliberate and *guarded*, not
//! accidental drift: see `tests/dup_identities.rs`, which fails if the count of
//! either ever changes. The dependency is renamed (`vello_cpu_oracle`) at the
//! workspace level so every use site says which identity it means.
//!
//! # Relationship to `frust-render`'s command walk
//!
//! The command semantics below mirror `frust-render`'s `convert.rs` walk
//! (its `SceneSink` seam) — the hoisted `ClearRect`, the per-corner rounded
//! shapes, the pre-flattened dash segments, the largest-corner shadow
//! downgrade, and the inline snapshot emulation are all reproduced here. They
//! are *reproduced* rather than reused because that seam is `pub(crate)` to
//! `frust-render` and shaped around `vello` types, and because this crate must
//! stay a GPU-free leaf (no `frust-render`, no `vello`, no `wgpu` edge).
//!
//! That duplication is the point of an oracle — an independent second
//! implementation of the same contract — but it also means a change to
//! `convert.rs`'s semantics must be mirrored here, or a golden diff will blame
//! the engine for a divergence that lives in this file. The
//! [`SkipReport`]-carrying variants below are the only intentional
//! divergences.
//!
//! # Known fidelity gaps (reported, never silent)
//!
//! Each of these is *counted* into [`SkipReport`] so a corpus can route the
//! affected case into a lower golden class instead of silently comparing
//! against a lie:
//!
//! - [`frust_scene::Command::ShaderQuad`] has no CPU equivalent — there is no
//!   shader pre-pass without a GPU. It lowers to the same opaque dark
//!   placeholder fill `convert.rs` uses on a shader miss, and increments
//!   [`SkipReport::shader_quads`].
//! - An image *brush* handed to a fill (as opposed to the dedicated
//!   [`frust_scene::Command::Image`]) paints transparent, matching the CPU
//!   tier's own downgrade, and increments [`SkipReport::image_brush_fills`].
//! - A [`frust_scene::Command::GlyphRun`] whose font blob does not begin with a
//!   recognizable sfnt tag is skipped and counted in
//!   [`SkipReport::unrenderable_glyph_runs`] — `glifo` unwraps its font parse,
//!   so an unparseable blob would otherwise panic the oracle. The sniff is
//!   deliberately shallow (see [`looks_like_sfnt`]); a structurally valid but
//!   internally corrupt font is still `glifo`'s to reject.
//!
//! # Determinism
//!
//! The deterministic-input contract on [`SceneRenderer::render`] binds the
//! backend too, not just the caller: the render context is built with a pinned
//! [`RenderSettings`](vello_cpu_oracle::RenderSettings) — `Level::baseline()`
//! (the SIMD level the target guarantees statically, rather than
//! `Level::try_detect()`'s host-CPU-dependent answer) and zero worker threads —
//! so two machines of the same target produce the same bytes. The pinned level
//! is recorded in [`BackendMeta::driver`] so a promoted baseline carries it.
//!
//! # Alpha
//!
//! `vello_cpu` pixmaps are PREMULTIPLIED (`PixelFormat::Rgba8` is documented as
//! premultiplied RGBA8), and [`RenderedImage::rgba8`] is handed back in exactly
//! that form, tagged [`AlphaKind::Premultiplied`]. The tag is set from that
//! fact rather than assumed: un-premultiplying would be a lossy round trip
//! (color channels of a low-alpha pixel carry almost no information once
//! divided out), which is the wrong default for a comparator. A consumer that
//! genuinely wants straight alpha should divide it out itself, or take
//! `Pixmap::take_unpremultiplied`'s route.

use frust_scene::{Command, GlyphRun, PathStyle, Scene};
use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Stroke};
use peniko::{BlendMode, Brush, Color, Compose, Fill, ImageData, Mix};
use vello_cpu_oracle::{
    Image, ImageSource, Level, PaintType, Pixmap, RenderContext, RenderSettings, Resources,
};

use crate::render::{AlphaKind, BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// The oracle's stable backend id ([`SceneRenderer::id`]), matched against a
/// [`crate::case::BackendSet`] and recorded into every
/// [`crate::meta::GoldenMeta`] it produces.
pub const ORACLE_ID: &str = "vello-cpu-0.2";

/// Flattening tolerance (logical px) for the shapes handed to `vello_cpu` as
/// `BezPath`s — its clip-layer and rounded-fill entry points take a path, not a
/// `kurbo` shape. Well below a pixel, so corners stay crisp without exploding
/// the segment count. Matches `frust-render`'s CPU tier so the two rasterize
/// identical geometry.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// The transparent paint used where `vello_cpu` has no equivalent for a scene
/// brush (an image brush handed to a fill; see the module docs' gap list).
const FALLBACK_PAINT: Color = Color::new([0.0, 0.0, 0.0, 0.0]);

/// The opaque dark placeholder a [`Command::ShaderQuad`] lowers to — the exact
/// color `frust-render`'s `convert.rs` fills on a shader miss, so the oracle
/// and the engine's own miss path agree pixel-for-pixel.
const SHADER_PLACEHOLDER: Color = Color::from_rgba8(16, 16, 16, 255);

/// Fidelity gaps encountered during the most recent [`CpuOracle::render`].
///
/// A non-empty report means the frame is not a faithful reference for at least
/// one command: a corpus should route such a case into a looser golden class
/// (or skip it) rather than promote its bytes as truth. Reset at the start of
/// every render, so it always describes the frame just produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkipReport {
    /// [`Command::ShaderQuad`]s lowered to the placeholder fill — there is no
    /// shader pre-pass without a GPU.
    pub shader_quads: usize,
    /// Fills whose brush was a [`Brush::Image`], painted transparent.
    pub image_brush_fills: usize,
    /// [`Command::GlyphRun`]s skipped because the run was empty or its font
    /// blob is not recognizably an sfnt font (see [`looks_like_sfnt`]).
    pub unrenderable_glyph_runs: usize,
    /// [`Command::Image`]s skipped because the decoded image had a zero
    /// dimension, or one larger than `u16::MAX` (which `vello_cpu`'s
    /// `ImageSource::from_peniko_image_data` panics on).
    pub unrenderable_images: usize,
}

impl SkipReport {
    /// Whether the frame was rendered with no fidelity gap at all — the
    /// precondition for treating its bytes as a pixel-exact reference.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether `data` plausibly begins with an sfnt font header.
///
/// `glifo` resolves a [`peniko::FontData`] with an `unwrap`, so an unparseable
/// blob panics rather than degrading — unacceptable in a test oracle that a
/// fuzz or corpus card points at arbitrary scenes. This is a SNIFF, not
/// validation: it rejects the empty and obviously-not-a-font blobs (the shape
/// every "no real font here" test fixture in this repo uses) by checking the
/// four-byte tag only. A well-formed header wrapping corrupt tables still
/// reaches `glifo`.
pub fn looks_like_sfnt(data: &[u8]) -> bool {
    matches!(
        data.first_chunk::<4>(),
        // TrueType outlines (0x00010000), Apple's `true`, CFF outlines
        // (`OTTO`), and a TrueType collection (`ttcf`).
        Some(&[0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO" | b"ttcf")
    )
}

/// A `vello_cpu` 0.2.0-backed [`SceneRenderer`]: the deterministic reference
/// every golden case is diffed against.
///
/// Owns its render context, its [`Resources`], and the target [`Pixmap`], all
/// reused across renders (resized in place when a [`RenderSpec`] asks for a
/// different size), so a corpus of hundreds of cases does not reallocate the
/// whole rasterizer per case.
pub struct CpuOracle {
    ctx: RenderContext,
    resources: Resources,
    pixmap: Pixmap,
    width: u16,
    height: u16,
    /// Captured once at construction — [`SceneRenderer::meta`]'s contract is
    /// static backend identity, not per-render state.
    meta: BackendMeta,
    /// Fidelity gaps from the most recent render (see [`SkipReport`]).
    skips: SkipReport,
    /// How many commands the most recent render's walk dispatched — the
    /// coverage assertion the variant-routing test makes.
    routed: usize,
}

impl CpuOracle {
    /// Creates an oracle with a placeholder 1x1 target; every
    /// [`SceneRenderer::render`] resizes it to its [`RenderSpec`] first.
    pub fn new() -> Self {
        let settings = pinned_settings();
        Self {
            ctx: RenderContext::new_with(1, 1, settings),
            resources: Resources::new(),
            pixmap: Pixmap::new(1, 1),
            width: 1,
            height: 1,
            meta: BackendMeta {
                backend: ORACLE_ID.to_string(),
                adapter: "vello_cpu 0.2.0 (vello_common 0.2.0, glifo 0.3.0)".to_string(),
                driver: format!(
                    "simd={}, threads={}, pipeline=u8",
                    simd_label(settings.level),
                    settings.num_threads
                ),
                device_kind: "cpu".to_string(),
            },
            skips: SkipReport::default(),
            routed: 0,
        }
    }

    /// Fidelity gaps from the most recent render (see [`SkipReport`]).
    pub fn skips(&self) -> SkipReport {
        self.skips
    }

    /// How many [`Command`]s the most recent render's walk dispatched.
    ///
    /// Every command the walk sees is counted, including the pops and the
    /// commands inside a snapshot bracket — so a scene holding one of each
    /// variant reports its own full length, which is what "every variant is
    /// routed" means operationally.
    pub fn routed_commands(&self) -> usize {
        self.routed
    }

    /// Resizes the render target when `spec` asks for a different size.
    fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }
        self.ctx.reset_and_resize(width, height);
        self.pixmap.resize(width, height);
        self.width = width;
        self.height = height;
    }
}

impl Default for CpuOracle {
    fn default() -> Self {
        Self::new()
    }
}

/// The pinned, host-independent rasterizer settings every oracle context is
/// built with (see the module docs' Determinism section).
fn pinned_settings() -> RenderSettings {
    RenderSettings {
        // NOT `Level::try_detect()` (vello_cpu's own default): that answers
        // with whatever SIMD the *host* CPU happens to have, which would make
        // a promoted baseline machine-specific.
        level: Level::baseline(),
        // Single-threaded rasterization. The `multithreading` feature is off in
        // this crate's pin, so this is already the only option — set
        // explicitly so a future feature change cannot silently make the
        // oracle thread-count dependent.
        num_threads: 0,
    }
}

/// A short, stable label for a SIMD [`Level`] (`Level` is `Debug`-only and its
/// `Debug` output carries the level's proof token, e.g. `Avx2(Avx2)`).
fn simd_label(level: Level) -> String {
    let rendered = format!("{level:?}");
    rendered
        .split('(')
        .next()
        .unwrap_or(&rendered)
        .trim()
        .to_string()
}

impl SceneRenderer for CpuOracle {
    fn id(&self) -> &'static str {
        ORACLE_ID
    }

    fn meta(&self) -> BackendMeta {
        self.meta.clone()
    }

    fn render(&mut self, scene: &Scene, spec: &RenderSpec) -> anyhow::Result<RenderedImage> {
        // Refuse rather than clamp: a spec is part of the deterministic-input
        // contract, so a size this backend cannot honour must fail loudly
        // instead of quietly rendering a different frame than the one asked
        // for.
        let width = u16::try_from(spec.width).map_err(|_| {
            anyhow::anyhow!(
                "RenderSpec width {} exceeds vello_cpu's u16 target size",
                spec.width
            )
        })?;
        let height = u16::try_from(spec.height).map_err(|_| {
            anyhow::anyhow!(
                "RenderSpec height {} exceeds vello_cpu's u16 target size",
                spec.height
            )
        })?;
        anyhow::ensure!(
            width > 0 && height > 0,
            "RenderSpec must have a non-zero size, got {width}x{height}"
        );
        anyhow::ensure!(
            spec.scale.is_finite() && spec.scale > 0.0,
            "RenderSpec scale must be finite and positive, got {}",
            spec.scale
        );

        self.resize(width, height);
        self.ctx.reset();
        self.skips = SkipReport::default();
        self.routed = 0;

        // The base color, exactly as vello's `RenderParams::base_color`
        // behaves on the GPU path: the whole target first, under no transform.
        // A fully transparent base color is a no-op fill over the (already
        // cleared) target, which is the same result.
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(spec.base_color);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));

        // `scale` is applied AHEAD of `root` (see `RenderSpec::scale`), and
        // both ahead of each command's own recorded transform — the same
        // pre-multiplied-root shape `convert.rs`'s walk takes.
        let root = Affine::scale(spec.scale) * spec.root;
        {
            let mut painter = Painter {
                ctx: &mut self.ctx,
                resources: &mut self.resources,
                skips: SkipReport::default(),
            };
            self.routed = encode_commands(scene.commands(), root, &mut painter);
            self.skips = painter.skips;
        }

        self.ctx.flush();
        self.ctx.render(&mut self.pixmap, &mut self.resources);

        Ok(RenderedImage {
            width: spec.width,
            height: spec.height,
            // Premultiplied, straight from the pixmap — see the module docs'
            // Alpha section for why it is not converted.
            rgba8: self.pixmap.data_as_u8_slice().to_vec(),
            alpha: AlphaKind::Premultiplied,
            meta: self.meta.clone(),
        })
    }
}

/// Replays individual draw operations into a `vello_cpu` [`RenderContext`].
///
/// `vello_cpu` is a state machine (set paint/transform/stroke, then draw)
/// rather than vello's per-call-parameter shape, so every method sets the
/// context state fresh before drawing. The method set mirrors `frust-render`'s
/// `SceneSink` one-for-one (see the module docs) so the two walks can be diffed
/// by eye.
struct Painter<'a> {
    ctx: &'a mut RenderContext,
    resources: &'a mut Resources,
    skips: SkipReport,
}

impl Painter<'_> {
    /// Sets the context's current paint from a scene [`Brush`]. An image brush
    /// handed to a fill has no `vello_cpu` equivalent and falls back to
    /// transparent, counted into [`SkipReport::image_brush_fills`].
    fn set_brush(&mut self, brush: &Brush) {
        match brush {
            Brush::Solid(color) => self.ctx.set_paint(*color),
            Brush::Gradient(gradient) => self.ctx.set_paint(PaintType::Gradient(gradient.clone())),
            Brush::Image(_) => {
                self.skips.image_brush_fills += 1;
                self.ctx.set_paint(FALLBACK_PAINT);
            }
        }
    }

    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect) {
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(style);
        self.set_brush(brush);
        self.ctx.fill_rect(rect);
    }

    fn fill_rounded_rect(
        &mut self,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radii: RoundedRectRadii,
    ) {
        let path = RoundedRect::from_rect(*rect, radii).to_path(FLATTEN_TOLERANCE);
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.set_brush(brush);
        self.ctx.fill_path(&path);
    }

    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64) {
        let path = Line::new(p0, p1).to_path(FLATTEN_TOLERANCE);
        self.ctx.set_transform(transform);
        self.set_brush(brush);
        self.ctx.set_stroke(Stroke::new(width));
        self.ctx.stroke_path(&path);
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun) {
        // An empty run paints nothing on any backend; an unparseable font blob
        // would panic inside `glifo` (module docs' gap list). Both are skipped
        // here, before the builder is constructed.
        if run.glyphs.is_empty() || !looks_like_sfnt(run.font.font().data.data()) {
            self.skips.unrenderable_glyph_runs += 1;
            return;
        }
        self.ctx.set_transform(run.transform);
        self.set_brush(&run.brush);
        // Reads the context's current paint/transform and fills glyph outlines
        // directly. `hint(false)`: hinting is a per-size, per-target
        // adjustment, and a golden reference must not vary with it.
        self.ctx
            .glyph_run(self.resources, run.font.font())
            .font_size(run.font_size)
            .hint(false)
            .fill_glyphs(run.glyphs.iter().map(|g| vello_cpu_oracle::Glyph {
                id: g.id,
                x: g.x,
                y: g.y,
            }));
    }

    fn push_clip(&mut self, transform: Affine, rect: &Rect) {
        self.ctx.set_transform(transform);
        self.ctx.push_clip_layer(&rect.to_path(FLATTEN_TOLERANCE));
    }

    fn push_clip_rounded(&mut self, transform: Affine, rect: &Rect, radii: RoundedRectRadii) {
        let path = RoundedRect::from_rect(*rect, radii).to_path(FLATTEN_TOLERANCE);
        self.ctx.set_transform(transform);
        self.ctx.push_clip_layer(&path);
    }

    fn pop_clip(&mut self) {
        self.ctx.pop_layer();
    }

    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
        // `ImageSource::from_peniko_image_data` panics above `u16::MAX` in
        // either dimension, and a zero-sized image has no sampling space at
        // all — both are reported rather than rasterized (module docs' gaps).
        if data.width == 0
            || data.height == 0
            || data.width > u32::from(u16::MAX)
            || data.height > u32::from(u16::MAX)
        {
            self.skips.unrenderable_images += 1;
            return;
        }
        let natural_w = f64::from(data.width);
        let natural_h = f64::from(data.height);
        // The pixels travel with the paint (`ImageSource::Pixmap`) rather than
        // through a `Resources`-registered `ImageId`: an oracle renders each
        // case once, so a per-render handle registry would buy nothing and
        // would make the output depend on registration order.
        let image = Image {
            image: ImageSource::from_peniko_image_data(data),
            sampler: peniko::ImageSampler::default(),
        };
        // The command transform maps the local dest rect onto the target;
        // the PAINT transform maps the image's natural pixel space onto that
        // same local dest, and vello_cpu samples through the composition of
        // the two.
        let paint_transform = Affine::translate((dest.x0, dest.y0))
            * Affine::scale_non_uniform(dest.width() / natural_w, dest.height() / natural_h);
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(PaintType::Image(image));
        self.ctx.set_paint_transform(paint_transform);
        self.ctx.fill_rect(dest);
        self.ctx.reset_paint_transform();
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: &Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.ctx.set_transform(transform);
        self.ctx.set_paint(color);
        // 0.2.0's fourth argument (`invert`) is new since the 0.0.9 the CPU
        // tier calls: `false` is the ordinary drop shadow (an inset shadow
        // paints the inverse coverage), which is the only shape
        // `Command::BlurredRoundedRect` models.
        self.ctx
            .fill_blurred_rounded_rect(rect, radius as f32, std_dev as f32, false);
    }

    fn push_layer(&mut self, transform: Affine, rect: &Rect, alpha: f32) {
        self.ctx.set_transform(transform);
        // A clip to `rect` plus an opacity of `alpha` — the CPU equivalent of
        // vello's `push_layer(Fill, Blend, alpha, transform, rect)`.
        self.ctx.push_layer(
            Some(&rect.to_path(FLATTEN_TOLERANCE)),
            None,
            Some(alpha),
            None,
            None,
        );
    }

    fn pop_layer(&mut self) {
        self.ctx.pop_layer();
    }

    fn clear_rect(&mut self, transform: Affine, rect: &Rect) {
        // The hole-punch: a layer whose composite is `Compose::DestOut`, with
        // an opaque fill inside it, erases the region (color AND alpha) on pop,
        // weighted by the fill's own coverage. NOT `Compose::Clear` — vello's
        // GPU pipeline applies Clear at 16-px-tile granularity past unaligned
        // edges, so `frust-render` uses DestOut on both of its tiers and the
        // oracle matches, or every hole-punch case would diff at its edges.
        self.ctx.set_transform(transform);
        let clip = rect.to_path(FLATTEN_TOLERANCE);
        self.ctx.push_layer(
            Some(&clip),
            Some(BlendMode::new(Mix::Normal, Compose::DestOut)),
            None,
            None,
            None,
        );
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(Color::BLACK);
        self.ctx.fill_rect(rect);
        self.ctx.pop_layer();
    }

    fn fill_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath) {
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.set_brush(brush);
        self.ctx.fill_path(path);
    }

    fn stroke_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath, width: f64) {
        self.ctx.set_transform(transform);
        self.set_brush(brush);
        self.ctx.set_stroke(Stroke::new(width));
        self.ctx.stroke_path(path);
    }
}

/// An open clip/opacity group, tracked so a [`Command::ClearRect`] can be
/// hoisted past it and re-establish it afterwards.
enum Group {
    /// A clip group; `Some(radii)` when the clip has rounded corners, so the
    /// hoist re-pushes it with its corners intact rather than squared.
    Clip(Option<RoundedRectRadii>),
    /// An opacity group, carrying the alpha to re-push with.
    Layer(f32),
    /// The alpha layer `PushSnapshot`'s inline emulation pushes when its
    /// `alpha < 1.0`, backend-identical to [`Group::Layer`] (same
    /// `push_layer`/`pop_layer` calls) but tagged separately so it is VISIBLE
    /// to `PopClip`/`PopLayer`'s guard: a `SceneBuilder` can record either of
    /// those with no matching push (`scene.rs`'s documented unbalanced-pop
    /// policy), and since every group lives on one shared stack, such a pop
    /// consumes whatever is innermost — which can be this entry rather than
    /// its own kind. When that happens, `snapshot_layer_pushed` must be
    /// cleared so the matching `PopSnapshot` does not pop the same
    /// already-consumed backend layer a second time (the layer-stack
    /// underflow this variant exists to prevent — `vello_cpu` panics on it,
    /// unlike vello's GPU resolver, which tolerates the imbalance).
    SnapshotLayer(f32),
}

/// Walks `commands` into `painter` under `root`, pre-multiplied onto every
/// command's own recorded transform, and returns how many commands were
/// dispatched (every command the walk sees, pops included).
///
/// This is the oracle's copy of `frust-render`'s `convert.rs` command walk (see
/// the module docs for why it is a copy). It always renders the WHOLE scene
/// from its root: there is no segment/prefix split (that is the GPU path's
/// snapshot-compositor concern) and no shader-override map or hole set, so
/// every [`Command::ShaderQuad`] takes its miss path and every
/// [`Command::PushSnapshot`] its inline-emulation path.
fn encode_commands(commands: &[Command], root: Affine, painter: &mut Painter<'_>) -> usize {
    // Active clip/opacity groups, innermost last. A `ClearRect` must reach the
    // scene ROOT to erase what was painted outside the group it sits in (a
    // `DestOut` composite inside a group only erases that group's own
    // accumulated content), so the walk pops every open group, clears at root
    // bounded by the intersection of their clip bounds, then re-pushes them.
    let mut groups: Vec<(Group, Affine, Rect)> = Vec::new();

    // `PushSnapshot`'s inline emulation, matching `convert.rs`: only the
    // OUTERMOST bracket is honoured, its `scale` becomes a correction affine
    // conjugated by the bracket's own transform (`M * S * M.inverse()`), and
    // its `alpha` an ordinary opacity layer on the same group stack. An
    // unbalanced `PopSnapshot` is ignored, the policy `PopLayer` follows.
    let mut snapshot_depth: usize = 0;
    let mut snapshot_correction = Affine::IDENTITY;
    let mut snapshot_layer_pushed = false;
    let mut routed = 0_usize;

    for command in commands {
        routed += 1;
        let combined = root * snapshot_correction;
        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => painter.fill_rect(Fill::NonZero, combined * *transform, brush, rect),
            Command::RoundedRect {
                rect,
                radii,
                brush,
                transform,
            } => painter.fill_rounded_rect(combined * *transform, brush, rect, radii_of(*radii)),
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => painter.stroke_line(combined * *transform, brush, *p0, *p1, *width),
            Command::GlyphRun(run) => {
                if combined == Affine::IDENTITY {
                    painter.draw_glyph_run(run);
                } else {
                    let mut corrected = run.clone();
                    corrected.transform = combined * corrected.transform;
                    painter.draw_glyph_run(&corrected);
                }
            }
            Command::PushClip { rect, transform } => {
                let transform = combined * *transform;
                groups.push((Group::Clip(None), transform, *rect));
                painter.push_clip(transform, rect);
            }
            Command::PushClipRounded {
                rect,
                radii,
                transform,
            } => {
                // The same clip stack `PushClip` uses (one `PopClip` pops
                // either); the radii ride along so the `ClearRect` hoist can
                // re-push the rounded shape.
                let radii = radii_of(*radii);
                let transform = combined * *transform;
                groups.push((Group::Clip(Some(radii)), transform, *rect));
                painter.push_clip_rounded(transform, rect, radii);
            }
            Command::PopClip => {
                // Guarded, unlike `convert.rs`'s unconditional pop: a
                // `SceneBuilder` records a pop with no matching push happily,
                // vello tolerates the imbalance, and `vello_cpu` panics on a
                // layer-stack underflow. An oracle must not be the component
                // that dies on a malformed scene.
                //
                // The popped entry can be a `Group::SnapshotLayer` rather than
                // a `Group::Clip` — all three `Group` kinds share one stack,
                // so an unbalanced pop consumes whichever is innermost. When
                // it is the snapshot's own emulated layer, `pop_clip`'s
                // backend call is still correct (it and `pop_layer` both
                // reduce to the same `ctx.pop_layer()`), but the bookkeeping
                // must say so or the matching `PopSnapshot` pops the same
                // now-empty backend layer again (see `Group::SnapshotLayer`).
                if let Some((kind, ..)) = groups.pop() {
                    painter.pop_clip();
                    if matches!(kind, Group::SnapshotLayer(_)) {
                        snapshot_layer_pushed = false;
                    }
                }
            }
            Command::Image {
                data,
                dest,
                transform,
            } => painter.draw_image(combined * *transform, data, dest),
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
                transform,
            } => {
                // Neither backend has a per-corner blurred primitive, so a
                // per-corner shadow lowers to its LARGEST corner — the same
                // downgrade `convert.rs` makes, for the reason
                // `CornerRadii::largest` documents.
                painter.draw_blurred_rounded_rect(
                    combined * *transform,
                    rect,
                    *color,
                    radii.largest(),
                    *std_dev,
                );
            }
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => {
                let transform = combined * *transform;
                groups.push((Group::Layer(*alpha), transform, *rect));
                painter.push_layer(transform, rect, *alpha);
            }
            Command::PopLayer => {
                // Guarded for the same reason as `PopClip` above, including
                // the same possibility of consuming a `Group::SnapshotLayer`.
                if let Some((kind, ..)) = groups.pop() {
                    painter.pop_layer();
                    if matches!(kind, Group::SnapshotLayer(_)) {
                        snapshot_layer_pushed = false;
                    }
                }
            }
            Command::ClearRect { rect, transform } if groups.is_empty() => {
                painter.clear_rect(combined * *transform, rect);
            }
            Command::ClearRect { rect, transform } => {
                // Hoist to root: bound the punch by every open group's clip
                // bbox, pop them all, clear, then re-push. Bboxes are exact for
                // the axis-aligned transforms frust emits; a rotated clip bounds
                // conservatively. The punch is computed in `combined`'s space,
                // and each group's bbox was recorded under the same `combined`,
                // so the already-rooted rect is cleared under the identity.
                let mut punch = (combined * *transform).transform_rect_bbox(*rect);
                for (_, t, r) in &groups {
                    punch = punch.intersect(t.transform_rect_bbox(*r));
                }
                if punch.width() > 0.0 && punch.height() > 0.0 {
                    for (kind, ..) in groups.iter().rev() {
                        match kind {
                            Group::Clip(_) => painter.pop_clip(),
                            Group::Layer(_) | Group::SnapshotLayer(_) => painter.pop_layer(),
                        }
                    }
                    painter.clear_rect(Affine::IDENTITY, &punch);
                    for (kind, t, r) in &groups {
                        match kind {
                            Group::Clip(None) => painter.push_clip(*t, r),
                            Group::Clip(Some(radii)) => painter.push_clip_rounded(*t, r, *radii),
                            Group::Layer(alpha) | Group::SnapshotLayer(alpha) => {
                                painter.push_layer(*t, r, *alpha)
                            }
                        }
                    }
                }
            }
            Command::Path {
                path,
                style,
                brush,
                transform,
            } => {
                let transform = combined * *transform;
                match style {
                    PathStyle::Fill => painter.fill_path(transform, brush, path),
                    PathStyle::Stroke { width, dash } => match dash {
                        Some(dash) if dash.is_effective() => {
                            // Dashes are pre-flattened into sub-paths with
                            // kurbo's own iterator, exactly as `convert.rs`
                            // does: `vello_cpu`'s `set_stroke` ignores a
                            // `Stroke`'s dash fields, so flattening at the
                            // decode site is what keeps the two comparable.
                            let dashed: BezPath =
                                kurbo::dash(path.iter(), dash.phase, &[dash.on, dash.off])
                                    .collect();
                            painter.stroke_path(transform, brush, &dashed, *width);
                        }
                        // No pattern, or a degenerate one: a solid stroke.
                        _ => painter.stroke_path(transform, brush, path, *width),
                    },
                }
            }
            Command::ShaderQuad {
                program: _,
                dest,
                transform,
                time: _,
            } => {
                // No shader pre-pass exists without a GPU, so this always takes
                // `convert.rs`'s MISS path: the same opaque dark placeholder
                // fill, reported through `SkipReport` so a corpus knows this
                // frame is not a faithful reference.
                painter.skips.shader_quads += 1;
                painter.fill_rect(
                    Fill::NonZero,
                    combined * *transform,
                    &Brush::Solid(SHADER_PLACEHOLDER),
                    dest,
                );
            }
            Command::PushSnapshot {
                key: _,
                rect,
                alpha,
                scale,
                transform,
            } => {
                // Always the inline-emulation (MISS) path — the oracle keeps no
                // rasterization cache and is handed no hole set, so a bracket is
                // painted exactly as if the recorder had emitted
                // `push_transform(scale_about(rect.center(), scale))` then
                // `push_layer(rect, alpha)`.
                if snapshot_depth == 0 {
                    if *scale != 1.0 {
                        let m = *transform;
                        let s = Affine::scale_about(*scale, rect.center());
                        // Conjugating `S` by the bracket's own transform is what
                        // makes the inserted scale land correctly on a body
                        // command whose transform was recorded as `M * (its own
                        // pushes)`.
                        snapshot_correction = m * s * m.inverse();
                    }
                    if *alpha < 1.0 {
                        let corrected = root * snapshot_correction * *transform;
                        groups.push((Group::SnapshotLayer(*alpha), corrected, *rect));
                        painter.push_layer(corrected, rect, *alpha);
                        snapshot_layer_pushed = true;
                    }
                }
                snapshot_depth += 1;
            }
            Command::PopSnapshot => {
                if snapshot_depth > 0 {
                    snapshot_depth -= 1;
                    if snapshot_depth == 0 {
                        // `snapshot_layer_pushed` is only true here when the
                        // `Group::SnapshotLayer` this bracket pushed is still
                        // on the stack: `PopClip`/`PopLayer` clear the flag
                        // themselves the moment either one consumes it (see
                        // `Group::SnapshotLayer`'s docs), so this pop can
                        // never underflow the backend's layer stack a second
                        // time — the review counterexample this guards
                        // against (`push_snapshot` alpha < 1.0, an unbalanced
                        // `PopClip`/`PopLayer`, then `pop_snapshot`).
                        if snapshot_layer_pushed {
                            groups.pop();
                            painter.pop_layer();
                            snapshot_layer_pushed = false;
                        }
                        snapshot_correction = Affine::IDENTITY;
                    }
                }
            }
        }
    }

    // Close whatever the scene left open, innermost first. A balanced scene
    // closes nothing; an unbalanced one would otherwise reach `vello_cpu`'s
    // rasterizer with a pending layer.
    for (kind, ..) in groups.iter().rev() {
        match kind {
            Group::Clip(_) => painter.pop_clip(),
            Group::Layer(_) | Group::SnapshotLayer(_) => painter.pop_layer(),
        }
    }

    routed
}

/// The scene's per-corner radii in the `kurbo` shape vocabulary — the one
/// conversion site for the scene-to-rasterizer radii hop (scene-layer purity
/// keeps `kurbo::RoundedRectRadii` out of `frust-scene`'s commands).
fn radii_of(radii: frust_scene::CornerRadii) -> RoundedRectRadii {
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
    use frust_scene::{CornerRadii, DashPattern, FontHandle, SceneBuilder, ShaderProgram};
    use peniko::color::palette::css::{BLUE, GREEN, RED};
    use peniko::{Blob, FontData};

    /// A `RenderSpec` of `size`x`size`, cleared to `base`, with no scaling and
    /// no root transform — the shape every test below renders under.
    fn spec(size: u32, base: Color) -> RenderSpec {
        RenderSpec {
            width: size,
            height: size,
            base_color: base,
            scale: 1.0,
            root: Affine::IDENTITY,
        }
    }

    /// The premultiplied RGBA8 pixel at `(x, y)` of a rendered frame.
    fn pixel(image: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y * image.width + x) * 4) as usize;
        [
            image.rgba8[idx],
            image.rgba8[idx + 1],
            image.rgba8[idx + 2],
            image.rgba8[idx + 3],
        ]
    }

    /// Every [`Command`] variant's name.
    ///
    /// The match is EXHAUSTIVE on purpose: it is the tripwire that fails the
    /// build the moment a 17th variant is added to `frust-scene` without the
    /// oracle learning to route it.
    fn variant_name(command: &Command) -> &'static str {
        match command {
            Command::FillRect { .. } => "FillRect",
            Command::RoundedRect { .. } => "RoundedRect",
            Command::Line { .. } => "Line",
            Command::GlyphRun(_) => "GlyphRun",
            Command::PushClip { .. } => "PushClip",
            Command::PushClipRounded { .. } => "PushClipRounded",
            Command::PopClip => "PopClip",
            Command::Image { .. } => "Image",
            Command::BlurredRoundedRect { .. } => "BlurredRoundedRect",
            Command::PushLayer { .. } => "PushLayer",
            Command::PopLayer => "PopLayer",
            Command::ClearRect { .. } => "ClearRect",
            Command::Path { .. } => "Path",
            Command::ShaderQuad { .. } => "ShaderQuad",
            Command::PushSnapshot { .. } => "PushSnapshot",
            Command::PopSnapshot => "PopSnapshot",
        }
    }

    /// The complete variant list, in `Command`'s own declaration order.
    const ALL_VARIANTS: [&str; 16] = [
        "FillRect",
        "RoundedRect",
        "Line",
        "GlyphRun",
        "PushClip",
        "PushClipRounded",
        "PopClip",
        "Image",
        "BlurredRoundedRect",
        "PushLayer",
        "PopLayer",
        "ClearRect",
        "Path",
        "ShaderQuad",
        "PushSnapshot",
        "PopSnapshot",
    ];

    /// A 4x4 opaque-red RGBA8 image, the smallest thing `Command::Image` can
    /// carry that still has a sampling space.
    fn image_data() -> ImageData {
        let pixels: Vec<u8> = (0..4 * 4).flat_map(|_| [255, 0, 0, 255]).collect();
        ImageData {
            data: Blob::from(pixels),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 4,
            height: 4,
        }
    }

    /// A triangle, used for both the fill and the stroke `Path` cases.
    fn triangle() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((3.0, 3.0));
        path.line_to((14.0, 3.0));
        path.line_to((8.0, 14.0));
        path.close_path();
        path
    }

    /// A scene holding at least one of every `Command` variant, balanced (two
    /// clip pushes share the one `PopClip` variant, so it appears twice).
    fn every_variant_scene() -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Brush::Solid(GREEN));
            builder.fill_rounded_rect(Rect::new(2.0, 2.0, 16.0, 16.0), 4.0, Brush::Solid(RED));
            builder.stroke_line(
                Point::new(0.0, 0.0),
                Point::new(20.0, 20.0),
                1.5,
                Brush::Solid(BLUE),
            );
            builder.draw_glyph_run(GlyphRun {
                font: FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0)),
                font_size: 12.0,
                brush: Brush::Solid(BLUE),
                transform: Affine::IDENTITY,
                glyphs: Vec::new(),
            });
            builder.draw_blurred_rounded_rect(
                Rect::new(1.0, 1.0, 18.0, 18.0),
                3.0,
                2.0,
                Color::BLACK,
            );
            builder.draw_image(&image_data(), Rect::new(1.0, 1.0, 9.0, 9.0));
            builder.draw_shader(
                &ShaderProgram::new("// no shader compiles on the CPU"),
                Rect::new(10.0, 10.0, 18.0, 18.0),
                0.25,
            );
            builder.push_clip(Rect::new(0.0, 0.0, 18.0, 18.0));
            builder.push_clip_rounded_radii(
                Rect::new(1.0, 1.0, 17.0, 17.0),
                CornerRadii::new(6.0, 0.0, 3.0, 1.0),
            );
            builder.push_layer(Rect::new(2.0, 2.0, 16.0, 16.0), 0.5);
            builder.push_snapshot(7, Rect::new(3.0, 3.0, 15.0, 15.0), 0.8, 1.25);
            builder.fill_path(triangle(), Brush::Solid(GREEN));
            builder.stroke_path_dashed(
                triangle(),
                1.0,
                DashPattern::new(3.0, 2.0).with_phase(1.0),
                Brush::Solid(RED),
            );
            builder.clear_rect(Rect::new(6.0, 6.0, 10.0, 10.0));
            builder.pop_snapshot();
            builder.pop_layer();
            builder.pop_clip();
            builder.pop_clip();
        }
        scene
    }

    #[test]
    fn every_command_variant_is_routed() {
        let scene = every_variant_scene();
        let seen: Vec<&'static str> = scene.commands().iter().map(variant_name).collect();
        for variant in ALL_VARIANTS {
            assert!(
                seen.contains(&variant),
                "Command::{variant} is missing from the coverage scene — the oracle's routing \
                 for it is unproven"
            );
        }
        let mut distinct = seen.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            ALL_VARIANTS.len(),
            "the coverage scene must hold every variant and nothing else, saw {distinct:?}"
        );

        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(20, Color::WHITE))
            .expect("the coverage scene renders");

        // Every command in the scene reached the walk's dispatch — the
        // operational meaning of "all 16 variants routed".
        assert_eq!(
            oracle.routed_commands(),
            scene.commands().len(),
            "the walk must dispatch every recorded command"
        );
        assert_eq!(image.rgba8.len(), 20 * 20 * 4);
        assert_eq!(image.alpha, AlphaKind::Premultiplied);

        // The two intentional gaps in this scene, reported rather than silent.
        let skips = oracle.skips();
        assert_eq!(skips.shader_quads, 1, "the shader quad is a reported gap");
        assert_eq!(
            skips.unrenderable_glyph_runs, 1,
            "the fontless glyph run is a reported gap"
        );
        assert_eq!(skips.image_brush_fills, 0);
        assert_eq!(skips.unrenderable_images, 0);
    }

    #[test]
    fn base_color_clears_the_whole_target() {
        let scene = Scene::new();
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, RED))
            .expect("an empty scene renders");
        assert_eq!(image.rgba8.len(), 8 * 8 * 4);
        assert_eq!(pixel(&image, 4, 4), [255, 0, 0, 255]);
        assert!(oracle.skips().is_empty());
    }

    #[test]
    fn a_transparent_base_color_leaves_the_target_clear() {
        // The alpha contract is only meaningful if an untouched pixel really is
        // transparent — premultiplied zero, not opaque black.
        let scene = Scene::new();
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(4, Color::TRANSPARENT))
            .expect("an empty scene renders");
        assert_eq!(pixel(&image, 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn solid_fill_lands_where_the_scene_put_it() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(20, Color::WHITE))
            .expect("scene renders");
        assert_eq!(pixel(&image, 5, 5), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 15, 15), [255, 255, 255, 255]);
    }

    #[test]
    fn the_render_spec_root_and_scale_are_both_applied() {
        // The same rect, once unscaled and once under `scale`/`root`: the
        // spec's transform is composed ahead of the command's own, so the
        // rect must move.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();

        let plain = oracle
            .render(&scene, &spec(20, Color::WHITE))
            .expect("scene renders");
        assert_eq!(pixel(&plain, 7, 7), [255, 255, 255, 255]);

        let scaled = oracle
            .render(
                &scene,
                &RenderSpec {
                    scale: 2.0,
                    ..spec(20, Color::WHITE)
                },
            )
            .expect("scene renders");
        assert_eq!(pixel(&scaled, 7, 7), [255, 0, 0, 255]);

        let translated = oracle
            .render(
                &scene,
                &RenderSpec {
                    root: Affine::translate((10.0, 10.0)),
                    ..spec(20, Color::WHITE)
                },
            )
            .expect("scene renders");
        assert_eq!(pixel(&translated, 12, 12), [255, 0, 0, 255]);
    }

    #[test]
    fn a_clip_restricts_what_is_painted() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 10.0, 20.0));
            builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Brush::Solid(BLUE));
            builder.pop_clip();
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(20, GREEN))
            .expect("scene renders");
        assert_eq!(pixel(&image, 5, 10)[2], 255, "inside the clip is blue");
        assert_eq!(pixel(&image, 15, 10)[2], 0, "outside the clip is not blue");
    }

    #[test]
    fn a_clear_rect_inside_a_clip_punches_through_to_the_root() {
        // The hoist is the whole reason `ClearRect` is not a plain composite:
        // a backdrop painted OUTSIDE the clip group must be erased too.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Brush::Solid(RED));
            builder.push_clip(Rect::new(0.0, 0.0, 20.0, 20.0));
            builder.clear_rect(Rect::new(4.0, 4.0, 12.0, 12.0));
            builder.pop_clip();
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(20, Color::TRANSPARENT))
            .expect("scene renders");
        assert_eq!(
            pixel(&image, 8, 8),
            [0, 0, 0, 0],
            "the punched region must be fully transparent, backdrop included"
        );
        assert_eq!(pixel(&image, 18, 18), [255, 0, 0, 255]);
    }

    #[test]
    fn an_unbalanced_pop_does_not_panic() {
        // A `SceneBuilder` records a pop with no matching push happily, so the
        // oracle must survive one (`vello_cpu` underflows its layer stack).
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.pop_clip();
            builder.pop_layer();
            builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("an unbalanced scene still renders");
        assert_eq!(pixel(&image, 2, 2), [255, 0, 0, 255]);
    }

    #[test]
    fn an_unbalanced_pop_clip_inside_a_snapshot_layer_does_not_panic() {
        // The review counterexample: an alpha < 1.0 `PushSnapshot` bracket
        // emulates its alpha as a `groups` entry and pushes ONE backend
        // layer; an unbalanced `PopClip` with no matching push then consumes
        // that same entry (`groups.pop()` does not distinguish which kind of
        // group is innermost) and pops the one backend layer that exists.
        // The matching `PopSnapshot` must not pop a SECOND time — before the
        // fix, it did unconditionally, underflowing `vello_cpu`'s layer
        // stack and panicking.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(1, Rect::new(0.0, 0.0, 8.0, 8.0), 0.5, 1.0);
            builder.pop_clip();
            builder.pop_snapshot();
            builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("the counterexample must render without panicking");
        assert_eq!(
            pixel(&image, 2, 2),
            [255, 0, 0, 255],
            "rendering must resume normally after the desynced pop"
        );
    }

    #[test]
    fn an_unbalanced_pop_layer_inside_a_snapshot_layer_does_not_panic() {
        // Same shape as the `PopClip` counterexample above, exercising the
        // `PopLayer` arm's own guard instead.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(1, Rect::new(0.0, 0.0, 8.0, 8.0), 0.5, 1.0);
            builder.pop_layer();
            builder.pop_snapshot();
            builder.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("the PopLayer counterexample must render without panicking");
        assert_eq!(
            pixel(&image, 2, 2),
            [255, 0, 0, 255],
            "rendering must resume normally after the desynced pop"
        );
    }

    #[test]
    fn an_unclosed_push_is_closed_by_the_walk() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 4.0, 4.0));
            builder.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Brush::Solid(RED));
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("a scene with an open group still renders");
        assert_eq!(pixel(&image, 2, 2), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 6, 6), [255, 255, 255, 255]);
    }

    #[test]
    fn renders_are_reproducible_byte_for_byte() {
        // The oracle's whole purpose: the same scene and spec produce the same
        // bytes, including across a resize that forces a context rebuild.
        let scene = every_variant_scene();
        let mut oracle = CpuOracle::new();
        let first = oracle
            .render(&scene, &spec(20, Color::WHITE))
            .expect("scene renders");
        let _ = oracle
            .render(&scene, &spec(9, GREEN))
            .expect("scene renders at another size");
        let second = oracle
            .render(&scene, &spec(20, Color::WHITE))
            .expect("scene renders again");
        assert_eq!(first.rgba8, second.rgba8);
        assert_eq!(first.meta, second.meta);
    }

    #[test]
    fn an_image_brush_fill_is_reported_as_a_gap() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(
                Rect::new(0.0, 0.0, 8.0, 8.0),
                Brush::Image(peniko::ImageBrush {
                    image: image_data(),
                    sampler: peniko::ImageSampler::default(),
                }),
            );
        }
        let mut oracle = CpuOracle::new();
        let image = oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("scene renders");
        assert_eq!(oracle.skips().image_brush_fills, 1);
        assert!(!oracle.skips().is_empty());
        // Painted transparent over the white base, i.e. the base survives.
        assert_eq!(pixel(&image, 4, 4), [255, 255, 255, 255]);
    }

    #[test]
    fn a_degenerate_image_is_reported_rather_than_rasterized() {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_image(
                &ImageData {
                    data: Blob::from(Vec::<u8>::new()),
                    format: peniko::ImageFormat::Rgba8,
                    alpha_type: peniko::ImageAlphaType::Alpha,
                    width: 0,
                    height: 0,
                },
                Rect::new(0.0, 0.0, 4.0, 4.0),
            );
        }
        let mut oracle = CpuOracle::new();
        oracle
            .render(&scene, &spec(8, Color::WHITE))
            .expect("scene renders");
        assert_eq!(oracle.skips().unrenderable_images, 1);
    }

    #[test]
    fn a_zero_sized_or_non_finite_spec_is_refused() {
        let scene = Scene::new();
        let mut oracle = CpuOracle::new();
        assert!(oracle.render(&scene, &spec(0, Color::WHITE)).is_err());
        assert!(
            oracle
                .render(
                    &scene,
                    &RenderSpec {
                        width: 70_000,
                        ..spec(8, Color::WHITE)
                    }
                )
                .is_err()
        );
        assert!(
            oracle
                .render(
                    &scene,
                    &RenderSpec {
                        scale: f64::NAN,
                        ..spec(8, Color::WHITE)
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn the_font_sniff_accepts_sfnt_tags_and_rejects_the_rest() {
        assert!(looks_like_sfnt(&[0x00, 0x01, 0x00, 0x00, 0xFF]));
        assert!(looks_like_sfnt(b"OTTO....."));
        assert!(looks_like_sfnt(b"true...."));
        assert!(looks_like_sfnt(b"ttcf...."));
        assert!(!looks_like_sfnt(b""));
        assert!(!looks_like_sfnt(b"not"));
        assert!(!looks_like_sfnt(b"notafont"));
    }

    #[test]
    fn backend_identity_is_stable_and_records_the_pinned_simd_level() {
        let oracle = CpuOracle::new();
        assert_eq!(oracle.id(), ORACLE_ID);
        let meta = oracle.meta();
        assert_eq!(meta.backend, ORACLE_ID);
        assert_eq!(meta.device_kind, "cpu");
        assert!(meta.adapter.contains("vello_cpu 0.2.0"));
        assert!(
            meta.driver.contains(&simd_label(Level::baseline())),
            "the pinned SIMD level belongs in a promoted baseline's provenance, got {}",
            meta.driver
        );
        assert!(meta.driver.contains("threads=0"));
    }
}
