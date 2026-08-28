//! Per-surface cache of rasterized [`Command::PushSnapshot`] bodies — the GPU
//! half of snapshot layers.
//!
//! A snapshot bracket is the scene layer's statement that a subtree's pixels
//! are stable while only its presentation `alpha`/`scale` animate (see
//! [`Command::PushSnapshot`]'s contract). This module rasterizes each
//! OUTERMOST bracket's body once into its own `Rgba8Unorm` texture and returns
//! a [`FramePlan`]: the ordered [`CompositeLayer`]s to draw, the set of bracket
//! keys [`crate::convert`]'s encode walk turns into HOLES (skipped entirely,
//! emitting nothing), and the two-segment split of the frame's command list
//! those two facts imply. A body is re-rasterized only when its content
//! fingerprint or its device size changes; an ordinary slide/fade/scale of the
//! bracket as a whole re-uses the texture untouched, which is the entire point
//! of the mechanism.
//!
//! ## Who consumes the plan
//!
//! The compositor (`compositor.rs`) draws each layer as one alpha-blended quad
//! in a wgpu render pass, OUTSIDE vello: a vello image quad over a whole page
//! costs ~100 ms/frame on a low-end mobile GPU, and dodging it is why these
//! textures never enter vello's image-override atlas at all. One frame is
//! therefore composed as
//!
//! ```text
//! vello(pre) -> composite(layers)
//!            -> [vello(trailing, holes skipped, transparent scratch)
//!                -> composite(full quad)]
//! ```
//!
//! with the bracketed second half present only when [`FramePlan::trailing`] is
//! `Some` — i.e. only when the scene draws something after the first
//! composited bracket that is not itself composited. Content recorded BETWEEN
//! two composited brackets therefore lands above both: an accepted z-order
//! approximation of a two-pass split, not a bug.
//!
//! ## Only segments that DRAW get a vello pass
//!
//! A segment is worth a vello pass only when it holds a command that paints a
//! pixel ([`draws_pixels`]): a `PushClip`/`PopClip` pair, a layer push/pop or
//! a composited bracket's own brackets emit no coverage, and a vello pass over
//! them still pays the full fine-stage cost of the target (~14.5 ms on an
//! Adreno 620, measured, whatever it draws). Both ends of the split are
//! therefore decided on drawing commands alone:
//!
//! - [`FramePlan::trailing`] is `None` when nothing outside the later
//!   composited brackets draws in that range — a scene ending in the app
//!   root's `PopClip` no longer buys a whole pass for it;
//! - [`FramePlan::pre_draws`] is `false` when the pre segment draws nothing,
//!   which lets the render path skip the MAIN vello pass entirely and hand the
//!   frame's `base_color` to the compositor as its render pass's clear (see
//!   `compositor::CompositeTarget::clear`). A page transition whose pre
//!   segment is only the app root's `PushClip` then costs one quad pass for
//!   the whole frame.
//!
//! ## Enclosing clips and layers
//!
//! A composited bracket is drawn as a quad OUTSIDE vello, so any clip the
//! scene had pushed around it is no longer applied to its pixels — inline, the
//! body would have been clipped. Each layer therefore carries
//! [`CompositeLayer::clip`]: the device-space intersection of the bboxes of
//! the clips enclosing its bracket (`None` when there are none), which the
//! compositor applies as a scissor rect. A rounded clip contributes its
//! rect, so the corners it would have cut are an accepted approximation —
//! a scissor is rectangular.
//!
//! An enclosing `PushLayer` bounds its content exactly as a clip does, so its
//! rect joins that intersection. Its OPACITY has nowhere to go — the quad's
//! blend already carries the bracket's own `alpha` — so a bracket enclosed by
//! a layer with `alpha < 1.0` is not composited at all: it yields no plan and
//! no hole and lowers inline, where the layer applies to it normally
//! ([`outermost_brackets`]).
//!
//! ## Coordinate mapping
//!
//! `PushSnapshot`'s `rect` is the body's bounds in the LOCAL space of the
//! bracket's own transform `M`, while the body's commands carry their own
//! already-composed transforms (which include `M`). The compositor maps the
//! texture's own pixel rect `(0, 0)`..`(width, height)` onto `rect` under `M *
//! scale_about(scale, rect.center())` — so texture pixel `(0, 0)` must be
//! `rect`'s local origin and texture pixel `(width, height)` its far corner.
//! Rasterizing therefore runs the body under
//!
//! ```text
//! root = scale(width / rect.width, height / rect.height)
//!      * translate(-rect.origin)
//!      * M.inverse()
//! ```
//!
//! read right to left: `M.inverse()` takes a body command's composed space
//! back to `rect`'s local space, `translate` puts the rect's origin at the
//! texture origin, and the scale spreads the rect over the texture's whole
//! pixel grid. The scale factor is `width / rect.width` rather than the raster
//! scale itself so the outward pixel rounding in [`snapshot_size`] lands
//! exactly on the texture edge instead of a fraction of a pixel short.
//!
//! Resolution comes from `raster` — `M`'s linear part with the translation
//! zeroed, i.e. the surface's device scale for the page — so a body is
//! rasterized at the pixel density it will be composited at. `raster` (not the
//! full `M`) is what the size derives from and what the fingerprint mixes in:
//! a bracket that merely slides keeps both, and keeps its texture.
//!
//! The fingerprint's own frame follows the same origin as `root`:
//! [`body_fingerprint`] hashes the body relative to `base = M *
//! Affine::translate(rect.origin)`, i.e. `rect`'s own top-left corner in `M`'s
//! space — the same point `root` maps onto texture pixel `(0, 0)`. A body
//! whose absolute geometry slides frame-to-frame (a widget's origin baked
//! directly into its paint commands rather than into `M`; see
//! `frust_scene::fingerprint`'s module docs) therefore fingerprints equal to
//! its unslid self once the bracket's own `rect` slides by the same amount —
//! exactly what happens when the widget that owns the bracket is itself the
//! thing sliding.
//!
//! ## Alpha
//!
//! vello renders STRAIGHT (un-premultiplied) alpha into a target texture — its
//! fine stage divides the premultiplied accumulator by alpha on the final
//! store — so the texture holds exactly what the compositor's own fragment
//! stage must premultiply as it samples, with the bracket's `alpha` riding on
//! the quad's blend rather than on any vello layer. The texture is handed on
//! exactly as vello rendered it: the crate's premultiply compute pass belongs
//! to premultiplied-expecting swapchains only, and running it here would
//! premultiply twice and darken every translucent edge. `compositor.rs`'s
//! `snapshot_layer_composites_pixel_identically` GPU smoke pins this end to
//! end by comparing a cached bracket against the same bracket lowered inline
//! (it lives beside the pass it exercises, not in `tests/gpu_smoke.rs`: the
//! compositor is crate-private).
//!
//! ## Atlas-generation churn
//!
//! Every `render_to_texture` call resolves vello's image atlas, and each
//! resolve advances the atlas generation counter; an entry unused for two
//! generations is evicted and re-uploaded on next use. So each EXTRA snapshot
//! render inside one frame ages the main scene's bitmap images by a
//! generation and can force their re-upload. This is why the cache renders
//! only what actually changed: the steady state of an animating bracket is
//! zero snapshot renders per frame, leaving the frame's single main-scene
//! resolve exactly as it was before the cache existed.
//!
//! ## What is never cached
//!
//! - A body containing a `ClearRect` (the platform-view hole punch). The punch
//!   is a destination-clearing composite the encode walk hoists to the scene
//!   root; inside a snapshot texture it would erase texture pixels instead of
//!   the surface, sealing the hole a hosted native view shows through.
//! - A body containing a `ShaderQuad`. The command-slice encode entry point
//!   carries no shader-override map, so a rasterized body would bake the
//!   miss placeholder in place of the live shader.
//! - A degenerate `rect` or a non-invertible bracket transform (no mapping to
//!   derive), and a `key` recorded twice in one frame (the cache is keyed by
//!   `key` alone, so two brackets sharing one would draw each other's pixels).
//! - A bracket whose texture would cover more than
//!   [`SNAPSHOT_AREA_BUDGET_FACTOR`] times the surface's own pixel area:
//!   [`snapshot_size`] clamps each AXIS to the adapter limit, which on its own
//!   still admits a 268 MB page.
//!
//! Each of those simply yields no layer and no hole, so the bracket lowers
//! inline via [`crate::convert`]'s MISS path in whichever segment it falls —
//! the path the renderer used before this cache existed, never a panic and
//! never a wrong-pixels shortcut.
//!
//! One refusal is frame-wide rather than per-bracket: a `ClearRect` recorded
//! AFTER the first composited bracket ([`punches_after_first_bracket`]). It
//! would land in the trailing segment, whose pass renders into a transparent
//! scratch texture the punch erases instead of the swapchain — so the frame
//! composites nothing at all and takes the ordinary single-pass path, exactly
//! as the kill switch does.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use frust_scene::{Command, Scene};
use kurbo::{Affine, Rect};

use crate::convert::{Segment, encode_commands_into};
use crate::shader_effects::clamp_size;

/// How many frames an entry may go untouched before it is dropped: an entry
/// last used before `frame - MAX_UNUSED_FRAMES` is gone.
/// Two frames of slack keeps a bracket that skips a frame (a scene diff
/// hiccup, an every-other-frame repaint) from paying a full re-rasterization,
/// while a page navigated away from releases its texture almost immediately
/// rather than at surface teardown.
const MAX_UNUSED_FRAMES: u64 = 2;

/// Fixed-point quantization step applied to `raster`'s coefficients before
/// they are hashed into an entry's fingerprint — the same step
/// `frust_scene`'s command fingerprint uses on every float it hashes, so
/// float noise from composing/inverting transforms can never flip a
/// fingerprint for two mathematically identical placements.
const RASTER_QUANT: f64 = 4096.0;

/// How far a `raster` coefficient may stray from zero and still count as
/// axis-aligned in [`snapshot_size`]. Composing and inverting transforms
/// leaves shear coefficients at float noise rather than at a clean `0.0`; a
/// real rotation is orders of magnitude above this.
const AXIS_ALIGNED_EPSILON: f64 = 1e-9;

/// Shaved off an axis-aligned device extent before it is rounded up in
/// [`snapshot_size`], so `392.72727... * 2.75` landing a few ULPs above 1080
/// does not buy a 1081st pixel column — and, one frame later when it lands a
/// few ULPs below, throw the texture away again.
const SIZE_EPSILON: f64 = 1e-6;

/// How many times the SURFACE's own pixel area one cached page may cover
/// before the bracket is refused and lowered inline.
///
/// [`snapshot_size`] clamps each AXIS to vello's atlas ceiling and the
/// adapter's limit, which alone permits an 8192x8192 `Rgba8Unorm` texture —
/// 268 MB for one page. An area budget is the missing half: a page is a
/// widget subtree drawn onto this surface, so its texture is at most the
/// surface's own physical size (its rect times the device scale), and 2x
/// leaves generous room for the legitimate overhang — a page recorded at full
/// extent while it slides in from off-screen, a rect rounded outward on both
/// axes — while still refusing a bracket whose rect is pathological.
///
/// A bracket over the budget yields no plan and no hole and lowers inline,
/// exactly like every other refusal in the module header's "what is never
/// cached".
const SNAPSHOT_AREA_BUDGET_FACTOR: u64 = 2;

/// One cached rasterization: the texture vello renders the body into, plus the
/// bookkeeping the reuse and eviction decisions read.
///
/// The `wgpu::Texture` itself is not held separately — `view` is refcounted
/// onto it and keeps it alive, and every use here (re-rendering into it,
/// sampling it from the compositor) goes through a view.
struct Entry {
    /// Render target for `render_to_texture`, and the handle the compositor
    /// samples. Cloned into each frame's [`CompositeLayer`]; `wgpu` views are
    /// refcounted, so the clone is a handle, not a copy of the pixels.
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// Content fingerprint of the body this texture holds, mixed with the size
    /// and the device scale it was rasterized under (see [`body_fingerprint`]).
    fingerprint: u64,
    /// Frame counter value when this entry was last part of a scene.
    last_used: u64,
    /// How many times this entry's texture has been rasterized. The
    /// steady-state assertion: an animating bracket holds this at 1.
    renders: u32,
}

