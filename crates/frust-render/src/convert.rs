//! Translation of the renderer-agnostic [`frust_scene::Scene`] display list
//! into `vello` draw calls.
//!
//! The mapping logic lives behind the [`SceneSink`] trait so it can be unit
//! tested without a GPU (see the tests below, which record calls into a plain
//! `Vec`). The only production implementor is `vello::Scene`; the public
//! [`encode_scene`] entry point is the one place a `vello` type appears in this
//! crate's API — deliberately, so a shell that owns its own `vello::Renderer`
//! can reuse Frust's scene encoding (mirrors how `frust-scene` allows
//! only `peniko` types in its own public API).

use frust_scene::{Command, DashPattern, GlyphRun, PathStyle, Scene};
use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, RoundedRectRadii, Stroke};
use peniko::{Brush, Color, Fill, ImageData};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

use crate::snapshot::FramePlan;

/// Sink for the individual draw operations a [`Scene`] decomposes into.
///
/// Implemented for `vello::Scene` (real rendering) and for a recording sink in
/// tests (GPU-free verification of the command mapping).
pub(crate) trait SceneSink {
    /// Fill an axis-aligned rectangle with `brush` under `transform`.
    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect);
    /// Fill an axis-aligned rounded rectangle with per-corner `radii` (a
    /// uniform radius arrives as four equal corners).
    fn fill_rounded_rect(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radii: RoundedRectRadii,
    );
    /// Stroke a straight line segment from `p0` to `p1` with the given `width`.
    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64);
    /// Draw a positioned run of glyphs.
    fn draw_glyph_run(&mut self, run: &GlyphRun);
    /// Push a rectangular clip onto the backend's clip stack, under `transform`.
    fn push_clip(&mut self, transform: Affine, rect: &Rect);
    /// Push a clip with per-corner rounded corners onto the backend's clip
    /// stack, under `transform`. Popped by [`SceneSink::pop_clip`], the same
    /// pop the rectangular clip uses.
    fn push_clip_rounded(&mut self, transform: Affine, rect: &Rect, radii: RoundedRectRadii);
    /// Pop the most recently pushed clip, rectangular or rounded.
    fn pop_clip(&mut self);
    /// Draw a decoded image (natural pixel size `data.width`x`data.height`),
    /// scaled to fill `dest`, under `transform`.
    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect);
    /// Draw a gaussian-blurred rounded-rectangle elevation shadow.
    ///
    /// A single `radius`, unlike the two rounded methods above: neither backend
    /// has a per-corner blurred primitive, so the walk lowers a per-corner
    /// shadow before it reaches a sink (see `encode_into_with_shaders`).
    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: &Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    );
    /// Push a translucent layer onto the backend's layer stack, under `transform`.
    fn push_layer(&mut self, transform: Affine, rect: &Rect, alpha: f32);
    /// Pop the most recently pushed layer.
    fn pop_layer(&mut self);
    /// Clear `rect` to full transparency (alpha 0) under `transform`, erasing
    /// the backdrop already drawn beneath it — the platform-view hole-punch.
    fn clear_rect(&mut self, transform: Affine, rect: &Rect);
    /// Fill an arbitrary vector path with `brush` under `transform`, using
    /// the nonzero winding rule.
    fn fill_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath);
    /// Stroke an arbitrary vector path with `brush`/`width` (round caps/joins)
    /// under `transform`.
    fn stroke_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath, width: f64);
}

/// Encodes every command in `scene` into `target` (a reused `vello::Scene`).
///
/// Call `target.reset()` before this to clear the previous frame — the render
/// path does exactly that, rebuilding the scene fresh every frame rather than
/// accumulating draw calls across frames.
///
/// This is the no-shader-map convenience entry the public seam
/// (`docs/ARCHITECTURE.md`'s scene-layer purity rule) exposes for shells that
/// drive their own `vello::Renderer`: with no per-frame shader-override map,
/// every `Command::ShaderQuad` lowers to its miss placeholder (a CPU-tier /
/// no-prepass caller has no compiled shader targets anyway). The in-crate
/// render path calls [`encode_into_with_overrides`] instead.
pub fn encode_scene(scene: &Scene, target: &mut vello::Scene) {
    encode_into(scene, target);
}

/// Generic worker behind [`encode_scene`]; kept separate so tests (and the
/// `cpu-tier` sink) can drive it with a non-`vello` sink and no shader map. The
/// empty map means every [`Command::ShaderQuad`] misses to its placeholder, so
/// the `adapter_max` passed here is immaterial — `u32::MAX` (no extra clamp).
pub(crate) fn encode_into(scene: &Scene, sink: &mut impl SceneSink) {
    encode_into_with_shaders(scene, sink, &HashMap::new(), u32::MAX);
}

/// The whole `scene` with no holes, resolving each [`Command::ShaderQuad`]
/// against `shader_images` (`(program id, clamped physical size)` → the shader
/// pre-pass's registered override [`ImageData`]). A hit lowers to a
/// `draw_image`; a miss keeps the placeholder fill. Every
/// [`Command::PushSnapshot`] takes its inline MISS path, since no bracket is
/// composited away.
pub(crate) fn encode_into_with_shaders(
    scene: &Scene,
    sink: &mut impl SceneSink,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
) {
    encode_range_with_overrides(
        scene,
        0..scene.commands().len(),
        sink,
        shader_images,
        adapter_max,
        &HashSet::new(),
    );
}

/// One segment of a frame: the commands in `range`, encoded under the scene
/// root, with every bracket whose `key` is in `holes` skipped entirely.
///
/// This is the single entry the render path drives BOTH of a frame's vello
/// passes through (see [`crate::snapshot::FramePlan`] for the split): the
/// pre-segment and, when there is one, the trailing segment. A sub-range is
/// encoded exactly as if it were the whole scene — the root stays
/// `Affine::IDENTITY` (each command already carries its own composed
/// transform) and the `ClearRect` hoist is scoped to the range, so a punch
/// recorded inside it lands at the range's own root rather than reaching for
/// groups the segment never opened.
///
/// `range` is clamped to the scene rather than trusted: a plan is always built
/// from the same frame's scene, so a stale range is a bug, but the render loop
/// is not a place to panic.
pub(crate) fn encode_range_with_overrides(
    scene: &Scene,
    range: Range<usize>,
    sink: &mut impl SceneSink,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
    holes: &HashSet<u64>,
) {
    let commands = scene.commands();
    let end = range.end.min(commands.len());
    let start = range.start.min(end);
    encode_commands(
        &commands[start..end],
        Affine::IDENTITY,
        sink,
        shader_images,
        adapter_max,
        holes,
    );
}

/// The whole `scene` under `plan`'s holes: every composited bracket skipped,
/// every other bracket lowered inline.
///
/// The frame this produces is missing exactly the composited pages — the
/// compositor draws them, and until it is wired in they are simply absent.
/// Once the render path renders per segment it calls
/// [`encode_range_with_overrides`] directly with [`crate::snapshot::FramePlan`]'s
/// own ranges, and this whole-scene shorthand goes away.
pub(crate) fn encode_into_with_overrides(
    scene: &Scene,
    sink: &mut impl SceneSink,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
    plan: &FramePlan,
) {
    encode_range_with_overrides(
        scene,
        0..scene.commands().len(),
        sink,
        shader_images,
        adapter_max,
        &plan.holes,
    );
}

/// Encodes an arbitrary command slice into `sink` under `root`, pre-multiplied
/// onto every command's own recorded transform (including the `ClearRect`
/// hoist, so a punch computed from a sub-scene still lands in `root`'s
/// coordinate space). The entry point a caller that doesn't own a whole
/// [`Scene`] can drive directly with just a body's own command range and the
/// transform it should be rasterized under — [`crate::snapshot`]'s
/// rasterization pre-pass is the caller. No shader-override map and no holes:
/// every [`Command::ShaderQuad`] misses to its placeholder and every
/// [`Command::PushSnapshot`] takes the MISS path, same as [`encode_into`].
pub(crate) fn encode_commands_into(commands: &[Command], root: Affine, sink: &mut impl SceneSink) {
    encode_commands(
        commands,
        root,
        sink,
        &HashMap::new(),
        u32::MAX,
        &HashSet::new(),
    );
}

