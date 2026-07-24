//! Experimental CPU render tier: translation of a [`frust_scene::Scene`]
//! into a [`vello_cpu`] `RenderContext`, rasterized headless into a
//! premultiplied-RGBA8 [`Pixmap`](vello_cpu::Pixmap).
//!
//! Compiled only behind the non-default `cpu-tier` feature (see
//! `frust-render/Cargo.toml` and PLAN.md D4c). The GPU (vello 0.9) path is
//! unaffected when the feature is off — this whole module, and every `vello_cpu`
//! type it names, then vanishes from the build.
//!
//! # Why this exists
//!
//! The tier probe ([`crate::tier::select_render_tier`]) picks [`RenderTier::Cpu`](crate::RenderTier::Cpu)
//! when the adapter lacks the GPU tier's required downlevel flags (the iOS
//! Simulator's missing `INDIRECT_EXECUTION` is the canonical case) *and* this
//! feature is compiled in. The device still exists — basic render pipelines
//! (the blit) work, only vello's compute path doesn't — so the produced
//! [`Pixmap`](vello_cpu::Pixmap) is uploaded into the same intermediate
//! `Rgba8Unorm` target the GPU path blits from, and the acquire/blit/present
//! tail in [`crate::renderer`] is shared verbatim.
//!
//! # Command coverage vs. the GPU path
//!
//! The translation reuses the crate-private [`SceneSink`](crate::convert::SceneSink)
//! seam that `convert::encode_into` already walks for the GPU path, so every
//! `Command` variant is routed through one `match` shared with vello — there is
//! no second command-coverage list to keep in sync. Where `vello_cpu` 0.0.9's
//! API cannot reproduce a GPU-path effect exactly, the downgrade is documented
//! inline and collected in the list below for the phase 6e visual review:
//!
//! - **Image *brushes*** (a `Brush::Image` handed to a fill, as opposed to the
//!   dedicated `Command::Image`) are painted transparent — no shipping widget
//!   emits one, so this is a theoretical gap, not a visible one.
//! - **`Command::Image`** is best-effort: the peniko `ImageData` is converted to
//!   a `vello_cpu` image paint and sampled into `dest`. Pixel fidelity is
//!   untested against the GPU path (no headless image golden yet) — see phase 6e.
//! - Everything else (solid/gradient fills, rounded rects, lines, arbitrary
//!   fill/stroke paths, clips, opacity layers, blurred-rounded-rect shadows,
//!   glyph runs) maps onto a direct `vello_cpu` equivalent.

use frust_scene::{GlyphRun, Scene};
use kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Shape, Stroke};
use peniko::{Brush, Color, Fill, ImageData};

use crate::convert::{SceneSink, encode_into};

/// Flattening tolerance (logical px) for the rounded-rect / rect / line shapes
/// we hand `vello_cpu` as `BezPath`s. `vello_cpu`'s own fast paths take
/// concrete `kurbo` shapes, but its clip/opacity-layer and rounded-fill entry
/// points want a `BezPath`, so we pre-flatten. A tolerance well below a pixel
/// keeps corners crisp without exploding the segment count.
const FLATTEN_TOLERANCE: f64 = 0.1;

/// The transparent fallback paint used where `vello_cpu` has no equivalent for
/// a scene brush (currently only `Brush::Image` handed to a fill; see the
/// module docs' downgrade note).
const FALLBACK_PAINT: Color = Color::new([0.0, 0.0, 0.0, 0.0]);

/// A [`vello_cpu`]-backed render tier: owns a reusable `RenderContext`, its
/// `Resources`, and the target `Pixmap` so per-frame allocation is avoided
/// (mirroring the GPU path's reused `vello::Scene`).
///
/// Dimensions are `u16` (vello_cpu's native size type); larger surfaces are
/// clamped to `u16::MAX` — far beyond any real window, so the clamp is a
/// safety net, not an expected path.
pub(crate) struct CpuTierRenderer {
    ctx: vello_cpu::RenderContext,
    resources: vello_cpu::Resources,
    pixmap: vello_cpu::Pixmap,
    width: u16,
    height: u16,
}

/// Clamps a `(u32, u32)` surface size to the `(u16, u16)` vello_cpu wants,
/// with a floor of 1 (a zero-sized `RenderContext`/`Pixmap` is invalid).
fn clamp_dims(width: u32, height: u32) -> (u16, u16) {
    let w = width.clamp(1, u16::MAX as u32) as u16;
    let h = height.clamp(1, u16::MAX as u32) as u16;
    (w, h)
}