/// One cached page to composite this frame: the texture to sample and the
/// placement/opacity to sample it with.
///
/// Crate-private, like everything else here — a `wgpu` type never leaves
/// `frust-render` (`docs/CODE_STANDARDS.md`). The mapping the compositor
/// applies is the module's coordinate-mapping section read forwards: texture
/// pixels `(0, 0)`..`(width, height)` cover `rect` under `transform *
/// scale_about(scale, rect.center())`, with `alpha` on the blend.
pub(crate) struct CompositeLayer {
    /// The owning [`Command::PushSnapshot`]'s `key`, which is also its entry
    /// in [`FramePlan::holes`].
    pub key: u64,
    /// The bracket's bounds in the LOCAL space of `transform`.
    pub rect: Rect,
    /// The bracket's presentation opacity this frame.
    pub alpha: f32,
    /// The bracket's presentation scale this frame, about `rect`'s center.
    pub scale: f64,
    /// The bracket's own transform `M` — the body's device placement, before
    /// the presentation scale.
    pub transform: Affine,
    /// The texture's natural pixel size, which is what `rect` maps onto.
    pub width: u32,
    pub height: u32,
    /// The clips enclosing the bracket, intersected, in DEVICE pixels —
    /// `None` when the bracket sits under no clip at all. The quad is drawn
    /// outside vello, so this is the only thing that still confines it to what
    /// an inline body would have been clipped to; the compositor applies it as
    /// a scissor rect (see the module header's "Enclosing clips").
    pub clip: Option<Rect>,
    /// A refcounted handle on the cached texture.
    pub view: wgpu::TextureView,
}

/// How one frame is rendered: which command ranges vello draws, which cached
/// pages the compositor draws between them, and which brackets the encode walk
/// must skip so the two never draw the same pixels twice.
///
/// See the module header for the pass order these four fields describe.
pub(crate) struct FramePlan {
    /// Commands before the first composited bracket — the main vello pass.
    /// The WHOLE command range when nothing is composited, which is what makes
    /// a disabled cache (and a frame with no cache hits) an ordinary
    /// single-pass frame.
    ///
    /// Its [`Segment::prefix`] is ALWAYS empty: the pre segment starts at the
    /// scene root, so it opens every group it needs itself.
    pub pre: Segment,
    /// Whether [`Self::pre`] holds a command that paints a pixel
    /// ([`draws_pixels`]).
    ///
    /// `false` with a non-empty [`Self::layers`] is the render path's licence
    /// to skip the main vello pass altogether and let the compositor's own
    /// pass clear the frame instead: the pre segment is then pure structure
    /// (the app root's `PushClip` and nothing else, the measured shape of a
    /// page transition) and a vello pass over it would draw no pixel at the
    /// full cost of one.
    pub pre_draws: bool,
    /// Every composited bracket, in scene order.
    pub layers: Vec<CompositeLayer>,
    /// `layers`' keys: the brackets [`crate::convert`] turns into holes.
    pub holes: HashSet<u64>,
    /// Commands after the first composited bracket, `Some` only when that
    /// range holds at least one command that PAINTS and is not itself inside a
    /// composited bracket — i.e. only when a second vello pass would actually
    /// put pixels on the frame. `None` is the common case (a page transition
    /// whose brackets are the last thing in the scene, bar the app root's
    /// `PopClip`) and saves that whole pass.
    ///
    /// Unlike [`Self::pre`], this one carries a [`Segment::prefix`]: a
    /// trailing segment is a slice out of the MIDDLE of the frame, so it
    /// begins inside whatever the scene had pushed around it — the app root's
    /// clip around both pages (`nav/navigator.rs`), a scroll viewport's clip
    /// around a switcher (`scroll.rs`). Encoding the slice alone would draw
    /// that content unclipped and pop groups this pass never pushed. The
    /// prefix never contains an index inside a composited bracket's span:
    /// that body is a hole the trailing pass skips outright.
    pub trailing: Option<Segment>,
}

impl FramePlan {
    /// The plan for a frame with nothing composited: vello draws every
    /// command, exactly as it did before this cache existed.
    fn inline(commands: &[Command]) -> Self {
        Self {
            pre: Segment::whole(commands.len()),
            pre_draws: commands.iter().any(draws_pixels),
            layers: Vec::new(),
            holes: HashSet::new(),
            trailing: None,
        }
    }
}

/// One outermost `PushSnapshot`..`PopSnapshot` bracket located in a frame's
/// command list, with `body` the half-open index range of the commands
/// between them — so the `PushSnapshot` itself sits at `body.start - 1` and
/// the matching `PopSnapshot` at `body.end` (see [`bracket_span`]).
#[derive(Clone, Debug, PartialEq)]
struct Bracket {
    key: u64,
    rect: Rect,
    alpha: f32,
    scale: f64,
    transform: Affine,
    body: Range<usize>,
    /// The clips enclosing the bracket, intersected, in device pixels — see
    /// [`CompositeLayer::clip`].
    clip: Option<Rect>,
}

/// The bracket's own INCLUSIVE index span as a half-open range: its
/// `PushSnapshot` through its `PopSnapshot`. This is the span a composited
/// bracket contributes nothing outside of, so it is what the trailing-segment
/// decision subtracts.
fn bracket_span(body: &Range<usize>) -> Range<usize> {
    body.start.saturating_sub(1)..body.end + 1
}

/// Whether a command paints a pixel, as opposed to only shaping the state the
/// painting commands are interpreted in.
///
/// This is what makes a vello pass worth its cost: a clip/layer bracket or a
/// snapshot bracket emits no coverage of its own, so a segment holding nothing
/// else has nothing for the fine stage to do — and pays for it anyway. A
/// `ClearRect` counts: erasing pixels IS writing them, and its hoist to the
/// scene root is exactly a destination composite.
fn draws_pixels(command: &Command) -> bool {
    match command {
        Command::FillRect { .. }
        | Command::RoundedRect { .. }
        | Command::Line { .. }
        | Command::GlyphRun(_)
        | Command::Image { .. }
        | Command::BlurredRoundedRect { .. }
        | Command::ClearRect { .. }
        | Command::Path { .. }
        | Command::ShaderQuad { .. } => true,
        Command::PushClip { .. }
        | Command::PushClipRounded { .. }
        | Command::PopClip
        | Command::PushLayer { .. }
        | Command::PopLayer
        | Command::PushSnapshot { .. }
        | Command::PopSnapshot => false,
    }
}

/// The two-segment split of a frame's command list, given the frame's
/// `commands` and the [`bracket_span`]s of the composited brackets in scene
/// order: the pre [`Segment`], whether the pre segment DRAWS, and the trailing
/// [`Segment`] when there is one.
///
/// `pre` runs up to the first composited bracket's `PushSnapshot` and always
/// carries an empty prefix (it starts at the scene root); the trailing segment
/// starts one past that same bracket's `PopSnapshot`, runs to the end, and
/// carries the [`open_group_pushes`] its pass must re-establish. Both halves
/// are judged on drawing commands alone ([`draws_pixels`]): every later
/// composited bracket inside the trailing range is skipped as a hole, so the
/// range is worth a vello pass exactly when something OUTSIDE those brackets
/// paints in it — and the pre range's own pass is worth running exactly when
/// something paints there (see [`FramePlan::pre_draws`]).
///
/// The trailing range and its prefix are derived HERE, in one walk order, so
/// the renderer, the compositor's own frame helper and the tests all consume
/// one value rather than three re-derivations of the same decision that can
/// drift apart (a caller that forgot the prefix used to leave every host test
/// green).
fn frame_split(commands: &[Command], spans: &[Range<usize>]) -> (Segment, bool, Option<Segment>) {
    let len = commands.len();
    let Some(first) = spans.first() else {
        return (Segment::whole(len), commands.iter().any(draws_pixels), None);
    };
    let pre = 0..first.start.min(len);
    let pre_draws = commands[pre.clone()].iter().any(draws_pixels);
    let start = first.end.min(len);
    let draws = (start..len).any(|index| {
        draws_pixels(&commands[index]) && !spans[1..].iter().any(|span| span.contains(&index))
    });
    let trailing = draws.then(|| {
        // The depth-0 invariant [`open_group_pushes`] is written against:
        // `start` is one past an OUTERMOST composited bracket's
        // `PopSnapshot`, so no snapshot bracket — and therefore no MISS'd
        // bracket's emulated alpha layer — can be open here.
        debug_assert_eq!(
            snapshot_depth_before(commands, start),
            0,
            "a frame split must not fall inside a snapshot bracket"
        );
        Segment {
            range: start..len,
            prefix: open_group_pushes(commands, start, spans),
        }
    });
    (
        Segment {
            range: pre,
            prefix: Vec::new(),
        },
        pre_draws,
        trailing,
    )
}

/// The `PushSnapshot`/`PopSnapshot` nesting depth immediately BEFORE `index`:
/// pushes minus pops over `commands[..index]`, saturating at zero so an
/// unbalanced pop reads as depth 0 — the same tolerance the encode walk and
/// [`outermost_brackets`] show one.
///
/// Exists to assert the invariant [`open_group_pushes`] relies on, so a future
/// split rule that could fall inside a bracket trips a debug assertion rather
/// than silently mis-modelling the emulated layer a MISS'd bracket pushes.
fn snapshot_depth_before(commands: &[Command], index: usize) -> usize {
    commands
        .iter()
        .take(index)
        .fold(0usize, |depth, command| match command {
            Command::PushSnapshot { .. } => depth + 1,
            Command::PopSnapshot => depth.saturating_sub(1),
            _ => depth,
        })
}

/// The whole per-frame decision for one cacheable bracket, computed without
/// touching the GPU: what to rasterize, at what size, under what root, and
/// whether the existing entry already holds exactly that.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    key: u64,
    body: Range<usize>,
    /// Transform the body's commands are rasterized under (see the module's
    /// coordinate-mapping section).
    root: Affine,
    width: u32,
    height: u32,
    fingerprint: u64,
    /// `true` when the cached entry's size and fingerprint already match, so
    /// this frame performs no GPU work for the bracket.
    reuse: bool,
    /// The bracket's placement this frame, carried through so a successful
    /// plan becomes a [`CompositeLayer`] without re-scanning the scene. None
    /// of the three participates in the reuse decision — they are exactly the
    /// per-frame presentation the cache exists to animate for free.
    rect: Rect,
    alpha: f32,
    scale: f64,
    transform: Affine,
    /// The enclosing clips' intersected device-space bbox, carried through to
    /// the layer — see [`CompositeLayer::clip`]. Like the three above, no part
    /// of the reuse decision: clipping happens at composite time.
    clip: Option<Rect>,
}

/// The per-surface snapshot cache. Crate-private: no `wgpu`/`vello` type
/// leaves `frust-render`.
pub(crate) struct SnapshotCache {
    entries: HashMap<u64, Entry>,
    /// Monotonic frame counter — the clock `last_used` ages against. Advanced
    /// once per enabled [`Self::prepare`].
    frame: u64,
    /// Reused encoding buffer for the body being rasterized, reset per render
    /// exactly like the per-frame main scene.
    scratch: vello::Scene,
    /// Whether snapshot layers are on for this surface, resolved once by the
    /// caller from `FRUST_NO_SNAPSHOT_LAYERS`
    /// ([`crate::context::snapshot_layers_disabled`]) and held here so tests
    /// are not hostage to a process-global.
    enabled: bool,
}