/// The command walk shared by every entry point above, parameterized by the
/// root transform every command's own transform is pre-multiplied by, the
/// per-frame shader-override map (empty for the no-shader callers), the
/// `adapter_max` used to recompute each [`Command::ShaderQuad`]'s `(id, w, h)`
/// map key (identical to the pre-pass's keying), and the set of
/// [`Command::PushSnapshot`] keys this pass must leave as holes (empty for
/// every caller that draws the whole scene itself). Kept generic over
/// [`SceneSink`] so it is GPU-free unit-testable.
fn encode_commands(
    commands: &[Command],
    root: Affine,
    sink: &mut impl SceneSink,
    shader_images: &HashMap<(u64, u32, u32), ImageData>,
    adapter_max: u32,
    holes: &HashSet<u64>,
) {
    // Active clip/opacity group stack, tracked so a `ClearRect` can be HOISTED
    // to the root: a `Compose::Clear` inside a vello layer group only clears
    // that group's own accumulated content — anything painted OUTSIDE the
    // group (an app-root backdrop below a scroll_view's clip) survives the
    // group composite, defeating the Mode B hole punch (pixel-proven in
    // `tests/gpu_smoke.rs`). On `ClearRect` the walk pops
    // every open group, emits the clear at root — bounded by the intersection
    // of the popped groups' clip bounds so a partially-scrolled slot still
    // clips to its viewport — then re-pushes the same groups and continues.
    // Content painted before the slot (any nesting) is cleared; content
    // painted after (overlapping chrome) composites over the hole as before.
    // Caveat: an opacity group split this way composites its two halves
    // independently (a transient, transition-only artifact where translucent
    // group content overlaps a slot mid-animation — documented tradeoff).
    enum Group {
        /// A clip group; `Some(radii)` when the clip has rounded corners, so
        /// the hoist below re-pushes it with its corners intact.
        Clip(Option<RoundedRectRadii>),
        Layer(f32),
    }
    let mut groups: Vec<(Group, Affine, Rect)> = Vec::new();

    // `Command::PushSnapshot`'s HOLE/MISS paths (only the OUTERMOST open
    // bracket is ever honoured — a syntactically nested one contributes
    // nothing of its own, see `Command::PushSnapshot`'s doc comment, point 6),
    // tracked as —
    //   - `snapshot_depth`: how many MISS'd (inline-emulated) brackets are
    //     currently open, so a matching `PopSnapshot` (and only it) can close
    //     the outer one and an unbalanced pop is ignored, same policy as
    //     `PopLayer`;
    //   - `snapshot_correction`: the affine every subsequent command's own
    //     `transform` is left-multiplied by (after `root`) while the outer
    //     MISS'd bracket is open. `Affine::IDENTITY` outside one, so applying
    //     it unconditionally below is a no-op then. Set to `M * S *
    //     M.inverse()` where `M` is the outer `PushSnapshot`'s own transform
    //     and `S` is `scale_about(scale, rect.center())`: conjugating `S` by
    //     `M` is what makes "insert `push_transform(S)` right after
    //     `PushSnapshot`" (the contract's point 5) land correctly on a body
    //     command whose transform was already recorded as `M * (whatever the
    //     body itself pushed)`, for a body that pushes its own nested
    //     transforms and not just the flat case;
    //   - `snapshot_layer_pushed`: whether the outer MISS'd bracket's
    //     `alpha < 1.0` emulated an alpha layer (via the existing `groups`
    //     mechanism, same as `PushLayer`), so the matching close pops it. A
    //     holed bracket never pushes one — its alpha rides on the
    //     compositor's blend, not on a vello layer;
    //   - `snapshot_skip_depth`: how many brackets deep the walk is inside a
    //     HOLED bracket's skipped body. Set to 1 the instant a hole is found
    //     (`snapshot_depth` itself is never incremented for a hole — the body
    //     contributes no correction, since it is never walked at all), then
    //     tracks nested `PushSnapshot`/`PopSnapshot` depth with no other
    //     command processed, until the matching `PopSnapshot` brings it back
    //     to 0.
    let mut snapshot_depth: usize = 0;
    let mut snapshot_correction = Affine::IDENTITY;
    let mut snapshot_layer_pushed = false;
    let mut snapshot_skip_depth: usize = 0;

    for command in commands {
        if snapshot_skip_depth > 0 {
            // Inside a holed bracket's body: every command is skipped except
            // depth-tracking, so the matching `PopSnapshot` (and only it) ends
            // the skip. Nothing was pushed on the way in, so nothing is popped
            // on the way out — the whole bracket contributes zero ops.
            match command {
                Command::PushSnapshot { .. } => snapshot_skip_depth += 1,
                Command::PopSnapshot => snapshot_skip_depth -= 1,
                _ => {}
            }
            continue;
        }

        let combined = root * snapshot_correction;
        match command {
            Command::FillRect {
                rect,
                brush,
                transform,
            } => sink.fill_rect(Fill::NonZero, combined * *transform, brush, rect),
            Command::RoundedRect {
                rect,
                radii,
                brush,
                transform,
            } => sink.fill_rounded_rect(
                Fill::NonZero,
                combined * *transform,
                brush,
                rect,
                radii_of(*radii),
            ),
            Command::Line {
                p0,
                p1,
                width,
                brush,
                transform,
            } => sink.stroke_line(combined * *transform, brush, *p0, *p1, *width),
            Command::GlyphRun(run) => {
                if combined == Affine::IDENTITY {
                    sink.draw_glyph_run(run);
                } else {
                    let mut corrected = run.clone();
                    corrected.transform = combined * corrected.transform;
                    sink.draw_glyph_run(&corrected);
                }
            }
            Command::PushClip { rect, transform } => {
                let transform = combined * *transform;
                groups.push((Group::Clip(None), transform, *rect));
                sink.push_clip(transform, rect);
            }
            Command::PushClipRounded {
                rect,
                radii,
                transform,
            } => {
                // Same clip stack as `PushClip` (one `PopClip` pops either);
                // the radii ride along so the `ClearRect` hoist can re-push
                // the rounded shape rather than squaring its corners.
                let radii = radii_of(*radii);
                let transform = combined * *transform;
                groups.push((Group::Clip(Some(radii)), transform, *rect));
                sink.push_clip_rounded(transform, rect, radii);
            }
            Command::PopClip => {
                groups.pop();
                sink.pop_clip();
            }
            Command::Image {
                data,
                dest,
                transform,
            } => {
                sink.draw_image(combined * *transform, data, dest);
            }
            Command::BlurredRoundedRect {
                rect,
                radii,
                std_dev,
                color,
                transform,
            } => {
                // vello 0.9's `draw_blurred_rounded_rect` and vello_cpu's
                // `fill_blurred_rounded_rect` both take ONE radius, so a
                // per-corner shadow lowers to its largest corner here — the
                // single place the downgrade lives, shared by both tiers (see
                // `frust_scene::CornerRadii::largest` for why the largest).
                sink.draw_blurred_rounded_rect(
                    combined * *transform,
                    rect,
                    *color,
                    radii.largest(),
                    *std_dev,
                )
            }
            Command::PushLayer {
                rect,
                alpha,
                transform,
            } => {
                let transform = combined * *transform;
                groups.push((Group::Layer(*alpha), transform, *rect));
                sink.push_layer(transform, rect, *alpha);
            }
            Command::PopLayer => {
                groups.pop();
                sink.pop_layer();
            }
            Command::ClearRect { rect, transform } if groups.is_empty() => {
                sink.clear_rect(combined * *transform, rect);
            }
            Command::ClearRect { rect, transform } => {
                // Hoist to root (see the `groups` doc above): bound the punch
                // by every open group's clip bbox, pop them all, clear, then
                // re-push. Bboxes are exact for the axis-aligned transforms
                // frust emits (translate/scale); a rotated clip would bound
                // conservatively.
                let mut punch = (combined * *transform).transform_rect_bbox(*rect);
                for (_, t, r) in &groups {
                    punch = punch.intersect(t.transform_rect_bbox(*r));
                }
                if punch.width() > 0.0 && punch.height() > 0.0 {
                    for (kind, ..) in groups.iter().rev() {
                        match kind {
                            Group::Clip(_) => sink.pop_clip(),
                            Group::Layer(_) => sink.pop_layer(),
                        }
                    }
                    sink.clear_rect(Affine::IDENTITY, &punch);
                    for (kind, t, r) in &groups {
                        match kind {
                            Group::Clip(None) => sink.push_clip(*t, r),
                            Group::Clip(Some(radii)) => sink.push_clip_rounded(*t, r, *radii),
                            Group::Layer(alpha) => sink.push_layer(*t, r, *alpha),
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
                    PathStyle::Fill => sink.fill_path(transform, brush, path),
                    PathStyle::Stroke { width, dash } => match dash {
                        Some(dash) if dash.is_effective() => {
                            let dashed = dash_path(path, *dash);
                            sink.stroke_path(transform, brush, &dashed, *width);
                        }
                        // No pattern, or a degenerate one (see
                        // `DashPattern::is_effective`): a plain solid stroke.
                        _ => sink.stroke_path(transform, brush, path, *width),
                    },
                }
            }
            Command::ShaderQuad {
                program,
                dest,
                transform,
                time: _,
            } => {
                // Resolve the program's shader pre-pass output (an offscreen
                // texture registered with vello as an image override — see
                // `crate::renderer`'s pre-pass). A hit
                // lowers to the same `draw_image` path a `Command::Image` uses,
                // reusing `natural_to_dest_transform`'s natural→dest scaling so
                // the physical-pixel target lands pixel-for-pixel in `dest`.
                //
                // The map is keyed by `(id, clamped physical size)`, so the same
                // program drawn at two sizes in one frame resolves each quad to
                // its own texture — recompute the identical key the pre-pass
                // stored the entry under (`physical_size` then `clamp_size` with
                // the same `adapter_max`). Deliberately keyed off the RAW
                // (uncorrected, un-rooted) transform: the pre-pass walk that
                // populated `shader_images` reads `scene.commands()` directly
                // with no root/snapshot-correction knowledge, so recomputing
                // the key from the corrected transform would never hit.
                let (w, h) = crate::shader_effects::clamp_size(
                    crate::renderer::physical_size(*transform, *dest),
                    adapter_max,
                );
                let transform = combined * *transform;
                match shader_images.get(&(program.id(), w, h)) {
                    Some(image) => sink.draw_image(transform, image, dest),
                    None => {
                        // Miss: the CPU tier (no shader pre-pass runs), a
                        // failed shader compile, or no override registered this
                        // frame. Fill `dest` with an opaque dark placeholder so
                        // the quad renders visible geometry rather than silently
                        // dropping, warning once per process (not per frame).
                        static WARNED_SHADER_MISS: OnceLock<()> = OnceLock::new();
                        WARNED_SHADER_MISS.get_or_init(|| {
                            log::warn!(
                                "Command::ShaderQuad lowered to a placeholder fill — no shader \
                                 override registered (CPU tier, failed compile, or no pre-pass)"
                            );
                        });
                        sink.fill_rect(
                            Fill::NonZero,
                            transform,
                            &Brush::Solid(Color::from_rgba8(16, 16, 16, 255)),
                            dest,
                        );
                    }
                }
            }
            Command::PushSnapshot {
                key,
                rect,
                alpha,
                scale,
                transform,
            } => {
                if snapshot_depth == 0 {
                    if holes.contains(key) {
                        // HOLE: this bracket's pixels come from the
                        // compositor, so this pass emits NOTHING for it — no
                        // layer, no transform, no image — and skips the body
                        // outright. `rect`/`alpha`/`scale` are the
                        // compositor's to apply (`crate::snapshot`'s
                        // `CompositeLayer`); applying any of them here would
                        // double them.
                        snapshot_skip_depth = 1;
                        continue;
                    }
                    // MISS: emulate the bracket inline (unchanged from before
                    // the cache existed).
                    if *scale != 1.0 {
                        let m = *transform;
                        let s = Affine::scale_about(*scale, rect.center());
                        snapshot_correction = m * s * m.inverse();
                    }
                    if *alpha < 1.0 {
                        let corrected = root * snapshot_correction * *transform;
                        groups.push((Group::Layer(*alpha), corrected, *rect));
                        sink.push_layer(corrected, rect, *alpha);
                        snapshot_layer_pushed = true;
                    }
                }
                snapshot_depth += 1;
            }
            Command::PopSnapshot => {
                if snapshot_depth > 0 {
                    snapshot_depth -= 1;
                    if snapshot_depth == 0 {
                        if snapshot_layer_pushed {
                            groups.pop();
                            sink.pop_layer();
                            snapshot_layer_pushed = false;
                        }
                        snapshot_correction = Affine::IDENTITY;
                    }
                }
            }
        }
    }
}

/// The scene's per-corner radii as the `kurbo` shape vocabulary both sinks
/// build their concrete `RoundedRect` from — the one conversion site for the
/// scene→render radii hop (scene-layer purity keeps `kurbo::RoundedRectRadii`
/// out of `frust-scene`'s commands, `docs/ARCHITECTURE.md`).
fn radii_of(radii: frust_scene::CornerRadii) -> RoundedRectRadii {
    RoundedRectRadii::new(
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    )
}

/// Expands `path` into its dash segments — the sub-paths a dashed stroke is
/// actually made of.
///
/// Dashing happens HERE, in the shared command walk, rather than in either
/// sink: vello 0.9's `Scene::stroke` does honour a `kurbo::Stroke`'s dash
/// fields, but `vello_cpu`'s `set_stroke` does not, so flattening once at the
/// decode site is what keeps the two tiers pixel-comparable. `kurbo::dash` is
/// the same iterator vello itself uses, so the GPU tier's output is unchanged
/// by taking this route. Each dash becomes its own open sub-path, capped and
/// joined by the single `Stroke` the sink applies to the whole result.
fn dash_path(path: &BezPath, dash: DashPattern) -> BezPath {
    kurbo::dash(path.iter(), dash.phase, &[dash.on, dash.off]).collect()
}

impl SceneSink for vello::Scene {
    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect) {
        self.fill(style, transform, brush, None, rect);
    }

    fn fill_rounded_rect(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radii: RoundedRectRadii,
    ) {
        // `kurbo::RoundedRect` implements `Shape`, so it fills through the same
        // path as a plain rect.
        let rounded = RoundedRect::from_rect(*rect, radii);
        self.fill(style, transform, brush, None, &rounded);
    }

    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64) {
        let line = Line::new(p0, p1);
        self.stroke(&Stroke::new(width), transform, brush, None, &line);
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun) {
        // `FontHandle` wraps `peniko::FontData`, which is exactly what
        // `vello::Scene::draw_glyphs` accepts in 0.9.
        self.draw_glyphs(run.font.font())
            .font_size(run.font_size)
            .brush(&run.brush)
            .transform(run.transform)
            .draw(
                Fill::NonZero,
                run.glyphs.iter().map(|g| vello::Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                }),
            );
    }

    fn push_clip(&mut self, transform: Affine, rect: &Rect) {
        // A `push_layer` with a plain blend mode clips subsequent draws to the
        // shape (the clip is implicit in the layer). The clip is applied under
        // the command's own transform so it lines up with the (equally
        // transformed) draws it encloses — e.g. the desktop shell's HiDPI scale.
        self.push_layer(
            Fill::NonZero,
            peniko::BlendMode::default(),
            1.0,
            transform,
            rect,
        );
    }

    fn push_clip_rounded(&mut self, transform: Affine, rect: &Rect, radii: RoundedRectRadii) {
        // Same layer-as-clip mechanism as `push_clip` above, with a rounded
        // shape: `vello::Scene::push_layer` takes `clip: &impl Shape`, and
        // `kurbo::RoundedRect` is one. The concrete `RoundedRect` is built
        // HERE, inside the render crate — `frust-scene` carries only `Rect` +
        // its own `CornerRadii` (scene-layer purity, `docs/ARCHITECTURE.md`).
        let rounded = RoundedRect::from_rect(*rect, radii);
        self.push_layer(
            Fill::NonZero,
            peniko::BlendMode::default(),
            1.0,
            transform,
            &rounded,
        );
    }

    fn pop_clip(&mut self) {
        self.pop_layer();
    }

    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
        // vello's `Scene::draw_image` draws at the image's *natural* pixel
        // size under the given transform; map
        // natural -> dest by scaling then translating to `dest`'s origin,
        // composed under the incoming (widget-position) transform.
        let Some(image_transform) = natural_to_dest_transform(transform, data, dest) else {
            return;
        };
        vello::Scene::draw_image(self, data, image_transform);
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: &Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        vello::Scene::draw_blurred_rounded_rect(self, transform, *rect, color, radius, std_dev);
    }

    // These two impls rely on `vello::Scene`'s *inherent* `push_layer`/
    // `pop_layer` methods outranking this trait's identically-named methods in
    // Rust's method-resolution order (inherent methods are always preferred
    // over trait methods) — `self.push_layer(...)`/`vello::Scene::pop_layer(self)`
    // therefore call vello's own methods, not recurse into this `SceneSink`
    // impl. This is implicit, not enforced by the compiler: a `vello` version
    // bump that renames/removes either inherent method would silently make
    // these calls recurse (infinite loop) instead of failing to compile.
    // Re-verify this after any `vello` version bump; fully-qualify
    // (`<vello::Scene>::push_layer`) if resolution ever becomes ambiguous.
    fn push_layer(&mut self, transform: Affine, rect: &Rect, alpha: f32) {
        self.push_layer(
            Fill::NonZero,
            peniko::BlendMode::default(),
            alpha,
            transform,
            rect,
        );
    }

    fn pop_layer(&mut self) {
        vello::Scene::pop_layer(self);
    }

    fn clear_rect(&mut self, transform: Affine, rect: &Rect) {
        // The hole-punch: a layer whose composite is `Compose::DestOut` with an
        // OPAQUE fill erases the destination (color *and* alpha) exactly where
        // the source covers — `dst' = dst·(1−src.a)`, so a full-alpha fill
        // zeroes the rect while the fill's own antialiased coverage keeps the
        // erase pixel-exact at the edges. Deliberately NOT `Compose::Clear`:
        // vello 0.9 applies Clear at 16-px-tile granularity, ignoring the
        // layer's per-pixel clip coverage in boundary tiles, which bleeds the
        // punch up to 15 px past an unaligned rect edge (pixel-proven by
        // `tests/gpu_smoke.rs`'s unaligned-edge probe on Metal
        // and as a visible ring on cupid). DestOut weights the erase by the
        // source's own alpha, so unpainted pixels in a boundary tile are
        // untouched by construction.
        //
        // Fill the clip inside the layer so the erase has full geometric
        // coverage across `rect` (the color is irrelevant — only alpha drives
        // DestOut).
        //
        // `self.push_layer`/`vello::Scene::pop_layer` resolve to vello's own
        // inherent methods, not this `SceneSink` impl (see the note on the
        // `push_layer` impl above — re-verify after any vello bump).
        let blend = peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestOut);
        self.push_layer(Fill::NonZero, blend, 1.0, transform, rect);
        self.fill(
            Fill::NonZero,
            transform,
            &Brush::Solid(Color::BLACK),
            None,
            rect,
        );
        vello::Scene::pop_layer(self);
    }

    fn fill_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath) {
        self.fill(Fill::NonZero, transform, brush, None, path);
    }

    fn stroke_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath, width: f64) {
        self.stroke(&Stroke::new(width), transform, brush, None, path);
    }
}