impl CpuTierRenderer {
    /// Creates a renderer sized for `width`x`height` logical device pixels.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let (w, h) = clamp_dims(width, height);
        Self {
            ctx: vello_cpu::RenderContext::new(w, h),
            resources: vello_cpu::Resources::new(),
            pixmap: vello_cpu::Pixmap::new(w, h),
            width: w,
            height: h,
        }
    }

    /// Resizes the render target if `width`x`height` differs from the current
    /// size. `RenderContext` has no in-place resize, so it is rebuilt; the
    /// `Pixmap` resizes its buffer in place.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = clamp_dims(width, height);
        if w == self.width && h == self.height {
            return;
        }
        self.ctx = vello_cpu::RenderContext::new(w, h);
        self.pixmap.resize(w, h);
        self.width = w;
        self.height = h;
    }

    /// Renders `scene` (clearing to `base_color` first, mirroring vello's
    /// `RenderParams::base_color`) into the internal pixmap, resizing to
    /// `width`x`height` first if needed, and returns the premultiplied-RGBA8
    /// bytes (`width * height * 4`, R,G,B,A order — directly uploadable into an
    /// `Rgba8Unorm` texture).
    pub(crate) fn render(
        &mut self,
        scene: &Scene,
        base_color: Color,
        width: u32,
        height: u32,
    ) -> &[u8] {
        self.resize(width, height);
        self.ctx.reset();

        // Clear to the base color: vello's GPU path does this via
        // `RenderParams::base_color`; on the CPU we fill the whole target first.
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(base_color);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, self.width as f64, self.height as f64));

        {
            let mut sink = CpuSink {
                ctx: &mut self.ctx,
                resources: &mut self.resources,
            };
            // Reuse the *same* command walk the GPU path uses (`convert`), so
            // command coverage stays single-sourced (PLAN.md D4c).
            encode_into(scene, &mut sink);
        }

        self.ctx.flush();
        self.ctx
            .render_to_pixmap(&mut self.resources, &mut self.pixmap);
        self.pixmap.data_as_u8_slice()
    }

    /// The current target width (device px). Test-only today — the render path
    /// uploads with the swapchain's own dimensions, not this — but kept for the
    /// resize regression test.
    #[cfg(test)]
    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    /// The current target height (device px). Test-only (see [`width`](Self::width)).
    #[cfg(test)]
    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    /// Samples one pixel (premultiplied RGBA8) — test-only, for the headless
    /// pixel-assertion suite below.
    #[cfg(test)]
    pub(crate) fn sample(&self, x: u16, y: u16) -> vello_cpu::color::PremulRgba8 {
        self.pixmap.sample(x, y)
    }
}

/// A [`SceneSink`] that replays each scene command into a `vello_cpu`
/// `RenderContext`. `vello_cpu` is a state machine (set paint/transform/stroke,
/// then draw), unlike vello's per-call parameters, so each method sets the
/// context state fresh before drawing.
struct CpuSink<'a> {
    ctx: &'a mut vello_cpu::RenderContext,
    resources: &'a mut vello_cpu::Resources,
}

/// Sets the context's current paint from a scene [`Brush`]. `Solid`/`Gradient`
/// map onto `vello_cpu`'s `PaintType`; an image brush handed to a fill falls
/// back to transparent (see the module docs' downgrade note).
fn set_brush(ctx: &mut vello_cpu::RenderContext, brush: &Brush) {
    match brush {
        Brush::Solid(color) => ctx.set_paint(*color),
        Brush::Gradient(gradient) => {
            ctx.set_paint(vello_cpu::PaintType::Gradient(gradient.clone()))
        }
        Brush::Image(_) => ctx.set_paint(FALLBACK_PAINT),
    }
}

