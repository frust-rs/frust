//! `BenchView`: the escape-hatch `View`/`Widget` pair that paints the bench's
//! three scenes directly through `frust-scene`'s `PaintScene` vocabulary — the
//! same hand-rolled pattern as `examples/shadertoy/src/shader_view.rs`.
//!
//! THROWAWAY LAB (see `lib.rs`'s top-of-file note). This widget reaches below
//! the `frust` facade on purpose: it drives `draw_image` (scene B's composited
//! cached-layer quads), `draw_shader` (mode C's `render_to_texture`-style live
//! layer, resolved by `frust-render`'s `shader_effects` texture-override
//! pre-pass), and the raw fill/glyph/path primitives (scene A's vector
//! content) so the spike can compare their GPU cost on device. It is NOT
//! shipped API surface.
//!
//! Every mode requests a frame every paint (a perpetual animator), so
//! `FRUST_TRACE`/`FRUST_TRACE_RAW` perf lines flow at the display refresh rate
//! and the measurement compares steady-state per-frame cost.

use frust::FrameTime;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust_scene::ShaderProgram;
use frust_text::{TextContext, TextLayout, TextStyle};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::{Brush, Color};

use crate::scene::{self, BACKGROUND, Mode};

/// Wrap the spinner's time origin at this many seconds to dodge `f32` drift on
/// a long soak — imperceptible in practice (an hour).
const TIME_WRAP_SECS: f64 = 3600.0;

/// The trivial WGSL fragment for mode C's live relayer layer: a time-varying
/// opaque fill (alpha = 1.0, the v1 `ShaderProgram` contract). Cheap on
/// purpose — mode C isolates the *dispatch* overhead of re-rendering one layer
/// into its dedicated `Rgba8Unorm` target each frame, not fragment ALU cost.
const RELAYER_WGSL: &str = r#"
@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {
    let uv = in.position.xy / frust_u.resolution;
    let t = 0.5 + 0.5 * sin(frust_u.time + uv.x * 6.28318);
    return vec4<f32>(0.2 + 0.3 * uv.y, 0.3 * t, 0.6, 1.0);
}
"#;

/// The view descriptor: which scene to paint and the device-pixel ratio the
/// offscreen textures are sized against.
pub struct BenchView {
    pub mode: Mode,
    pub dpr: f64,
}

/// Construct a [`BenchView`].
pub fn bench_view(mode: Mode, dpr: f64) -> BenchView {
    BenchView { mode, dpr }
}

/// Content built once (per size) at layout time: the shaped labels plus the
/// composite-scene band geometry, textures, and total texture memory.
struct BuiltContent {
    plan: scene::ScenePlan,
    /// Shaped labels paired with their widget-relative baseline origins.
    labels: Vec<(TextLayout, Point)>,
    bands: Vec<Rect>,
    images: Vec<peniko::ImageData>,
    memory_bytes: u64,
}

/// The retained widget.
pub struct BenchViewWidget {
    mode: Mode,
    dpr: f64,
    size: Size,
    /// The live relayer shader program (created once — the cache-once contract).
    program: ShaderProgram,
    /// Frame time first observed by this instance; the spinner's phase is
    /// always relative to this, never an absolute clock (see `FrameTime`).
    start: Option<FrameTime>,
    /// Built lazily on the first layout / whenever `size` changes.
    content: Option<BuiltContent>,
}

