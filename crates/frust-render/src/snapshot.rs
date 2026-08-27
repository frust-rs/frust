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
//! premultiply twice and darken every translucent edge. The GPU smoke
//! (`tests/gpu_smoke.rs`) pins this end to end by comparing a cached bracket
//! against the same bracket lowered inline.
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
//!
//! Each of those simply yields no layer and no hole, so the bracket lowers
//! inline via [`crate::convert`]'s MISS path in whichever segment it falls —
//! the path the renderer used before this cache existed, never a panic and
//! never a wrong-pixels shortcut.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use frust_scene::{Command, Scene};
use kurbo::{Affine, Rect};

use crate::convert::encode_commands_into;
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
// Placement carried for a consumer that is not written yet: `compositor.rs`
// reads every field below to build one quad. Until it exists, only `key`
// (which becomes a hole) has an in-crate reader — the allow covers the rest
// rather than fabricating uses for them, and comes off with the first quad.
#[allow(dead_code)]
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
    // Read by the render path once it draws per segment; today it still
    // encodes the whole scene and reads `holes` alone. Same for `layers` and
    // `trailing` below.
    #[allow(dead_code)]
    pub pre: Range<usize>,
    /// Every composited bracket, in scene order.
    #[allow(dead_code)]
    pub layers: Vec<CompositeLayer>,
    /// `layers`' keys: the brackets [`crate::convert`] turns into holes.
    pub holes: HashSet<u64>,
    /// Commands after the first composited bracket, `Some` only when that
    /// range holds at least one command that is not itself inside a composited
    /// bracket — i.e. only when a second vello pass would actually draw
    /// something. `None` is the common case (a page transition whose brackets
    /// are the last thing in the scene) and saves that whole pass.
    #[allow(dead_code)]
    pub trailing: Option<Range<usize>>,
}