/// Compose `transform` (the widget's own position/scale) with the affine that
/// maps an image's natural pixel rect `(0, 0, width, height)` onto `dest` —
/// what vello's "draws at natural size under the given transform" contract
/// (`vello::Scene::draw_image`'s doc comment) needs to land pixel-for-pixel
/// inside `dest`. `None` for a degenerate (zero-area) natural size.
fn natural_to_dest_transform(transform: Affine, data: &ImageData, dest: &Rect) -> Option<Affine> {
    let natural_w = data.width as f64;
    let natural_h = data.height as f64;
    if natural_w <= 0.0 || natural_h <= 0.0 {
        return None;
    }
    let scale_x = dest.width() / natural_w;
    let scale_y = dest.height() / natural_h;
    Some(
        transform
            * Affine::translate((dest.x0, dest.y0))
            * Affine::scale_non_uniform(scale_x, scale_y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::{CornerRadii, FontHandle, Glyph, GlyphRun, SceneBuilder};
    use kurbo::{PathEl, Shape};
    use peniko::color::palette::css::RED;
    use peniko::{Blob, FontData};

    /// Structural record of a sink call, comparable without touching GPU state.
    #[derive(Debug, PartialEq)]
    enum Event {
        FillRect {
            rect: Rect,
            transform: Affine,
        },
        RoundedRect {
            rect: Rect,
            radii: RoundedRectRadii,
            transform: Affine,
        },
        Line {
            p0: Point,
            p1: Point,
            width: f64,
            transform: Affine,
        },
        GlyphRun {
            font_size: f32,
            glyphs: usize,
            transform: Affine,
        },
        PushClip {
            rect: Rect,
            transform: Affine,
        },
        PushClipRounded {
            rect: Rect,
            radii: RoundedRectRadii,
            transform: Affine,
        },
        PopClip,
        Image {
            width: u32,
            height: u32,
            dest: Rect,
            transform: Affine,
        },
        BlurredRoundedRect {
            rect: Rect,
            color: Color,
            radius: f64,
            std_dev: f64,
            transform: Affine,
        },
        PushLayer {
            rect: Rect,
            alpha: f32,
            transform: Affine,
        },
        PopLayer,
        ClearRect {
            rect: Rect,
            transform: Affine,
        },
        FillPath {
            path: BezPath,
            transform: Affine,
        },
        StrokePath {
            path: BezPath,
            width: f64,
            transform: Affine,
        },
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Vec<Event>,
    }

    impl SceneSink for RecordingSink {
        fn fill_rect(&mut self, style: Fill, transform: Affine, _brush: &Brush, rect: &Rect) {
            assert_eq!(style, Fill::NonZero);
            self.events.push(Event::FillRect {
                rect: *rect,
                transform,
            });
        }

        fn fill_rounded_rect(
            &mut self,
            style: Fill,
            transform: Affine,
            _brush: &Brush,
            rect: &Rect,
            radii: RoundedRectRadii,
        ) {
            assert_eq!(style, Fill::NonZero);
            self.events.push(Event::RoundedRect {
                rect: *rect,
                radii,
                transform,
            });
        }

        fn stroke_line(
            &mut self,
            transform: Affine,
            _brush: &Brush,
            p0: Point,
            p1: Point,
            width: f64,
        ) {
            self.events.push(Event::Line {
                p0,
                p1,
                width,
                transform,
            });
        }

        fn draw_glyph_run(&mut self, run: &GlyphRun) {
            self.events.push(Event::GlyphRun {
                font_size: run.font_size,
                glyphs: run.glyphs.len(),
                transform: run.transform,
            });
        }

        fn push_clip(&mut self, transform: Affine, rect: &Rect) {
            self.events.push(Event::PushClip {
                rect: *rect,
                transform,
            });
        }

        fn push_clip_rounded(&mut self, transform: Affine, rect: &Rect, radii: RoundedRectRadii) {
            self.events.push(Event::PushClipRounded {
                rect: *rect,
                radii,
                transform,
            });
        }

        fn pop_clip(&mut self) {
            self.events.push(Event::PopClip);
        }

        fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
            self.events.push(Event::Image {
                width: data.width,
                height: data.height,
                dest: *dest,
                transform,
            });
        }

        fn draw_blurred_rounded_rect(
            &mut self,
            transform: Affine,
            rect: &Rect,
            color: Color,
            radius: f64,
            std_dev: f64,
        ) {
            self.events.push(Event::BlurredRoundedRect {
                rect: *rect,
                color,
                radius,
                std_dev,
                transform,
            });
        }

        fn push_layer(&mut self, transform: Affine, rect: &Rect, alpha: f32) {
            self.events.push(Event::PushLayer {
                rect: *rect,
                alpha,
                transform,
            });
        }

        fn pop_layer(&mut self) {
            self.events.push(Event::PopLayer);
        }

        fn clear_rect(&mut self, transform: Affine, rect: &Rect) {
            self.events.push(Event::ClearRect {
                rect: *rect,
                transform,
            });
        }

        fn fill_path(&mut self, transform: Affine, _brush: &Brush, path: &BezPath) {
            self.events.push(Event::FillPath {
                path: path.clone(),
                transform,
            });
        }

        fn stroke_path(&mut self, transform: Affine, _brush: &Brush, path: &BezPath, width: f64) {
            self.events.push(Event::StrokePath {
                path: path.clone(),
                width,
                transform,
            });
        }
    }

    fn empty_font() -> FontHandle {
        FontHandle::new(FontData::new(Blob::from(Vec::<u8>::new()), 0))
    }

    fn two_by_two_image() -> ImageData {
        image_of_size(2, 2)
    }

    /// An `ImageData` of a given natural pixel size — the `RecordingSink`
    /// records `width`/`height`, so distinct sizes let a test tell two override
    /// entries apart.
    fn image_of_size(w: u32, h: u32) -> ImageData {
        ImageData {
            data: Blob::from(vec![0u8; (w * h * 4) as usize]),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: w,
            height: h,
        }
    }

    #[test]
    fn fill_rect_maps_to_fill_call_with_nonzero_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((7.0, 3.0));
        builder.push_transform(translate);
        let rect = Rect::new(0.0, 0.0, 10.0, 20.0);
        builder.fill_rect(rect, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::FillRect {
                rect,
                transform: translate,
            }]
        );
    }

    #[test]
    fn glyph_run_maps_preserving_size_count_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let run = GlyphRun {
            font: empty_font(),
            font_size: 18.0,
            brush: Brush::Solid(RED),
            transform: Affine::IDENTITY,
            glyphs: vec![
                Glyph {
                    id: 1,
                    x: 0.0,
                    y: 0.0,
                },
                Glyph {
                    id: 2,
                    x: 9.0,
                    y: 0.0,
                },
                Glyph {
                    id: 3,
                    x: 18.0,
                    y: 0.0,
                },
            ],
        };
        builder.draw_glyph_run(run);

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::GlyphRun {
                font_size: 18.0,
                glyphs: 3,
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn clip_commands_map_to_push_and_pop_in_order() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip = Rect::new(0.0, 0.0, 5.0, 5.0);
        builder.push_clip(clip);
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Brush::Solid(RED));
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClip {
                    rect: clip,
                    transform: Affine::IDENTITY,
                },
                Event::FillRect {
                    rect: Rect::new(1.0, 1.0, 2.0, 2.0),
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
            ]
        );
    }

    #[test]
    fn rounded_clip_maps_to_rounded_push_and_the_shared_pop() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((6.0, 2.0));
        builder.push_transform(translate);
        let clip = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.push_clip_rounded(clip, 8.0);
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Brush::Solid(RED));
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClipRounded {
                    rect: clip,
                    radii: RoundedRectRadii::from(8.0),
                    transform: translate,
                },
                Event::FillRect {
                    rect: Rect::new(1.0, 1.0, 2.0, 2.0),
                    transform: translate,
                },
                Event::PopClip,
            ]
        );
    }

    #[test]
    fn rounded_clip_and_layer_nest_preserving_push_pop_order() {
        // Mirrors `clip_and_layer_nest_preserving_push_pop_order` for the
        // rounded variant: an alpha layer inside a rounded clip must nest and
        // unwind in the same order.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip_rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let layer_rect = Rect::new(10.0, 10.0, 50.0, 50.0);

        builder.push_clip_rounded(clip_rect, 12.0);
        builder.push_layer(layer_rect, 0.6);
        builder.pop_layer();
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClipRounded {
                    rect: clip_rect,
                    radii: RoundedRectRadii::from(12.0),
                    transform: Affine::IDENTITY,
                },
                Event::PushLayer {
                    rect: layer_rect,
                    alpha: 0.6,
                    transform: Affine::IDENTITY,
                },
                Event::PopLayer,
                Event::PopClip,
            ]
        );
    }

    #[test]
    fn clear_rect_hoist_re_pushes_a_rounded_clip_with_its_radius() {
        // The hole-punch hoist pops every open group, clears at root, then
        // re-pushes them. A rounded clip must come back rounded — re-pushing it
        // as a plain rect would square the corners of everything painted after
        // a nested slot's punch.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::new(20.0, 20.0, 60.0, 60.0);
        builder.push_clip_rounded(clip, 9.0);
        builder.clear_rect(hole);
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClipRounded {
                    rect: clip,
                    radii: RoundedRectRadii::from(9.0),
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
                Event::ClearRect {
                    rect: hole,
                    transform: Affine::IDENTITY,
                },
                Event::PushClipRounded {
                    rect: clip,
                    radii: RoundedRectRadii::from(9.0),
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
            ]
        );
    }

    /// The rounded clip must reach a real `vello::Scene` (via
    /// `kurbo::RoundedRect`, built inside this crate) without panicking — the
    /// `RecordingSink` checks above only verify the structural mapping.
    #[test]
    fn rounded_clip_encodes_into_a_real_vello_scene_without_panicking() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_transform(Affine::translate((1.0, 1.0)));
        builder.push_clip_rounded(Rect::new(0.0, 0.0, 64.0, 64.0), 16.0);
        builder.draw_image(&two_by_two_image(), Rect::new(0.0, 0.0, 64.0, 64.0));
        builder.pop_clip();

        let mut vello_scene = vello::Scene::new();
        encode_scene(&scene, &mut vello_scene);
    }

    #[test]
    fn rounded_rect_maps_to_rounded_fill_with_radius() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.fill_rounded_rect(rect, 3.0, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        // The uniform-radius regression: the pre-per-corner spelling must still
        // reach the sink as four equal corners, i.e. the identical shape it
        // encoded before per-corner radii existed.
        assert_eq!(
            sink.events,
            vec![Event::RoundedRect {
                rect,
                radii: RoundedRectRadii::new(3.0, 3.0, 3.0, 3.0),
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn rounded_rect_radii_map_each_corner_through_in_kurbo_order() {
        // The corner-order contract: `CornerRadii`'s clockwise-from-top-left
        // fields must land on the same fields of `kurbo::RoundedRectRadii` —
        // a transposition here would round the wrong corners with no other
        // signal than a visual one.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.fill_rounded_rect_radii(
            rect,
            CornerRadii::new(1.0, 2.0, 3.0, 4.0),
            Brush::Solid(RED),
        );

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::RoundedRect {
                rect,
                radii: RoundedRectRadii::new(1.0, 2.0, 3.0, 4.0),
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn rounded_clip_radii_map_through_and_survive_the_clear_rect_hoist() {
        // The per-corner half of the hoist contract: a punch pops every open
        // group and re-pushes it, and the re-push must carry the SAME per-corner
        // radii — squaring (or uniform-ising) them would reshape everything
        // painted after a nested slot's punch.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::new(20.0, 20.0, 60.0, 60.0);
        let radii = CornerRadii::new(9.0, 9.0, 0.0, 0.0);
        builder.push_clip_rounded_radii(clip, radii);
        builder.clear_rect(hole);
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        let expected_radii = RoundedRectRadii::new(9.0, 9.0, 0.0, 0.0);
        assert_eq!(
            sink.events,
            vec![
                Event::PushClipRounded {
                    rect: clip,
                    radii: expected_radii,
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
                Event::ClearRect {
                    rect: hole,
                    transform: Affine::IDENTITY,
                },
                Event::PushClipRounded {
                    rect: clip,
                    radii: expected_radii,
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
            ]
        );
    }

    #[test]
    fn per_corner_blurred_shadow_lowers_to_its_largest_corner() {
        // Neither tier has a per-corner blurred primitive, so the walk collapses
        // the four corners to the largest one before the sink sees them.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.draw_blurred_rounded_rect_radii(
            rect,
            CornerRadii::new(12.0, 12.0, 0.0, 0.0),
            2.5,
            RED,
        );

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::BlurredRoundedRect {
                rect,
                color: RED,
                radius: 12.0,
                std_dev: 2.5,
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn line_maps_to_stroke_with_endpoints_and_width() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let p0 = Point::new(1.0, 2.0);
        let p1 = Point::new(7.0, 9.0);
        builder.stroke_line(p0, p1, 1.5, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::Line {
                p0,
                p1,
                width: 1.5,
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn commands_encode_in_recorded_order() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);
        assert_eq!(sink.events.len(), 2);
    }

    #[test]
    fn shader_quad_miss_maps_to_placeholder_fill_rect_with_dest_and_transform() {
        // A `ShaderQuad` with no matching entry in the shader-override map (the
        // CPU tier, a failed compile, or no pre-pass) lowers to the placeholder
        // fill covering `dest` under the widget transform.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);
        let program = frust_scene::ShaderProgram::new("fn main() {}");
        let dest = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.draw_shader(&program, dest, 1.0);

        let mut sink = RecordingSink::default();
        // Empty map == miss for every program.
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::FillRect {
                rect: dest,
                transform: translate,
            }]
        );
    }

    #[test]
    fn shader_quad_lowers_to_placeholder_when_pre_pass_disabled() {
        // The `FRUST_NO_SHADER_EFFECTS` kill switch's downstream contract
        // (`renderer::run_shader_prepass`'s `disabled` short-circuit returns an
        // empty map with zero GPU work — see its own doc comment and
        // `renderer::tests::shader_prepass_is_a_full_no_op_when_disabled`):
        // feeding that empty map through the encode walk (the exact call
        // `SurfaceRenderer::encode`'s Gpu arm makes) must lower every
        // `Command::ShaderQuad` to the placeholder fill, identical to a
        // never-had-a-pre-pass caller.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);
        let program = frust_scene::ShaderProgram::new("fn main() {}");
        let dest = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.draw_shader(&program, dest, 1.0);

        // Drive `encode_into_with_shaders` with a `RecordingSink` rather than
        // a real `vello::Scene`, so the assertion needs no GPU device.
        let mut sink = RecordingSink::default();
        encode_into_with_shaders(&scene, &mut sink, &HashMap::new(), u32::MAX);

        assert_eq!(
            sink.events,
            vec![Event::FillRect {
                rect: dest,
                transform: translate,
            }]
        );
    }

    #[test]
    fn shader_quad_hit_maps_to_draw_image_with_dest_and_transform() {
        // A `ShaderQuad` whose program id is in the shader-override map lowers
        // to a `draw_image` of the registered override texture, scaled to fill
        // `dest` under the widget transform — the same mapping `Command::Image`
        // records.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);
        let program = frust_scene::ShaderProgram::new("fn main() {}");
        let dest = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.draw_shader(&program, dest, 1.0);

        // Stand in for the pre-pass's registered override handle (a 2x2 image),
        // keyed by (id, clamped physical size). Under a pure translate the
        // physical size equals dest's 40x40; `max_dim` is large enough not to
        // clamp, so the encode side recomputes the identical (id, 40, 40) key.
        let max_dim = 16384;
        let mut shader_images = HashMap::new();
        shader_images.insert((program.id(), 40, 40), two_by_two_image());

        let mut sink = RecordingSink::default();
        encode_into_with_shaders(&scene, &mut sink, &shader_images, max_dim);

        assert_eq!(
            sink.events,
            vec![Event::Image {
                width: 2,
                height: 2,
                dest,
                transform: translate,
            }]
        );
    }

    #[test]
    fn shader_quad_same_id_two_sizes_resolve_distinct_images() {
        // The two-size keying regression: ONE `ShaderProgram` (a single id)
        // drawn at two different physical sizes in the same frame must resolve
        // each quad to its own size's registered override, not collapse both to
        // one image. With an id-only map the second entry would overwrite the
        // first and both quads would draw the same texture.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let program = frust_scene::ShaderProgram::new("fn main() {}");
        // Identity transform, so each dest's physical size is its own extent.
        let dest_a = Rect::new(0.0, 0.0, 40.0, 40.0);
        let dest_b = Rect::new(0.0, 0.0, 80.0, 60.0);
        builder.draw_shader(&program, dest_a, 1.0);
        builder.draw_shader(&program, dest_b, 1.0);

        // Distinct natural sizes so the recorded events distinguish which
        // override each quad resolved to. `max_dim` large enough not to clamp.
        let max_dim = 16384;
        let mut shader_images = HashMap::new();
        shader_images.insert((program.id(), 40, 40), image_of_size(2, 2));
        shader_images.insert((program.id(), 80, 60), image_of_size(3, 3));

        let mut sink = RecordingSink::default();
        encode_into_with_shaders(&scene, &mut sink, &shader_images, max_dim);

        assert_eq!(
            sink.events,
            vec![
                Event::Image {
                    width: 2,
                    height: 2,
                    dest: dest_a,
                    transform: Affine::IDENTITY,
                },
                Event::Image {
                    width: 3,
                    height: 3,
                    dest: dest_b,
                    transform: Affine::IDENTITY,
                },
            ]
        );
    }

    #[test]
    fn image_maps_to_draw_image_with_dest_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((3.0, 4.0));
        builder.push_transform(translate);
        let data = two_by_two_image();
        let dest = Rect::new(0.0, 0.0, 40.0, 40.0);
        builder.draw_image(&data, dest);

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::Image {
                width: 2,
                height: 2,
                dest,
                transform: translate,
            }]
        );
    }

    #[test]
    fn image_identity_cloned_data_yields_same_blob_id() {
        // Verify that the image-identity probe keys on Blob::id() (the linebender
        // resource handle), not the pointer address. When the same ImageData is cloned
        // into two separate Command slots, Blob::id() must return the SAME value for both.
        //
        // This is critical for atlas reuse tracking: the old approach
        // (keying on &data.data as *const _ as u64) would see different pointer
        // addresses for allocations in different frames, breaking identity tracking.
        let data1 = two_by_two_image();
        let data2 = data1.clone(); // Clone into a separate slot

        // Verify that both clones yield the same Blob ID via Blob::id()
        let id1 = data1.data.id();
        let id2 = data2.data.id();

        assert_eq!(
            id1, id2,
            "Cloned ImageData must have the same Blob::id() for atlas reuse tracking"
        );

        // Also verify that the clones are distinct values (to show we're testing
        // the ID, not pointer equality)
        assert_ne!(
            &data1.data as *const _, &data2.data as *const _,
            "ImageData pointers must be different (clones in different slots)"
        );
    }

    #[test]
    fn blurred_rounded_rect_maps_to_shadow_call_with_fields_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((2.0, 3.0));
        builder.push_transform(translate);
        let rect = Rect::new(0.0, 0.0, 10.0, 8.0);
        builder.draw_blurred_rounded_rect(rect, 4.0, 2.5, RED);

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::BlurredRoundedRect {
                rect,
                color: RED,
                radius: 4.0,
                std_dev: 2.5,
                transform: translate,
            }]
        );
    }

    #[test]
    fn push_pop_layer_map_to_layer_calls_in_order_with_alpha_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let scale = Affine::scale(2.0);
        builder.push_transform(scale);
        let rect = Rect::new(0.0, 0.0, 5.0, 5.0);
        builder.push_layer(rect, 0.4);
        builder.pop_layer();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushLayer {
                    rect,
                    alpha: 0.4,
                    transform: scale,
                },
                Event::PopLayer,
            ]
        );
    }

    #[test]
    fn clear_rect_maps_to_clear_call_with_rect_and_transform() {
        // The platform-view hole-punch: a `ClearRect` command lowers to the
        // sink's `clear_rect` under the recorded (widget-position) transform.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((12.0, 8.0));
        builder.push_transform(translate);
        let rect = Rect::new(0.0, 0.0, 80.0, 60.0);
        builder.clear_rect(rect);

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::ClearRect {
                rect,
                transform: translate,
            }]
        );
    }

    #[test]
    fn mode_a_scene_encodes_no_clear_rect_even_with_a_slot_sized_region() {
        // The encode-level half of the Mode A contract: when the surface's RESOLVED
        // translucency is `false` (an opaque swapchain — including a
        // `TranslucentPreferred` request that fell back, see context.rs's
        // `forced_mismatch_translucent_request_resolves_not_translucent`), the
        // slot widget emits NO `ClearRect`, so the encode walk must produce no
        // `clear_rect` on the sink at all — nothing gets `DestOut`-zeroed and
        // the slot region simply keeps whatever painted there (Mode A: the
        // native view covers it from on top).
        //
        // Asserted at ENCODE level rather than at the recording-`PaintScene`
        // level deliberately: a recording-level test previously
        // missed a real defect in this exact punch path.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let backdrop = Rect::new(0.0, 0.0, 200.0, 200.0);
        let slot = Rect::new(50.0, 50.0, 150.0, 150.0);
        builder.fill_rect(backdrop, Brush::Solid(RED));
        // A Mode A slot paints nothing of its own; the scene carries only the
        // app's own content over the slot's region.
        builder.push_clip(slot);
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert!(
            !sink
                .events
                .iter()
                .any(|e| matches!(e, Event::ClearRect { .. })),
            "an opaque-resolved surface must encode no punch: {:?}",
            sink.events
        );
    }

    #[test]
    fn clear_rect_punches_beneath_a_backdrop_fill_preserving_order() {
        // The exact hole-punch shape: an opaque backdrop fill, then a slot clear over
        // part of it — the clear must encode AFTER the fill so it erases it.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let backdrop = Rect::new(0.0, 0.0, 200.0, 200.0);
        let hole = Rect::new(50.0, 50.0, 150.0, 150.0);
        builder.fill_rect(backdrop, Brush::Solid(RED));
        builder.clear_rect(hole);

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::FillRect {
                    rect: backdrop,
                    transform: Affine::IDENTITY,
                },
                Event::ClearRect {
                    rect: hole,
                    transform: Affine::IDENTITY,
                },
            ]
        );
    }

    #[test]
    fn clip_and_layer_nest_preserving_push_pop_order() {
        // push_clip -> push_layer -> pop_layer -> pop_clip: the encode step
        // must preserve command-stream order, mirroring the builder-level
        // nesting test in frust-scene.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip_rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let layer_rect = Rect::new(10.0, 10.0, 50.0, 50.0);

        builder.push_clip(clip_rect);
        builder.push_layer(layer_rect, 0.6);
        builder.pop_layer();
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClip {
                    rect: clip_rect,
                    transform: Affine::IDENTITY,
                },
                Event::PushLayer {
                    rect: layer_rect,
                    alpha: 0.6,
                    transform: Affine::IDENTITY,
                },
                Event::PopLayer,
                Event::PopClip,
            ]
        );
    }

    /// Encodes into a *real* `vello::Scene` (no GPU) to prove the new commands
    /// don't panic through the actual `SceneSink` impl — a round-trip
    /// check, not just the `RecordingSink` structural check.
    #[test]
    fn new_commands_encode_into_a_real_vello_scene_without_panicking() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((1.0, 1.0));
        builder.push_transform(translate);
        builder.draw_blurred_rounded_rect(Rect::new(0.0, 0.0, 20.0, 20.0), 4.0, 3.0, RED);
        builder.push_clip(Rect::new(0.0, 0.0, 50.0, 50.0));
        builder.push_layer(Rect::new(5.0, 5.0, 15.0, 15.0), 0.5);
        builder.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));
        builder.pop_layer();
        builder.pop_clip();
        // The hole-punch's `Compose::DestOut` layer must round-trip through the
        // real `vello::Scene` sink without panicking.
        builder.clear_rect(Rect::new(2.0, 2.0, 8.0, 8.0));

        let mut vello_scene = vello::Scene::new();
        encode_scene(&scene, &mut vello_scene);
    }

    fn triangle_path() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((5.0, 10.0));
        path.close_path();
        path
    }

    #[test]
    fn fill_path_maps_to_fill_path_call_with_path_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let translate = Affine::translate((2.0, 3.0));
        builder.push_transform(translate);
        let path = triangle_path();
        builder.fill_path(path.clone(), Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::FillPath {
                path,
                transform: translate,
            }]
        );
    }

    #[test]
    fn stroke_path_maps_to_stroke_path_call_with_width_and_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let path = triangle_path();
        builder.stroke_path(path.clone(), 2.5, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        // The undashed regression: a plain stroke reaches the sink with its
        // path byte-identical, never routed through the dash expansion.
        assert_eq!(
            sink.events,
            vec![Event::StrokePath {
                path,
                width: 2.5,
                transform: Affine::IDENTITY,
            }]
        );
    }

    /// The length of every dash in a dashed path — one entry per `MoveTo`-started
    /// sub-path, in emission order.
    ///
    /// Only `MoveTo`/`LineTo` appear, since every dashed path here is built from
    /// straight segments; anything else means the expansion changed shape and is
    /// a test-worthy surprise rather than something to silently measure.
    fn dash_lengths(path: &BezPath) -> Vec<f64> {
        let mut lengths: Vec<f64> = Vec::new();
        let mut last: Option<Point> = None;
        for element in path.elements() {
            match element {
                PathEl::MoveTo(p) => {
                    lengths.push(0.0);
                    last = Some(*p);
                }
                PathEl::LineTo(p) => {
                    let from = last.expect("a LineTo always follows a MoveTo");
                    *lengths.last_mut().expect("a dash is open") += (*p - from).hypot();
                    last = Some(*p);
                }
                other => panic!("expected only MoveTo/LineTo in a dashed path, got {other:?}"),
            }
        }
        lengths
    }

    /// `dash_lengths` sorted, so an assertion states which dashes exist without
    /// pinning kurbo's emission order — the dash iterator deliberately emits a
    /// sub-path's *first* dash last so it can merge with one wrapping through a
    /// `ClosePath`.
    fn sorted_dash_lengths(path: &BezPath) -> Vec<f64> {
        let mut lengths = dash_lengths(path);
        lengths.sort_by(f64::total_cmp);
        lengths
    }

    fn horizontal_line_path(length: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((length, 0.0));
        path
    }

    fn assert_close(got: &[f64], want: &[f64]) {
        assert_eq!(
            got.len(),
            want.len(),
            "dash count: got {got:?}, want {want:?}"
        );
        for (g, w) in got.iter().zip(want) {
            assert!(
                (g - w).abs() < 1e-6,
                "dash lengths differ: got {got:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn dashed_line_decodes_into_one_stroke_of_evenly_spaced_dashes() {
        // A 10-long line under a 2-on/2-off pattern: dashes at [0,2], [4,6],
        // [8,10] — three sub-paths of length 2, all inside ONE stroke call (the
        // dash expansion happens before the sink, so the sink still sees a
        // single stroke with a single width).
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.stroke_path_dashed(
            horizontal_line_path(10.0),
            2.0,
            DashPattern::new(2.0, 2.0),
            Brush::Solid(RED),
        );

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(sink.events.len(), 1);
        match &sink.events[0] {
            Event::StrokePath {
                path,
                width,
                transform,
            } => {
                assert_eq!(*width, 2.0);
                assert_eq!(*transform, Affine::IDENTITY);
                assert_close(&sorted_dash_lengths(path), &[2.0, 2.0, 2.0]);
            }
            other => panic!("expected StrokePath, got {other:?}"),
        }
    }

    #[test]
    fn dash_phase_shifts_the_pattern_along_the_path() {
        // The same 10-long line, offset one unit into the cycle: the run that
        // started at 0 is clipped to length 1 and everything else slides along,
        // still totalling half the path (2 on out of every 4).
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.stroke_path_dashed(
            horizontal_line_path(10.0),
            2.0,
            DashPattern::new(2.0, 2.0).with_phase(1.0),
            Brush::Solid(RED),
        );

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        match &sink.events[0] {
            Event::StrokePath { path, .. } => {
                assert_close(&sorted_dash_lengths(path), &[1.0, 2.0, 2.0]);
            }
            other => panic!("expected StrokePath, got {other:?}"),
        }
    }

    #[test]
    fn dashed_rect_perimeter_decodes_into_dashes_covering_half_of_it() {
        // A closed 10x10 rect (perimeter 40) under a 4-on/4-off pattern: five
        // dashes of length 4 — dashes wrap around the corners and through the
        // `ClosePath` seam, so this covers the closed-sub-path case a straight
        // line cannot.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.stroke_path_dashed(
            Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01),
            1.0,
            DashPattern::new(4.0, 4.0),
            Brush::Solid(RED),
        );

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        match &sink.events[0] {
            Event::StrokePath { path, .. } => {
                let lengths = sorted_dash_lengths(path);
                assert_close(&lengths, &[4.0, 4.0, 4.0, 4.0, 4.0]);
                assert!(
                    (lengths.iter().sum::<f64>() - 20.0).abs() < 1e-6,
                    "dashes must cover half the 40-unit perimeter, got {lengths:?}"
                );
            }
            other => panic!("expected StrokePath, got {other:?}"),
        }
    }

    #[test]
    fn degenerate_dash_patterns_stroke_the_path_solid() {
        // A zero-length gap (and a zero-length dash, and a non-finite one) has
        // no dashed interpretation, so the walk must stroke the original path
        // rather than hand a degenerate cycle to the dash iterator.
        for dash in [
            DashPattern::new(4.0, 0.0),
            DashPattern::new(0.0, 4.0),
            DashPattern::new(f64::NAN, 4.0),
        ] {
            let mut scene = Scene::new();
            let mut builder = SceneBuilder::new(&mut scene);
            let path = horizontal_line_path(10.0);
            builder.stroke_path_dashed(path.clone(), 2.0, dash, Brush::Solid(RED));

            let mut sink = RecordingSink::default();
            encode_into(&scene, &mut sink);

            assert_eq!(
                sink.events,
                vec![Event::StrokePath {
                    path: path.clone(),
                    width: 2.0,
                    transform: Affine::IDENTITY,
                }],
                "{dash:?} should stroke solid"
            );
        }
    }

    /// The per-corner and dashed commands must reach a real `vello::Scene`
    /// (through `kurbo::RoundedRect`/`kurbo::dash`, both built inside this
    /// crate) without panicking — the `RecordingSink` checks above only verify
    /// the structural mapping.
    #[test]
    fn per_corner_and_dashed_commands_encode_into_a_real_vello_scene_without_panicking() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let radii = CornerRadii::new(16.0, 0.0, 8.0, 4.0);
        builder.push_clip_rounded_radii(Rect::new(0.0, 0.0, 64.0, 64.0), radii);
        builder.fill_rounded_rect_radii(Rect::new(0.0, 0.0, 32.0, 32.0), radii, Brush::Solid(RED));
        builder.draw_blurred_rounded_rect_radii(Rect::new(0.0, 0.0, 32.0, 32.0), radii, 2.0, RED);
        builder.pop_clip();
        builder.stroke_path_dashed(
            triangle_path(),
            2.0,
            DashPattern::new(3.0, 2.0).with_phase(1.0),
            Brush::Solid(RED),
        );

        let mut vello_scene = vello::Scene::new();
        encode_scene(&scene, &mut vello_scene);
    }

    /// A widget can paint a stroked arc via `PaintScene`/`SceneBuilder` without
    /// any `frust-render` dependency, and it reaches a real `vello::Scene`
    /// without panicking.
    #[test]
    fn arc_path_fill_and_stroke_encode_into_a_real_vello_scene_without_panicking() {
        let path =
            frust_scene::arc_path(Point::new(10.0, 10.0), 8.0, 0.0, std::f64::consts::PI / 2.0);

        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_path(path.clone(), Brush::Solid(RED));
        builder.stroke_path(path, 2.0, Brush::Solid(RED));

        let mut vello_scene = vello::Scene::new();
        encode_scene(&scene, &mut vello_scene);
    }

    #[test]
    fn natural_to_dest_transform_scales_and_translates_under_identity() {
        // A 2x2 natural image into a 40x40 dest offset by (5, 6) under an
        // identity widget transform: uniform 20x scale (40 / 2), then
        // translated to dest's origin.
        let data = two_by_two_image();
        let dest = Rect::new(5.0, 6.0, 45.0, 46.0);
        let transform =
            natural_to_dest_transform(Affine::IDENTITY, &data, &dest).expect("non-degenerate");

        // The natural-space corners (0,0) and (2,2) must map exactly onto
        // dest's corners.
        assert_eq!(transform * Point::new(0.0, 0.0), Point::new(5.0, 6.0));
        assert_eq!(transform * Point::new(2.0, 2.0), Point::new(45.0, 46.0));
    }

    #[test]
    fn natural_to_dest_transform_composes_with_widget_transform() {
        let data = two_by_two_image();
        let dest = Rect::new(0.0, 0.0, 4.0, 4.0);
        let widget_transform = Affine::translate((10.0, 20.0));
        let transform =
            natural_to_dest_transform(widget_transform, &data, &dest).expect("non-degenerate");

        // Natural (0,0) maps to dest's origin (0,0), then the widget's own
        // translate is applied on top.
        assert_eq!(transform * Point::new(0.0, 0.0), Point::new(10.0, 20.0));
    }

    #[test]
    fn natural_to_dest_transform_is_none_for_zero_area_natural_size() {
        let data = ImageData {
            data: Blob::from(Vec::<u8>::new()),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: 0,
            height: 0,
        };
        let dest = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(natural_to_dest_transform(Affine::IDENTITY, &data, &dest).is_none());
    }

    /// The `Command::PushSnapshot` MISS path (acceptance criterion): a
    /// snapshot body with `alpha` 0.5 and `scale` 0.9 must lower to the exact
    /// same op sequence as the equivalent `push_transform` +
    /// `push_layer` recording the contract (`Command::PushSnapshot`'s doc
    /// comment, point 5) describes.
    #[test]
    fn snapshot_body_lowers_like_the_equivalent_push_transform_and_push_layer() {
        let outer = Affine::translate((5.0, 7.0));
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let body_rect = Rect::new(2.0, 2.0, 10.0, 10.0);

        let mut snapshot_scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut snapshot_scene);
        builder.push_transform(outer);
        builder.push_snapshot(1, rect, 0.5, 0.9);
        builder.fill_rect(body_rect, Brush::Solid(RED));
        builder.pop_snapshot();

        let mut equivalent_scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut equivalent_scene);
        builder.push_transform(outer);
        builder.push_transform(Affine::scale_about(0.9, rect.center()));
        builder.push_layer(rect, 0.5);
        builder.fill_rect(body_rect, Brush::Solid(RED));
        builder.pop_layer();
        builder.pop_transform();

        let mut snapshot_sink = RecordingSink::default();
        encode_into(&snapshot_scene, &mut snapshot_sink);
        let mut equivalent_sink = RecordingSink::default();
        encode_into(&equivalent_scene, &mut equivalent_sink);

        assert_eq!(snapshot_sink.events.len(), 3);
        assert_events_close(&snapshot_sink.events, &equivalent_sink.events);
    }

    /// The plain-pass-through half of the same contract: `alpha: 1.0` and
    /// `scale: 1.0` (both no-ops) must lower the body with NO emulated
    /// layer/transform correction at all — identical to the body recorded
    /// with no bracket around it.
    #[test]
    fn snapshot_body_with_no_op_alpha_and_scale_lowers_like_the_bare_body() {
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let body_rect = Rect::new(2.0, 2.0, 10.0, 10.0);

        let mut snapshot_scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut snapshot_scene);
        builder.push_snapshot(1, rect, 1.0, 1.0);
        builder.fill_rect(body_rect, Brush::Solid(RED));
        builder.pop_snapshot();

        let mut bare_scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut bare_scene);
        builder.fill_rect(body_rect, Brush::Solid(RED));

        let mut snapshot_sink = RecordingSink::default();
        encode_into(&snapshot_scene, &mut snapshot_sink);
        let mut bare_sink = RecordingSink::default();
        encode_into(&bare_scene, &mut bare_sink);

        assert_eq!(snapshot_sink.events, bare_sink.events);
    }

    /// An unbalanced `PopSnapshot` (more pops than pushes) must not disturb
    /// the walk beyond the matched pair — the same policy `Command::PopLayer`
    /// follows. `SceneBuilder::pop_snapshot` itself already refuses to record
    /// an unmatched pop (see `frust-scene`'s own test), so this exercises the
    /// walk's own depth guard the only way a public recording can: an extra
    /// `pop_snapshot()` call after the bracket already closed.
    #[test]
    fn extra_pop_snapshot_after_the_bracket_closed_is_ignored() {
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_snapshot(1, rect, 0.5, 1.0);
        builder.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));
        builder.pop_snapshot();
        // Already balanced — recorded as a no-op, so this changes nothing.
        builder.pop_snapshot();
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_into(&scene, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushLayer {
                    rect,
                    alpha: 0.5,
                    transform: Affine::IDENTITY,
                },
                Event::FillRect {
                    rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                    transform: Affine::IDENTITY,
                },
                Event::PopLayer,
                Event::FillRect {
                    rect: Rect::new(1.0, 1.0, 2.0, 2.0),
                    transform: Affine::IDENTITY,
                },
            ]
        );
    }

    /// The set of holes a test names, spelled once.
    fn holes_of<const N: usize>(keys: [u64; N]) -> HashSet<u64> {
        HashSet::from(keys)
    }

    /// Encode the whole `scene` with `holes`, the way the render path's
    /// pre-segment does.
    fn encode_holed(scene: &Scene, holes: &HashSet<u64>) -> Vec<Event> {
        let mut sink = RecordingSink::default();
        encode_range_with_overrides(
            scene,
            0..scene.commands().len(),
            &mut sink,
            &HashMap::new(),
            u32::MAX,
            holes,
        );
        sink.events
    }

    #[test]
    fn a_holed_bracket_contributes_no_ops_while_its_siblings_encode() {
        // The hole acceptance criterion: a bracket whose key is composited
        // emits NOTHING — no layer, no transform, no image, none of the
        // body's own ops — while everything recorded around it encodes
        // untouched.
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let before = Rect::new(0.0, 0.0, 1.0, 1.0);
        let after = Rect::new(1.0, 1.0, 2.0, 2.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(before, Brush::Solid(RED));
        builder.push_snapshot(7, rect, 1.0, 1.0);
        builder.fill_rect(Rect::new(2.0, 2.0, 10.0, 10.0), Brush::Solid(RED));
        builder.pop_snapshot();
        builder.fill_rect(after, Brush::Solid(RED));

        assert_eq!(
            encode_holed(&scene, &holes_of([7])),
            vec![
                Event::FillRect {
                    rect: before,
                    transform: Affine::IDENTITY,
                },
                Event::FillRect {
                    rect: after,
                    transform: Affine::IDENTITY,
                },
            ]
        );
    }

    #[test]
    fn a_holed_brackets_alpha_and_scale_never_reach_the_sink() {
        // The compositor applies both on its own quad, so emitting either
        // here would double them. An `alpha`/`scale` that the MISS path would
        // have emulated with a layer and a transform correction must leave
        // the recording completely empty.
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_transform(Affine::translate((5.0, 7.0)));
        builder.push_snapshot(3, rect, 0.4, 0.5);
        builder.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Brush::Solid(RED));
        builder.pop_snapshot();

        assert_eq!(encode_holed(&scene, &holes_of([3])), vec![]);
    }

    #[test]
    fn a_holed_bracket_skips_a_nested_bracket_that_is_also_holed() {
        // Nesting acceptance criterion: only the OUTERMOST bracket is ever
        // honoured. A holed outer bracket skips everything up to its own
        // matching `PopSnapshot`, including a nested `PushSnapshot` whose key
        // is *also* a hole, and including body content recorded after the
        // inner bracket closed.
        let outer_rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let inner_rect = Rect::new(2.0, 2.0, 10.0, 10.0);
        let tail = Rect::new(7.0, 7.0, 8.0, 8.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_snapshot(1, outer_rect, 1.0, 1.0);
        builder.push_snapshot(2, inner_rect, 1.0, 1.0);
        builder.fill_rect(Rect::new(3.0, 3.0, 4.0, 4.0), Brush::Solid(RED));
        builder.pop_snapshot();
        builder.fill_rect(Rect::new(5.0, 5.0, 6.0, 6.0), Brush::Solid(RED));
        builder.pop_snapshot();
        builder.fill_rect(tail, Brush::Solid(RED));

        assert_eq!(
            encode_holed(&scene, &holes_of([1, 2])),
            vec![Event::FillRect {
                rect: tail,
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn a_bracket_that_is_not_holed_still_lowers_inline() {
        // The other half of the same walk: a bracket the cache could not take
        // (uncacheable body, kill switch, failed rasterization) is absent
        // from `holes` and keeps the untouched MISS emulation, in whichever
        // segment it falls.
        let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
        let body_rect = Rect::new(2.0, 2.0, 10.0, 10.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.push_snapshot(1, rect, 0.5, 1.0);
        builder.fill_rect(body_rect, Brush::Solid(RED));
        builder.pop_snapshot();

        // A hole for some OTHER bracket must not touch this one.
        assert_eq!(
            encode_holed(&scene, &holes_of([99])),
            vec![
                Event::PushLayer {
                    rect,
                    alpha: 0.5,
                    transform: Affine::IDENTITY,
                },
                Event::FillRect {
                    rect: body_rect,
                    transform: Affine::IDENTITY,
                },
                Event::PopLayer,
            ]
        );
    }

    #[test]
    fn encode_range_with_overrides_encodes_exactly_the_given_range() {
        let rects = [
            Rect::new(0.0, 0.0, 1.0, 1.0),
            Rect::new(1.0, 1.0, 2.0, 2.0),
            Rect::new(2.0, 2.0, 3.0, 3.0),
        ];
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        for rect in rects {
            builder.fill_rect(rect, Brush::Solid(RED));
        }

        let mut sink = RecordingSink::default();
        encode_range_with_overrides(
            &scene,
            1..3,
            &mut sink,
            &HashMap::new(),
            u32::MAX,
            &HashSet::new(),
        );

        assert_eq!(
            sink.events,
            vec![
                Event::FillRect {
                    rect: rects[1],
                    transform: Affine::IDENTITY,
                },
                Event::FillRect {
                    rect: rects[2],
                    transform: Affine::IDENTITY,
                },
            ]
        );
    }

    #[test]
    fn encode_range_with_overrides_hoists_a_clear_rect_within_the_range_only() {
        // The hoist pops the groups the SEGMENT opened, not the ones an
        // earlier segment did — a clip pushed before the range simply is not
        // this pass's to pop, and the punch is bounded by the in-range clip
        // alone.
        let outside = Rect::new(0.0, 0.0, 10.0, 10.0);
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::new(20.0, 20.0, 60.0, 60.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(outside, Brush::Solid(RED));
        builder.push_clip(clip);
        builder.clear_rect(hole);
        builder.pop_clip();

        let mut sink = RecordingSink::default();
        encode_range_with_overrides(
            &scene,
            1..scene.commands().len(),
            &mut sink,
            &HashMap::new(),
            u32::MAX,
            &HashSet::new(),
        );

        assert_eq!(
            sink.events,
            vec![
                Event::PushClip {
                    rect: clip,
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
                Event::ClearRect {
                    rect: hole,
                    transform: Affine::IDENTITY,
                },
                Event::PushClip {
                    rect: clip,
                    transform: Affine::IDENTITY,
                },
                Event::PopClip,
            ]
        );
    }

    #[test]
    fn encode_range_with_overrides_clamps_a_range_past_the_end() {
        let rect = Rect::new(0.0, 0.0, 1.0, 1.0);
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        builder.fill_rect(rect, Brush::Solid(RED));

        let mut sink = RecordingSink::default();
        encode_range_with_overrides(
            &scene,
            0..999,
            &mut sink,
            &HashMap::new(),
            u32::MAX,
            &HashSet::new(),
        );

        assert_eq!(
            sink.events,
            vec![Event::FillRect {
                rect,
                transform: Affine::IDENTITY,
            }]
        );
    }

    #[test]
    fn encode_commands_into_premultiplies_root_onto_every_command_transform() {
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let cmd_transform = Affine::translate((3.0, 4.0));
        builder.push_transform(cmd_transform);
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        builder.fill_rect(rect, Brush::Solid(RED));

        let root = Affine::scale(2.0);
        let mut sink = RecordingSink::default();
        encode_commands_into(scene.commands(), root, &mut sink);

        assert_eq!(
            sink.events,
            vec![Event::FillRect {
                rect,
                transform: root * cmd_transform,
            }]
        );
    }

    #[test]
    fn encode_commands_into_clear_rect_hoist_respects_a_non_identity_root() {
        // The hole-punch hoist must still clear the RIGHT rect — in root's
        // own coordinate space — when the slice is encoded under a
        // non-identity root, e.g. the snapshot cache's rasterization
        // pre-pass encoding a body whose own commands carry no knowledge of
        // the root the pre-pass rasterizes at.
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        let hole = Rect::new(20.0, 20.0, 60.0, 60.0);
        builder.push_clip(clip);
        builder.clear_rect(hole);
        builder.pop_clip();

        let root = Affine::translate((5.0, 5.0));
        let mut sink = RecordingSink::default();
        encode_commands_into(scene.commands(), root, &mut sink);

        assert_eq!(
            sink.events,
            vec![
                Event::PushClip {
                    rect: clip,
                    transform: root,
                },
                Event::PopClip,
                Event::ClearRect {
                    rect: root.transform_rect_bbox(hole),
                    transform: Affine::IDENTITY,
                },
                Event::PushClip {
                    rect: clip,
                    transform: root,
                },
                Event::PopClip,
            ]
        );
    }

    /// Compares two recorded event sequences allowing a tiny tolerance on
    /// every `Affine`/`Rect` coefficient — the snapshot MISS path derives its
    /// emulated transform via a `transform * scale * transform.inverse()`
    /// conjugation (see `encode_commands`'s `snapshot_correction`), which is
    /// mathematically but not always bit-for-bit identical to the
    /// same value composed the other way `push_transform`/`push_layer`
    /// recording took.
    fn assert_events_close(got: &[Event], want: &[Event]) {
        assert_eq!(got.len(), want.len(), "got {got:?}, want {want:?}");
        for (g, w) in got.iter().zip(want) {
            match (g, w) {
                (
                    Event::PushLayer {
                        rect: gr,
                        alpha: ga,
                        transform: gt,
                    },
                    Event::PushLayer {
                        rect: wr,
                        alpha: wa,
                        transform: wt,
                    },
                ) => {
                    assert_eq!(gr, wr);
                    assert_eq!(ga, wa);
                    assert_affine_close(*gt, *wt);
                }
                (
                    Event::FillRect {
                        rect: gr,
                        transform: gt,
                    },
                    Event::FillRect {
                        rect: wr,
                        transform: wt,
                    },
                ) => {
                    assert_eq!(gr, wr);
                    assert_affine_close(*gt, *wt);
                }
                (Event::PopLayer, Event::PopLayer) => {}
                other => panic!("event shape mismatch: {other:?}"),
            }
        }
    }

    fn assert_affine_close(got: Affine, want: Affine) {
        for (g, w) in got.as_coeffs().iter().zip(want.as_coeffs()) {
            assert!(
                (g - w).abs() < 1e-9,
                "affine coefficients differ: got {got:?}, want {want:?}"
            );
        }
    }
}