impl<State: 'static> View<State> for BenchView {
    type Element = BenchViewWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BenchViewWidget {
        BenchViewWidget {
            mode: self.mode,
            dpr: self.dpr,
            size: Size::ZERO,
            // Created exactly once here (never in `rebuild`/`paint`) per
            // `ShaderProgram::new`'s cache-once contract.
            program: ShaderProgram::new(RELAYER_WGSL),
            start: None,
            content: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BenchViewWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.mode != self.mode {
            element.mode = self.mode;
            flags |= ChangeFlags::PAINT;
        }
        if prev.dpr != self.dpr {
            element.dpr = self.dpr;
            // A dpr change invalidates the physical-resolution textures.
            element.content = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl BenchViewWidget {
    /// (Re)build the shaped labels + composite textures for the current size.
    fn build_content(&mut self, tcx: &mut TextContext) {
        let plan = scene::plan(self.size);

        let labels = plan
            .labels
            .iter()
            .map(|spec| {
                let style = TextStyle::new(spec.size, spec.color);
                let layout = tcx.layout(&spec.text, &style, None);
                (layout, spec.origin)
            })
            .collect();

        let bands = scene::layer_bands(self.size);
        let images = bands
            .iter()
            .enumerate()
            .map(|(i, band)| scene::build_layer_image(*band, self.dpr, (i as u8).wrapping_mul(53)))
            .collect();
        let memory_bytes = scene::layer_memory_bytes(self.size, self.dpr);

        self.content = Some(BuiltContent {
            plan,
            labels,
            bands,
            images,
            memory_bytes,
        });
    }

    /// Total offscreen texture memory of the composite scenes (0 until content
    /// is built) — surfaced for the on-screen HUD / SPIKE.md table.
    pub fn texture_memory_bytes(&self) -> u64 {
        self.content.as_ref().map_or(0, |c| c.memory_bytes)
    }

    /// Paint scene A: the full vector content, re-rasterized every frame.
    fn paint_vector(&self, origin: Point, content: &BuiltContent, scene: &mut dyn PaintScene) {
        let t = Vec2::new(origin.x, origin.y);
        for r in &content.plan.rects {
            scene.fill_rounded_rect(
                Point::new(r.rect.x0 + t.x, r.rect.y0 + t.y),
                Size::new(r.rect.width(), r.rect.height()),
                r.radius,
                r.color,
            );
        }
        for d in &content.plan.dots {
            scene.fill_rounded_rect(
                Point::new(d.center.x - d.radius + t.x, d.center.y - d.radius + t.y),
                Size::new(d.radius * 2.0, d.radius * 2.0),
                d.radius,
                d.color,
            );
        }
        for icon in &content.plan.icons {
            scene.fill_path(origin, &icon.path, &Brush::Solid(icon.color));
        }
        for (layout, rel) in &content.labels {
            let at = Point::new(origin.x + rel.x, origin.y + rel.y);
            for run in layout.to_scene_runs(at) {
                scene.draw_glyph_run(run);
            }
        }
    }

    /// Paint the composite scenes (B and B+1relayer). In mode C the first band
    /// is a live `ShaderQuad` (re-rendered every frame) while the rest are
    /// static image quads; in mode B every band is a static image quad.
    fn paint_composite(
        &self,
        origin: Point,
        content: &BuiltContent,
        time: f32,
        relayer: bool,
        scene: &mut dyn PaintScene,
    ) {
        for (i, band) in content.bands.iter().enumerate() {
            let dest = Rect::new(
                origin.x + band.x0,
                origin.y + band.y0,
                origin.x + band.x1,
                origin.y + band.y1,
            );
            if relayer && i == 0 {
                // The one live layer: forces `shader_effects` to re-render this
                // band's dedicated Rgba8Unorm target this frame (the
                // render_to_texture-style per-layer dispatch the spike isolates).
                scene.draw_shader(&self.program, dest, time);
            } else {
                // A cached layer: the same ImageData handle every frame, so
                // vello reuses its uploaded texture (zero re-render).
                scene.draw_image(&content.images[i], dest);
            }
        }
        self.paint_spinner(origin, time, scene);
    }

    /// A small live vector spinner (8 orbiting dots) so the composite scenes
    /// are never trivially static — genuine per-frame vector work alongside the
    /// composited quads.
    fn paint_spinner(&self, origin: Point, time: f32, scene: &mut dyn PaintScene) {
        let center = Point::new(
            origin.x + self.size.width - 44.0,
            origin.y + self.size.height - 44.0,
        );
        let r = 16.0;
        let dot_r = 3.0;
        let base = time as f64 * 2.0;
        for i in 0..8 {
            let ang = base + i as f64 * std::f64::consts::FRAC_PI_4;
            let px = center.x + r * ang.cos();
            let py = center.y + r * ang.sin();
            // Trailing-fade alpha around the ring.
            let phase = ((i as f64) / 8.0 + time as f64 * 0.5).fract();
            let alpha = (0.25 + 0.75 * phase) as f32;
            let color = Color::new([0.55, 0.68, 0.95, alpha]);
            scene.fill_rounded_rect(
                Point::new(px - dot_r, py - dot_r),
                Size::new(dot_r * 2.0, dot_r * 2.0),
                dot_r,
                color,
            );
        }
    }
}

impl Widget for BenchViewWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let new_size = bc.max();
        let dirty = self.content.is_none() || new_size != self.size;
        self.size = new_size;
        if dirty {
            let tcx = ctx.text_context::<TextContext>();
            self.build_content(tcx);
        }
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();

        // Dark full-screen background (baseline overdraw), painted in every mode.
        scene.fill_rect(origin, size, BACKGROUND);

        let now = ctx.frame_time();
        let start = *self.start.get_or_insert(now);
        let elapsed = now.saturating_sub(start).as_secs_f64() % TIME_WRAP_SECS;
        let time = elapsed as f32;

        if let Some(content) = &self.content {
            match self.mode {
                Mode::Vector => self.paint_vector(origin, content, scene),
                Mode::Composite => self.paint_composite(origin, content, time, false, scene),
                Mode::CompositeRelayer => self.paint_composite(origin, content, time, true, scene),
            }
        }

        // Perpetual animator: keep asking for frames in every mode so the perf
        // trace flows at refresh rate for the steady-state comparison.
        ctx.request_frame();
    }
}