impl FramePlan {
    /// The plan for a frame with nothing composited: vello draws all `len`
    /// commands, exactly as it did before this cache existed.
    fn inline(len: usize) -> Self {
        Self {
            pre: 0..len,
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
}

/// The bracket's own INCLUSIVE index span as a half-open range: its
/// `PushSnapshot` through its `PopSnapshot`. This is the span a composited
/// bracket contributes nothing outside of, so it is what the trailing-segment
/// decision subtracts.
fn bracket_span(body: &Range<usize>) -> Range<usize> {
    body.start.saturating_sub(1)..body.end + 1
}

/// The two-segment split of a frame's command list, given `len` commands and
/// the [`bracket_span`]s of the composited brackets in scene order.
///
/// `pre` runs up to the first composited bracket's `PushSnapshot`; the
/// trailing segment starts one past that same bracket's `PopSnapshot` and runs
/// to the end. The trailing pass exists only if it would draw something: every
/// later composited bracket inside it is skipped as a hole, so the segment is
/// worth a vello pass exactly when it is longer than those brackets put
/// together.
fn frame_split(len: usize, spans: &[Range<usize>]) -> (Range<usize>, Option<Range<usize>>) {
    let Some(first) = spans.first() else {
        return (0..len, None);
    };
    let start = first.end.min(len);
    let composited: usize = spans[1..]
        .iter()
        .map(|span| span.end.min(len).saturating_sub(span.start.min(len)))
        .sum();
    let trailing = ((len - start) > composited).then_some(start..len);
    (0..first.start.min(len), trailing)
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

/// Device pixel size for a body: the bounding box of `rect` under `raster`,
/// rounded OUTWARD so no part of the body falls off the texture's last pixel,
/// then clamped to vello's atlas ceiling and the adapter's own limit by the
/// same [`clamp_size`] policy the shader pre-pass uses (which also floors a
/// degenerate `0` to `1`).
fn snapshot_size(raster: Affine, rect: Rect, adapter_max: u32) -> (u32, u32) {
    let bbox = raster.transform_rect_bbox(rect);
    let to_u32 = |v: f64| {
        let v = v.ceil();
        if v.is_finite() && v > 0.0 {
            v as u32
        } else {
            0
        }
    };
    clamp_size((to_u32(bbox.width()), to_u32(bbox.height())), adapter_max)
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

/// Every OUTERMOST balanced bracket in `commands`, in recording order.
///
/// Nested brackets are skipped entirely (the command contract makes only the
/// outermost one binding), an unbalanced `PopSnapshot` is ignored the way the
/// encode walk ignores it, and a bracket left open at the end of the list has
/// no bounded body so it yields nothing — that bracket keeps the inline path.
fn outermost_brackets(commands: &[Command]) -> Vec<Bracket> {
    let mut brackets = Vec::new();
    let mut depth: usize = 0;
    let mut open: Option<(usize, Bracket)> = None;
    for (index, command) in commands.iter().enumerate() {
        match command {
            Command::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
                transform,
            } => {
                if depth == 0 {
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
    ) -> FramePlan {
        let len = scene.commands().len();
        if !self.enabled {
            return FramePlan::inline(len);
        }
        self.frame += 1;
        let mut layers = Vec::new();
        let mut spans = Vec::new();
        for plan in self.plan(scene, adapter_max) {
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
                view: entry.view.clone(),
            });
            spans.push(bracket_span(&plan.body));
        }
        self.evict();
        let holes = layers.iter().map(|layer| layer.key).collect();
        let (pre, trailing) = frame_split(len, &spans);
        FramePlan {
            pre,
            layers,
            holes,
            trailing,
        }
    }

    /// The GPU-free half of [`Self::prepare`]: what this frame would rasterize
    /// and what it would re-use. Split out so the scan, the size/root
    /// derivation, the fingerprint decision and the kill switch are all
    /// assertable without a device.
    fn plan(&self, scene: &Scene, adapter_max: u32) -> Vec<Plan> {
        if !self.enabled {
            return Vec::new();
        }
        let commands = scene.commands();
        let brackets = outermost_brackets(commands);
        let ambiguous = ambiguous_keys(brackets.iter().map(|bracket| bracket.key));
        let mut plans = Vec::new();
        for bracket in brackets {
            if ambiguous.contains(&bracket.key)
                || !is_cacheable_body(&commands[bracket.body.clone()])
            {
                continue;
            }
            let raster = raster_transform(bracket.transform);
            let (width, height) = snapshot_size(raster, bracket.rect, adapter_max);
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
            });
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
    /// Called when the surface goes away. `_renderer` is vestigial — these
    /// textures are never handed to vello, so there is nothing to surrender
    /// before it dies — and stays only so the teardown call site keeps its
    /// shape until the compositor owns that sequence.
    pub(crate) fn clear(&mut self, _renderer: &mut vello::Renderer) {
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

    /// The split a frame would take if every eligible bracket in `scene` hit
    /// the cache — [`SnapshotCache::prepare`]'s own arithmetic, minus the GPU
    /// work that decides which plans actually became layers.
    fn split_of(scene: &Scene) -> (Range<usize>, Option<Range<usize>>) {
        let spans: Vec<Range<usize>> = SnapshotCache::new(true)
            .plan(scene, 8192)
            .iter()
            .map(|plan| bracket_span(&plan.body))
            .collect();
        frame_split(scene.commands().len(), &spans)
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
        let plans = cache.plan(&scene, 8192);
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
            SnapshotCache::new(false).plan(&scene, 8192).is_empty(),
            "the kill switch must short-circuit before any bracket is planned"
        );
        assert_eq!(SnapshotCache::new(true).plan(&scene, 8192).len(), 1);
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
            SnapshotCache::new(true).plan(&scene, 8192).is_empty(),
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
                .plan(&shader_scene, 8192)
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
        assert!(SnapshotCache::new(true).plan(&scene, 8192).is_empty());
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
        let keys: Vec<u64> = SnapshotCache::new(true)
            .plan(&scene, 8192)
            .iter()
            .map(|plan| plan.key)
            .collect();
        assert_eq!(keys, vec![1, 2], "both brackets composite, in scene order");
        assert_eq!(split_of(&scene), (0..1, Some(4..9)));
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
        assert_eq!(split_of(&scene), (0..1, None));
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
        assert_eq!(split_of(&scene), (0..0, None));
    }

    #[test]
    fn frame_split_leaves_an_uncacheable_bracket_in_the_trailing_segment() {
        // A bracket the cache refuses (a hole punch in its body) is neither a
        // layer nor a hole, so it keeps the trailing segment alive and lowers
        // inline there.
        let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
        let body = Rect::new(2.0, 2.0, 8.0, 8.0);
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_snapshot(1, rect, 1.0, 1.0);
            builder.fill_rect(body, Brush::Solid(RED));
            builder.pop_snapshot();
            builder.push_snapshot(2, rect, 1.0, 1.0);
            builder.clear_rect(body);
            builder.pop_snapshot();
        }
        let keys: Vec<u64> = SnapshotCache::new(true)
            .plan(&scene, 8192)
            .iter()
            .map(|plan| plan.key)
            .collect();
        assert_eq!(keys, vec![1], "the hole-punch bracket must not composite");
        assert_eq!(split_of(&scene), (0..0, Some(3..6)));
    }

    #[test]
    fn a_frame_with_nothing_composited_is_one_whole_vello_pass() {
        // The kill switch's plan, and equally the plan for a frame whose
        // brackets all missed: one vello pass over everything.
        let plan = FramePlan::inline(9);
        assert_eq!(plan.pre, 0..9);
        assert!(plan.layers.is_empty());
        assert!(plan.holes.is_empty());
        assert!(plan.trailing.is_none());
        assert_eq!(frame_split(9, &[]), (0..9, None));
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
            let plan = cache.prepare(&device, &queue, &mut renderer, &scene, 8192);
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
            assert_eq!(plan.holes, HashSet::from([KEY]));
            assert_eq!(plan.pre, 0..0);
            assert_eq!(plan.trailing, None);
            assert_eq!(cache.renders(KEY), Some(1));

            // Same scene again, then the same body slid across the surface:
            // both re-use the texture, so the counter must not move.
            cache.prepare(&device, &queue, &mut renderer, &scene, 8192);
            assert_eq!(
                cache.renders(KEY),
                Some(1),
                "an unchanged body must not re-render"
            );
            let slid_transform = Affine::translate((17.0, 3.0)) * transform;
            let slid = bracket_scene(slid_transform, rect, body, Brush::Solid(RED));
            let plan = cache.prepare(&device, &queue, &mut renderer, &slid, 8192);
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
            cache.prepare(&device, &queue, &mut renderer, &recolored, 8192);
            assert_eq!(cache.renders(KEY), Some(2));

            // The scene stops drawing the bracket: aged out and dropped.
            let empty = Scene::new();
            for _ in 0..=MAX_UNUSED_FRAMES {
                cache.prepare(&device, &queue, &mut renderer, &empty, 8192);
            }
            assert_eq!(cache.len(), 0, "an undrawn bracket must be evicted");

            // Kill switch: one whole-scene vello pass, no entries, no GPU work.
            let mut disabled = SnapshotCache::new(false);
            let plan = disabled.prepare(&device, &queue, &mut renderer, &scene, 8192);
            assert!(plan.layers.is_empty() && plan.holes.is_empty());
            assert_eq!(plan.pre, 0..scene.commands().len());
            assert_eq!(plan.trailing, None);
            assert_eq!(disabled.len(), 0);
        }
    }
}