impl SceneSink for CpuSink<'_> {
    fn fill_rect(&mut self, style: Fill, transform: Affine, brush: &Brush, rect: &Rect) {
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(style);
        set_brush(self.ctx, brush);
        self.ctx.fill_rect(rect);
    }

    fn fill_rounded_rect(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: &Brush,
        rect: &Rect,
        radius: f64,
    ) {
        let path = RoundedRect::from_rect(*rect, radius).to_path(FLATTEN_TOLERANCE);
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(style);
        set_brush(self.ctx, brush);
        self.ctx.fill_path(&path);
    }

    fn stroke_line(&mut self, transform: Affine, brush: &Brush, p0: Point, p1: Point, width: f64) {
        let path = Line::new(p0, p1).to_path(FLATTEN_TOLERANCE);
        self.ctx.set_transform(transform);
        set_brush(self.ctx, brush);
        self.ctx.set_stroke(Stroke::new(width));
        self.ctx.stroke_path(&path);
    }

    fn draw_glyph_run(&mut self, run: &GlyphRun) {
        self.ctx.set_transform(run.transform);
        set_brush(self.ctx, &run.brush);
        // vello_cpu's glyph run reads the context's current paint/transform;
        // it fills glyph outlines directly (no atlas cache here).
        self.ctx
            .glyph_run(self.resources, run.font.font())
            .font_size(run.font_size)
            .hint(false)
            .fill_glyphs(run.glyphs.iter().map(|g| vello_cpu::Glyph {
                id: g.id,
                x: g.x,
                y: g.y,
            }));
    }

    fn push_clip(&mut self, transform: Affine, rect: &Rect) {
        self.ctx.set_transform(transform);
        self.ctx.push_clip_layer(&rect.to_path(FLATTEN_TOLERANCE));
    }

    fn pop_clip(&mut self) {
        self.ctx.pop_layer();
    }

    fn draw_image(&mut self, transform: Affine, data: &ImageData, dest: &Rect) {
        let natural_w = data.width as f64;
        let natural_h = data.height as f64;
        if natural_w <= 0.0 || natural_h <= 0.0 {
            return;
        }
        let source = vello_cpu::ImageSource::from_peniko_image_data(data);
        let image = vello_cpu::Image {
            image: source,
            sampler: peniko::ImageSampler::default(),
        };
        // The widget transform maps the local dest rect onto the screen;
        // `set_transform` applies it to the fill shape. The *paint* transform
        // maps the image's natural pixel space onto that same local dest, and
        // vello_cpu samples the image through `transform * paint_transform`.
        let paint_transform = Affine::translate((dest.x0, dest.y0))
            * Affine::scale_non_uniform(dest.width() / natural_w, dest.height() / natural_h);
        self.ctx.set_transform(transform);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_paint(vello_cpu::PaintType::Image(image));
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
        self.ctx
            .fill_blurred_rounded_rect(rect, radius as f32, std_dev as f32);
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
        // The hole-punch: a clip layer whose composite is `Compose::DestOut`
        // with an opaque fill erases the region on pop, weighted by the fill's
        // own coverage (see `convert.rs`'s vello impl for why NOT
        // `Compose::Clear` — vello's GPU pipeline applies Clear at 16-px-tile
        // granularity past unaligned edges; DestOut is pixel-exact on both
        // tiers, kept identical here for cross-tier parity).
        self.ctx.set_transform(transform);
        let clip = rect.to_path(FLATTEN_TOLERANCE);
        self.ctx.push_layer(
            Some(&clip),
            Some(peniko::BlendMode::new(
                peniko::Mix::Normal,
                peniko::Compose::DestOut,
            )),
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
        set_brush(self.ctx, brush);
        self.ctx.fill_path(path);
    }

    fn stroke_path(&mut self, transform: Affine, brush: &Brush, path: &BezPath, width: f64) {
        self.ctx.set_transform(transform);
        set_brush(self.ctx, brush);
        self.ctx.set_stroke(Stroke::new(width));
        self.ctx.stroke_path(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::SceneBuilder;
    use peniko::color::palette::css::{BLUE, GREEN, RED};

    /// Opaque-red premultiplied bytes are just (255, 0, 0, 255).
    fn is_opaque_red(p: vello_cpu::color::PremulRgba8) -> bool {
        p.r > 200 && p.g < 40 && p.b < 40 && p.a > 200
    }

    #[test]
    fn solid_fill_rect_produces_expected_pixels() {
        // A 20x20 target, cleared to white, with a red rect over the top-left
        // 10x10 quadrant. Assert a pixel inside the rect is red and one outside
        // it is (the white) background — the CPU tier runs fully headless, so we
        // can assert real rasterized pixels (task 06 acceptance criterion).
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Brush::Solid(RED));
        }

        let mut renderer = CpuTierRenderer::new(20, 20);
        renderer.render(&scene, Color::WHITE, 20, 20);

        // Inside the rect.
        assert!(
            is_opaque_red(renderer.sample(5, 5)),
            "pixel inside the red rect should be red, got {:?}",
            renderer.sample(5, 5)
        );
        // Outside the rect: the white clear color.
        let bg = renderer.sample(15, 15);
        assert!(
            bg.r > 200 && bg.g > 200 && bg.b > 200,
            "pixel outside the rect should be the white background, got {bg:?}"
        );
    }

    #[test]
    fn clip_restricts_fill_to_the_clip_rect() {
        // Clip to the left half, then fill the whole target blue. Only the left
        // half should be blue; the right half stays the (green) clear color.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.push_clip(Rect::new(0.0, 0.0, 10.0, 20.0));
            builder.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Brush::Solid(BLUE));
            builder.pop_clip();
        }

        let mut renderer = CpuTierRenderer::new(20, 20);
        renderer.render(&scene, GREEN, 20, 20);

        let left = renderer.sample(5, 10);
        assert!(
            left.b > 200 && left.r < 40,
            "pixel inside the clip should be blue, got {left:?}"
        );
        let right = renderer.sample(15, 10);
        assert!(
            right.g > 120 && right.b < 40,
            "pixel outside the clip should be the green clear color, got {right:?}"
        );
    }

    #[test]
    fn base_color_clears_the_whole_target() {
        // An empty scene renders as a flat field of the base color.
        let scene = Scene::new();
        let mut renderer = CpuTierRenderer::new(8, 8);
        let bytes = renderer.render(&scene, RED, 8, 8);
        assert_eq!(bytes.len(), 8 * 8 * 4);
        assert!(is_opaque_red(renderer.sample(4, 4)));
    }

    #[test]
    fn resize_changes_target_dimensions_and_buffer_length() {
        let scene = Scene::new();
        let mut renderer = CpuTierRenderer::new(4, 4);
        assert_eq!(renderer.render(&scene, Color::BLACK, 4, 4).len(), 4 * 4 * 4);
        let bytes = renderer.render(&scene, Color::BLACK, 16, 8);
        assert_eq!(bytes.len(), 16 * 8 * 4);
        assert_eq!(renderer.width(), 16);
        assert_eq!(renderer.height(), 8);
    }

    #[test]
    fn zero_dimensions_are_clamped_to_one() {
        let (w, h) = clamp_dims(0, 0);
        assert_eq!((w, h), (1, 1));
    }

    #[test]
    fn oversized_dimensions_are_clamped_to_u16_max() {
        let (w, h) = clamp_dims(100_000, 70_000);
        assert_eq!((w, h), (u16::MAX, u16::MAX));
    }

    #[test]
    fn every_command_variant_rasterizes_without_panicking() {
        // Exercises the full SceneSink surface (mirroring convert.rs's
        // "encode into a real vello::Scene" smoke test): rounded rect, line,
        // path fill/stroke, blurred shadow, nested clip + opacity layer.
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            builder.draw_blurred_rounded_rect(
                Rect::new(1.0, 1.0, 18.0, 18.0),
                3.0,
                2.0,
                Color::BLACK,
            );
            builder.fill_rounded_rect(Rect::new(2.0, 2.0, 16.0, 16.0), 4.0, Brush::Solid(RED));
            builder.stroke_line(
                Point::new(0.0, 0.0),
                Point::new(20.0, 20.0),
                1.5,
                Brush::Solid(BLUE),
            );
            builder.push_clip(Rect::new(0.0, 0.0, 15.0, 15.0));
            builder.push_layer(Rect::new(2.0, 2.0, 12.0, 12.0), 0.5);
            let mut path = BezPath::new();
            path.move_to((3.0, 3.0));
            path.line_to((10.0, 3.0));
            path.line_to((6.0, 11.0));
            path.close_path();
            builder.fill_path(path.clone(), Brush::Solid(GREEN));
            builder.stroke_path(path, 1.0, Brush::Solid(RED));
            builder.pop_layer();
            builder.pop_clip();
        }

        let mut renderer = CpuTierRenderer::new(20, 20);
        let bytes = renderer.render(&scene, Color::WHITE, 20, 20);
        assert_eq!(bytes.len(), 20 * 20 * 4);
    }

    /// The GPU-upload leg of the CPU tier (the counterpart of `tests/gpu_smoke.rs`
    /// for the vello path): rasterize a scene into the pixmap, upload it into an
    /// `Rgba8Unorm` texture with `write_texture` exactly as `SurfaceRenderer`'s
    /// CPU branch does, and read it back. Needs a real device, so it is
    /// `#[ignore]`d and run manually on hardware.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render --features cpu-tier -- --ignored`"]
    fn cpu_pixmap_uploads_into_an_rgba8_texture() {
        pollster::block_on(async {
            const SIZE: u32 = 20;
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust cpu_tier upload smoke"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frust cpu_tier upload target"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[],
            });

            let mut scene = Scene::new();
            {
                let mut builder = SceneBuilder::new(&mut scene);
                builder.fill_rect(
                    Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
                    Brush::Solid(RED),
                );
            }
            let mut renderer = CpuTierRenderer::new(SIZE, SIZE);
            let pixels = renderer.render(&scene, Color::WHITE, SIZE, SIZE);

            queue.write_texture(
                texture.as_image_copy(),
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * SIZE),
                    rows_per_image: Some(SIZE),
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );

            let bytes_per_row = SIZE * 4;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust cpu_tier readback"),
                size: (bytes_per_row * SIZE) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
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

            let data = slice.get_mapped_range();
            // Centre pixel came from a red fill: red-dominant after the round-trip.
            let centre = ((SIZE / 2) * bytes_per_row + (SIZE / 2) * 4) as usize;
            let (r, g, b) = (data[centre], data[centre + 1], data[centre + 2]);
            assert!(
                r > g && r > b,
                "centre pixel should be red-dominant after upload, got rgb=({r},{g},{b})"
            );
        });
    }
}