/// `M`'s linear part with the translation zeroed: the page's device scale
/// (plus any rotation/skew), which is what a body's rasterization resolution
/// must follow. Dropping the translation is what lets a bracket slide across
/// the surface without invalidating its texture.
fn raster_transform(transform: Affine) -> Affine {
    let c = transform.as_coeffs();
    Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
}

/// Device pixel size for a body, rounded OUTWARD so no part of the body falls
/// off the texture's last pixel, then clamped to vello's atlas ceiling and the
/// adapter's own limit by the same [`clamp_size`] policy the shader pre-pass
/// uses (which also floors a degenerate `0` to `1`).
///
/// For an axis-aligned `raster` — every surface frust ships on, where `raster`
/// is a pure device scale — the extent is the rect's OWN extent times that
/// scale, computed before any translation and therefore independent of where
/// the rect sits. Transforming the corners instead makes the size a function
/// of position: a 1080-wide page measured 1080 px at `x0 = 0.0` but 1081 px at
/// `x0 = -6.25` (device-measured), and since the size is half the reuse
/// decision, the page re-rasterized every time its fractional slide crossed a
/// pixel column — the whole cache defeated by a slide. [`SIZE_EPSILON`]
/// absorbs the float noise of the one remaining multiply so a product a hair
/// over a whole pixel does not round a column up either.
///
/// A rotated or skewed `raster` has no such axis-wise extent and keeps the
/// outward bounding-box rule; it is not a placement frust's shells produce
/// today.
fn snapshot_size(raster: Affine, rect: Rect, adapter_max: u32) -> (u32, u32) {
    let coefficients = raster.as_coeffs();
    let axis_aligned = coefficients[1].abs() <= AXIS_ALIGNED_EPSILON
        && coefficients[2].abs() <= AXIS_ALIGNED_EPSILON;
    let (width, height) = if axis_aligned {
        (
            rect.width() * coefficients[0].abs() - SIZE_EPSILON,
            rect.height() * coefficients[3].abs() - SIZE_EPSILON,
        )
    } else {
        let bbox = raster.transform_rect_bbox(rect);
        (bbox.width(), bbox.height())
    };
    let to_u32 = |v: f64| {
        let v = v.ceil();
        if v.is_finite() && v > 0.0 {
            v as u32
        } else {
            0
        }
    };
    clamp_size((to_u32(width), to_u32(height)), adapter_max)
}

/// The rasterization root for a body (see the module's coordinate-mapping
/// section). `None` when `rect` is degenerate or `transform` is not
/// invertible — neither has a mapping onto a texture, so the bracket keeps the
/// inline path.
fn raster_root(transform: Affine, rect: Rect, width: u32, height: u32) -> Option<Affine> {
    if !(rect.width() > 0.0 && rect.height() > 0.0) || !rect.is_finite() {
        return None;
    }
    let determinant = transform.determinant();
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let scale_x = f64::from(width) / rect.width();
    let scale_y = f64::from(height) / rect.height();
    Some(
        Affine::scale_non_uniform(scale_x, scale_y)
            * Affine::translate((-rect.x0, -rect.y0))
            * transform.inverse(),
    )
}

/// An entry is only valid for pixels rasterized at the same size, under the
/// same device scale, from the same content — all three folded into this one
/// hash. The content half is hashed relative to `base = transform *
/// Affine::translate(rect.origin)` (`Scene::fingerprint_range`) — `rect`'s own
/// top-left corner in the bracket's space, the same origin [`raster_root`]
/// maps onto texture pixel `(0, 0)` — rather than relative to `transform`
/// alone. That is what keeps a body that merely slides/fades/scales as a
/// whole on the same fingerprint (the bracket's own transform sliding, `rect`
/// unchanged), AND what keeps a body whose absolute geometry slides instead
/// (a widget's origin baked directly into its paint commands, `transform`
/// unchanged) on the same fingerprint once the bracket's own `rect` slides
/// with it — see the module docs and `frust_scene::fingerprint`'s.
fn body_fingerprint(
    scene: &Scene,
    body: Range<usize>,
    rect: Rect,
    transform: Affine,
    width: u32,
    height: u32,
    raster: Affine,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    for coefficient in raster.as_coeffs() {
        ((coefficient * RASTER_QUANT).round() as i64).hash(&mut hasher);
    }
    let base = transform * Affine::translate((rect.x0, rect.y0));
    scene.fingerprint_range(body, base) ^ hasher.finish()
}

/// The reuse decision: a cached entry is re-used only when its size AND
/// fingerprint match this frame's exactly. Deliberately an equality test, not
/// a tolerance — the fingerprint is already quantized upstream.
fn reuses_entry(existing: Option<(u32, u32, u64)>, wanted: (u32, u32, u64)) -> bool {
    existing == Some(wanted)
}

/// One clip or layer open around a bracket, as [`outermost_brackets`] tracks
/// it: the bbox it confines its content to, in DEVICE pixels, and the opacity
/// it composites that content at.
///
/// A clip has no opacity of its own and carries `1.0`; a `PushLayer` carries
/// its own `alpha`, which is what disqualifies a bracket from compositing
/// (see [`enclosed_by_translucent_layer`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Enclosing {
    bbox: Rect,
    alpha: f32,
}

/// The clips and layers enclosing a bracket, their bboxes intersected into one
/// device-space rect — `None` for a bracket under none of either. An empty
/// intersection is returned as it falls out (a backwards rect), leaving "this
/// quad covers nothing" for the compositor to decide rather than conflating it
/// with "no clip at all".
///
/// A `PushLayer` bounds its content to its own rect exactly as a clip does, so
/// it contributes its bbox here too; only its OPACITY is beyond what a scissor
/// can express.
fn enclosing_clip(groups: &[Enclosing]) -> Option<Rect> {
    groups
        .iter()
        .map(|group| group.bbox)
        .reduce(|outer, inner| outer.intersect(inner))
}

/// Whether any enclosing group composites its content at less than full
/// opacity — an enclosing `PushLayer` with `alpha < 1.0`.
///
/// Such a bracket must NOT be composited: the quad pass has one alpha per
/// quad, already spent on the bracket's own presentation `alpha`, and folding
/// the layer's in would still be wrong (a layer composites its whole content
/// once, after it is drawn, which is not the same as fading each of its parts
/// separately). The bracket yields no plan and lowers inline instead, where
/// `convert.rs` pushes the layer around it exactly as recorded.
fn enclosed_by_translucent_layer(groups: &[Enclosing]) -> bool {
    groups.iter().any(|group| group.alpha < 1.0)
}

/// The command indices of the clip/layer pushes still OPEN at `split`, in
/// recording order — a trailing [`Segment`]'s [`Segment::prefix`].
///
/// Mirrors `convert.rs`'s own group stack: `PushClip`/`PushClipRounded`/
/// `PushLayer` open a group and either pop closes the innermost one, so an
/// unbalanced `PopClip` pops nothing, the same tolerance the encode walk
/// shows it.
///
/// Commands inside a composited bracket's `spans` are skipped: that body never
/// reaches the trailing pass (it is a hole), so a group opened and closed
/// inside it is not part of the state the trailing segment starts in.
///
/// ## The one group `convert.rs` pushes that this deliberately does not model
///
/// The encode walk pushes a `Group::Layer` of its own for a MISS'd
/// `PushSnapshot` whose `alpha < 1.0` (its inline emulation), and this mirror
/// does not. That is correct ONLY because a split never falls inside a
/// bracket: [`frame_split`] starts the trailing segment one past an
/// OUTERMOST composited bracket's `PopSnapshot`, where the snapshot depth is
/// therefore 0 ([`snapshot_depth_before`], debug-asserted there), so no
/// bracket — MISS'd or otherwise — is open at `split` and no emulated layer
/// can be part of the state the segment starts in. A split rule that could
/// land inside a bracket would have to model it here.
fn open_group_pushes(commands: &[Command], split: usize, spans: &[Range<usize>]) -> Vec<usize> {
    let mut open: Vec<usize> = Vec::new();
    for (index, command) in commands.iter().enumerate().take(split) {
        if spans.iter().any(|span| span.contains(&index)) {
            continue;
        }
        match command {
            Command::PushClip { .. }
            | Command::PushClipRounded { .. }
            | Command::PushLayer { .. } => open.push(index),
            Command::PopClip | Command::PopLayer => {
                open.pop();
            }
            _ => {}
        }
    }
    open
}

/// Every OUTERMOST balanced bracket in `commands` that may be composited, in
/// recording order, each carrying the clips and layers enclosing it.
///
/// Nested brackets are skipped entirely (the command contract makes only the
/// outermost one binding), an unbalanced `PopSnapshot` is ignored the way the
/// encode walk ignores it, and a bracket left open at the end of the list has
/// no bounded body so it yields nothing — that bracket keeps the inline path.
///
/// The clip/layer stack is tracked across the whole list, in device space
/// (`transform.transform_rect_bbox(rect)`), because a composited bracket's
/// pixels leave vello and nothing else would re-apply what they were recorded
/// under. A rounded clip contributes its rect: the compositor's scissor cannot
/// round corners. A `PushLayer` contributes its rect the same way, and either
/// pop closes the innermost group — one stack, exactly as `convert.rs`'s
/// encode walk keeps one. An unbalanced `PopClip` pops nothing, the same
/// tolerance that walk shows it.
///
/// A bracket enclosed by a layer with `alpha < 1.0` is DROPPED here rather
/// than returned: the quad pass cannot apply that opacity
/// ([`enclosed_by_translucent_layer`]), so the bracket must lower inline, and
/// dropping it is exactly that — no plan, no layer, no hole.
fn outermost_brackets(commands: &[Command]) -> Vec<Bracket> {
    let mut brackets = Vec::new();
    let mut depth: usize = 0;
    let mut groups: Vec<Enclosing> = Vec::new();
    let mut open: Option<(usize, Bracket)> = None;
    for (index, command) in commands.iter().enumerate() {
        match command {
            Command::PushClip { rect, transform }
            | Command::PushClipRounded {
                rect, transform, ..
            } => groups.push(Enclosing {
                bbox: transform.transform_rect_bbox(*rect),
                alpha: 1.0,
            }),
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => groups.push(Enclosing {
                bbox: transform.transform_rect_bbox(*rect),
                alpha: *alpha,
            }),
            Command::PopClip | Command::PopLayer => {
                groups.pop();
            }
            Command::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
                transform,
            } => {
                if depth == 0 && !enclosed_by_translucent_layer(&groups) {
                    open = Some((
                        index,
                        Bracket {
                            key: *key,
                            rect: *rect,
                            alpha: *alpha,
                            scale: *scale,
                            transform: *transform,
                            // Closed below, once the matching pop is found.
                            body: 0..0,
                            clip: enclosing_clip(&groups),
                        },
                    ));
                }
                depth += 1;
            }
            Command::PopSnapshot => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0
                    && let Some((push, bracket)) = open.take()
                {
                    brackets.push(Bracket {
                        body: push + 1..index,
                        ..bracket
                    });
                }
            }
            _ => {}
        }
    }
    brackets
}

/// Whether a body may be rasterized at all (see the module's "what is never
/// cached" section): a `ClearRect` must reach the surface, and a `ShaderQuad`
/// has no override map on the command-slice encode path.
fn is_cacheable_body(body: &[Command]) -> bool {
    !body.iter().any(|command| {
        matches!(
            command,
            Command::ClearRect { .. } | Command::ShaderQuad { .. }
        )
    })
}

/// Whether a `ClearRect` lands in the frame's TRAILING segment: at or after
/// the first composited bracket's own [`bracket_span`] end, and outside every
/// composited bracket's span.
///
/// A punch there cannot be composited correctly, so the whole frame gives up
/// on compositing rather than paint it wrong. The trailing segment renders
/// into the compositor's TRANSPARENT scratch texture, and `convert.rs` hoists
/// a `ClearRect` to that pass's own root — so the punch erases scratch pixels
/// and never reaches the swapchain, leaving the composited page underneath
/// fully visible exactly where a hosted native view was supposed to show
/// through. The measured shape is a camera page (bracketed, but uncacheable
/// via [`is_cacheable_body`], so it falls into the trailing segment) pushed
/// over an already-composited page on a translucent Android surface.
///
/// A punch in the PRE segment is fine and is deliberately not reported: that
/// pass writes the real target, `pre_draws` is true for it, and a quad drawn
/// over it afterwards matches the inline z-order.
///
/// Reported from [`SnapshotCache::plan`], which is GPU-free — the frame falls
/// back to the ordinary single-pass path before anything is rasterized.
fn punches_after_first_bracket(commands: &[Command], spans: &[Range<usize>]) -> bool {
    let Some(first) = spans.first() else {
        return false;
    };
    commands
        .iter()
        .enumerate()
        .skip(first.end)
        .any(|(index, command)| {
            matches!(command, Command::ClearRect { .. })
                && !spans.iter().any(|span| span.contains(&index))
        })
}

/// Keys appearing more than once in one frame's brackets. The cache is keyed
/// by `key` alone, so an ambiguous key would draw one bracket's pixels in the
/// other's place; both take the inline path instead.
fn ambiguous_keys(keys: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut seen: HashMap<u64, u32> = HashMap::new();
    for key in keys {
        *seen.entry(key).or_default() += 1;
    }
    seen.into_iter()
        .filter(|&(_, count)| count > 1)
        .map(|(key, _)| key)
        .collect()
}

/// Keys whose entry has gone unused for longer than `max_unused` frames.
/// `entries` yields `(key, last_used)`; the boundary is inclusive of
/// `frame - max_unused` (that entry survives).
fn evictable_keys(
    entries: impl Iterator<Item = (u64, u64)>,
    frame: u64,
    max_unused: u64,
) -> Vec<u64> {
    entries
        .filter(|&(_, last_used)| last_used + max_unused < frame)
        .map(|(key, _)| key)
        .collect()
}

impl SnapshotCache {
    /// A cache for one surface. `enabled` is the resolved
    /// `FRUST_NO_SNAPSHOT_LAYERS` kill switch, inverted — `false` makes every
    /// call a no-op and every bracket lower inline.
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            scratch: vello::Scene::new(),
            enabled,
        }
    }

    /// This frame's [`FramePlan`]: rasterize every outermost bracket whose
    /// body changed, re-use the rest untouched, age out what the scene stopped
    /// drawing, and report the composite layers, the holes and the segment
    /// split the frame is then rendered by (see the module header).
    ///
    /// Renders happen through `renderer` — the only reason it is still a
    /// parameter, now that nothing here registers a texture with vello — and
    /// it submits each of them itself; wgpu serializes queue submissions, so
    /// both the caller's own `render_to_texture` and the compositor's sampling
    /// see complete textures as long as they run after this. Same ordering
    /// argument the shader pre-pass makes.
    ///
    /// `surface` is the swapchain's pixel size, the yardstick each bracket's
    /// own texture area is budgeted against
    /// ([`SNAPSHOT_AREA_BUDGET_FACTOR`]); a zero-area surface budgets nothing
    /// and composites nothing.
    ///
    /// A `false` `enabled` flag short-circuits before any GPU work, any
    /// eviction, and the frame clock itself: the switch means "the pre-pass is
    /// off", not "age everything out". Never panics — a failed rasterization
    /// drops the entry and omits it from the plan, so the bracket lowers
    /// inline exactly as it would have with no cache at all.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut vello::Renderer,
        scene: &Scene,
        adapter_max: u32,
        surface: (u32, u32),
    ) -> FramePlan {
        if !self.enabled {
            return FramePlan::inline(scene.commands());
        }
        self.frame += 1;
        let mut layers = Vec::new();
        let mut spans = Vec::new();
        for plan in self.plan(scene, adapter_max, surface) {
            if !plan.reuse && !self.render(device, queue, renderer, scene, &plan) {
                continue;
            }
            let frame = self.frame;
            let Some(entry) = self.entries.get_mut(&plan.key) else {
                continue;
            };
            entry.last_used = frame;
            layers.push(CompositeLayer {
                key: plan.key,
                rect: plan.rect,
                alpha: plan.alpha,
                scale: plan.scale,
                transform: plan.transform,
                width: entry.width,
                height: entry.height,
                clip: plan.clip,
                view: entry.view.clone(),
            });
            spans.push(bracket_span(&plan.body));
        }
        self.evict();
        let holes = layers.iter().map(|layer| layer.key).collect();
        // One walk decides both segments AND the trailing prefix, so the
        // renderer cannot forward a range without the groups it starts inside.
        let (pre, pre_draws, trailing) = frame_split(scene.commands(), &spans);
        FramePlan {
            pre,
            pre_draws,
            layers,
            holes,
            trailing,
        }
    }

    /// The GPU-free half of [`Self::prepare`]: what this frame would rasterize
    /// and what it would re-use. Split out so the scan, the size/root
    /// derivation, the fingerprint decision, the budgets and the kill switch
    /// are all assertable without a device.
    ///
    /// `surface` is the swapchain's pixel size; a bracket whose texture would
    /// cover more than [`SNAPSHOT_AREA_BUDGET_FACTOR`] times that area is
    /// skipped and lowers inline. An empty result — every bracket skipped, or
    /// the whole frame refused by [`punches_after_first_bracket`] — is exactly
    /// the kill switch's plan, so the caller's ordinary single-pass path
    /// carries it with no special case.
    fn plan(&self, scene: &Scene, adapter_max: u32, surface: (u32, u32)) -> Vec<Plan> {
        if !self.enabled {
            return Vec::new();
        }
        let commands = scene.commands();
        let brackets = outermost_brackets(commands);
        let ambiguous = ambiguous_keys(brackets.iter().map(|bracket| bracket.key));
        // Guarded rather than divided into, so a zero-area surface (a
        // minimized window, a surface mid-resize) budgets nothing and
        // composites nothing instead of dividing by zero: every texture is at
        // least 1x1 after `clamp_size`, so `area > 0` refuses them all.
        let budget = SNAPSHOT_AREA_BUDGET_FACTOR * u64::from(surface.0) * u64::from(surface.1);
        let mut plans = Vec::new();
        for bracket in brackets {
            if ambiguous.contains(&bracket.key)
                || !is_cacheable_body(&commands[bracket.body.clone()])
            {
                continue;
            }
            let raster = raster_transform(bracket.transform);
            let (width, height) = snapshot_size(raster, bracket.rect, adapter_max);
            if u64::from(width) * u64::from(height) > budget {
                continue;
            }
            let Some(root) = raster_root(bracket.transform, bracket.rect, width, height) else {
                continue;
            };
            let fingerprint = body_fingerprint(
                scene,
                bracket.body.clone(),
                bracket.rect,
                bracket.transform,
                width,
                height,
                raster,
            );
            let reuse = reuses_entry(
                self.entries
                    .get(&bracket.key)
                    .map(|entry| (entry.width, entry.height, entry.fingerprint)),
                (width, height, fingerprint),
            );
            plans.push(Plan {
                key: bracket.key,
                body: bracket.body,
                root,
                width,
                height,
                fingerprint,
                reuse,
                rect: bracket.rect,
                alpha: bracket.alpha,
                scale: bracket.scale,
                transform: bracket.transform,
                clip: bracket.clip,
            });
        }
        // A hole punch recorded after the first composited bracket cannot
        // survive the trailing pass's transparent scratch, so the WHOLE frame
        // lowers inline rather than seal the hole a hosted native view shows
        // through (see [`punches_after_first_bracket`]). Decided here, before
        // any rasterization: the caller still ticks its frame clock and
        // evicts, it just composites nothing.
        let spans: Vec<Range<usize>> = plans.iter().map(|plan| bracket_span(&plan.body)).collect();
        if punches_after_first_bracket(commands, &spans) {
            return Vec::new();
        }
        plans
    }

    /// Rasterize one planned body into its (possibly freshly created) texture.
    /// Returns whether the entry is now present and holding this plan's
    /// pixels; `false` means the bracket must fall back to the inline path
    /// this frame.
    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut vello::Renderer,
        scene: &Scene,
        plan: &Plan,
    ) -> bool {
        let frame = self.frame;
        let Self {
            entries, scratch, ..
        } = self;

        // A size change cannot re-use the texture: drop the old one first, so
        // the fresh entry below allocates at this frame's size.
        let resized = entries
            .get(&plan.key)
            .is_some_and(|entry| entry.width != plan.width || entry.height != plan.height);
        if resized {
            entries.remove(&plan.key);
        }
        entries.entry(plan.key).or_insert_with(|| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust snapshot layer"),
                size: wgpu::Extent3d {
                    width: plan.width,
                    height: plan.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                // vello renders into it by compute (STORAGE_BINDING) and the
                // compositor samples it (TEXTURE_BINDING). Nothing copies it,
                // so there is no COPY_SRC: the pixels are read where they are
                // written, never staged through an atlas.
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            // The view is the only handle kept: it refcounts the texture, and
            // both the re-render and the composite go through one.
            Entry {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                width: plan.width,
                height: plan.height,
                // Overwritten by the render below; a fresh entry is only
                // ever reached with a render immediately following, and a
                // failed one is dropped rather than left holding this.
                fingerprint: 0,
                last_used: frame,
                renders: 0,
            }
        });

        scratch.reset();
        encode_commands_into(&scene.commands()[plan.body.clone()], plan.root, scratch);

        let params = vello::RenderParams {
            // Transparent, not the frame's base color: everything the body does
            // not paint must composite through to whatever is behind the
            // bracket on the surface.
            base_color: peniko::color::palette::css::TRANSPARENT,
            width: plan.width,
            height: plan.height,
            antialiasing_method: vello::AaConfig::Area,
        };
        let rendered = match entries.get(&plan.key) {
            Some(entry) => renderer.render_to_texture(device, queue, scratch, &entry.view, &params),
            None => return false,
        };
        if let Err(error) = rendered {
            log::warn!("frust-render: snapshot rasterization failed, painting inline: {error}");
            entries.remove(&plan.key);
            return false;
        }

        let Some(entry) = entries.get_mut(&plan.key) else {
            return false;
        };
        entry.fingerprint = plan.fingerprint;
        entry.renders = entry.renders.saturating_add(1);
        true
    }

    /// Drop every entry the scene has stopped drawing for more than
    /// [`MAX_UNUSED_FRAMES`] frames. Dropping the [`Entry`] releases the last
    /// handle on its texture, so eviction is the whole release.
    fn evict(&mut self) {
        let frame = self.frame;
        for key in evictable_keys(
            self.entries
                .iter()
                .map(|(&key, entry)| (key, entry.last_used)),
            frame,
            MAX_UNUSED_FRAMES,
        ) {
            self.entries.remove(&key);
        }
    }

    /// Drop every entry, releasing every cached texture.
    ///
    /// Called when the surface goes away. Takes no renderer: these textures
    /// are never handed to vello, so there is nothing to surrender before it
    /// dies — dropping the last view handle IS the release.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// How many times the entry for `key` has been rasterized — the
    /// steady-state probe (an animating bracket must stay at 1).
    #[cfg(test)]
    fn renders(&self, key: u64) -> Option<u32> {
        self.entries.get(&key).map(|entry| entry.renders)
    }

    /// How many entries are live, for eviction assertions.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::SceneBuilder;
    use kurbo::Point;
    use peniko::Brush;
    use peniko::color::palette::css::{BLUE, RED};

    const KEY: u64 = 7;

    /// The surface size every plan/split assertion here is budgeted against —
    /// large enough that no bracket in this module is refused for area. The
    /// budget itself is pinned by
    /// [`plan_skips_a_bracket_over_the_area_budget`].
    const SURFACE: (u32, u32) = (256, 256);

    /// A scene holding one bracket: `body` filled inside a `rect` bracket
    /// recorded under `transform`.
    fn bracket_scene(transform: Affine, rect: Rect, body: Rect, brush: Brush) -> Scene {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_transform(transform);
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.fill_rect(body, brush);
            builder.pop_snapshot();
            builder.pop_transform();
        }
        scene
    }

    fn push(key: u64, rect: Rect, transform: Affine) -> Command {
        Command::PushSnapshot {
            key,
            rect,
            alpha: 1.0,
            scale: 1.0,
            transform,
        }
    }

    /// The spans every eligible bracket in `scene` would contribute if it hit
    /// the cache — the input both halves of [`SnapshotCache::prepare`]'s
    /// split arithmetic take, minus the GPU work that decides which plans
    /// actually became layers.
    fn spans_of(scene: &Scene) -> Vec<Range<usize>> {
        SnapshotCache::new(true)
            .plan(scene, 8192, SURFACE)
            .iter()
            .map(|plan| bracket_span(&plan.body))
            .collect()
    }

    /// The split a frame would take if every eligible bracket in `scene` hit
    /// the cache.
    fn split_of(scene: &Scene) -> (Segment, bool, Option<Segment>) {
        frame_split(scene.commands(), &spans_of(scene))
    }

    /// The trailing segment's prefix for that same frame — a VIEW over
    /// [`frame_split`]'s own return value rather than a second derivation, so
    /// a test asserts exactly what the render path would hand
    /// `convert::encode_range_with_overrides`.
    fn prefix_of(scene: &Scene) -> Vec<usize> {
        split_of(scene)
            .2
            .map(|segment| segment.prefix)
            .unwrap_or_default()
    }

    /// A segment starting at the scene root: a range under no re-opened
    /// groups, which is every pre segment and every trailing segment of an
    /// unclipped scene.
    fn seg(range: Range<usize>) -> Segment {
        Segment {
            range,
            prefix: Vec::new(),
        }
    }

    /// The composited keys `plan` would report for `scene`, in scene order.
    fn planned_keys(scene: &Scene) -> Vec<u64> {
        SnapshotCache::new(true)
            .plan(scene, 8192, SURFACE)
            .iter()
            .map(|plan| plan.key)
            .collect()
    }

    fn clip(rect: Rect, transform: Affine) -> Command {
        Command::PushClip { rect, transform }
    }

    fn fill(rect: Rect) -> Command {
        Command::FillRect {
            rect,
            brush: Brush::Solid(RED),
            transform: Affine::IDENTITY,
        }
    }

    #[test]
    fn outermost_brackets_reports_the_outer_bracket_only() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY),
            push(2, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopSnapshot,
        ];
        let brackets = outermost_brackets(&commands);
        assert_eq!(brackets.len(), 1);
        assert_eq!(brackets[0].key, 1);
        // The body spans everything between the outer pair, nested bracket
        // commands included — they lower inline inside the rasterization.
        assert_eq!(brackets[0].body, 1..4);
    }

    #[test]
    fn outermost_brackets_finds_each_sibling_bracket() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            push(2, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
        ];
        let keys: Vec<u64> = outermost_brackets(&commands)
            .iter()
            .map(|bracket| bracket.key)
            .collect();
        assert_eq!(keys, vec![1, 2]);
    }

    #[test]
    fn outermost_brackets_ignores_an_unbalanced_pop() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        // A stray pop before any push, and one after the bracket closed.
        let commands = vec![
            Command::PopSnapshot,
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopSnapshot,
        ];
        let brackets = outermost_brackets(&commands);
        assert_eq!(brackets.len(), 1);
        assert_eq!(brackets[0].body, 2..3);
    }

    #[test]
    fn outermost_brackets_drops_a_bracket_left_open() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![push(1, rect, Affine::IDENTITY), fill(rect)];
        assert!(outermost_brackets(&commands).is_empty());
    }

    #[test]
    fn outermost_brackets_intersects_the_enclosing_clips_in_device_space() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            clip(Rect::new(0.0, 0.0, 100.0, 100.0), Affine::IDENTITY),
            clip(
                Rect::new(20.0, 0.0, 60.0, 40.0),
                Affine::translate((5.0, 5.0)),
            ),
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopClip,
            Command::PopClip,
        ];
        assert_eq!(
            outermost_brackets(&commands)[0].clip,
            Some(Rect::new(25.0, 5.0, 65.0, 45.0)),
            "each clip lands in device space and the two intersect"
        );
    }

    #[test]
    fn outermost_brackets_records_no_clip_for_an_unclipped_bracket() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        // A clip that opened AND closed before the bracket encloses nothing.
        let commands = vec![
            clip(Rect::new(0.0, 0.0, 4.0, 4.0), Affine::IDENTITY),
            Command::PopClip,
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
        ];
        assert_eq!(outermost_brackets(&commands)[0].clip, None);
    }

    #[test]
    fn outermost_brackets_takes_a_rotated_or_rounded_clips_bounding_rect() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let turned = Affine::rotate(std::f64::consts::FRAC_PI_4);
        let clipped = Rect::new(-10.0, -10.0, 10.0, 10.0);
        let commands = vec![
            Command::PushClipRounded {
                rect: clipped,
                radii: frust_scene::CornerRadii::uniform(4.0),
                transform: turned,
            },
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopClip,
        ];
        assert_eq!(
            outermost_brackets(&commands)[0].clip,
            Some(turned.transform_rect_bbox(clipped)),
            "a rounded clip contributes its rect; a rotated one its bbox"
        );
    }

    #[test]
    fn outermost_brackets_ignores_a_clip_opened_inside_the_body() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY),
            clip(Rect::new(1.0, 1.0, 2.0, 2.0), Affine::IDENTITY),
            fill(rect),
            Command::PopClip,
            Command::PopSnapshot,
            push(2, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
        ];
        let brackets = outermost_brackets(&commands);
        assert_eq!(
            (brackets[0].clip, brackets[1].clip),
            (None, None),
            "a body's own clip is rasterized into the texture, not around it"
        );
    }

    /// One enclosing clip (no opacity of its own) at `rect`.
    fn clip_group(rect: Rect) -> Enclosing {
        Enclosing {
            bbox: rect,
            alpha: 1.0,
        }
    }

    /// A `PushLayer` bounds its content exactly as a clip does, so its rect
    /// joins the enclosing intersection the compositor scissors by — a page
    /// inside a full-opacity layer must not paint outside that layer's rect.
    #[test]
    fn outermost_brackets_intersects_an_enclosing_layer_into_the_clip() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let clip = Rect::new(0.0, 0.0, 100.0, 20.0);
        let layer = Rect::new(0.0, 0.0, 50.0, 50.0);
        let commands = vec![
            Command::PushClip {
                rect: clip,
                transform: Affine::IDENTITY,
            },
            Command::PushLayer {
                rect: layer,
                alpha: 1.0,
                transform: Affine::IDENTITY,
            },
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopLayer,
            Command::PopClip,
        ];
        let brackets = outermost_brackets(&commands);
        assert_eq!(brackets.len(), 1);
        assert_eq!(
            brackets[0].clip,
            Some(Rect::new(0.0, 0.0, 50.0, 20.0)),
            "the layer's rect intersects with the clip's"
        );
    }

    /// A bracket inside a TRANSLUCENT layer is not composited at all: the quad
    /// pass has one alpha per quad and it is already the bracket's own, so the
    /// bracket lowers inline (no bracket, no plan, no hole, no split) where
    /// `convert.rs` pushes the layer around it as recorded.
    #[test]
    fn outermost_brackets_drops_a_bracket_inside_a_translucent_layer() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let layer = Rect::new(0.0, 0.0, 50.0, 50.0);
        let commands = vec![
            Command::PushLayer {
                rect: layer,
                alpha: 0.5,
                transform: Affine::IDENTITY,
            },
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopLayer,
        ];
        assert!(
            outermost_brackets(&commands).is_empty(),
            "a bracket under a translucent layer must lower inline"
        );
        // The same bracket under a FULL-opacity layer still composites — the
        // disqualification is the opacity, not the layer.
        let opaque = vec![
            Command::PushLayer {
                rect: layer,
                alpha: 1.0,
                transform: Affine::IDENTITY,
            },
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopLayer,
        ];
        assert_eq!(outermost_brackets(&opaque).len(), 1);
    }

    /// The whole-frame consequence of the drop: no plan, and the frame is one
    /// ordinary vello pass over everything.
    #[test]
    fn a_bracket_inside_a_translucent_layer_yields_no_plan() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_layer(Rect::new(0.0, 0.0, 50.0, 50.0), 0.5);
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.pop_layer();
        }
        assert!(
            SnapshotCache::new(true)
                .plan(&scene, 8192, SURFACE)
                .is_empty()
        );
        assert_eq!(
            split_of(&scene),
            (seg(0..scene.commands().len()), true, None)
        );
    }

    /// The MEASURED trailing shape: the app root's clip is open where the
    /// trailing segment starts, so the plan names its push as the prefix that
    /// pass must re-establish.
    #[test]
    fn the_trailing_prefix_names_the_clip_the_segment_starts_inside() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_clip();
        }
        assert_eq!(
            split_of(&scene),
            (
                seg(0..1),
                false,
                Some(Segment {
                    range: 4..6,
                    // The app root's `PushClip`, carried by the segment
                    // itself rather than beside it.
                    prefix: vec![0],
                })
            )
        );
        assert_eq!(prefix_of(&scene), vec![0]);
    }

    /// No trailing segment, no prefix — the common page-transition frame
    /// carries an empty one rather than a stale list.
    #[test]
    fn a_frame_with_no_trailing_segment_has_no_prefix() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.pop_clip();
        }
        assert!(split_of(&scene).2.is_none());
        assert!(prefix_of(&scene).is_empty());
        let inline = FramePlan::inline(scene.commands());
        assert!(inline.trailing.is_none());
        assert!(
            inline.pre.prefix.is_empty(),
            "the pre segment starts at the scene root"
        );
    }

    #[test]
    fn open_group_pushes_reports_only_what_is_still_open() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            clip(rect, Affine::IDENTITY), // 0: still open at the split
            clip(rect, Affine::IDENTITY), // 1: closed at 2
            Command::PopClip,             // 2
            Command::PushLayer {
                // 3: still open at the split
                rect,
                alpha: 1.0,
                transform: Affine::IDENTITY,
            },
            fill(rect), // 4
        ];
        assert_eq!(open_group_pushes(&commands, 5, &[]), vec![0, 3]);
        // Either pop closes the innermost group, exactly as the encode walk's
        // single stack does; an unbalanced pop closes nothing.
        let unbalanced = vec![Command::PopClip, clip(rect, Affine::IDENTITY)];
        assert_eq!(open_group_pushes(&unbalanced, 2, &[]), vec![1]);
        let layer_popped_as_clip = vec![
            Command::PushLayer {
                rect,
                alpha: 1.0,
                transform: Affine::IDENTITY,
            },
            Command::PopClip,
        ];
        assert!(open_group_pushes(&layer_popped_as_clip, 2, &[]).is_empty());
    }

    /// A group opened inside a COMPOSITED bracket's body is not part of the
    /// trailing segment's starting state: that body is a hole the pass skips
    /// outright, so its pushes must never be re-established.
    #[test]
    fn open_group_pushes_skips_a_composited_brackets_own_body() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            clip(rect, Affine::IDENTITY),    // 0: the app root's clip
            push(1, rect, Affine::IDENTITY), // 1
            clip(rect, Affine::IDENTITY),    // 2: inside the holed body
            fill(rect),                      // 3
            Command::PopSnapshot,            // 4: the body was left unbalanced
            fill(rect),                      // 5
        ];
        let spans = [bracket_span(&(2..4))];
        assert_eq!(open_group_pushes(&commands, 5, &spans), vec![0]);
    }

    #[test]
    fn enclosing_clip_reports_an_empty_intersection_as_it_falls_out() {
        let disjoint = enclosing_clip(&[
            clip_group(Rect::new(0.0, 0.0, 10.0, 10.0)),
            clip_group(Rect::new(20.0, 20.0, 30.0, 30.0)),
        ])
        .expect("two clips are still clips");
        assert!(
            disjoint.width() <= 0.0 || disjoint.height() <= 0.0,
            "{disjoint:?} must not read as a coverable region"
        );
        assert_eq!(enclosing_clip(&[]), None);
    }

    #[test]
    fn raster_transform_keeps_the_linear_part_and_drops_the_translation() {
        let transform = Affine::translate((30.0, 40.0)) * Affine::scale(2.0);
        assert_eq!(raster_transform(transform), Affine::scale(2.0));
    }

    #[test]
    fn snapshot_size_follows_the_device_scale_and_rounds_outward() {
        let rect = Rect::new(0.0, 0.0, 20.5, 10.0);
        // 2x device scale: 41 x 20 exactly; the translation must not matter.
        let transform = Affine::translate((3.0, 5.0)) * Affine::scale(2.0);
        let raster = raster_transform(transform);
        assert_eq!(snapshot_size(raster, rect, 8192), (41, 20));

        // A fractional extent rounds outward rather than truncating.
        let odd = Rect::new(0.0, 0.0, 20.1, 10.0);
        assert_eq!(snapshot_size(raster, odd, 8192), (41, 20));
    }

    /// The bug this card fixes: a page's texture size must depend on its
    /// EXTENT, never on where it currently sits. Sliding the same 1080-px-wide
    /// page across fractional offsets used to flip the size between 1080 and
    /// 1081 as the transformed corners rounded differently, and since the size
    /// is half the reuse decision, every crossing re-rasterized the whole page
    /// mid-transition. Size and fingerprint must both hold still.
    #[test]
    fn snapshot_size_and_fingerprint_are_independent_of_the_rects_position() {
        // A Pixel 5a page: 1080 device px at the 2.75 device scale.
        let scale = 2.75;
        let raster = raster_transform(Affine::scale(scale));
        let (logical_width, logical_height) = (1080.0 / scale, 2400.0 / scale);
        let body = Rect::new(4.0, 4.0, 100.0, 200.0);

        let mut sizes = Vec::new();
        let mut fingerprints = Vec::new();
        for x0 in [0.0, 0.3, -6.25, 15.35, 0.0026] {
            let rect = Rect::new(x0, 0.0, x0 + logical_width, logical_height);
            // The body slides with the bracket it belongs to, which is what a
            // page transition does (see the module's fingerprint section).
            let slid = Rect::new(body.x0 + x0, body.y0, body.x1 + x0, body.y1);
            let scene = bracket_scene(Affine::scale(scale), rect, slid, Brush::Solid(RED));
            let size = snapshot_size(raster, rect, 8192);
            sizes.push(size);
            fingerprints.push(body_fingerprint(
                &scene,
                1..2,
                rect,
                Affine::scale(scale),
                size.0,
                size.1,
                raster,
            ));
        }
        assert_eq!(
            sizes,
            vec![(1080, 2400); 5],
            "a page's texture size must not follow its fractional offset"
        );
        assert_eq!(
            fingerprints,
            vec![fingerprints[0]; 5],
            "a page that only slides must keep its fingerprint, and so its texture"
        );
    }

    /// Rotation has no axis-wise extent, so it keeps the outward bounding-box
    /// rule (a quarter turn swaps the two axes). Written as the exact matrix
    /// rather than `Affine::rotate`, whose trig leaves the transformed extent
    /// a few ULPs over 20 — which the outward rule then rounds to 21, exactly
    /// the rounding the axis-aligned branch exists to stop paying.
    #[test]
    fn snapshot_size_falls_back_to_the_bounding_box_for_a_rotated_raster() {
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let quarter_turn = Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            snapshot_size(raster_transform(quarter_turn), rect, 8192),
            (20, 40)
        );
    }

    #[test]
    fn snapshot_size_clamps_to_the_adapter_limit_and_floors_degenerate_extents() {
        let huge = Rect::new(0.0, 0.0, 20_000.0, 20_000.0);
        assert_eq!(snapshot_size(Affine::IDENTITY, huge, 4096), (4096, 4096));
        let empty = Rect::new(5.0, 5.0, 5.0, 5.0);
        assert_eq!(snapshot_size(Affine::IDENTITY, empty, 8192), (1, 1));
    }

    #[test]
    fn raster_root_maps_the_bodys_local_rect_onto_the_whole_texture() {
        // The mapping contract, at the pure-transform level: composed with the
        // bracket's own transform (which every body command already carries),
        // the root must send `rect`'s local corners to the texture's corners.
        let transform = Affine::translate((4.0, 6.0)) * Affine::scale(2.0);
        let rect = Rect::new(1.0, 2.0, 21.0, 12.0);
        let raster = raster_transform(transform);
        let (width, height) = snapshot_size(raster, rect, 8192);
        assert_eq!((width, height), (40, 20));
        let root = raster_root(transform, rect, width, height).expect("invertible");

        let to_texture = root * transform;
        let origin = to_texture * Point::new(rect.x0, rect.y0);
        let far = to_texture * Point::new(rect.x1, rect.y1);
        assert!(
            (origin.x).abs() < 1e-9 && (origin.y).abs() < 1e-9,
            "{origin:?}"
        );
        assert!(
            (far.x - f64::from(width)).abs() < 1e-9 && (far.y - f64::from(height)).abs() < 1e-9,
            "{far:?}"
        );
    }

    #[test]
    fn raster_root_absorbs_a_pure_translation_of_the_bracket() {
        // Sliding the bracket must not change what lands in the texture: the
        // root moves with it, so the same local content maps to the same pixels.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let still = Affine::scale(2.0);
        let slid = Affine::translate((37.0, -12.0)) * Affine::scale(2.0);
        let a = raster_root(still, rect, 40, 20).expect("invertible") * still;
        let b = raster_root(slid, rect, 40, 20).expect("invertible") * slid;
        for (left, right) in a.as_coeffs().iter().zip(b.as_coeffs().iter()) {
            assert!((left - right).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn raster_root_is_none_for_a_degenerate_rect_or_singular_transform() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        assert!(raster_root(Affine::IDENTITY, Rect::new(1.0, 1.0, 1.0, 9.0), 4, 4).is_none());
        assert!(raster_root(Affine::scale(0.0), rect, 4, 4).is_none());
    }

    #[test]
    fn body_fingerprint_survives_a_bracket_that_only_slides() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let still = Affine::scale(2.0);
        let slid = Affine::translate((37.0, -12.0)) * Affine::scale(2.0);
        let a = bracket_scene(still, rect, body, Brush::Solid(RED));
        let b = bracket_scene(slid, rect, body, Brush::Solid(RED));
        assert_eq!(
            body_fingerprint(&a, 1..2, rect, still, 40, 20, raster_transform(still)),
            body_fingerprint(&b, 1..2, rect, slid, 40, 20, raster_transform(slid))
        );
    }

    /// The new mechanism this card adds: a body whose ABSOLUTE geometry
    /// slides by `(dx, dy)` — identity bracket transform throughout, the
    /// slide baked directly into the body's own paint commands the way a
    /// widget's origin gets baked in — fingerprints EQUAL to its unslid self
    /// once the bracket's own `rect` slides by the same `(dx, dy)` (the
    /// widget that owns the bracket is the thing sliding, so its own
    /// `PushSnapshot` rect moves with it). `raster_root` also maps both
    /// bodies onto the identical texture-pixel frame, confirming the two
    /// scenes really do describe the same rasterized pixels.
    #[test]
    fn body_fingerprint_survives_an_absolute_coordinate_slide() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let transform = Affine::IDENTITY;
        let (dx, dy) = (37.0, -12.0);
        let shift = |r: Rect| Rect::new(r.x0 + dx, r.y0 + dy, r.x1 + dx, r.y1 + dy);
        let shifted_rect = shift(rect);
        let shifted_body = shift(body);

        let a = bracket_scene(transform, rect, body, Brush::Solid(RED));
        let b = bracket_scene(transform, shifted_rect, shifted_body, Brush::Solid(RED));

        let raster = raster_transform(transform);
        assert_eq!(
            body_fingerprint(&a, 1..2, rect, transform, 40, 20, raster),
            body_fingerprint(&b, 1..2, shifted_rect, transform, 40, 20, raster),
        );

        let root_a = raster_root(transform, rect, 40, 20).expect("invertible");
        let root_b = raster_root(transform, shifted_rect, 40, 20).expect("invertible");
        let corner_a = root_a * Point::new(body.x0, body.y0);
        let corner_b = root_b * Point::new(shifted_body.x0, shifted_body.y0);
        assert!(
            (corner_a.x - corner_b.x).abs() < 1e-9 && (corner_a.y - corner_b.y).abs() < 1e-9,
            "{corner_a:?} vs {corner_b:?}"
        );
    }

    #[test]
    fn body_fingerprint_changes_when_only_the_bracket_rect_shifts() {
        // The body's own geometry stays put while the bracket's `rect`
        // moves — an ordinary content change (or a bracket resize), not a
        // whole-bracket slide, so the fingerprint must differ.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let transform = Affine::IDENTITY;
        let shifted_rect = Rect::new(10.0, 0.0, 30.0, 10.0);

        let a = bracket_scene(transform, rect, body, Brush::Solid(RED));
        let b = bracket_scene(transform, shifted_rect, body, Brush::Solid(RED));

        let raster = raster_transform(transform);
        assert_ne!(
            body_fingerprint(&a, 1..2, rect, transform, 40, 20, raster),
            body_fingerprint(&b, 1..2, shifted_rect, transform, 40, 20, raster),
        );
    }

    #[test]
    fn body_fingerprint_changes_with_the_content() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let transform = Affine::scale(2.0);
        let raster = raster_transform(transform);
        let a = bracket_scene(
            transform,
            rect,
            Rect::new(2.0, 2.0, 8.0, 8.0),
            Brush::Solid(RED),
        );
        let recolored = bracket_scene(
            transform,
            rect,
            Rect::new(2.0, 2.0, 8.0, 8.0),
            Brush::Solid(BLUE),
        );
        let moved = bracket_scene(
            transform,
            rect,
            Rect::new(3.0, 2.0, 9.0, 8.0),
            Brush::Solid(RED),
        );
        let base = body_fingerprint(&a, 1..2, rect, transform, 40, 20, raster);
        assert_ne!(
            base,
            body_fingerprint(&recolored, 1..2, rect, transform, 40, 20, raster)
        );
        assert_ne!(
            base,
            body_fingerprint(&moved, 1..2, rect, transform, 40, 20, raster)
        );
    }

    #[test]
    fn body_fingerprint_changes_with_the_size_and_the_device_scale() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let transform = Affine::scale(2.0);
        let scene = bracket_scene(transform, rect, body, Brush::Solid(RED));
        let base = body_fingerprint(
            &scene,
            1..2,
            rect,
            transform,
            40,
            20,
            raster_transform(transform),
        );
        assert_ne!(
            base,
            body_fingerprint(
                &scene,
                1..2,
                rect,
                transform,
                41,
                20,
                raster_transform(transform)
            )
        );
        assert_ne!(
            base,
            body_fingerprint(
                &scene,
                1..2,
                rect,
                transform,
                40,
                20,
                raster_transform(Affine::scale(3.0))
            )
        );
    }

    #[test]
    fn reuses_entry_demands_an_exact_size_and_fingerprint_match() {
        assert!(reuses_entry(Some((40, 20, 99)), (40, 20, 99)));
        assert!(!reuses_entry(Some((40, 20, 99)), (40, 20, 100)));
        assert!(!reuses_entry(Some((40, 20, 99)), (41, 20, 99)));
        assert!(!reuses_entry(None, (40, 20, 99)));
    }

    #[test]
    fn evictable_keys_spares_the_boundary_frame_and_takes_the_older_one() {
        // frame 5, two frames of slack: last used at 3 survives, 2 goes.
        let entries = [(1, 3), (2, 2), (3, 5)];
        let mut evicted = evictable_keys(entries.into_iter(), 5, MAX_UNUSED_FRAMES);
        evicted.sort_unstable();
        assert_eq!(evicted, vec![2]);
    }

    #[test]
    fn evictable_keys_is_empty_early_in_a_surfaces_life() {
        // Frame 1 with everything just used: nothing is old enough yet.
        assert!(evictable_keys([(1, 1), (2, 1)].into_iter(), 1, MAX_UNUSED_FRAMES).is_empty());
    }

    #[test]
    fn ambiguous_keys_reports_only_a_key_recorded_twice() {
        assert_eq!(ambiguous_keys([1, 2, 1].into_iter()), vec![1]);
        assert!(ambiguous_keys([1, 2, 3].into_iter()).is_empty());
    }

    #[test]
    fn plan_derives_the_size_and_root_of_a_fresh_bracket() {
        let transform = Affine::translate((4.0, 6.0)) * Affine::scale(2.0);
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let scene = bracket_scene(
            transform,
            rect,
            Rect::new(2.0, 2.0, 8.0, 8.0),
            Brush::Solid(RED),
        );
        let cache = SnapshotCache::new(true);
        let plans = cache.plan(&scene, 8192, SURFACE);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].key, KEY);
        assert_eq!((plans[0].width, plans[0].height), (40, 20));
        assert!(!plans[0].reuse, "an empty cache can re-use nothing");
        assert_eq!(
            plans[0].root,
            raster_root(transform, rect, 40, 20).expect("invertible")
        );
    }

    #[test]
    fn plan_is_empty_when_the_kill_switch_is_set() {
        let scene = bracket_scene(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 20.0, 10.0),
            Rect::new(2.0, 2.0, 8.0, 8.0),
            Brush::Solid(RED),
        );
        assert!(
            SnapshotCache::new(false)
                .plan(&scene, 8192, SURFACE)
                .is_empty(),
            "the kill switch must short-circuit before any bracket is planned"
        );
        assert_eq!(
            SnapshotCache::new(true).plan(&scene, 8192, SURFACE).len(),
            1
        );
    }

    #[test]
    fn plan_skips_a_body_that_punches_a_hole_or_draws_a_shader() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.clear_rect(Rect::new(2.0, 2.0, 8.0, 8.0));
            builder.pop_snapshot();
        }
        assert!(
            SnapshotCache::new(true)
                .plan(&scene, 8192, SURFACE)
                .is_empty(),
            "a hole punch must reach the surface, not a snapshot texture"
        );

        let mut shader_scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut shader_scene);
            let program = frust_scene::ShaderProgram::new("@fragment fn fs_main() {}");
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.draw_shader(&program, Rect::new(2.0, 2.0, 8.0, 8.0), 0.0);
            builder.pop_snapshot();
        }
        assert!(
            SnapshotCache::new(true)
                .plan(&shader_scene, 8192, SURFACE)
                .is_empty(),
            "a shader body has no override map on the slice encode path"
        );
    }

    #[test]
    fn plan_skips_a_key_recorded_twice_in_one_frame() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            for _ in 0..2 {
                builder.push_snapshot(KEY, rect, 1.0, 1.0);
                builder.fill_rect(Rect::new(2.0, 2.0, 8.0, 8.0), Brush::Solid(RED));
                builder.pop_snapshot();
            }
        }
        assert!(
            SnapshotCache::new(true)
                .plan(&scene, 8192, SURFACE)
                .is_empty()
        );
    }

    /// A hole punch recorded AFTER the first composited bracket lowers the
    /// WHOLE frame inline. The measured shape is a camera page pushed over an
    /// already-composited page: bracketed, but uncacheable, so it lands in the
    /// trailing segment — whose pass renders into a TRANSPARENT scratch the
    /// punch erases instead of the swapchain, leaving the composited page
    /// visible exactly where the native view had to show through.
    ///
    /// Two controls: the same shape with the punch swapped for another
    /// uncacheable body still composites (so it is the punch, not the second
    /// bracket, that disables the frame), and a punch in the PRE segment is
    /// fine (that pass writes the real target).
    #[test]
    fn plan_lowers_the_frame_inline_when_a_punch_follows_the_first_bracket() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let program = frust_scene::ShaderProgram::new("@fragment fn fs_main() {}");

        // A cacheable bracket, then an uncacheable one whose body punches.
        let mut punched = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut punched);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            builder.push_snapshot(1, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.push_snapshot(2, rect, 1.0, 1.0);
            builder.clear_rect(body);
            builder.pop_snapshot();
            builder.pop_clip();
        }
        assert!(
            planned_keys(&punched).is_empty(),
            "a punch in the trailing segment must lower the whole frame inline"
        );
        assert_eq!(
            split_of(&punched),
            (seg(0..punched.commands().len()), true, None),
            "the inline frame is one ordinary vello pass, exactly as with the \
             kill switch"
        );

        // Control: the punch swapped for a shader quad — still an uncacheable
        // second bracket, but nothing to erase the scratch.
        let mut shaded = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut shaded);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            builder.push_snapshot(1, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.push_snapshot(2, rect, 1.0, 1.0);
            builder.draw_shader(&program, body, 0.0);
            builder.pop_snapshot();
            builder.pop_clip();
        }
        assert_eq!(
            planned_keys(&shaded),
            vec![1],
            "only the PUNCH disables the frame, not an uncacheable neighbour"
        );

        // Control: the same punch recorded BEFORE the composited bracket. It
        // lands in the pre segment, which writes the real target.
        let mut punched_first = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut punched_first);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0));
            builder.clear_rect(body);
            builder.push_snapshot(1, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.push_snapshot(2, rect, 1.0, 1.0);
            builder.draw_shader(&program, body, 0.0);
            builder.pop_snapshot();
            builder.pop_clip();
        }
        assert_eq!(
            planned_keys(&punched_first),
            vec![1],
            "a punch before the first bracket is the pre segment's, and fine"
        );
    }

    /// The per-bracket area budget: `snapshot_size` clamps each AXIS to the
    /// adapter limit, which alone still admits a 268 MB page, so a bracket
    /// whose texture would cover more than [`SNAPSHOT_AREA_BUDGET_FACTOR`]
    /// times the surface is refused and lowers inline.
    #[test]
    fn plan_skips_a_bracket_over_the_area_budget() {
        let surface = (100u32, 100u32);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let plan_for = |extent: f64| {
            let scene = bracket_scene(
                Affine::IDENTITY,
                Rect::new(0.0, 0.0, extent, extent),
                body,
                Brush::Solid(RED),
            );
            SnapshotCache::new(true).plan(&scene, 8192, surface).len()
        };
        // 300x300 = 90_000 device pixels against a 2 * 100 * 100 = 20_000
        // budget.
        assert_eq!(plan_for(300.0), 0, "an oversized page must lower inline");
        // 140x140 = 19_600, just inside it.
        assert_eq!(plan_for(140.0), 1, "a page within the budget composites");

        // A zero-area surface budgets nothing rather than dividing by zero.
        let scene = bracket_scene(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 10.0, 10.0),
            body,
            Brush::Solid(RED),
        );
        assert!(
            SnapshotCache::new(true)
                .plan(&scene, 8192, (0, 0))
                .is_empty(),
            "a zero-area surface composites nothing"
        );
    }

    #[test]
    fn bracket_span_covers_the_push_and_the_pop_themselves() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![
            fill(rect),
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
        ];
        let bracket = &outermost_brackets(&commands)[0];
        assert_eq!(bracket.body, 2..3, "body is just the one fill");
        assert_eq!(bracket_span(&bracket.body), 1..4);
    }

    #[test]
    fn frame_split_runs_pre_up_to_the_first_bracket_and_trails_after_it() {
        // [fill, bracket A, fill, bracket B, fill]: the pre-segment stops at
        // A's push, and everything after A's pop is a trailing segment
        // because the fills between and after the brackets survive holing.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(body, Brush::Solid(RED));
            for key in [1, 2] {
                builder.push_snapshot(key, rect, 1.0, 1.0);
                builder.fill_rect(body, Brush::Solid(RED));
                builder.pop_snapshot();
                builder.fill_rect(body, Brush::Solid(RED));
            }
        }
        assert_eq!(
            planned_keys(&scene),
            vec![1, 2],
            "both brackets composite, in scene order"
        );
        assert_eq!(split_of(&scene), (seg(0..1), true, Some(seg(4..9))));
    }

    #[test]
    fn frame_split_has_no_trailing_segment_when_the_bracket_ends_the_scene() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.push_snapshot(KEY, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
        }
        assert_eq!(split_of(&scene), (seg(0..1), true, None));
    }

    #[test]
    fn frame_split_has_no_trailing_segment_for_back_to_back_brackets() {
        // The page-transition shape: two composited brackets and nothing
        // else. The second one is a hole, so the range after the first would
        // draw nothing — no second vello pass.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            for key in [1, 2] {
                builder.push_snapshot(key, rect, 1.0, 1.0);
                builder.fill_rect(body, Brush::Solid(RED));
                builder.pop_snapshot();
            }
        }
        assert_eq!(split_of(&scene), (seg(0..0), false, None));
    }

    #[test]
    fn frame_split_leaves_an_uncacheable_bracket_in_the_trailing_segment() {
        // A bracket the cache refuses (a shader quad in its body, which has no
        // override map on the slice encode path) is neither a layer nor a
        // hole, so it keeps the trailing segment alive and lowers inline
        // there. The refusal is deliberately NOT a hole punch: a `ClearRect`
        // after the first composited bracket lowers the whole frame inline
        // instead (`plan_lowers_the_frame_inline_when_a_punch_follows_the_
        // first_bracket`), which would prove nothing about the split.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let program = frust_scene::ShaderProgram::new("@fragment fn fs_main() {}");
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(1, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.push_snapshot(2, rect, 1.0, 1.0);
            builder.draw_shader(&program, body, 0.0);
            builder.pop_snapshot();
        }
        assert_eq!(
            planned_keys(&scene),
            vec![1],
            "the uncacheable bracket must not composite"
        );
        assert_eq!(split_of(&scene), (seg(0..0), false, Some(seg(3..6))));
    }

    /// The depth-0 invariant [`open_group_pushes`] is written against: a
    /// trailing segment starts one past an OUTERMOST composited bracket's
    /// `PopSnapshot`, so no bracket is open there — and in particular no
    /// MISS'd bracket's emulated `Group::Layer` (`convert.rs` pushes one for
    /// an uncacheable bracket with `alpha < 1.0`), which this mirror
    /// deliberately does not model.
    ///
    /// The scene puts exactly that MISS'd translucent bracket BEFORE the
    /// composited one, so a prefix that walked into it would name its
    /// `PushSnapshot`.
    #[test]
    fn a_split_never_falls_inside_a_bracket() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 100.0, 100.0)); // 0: app root
            // 1: uncacheable (a punch in its body) AND translucent, so
            // `convert.rs` emulates it with a layer this walk never sees.
            builder.push_snapshot(7, rect, 0.5, 1.0); // 1
            builder.clear_rect(body); // 2: in the PRE segment, the legal place
            builder.pop_snapshot(); // 3
            builder.push_snapshot(1, rect, 1.0, 1.0); // 4: composited
            builder.fill_rect(body, Brush::Solid(RED)); // 5
            builder.pop_snapshot(); // 6
            builder.fill_rect(body, Brush::Solid(RED)); // 7: trailing
            builder.pop_clip(); // 8
        }
        // The spans the REAL plan produces: only the cacheable bracket
        // composites, so the split is taken from its span alone.
        assert_eq!(planned_keys(&scene), vec![1]);
        let spans = spans_of(&scene);
        assert_eq!(spans, vec![4..7]);

        let commands = scene.commands();
        let (_, _, trailing) = frame_split(commands, &spans);
        let trailing = trailing.expect("the fill after the bracket needs a pass");
        assert_eq!(trailing.range, 7..9);
        assert_eq!(
            trailing.prefix,
            vec![0],
            "only the app root's clip is open where the split falls"
        );
        assert!(
            !trailing
                .prefix
                .iter()
                .any(|&index| matches!(commands[index], Command::PushSnapshot { .. })),
            "a prefix must never name a snapshot bracket"
        );
        assert_eq!(
            snapshot_depth_before(commands, trailing.range.start),
            0,
            "the split lands outside every bracket"
        );
    }

    /// A nested bracket is never composited on its own: only the OUTERMOST one
    /// is a candidate, and an outermost body the cache refuses takes its whole
    /// subtree inline with it. Nothing composites, so there is no split at all
    /// — the frame is one vello pass.
    #[test]
    fn an_uncacheable_outer_bracket_composites_nothing_and_needs_no_split() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let program = frust_scene::ShaderProgram::new("@fragment fn fs_main() {}");
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(7, rect, 1.0, 1.0); // 0: the outer bracket
            builder.draw_shader(&program, body, 0.0); // 1: uncacheable body
            builder.push_snapshot(1, rect, 1.0, 1.0); // 2: the inner one
            builder.fill_rect(body, Brush::Solid(RED)); // 3
            builder.pop_snapshot(); // 4
            builder.pop_snapshot(); // 5
            builder.fill_rect(body, Brush::Solid(RED)); // 6
        }
        let commands = scene.commands();
        assert_eq!(
            outermost_brackets(commands)
                .iter()
                .map(|bracket| bracket.key)
                .collect::<Vec<_>>(),
            vec![7],
            "the inner bracket is not a candidate at all"
        );
        assert_eq!(
            snapshot_depth_before(commands, 3),
            2,
            "index 3 sits inside both brackets"
        );
        assert!(planned_keys(&scene).is_empty());
        assert_eq!(
            split_of(&scene),
            (seg(0..7), true, None),
            "nothing composited is one whole vello pass"
        );
    }

    #[test]
    fn snapshot_depth_before_counts_pushes_minus_pops() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY), // 0
            push(2, rect, Affine::IDENTITY), // 1
            Command::PopSnapshot,            // 2
            Command::PopSnapshot,            // 3
            Command::PopSnapshot,            // 4: unbalanced
        ];
        assert_eq!(snapshot_depth_before(&commands, 0), 0);
        assert_eq!(snapshot_depth_before(&commands, 2), 2);
        assert_eq!(snapshot_depth_before(&commands, 3), 1);
        assert_eq!(
            snapshot_depth_before(&commands, 5),
            0,
            "an unbalanced pop saturates rather than wrapping"
        );
    }

    #[test]
    fn a_frame_with_nothing_composited_is_one_whole_vello_pass() {
        // The kill switch's plan, and equally the plan for a frame whose
        // brackets all missed: one vello pass over everything.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![clip(rect, Affine::IDENTITY), fill(rect), Command::PopClip];
        let plan = FramePlan::inline(&commands);
        assert_eq!(plan.pre, seg(0..3));
        assert!(plan.pre_draws, "the one fill is what the pass exists for");
        assert!(plan.layers.is_empty());
        assert!(plan.holes.is_empty());
        assert!(plan.trailing.is_none());
        assert_eq!(frame_split(&commands, &[]), (seg(0..3), true, None));
        // With nothing composited there is no quad pass to clear the frame,
        // so the plan still runs its own vello pass whatever it holds.
        let structure = vec![clip(rect, Affine::IDENTITY), Command::PopClip];
        assert!(!FramePlan::inline(&structure).pre_draws);
    }

    #[test]
    fn draws_pixels_separates_painting_commands_from_structure() {
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let radii = frust_scene::CornerRadii::uniform(2.0);
        let painting = vec![
            fill(rect),
            Command::RoundedRect {
                rect,
                radii,
                brush: Brush::Solid(RED),
                transform: Affine::IDENTITY,
            },
            Command::Line {
                p0: Point::ZERO,
                p1: Point::new(1.0, 1.0),
                width: 1.0,
                brush: Brush::Solid(RED),
                transform: Affine::IDENTITY,
            },
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev: 1.0,
                color: RED,
                transform: Affine::IDENTITY,
            },
            Command::ClearRect {
                rect,
                transform: Affine::IDENTITY,
            },
            Command::Path {
                path: kurbo::BezPath::new(),
                style: frust_scene::PathStyle::Fill,
                brush: Brush::Solid(RED),
                transform: Affine::IDENTITY,
            },
            Command::ShaderQuad {
                program: frust_scene::ShaderProgram::new("@fragment fn fs_main() {}"),
                dest: rect,
                transform: Affine::IDENTITY,
                time: 0.0,
            },
        ];
        for command in &painting {
            assert!(draws_pixels(command), "{command:?} paints");
        }
        let structure = vec![
            clip(rect, Affine::IDENTITY),
            Command::PushClipRounded {
                rect,
                radii,
                transform: Affine::IDENTITY,
            },
            Command::PopClip,
            Command::PushLayer {
                rect,
                alpha: 0.5,
                transform: Affine::IDENTITY,
            },
            Command::PopLayer,
            push(1, rect, Affine::IDENTITY),
            Command::PopSnapshot,
        ];
        for command in &structure {
            assert!(!draws_pixels(command), "{command:?} paints nothing");
        }
    }

    /// The measured page-transition shape: the app root's `PushClip`, the
    /// composited bracket, the matching `PopClip`. Neither segment paints, so
    /// the frame is one compositor pass — no main vello pass (`pre_draws`
    /// false) and no trailing one.
    #[test]
    fn frame_split_reports_a_structure_only_pre_and_tail_as_drawing_nothing() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![
            clip(rect, Affine::IDENTITY),
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopClip,
        ];
        let spans = [bracket_span(&(2..3))];
        assert_eq!(
            frame_split(&commands, &spans),
            (seg(0..1), false, None),
            "a PushClip/PopClip pair is not worth two vello passes"
        );
    }

    #[test]
    fn frame_split_reports_a_pre_segment_that_paints() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![
            clip(rect, Affine::IDENTITY),
            fill(rect),
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopClip,
        ];
        let spans = [bracket_span(&(3..4))];
        assert_eq!(frame_split(&commands, &spans), (seg(0..2), true, None));
    }

    #[test]
    fn frame_split_keeps_a_trailing_segment_that_paints() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            Command::PopClip,
            fill(rect),
        ];
        let spans = [bracket_span(&(1..2))];
        assert_eq!(
            frame_split(&commands, &spans),
            (seg(0..0), false, Some(seg(3..5)))
        );
    }

    /// A later composited bracket inside the trailing range is a hole, so its
    /// own body's paint commands do not keep the pass alive.
    #[test]
    fn frame_split_ignores_paint_inside_a_later_composited_bracket() {
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let commands = vec![
            push(1, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
            push(2, rect, Affine::IDENTITY),
            fill(rect),
            Command::PopSnapshot,
        ];
        let spans = [bracket_span(&(1..2)), bracket_span(&(4..5))];
        assert_eq!(frame_split(&commands, &spans), (seg(0..0), false, None));
    }

    /// The de-registration invariant, asserted the only way a unit test can:
    /// the module's own source must not name vello's texture-override API at
    /// all. A cached page is sampled by the compositor, never uploaded into
    /// vello's image atlas — re-introducing a registration here would put the
    /// ~100 ms/frame image quad back on the mobile GPU this seam exists to
    /// get off. The needles are split so this assertion is not its own
    /// counterexample.
    #[test]
    fn no_snapshot_texture_is_handed_to_vello() {
        let source = include_str!("snapshot.rs");
        for needle in [
            concat!("register", "_texture"),
            concat!("mark_override", "_image_dirty"),
        ] {
            assert!(
                !source.contains(needle),
                "snapshot.rs must not call `{needle}`"
            );
        }
    }

    /// End-to-end (real device) confirmation of the bookkeeping the pure tests
    /// above can only cover one decision at a time: that an unchanged — and a
    /// merely slid — bracket performs ZERO further rasterizations, that a
    /// content change performs exactly one, that a bracket the scene stops
    /// drawing is dropped, and that the kill switch does no GPU work at all.
    /// `Entry` needs a real `wgpu::Texture` to exist, so this is the only
    /// place the counters — and the [`FramePlan`] a real rasterization
    /// produces — can be observed against real resources.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn steady_state_animation_rasterizes_a_body_exactly_once() {
        pollster::block_on(run());

        async fn run() {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust snapshot cache test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");

            let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
            let body = Rect::new(2.0, 2.0, 8.0, 8.0);
            let transform = Affine::scale(2.0);
            let scene = bracket_scene(transform, rect, body, Brush::Solid(RED));

            let mut cache = SnapshotCache::new(true);
            let plan = cache.prepare(&device, &queue, &mut renderer, &scene, 8192, SURFACE);
            // One bracket, filling the whole scene: it becomes the only
            // layer and the only hole, there is nothing before it to draw,
            // and nothing after it to need a second pass.
            let layer = match plan.layers.as_slice() {
                [layer] => layer,
                other => panic!("expected exactly one composite layer, got {}", other.len()),
            };
            assert_eq!(layer.key, KEY);
            assert_eq!((layer.width, layer.height), (40, 20));
            assert_eq!(layer.rect, rect);
            assert_eq!(layer.transform, transform);
            assert_eq!((layer.alpha, layer.scale), (1.0, 1.0));
            assert_eq!(layer.clip, None, "an unclipped bracket needs no scissor");
            assert_eq!(plan.holes, HashSet::from([KEY]));
            assert_eq!(plan.pre, seg(0..0));
            assert!(
                !plan.pre_draws,
                "an empty pre segment must not buy a vello pass"
            );
            assert_eq!(plan.trailing, None);
            assert_eq!(cache.renders(KEY), Some(1));

            // Same scene again, then the same body slid across the surface:
            // both re-use the texture, so the counter must not move.
            cache.prepare(&device, &queue, &mut renderer, &scene, 8192, SURFACE);
            assert_eq!(
                cache.renders(KEY),
                Some(1),
                "an unchanged body must not re-render"
            );
            let slid_transform = Affine::translate((17.0, 3.0)) * transform;
            let slid = bracket_scene(slid_transform, rect, body, Brush::Solid(RED));
            let plan = cache.prepare(&device, &queue, &mut renderer, &slid, 8192, SURFACE);
            assert!(plan.holes.contains(&KEY));
            assert_eq!(
                plan.layers[0].transform, slid_transform,
                "the layer follows the bracket even when the texture does not"
            );
            assert_eq!(
                cache.renders(KEY),
                Some(1),
                "a bracket that only slides must not re-render"
            );

            // Changed content: exactly one more rasterization.
            let recolored = bracket_scene(transform, rect, body, Brush::Solid(BLUE));
            cache.prepare(&device, &queue, &mut renderer, &recolored, 8192, SURFACE);
            assert_eq!(cache.renders(KEY), Some(2));

            // The scene stops drawing the bracket: aged out and dropped.
            let empty = Scene::new();
            for _ in 0..=MAX_UNUSED_FRAMES {
                cache.prepare(&device, &queue, &mut renderer, &empty, 8192, SURFACE);
            }
            assert_eq!(cache.len(), 0, "an undrawn bracket must be evicted");

            // Kill switch: one whole-scene vello pass, no entries, no GPU work.
            let mut disabled = SnapshotCache::new(false);
            let plan = disabled.prepare(&device, &queue, &mut renderer, &scene, 8192, SURFACE);
            assert!(plan.layers.is_empty() && plan.holes.is_empty());
            assert_eq!(plan.pre, seg(0..scene.commands().len()));
            assert!(plan.pre_draws, "the body's fill is in the pre segment now");
            assert_eq!(plan.trailing, None);
            assert_eq!(disabled.len(), 0);
        }
    }
}
